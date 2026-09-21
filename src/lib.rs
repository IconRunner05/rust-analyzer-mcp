pub mod blast;
pub mod cli;
pub mod config;
pub mod diagnostics;
pub mod locations;
pub mod lsp;
pub mod mcp;
pub mod position;
pub mod protocol;
pub mod settings;
pub mod uri;

pub use mcp::RustAnalyzerMCPServer;

/// Which build this is, in a form that says so.
///
/// The fork and the crate it was forked from carry the same `version`, so a version line that is
/// only that number cannot answer the one question anybody asks it -- which of the two is
/// installed -- and everyone who needs to know ends up reading `~/.cargo/.crates2.json` instead.
///
/// The revision answers it, and the tool count answers it a second way: the two builds do not
/// expose the same number of tools, and the count is the one fact here that cannot drift out of
/// date, because it is counted rather than written down.
pub fn version() -> String {
    format!(
        "rust-analyzer-mcp {} (fork, rev {}, {} tools)",
        env!("CARGO_PKG_VERSION"),
        env!("RUST_ANALYZER_MCP_REV"),
        mcp::tools::get_tools().len(),
    )
}

#[cfg(test)]
mod tests {
    /// A version line that is only the number cannot answer the question it is asked, because
    /// the fork and the crate it was forked from carry the same number.
    #[test]
    fn the_version_says_which_build_this_is() {
        let version = super::version();

        assert!(version.contains("fork"), "{version}");
        assert!(
            version.contains(&format!("{} tools", super::mcp::tools::get_tools().len())),
            "{version}"
        );
        assert!(
            !version.contains("rev unknown"),
            "a build from a git checkout is expected to know its revision: {version}"
        );
    }
}
