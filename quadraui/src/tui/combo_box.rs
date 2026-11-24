//! TUI entry point for [`crate::ComboBox`] — a thin delegate onto the
//! widget's own cell painter, `crate::primitives::combo_box::cell_paint`,
//! which documents the field chrome and dropdown row rules. This module
//! only adapts a raw ratatui [`Buffer`] into the [`crate::CellSurface`]
//! that painter takes.

use ratatui::buffer::Buffer;

use super::cell_surface::TuiCellSurface;
use crate::backend::ColorDepth;
use crate::primitives::combo_box::{cell_paint, ComboBox, ComboBoxLayout};
use crate::theme::Theme;

/// Draw a [`ComboBox`] at its resolved layout. `layout` must be the same
/// [`ComboBoxLayout`] the host hit-tests against, so paint and
/// click-routing can never disagree about which dropdown row is where.
pub fn draw_combo_box(
    buf: &mut Buffer,
    combo_box: &ComboBox,
    layout: &ComboBoxLayout,
    theme: &Theme,
) {
    // The painter never moves the cursor, so the sink is never read.
    let mut cursor_sink = None;
    let mut surface = TuiCellSurface::new(buf, ColorDepth::TrueColor, &mut cursor_sink);
    cell_paint::paint(combo_box, layout, &mut surface, theme);
}
