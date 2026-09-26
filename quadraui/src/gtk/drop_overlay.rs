//! GTK rasteriser for [`crate::DropOverlay`].
//!
//! Painting moved to the shared
//! [`crate::primitives::drop_zone::native_surface_paint::paint`] (#865,
//! `NativeSurface` Phase 2d slice 8/9) — see that fn's doc for the one
//! named divergence (Windows's pre-migration CPU-premixed highlight,
//! unlike GTK/macOS's real alpha blend) found and reported while
//! unifying `gtk::draw_drop_overlay`, `macos::drop_overlay::
//! draw_drop_overlay` and `win::drop_overlay::draw_drop_overlay` into
//! one implementation. This module now only carries the deprecated
//! [`draw_drop_overlay`] compatibility shim over the shared
//! [`super::surface::CairoSurface`] adapter (#1072 — consolidated from
//! this module's own private `RawDropOverlaySurface`).

use gtk4::cairo::Context;

use crate::primitives::drop_zone::DropOverlay;
use crate::theme::Theme;

/// Deprecated free-function shim (#865, CLAUDE.md rule 8): reproduces
/// the pre-#865 signature exactly for any external caller that held a
/// direct `quadraui::gtk::draw_drop_overlay` reference rather than going
/// through [`crate::Backend::draw_drop_overlay`] — the sanctioned entry
/// point, and the one every in-tree call site already uses, which is
/// why this shim has no in-repo caller left to trip the `-D
/// warnings`-denied `deprecated` lint.
#[deprecated(
    since = "0.0.1",
    note = "call `Backend::draw_drop_overlay` instead — this free function is a compatibility shim over the shared #865 implementation"
)]
pub fn draw_drop_overlay(cr: &Context, overlay: &DropOverlay, theme: &Theme) {
    // `translucent_fill: true` — the highlight rect is a translucent
    // overlay, so the fill must honour `color.a`.
    let mut surface = super::surface::CairoSurface {
        cr,
        layout: None,
        translucent_fill: true,
    };
    crate::primitives::drop_zone::native_surface_paint::paint(overlay, &mut surface, theme);
}
