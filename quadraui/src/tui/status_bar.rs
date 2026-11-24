//! TUI entry point for [`crate::StatusBar`] — a thin delegate onto the
//! widget's own cell painter,
//! [`crate::primitives::status_bar::cell_paint::paint`], which documents
//! the background wash, per-segment tint and ellipsis rules. This module
//! only adapts a raw ratatui [`Buffer`] into the [`crate::CellSurface`]
//! that painter takes.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::cell_surface::TuiCellSurface;
use crate::backend::ColorDepth;
use crate::primitives::status_bar::{cell_paint, StatusBar, StatusBarLayout};
use crate::theme::Theme;
use crate::types::WidgetId;

/// Draw a [`StatusBar`] into `area` on `buf`. Returns hit regions in
/// **bar-local cell coordinates** (relative to `area.x`/`area.y`) for
/// each segment carrying an `action_id`. Caller dispatches clicks against
/// the returned list.
///
/// # Arguments
///
/// - `buf`, `area` — ratatui buffer + bar rect (single row).
/// - `bar` — the primitive description.
/// - `layout` — the resolved layout, computed by the caller in character
///   cells. [`crate::primitives::status_bar::cell_layout`] is that
///   measurement.
/// - `theme` — used only for the fallback fill colour when the bar has no
///   segments at all.
/// - `hovered_id`, `pressed_id` — tint the matching clickable segment.
pub fn draw_status_bar(
    buf: &mut Buffer,
    area: Rect,
    bar: &StatusBar,
    layout: &StatusBarLayout,
    theme: &Theme,
    hovered_id: Option<&WidgetId>,
    pressed_id: Option<&WidgetId>,
) -> StatusBarLayout {
    // The painter never moves the cursor, so the sink is never read.
    let mut cursor_sink = None;
    let mut surface = TuiCellSurface::new(buf, ColorDepth::TrueColor, &mut cursor_sink);
    cell_paint::paint(
        bar,
        layout,
        crate::event::Rect::new(
            area.x as f32,
            area.y as f32,
            area.width as f32,
            area.height as f32,
        ),
        &mut surface,
        theme,
        hovered_id,
        pressed_id,
    )
}
