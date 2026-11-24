//! TUI entry point for [`crate::RichTextPopup`] — a thin delegate onto
//! the widget's own cell painter,
//! `crate::primitives::rich_text_popup::cell_paint`, which documents the
//! box chrome, selection inversion, focused-link underline and scrollbar
//! it paints. This module only adapts a raw ratatui [`Buffer`] into the
//! [`crate::CellSurface`] that painter takes.

use ratatui::buffer::Buffer;

use super::cell_surface::TuiCellSurface;
use crate::backend::ColorDepth;
use crate::primitives::rich_text_popup::{cell_paint, RichTextPopup, RichTextPopupLayout};
use crate::theme::Theme;

/// Draw a [`RichTextPopup`] into the buffer at its resolved layout.
/// `layout` must be the same [`RichTextPopupLayout`] the host hit-tests
/// against, so paint and click-routing can never disagree.
pub fn draw_rich_text_popup(
    buf: &mut Buffer,
    popup: &RichTextPopup,
    layout: &RichTextPopupLayout,
    theme: &Theme,
) {
    // The painter never moves the cursor, so the sink is never read.
    let mut cursor_sink = None;
    let mut surface = TuiCellSurface::new(buf, ColorDepth::TrueColor, &mut cursor_sink);
    cell_paint::paint(popup, layout, &mut surface, theme);
}
