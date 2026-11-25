//! TUI entry points for [`crate::ToastOverlay`] — thin delegates onto the
//! widget's own cell painter, `crate::primitives::toast::cell_paint`,
//! which documents the box chrome, title/body wrapping, dismiss glyph
//! and action row. This module only adapts a raw ratatui [`Buffer`] into
//! the [`crate::CellSurface`] that painter takes.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::cell_surface::TuiCellSurface;
use crate::backend::ColorDepth;
use crate::primitives::toast::{
    cell_paint, cell_toast_stack_layout, ToastOverlay, ToastStackLayout,
};
use crate::theme::Theme;

/// The overlay area as a quadraui [`crate::Rect`], the unit
/// [`cell_toast_stack_layout`] and the cell painter both work in.
fn q_area(area: Rect) -> crate::event::Rect {
    crate::event::Rect::new(
        area.x as f32,
        area.y as f32,
        area.width as f32,
        area.height as f32,
    )
}

/// Compute the TUI cell-unit layout for a [`ToastOverlay`] without painting.
///
/// `area`'s origin is baked into the returned bounds (absolute buffer
/// coordinates, matching `tui_menu_bar_layout`) —
/// hosts call `layout.hit_test(x, y)` with raw click coordinates, no
/// localisation needed.
pub fn tui_toast_stack_layout(stack: &ToastOverlay, area: Rect) -> ToastStackLayout {
    cell_toast_stack_layout(stack, q_area(area))
}

/// Draw a [`ToastOverlay`] overlay onto `buf`. Returns the layout for
/// host click dispatch.
pub fn draw_toast_stack(
    buf: &mut Buffer,
    area: Rect,
    stack: &ToastOverlay,
    theme: &Theme,
) -> ToastStackLayout {
    let clip = q_area(area);
    let layout = cell_toast_stack_layout(stack, clip);
    // The painter never moves the cursor, so the sink is never read.
    let mut cursor_sink = None;
    let mut surface = TuiCellSurface::new(buf, ColorDepth::TrueColor, &mut cursor_sink);
    cell_paint::paint(stack, &layout, &mut surface, theme, clip);
    layout
}
