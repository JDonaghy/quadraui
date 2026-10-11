//! Proof that `PaintSurface` is genuinely public API.
//!
//! This file is a separate compilation unit, so anything it can name
//! and call is real public API, not an accidentally-`pub(crate)` item.
//!
//! Three things are asserted, one per backend family:
//!
//! 1. A free function generic over `&mut dyn quadraui::PaintSurface`
//!    type-checks here at all.
//! 2. [`quadraui::Backend::paint_surface`] answers `Some` on every pixel
//!    backend compiled in (GTK, Win-GUI), and the trait object it hands
//!    back dispatches to the same backend instance `Backend`'s own methods
//!    see (`surface_viewport` agrees with `Backend::viewport`).
//! 3. `Backend::paint_surface` answers `None` on `TuiBackend` — the
//!    structural absence `PaintSurface`'s own module doc names ("Why TUI
//!    stays out"), not a missing override.
//!
//! Calling an actual paint verb (`surface_fill_rect` et al.) needs a live
//! Cairo/Direct2D render target wired up through each backend's own
//! `attach_surface`-equivalent machinery — that round trip is covered by
//! `src/gtk/backend.rs` and `src/win/backend.rs` tests. This file's job is
//! narrower: prove the trait and its accessor are public.

#[cfg(any(feature = "tui", feature = "gtk", feature = "win"))]
use quadraui::Backend;
#[cfg(any(feature = "gtk", feature = "win"))]
use quadraui::{PaintSurface, Viewport};

/// Code outside this crate can write a function generic over
/// `&mut dyn PaintSurface`, the same shape every in-tree
/// `primitives::*::native_surface_paint::paint` helper uses.
/// `surface_viewport` is read-only and safe to call on any
/// backend in any state (no live Cairo/Direct2D context required).
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
