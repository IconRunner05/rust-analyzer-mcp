use anyhow::Result;
use log::{debug, error, info, warn};
use serde_json::json;
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter};

use crate::{
    cli::DEFAULT_IDLE_TIMEOUT,
    lsp::RustAnalyzerClient,
    protocol::mcp::{MCPError, MCPRequest, MCPResponse},
    settings::Settings,
    uri,
};

pub struct RustAnalyzerMCPServer {
    /// One rust-analyzer per workspace that has been asked about, kept by its root.
    ///
    /// The workspace used to be a single setting, which made it shared mutable state: two callers
    /// of one server -- an agent and the subagents it fans out, say -- would each point it at
    /// their own project, and the loser of that race got answered from the winner's index. Not
    /// wrongly enough to notice: a file the loaded workspace does not contain comes back empty,
    /// which reads as dead code.
    ///
    /// A call that names its workspace is therefore answered by that workspace's own
    /// rust-analyzer, and cannot be retargeted by anyone else. Each one costs what a
    /// rust-analyzer costs, which is why nothing spawns one implicitly: only a call that asks for
    /// a workspace by name, or `set_workspace`, adds to this -- and see
    /// [`Self::sweep_workspaces`] for what takes one away.
    pub(super) clients: std::collections::HashMap<PathBuf, Held>,
    /// The workspace a call that does not name one is answered by.
    pub(super) workspace_root: PathBuf,
    /// What rust-analyzer is asked to run with, for every rust-analyzer this server starts.
    pub(super) settings: Settings,
    /// How long a workspace may go unasked about before its rust-analyzer is shut down, or
    /// `None` to keep every one for the server's life.
    pub(super) idle_timeout: Option<Duration>,
}

/// A workspace's rust-analyzer, and when a call last asked about that workspace.
pub(super) struct Held {
    pub(super) client: RustAnalyzerClient,
    pub(super) last_asked: Instant,
}

impl Held {
    fn new(client: RustAnalyzerClient) -> Self {
        Self {
            client,
            last_asked: Instant::now(),
        }
    }
}

/// Why [`RustAnalyzerMCPServer::sweep_workspaces`] lets a workspace go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Release {
    /// Its rust-analyzer has exited, or is on its way out.
    Exited,
    /// Its root is gone from disk, so nothing can name it again.
    LeftTheDisk,
    /// Nothing has asked about it for longer than the idle timeout.
    Idle,
}

impl Default for RustAnalyzerMCPServer {
    fn default() -> Self {
        Self::new()
    }
}

