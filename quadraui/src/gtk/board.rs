//! GTK rasteriser for [`crate::primitives::board::BoardModel`].
//!
//! Painting is implemented in the shared
//! [`crate::primitives::board::native_surface_paint::paint`] — see that fn's doc for the
//! four named divergences (column-header overflow; card-title wrapping;
//! rounded vs. straight card borders; per-element font size) found
//! while unifying `gtk::board::draw_board`, `macos::board::draw_board`
//! and `win::board::draw_board` into one implementation. This module now
//! only carries [`gtk_board_layout`] (backend-specific pure geometry — no
//! painting involved).

use crate::primitives::board::{BoardLayout, BoardModel};
use crate::primitives::layout_metrics::pixel_board_layout;

/// Compute the GTK pixel-unit layout for a [`BoardModel`] without
/// painting. Shares its column/card measure with `mac_board_layout` /
/// `win_board_layout` via [`pixel_board_layout`].
pub fn gtk_board_layout(model: &BoardModel, x: f64, y: f64, w: f64, h: f64) -> BoardLayout {
    pixel_board_layout(model, x as f32, y as f32, w as f32, h as f32)
}
