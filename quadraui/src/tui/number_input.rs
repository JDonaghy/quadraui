//! TUI entry point for [`crate::NumberInput`] — a thin delegate onto the
//! widget's own cell painter,
//! `crate::primitives::number_input::cell_paint`, which documents the
//! border, text and stepper rules. This module only adapts a raw ratatui
//! [`Buffer`] into the [`crate::CellSurface`] that painter takes.

use ratatui::buffer::Buffer;

use super::cell_surface::TuiCellSurface;
use crate::backend::ColorDepth;
use crate::primitives::number_input::{cell_paint, NumberInput, NumberInputLayout};
use crate::theme::Theme;

/// Draw a [`NumberInput`] at its resolved layout. `layout` must be the
/// same [`NumberInputLayout`] the host hit-tests against, so paint and
/// click-routing can never disagree about where the stepper cells are.
pub fn draw_number_input(
    buf: &mut Buffer,
    number_input: &NumberInput,
    layout: &NumberInputLayout,
    theme: &Theme,
) {
    // The painter never moves the cursor, so the sink is never read.
    let mut cursor_sink = None;
    let mut surface = TuiCellSurface::new(buf, ColorDepth::TrueColor, &mut cursor_sink);
    cell_paint::paint(number_input, layout, &mut surface, theme);
}
