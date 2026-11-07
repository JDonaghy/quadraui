//! Integration test for headless capture mode.
//!
//! Runs [`quadraui_gallery::capture::run_capture`] — the exact function
//! `src/main.rs`'s `--capture <dir>` flag calls — into a temp dir and
//! asserts on its on-disk output: the TUI SVG for one demo exists, is
//! non-empty, contains a known label, and is listed in `manifest.json`.
#![cfg(feature = "tui")]

use std::fs;

use quadraui_gallery::registry::registry;

/// `run_capture` writes real images for the TUI backend and
/// `status`-only `unsupported` entries for every backend this build or
/// host cannot drive — both kinds must show up in the manifest, and the
/// real ones must point at a file that actually exists on disk.
#[test]
fn capture_writes_tui_svg_and_lists_it_in_the_manifest() {
    let dir = tempfile::tempdir().expect("create temp capture dir");

    let manifest = quadraui_gallery::capture::run_capture(
        dir.path(),
        quadraui_gallery::capture::CaptureTheme::Dark,
    )
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
    // Not just the sidebar row: "add severity" only appears in
    // `ToastDemo`'s own hint bar (same string `tests/gallery_driver.rs`
    // asserts on), so this proves the group→demo click sequence
    // actually navigated to the demo's content, not just that its name
    // is painted somewhere incidental (e.g. the sidebar list).
    assert!(
        svg.contains("add severity"),
        "captured SVG should show the demo's own content (ToastDemo's hint bar), \
         not just its sidebar label:\n{svg}"
    );

    // ── win: a real path only on a Windows host built with the `win`
    // feature (that leg's own coverage lives in `src/capture.rs`'s
    // `#[cfg(target_os = "windows")]` tests), otherwise a status-only
    // `unsupported` row — never a faked image.
    let win_entry = manifest
        .iter()
        .find(|e| e.backend == "win" && e.demo == demo.name() && e.variant == variant)
        .unwrap_or_else(|| {
            panic!(
                "manifest should list a win entry for {}/{variant}",
                demo.name()
            )
        });
    if cfg!(all(feature = "win", target_os = "windows")) {
        assert!(
            win_entry.status.is_none(),
            "win entry should have a real path when built with --features win on \
             a Windows host: {win_entry:#?}"
        );
    } else {
        assert_eq!(
            win_entry.status.as_deref(),
            Some("unsupported"),
            "win capture needs --features win on a Windows host, so elsewhere its \
             entry must be unsupported, not a path"
        );
        assert!(win_entry.path.is_none(), "win entry must not carry a path");
    }

    // ── gtk: a real path when this binary was built with the `gtk`
    // feature too (see `tests/capture_gtk_driver.rs` for that leg's own
    // dedicated coverage), otherwise a status-only `unsupported` row.
    let gtk_entry = manifest
        .iter()
        .find(|e| e.backend == "gtk" && e.demo == demo.name() && e.variant == variant)
        .unwrap_or_else(|| {
            panic!(
                "manifest should list a gtk entry for {}/{variant}",
                demo.name()
            )
        });
    if cfg!(feature = "gtk") {
        assert!(
            gtk_entry.status.is_none(),
            "gtk entry should have a real path when built with --features gtk: {gtk_entry:#?}"
        );
    } else {
        assert_eq!(
            gtk_entry.status.as_deref(),
            Some("unsupported"),
            "gtk entry without the `gtk` feature must be unsupported, not a path"
        );
        assert!(gtk_entry.path.is_none(), "gtk entry must not carry a path");
    }
}

/// Every manifest row names a demo/group/variant that really exists in
/// the registry — a table-driven guard against a future capture bug
/// that invents rows (or silently drops some) rather than mirroring
/// `registry()` exactly.
#[test]
fn every_manifest_row_matches_a_registered_demo_variant() {
    let dir = tempfile::tempdir().expect("create temp capture dir");
    let manifest = quadraui_gallery::capture::run_capture(
        dir.path(),
        quadraui_gallery::capture::CaptureTheme::Dark,
    )
    .expect("run_capture");

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
        // `tui` is always compiled into this test binary (the module's
        // own `#![cfg(feature = "tui")]`), so every `tui` row is a real
        // capture attempt, never an `"unsupported"` placeholder — a
        // `capture_action` whose needle isn't actually painted (a panic
        // caught by `run_capture`'s own `catch_unwind`) must not slip
        // through as a quiet `"error"` row for a demo this issue opens
        // an overlay on. Scoped to those demos rather than every row:
        // `click_demo_zone`'s own doc in `capture.rs` already names an
        // unrelated, pre-existing name-collision bug (clicking "Panel"
        // can select "Bottom Panel" first) that a blanket assertion
        // would also trip on.
        const OVERLAY_DEMOS: &[&str] = &[
            "Dialog",
            "Message Dialog",
            "Context Menu",
            "File Dialog",
            "Help Overlay",
            "Menu Bar",
        ];
        assert!(
            !(entry.backend == "tui"
                && OVERLAY_DEMOS.contains(&entry.demo.as_str())
                && entry.status.as_deref() == Some("error")),
            "tui row for {:?}/{:?} captured as an error: {entry:#?}",
            entry.demo,
            entry.variant
        );
    }
}

