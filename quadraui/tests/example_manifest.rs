//! Manifest hygiene for `quadraui/examples/` — every example file must have
//! an explicit `[[example]]` entry in `quadraui/Cargo.toml` whose
//! `required-features` names the backend it actually calls into (#595).
//!
//! Why this is a *test* and not a review checklist: cargo's example
//! autodiscovery is silently permissive. Drop `examples/gtk_foo.rs` into the
//! tree with no `[[example]]` stanza and cargo happily picks it up — with
//! **no** `required-features` — so `cargo test --features tui` tries to
//! compile a file whose body is `quadraui::gtk::run(..)` and dies on
//! `could not find gtk in quadraui`. Nothing about the example itself is
//! wrong; the manifest is. That is precisely how #595's first CI run went
//! red on the `tui (build, test, clippy)` job.
//!
//! Two properties make that failure mode expensive enough to gate:
//!
//! - **`cargo build` does not catch it.** Examples aren't built by a plain
//!   `cargo build`, only by `cargo test` / `cargo build --examples`, so the
//!   build step goes green and the break surfaces one step later.
//! - **It is invisible locally to whoever added the pair.** A worker who
//!   runs `cargo test --features gtk` (or runs the demo, per CLAUDE.md's
//!   "demos are mandatory") sees nothing — the mismatch only bites the
//!   *other* backend's job.
//!
//! This test reads the real manifest and the real directory listing, so it
//! fails at the moment the pair is added, in whichever feature set the
//! worker happened to run.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// The `quadraui` crate root — where `Cargo.toml` and `examples/` live.
fn crate_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// One `[[example]]` stanza, reduced to the fields this test cares about.
#[derive(Debug, Default)]
struct ExampleEntry {
    name: String,
    path: String,
    required_features: Vec<String>,
}

/// Minimal, deliberately dumb TOML slice: collect every `[[example]]` table
/// and its `name` / `path` / `required-features` keys.
///
/// A full TOML parser isn't a dependency of this crate and isn't worth
/// adding for three flat string keys. The manifest's example stanzas are
/// uniform single-line `key = value` pairs, and the assertions below fail
/// loudly (missing entry / empty features) rather than silently passing if
/// this parser ever fails to understand a stanza.
fn parse_example_entries(manifest: &str) -> Vec<ExampleEntry> {
    let mut entries = Vec::new();
    let mut current: Option<ExampleEntry> = None;

    for raw in manifest.lines() {
        let line = raw.trim();
        if line.starts_with('[') {
            // Any new table header closes the stanza we were filling.
            if let Some(entry) = current.take() {
                entries.push(entry);
            }
            if line == "[[example]]" {
                current = Some(ExampleEntry::default());
            }
            continue;
        }
        let Some(entry) = current.as_mut() else {
            continue;
        };
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        match key {
            "name" => entry.name = unquote(value),
            "path" => entry.path = unquote(value),
            "required-features" => {
                entry.required_features = value
                    .trim_start_matches('[')
                    .trim_end_matches(']')
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(unquote)
                    .collect();
            }
            _ => {}
        }
    }
    if let Some(entry) = current.take() {
        entries.push(entry);
    }
    entries
}

fn unquote(value: &str) -> String {
    value.trim().trim_matches('"').to_string()
}

/// Every `examples/*.rs`, relative to the crate root, as it would be spelled
/// in a `path = ...` key. `examples/common/` is a module directory shared by
/// the runners, not an example target, so it's excluded by only listing
/// files at the top level.
fn example_files(root: &Path) -> Vec<String> {
    let dir = root.join("examples");
    let mut files: Vec<String> = fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", dir.display()))
        .map(|e| e.expect("readable dir entry"))
        .filter(|e| e.file_type().expect("entry file type").is_file())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".rs"))
        .map(|n| format!("examples/{n}"))
        .collect();
    files.sort();
    files
}

/// Backend feature a file name commits the example to, by prefix.
/// `msv_*` (multi-section view) examples are TUI runners.
fn required_backend_for(file: &str) -> Option<&'static str> {
    let name = file.strip_prefix("examples/").unwrap_or(file);
    if name.starts_with("gtk_") {
        Some("gtk")
    } else if name.starts_with("macos_") {
        Some("macos")
    } else if name.starts_with("tui_") || name.starts_with("msv_") {
        Some("tui")
    } else {
        None
    }
}

