//! Shared path-resolution helpers.
//!
//! The `tests/*.rs` files this crate replaces (#1110) located the repo root
//! via `env!("CARGO_MANIFEST_DIR")` — reliable there because that constant is
//! always the `quadraui/` crate directory, one level below the root. This
//! binary lives at `tools/lint/` instead, and (unlike a `cargo test` target)
//! is meant to be run with `cargo run -p quadraui-repo-lint` from wherever a
//! human or CI job happens to have its shell open — usually the repo root,
//! but not guaranteed. So instead of trusting a compile-time constant, walk
//! up from the actual current directory at run time looking for the two
//! files that only ever coexist at the quadraui repo root.

use std::path::PathBuf;

/// The quadraui repo root: the nearest ancestor of the current directory
/// that contains both `CLAUDE.md` and `.git`.
pub fn repo_root() -> PathBuf {
    let start = std::env::current_dir().expect("current directory is readable");
    let mut dir = start.clone();
    loop {
        if dir.join("CLAUDE.md").is_file() && dir.join(".git").exists() {
            return dir;
        }
        if !dir.pop() {
            panic!(
                "could not find the quadraui repo root (an ancestor directory \
                 containing both CLAUDE.md and .git) walking up from {}. Run \
                 this binary from inside the quadraui checkout.",
                start.display()
            );
        }
    }
}

/// The `quadraui/` crate directory — where `Cargo.toml`, `docs/`, and
/// `examples/` live.
pub fn quadraui_dir() -> PathBuf {
    repo_root().join("quadraui")
}