/// `Dialog`'s `capture_action` (`CaptureAction::ClickText("Open dialog")`)
/// must actually open the modal before the screenshot — the captured SVG
/// should show its button labels, not just the idle row list a capture
/// that only clicked the variant tab would leave behind.
#[test]
fn capture_opens_the_dialog_demo_instead_of_leaving_it_idle() {
    let dir = tempfile::tempdir().expect("create temp capture dir");
    let manifest = quadraui_gallery::capture::run_capture(
        dir.path(),
        quadraui_gallery::capture::CaptureTheme::Dark,
    )
    .expect("run_capture");

    let entry = manifest
        .iter()
        .find(|e| e.backend == "tui" && e.demo == "Dialog" && e.variant == "Confirm")
        .expect("manifest should list a tui Dialog/Confirm entry");
    assert!(
        entry.status.is_none(),
        "Dialog/Confirm should have captured cleanly: {entry:#?}"
    );
    let path = entry
        .path
        .as_deref()
        .expect("Dialog/Confirm should have a path");
    let svg = fs::read_to_string(dir.path().join(path)).expect("read captured SVG");
    assert!(
        svg.contains("OK") && svg.contains("Cancel"),
        "capturing Dialog/Confirm should show the opened dialog's own button \
         labels, not its idle row list:\n{svg}"
    );
}

/// `HelpOverlayDemo`'s `capture_action` (`CaptureAction::PressChar('?')`)
/// must actually open the cheatsheet before the screenshot — the status
/// bar's own hint flips from the idle `"? = open help..."` text to
/// `"Esc or ? to close"` once the overlay is open, so that string
/// appearing in the capture is proof the key press was handled, not
/// merely sent.
#[test]
fn capture_opens_the_help_overlay_demo_instead_of_leaving_it_idle() {
    let dir = tempfile::tempdir().expect("create temp capture dir");
    let manifest = quadraui_gallery::capture::run_capture(
        dir.path(),
        quadraui_gallery::capture::CaptureTheme::Dark,
    )
    .expect("run_capture");

    let entry = manifest
        .iter()
        .find(|e| e.backend == "tui" && e.demo == "Help Overlay" && e.variant == "Explorer")
        .expect("manifest should list a tui Help Overlay/Explorer entry");
    assert!(
        entry.status.is_none(),
        "Help Overlay/Explorer should have captured cleanly: {entry:#?}"
    );
    let path = entry
        .path
        .as_deref()
        .expect("Help Overlay/Explorer should have a path");
    let svg = fs::read_to_string(dir.path().join(path)).expect("read captured SVG");
    assert!(
        svg.contains("Esc or ? to close"),
        "capturing Help Overlay/Explorer should show the opened cheatsheet's \
         own close hint, not the idle \"? = open help\" prompt:\n{svg}"
    );
}

