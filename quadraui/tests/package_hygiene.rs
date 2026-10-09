//! Guard against the published `.crate` shipping internal-only material
//! under `docs/` — session notes, independent framework/code-smell
//! audits, draft design proposals — that is noise on crates.io's source
//! viewer and docs.rs's packaged-source tab, and gives no consumer
//! anything they need at build or run time. `quadraui/Cargo.toml`'s
//! `exclude` list is what actually keeps those files out; this module is
//! the regression guard on that list staying correct as `docs/` grows.
//!
//! Two layers of guard, with different tradeoffs:
//!
//! - [`cargo_toml_exclude_matches_doc_classification`] parses
//!   `Cargo.toml`'s `exclude` array directly and checks it against
//!   [`INTERNAL_DOCS`]/[`CONSUMER_DOCS`] below. It is NOT `#[ignore]`d —
//!   it runs on a plain `cargo test`, so it fails the instant someone
//!   adds a new internal doc to [`INTERNAL_DOCS`] without a matching
//!   `Cargo.toml` entry, or vice versa, with no extra flag required.
//! - [`published_crate_excludes_internal_docs`]/
//!   [`published_crate_ships_consumer_docs`] run the actual
//!   `cargo package --list -p quadraui` cargo would use to build the
//!   `.crate` tarball, rather than re-implementing cargo's
//!   gitignore-style include/exclude matching — belt-and-braces proof
//!   that cargo's own matching agrees with the manifest-parsing test
//!   above. `#[ignore]`d by default: it shells out to a real `cargo`
//!   subprocess, which is slower and more environment-sensitive (needs
//!   a `cargo` on `PATH` or `$CARGO`, and a writable target dir) than
//!   this crate's other unit tests. Run explicitly:
//!
//!   ```sh
//!   cargo test -p quadraui --test package_hygiene -- --ignored
//!   ```

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

    // `cargo package --list` is documented/observed to print forward
    // slashes on every platform this crate's CI runs, but that is
    // unverified on a Windows host — normalise defensively so a stray
    // `\` there doesn't silently fail every `files.contains("docs/...")`
    // check below.
    let files: HashSet<String> = String::from_utf8(output.stdout)
        .expect("cargo package --list output is UTF-8")
        .lines()
        .map(|line| line.trim().replace('\\', "/"))
        .filter(|line| !line.is_empty())
        .collect();

    assert!(
        !files.is_empty(),
        "`cargo package --list -p quadraui` printed no files at all — \
         the command probably failed silently"
    );

    files
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
];

/// Consumer-facing docs that must keep shipping — app authors are pointed
/// at these from the root `README.md` and `src/lib.rs`'s own rustdoc.
/// `docs/decisions/*` lives here, not in `INTERNAL_DOCS`, even though both
/// files are also read at agent session start: `DECISIONS.md`'s own D-019
/// entry says explicitly it is written for an outside developer deciding
/// whether to depend on the crate — see `Cargo.toml`'s `exclude` comment
/// for the long form.
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
    "docs/decisions/BACKEND_TRAIT_PROPOSAL.md",
    "docs/decisions/DECISIONS.md",
];

