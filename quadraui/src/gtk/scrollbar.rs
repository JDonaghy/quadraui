//! GTK rasteriser for [`crate::Scrollbar`].
//!
//! Painting moved to the shared
//! [`crate::primitives::scrollbar::native_surface_paint::paint`] (#811,
//! `NativeSurface` Phase 2d) — see that fn's doc for the one named
//! divergence (quadraui#791) re-verified (already fixed) while unifying
//! `gtk::draw_scrollbar`, `macos::scrollbar::draw_scrollbar` and
//! `win::scrollbar::draw_scrollbar` into one implementation. This module
//! now only carries the deprecated [`draw_scrollbar`] compatibility shim
//! over the shared [`super::surface::CairoSurface`] adapter (#1072 —
//! consolidated from this module's own private `RawScrollbarSurface`,
//! used by any free-function rasteriser too (`gtk::data_table`,
//! `gtk::list`) that paints an embedded scrollbar from only a
//! `cr: &Context` — not a live `GtkBackend`).

use gtk4::cairo::Context;

use crate::primitives::scrollbar::Scrollbar;
use crate::theme::Theme;

/// Deprecated free-function shim (#811, CLAUDE.md rule 8): reproduces
/// the pre-#811 signature exactly for any external caller that held a
/// direct `quadraui::gtk::draw_scrollbar` reference rather than going
/// through [`crate::Backend::draw_scrollbar`] — the sanctioned entry
/// point, and the one every in-tree call site already uses, which is
/// why this shim has no in-repo caller left to trip the `-D
/// warnings`-denied `deprecated` lint.
#[deprecated(
    since = "0.0.1",
    note = "call `Backend::draw_scrollbar` instead — this free function is a compatibility shim over the shared #811 implementation"
)]
pub fn draw_scrollbar(cr: &Context, scrollbar: &Scrollbar, theme: &Theme) {
    // `translucent_fill: true` — a scrollbar's entire visual identity is
    // a translucent overlay (see `super::surface`'s module doc /
    // quadraui#791), so the fill must honour `color.a`.
    let mut surface = super::surface::CairoSurface {
        cr,
        layout: None,
        translucent_fill: true,
    };
    crate::primitives::scrollbar::native_surface_paint::paint(scrollbar, &mut surface, theme);
}
