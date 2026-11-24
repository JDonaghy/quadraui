//! TUI entry points for [`crate::TextDisplay`] — thin delegates onto the
//! widget's own cell painter,
//! `crate::primitives::text_display::cell_paint`, which documents the
//! title row, the per-line spans/timestamp, the wrap continuation marker
//! and the scrollbar gutter. This module only adapts a raw ratatui
//! [`Buffer`] into the [`crate::CellSurface`] that painter takes.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::cell_surface::TuiCellSurface;
use crate::backend::ColorDepth;
use crate::primitives::text_display::{cell_layout, cell_paint, TextDisplay, TextDisplayLayout};
use crate::theme::Theme;

/// Draw a [`TextDisplay`] into `area` on `buf`.
///
/// Also what [`crate::primitive::Primitive::paint_cells`] reaches the
/// cell painter through: the `Buffer` → [`crate::CellSurface`] adapter is
/// private to this module's parent, and [`cell_layout`] resolves the same
/// layout from the same `area` either way.
pub fn draw_text_display(buf: &mut Buffer, area: Rect, display: &TextDisplay, theme: &Theme) {
    let resolved = cell_layout(display, q_rect(area));
    // The painter never moves the terminal cursor, so the sink is never
    // read.
    let mut cursor_sink = None;
    let mut surface = TuiCellSurface::new(buf, ColorDepth::TrueColor, &mut cursor_sink);
    cell_paint::paint(display, &resolved, &mut surface, theme);
}

/// Compute the text-display layout using TUI-native metrics (1 cell per
/// line, 1-cell scrollbar gutter). Consumers call this to drive
/// hit-testing for scrollbar drag interaction without re-deriving
/// metrics.
///
/// Delegates to [`crate::primitives::text_display::cell_layout`], the one
/// cell-units layout every cell-grid caller shares, so paint and
/// hit-test agree on every row. The returned layout's coordinates are
/// **body-local** (`y = 0` at the top of the body, below any title row),
/// the same contract the pixel backends' own helpers carry.
pub fn tui_text_display_layout(display: &TextDisplay, area: Rect) -> TextDisplayLayout {
    cell_layout(display, q_rect(area)).layout
}

/// A ratatui cell rect as the quadraui [`crate::Rect`] the widget module
/// lays out against.
fn q_rect(area: Rect) -> crate::event::Rect {
    crate::event::Rect::new(
        area.x as f32,
        area.y as f32,
        area.width as f32,
        area.height as f32,
    )
}