#[test]
#[ignore = "shells out to a real `cargo package --list` subprocess"]
fn published_crate_excludes_internal_docs() {
    let files = packaged_files();

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

/// #1393: the published `.crate` shipped no license text at all —
/// `quadraui/LICENSE-MIT` and `quadraui/LICENSE-APACHE` are symlinks to
/// the repo-root originals (see `Cargo.toml`'s `license.workspace`
/// comment for why symlinks rather than a `license-file` field), added
/// specifically so cargo's default packaging picks them up. This is the
/// regression guard on that: `cargo package --list` resolving a symlink
/// to a path outside the crate dir down to zero files, or the symlinks
/// going missing again, both fail silently otherwise — there's no
/// `cargo publish` dry run in this repo's quality gate that would have
/// caught a licenseless tarball on its own.
#[test]
#[ignore = "shells out to a real `cargo package --list` subprocess"]
fn published_crate_ships_license_files() {
    let files = packaged_files();

    for license_file in ["LICENSE-MIT", "LICENSE-APACHE"] {
        assert!(
            files.contains(license_file),
            "{license_file} is missing from the published package — this \
             crate is dual-licensed (`license.workspace = true` resolves \
             to \"MIT OR Apache-2.0\") but its `.crate` tarball carries no \
             license text at all. Check that `quadraui/{license_file}` \
             still exists as a symlink to the repo-root original and \
             isn't covered by Cargo.toml's `exclude`."
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
            // Only classify tracked markdown docs — a stray `.DS_Store`,
            // editor swap file, or scratch note dropped under `docs/`
            // isn't a doc either list needs to know about, and would
            // otherwise turn a routine `cargo test` red with a
            // classification failure that has nothing to do with package
            // hygiene.
            let is_dotfile = path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with('.'));
            if is_dotfile || path.extension().and_then(|e| e.to_str()) != Some("md") {
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

/// Parses `Cargo.toml`'s `[package] exclude = [...]` array into a set of
/// entries exactly as written (e.g. `"docs/audits/"`, a directory prefix,
/// alongside `"docs/BACKEND.md"`, an exact file).
///
/// Deliberately a hand-rolled line scan, not a TOML parser — `exclude` is
/// always a flat array of double-quoted strings in this manifest (see the
/// literal below), and `src/lib.rs`'s own `package_version`/
/// `declared_rust_version` tests already establish the pattern of parsing
/// `include_str!("../Cargo.toml")` this way rather than adding a parsing
/// dependency just to read one key back out of a manifest this crate
/// already owns.
fn cargo_toml_exclude_entries() -> HashSet<String> {
    let manifest = std::fs::read_to_string(crate_root().join("Cargo.toml"))
        .expect("quadraui/Cargo.toml is readable");
    let after_key = manifest
        .split_once("exclude = [")
        .expect("quadraui/Cargo.toml has a `[package] exclude = [...]` array")
        .1;
    let array_body = after_key
        .split_once(']')
        .expect("`exclude = [` is closed by a `]` somewhere later in the manifest")
        .0;
    array_body
        .split(',')
        .filter_map(|entry| {
            let entry = entry.trim();
            entry
                .strip_prefix('"')
                .and_then(|s| s.strip_suffix('"'))
                .map(str::to_string)
        })
        .collect()
}

/// A doc path is excluded from the published tarball if it (or a directory
/// prefix of it, e.g. `"docs/audits/"` covering
/// `"docs/audits/FRAMEWORK_AUDIT_2026-09-26.md"`) appears in `exclude`.
fn is_excluded(exclude: &HashSet<String>, path: &str) -> bool {
    exclude.contains(path)
        || exclude
            .iter()
            .any(|e| e.ends_with('/') && path.starts_with(e))
}

/// The regression guard the two `#[ignore]`d subprocess tests above cannot
/// provide on their own: this runs on every plain `cargo test`, with no
/// `--ignored` flag (nothing in this repo's CI or `docs/TESTING.md` quality
/// gate ever passes one), so it is the one check that actually fires the
/// instant `INTERNAL_DOCS`/`CONSUMER_DOCS` and `Cargo.toml`'s `exclude`
/// array drift apart — e.g. a new internal doc added to `INTERNAL_DOCS`
/// here without a matching `Cargo.toml` entry, which the subprocess tests
/// above would only catch on an explicit `--ignored` run nobody makes by
/// default.
#[test]
fn cargo_toml_exclude_matches_doc_classification() {
    let exclude = cargo_toml_exclude_entries();
    assert!(
        !exclude.is_empty(),
        "parsed zero entries out of quadraui/Cargo.toml's `exclude` array — \
         the parser probably doesn't match the manifest's current layout"
    );

    for internal in INTERNAL_DOCS {
        assert!(
            is_excluded(&exclude, internal),
            "{internal} is in INTERNAL_DOCS but Cargo.toml's `exclude` array \
             doesn't cover it — add it (or a covering directory prefix) to \
             `exclude`, or this file WILL ship in the published package"
        );
    }

    for consumer_doc in CONSUMER_DOCS {
        assert!(
            !is_excluded(&exclude, consumer_doc),
            "{consumer_doc} is in CONSUMER_DOCS but Cargo.toml's `exclude` \
             array covers it — either un-exclude it, or move it to \
             INTERNAL_DOCS if it really shouldn't ship"
        );
    }
}
