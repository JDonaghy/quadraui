//! macOS layout helper for [`crate::Chart`].
//!
//! Painting is implemented in the shared [`crate::primitives::chart::paint`].
//! This module carries [`mac_chart_layout`], the pixel-unit layout query
//! used for both hit-testing (`MacBackend::chart_layout`) and painting
//! (`MacBackend::draw_chart`, via `Backend::chart_layout`).

use crate::primitives::chart::{Chart, ChartLayout};
use crate::primitives::layout_metrics::pixel_chart_layout;

/// Compute the macOS pixel-unit layout for `chart`. Shares its geometry
/// with `gtk_chart_layout` / `win_chart_layout` via [`pixel_chart_layout`].
pub fn mac_chart_layout(
    chart: &Chart,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    line_height: f64,
    char_width: f64,
) -> ChartLayout {
    pixel_chart_layout(
        chart,
        x as f32,
        y as f32,
        w as f32,
        h as f32,
        line_height as f32,
        char_width as f32,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::chart::{ChartHit, ChartKind, Series};
    use crate::types::WidgetId;

    fn sample_line() -> Chart {
        Chart {
            id: WidgetId::new("ch"),
            kind: ChartKind::Line,
            series: vec![Series {
                label: "x".into(),
                data: vec![0.0, 1.0, 0.5, 2.0, 1.0],
                color: Some(crate::types::Color::rgb(80, 160, 255)),
                fill: false,
            }],
            x_label: None,
            y_label: None,
            y_range: Some((0.0, 2.0)),
            x_range: None,
            show_legend: false,
            y_ticks: Some(0),
            x_ticks: Some(0),
            show_grid: false,
        }
    }

    #[test]
    fn hit_test_inside_plot_area_returns_body() {
        let chart = sample_line();
        let layout = mac_chart_layout(&chart, 0.0, 0.0, 200.0, 120.0, 16.0, 8.0);
        let cx = layout.plot_area.x + layout.plot_area.width * 0.5;
        let cy = layout.plot_area.y + layout.plot_area.height * 0.5;
        assert_eq!(layout.hit_test(cx, cy), ChartHit::Body(WidgetId::new("ch")),);
    }

    /// Regression test: layout helpers must return coords in the same frame
    /// across backends. `mac_chart_layout` bakes `x`/`y` straight into
    /// `plot_area` (absolute frame, matching the GTK/TUI twins) — call it
    /// directly at a non-zero origin and prove a click at the resulting
    /// absolute plot-area centre still resolves through `hit_test`.
    #[test]
    fn hit_test_inside_plot_area_at_nonzero_origin() {
        let chart = sample_line();
        let origin_x = 7.0_f64;
        let origin_y = 13.0_f64;
        let layout = mac_chart_layout(&chart, origin_x, origin_y, 200.0, 120.0, 16.0, 8.0);
        let cx = layout.plot_area.x + layout.plot_area.width * 0.5;
        let cy = layout.plot_area.y + layout.plot_area.height * 0.5;
        assert_eq!(layout.hit_test(cx, cy), ChartHit::Body(WidgetId::new("ch")),);
    }
}
