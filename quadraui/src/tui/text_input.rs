//! TUI entry points for [`crate::TextInput`] — thin delegates onto the
//! widget's own cell painter,
//! `crate::primitives::text_input::cell_paint`, which documents the
//! border chrome, the placeholder/line rows and the block caret. This
//! module only adapts a raw ratatui [`Buffer`] into the
//! [`crate::CellSurface`] that painter takes.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::cell_surface::TuiCellSurface;
use crate::backend::ColorDepth;
use crate::primitives::text_input::{cell_layout, cell_paint, TextInput, TextInputLayout};
use crate::theme::Theme;

/// Compute the TUI text-input layout using cell metrics.
///
/// Delegates to [`crate::primitives::text_input::cell_layout`], the one
/// cell-units layout every cell-grid caller shares, so paint and
/// hit-test agree on every row and caret column.
pub fn tui_text_input_layout(ti: &TextInput, rect: Rect) -> TextInputLayout {
    cell_layout(
        ti,
        crate::event::Rect::new(
            rect.x as f32,
            rect.y as f32,
            rect.width as f32,
            rect.height as f32,
        ),
    )
}

/// Draw a [`TextInput`] into `rect` on `buf` and return the layout its
/// glyphs were placed with, so callers never re-derive it and risk it
/// drifting from what was painted.
///
/// Also what [`crate::primitive::Primitive::paint_cells`] reaches the
/// cell painter through: the `Buffer` → [`crate::CellSurface`] adapter is
/// private to this module's parent, and [`cell_layout`] resolves the same
/// layout from the same `rect` either way.
pub fn draw_text_input(
    buf: &mut Buffer,
    rect: Rect,
    ti: &TextInput,
    theme: &Theme,
) -> TextInputLayout {
    let layout = tui_text_input_layout(ti, rect);
    // The painter never moves the terminal cursor, so the sink is never
    // read.
    let mut cursor_sink = None;
    let mut surface = TuiCellSurface::new(buf, ColorDepth::TrueColor, &mut cursor_sink);
    cell_paint::paint(ti, &layout, &mut surface, theme);
    layout
}
