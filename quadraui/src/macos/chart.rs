//! macOS layout helper for [`crate::Chart`].
//!
//! Painting moved to the shared [`crate::primitives::chart::paint`]
//! (#810, NativeSurface Phase 2c) — see that fn's doc for the
//! divergences (the quadraui#791 clip, most notably) resolved while
//! unifying `gtk::chart::draw_chart`, `macos::chart::draw_chart` and
//! `win::chart::draw_chart` into one implementation. This module now
//! only carries [`mac_chart_layout`], the pixel-unit layout query used
//! for both hit-testing (`MacBackend::chart_layout`) and painting
//! (`MacBackend::draw_chart`, via `Backend::chart_layout`).
//!
//! The deleted `macos::chart::draw_chart` free function had zero call
//! sites in `coord-tui`/`vimcode` (`grep -rn draw_chart ~/src/coord-tui/src
//! ~/src/vimcode/src` — both only call `Backend::draw_chart`, whose
//! signature is unchanged), so it needed no deprecation shim (CLAUDE.md
//! rule 1).

use crate::primitives::chart::{Chart, ChartLayout, ChartMeasure};

/// Compute the macOS pixel-unit layout for `chart`.
pub fn mac_chart_layout(
    chart: &Chart,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    line_height: f64,
    char_width: f64,
) -> ChartLayout {
    chart.layout(
        x as f32,
        y as f32,
        ChartMeasure {
            width: w as f32,
            height: h as f32,
            char_width: char_width as f32,
            line_height: line_height as f32,
        },
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

    /// Regression for quadraui#494 / LESSONS.md "Layout helpers must
    /// return coords in the same frame across backends": every other
    /// test in this file lays out at origin (0, 0), where an
    /// origin-offset bug is invisible (see #44's mac_tree_layout).
    /// `mac_chart_layout` bakes `x`/`y` straight into `plot_area`
    /// (absolute frame, matching the GTK/TUI twins) — call it directly
    /// at a non-zero origin and prove a click at the resulting absolute
    /// plot-area centre still resolves through `hit_test`.
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
