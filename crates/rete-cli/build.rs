//! Embed the engine's identity for the build record (`rete card` → Build):
//! the git commit the binary was built from, the target triple and the cargo
//! profile.
//!
//! The commit comes from, in order:
//!
//! 1. `RETE_SOURCE_REVISION` — set it wherever `.git` is not readable: a
//!    Docker build (the CLI image's `ARG`), a git worktree mounted into the dev
//!    container (its `.git` file points at a host path), CI. The same variable
//!    `scripts/build_wasm.sh` stamps the browser engine with.
//! 2. `git rev-parse HEAD`, when git and the repository are both reachable.
//! 3. `.cargo_vcs_info.json`, which `cargo package` writes into a crates.io
//!    tarball.
//! 4. `unknown`.
//!
//! Nothing here can fail the build: a commit that cannot be found is recorded
//! as `unknown`, never guessed.

use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=RETE_SOURCE_REVISION");
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default());
    let commit = from_env()
        .or_else(|| from_git(&manifest))
        .or_else(|| from_vcs_info(&manifest))
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=RETE_GIT_COMMIT={commit}");
    println!(
        "cargo:rustc-env=RETE_TARGET={}",
        std::env::var("TARGET").unwrap_or_else(|_| "unknown".into())
    );
    println!(
        "cargo:rustc-env=RETE_PROFILE={}",
        std::env::var("PROFILE").unwrap_or_else(|_| "unknown".into())
    );
}

/// A full or abbreviated hex object name, and nothing else.
fn clean(s: &str) -> Option<String> {
    let s = s.trim();
    (s.len() >= 7 && s.len() <= 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| s.to_ascii_lowercase())
}

fn from_env() -> Option<String> {
    clean(&std::env::var("RETE_SOURCE_REVISION").ok()?)
}

fn from_git(manifest: &Path) -> Option<String> {
    let out = Command::new("git")
        .args(["-c", "safe.directory=*", "rev-parse", "HEAD"])
        .current_dir(manifest)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let commit = clean(&String::from_utf8_lossy(&out.stdout))?;
    // Rebuild when HEAD moves. Only paths that exist are watched: cargo treats a
    // missing one as always stale, which would recompile the CLI on every build.
    if let Some(git_dir) = git_path(manifest, "--git-dir") {
        let head = git_dir.join("HEAD");
        if head.exists() {
            println!("cargo:rerun-if-changed={}", head.display());
        }
        if let Ok(text) = std::fs::read_to_string(&head) {
            if let Some(r) = text.trim().strip_prefix("ref: ") {
                let common = git_path(manifest, "--git-common-dir").unwrap_or(git_dir.clone());
                for p in [common.join(r), common.join("packed-refs")] {
                    if p.exists() {
                        println!("cargo:rerun-if-changed={}", p.display());
                    }
                }
            }
        }
    }
    Some(commit)
}

fn git_path(manifest: &Path, flag: &str) -> Option<PathBuf> {
    let out = Command::new("git")
        .args(["-c", "safe.directory=*", "rev-parse", flag])
        .current_dir(manifest)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let p = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
    Some(if p.is_absolute() { p } else { manifest.join(p) })
}

/// `{"git": {"sha1": "…"}, …}` — parsed by hand: a build script with no
/// dependencies stays a build script nobody has to think about.
fn from_vcs_info(manifest: &Path) -> Option<String> {
    let text = std::fs::read_to_string(manifest.join(".cargo_vcs_info.json")).ok()?;
    let at = text.find("\"sha1\"")?;
    let rest = &text[at + 6..];
    let start = rest.find('"')? + 1;
    let end = start + rest[start..].find('"')?;
    clean(&rest[start..end])
}
