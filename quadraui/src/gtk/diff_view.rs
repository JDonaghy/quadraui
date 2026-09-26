//! GTK rasteriser for [`crate::primitives::diff_view::DiffView`].
//!
//! Painting moved to the shared
//! [`crate::primitives::diff_view::native_surface_paint::paint`] (#866,
//! `NativeSurface` Phase 2d slice 9/9) — see that fn's doc for the two
//! named divergences (row/header text vertical alignment; header-label
//! ellipsize vs. hard-clip) found while unifying
//! `gtk::diff_view::draw_diff_view`, `macos::diff_view::draw_diff_view`
//! and `win::diff_view::draw_diff_view` into one implementation. This
//! module now only carries the deprecated [`draw_diff_view`]
//! compatibility shim over the shared [`super::surface::CairoSurface`]
//! adapter (#1072 — consolidated from this module's own private
//! `RawGtkDiffViewSurface`).

use gtk4::cairo::Context;
use gtk4::pango;

use crate::primitives::diff_view::{DiffView, DiffViewLayout};
use crate::theme::Theme;

/// Deprecated free-function shim (#866, CLAUDE.md rule 8): reproduces
/// the pre-#866 signature exactly for any external caller that held a
/// direct `quadraui::gtk::draw_diff_view` reference rather than going
/// through [`crate::Backend::draw_diff_view`] — the sanctioned entry
/// point, and the one every in-tree call site already uses, which is why
/// this shim has no in-repo caller left to trip the `-D
/// warnings`-denied `deprecated` lint.
#[deprecated(
    since = "0.0.1",
    note = "call `Backend::draw_diff_view` instead — this free function is a compatibility shim over the shared #866 implementation"
)]
#[allow(clippy::too_many_arguments)]
pub fn draw_diff_view(
    cr: &Context,
    layout: &pango::Layout,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    view: &DiffView,
    theme: &Theme,
    line_height: f64,
) -> DiffViewLayout {
    let mut surface = super::surface::CairoSurface {
        cr,
        layout: Some(layout),
        translucent_fill: true,
    };
    crate::primitives::diff_view::native_surface_paint::paint(
        view,
        &mut surface,
        theme,
        crate::event::Rect::new(x as f32, y as f32, w as f32, h as f32),
        line_height as f32,
    )
}
