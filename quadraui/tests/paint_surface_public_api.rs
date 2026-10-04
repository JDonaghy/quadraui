//! Compiles-as-an-external-crate proof that `PaintSurface` (issue #1101) is
//! genuinely unsealed — this file is a separate compilation unit from
//! `quadraui`'s own `src/`, the same position `coord-tui`/`vimcode` are in,
//! so anything it can name and call is real public API, not an
//! accidentally-`pub(crate)`-adjacent item only reachable from in-tree
//! `#[cfg(test)]` modules.
//!
//! Three things are asserted, one per backend family:
//!
//! 1. A free function generic over `&mut dyn quadraui::PaintSurface`
//!    type-checks here at all — before #1101 this trait was
//!    `pub(crate)`, so this file could not even name it.
//! 2. [`quadraui::Backend::paint_surface`] answers `Some` on every pixel
//!    backend compiled in (today: GTK, Win-GUI — `MacBackend` only builds
//!    under `target_os = "macos"`, covered by `macos_appkit_features.rs`'s
//!    sibling pattern, not here), and the trait object it hands back
//!    dispatches to the same backend instance `Backend`'s own methods see
//!    (`surface_viewport` agrees with `Backend::viewport`).
//! 3. `Backend::paint_surface` answers `None` on `TuiBackend` — the
//!    structural absence `PaintSurface`'s own module doc names ("Why TUI
//!    stays out"), not a missing override.
//!
//! Calling an actual paint verb (`surface_fill_rect` et al.) needs a live
//! Cairo/Direct2D render target wired up through each backend's own
//! `attach_surface`-equivalent, crate-private machinery this file (an
//! external-crate position) cannot reach — that round trip is already
//! covered, per backend, by `src/gtk/backend.rs`'s and `src/win/backend.rs`'s
//! own `#[cfg(test)] mod tests`. This file's job is narrower and
//! complementary: prove the *seam* (the trait, and the accessor that reaches
//! it) is public, not that every verb paints correctly.

use quadraui::Backend;
#[cfg(any(feature = "gtk", feature = "win"))]
use quadraui::{PaintSurface, Viewport};

/// Exercises exactly the capability #1101 adds: code outside this crate
/// writing a function generic over `&mut dyn PaintSurface`, the same shape
/// every in-tree `primitives::*::native_surface_paint::paint` helper
/// already uses. `surface_viewport` is read-only and safe to call on any
/// backend in any state (no live Cairo/Direct2D context required), unlike
/// the fill/stroke/text verbs — see this file's module doc for why those
/// stay out of scope here.
#[cfg(any(feature = "gtk", feature = "win"))]
fn read_viewport_through_the_seam(surface: &mut dyn PaintSurface) -> Viewport {
    surface.surface_viewport()
}

#[cfg(feature = "tui")]
#[test]
fn tui_backend_has_no_paint_surface_by_structural_design() {
    use quadraui::tui::backend::TuiBackend;

    let mut backend = TuiBackend::new();
    assert!(
        backend.paint_surface().is_none(),
        "TuiBackend must answer None from Backend::paint_surface — a cell grid has no \
         sub-cell Rect to paint into (PaintSurface's module doc, \"Why TUI stays out\"); \
         see tests/conformance/caps.rs's ACCEPTED_DEFAULTS entry for the same claim enforced \
         against silent regressions"
    );
}

#[cfg(feature = "gtk")]
#[test]
fn gtk_backend_paint_surface_is_public_and_reachable() {
    use quadraui::gtk::backend::GtkBackend;

    let mut backend = GtkBackend::new();
    let expected = Backend::viewport(&backend);
    let surface = backend
        .paint_surface()
        .expect("GtkBackend implements PaintSurface directly, so this must be Some");
    assert_eq!(
        read_viewport_through_the_seam(surface),
        expected,
        "the PaintSurface trait object Backend::paint_surface hands back must be the same \
         GtkBackend instance Backend's own viewport() reads from"
    );
}

#[cfg(feature = "win")]
#[test]
fn win_backend_paint_surface_is_public_and_reachable() {
    use quadraui::win::backend::WinBackend;

    let mut backend = WinBackend::new();
    let expected = Backend::viewport(&backend);
    let surface = backend
        .paint_surface()
        .expect("WinBackend implements PaintSurface directly, so this must be Some");
    assert_eq!(
        read_viewport_through_the_seam(surface),
        expected,
        "the PaintSurface trait object Backend::paint_surface hands back must be the same \
         WinBackend instance Backend's own viewport() reads from"
    );
}
