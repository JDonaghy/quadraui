//! GTK layout helper for [`crate::Chart`].
//!
//! Painting moved to the shared [`crate::primitives::chart::paint`]
//! (#810, NativeSurface Phase 2c) — see that fn's doc for the
//! divergences (the quadraui#791 clip, most notably) resolved while
//! unifying `gtk::chart::draw_chart`, `macos::chart::draw_chart` and
//! `win::chart::draw_chart` into one implementation. This module now
//! only carries [`gtk_chart_layout`], the pixel-unit layout query used
//! for both hit-testing (`GtkBackend::chart_layout`) and painting
//! (`GtkBackend::draw_chart`, via `Backend::chart_layout`).
//!
//! The deleted `gtk::chart::draw_chart` free function had zero call
//! sites in `coord-tui`/`vimcode` (`grep -rn draw_chart ~/src/coord-tui/src
//! ~/src/vimcode/src` — both only call `Backend::draw_chart`, whose
//! signature is unchanged), so it needed no deprecation shim (CLAUDE.md
//! rule 1).

use crate::primitives::chart::{Chart, ChartLayout, ChartMeasure};

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
    use crate::primitives::chart::{ChartKind, Series};
    use crate::types::WidgetId;

    fn series(label: &str, data: Vec<f64>) -> Series {
        Series {
            label: label.into(),
            data,
            color: None,
            fill: false,
        }
    }

    fn bar_chart(kind: ChartKind, series: Vec<Series>, y_range: (f64, f64)) -> Chart {
        Chart {
            id: WidgetId::new("chart"),
            kind,
            series,
            x_label: None,
            y_label: None,
            y_range: Some(y_range),
            x_range: None,
            show_legend: false,
            y_ticks: Some(0),
            x_ticks: Some(0),
            show_grid: false,
        }
    }

    /// `chart_layout` is documented **ABSOLUTE** (issue #505):
    /// `plot_area` / `data_point_positions` must be shifted by the
    /// chart's own origin, not left at (0, 0) — the case that hides a
    /// LOCAL/ABSOLUTE mixup.
    fn layout_round_trip_at(x: f64, y: f64) {
        let chart = bar_chart(
            ChartKind::Bar,
            vec![series("a", vec![1.0, 2.0])],
            (0.0, 2.0),
        );
        let layout = gtk_chart_layout(&chart, x, y, 100.0, 40.0, 12.0, 6.0);

        assert_eq!(layout.plot_area.x as f64, x);
        assert!(!layout.data_point_positions.is_empty());
        for &(_, _, px, py) in &layout.data_point_positions {
            assert!(
                px as f64 >= x && py as f64 >= y,
                "data point ({px}, {py}) must not fall left of/above the chart's own origin ({x}, {y})"
            );
        }
    }

    #[test]
    fn layout_round_trip() {
        layout_round_trip_at(0.0, 0.0);
    }

    /// Non-zero-origin regression guard (issue #505 / LESSONS.md).
    #[test]
    fn layout_round_trip_at_nonzero_origin() {
        layout_round_trip_at(7.0, 13.0);
    }
}
