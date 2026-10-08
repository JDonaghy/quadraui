//! Guard against the published `.crate` shipping internal-only material
//! under `docs/` — session notes, independent framework/code-smell
//! audits, draft design proposals, the primitive-distinctness decision
//! log — that is noise on crates.io's source viewer and docs.rs's
//! packaged-source tab, and gives no consumer anything they need at
//! build or run time. `quadraui/Cargo.toml`'s `exclude` list is what
//! actually keeps those files out; this test is the regression guard on
//! that list staying correct as `docs/` grows.
//!
//! This test runs the actual `cargo package --list -p quadraui` cargo
//! would use to build the `.crate` tarball — the same command
//! `Cargo.toml`'s own `exclude` comment points at — rather than
//! re-implementing cargo's gitignore-style include/exclude matching,
//! so it fails the instant someone adds a new internal doc without
//! excluding it, or accidentally excludes a consumer-facing one.
//!
//! `#[ignore]`d by default: it shells out to a real `cargo` subprocess,
//! which is slower and more environment-sensitive (needs a `cargo` on
//! `PATH` or `$CARGO`, and a writable target dir) than this crate's
//! other unit tests. Run explicitly:
//!
//! ```sh
//! cargo test -p quadraui --test package_hygiene -- --ignored
//! ```

use std::collections::HashSet;
use std::path::PathBuf;
use std::process::Command;

/// `quadraui/`'s own crate dir (this test's `CARGO_MANIFEST_DIR`).
fn crate_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Runs `cargo package --list -p quadraui` and returns the set of paths
/// (relative to the crate root, forward-slash separated, exactly as cargo
/// prints them) that would end up in the published tarball.
///
/// `--allow-dirty`: this crate's own worktree is routinely uncommitted
/// mid-session (this test may run against exactly that state), and a
/// dirty-tree refusal here would say nothing about whether the
/// include/exclude rules themselves are correct — the thing this test
/// actually checks.
fn packaged_files() -> HashSet<String> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let output = Command::new(&cargo)
        .args([
            "package",
            "--list",
            "-p",
            "quadraui",
            "--allow-dirty",
            "--quiet",
        ])
        .current_dir(crate_root())
        .output()
        .unwrap_or_else(|e| panic!("failed to spawn `{cargo} package --list -p quadraui`: {e}"));

    assert!(
        output.status.success(),
        "`cargo package --list -p quadraui` exited with {:?}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    String::from_utf8(output.stdout)
        .expect("cargo package --list output is UTF-8")
        .lines()
        .map(|line| line.trim().to_string())
        .filter(|line| !line.is_empty())
        .collect()
}

/// Internal-only docs (contributor/maintainer material) that must NOT ride
/// along in the published tarball. Mirrors `Cargo.toml`'s `exclude` list —
/// kept as a literal list here (not derived from `Cargo.toml`) so this test
/// catches a mismatch between the two instead of trivially agreeing with
/// whatever `exclude` currently says.
const INTERNAL_DOCS: &[&str] = &[
    "docs/ARCHITECTURE.md",
    "docs/BACKEND.md",
    "docs/IME_INPUT_PROPOSAL.md",
    "docs/LESSONS.md",
    "docs/NATIVE_GUI_LESSONS.md",
    "docs/PRIMITIVE_RULES.md",
    "docs/ROWS_PROVIDER_PROPOSAL.md",
    "docs/SESSION_HISTORY.md",
    "docs/SMELL_AUDIT_2026-07.md",
    "docs/TESTING.md",
    "docs/UI_CRATE_DESIGN.md",
    "docs/WEB_BACKEND_PROPOSAL.md",
    "docs/audits/FRAMEWORK_AUDIT_2026-09-26.md",
    "docs/decisions/BACKEND_TRAIT_PROPOSAL.md",
    "docs/decisions/DECISIONS.md",
];

/// Consumer-facing docs that must keep shipping — app authors are pointed
/// at these from the root `README.md` and `src/lib.rs`'s own rustdoc.
const CONSUMER_DOCS: &[&str] = &[
    "docs/APP_ARCHITECTURE.md",
    "docs/CLIPBOARD.md",
    "docs/COMPOSE.md",
    "docs/CONSUMER_PATTERNS.md",
    "docs/EVENT_ROUTING_TOUR.md",
    "docs/GUIDE.md",
    "docs/INSTALL.md",
    "docs/KITTY_KEYBOARD_PROTOCOL.md",
    "docs/TUI_CONSUMER_TOUR.md",
];

#[test]
#[ignore = "shells out to a real `cargo package --list` subprocess"]
fn published_crate_excludes_internal_docs() {
    let files = packaged_files();
    assert!(
        !files.is_empty(),
        "`cargo package --list -p quadraui` printed no files at all — \
         the command probably failed silently"
    );

    for internal in INTERNAL_DOCS {
        assert!(
            !files.contains(*internal),
            "{internal} is in the published package — it's internal-only \
             material and should be covered by Cargo.toml's `exclude`"
        );
    }
}

#[test]
#[ignore = "shells out to a real `cargo package --list` subprocess"]
fn published_crate_ships_consumer_docs() {
    let files = packaged_files();
    assert!(
        !files.is_empty(),
        "`cargo package --list -p quadraui` printed no files at all — \
         the command probably failed silently"
    );

    for consumer_doc in CONSUMER_DOCS {
        assert!(
            files.contains(*consumer_doc),
            "{consumer_doc} is missing from the published package — it's \
             consumer-facing documentation the root README and lib.rs \
             rustdoc point readers at, and should NOT be covered by \
             Cargo.toml's `exclude`"
        );
    }
}

/// `INTERNAL_DOCS` and `CONSUMER_DOCS` above must not overlap and, together
/// with the always-excluded `tests/`/`benches/` directories, should account
/// for everything currently under `docs/` — guards this test's own two
/// lists from silently going stale as new docs are added on either side
/// without being added here.
#[test]
fn internal_and_consumer_doc_lists_cover_docs_dir_with_no_overlap() {
    let internal: HashSet<&str> = INTERNAL_DOCS.iter().copied().collect();
    let consumer: HashSet<&str> = CONSUMER_DOCS.iter().copied().collect();
    assert!(
        internal.is_disjoint(&consumer),
        "a path appears in both INTERNAL_DOCS and CONSUMER_DOCS"
    );

    let mut seen = HashSet::new();
    let mut stack = vec![crate_root().join("docs")];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("failed to read {}: {e}", dir.display()))
        {
            let entry = entry.expect("readable dir entry");
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let rel = path
                .strip_prefix(crate_root())
                .expect("entry is under crate_root()/docs")
                .to_str()
                .expect("utf8 path")
                .replace('\\', "/");
            seen.insert(rel);
        }
    }

    let known: HashSet<String> = internal
        .iter()
        .chain(consumer.iter())
        .map(|s| s.to_string())
        .collect();

    let unknown: Vec<&String> = seen.difference(&known).collect();
    assert!(
        unknown.is_empty(),
        "docs/ contains file(s) not classified in either INTERNAL_DOCS or \
         CONSUMER_DOCS in this test: {unknown:?} — classify them (see \
         quadraui/Cargo.toml's `exclude` comment for the policy) and add \
         them to one of the two lists above"
    );
}