impl RustAnalyzerMCPServer {
    pub fn new() -> Self {
        Self::with_workspace(std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
    }

    pub fn with_workspace(workspace_root: PathBuf) -> Self {
        Self {
            clients: std::collections::HashMap::new(),
            workspace_root: uri::absolute(&workspace_root),
            settings: Settings::default(),
            idle_timeout: Some(DEFAULT_IDLE_TIMEOUT),
        }
    }

    /// Shuts down a workspace's rust-analyzer once nothing has asked about it for `idle_timeout`;
    /// `None` keeps every one for the server's life.
    pub fn with_idle_timeout(mut self, idle_timeout: Option<Duration>) -> Self {
        self.idle_timeout = idle_timeout;
        self
    }

    /// The rust-analyzer answering for the workspace this call is about, if it has been started.
    pub(super) fn client(&mut self) -> Option<&mut RustAnalyzerClient> {
        self.clients
            .get_mut(&self.workspace_root)
            .map(|held| &mut held.client)
    }

    /// Points the rest of this call at `workspace_path`, leaving the default alone.
    ///
    /// A workspace named by a call is checked the way `set_workspace` checks one: a directory
    /// with no `Cargo.toml` is refused rather than started in, because rust-analyzer there loads
    /// nothing and answers every question emptily.
    pub(super) fn select_workspace(&mut self, workspace_path: Option<&str>) -> Result<()> {
        let Some(workspace_path) = workspace_path else {
            return Ok(());
        };

        let named = uri::uri_to_path(workspace_path).unwrap_or_else(|| workspace_path.into());
        self.workspace_root = manifest_directory(&uri::absolute(&named))
            .map_err(|e| anyhow::anyhow!("{e} That is what this call's workspace_path names."))?;
        Ok(())
    }

    /// Forgets every workspace but the one in use, shutting its rust-analyzer down.
    ///
    /// A workspace named per call is kept, on the grounds that it will be named again -- until
    /// [`Self::sweep_workspaces`] lets it go. Moving the default with `set_workspace`
    /// says the opposite of all of them at once, and an idle rust-analyzer is a gigabyte or so of
    /// index nobody is asking about.
    pub(super) async fn drop_other_workspaces(&mut self) {
        let elsewhere: Vec<PathBuf> = self
            .clients
            .keys()
            .filter(|root| **root != self.workspace_root)
            .cloned()
            .collect();

        for root in elsewhere {
            if let Some(mut held) = self.clients.remove(&root) {
                info!("Shutting down rust-analyzer for {}", root.display());
                // It kills the process either way, so a failed handshake is nothing to report.
                let _ = held.client.shutdown().await;
            }
        }
    }

    /// Runs rust-analyzer with `settings`, whatever workspace it is pointed at.
    pub fn with_settings(mut self, settings: Settings) -> Self {
        self.settings = settings;
        self
    }

    /// Lets go of every workspace whose rust-analyzer has exited, whose root has left the disk, or
    /// that nothing has asked about for [`Self::idle_timeout`] -- shutting each rust-analyzer down
    /// and waiting on it, so none is left `<defunct>`.
    ///
    /// Run before every tool call and on a timer from [`Self::run`]: a server nobody is calling
    /// is exactly the one holding indexes nobody needs, and it would never sweep if only a call
    /// could make it.
    ///
    /// A workspace let go is not refused afterwards. The next call naming it starts a fresh
    /// rust-analyzer, whose cold index the read tools already report as an error rather than as
    /// an empty answer.
    ///
    /// With `spare` set, the workspace this call is about keeps its rust-analyzer unless that has
    /// exited: [`Self::ensure_client_started`] would only start it again on the next line. A root
    /// counts as gone from disk only on a definite `Ok(false)`; a stat that fails for any other
    /// reason is no proof of anything.
    async fn sweep_workspaces(&mut self, now: Instant, spare: Option<PathBuf>) {
        let released: Vec<(PathBuf, Release)> = self
            .clients
            .iter()
            .filter_map(|(root, held)| {
                let why = if held.client.is_gone() {
                    Release::Exited
                } else if spare.as_ref() == Some(root) {
                    return None;
                } else if matches!(root.try_exists(), Ok(false)) {
                    Release::LeftTheDisk
                } else if self.idle_timeout.is_some_and(|timeout| {
                    now.saturating_duration_since(held.last_asked) >= timeout
                }) {
                    Release::Idle
                } else {
                    return None;
                };
                Some((root.clone(), why))
            })
            .collect();

        for (root, why) in released {
            let Some(mut held) = self.clients.remove(&root) else {
                continue;
            };
            let client = &mut held.client;

            match why {
                Release::Exited => {
                    match client.exit_status() {
                        Some(status) => {
                            warn!("rust-analyzer for {} exited ({status})", root.display())
                        }
                        None => warn!("rust-analyzer for {} closed its connection", root.display()),
                    }
                    // Nothing to hand shake with: the reader finishing means stdout closed, which
                    // rust-analyzer may be doing on its way out rather than after. force_kill
                    // kills the one that has not exited yet and waits on both.
                    client.force_kill().await;
                }
                Release::LeftTheDisk | Release::Idle => {
                    info!(
                        "Shutting down rust-analyzer for {} ({})",
                        root.display(),
                        match why {
                            Release::LeftTheDisk => "its root has left the disk",
                            _ => "idle",
                        }
                    );
                    // It kills the process either way, so a failed handshake is nothing to report.
                    let _ = client.shutdown().await;
                }
            }
        }
    }

    pub(super) async fn ensure_client_started(&mut self) -> Result<()> {
        let root = self.workspace_root.clone();
        self.sweep_workspaces(Instant::now(), Some(root.clone()))
            .await;

        if !self.clients.contains_key(&root) {
            let mut client = RustAnalyzerClient::new(root.clone(), self.settings.to_json());
            client.start().await?;
            self.clients.insert(root.clone(), Held::new(client));
        }
        if let Some(held) = self.clients.get_mut(&root) {
            held.last_asked = Instant::now();
        }
        Ok(())
    }

    pub(super) async fn open_document_if_needed(&mut self, file_path: &str) -> Result<String> {
        let path = self.resolve_path(file_path);
        let uri = uri::path_to_uri(&path)?;
        let content = tokio::fs::read_to_string(&path)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to read file {}: {}", path.display(), e))?;

        let Some(client) = self.client() else {
            return Err(anyhow::anyhow!("Client not initialized"));
        };

        client.open_document(&uri, &content).await?;
        Ok(uri)
    }

    /// Brings rust-analyzer up to date with every document it has been told about.
    ///
    /// One document at a time is enough for a question about one file, but a rename reaches
    /// across the workspace and is worked out from whatever rust-analyzer holds for each file it
    /// touches. Anything stale in there comes back as an edit to a line that has moved.
    pub(super) async fn refresh_open_documents(&mut self) -> Result<()> {
        let Some(client) = self.client() else {
            return Err(anyhow::anyhow!("Client not initialized"));
        };

        for uri in client.open_document_uris().await {
            let Some(path) = uri::uri_to_path(&uri) else {
                continue;
            };

            match tokio::fs::read_to_string(&path).await {
                Ok(content) => client.open_document(&uri, &content).await?,
                // Gone from disk, which a rename of a module's file does to it. Left open, it
                // would go on existing as far as rust-analyzer is concerned.
                Err(_) => client.close_document(&uri).await?,
            }
        }

        Ok(())
    }

    /// The file a tool call's `file_path` argument names.
    ///
    /// Clients spell that argument every way they have one to hand: relative to the workspace
    /// root, absolute, or as the `file:` URI our own results are full of.
    pub(super) fn resolve_path(&self, file_path: &str) -> PathBuf {
        let path = match uri::uri_to_path(file_path) {
            Some(path) => path,
            // Joining an absolute path onto the root yields that path, so this covers both.
            None => self.workspace_root.join(file_path),
        };

        uri::absolute(&path)
    }

    /// Runs the server until its stdin reaches EOF or a shutdown signal arrives.
    ///
    /// Installs process-wide signal handlers that remain in effect after this returns. Reads
    /// stdin through [`tokio::io::stdin`], whose parked blocking read cannot be cancelled: after
    /// a signal-triggered exit the caller must not wait for the runtime to shut down on its own.
    /// See this crate's `main.rs`, which uses [`tokio::runtime::Runtime::shutdown_background`].
    pub async fn run(&mut self) -> Result<()> {
        info!("Starting rust-analyzer MCP server");

        let stdin = tokio::io::stdin();
        let stdout = tokio::io::stdout();
        // Lines rather than read_line(): next_line() is cancellation-safe, which the sweep arm
        // below needs -- it wins the select between requests, mid-line or not.
        let mut lines = BufReader::new(stdin).lines();
        let mut writer = BufWriter::new(stdout);
        let mut sweep = tokio::time::interval(sweep_period(self.idle_timeout));
        sweep.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        // Created once, up front: the streams buffer signals delivered while a request is being
        // handled, and installing a handler permanently replaces the default disposition, so
        // every signal must be consumed here to have an effect.
        let mut shutdown = ShutdownSignal::new()?;
        // How many shutdown signals were consumed; the second one escalates the cleanup below.
        let mut signals_seen = 0u32;
        // The first fatal I/O error, reported only after the cleanup ran.
        let mut result = Ok(());

        loop {
            let line = tokio::select! {
                // Biased with the signal arm first: a signal that latched while a request was
                // being handled must win over lines already buffered on stdin, so that no new
                // request is accepted after shutdown was requested.
                biased;
                _ = shutdown.recv() => {
                    info!("Received shutdown signal");
                    signals_seen += 1;
                    break;
                }
                read = lines.next_line() => match read {
                    Ok(Some(line)) => line,
                    Ok(None) => break, // EOF
                    Err(e) => {
                        error!("Error reading from stdin: {}", e);
                        result = Err(e.into());
                        break;
                    }
                },
                _ = sweep.tick() => {
                    self.sweep_workspaces(Instant::now(), None).await;
                    continue;
                }
            };

            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            let Ok(request) = serde_json::from_str::<MCPRequest>(line) else {
                debug!("Failed to parse request: {}", line);
                continue;
            };

            // A message without an `id` is a JSON-RPC notification, which must never be answered,
            // not even with an error: a client that receives a response it did not ask for treats
            // it as a protocol violation and closes the transport. `notifications/initialized` is
            // part of every MCP handshake, so this used to break every spec-compliant client.
            if request.id.is_none() {
                debug!("Ignoring notification: {}", request.method);
                continue;
            }

            debug!("Received request: {}", request.method);
            // A shutdown signal must not wait for the request to finish: a tool call that
            // cold-starts rust-analyzer can run for minutes.
            let response = tokio::select! {
                biased;
                _ = shutdown.recv() => {
                    info!("Received shutdown signal");
                    signals_seen += 1;
                    break;
                }
                response = self.handle_request(request) => response,
            };
            // Break on errors instead of returning so rust-analyzer still gets cleaned up.
            let response_json = match serde_json::to_string(&response) {
                Ok(json) => json,
                Err(e) => {
                    error!("Failed to serialize response: {}", e);
                    result = Err(e.into());
                    break;
                }
            };
            // Also raced against the signals: if the host stops reading stdout, a response that
            // fills the pipe would otherwise block here forever with the signals unpolled.
            let written = async {
                writer.write_all(response_json.as_bytes()).await?;
                writer.write_all(b"\n").await?;
                writer.flush().await
            };
            let written = tokio::select! {
                biased;
                _ = shutdown.recv() => {
                    info!("Received shutdown signal");
                    signals_seen += 1;
                    break;
                }
                written = written => written,
            };
            if let Err(e) = written {
                error!("Error writing to stdout: {}", e);
                result = Err(e.into());
                break;
            }
        }

        // Cleanup. client.shutdown() bounds its own graceful handshake and always ends up
        // killing the process, so this cannot stall. A second signal — counting the one that may
        // have triggered the exit — skips the handshake and kills rust-analyzer immediately.
        info!("Shutting down");
        for Held { client, .. } in self.clients.values_mut() {
            let graceful = {
                let shutting_down = client.shutdown();
                tokio::pin!(shutting_down);
                loop {
                    tokio::select! {
                        biased;
                        _ = shutdown.recv() => {
                            signals_seen += 1;
                            if signals_seen >= 2 {
                                info!("Received another shutdown signal, killing rust-analyzer");
                                break false;
                            }
                        }
                        res = &mut shutting_down => {
                            let _ = res;
                            break true;
                        }
                    }
                }
            };
            if !graceful {
                client.force_kill().await;
            }
        }

        result
    }

    async fn handle_request(&mut self, request: MCPRequest) -> MCPResponse {
        match request.method.as_str() {
            "initialize" => MCPResponse::Success {
                jsonrpc: "2.0".to_string(),
                id: request.id,
                result: json!({
                    "protocolVersion": "2024-11-05",
                    "serverInfo": {
                        "name": "rust-analyzer-mcp",
                        "version": env!("CARGO_PKG_VERSION")
                    },
                    "capabilities": {
                        "tools": {}
                    }
                }),
            },
            // A liveness check the client may send at any point, including before `initialize`.
            // Its result is empty; what matters is that it comes back at all.
            "ping" => MCPResponse::Success {
                jsonrpc: "2.0".to_string(),
                id: request.id,
                result: json!({}),
            },
            "tools/list" => MCPResponse::Success {
                jsonrpc: "2.0".to_string(),
                id: request.id,
                result: json!({
                    "tools": super::tools::get_tools()
                }),
            },
            "tools/call" => {
                let Some(params) = request.params else {
                    return MCPResponse::Error {
                        jsonrpc: "2.0".to_string(),
                        id: request.id,
                        error: MCPError {
                            code: -32602,
                            message: "Invalid params".to_string(),
                            data: None,
                        },
                    };
                };

                let Some(tool_name) = params["name"].as_str() else {
                    return MCPResponse::Error {
                        jsonrpc: "2.0".to_string(),
                        id: request.id,
                        error: MCPError {
                            code: -32602,
                            message: "Missing tool name".to_string(),
                            data: None,
                        },
                    };
                };

                let args = params
                    .get("arguments")
                    .cloned()
                    .unwrap_or_else(|| json!({}));

                match super::handlers::handle_tool_call(self, tool_name, args).await {
                    Ok(result) => MCPResponse::Success {
                        jsonrpc: "2.0".to_string(),
                        id: request.id,
                        result: serde_json::to_value(result).unwrap(),
                    },
                    Err(e) => {
                        error!("Tool call error: {}", e);
                        MCPResponse::Error {
                            jsonrpc: "2.0".to_string(),
                            id: request.id,
                            error: MCPError {
                                code: -1,
                                message: e.to_string(),
                                data: None,
                            },
                        }
                    }
                }
            }
            _ => MCPResponse::Error {
                jsonrpc: "2.0".to_string(),
                id: request.id,
                error: MCPError {
                    code: -32601,
                    message: format!("Method not found: {}", request.method),
                    data: None,
                },
            },
        }
    }
}

/// How often an idle server sweeps: often enough that a workspace outlives the idle timeout by
/// at most half of it, and that an exited rust-analyzer is waited on within a minute even with no
/// timeout at all.
fn sweep_period(idle_timeout: Option<Duration>) -> Duration {
    const AT_MOST: Duration = Duration::from_secs(60);
    idle_timeout.map_or(AT_MOST, |timeout| {
        (timeout / 2).clamp(Duration::from_secs(1), AT_MOST)
    })
}

/// The workspace root `workspace_path` names, refusing anything that is not one.
///
/// Takes a `file:` URI as readily as a path, and takes the manifest itself to mean the directory
/// holding it. A directory with no manifest in it is not a workspace: rust-analyzer started on one
/// has nothing loaded and says nothing about any file, which reads as a workspace full of code
/// nothing refers to. Refusing costs a typo'd path; accepting quietly redirects every question
/// asked afterwards.
pub(super) fn manifest_directory(workspace_path: &std::path::Path) -> Result<PathBuf> {
    let root = match workspace_path.file_name() {
        Some(name) if name == "Cargo.toml" => workspace_path
            .parent()
            .unwrap_or(workspace_path)
            .to_path_buf(),
        _ => workspace_path.to_path_buf(),
    };

    if !root.join("Cargo.toml").is_file() {
        return Err(anyhow::anyhow!(
            "{} is not a Rust workspace: no Cargo.toml in it. rust-analyzer started there would \
             load nothing and answer every question about every file with silence.",
            root.display()
        ));
    }

    Ok(root)
}

/// Merged stream of the signals that request server shutdown.
///
/// SIGINT, SIGTERM and SIGHUP on Unix; Ctrl+C and console-close events on Windows. The streams
/// are persistent, so signals delivered while no `recv()` is pending stay latched instead of
/// falling through to the default disposition. Note that registering SIGHUP also overrides an
/// inherited SIG_IGN disposition (e.g. from nohup), so a hangup always shuts the server down.
struct ShutdownSignal {
    #[cfg(unix)]
    sigint: tokio::signal::unix::Signal,
    #[cfg(unix)]
    sigterm: tokio::signal::unix::Signal,
    #[cfg(unix)]
    sighup: tokio::signal::unix::Signal,
    #[cfg(windows)]
    ctrl_c: tokio::signal::windows::CtrlC,
    #[cfg(windows)]
    ctrl_close: tokio::signal::windows::CtrlClose,
}

impl ShutdownSignal {
    fn new() -> Result<Self> {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{signal, SignalKind};

            Ok(Self {
                sigint: signal(SignalKind::interrupt())?,
                sigterm: signal(SignalKind::terminate())?,
                sighup: signal(SignalKind::hangup())?,
            })
        }
        #[cfg(windows)]
        {
            use tokio::signal::windows;

            Ok(Self {
                ctrl_c: windows::ctrl_c()?,
                ctrl_close: windows::ctrl_close()?,
            })
        }
        #[cfg(not(any(unix, windows)))]
        Ok(Self {})
    }

