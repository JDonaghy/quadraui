//! End-to-end test for the `tui_codicon_tree` example, driven in-process
//! by [`TuiDriver`] — see `tests/tui_example_driver.rs`'s module doc for
//! why this pulls in the example's own `AppLogic` source via `#[path]`
//! rather than a test-only copy.
//!
//! Kept in its own file rather than folded into `tui_example_driver.rs`
//! to avoid colliding with concurrent edits to that shared file.

#![cfg(feature = "tui")]

use quadraui::tui::testing::TuiDriver;

#[path = "../examples/common/codicon_tree_demo.rs"]
mod codicon_tree_demo;
use codicon_tree_demo::CodiconTreeDemo;

/// With nerd fonts on (the demo's default), rows paint the bundled
/// codicon glyph — a non-ASCII PUA codepoint — rather than the plain
/// fallback text, and every row label is still visible.
#[test]
fn codicon_tree_paints_glyphs_when_nerd_fonts_on() {
    let driver = TuiDriver::new(CodiconTreeDemo::new(), 40, 10);
    assert!(driver.app().nerd_fonts_enabled());
    let screen = driver.screen();
    for label in ["quadraui", "src", "types.rs", "Cargo.toml", "README.md"] {
        assert!(
            screen.contains(label),
            "row label {label:?} should be visible:\n{screen}"
        );
    }
    assert!(
        !screen.contains("DIR") && !screen.contains("TOML") && !screen.contains("MD"),
        "nerd-fonts-on should paint codicon glyphs, not the ASCII fallbacks:\n{screen}"
    );
}

/// `n` toggles nerd fonts off: the same rows now paint each icon's
/// plain-text fallback instead of the codicon glyph — the TUI-degrades
/// path every `Icon` goes through (see `Icon::resolve`'s doc).
#[test]
fn codicon_tree_falls_back_to_plain_text_when_nerd_fonts_off() {
    let mut driver = TuiDriver::new(CodiconTreeDemo::new(), 40, 10);
    driver.type_char('n');
    assert!(!driver.app().nerd_fonts_enabled());

    let screen = driver.screen();
    assert!(
        screen.contains("DIR"),
        "folder rows should fall back to the \"DIR\" text:\n{screen}"
    );
    assert!(
        screen.contains("RS"),
        "file-code rows should fall back to the \"RS\" text:\n{screen}"
    );
    for label in ["quadraui", "src", "types.rs", "Cargo.toml", "README.md"] {
        assert!(
            screen.contains(label),
            "row label {label:?} should still be visible:\n{screen}"
        );
    }
}
