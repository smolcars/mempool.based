//! Sets `SOURCE_REVISION` for the binary: the commit it was built from, reported at `/healthz`
//! and in `server_starting`. Taken from the `SOURCE_REVISION` variable when it is set (the image
//! build, whose context has no `.git`), else from git when the workspace is a checkout, else
//! `unknown`, as in a build from a source archive.

use std::path::Path;
use std::process::Command;
use std::{env, io};

fn main() -> io::Result<()> {
    println!("cargo::rerun-if-env-changed=SOURCE_REVISION");
    let revision = match env::var("SOURCE_REVISION") {
        Ok(r) if !r.is_empty() => r,
        _ => from_git().unwrap_or_else(|| "unknown".to_owned()),
    };
    let hex = revision.len() == 40 && revision.bytes().all(|b| b.is_ascii_hexdigit());
    if !hex && revision != "unknown" {
        return Err(io::Error::other(format!(
            "SOURCE_REVISION must be a 40-character commit hash, got {revision:?}"
        )));
    }
    println!(
        "cargo::rustc-env=SOURCE_REVISION={}",
        revision.to_ascii_lowercase()
    );
    Ok(())
}

/// `HEAD` of the checkout this workspace is, and the files to watch for it moving. `None` without
/// git, outside a checkout, or inside some other repository (an archive unpacked into one).
fn from_git() -> Option<String> {
    let workspace = Path::new(&env::var_os("CARGO_MANIFEST_DIR")?).join("../..");
    let out = Command::new("git")
        .current_dir(&workspace)
        .args(["rev-parse", "HEAD", "--show-toplevel", "--absolute-git-dir"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let out = String::from_utf8(out.stdout).ok()?;
    let mut lines = out.lines();
    let (head, top, git_dir) = (lines.next()?, lines.next()?, lines.next()?);
    if Path::new(top).canonicalize().ok()? != workspace.canonicalize().ok()? {
        return None;
    }
    // A commit, checkout or reset moves HEAD or appends to its log; either reruns this.
    for f in ["HEAD", "logs/HEAD"] {
        let path = Path::new(git_dir).join(f);
        if path.exists() {
            println!("cargo::rerun-if-changed={}", path.display());
        }
    }
    Some(head.to_owned())
}