    /// Completes when the next shutdown signal arrives. Cancellation-safe.
    async fn recv(&mut self) {
        #[cfg(unix)]
        {
            tokio::select! {
                _ = self.sigint.recv() => {}
                _ = self.sigterm.recv() => {}
                _ = self.sighup.recv() => {}
            }
        }
        #[cfg(windows)]
        {
            tokio::select! {
                _ = self.ctrl_c.recv() => {}
                _ = self.ctrl_close.recv() => {}
            }
        }
        #[cfg(not(any(unix, windows)))]
        {
            // No signal support; only a stdin EOF stops the server.
            std::future::pending::<()>().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_paths_are_resolved_however_they_are_spelled() {
        let server = RustAnalyzerMCPServer::with_workspace(workspace());
        let absolute = server.workspace_root.join("src/lib.rs");
        let uri = uri::path_to_uri(&absolute).unwrap();

        for spelling in ["src/lib.rs", &absolute.display().to_string(), &uri] {
            let resolved = server.resolve_path(spelling);

            assert_eq!(resolved, absolute, "{spelling}");
            // Equality alone would not catch the Windows extended-length form, which compares
            // equal to the path meant while being unusable.
            assert!(std::fs::read_to_string(&resolved).is_ok(), "{spelling}");
        }
    }

    #[test]
    fn a_path_that_does_not_exist_still_resolves() {
        // Nothing to canonicalize against, but the error belongs to whoever reads the file.
        let server = RustAnalyzerMCPServer::with_workspace(workspace());
        let missing = server.workspace_root.join("src/nowhere.rs");

        assert_eq!(server.resolve_path("src/nowhere.rs"), missing);
    }

    #[tokio::test]
    async fn a_workspace_that_left_the_disk_is_forgotten_and_one_still_there_is_kept() {
        // Both halves in one fixture, because each rules out a different wrong sweep: forgetting
        // nothing passes the second assertion, and forgetting everything passes the first.
        let deleted = temporary_directory("left-the-disk");
        let mut server = RustAnalyzerMCPServer::with_workspace(workspace());
        // Neither of them may be the workspace in use, which is never swept.
        server.workspace_root = PathBuf::from("/this-server-was-not-pointed-here");
        for root in [workspace(), deleted.clone()] {
            server.clients.insert(
                root.clone(),
                Held::new(RustAnalyzerClient::new(root, json!({}))),
            );
        }

        std::fs::remove_dir_all(&deleted).unwrap();
        let current = server.workspace_root.clone();
        server.sweep_workspaces(Instant::now(), Some(current)).await;

        assert!(
            !server.clients.contains_key(&deleted),
            "a root that is gone cannot be named again, so it must not be held"
        );
        assert!(
            server.clients.contains_key(&workspace()),
            "a root still on disk must survive: evicting one costs a reindex that reads as \
             ordinary cold-start latency rather than as a bug"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_rust_analyzer_that_died_is_waited_on_whoever_is_asking() {
        // A root that is on disk, so that only having gone can account for the eviction.
        let elsewhere = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        // Swept by the timer, by a call about another workspace, and by a call about this one --
        // which is spared everything except having exited, and used to be dropped unwaited.
        for spare in [None, Some(workspace()), Some(elsewhere.clone())] {
            a_dead_rust_analyzer_is_waited_on(elsewhere.clone(), spare).await;
        }
    }

    #[cfg(unix)]
    async fn a_dead_rust_analyzer_is_waited_on(elsewhere: PathBuf, spare: Option<PathBuf>) {
        let child = tokio::process::Command::new("sh")
            .args(["-c", "exit 0"])
            .spawn()
            .unwrap();
        let pid = child.id().unwrap();

        let mut server = RustAnalyzerMCPServer::with_workspace(workspace());
        server.clients.insert(
            elsewhere.clone(),
            Held::new(RustAnalyzerClient::gone_over(elsewhere.clone(), Some(child)).await),
        );

        // The assertion below cannot discriminate until the child really is defunct: before that,
        // "not a zombie" is true of a process that is simply still running.
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while !is_defunct(pid) {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("fixture: the child never became defunct, so this proves nothing");

        server.sweep_workspaces(Instant::now(), spare.clone()).await;

        // The symptom before the mechanism, and in that order on purpose: holding the client is
        // what leaves the child defunct, so asserting membership first would short-circuit this
        // and leave it unable to fail for the reason it is here.
        assert!(
            !is_defunct(pid),
            "pid {pid} is still <defunct> after a sweep sparing {spare:?}"
        );
        assert!(
            !server.clients.contains_key(&elsewhere),
            "a client whose rust-analyzer has gone must not be held (sparing {spare:?})"
        );
    }

    #[tokio::test]
    async fn a_workspace_nobody_asks_about_is_let_go_once_the_idle_timeout_passes() {
        let timeout = Duration::from_secs(60);
        let asked = Instant::now();
        let (recent, idle, current) = (
            workspace(),
            PathBuf::from(env!("CARGO_MANIFEST_DIR")),
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test-project-diagnostics"),
        );
        let fill = |server: &mut RustAnalyzerMCPServer| {
            for (root, last_asked) in [
                (&recent, asked + timeout),
                (&idle, asked),
                (&current, asked),
            ] {
                let client = RustAnalyzerClient::new(root.clone(), json!({}));
                server
                    .clients
                    .insert(root.clone(), Held { client, last_asked });
            }
        };
        let now = asked + timeout + Duration::from_secs(1);

        // Each wrong rule fails a different assertion: never letting go keeps `idle`, timing from
        // the server's start rather than the last call drops `recent`, and ignoring the call in
        // progress drops `current`.
        let mut server =
            RustAnalyzerMCPServer::with_workspace(current.clone()).with_idle_timeout(Some(timeout));
        fill(&mut server);
        server.sweep_workspaces(now, Some(current.clone())).await;
        assert!(!server.clients.contains_key(&idle), "idle past the timeout");
        assert!(
            server.clients.contains_key(&recent),
            "asked about within it"
        );
        assert!(
            server.clients.contains_key(&current),
            "the call in progress"
        );

        // The timer spares nothing, the default included: a server nobody calls needs no index.
        server.sweep_workspaces(now, None).await;
        assert!(
            !server.clients.contains_key(&current),
            "idle default, swept by the timer"
        );

        // And no timeout keeps all of them for the server's life.
        let mut server =
            RustAnalyzerMCPServer::with_workspace(current.clone()).with_idle_timeout(None);
        fill(&mut server);
        server.sweep_workspaces(now + timeout * 1000, None).await;
        assert_eq!(server.clients.len(), 3, "no timeout, nothing idle");
    }

    #[cfg(unix)]
    fn is_defunct(pid: u32) -> bool {
        let reported = std::process::Command::new("ps")
            .args(["-o", "stat=", "-p", &pid.to_string()])
            .output()
            .unwrap();
        String::from_utf8_lossy(&reported.stdout)
            .trim()
            .starts_with('Z')
    }

    /// A real directory, so that `canonicalize()` has something to work with.
    fn workspace() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test-project")
    }

    /// A directory of this test's own, named so that a concurrent run cannot delete it.
    fn temporary_directory(purpose: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "rust-analyzer-mcp-{purpose}-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }
}
