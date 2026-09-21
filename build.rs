//! Records which build this is, so that `--version` can say.
//!
//! The fork and the crates.io crate carry the same `version`, and a binary that cannot say which
//! of the two it is sends everyone who needs to know to `~/.cargo/.crates2.json` to find out.
//! The revision is the thing that actually distinguishes them, and it is known here.

use std::process::Command;

fn main() {
    // `.git` is a directory in a clone and a file in a worktree, so ask git where HEAD lives
    // rather than assuming. Without this the revision would be baked in once and never updated.
    if let Some(head) = git(&["rev-parse", "--git-path", "HEAD"]) {
        println!("cargo:rerun-if-changed={head}");
    }

    // Absent in a build from a published tarball, which has no git history -- and that is itself
    // the answer, since the fork is only ever installed from git.
    let revision = git(&["rev-parse", "--short", "HEAD"]).unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=RUST_ANALYZER_MCP_REV={revision}");
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