fn entries_by_path() -> BTreeMap<String, ExampleEntry> {
    let manifest = fs::read_to_string(crate_root().join("Cargo.toml")).expect("read Cargo.toml");
    let entries = parse_example_entries(&manifest);
    assert!(
        !entries.is_empty(),
        "parsed zero [[example]] stanzas out of quadraui/Cargo.toml — the \
         parser in this test has drifted from the manifest format, so every \
         other assertion here would pass vacuously"
    );
    let mut by_path = BTreeMap::new();
    for entry in entries {
        if let Some(prev) = by_path.insert(entry.path.clone(), entry) {
            panic!(
                "duplicate [[example]] stanzas for path {:?} (name {:?})",
                prev.path, prev.name
            );
        }
    }
    by_path
}

/// The regression #595 hit: a new `examples/*.rs` with no `[[example]]`
/// stanza is autodiscovered with no `required-features` and compiled under
/// every feature set.
#[test]
fn every_example_file_has_a_manifest_entry() {
    let by_path = entries_by_path();
    let missing: Vec<String> = example_files(&crate_root())
        .into_iter()
        .filter(|f| !by_path.contains_key(f))
        .collect();

    assert!(
        missing.is_empty(),
        "these example files have no [[example]] entry in quadraui/Cargo.toml: \
         {missing:?}\n\nWithout one, cargo autodiscovers them with NO \
         required-features, so e.g. a gtk_* example is compiled by \
         `cargo test --features tui` and fails on `quadraui::gtk` being \
         configured out. Add:\n\n\
         [[example]]\nname = \"<file stem>\"\npath = \"examples/<file>.rs\"\n\
         required-features = [\"<backend>\"]"
    );
}

/// Every stanza must actually gate on its backend — an entry that exists but
/// declares no `required-features` (or the wrong one) fails identically to
/// having no entry at all.
#[test]
fn every_example_entry_requires_its_backend_feature() {
    let by_path = entries_by_path();
    let mut problems = Vec::new();

    for file in example_files(&crate_root()) {
        let Some(entry) = by_path.get(&file) else {
            continue; // reported by every_example_file_has_a_manifest_entry
        };
        assert_eq!(
            entry.name,
            file.trim_start_matches("examples/").trim_end_matches(".rs"),
            "[[example]] name must match its file stem for {file}"
        );
        match required_backend_for(&file) {
            Some(feature) if !entry.required_features.iter().any(|f| f == feature) => problems
                .push(format!(
                    "{file}: required-features = {:?} does not include {feature:?}",
                    entry.required_features
                )),
            None if entry.required_features.is_empty() => {
                problems.push(format!("{file}: no required-features declared"));
            }
            _ => {}
        }
    }

    assert!(
        problems.is_empty(),
        "example manifest entries are missing their backend feature gate:\n  {}",
        problems.join("\n  ")
    );
}

/// Anti-vacuity + parser sanity: the stanza shape this test relies on is the
/// one the manifest actually uses, and a missing/ungated stanza is really
/// detected rather than skipped.
#[test]
fn parser_detects_missing_and_ungated_entries() {
    let manifest = r#"
[package]
name = "demo"

[[example]]
name = "tui_ok"
path = "examples/tui_ok.rs"
required-features = ["tui"]

[[example]]
name = "gtk_ungated"
path = "examples/gtk_ungated.rs"

[[example]]
name = "tui_terminal"
path = "examples/tui_terminal.rs"
required-features = ["tui", "terminal"]

[dev-dependencies]
serde_json = "1.0"
"#;
    let entries = parse_example_entries(manifest);
    assert_eq!(entries.len(), 3, "parsed: {entries:?}");

    assert_eq!(entries[0].name, "tui_ok");
    assert_eq!(entries[0].path, "examples/tui_ok.rs");
    assert_eq!(entries[0].required_features, vec!["tui".to_string()]);

    // The exact shape #595 shipped: a stanza with no feature gate at all.
    assert!(entries[1].required_features.is_empty());
    assert!(!entries[1]
        .required_features
        .iter()
        .any(|f| f == required_backend_for("examples/gtk_ungated.rs").unwrap()));

    // Multi-feature lists parse as lists, not as one blob.
    assert_eq!(
        entries[2].required_features,
        vec!["tui".to_string(), "terminal".to_string()]
    );

    // A trailing `[dev-dependencies]` table must not leak keys into the last
    // stanza.
    assert_eq!(entries[2].path, "examples/tui_terminal.rs");

    assert_eq!(
        required_backend_for("examples/msv_sc_panel.rs"),
        Some("tui")
    );
    assert_eq!(
        required_backend_for("examples/macos_demo.rs"),
        Some("macos")
    );
    assert_eq!(required_backend_for("examples/whatever.rs"), None);
}
