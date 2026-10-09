//! Integration test for headless capture mode (#1348).
//!
//! Runs [`quadraui_gallery::capture::run_capture`] — the exact function
//! `src/main.rs`'s `--capture <dir>` flag calls — into a temp dir and
//! asserts on its on-disk output: the TUI SVG for one demo exists, is
//! non-empty, contains a known label, and is listed in `manifest.json`.
#![cfg(feature = "tui")]

use std::fs;

use quadraui_gallery::registry::registry;

/// `run_capture` writes real images for the TUI backend and
/// `status`-only entries (`unsupported`/`capture-pending`) for every
/// other backend — both kinds must show up in the manifest, and the
/// real ones must point at a file that actually exists on disk.
#[test]
fn capture_writes_tui_svg_and_lists_it_in_the_manifest() {
    let dir = tempfile::tempdir().expect("create temp capture dir");

    let manifest = quadraui_gallery::capture::run_capture(dir.path())
        .expect("run_capture should succeed into a fresh temp dir");

    // At least one registered demo exists today (ToastDemo) — this
    // assertion would fail loudly, not silently pass, if the registry
    // were ever emptied.
    let demos = registry();
    assert!(!demos.is_empty(), "registry() must list at least one demo");
    let demo = &demos[0];
    let variant = demo.variants()[0];

    // ── manifest.json on disk, and parses ───────────────────────────
    let manifest_path = dir.path().join("manifest.json");
    assert!(
        manifest_path.is_file(),
        "manifest.json must be written to the capture dir"
    );
    let manifest_text = fs::read_to_string(&manifest_path).expect("read manifest.json");
    let manifest_json: serde_json::Value =
        serde_json::from_str(&manifest_text).expect("manifest.json must be valid JSON");
    let rows = manifest_json
        .as_array()
        .expect("manifest.json must be a JSON array");
    assert_eq!(
        rows.len(),
        manifest.len(),
        "manifest.json on disk must match the Vec<ManifestEntry> run_capture returned"
    );

    // ── the TUI entry for this demo/variant has a real path ─────────
    let tui_entry = manifest
        .iter()
        .find(|e| e.backend == "tui" && e.demo == demo.name() && e.variant == variant)
        .unwrap_or_else(|| {
            panic!(
                "manifest should list a tui entry for {}/{variant}, got: {manifest:#?}",
                demo.name()
            )
        });
    let path = tui_entry.path.as_deref().unwrap_or_else(|| {
        panic!(
            "tui entry for {}/{variant} should have a path, not a status",
            demo.name()
        )
    });
    assert!(
        tui_entry.status.is_none(),
        "a tui entry with a real path should not also carry a status"
    );

    // ── the SVG file itself exists, is non-empty SVG, and names the demo ─
    let svg_path = dir.path().join(path);
    assert!(
        svg_path.is_file(),
        "{} should exist on disk",
        svg_path.display()
    );
    let svg = fs::read_to_string(&svg_path).expect("read captured SVG");
    assert!(!svg.trim().is_empty(), "captured SVG must not be empty");
    assert!(
        svg.starts_with("<svg"),
        "captured file must be an SVG document:\n{svg}"
    );
    assert!(
        svg.contains(demo.name()),
        "captured SVG should contain the demo's own name ({}) as a painted label:\n{svg}",
        demo.name()
    );

    // ── every other backend gets a status-only entry, never a faked image ─
    for backend in ["gtk", "win"] {
        let entry = manifest
            .iter()
            .find(|e| e.backend == backend && e.demo == demo.name() && e.variant == variant)
            .unwrap_or_else(|| {
                panic!(
                    "manifest should list a {backend} entry for {}/{variant}",
                    demo.name()
                )
            });
        assert_eq!(
            entry.status.as_deref(),
            Some("capture-pending"),
            "{backend} has no headless capture path yet, so its entry must be capture-pending, not a path"
        );
        assert!(
            entry.path.is_none(),
            "{backend} entry must not carry a path"
        );
    }
}

/// Every manifest row names a demo/group/variant that really exists in
/// the registry — a table-driven guard against a future capture bug
/// that invents rows (or silently drops some) rather than mirroring
/// `registry()` exactly.
#[test]
fn every_manifest_row_matches_a_registered_demo_variant() {
    let dir = tempfile::tempdir().expect("create temp capture dir");
    let manifest = quadraui_gallery::capture::run_capture(dir.path()).expect("run_capture");

    let demos = registry();
    let expected_rows: usize = demos.iter().map(|d| d.variants().len()).sum::<usize>()
        * manifest
            .iter()
            .map(|e| e.backend.clone())
            .collect::<std::collections::HashSet<_>>()
            .len();
    assert_eq!(
        manifest.len(),
        expected_rows,
        "expected one manifest row per (demo, variant, backend) combination"
    );

    for entry in &manifest {
        let demo = demos
            .iter()
            .find(|d| d.name() == entry.demo)
            .unwrap_or_else(|| panic!("manifest row names unknown demo {:?}", entry.demo));
        assert_eq!(
            demo.group(),
            entry.group,
            "manifest row's group must match the demo's own"
        );
        assert!(
            demo.variants().contains(&entry.variant.as_str()),
            "manifest row names variant {:?} not in {:?}'s variants {:?}",
            entry.variant,
            entry.demo,
            demo.variants()
        );
    }
}
