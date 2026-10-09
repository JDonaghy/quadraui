//! macOS leg of the headless capture mode integration test (#1348) —
//! the `tests/capture_driver.rs` twin, scoped to the backend that needs
//! a real macOS host plus `--features macos` to exercise at all (see
//! `Cargo.toml`'s `macos` feature comment for why it's `target_os`-gated
//! in full).
#![cfg(all(feature = "macos", target_os = "macos"))]

use std::fs;

use quadraui_gallery::registry::registry;

/// `run_capture`'s macOS leg writes a real PNG (via the headless
/// `BitmapSurface` → `image` crate path) for every (demo, variant), and
/// lists it in the manifest with a `path`, not a `status`.
#[test]
fn capture_writes_macos_png_and_lists_it_in_the_manifest() {
    let dir = tempfile::tempdir().expect("create temp capture dir");
    let manifest = quadraui_gallery::capture::run_capture(dir.path()).expect("run_capture");

    let demos = registry();
    let demo = &demos[0];
    let variant = demo.variants()[0];

    let entry = manifest
        .iter()
        .find(|e| e.backend == "macos" && e.demo == demo.name() && e.variant == variant)
        .unwrap_or_else(|| {
            panic!(
                "manifest should list a macos entry for {}/{variant}",
                demo.name()
            )
        });
    assert!(
        entry.status.is_none(),
        "macos entry on a real mac host should have a path, not a status"
    );
    let path = entry
        .path
        .as_deref()
        .expect("macos entry should carry a path");

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
