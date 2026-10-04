//! GTK rasteriser for [`crate::primitives::board::BoardModel`].
//!
//! Painting moved to the shared
//! [`crate::primitives::board::native_surface_paint::paint`] (#1085,
//! `PaintSurface` Phase 4 slice 8/8) — see that fn's doc for the four
//! named divergences (column-header overflow; card-title wrapping;
//! rounded vs. straight card borders; per-element font size) found
//! while unifying `gtk::board::draw_board`, `macos::board::draw_board`
//! and `win::board::draw_board` into one implementation. This module now
//! only carries [`gtk_board_layout`] (still real, backend-specific pure
//! geometry — no painting involved); the deprecated `draw_board`
//! compatibility shim over the shared [`super::surface::CairoSurface`]
//! adapter was removed in issue #1109 (zero uses in coord-tui's `main`
//! and vimcode's `develop`).

use crate::primitives::board::{BoardLayout, BoardModel};
use crate::primitives::layout_metrics::pixel_board_layout;

/// Compute the GTK pixel-unit layout for a [`BoardModel`] without
/// painting. Shares its column/card measure with `mac_board_layout` /
/// `win_board_layout` via [`pixel_board_layout`] (issue #1079).
pub fn gtk_board_layout(model: &BoardModel, x: f64, y: f64, w: f64, h: f64) -> BoardLayout {
    pixel_board_layout(model, x as f32, y as f32, w as f32, h as f32)
}
