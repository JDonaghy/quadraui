//! TUI entry point for [`crate::MenuBar`] — a thin delegate onto the
//! widget's own cell painter,
//! [`crate::primitives::menu_bar::cell_paint`], which documents the
//! strip's colouring and Alt-underline rules. This module only adapts a
//! raw ratatui [`Buffer`] into the [`crate::CellSurface`] that painter
//! takes, and the cell-unit [`MenuBarLayout`] it paints against.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::cell_surface::TuiCellSurface;
use crate::backend::ColorDepth;
use crate::primitives::menu_bar::{cell_paint, MenuBar, MenuBarLayout};
use crate::theme::Theme;

/// Compute the TUI cell-unit layout for a [`MenuBar`] without painting.
/// Consumer click routers call this to resolve mouse events against
/// the same layout the rasteriser used to paint.
pub fn tui_menu_bar_layout(bar: &MenuBar, area: Rect) -> MenuBarLayout {
    bar.cell_layout(cell_rect(area))
}

/// Draw a [`MenuBar`] into `area` on `buf`. Returns the layout for
/// host click dispatch via `layout.hit_test(x, y)`.
pub fn draw_menu_bar(buf: &mut Buffer, area: Rect, bar: &MenuBar, theme: &Theme) -> MenuBarLayout {
    let layout = tui_menu_bar_layout(bar, area);
    draw_menu_bar_layout(buf, bar, &layout, theme);
    layout
}

/// Draw a [`MenuBar`] at an already-resolved `layout` — the entry point
/// [`crate::primitive::Primitive::paint_cells`] reaches, since the
/// `Buffer` → [`crate::CellSurface`] adapter is private to this module's
/// parent.
pub(crate) fn draw_menu_bar_layout(
    buf: &mut Buffer,
    bar: &MenuBar,
    layout: &MenuBarLayout,
    theme: &Theme,
) {
    // The painter never moves the cursor, so the sink is never read.
    let mut cursor_sink = None;
    let mut surface = TuiCellSurface::new(buf, ColorDepth::TrueColor, &mut cursor_sink);
    cell_paint::paint(bar, layout, &mut surface, theme);
}

/// A ratatui cell `Rect` as the crate's own float geometry — one cell is
/// one unit.
fn cell_rect(area: Rect) -> crate::event::Rect {
    crate::event::Rect::new(
        area.x as f32,
        area.y as f32,
        area.width as f32,
        area.height as f32,
    )
}