/// `MenuBarDemo`'s `capture_action` clicks the top-level `File`/`View`
/// label — the captured SVG should show that dropdown's own item labels,
/// not just the closed menu bar a capture that only clicked the variant
/// tab would leave behind.
#[test]
fn capture_opens_the_menu_bar_demo_instead_of_leaving_it_idle() {
    let dir = tempfile::tempdir().expect("create temp capture dir");
    let manifest = quadraui_gallery::capture::run_capture(
        dir.path(),
        quadraui_gallery::capture::CaptureTheme::Dark,
    )
    .expect("run_capture");

    for (variant, needle) in [
        ("Dropdown", "New File"),
        ("Submenu cascade", "Toggle Sidebar"),
    ] {
        let entry = manifest
            .iter()
            .find(|e| e.backend == "tui" && e.demo == "Menu Bar" && e.variant == variant)
            .unwrap_or_else(|| panic!("manifest should list a tui Menu Bar/{variant} entry"));
        assert!(
            entry.status.is_none(),
            "Menu Bar/{variant} should have captured cleanly: {entry:#?}"
        );
        let path = entry
            .path
            .as_deref()
            .unwrap_or_else(|| panic!("Menu Bar/{variant} should have a path"));
        let svg = fs::read_to_string(dir.path().join(path)).expect("read captured SVG");
        assert!(
            svg.contains(needle),
            "capturing Menu Bar/{variant} should show the opened dropdown's \
             own item {needle:?}, not the closed menu bar:\n{svg}"
        );
    }
}

/// `ContextMenuDemo`'s `capture_action` right-clicks the status bar's
/// idle hint — on TUI (no native OS menu) every `MenuStyle` variant
/// resolves to the painted `ContextMenuController` path, so the captured
/// SVG should show that menu's own item labels for all three variants.
#[test]
fn capture_opens_the_context_menu_demo_instead_of_leaving_it_idle() {
    let dir = tempfile::tempdir().expect("create temp capture dir");
    let manifest = quadraui_gallery::capture::run_capture(
        dir.path(),
        quadraui_gallery::capture::CaptureTheme::Dark,
    )
    .expect("run_capture");

    for variant in ["Auto", "Native", "Custom"] {
        let entry = manifest
            .iter()
            .find(|e| e.backend == "tui" && e.demo == "Context Menu" && e.variant == variant)
            .unwrap_or_else(|| panic!("manifest should list a tui Context Menu/{variant} entry"));
        assert!(
            entry.status.is_none(),
            "Context Menu/{variant} should have captured cleanly: {entry:#?}"
        );
        let path = entry
            .path
            .as_deref()
            .unwrap_or_else(|| panic!("Context Menu/{variant} should have a path"));
        let svg = fs::read_to_string(dir.path().join(path)).expect("read captured SVG");
        assert!(
            svg.contains("Select All"),
            "capturing Context Menu/{variant} should show the opened \
             painted menu's own item labels, not the idle status bar hint:\n{svg}"
        );
    }
}

/// `--theme light` (here, `CaptureTheme::Light` passed straight to
/// `run_capture`) renders every demo under `Theme::light_default()` and
/// records that on every manifest row's `theme` field — the default
/// (omitted) leg keeps recording `"dark"`, so this only changes what a
/// caller explicitly asks for.
#[test]
fn theme_flag_is_recorded_on_every_manifest_row_and_changes_the_painted_background() {
    let dark_dir = tempfile::tempdir().expect("create temp capture dir");
    let dark_manifest = quadraui_gallery::capture::run_capture(
        dark_dir.path(),
        quadraui_gallery::capture::CaptureTheme::Dark,
    )
    .expect("run_capture (dark)");
    assert!(
        dark_manifest.iter().all(|e| e.theme == "dark"),
        "every row of a Dark run must record theme == \"dark\": {dark_manifest:#?}"
    );

    let light_dir = tempfile::tempdir().expect("create temp capture dir");
    let light_manifest = quadraui_gallery::capture::run_capture(
        light_dir.path(),
        quadraui_gallery::capture::CaptureTheme::Light,
    )
    .expect("run_capture (light)");
    assert!(
        light_manifest.iter().all(|e| e.theme == "light"),
        "every row of a Light run must record theme == \"light\": {light_manifest:#?}"
    );

    let demos = registry();
    let demo = &demos[0];
    let variant = demo.variants()[0];
    let dark_entry = dark_manifest
        .iter()
        .find(|e| e.backend == "tui" && e.demo == demo.name() && e.variant == variant)
        .expect("dark tui entry");
    let light_entry = light_manifest
        .iter()
        .find(|e| e.backend == "tui" && e.demo == demo.name() && e.variant == variant)
        .expect("light tui entry");
    let dark_svg = fs::read_to_string(dark_dir.path().join(dark_entry.path.as_deref().unwrap()))
        .expect("read dark SVG");
    let light_svg = fs::read_to_string(light_dir.path().join(light_entry.path.as_deref().unwrap()))
        .expect("read light SVG");
    assert_ne!(
        dark_svg, light_svg,
        "a light-theme capture should paint different chrome colours than dark"
    );
}
