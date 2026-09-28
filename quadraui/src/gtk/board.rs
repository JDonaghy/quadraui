//! GTK rasteriser for [`crate::primitives::board::BoardModel`].
//!
//! Painting moved to the shared
//! [`crate::primitives::board::native_surface_paint::paint`] (#1085,
//! `NativeSurface` Phase 4 slice 8/8) — see that fn's doc for the three
//! named divergences (column-header overflow; card-title wrapping;
//! rounded vs. straight card borders) found while unifying
//! `gtk::board::draw_board`, `macos::board::draw_board` and
//! `win::board::draw_board` into one implementation. This module now
//! only carries [`gtk_board_layout`] (still real, backend-specific pure
//! geometry — no painting involved) and the deprecated [`draw_board`]
//! compatibility shim over the shared [`super::surface::CairoSurface`]
//! adapter (mirrors `gtk::diff_view`'s #866 shim).

use gtk4::cairo::Context;
use gtk4::pango;

use crate::primitives::board::{BoardLayout, BoardModel};
use crate::primitives::layout_metrics::pixel_board_layout;
use crate::theme::Theme;

/// Compute the GTK pixel-unit layout for a [`BoardModel`] without
/// painting. Shares its column/card measure with `mac_board_layout` /
/// `win_board_layout` via [`pixel_board_layout`] (issue #1079).
pub fn gtk_board_layout(model: &BoardModel, x: f64, y: f64, w: f64, h: f64) -> BoardLayout {
    pixel_board_layout(model, x as f32, y as f32, w as f32, h as f32)
}

/// Deprecated free-function shim (#1085, CLAUDE.md rule 8): reproduces
/// the pre-#1085 signature exactly for any external caller that held a
/// direct `quadraui::gtk::draw_board` reference rather than going through
/// [`crate::Backend::draw_board`] — the sanctioned entry point, and the
/// one every in-tree call site already uses, which is why this shim has
/// no in-repo caller left to trip the `-D warnings`-denied `deprecated`
/// lint.
#[deprecated(
    since = "0.0.1",
    note = "call `Backend::draw_board` instead — this free function is a compatibility shim over the shared #1085 implementation"
)]
#[allow(clippy::too_many_arguments)]
pub fn draw_board(
    cr: &Context,
    pango_layout: &pango::Layout,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    model: &BoardModel,
    theme: &Theme,
) -> BoardLayout {
    let mut surface = super::surface::CairoSurface {
        cr,
        layout: Some(pango_layout),
        translucent_fill: true,
    };
    crate::primitives::board::native_surface_paint::paint(
        model,
        &mut surface,
        theme,
        crate::event::Rect::new(x as f32, y as f32, w as f32, h as f32),
    )
}
