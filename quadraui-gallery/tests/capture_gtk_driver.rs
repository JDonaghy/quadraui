//! GTK leg of the headless capture mode integration test — the
//! `tests/capture_driver.rs` (TUI) and `tests/capture_macos_driver.rs`
//! twin, scoped to `--features gtk`. Needs no display server: the GTK
//! backend's own headless `GtkDriver` (a Cairo `ImageSurface`) runs on
//! any host with the GTK4 system libraries installed, including a
//! plain Linux CI runner.
#![cfg(feature = "gtk")]

use std::fs;

use quadraui_gallery::registry::registry;

/// `run_capture`'s GTK leg writes a real PNG (via the headless
/// `GtkDriver` → `image` crate path) for every (demo, variant), and
/// lists it in the manifest with a `path`, not a `status`.
#[test]
fn capture_writes_gtk_png_and_lists_it_in_the_manifest() {
    let dir = tempfile::tempdir().expect("create temp capture dir");
    let manifest = quadraui_gallery::capture::run_capture(dir.path()).expect("run_capture");

    let demos = registry();
    let demo = &demos[0];
    let variant = demo.variants()[0];

    let entry = manifest
        .iter()
        .find(|e| e.backend == "gtk" && e.demo == demo.name() && e.variant == variant)
        .unwrap_or_else(|| {
            panic!(
                "manifest should list a gtk entry for {}/{variant}",
                demo.name()
            )
        });
    assert!(
        entry.status.is_none(),
        "gtk entry built with the `gtk` feature should have a path, not a status"
    );
    let path = entry
        .path
        .as_deref()
        .expect("gtk entry should carry a path");

    let png_path = dir.path().join(path);
    assert!(
        png_path.is_file(),
        "{} should exist on disk",
        png_path.display()
    );
    let bytes = fs::read(&png_path).expect("read captured PNG");
    assert!(!bytes.is_empty(), "captured PNG must not be empty");
    // PNG magic bytes — confirms the `image` crate actually wrote a PNG,
    // not just some non-empty file.
    assert_eq!(
        &bytes[..8],
        b"\x89PNG\r\n\x1a\n",
        "captured file must be a real PNG"
    );
}
