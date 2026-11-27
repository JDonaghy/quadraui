//! Direct2D entry point for [`crate::Chart`]'s DIP layout; layout and
//! painting both live with the widget (`primitives::chart`).

use crate::event::Rect;
use crate::primitives::chart::{Chart, ChartLayout};

/// Compute a [`Chart`]'s layout without painting.
pub fn win_chart_layout(
    chart: &Chart,
    rect: Rect,
    char_width: f32,
    line_height: f32,
) -> ChartLayout {
    chart.layout_in(rect, char_width, line_height)
}
