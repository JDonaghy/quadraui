//! TUI entry point for [`crate::DatePicker`] — a thin delegate onto the
//! widget's own cell painter,
//! `crate::primitives::date_picker::cell_paint`, which documents the
//! header, day-grid and time-field rules. This module only adapts a raw
//! ratatui [`Buffer`] into the [`crate::CellSurface`] that painter takes.

use ratatui::buffer::Buffer;

use super::cell_surface::TuiCellSurface;
use crate::backend::ColorDepth;
use crate::primitives::date_picker::{cell_paint, DatePicker, DatePickerLayout};
use crate::theme::Theme;

/// Draw a [`DatePicker`] at its resolved `layout`. `layout` must be the
/// same [`DatePickerLayout`] the host hit-tests against, so paint and
/// click-routing can never disagree about which day sits where.
pub fn draw_date_picker(
    buf: &mut Buffer,
    date_picker: &DatePicker,
    layout: &DatePickerLayout,
    theme: &Theme,
) {
    // The painter never moves the cursor, so the sink is never read.
    let mut cursor_sink = None;
    let mut surface = TuiCellSurface::new(buf, ColorDepth::TrueColor, &mut cursor_sink);
    cell_paint::paint(date_picker, layout, &mut surface, theme);
}
