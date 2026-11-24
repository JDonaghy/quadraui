//! TUI entry points for [`crate::Scrollbar`] — thin delegates onto the
//! widget's own cell painter,
//! `crate::primitives::scrollbar::cell_paint`, which documents the
//! glyphs each axis paints and what `cell_bg` controls. This module only
//! adapts a raw ratatui [`Buffer`] into the [`crate::CellSurface`] that
//! painter takes.

use ratatui::buffer::Buffer;

use super::cell_surface::TuiCellSurface;
use crate::backend::ColorDepth;
use crate::primitives::scrollbar::{cell_paint, Scrollbar, ScrollbarLayout};
use crate::theme::Theme;
use crate::types::Color;

/// Draw a [`Scrollbar`] into the buffer at its own
/// [`Scrollbar::track`].
///
/// The track and thumb are read from `scrollbar` directly — no math
/// happens here. `cell_bg` is the background behind both glyphs; see
/// the cell painter's own doc for which background each axis wants.
pub fn draw_scrollbar(buf: &mut Buffer, scrollbar: &Scrollbar, theme: &Theme, cell_bg: Color) {
    let layout = ScrollbarLayout::resolve(scrollbar, scrollbar.track);
    draw_scrollbar_layout(buf, &layout, theme, cell_bg);
}

/// Draw a [`Scrollbar`] into the track `layout` resolved.
///
/// `layout` must be the same [`ScrollbarLayout`] the host hit-tests
/// against so paint and click-routing can never disagree.
pub(crate) fn draw_scrollbar_layout(
    buf: &mut Buffer,
    layout: &ScrollbarLayout,
    theme: &Theme,
    cell_bg: Color,
) {
    // The painter never moves the cursor, so the sink is never read.
    let mut cursor_sink = None;
    let mut surface = TuiCellSurface::new(buf, ColorDepth::TrueColor, &mut cursor_sink);
    cell_paint::paint(layout, &mut surface, theme, cell_bg);
}
