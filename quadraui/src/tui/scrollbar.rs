//! TUI entry points for [`crate::Scrollbar`] — thin delegates onto the
//! widget's own cell painter,
//! `crate::primitives::scrollbar::cell_paint`, which documents the
//! glyphs each axis paints and what `cell_bg` controls. This module only
//! resolves the bar's own track into the layout that painter takes.

use crate::cell_surface::CellSurface;
use crate::primitives::scrollbar::{cell_paint, Scrollbar, ScrollbarLayout};
use crate::theme::Theme;
use crate::types::Color;

/// Draw a [`Scrollbar`] onto `surface` at its own
/// [`Scrollbar::track`].
///
/// The track and thumb are read from `scrollbar` directly — no math
/// happens here. `cell_bg` is the background behind both glyphs; see
/// the cell painter's own doc for which background each axis wants.
pub fn draw_scrollbar(
    surface: &mut dyn CellSurface,
    scrollbar: &Scrollbar,
    theme: &Theme,
    cell_bg: Color,
) {
    let layout = ScrollbarLayout::resolve(scrollbar, scrollbar.track);
    cell_paint::paint(&layout, surface, theme, cell_bg);
}
