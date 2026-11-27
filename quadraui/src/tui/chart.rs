//! TUI entry points for [`crate::Chart`]: the cell layout and the cell
//! painter both live with the widget (`primitives::chart`); these keep
//! the backend's existing `draw_chart`/`chart_layout` paths reaching them.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use crate::primitives::chart::{Chart, ChartLayout};
use crate::theme::Theme;

/// Compute the TUI cell-unit layout for a [`Chart`] without painting.
pub fn tui_chart_layout(chart: &Chart, area: Rect) -> ChartLayout {
    let rect = crate::Rect::new(
        area.x as f32,
        area.y as f32,
        area.width as f32,
        area.height as f32,
    );
    chart.layout_in(rect, 1.0, 1.0)
}

/// Draw a [`Chart`] into `area` on `buf`, with `hovered_point` marked and
/// a crosshair at data index `crosshair_x`. Returns the layout for host
/// click dispatch.
pub fn draw_chart(
    buf: &mut Buffer,
    area: Rect,
    chart: &Chart,
    theme: &Theme,
    hovered_point: Option<(usize, usize)>,
    crosshair_x: Option<f64>,
) -> ChartLayout {
    let layout = tui_chart_layout(chart, area);
    crate::primitives::chart::paint_cells::paint(
        chart,
        &layout,
        &mut super::BufferCellSurface::new(buf),
        theme,
        hovered_point,
        crosshair_x,
    );
    layout
}
