//! Records which build this is, so that `--version` can say.
//!
//! The fork and the crates.io crate carry the same `version`, and a binary that cannot say which
//! of the two it is sends everyone who needs to know to `~/.cargo/.crates2.json` to find out.
//! The revision is the thing that actually distinguishes them, and it is known here.

use std::{path::Path, process::Command};

fn main() {
    for path in watched() {
        println!("cargo:rerun-if-changed={path}");
    }

    // Absent in a build from a published tarball, which has no git history -- and that is itself
    // the answer, since the fork is only ever installed from git.
    let revision = git(&["rev-parse", "--short", "HEAD"]).unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=RUST_ANALYZER_MCP_REV={revision}");
}

/// The files that change when HEAD moves, so that the revision is rebuilt rather than remembered.
///
/// Watching `HEAD` alone is the obvious thing and it is not enough: on a branch, `HEAD` holds
/// `ref: refs/heads/<branch>` and does not change when a commit is made -- the ref it names does.
/// Watching only `HEAD` therefore catches switching branches and misses committing on one, which
/// is the common case, and bakes in whichever revision was current the first time this ran.
/// Measured: four commits in, `--version` still named the first of them.
///
/// So watch `HEAD` for the switch, the ref it names for the commit, and `packed-refs` for the
/// case where that ref has no file of its own. Detached HEAD needs only the first, since `HEAD`
/// then holds the revision itself.
///
/// Only paths that exist are returned: cargo treats a named path that is missing as changed, and
/// would rebuild this crate on every single invocation.
fn watched() -> Vec<String> {
    // `.git` is a directory in a clone and a file in a worktree, and a worktree's HEAD is not
    // where a clone's is, so ask git rather than assuming either.
    let candidates = [
        git(&["rev-parse", "--git-path", "HEAD"]),
        git(&["symbolic-ref", "-q", "HEAD"])
            .and_then(|branch| git(&["rev-parse", "--git-path", &branch])),
        git(&["rev-parse", "--git-path", "packed-refs"]),
    ];

    candidates
        .into_iter()
        .flatten()
        .filter(|path| Path::new(path).exists())
        .collect()
}

fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }

    let text = String::from_utf8(output.stdout).ok()?;
    let text = text.trim().to_string();
    (!text.is_empty()).then_some(text)
}
