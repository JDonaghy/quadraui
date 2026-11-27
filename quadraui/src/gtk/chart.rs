//! GTK entry point for [`crate::Chart`]'s pixel layout; layout and
//! painting both live with the widget (`primitives::chart`).

use crate::primitives::chart::{Chart, ChartLayout};

/// Compute the GTK pixel-unit layout for a [`Chart`] without painting.
pub fn gtk_chart_layout(
    chart: &Chart,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    line_height: f64,
    char_width: f64,
) -> ChartLayout {
    let rect = crate::Rect::new(x as f32, y as f32, w as f32, h as f32);
    chart.layout_in(rect, char_width as f32, line_height as f32)
}
