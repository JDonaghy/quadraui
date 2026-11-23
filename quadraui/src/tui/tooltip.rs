//! TUI entry points for [`crate::Tooltip`] — thin delegates onto the
//! widget's own cell painter, `crate::primitives::tooltip::cell_paint`,
//! which documents the border chrome and content rules. This module only
//! adapts a raw ratatui [`Buffer`] into the [`crate::CellSurface`] that
//! painter takes.

use ratatui::buffer::Buffer;

use super::cell_surface::TuiCellSurface;
use crate::backend::ColorDepth;
use crate::event::Rect as QRect;
use crate::primitives::tooltip::{cell_paint, Tooltip, TooltipChrome, TooltipLayout};
use crate::theme::Theme;

/// The whole-cell bounds [`draw_tooltip`] actually paints into —
/// `layout.bounds` rounded to integer terminal columns/rows.
///
/// Backends must register *this* (not the raw float `layout.bounds`) as
/// the tooltip's zone via `Backend::register_zone`, so a
/// structural-parity observer sees a surface that matches the cells
/// actually painted.
pub fn painted_bounds(layout: &TooltipLayout) -> QRect {
    cell_paint::painted_bounds(layout)
}

/// Draw a [`Tooltip`] into `layout.bounds` on `buf` with the default
/// chrome — a [`crate::TooltipBorder::Full`] box, no title.
///
/// To ask for different chrome, call [`draw_tooltip_with_chrome`].
pub fn draw_tooltip(buf: &mut Buffer, tooltip: &Tooltip, layout: &TooltipLayout, theme: &Theme) {
    draw_tooltip_with_chrome(buf, tooltip, layout, &TooltipChrome::default(), theme);
}

/// Draw a [`Tooltip`] into `layout.bounds` on `buf`, with the border and
/// optional title requested by `chrome`.
pub fn draw_tooltip_with_chrome(
    buf: &mut Buffer,
    tooltip: &Tooltip,
    layout: &TooltipLayout,
    chrome: &TooltipChrome,
    theme: &Theme,
) {
    // The painter never moves the cursor, so the sink is never read.
    let mut cursor_sink = None;
    let mut surface = TuiCellSurface::new(buf, ColorDepth::TrueColor, &mut cursor_sink);
    cell_paint::paint(tooltip, layout, chrome, &mut surface, theme);
}
