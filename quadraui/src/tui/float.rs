//! TUI entry point for [`crate::Float`] — a thin delegate onto the
//! widget's own cell painter, `crate::primitives::float::cell_paint`,
//! which documents the chrome it paints. This module only adapts a raw
//! ratatui [`Buffer`] into the [`crate::CellSurface`] that painter takes.

use ratatui::buffer::Buffer;

use super::cell_surface::TuiCellSurface;
use crate::backend::ColorDepth;
use crate::primitives::float::{cell_paint, Float, FloatLayout};
use crate::theme::Theme;

/// Draw a [`Float`]'s chrome into `buf` at `layout.bounds`. `layout`
/// must be the same [`FloatLayout`] the host uses for hit-testing
/// (typically the value `float.layout(viewport, measure)` returned) so
/// paint and click-routing can never disagree.
pub fn draw_float(buf: &mut Buffer, float: &Float, layout: &FloatLayout, theme: &Theme) {
    // The painter never moves the cursor, so the sink is never read.
    let mut cursor_sink = None;
    let mut surface = TuiCellSurface::new(buf, ColorDepth::TrueColor, &mut cursor_sink);
    cell_paint::paint(float, layout, &mut surface, theme);
}
