//! GTK rasteriser for [`crate::Chart`].
//!
//! Sparklines render as Cairo polylines. Line charts use Cairo paths
//! with optional area fill. Bar charts use Cairo rectangles — one per
//! series segment, stacked ([`ChartKind::Bar`]) or side by side
//! ([`ChartKind::BarGrouped`]) from the shared
//! [`Chart::bar_column_spans_all`] geometry (#584). Axis labels and legends
//! use Pango.

use gtk4::cairo::Context;
use gtk4::pango;

use super::set_source;
use crate::primitives::chart::{Chart, ChartKind, ChartLayout, ChartMeasure};
use crate::theme::Theme;
use crate::types::Color;

const SERIES_COLORS: [Color; 6] = [
    Color::rgb(80, 160, 255),
    Color::rgb(255, 120, 80),
    Color::rgb(80, 220, 120),
    Color::rgb(220, 180, 60),
    Color::rgb(180, 100, 240),
    Color::rgb(240, 100, 180),
];

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

/// Draw a [`Chart`] onto `cr`. Returns the layout for host click dispatch.
#[allow(clippy::too_many_arguments)]
pub fn draw_chart(
    cr: &Context,
    pango_layout: &pango::Layout,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    chart: &Chart,
    theme: &Theme,
    line_height: f64,
    char_width: f64,
    hovered_point: Option<(usize, usize)>,
    crosshair_x: Option<f64>,
) -> ChartLayout {
    let layout = gtk_chart_layout(chart, x, y, w, h, line_height, char_width);

    match chart.kind {
        ChartKind::Sparkline => paint_sparkline(cr, &layout, chart, theme),
        ChartKind::Line => paint_line(cr, pango_layout, &layout, chart, theme),
        ChartKind::Bar | ChartKind::BarGrouped => {
            paint_bar(cr, pango_layout, &layout, chart, theme)
        }
    }

    if let Some(data_x) = crosshair_x {
        paint_crosshair_gtk(cr, pango_layout, &layout, chart, theme, data_x);
    }

    if let Some((si, di)) = hovered_point {
        paint_hover_marker_gtk(cr, &layout, si, di, chart);
    }

    layout
}

fn series_color(chart: &Chart, idx: usize) -> Color {
    chart
        .series
        .get(idx)
        .and_then(|s| s.color)
        .unwrap_or(SERIES_COLORS[idx % SERIES_COLORS.len()])
}

fn paint_sparkline(cr: &Context, layout: &ChartLayout, chart: &Chart, theme: &Theme) {
    let pa = &layout.plot_area;
    let px = pa.x as f64;
    let py = pa.y as f64;
    let pw = pa.width as f64;
    let ph = pa.height as f64;

    // Background.
    set_source(cr, theme.background);
    cr.rectangle(px, py, pw, ph);
    cr.fill().ok();

    if let Some(s) = chart.series.first() {
        if s.data.is_empty() || pw <= 0.0 || ph <= 0.0 {
            return;
        }
        let (y_min, y_max) = chart.effective_y_range();
        let range = y_max - y_min;
        let color = series_color(chart, 0);
        set_source(cr, color);
        cr.set_line_width(1.5);

        let n = s.data.len();
        for (i, &val) in s.data.iter().enumerate() {
            let norm = if range > 0.0 {
                ((val - y_min) / range).clamp(0.0, 1.0)
            } else {
                0.5
            };
            let sx = px
                + if n <= 1 {
                    0.0
                } else {
                    (i as f64 / (n - 1) as f64) * pw
                };
            let sy = py + ph - norm * ph;
            if i == 0 {
                cr.move_to(sx, sy);
            } else {
                cr.line_to(sx, sy);
            }
        }
        cr.stroke().ok();

        // Area fill if requested.
        if s.fill && n > 1 {
            let (r, g, b) = super::cairo_rgb(color);
            cr.set_source_rgba(r, g, b, 0.2);
            for (i, &val) in s.data.iter().enumerate() {
                let norm = if range > 0.0 {
                    ((val - y_min) / range).clamp(0.0, 1.0)
                } else {
                    0.5
                };
                let sx = px + (i as f64 / (n - 1) as f64) * pw;
                let sy = py + ph - norm * ph;
                if i == 0 {
                    cr.move_to(sx, sy);
                } else {
                    cr.line_to(sx, sy);
                }
            }
            cr.line_to(px + pw, py + ph);
            cr.line_to(px, py + ph);
            cr.close_path();
            cr.fill().ok();
        }
    }
}

fn paint_line(
    cr: &Context,
    pango_layout: &pango::Layout,
    layout: &ChartLayout,
    chart: &Chart,
    theme: &Theme,
) {
    let pa = &layout.plot_area;
    let px = pa.x as f64;
    let py = pa.y as f64;
    let pw = pa.width as f64;
    let ph = pa.height as f64;

    // Background.
    set_source(cr, theme.background);
    cr.rectangle(px, py, pw, ph);
    cr.fill().ok();

    if pw <= 0.0 || ph <= 0.0 {
        return;
    }

    // Axes.
    set_source(cr, theme.muted_fg);
    cr.set_line_width(1.0);
    cr.move_to(px, py);
    cr.line_to(px, py + ph);
    cr.line_to(px + pw, py + ph);
    cr.stroke().ok();

    let (y_min, y_max) = chart.effective_y_range();
    let range = y_max - y_min;

    // Plot each series.
    for (si, s) in chart.series.iter().enumerate() {
        if s.data.is_empty() {
            continue;
        }
        let color = series_color(chart, si);
        set_source(cr, color);
        cr.set_line_width(2.0);

        let n = s.data.len();
        let mut points = Vec::with_capacity(n);
        for (i, &val) in s.data.iter().enumerate() {
            let norm = if range > 0.0 {
                ((val - y_min) / range).clamp(0.0, 1.0)
            } else {
                0.5
            };
            let sx = px
                + if n <= 1 {
                    0.0
                } else {
                    (i as f64 / (n - 1) as f64) * pw
                };
            let sy = py + ph - norm * ph;
            points.push((sx, sy));
            if i == 0 {
                cr.move_to(sx, sy);
            } else {
                cr.line_to(sx, sy);
            }
        }
        cr.stroke().ok();

        // Area fill.
        if s.fill && n > 1 {
            let (r, g, b) = super::cairo_rgb(color);
            cr.set_source_rgba(r, g, b, 0.15);
            for (i, &(sx, sy)) in points.iter().enumerate() {
                if i == 0 {
                    cr.move_to(sx, sy);
                } else {
                    cr.line_to(sx, sy);
                }
            }
            cr.line_to(px + pw, py + ph);
            cr.line_to(px, py + ph);
            cr.close_path();
            cr.fill().ok();
        }
    }

    paint_legend_gtk(cr, pango_layout, layout, chart, theme);
    paint_axis_labels_gtk(cr, pango_layout, layout, chart, theme);
}

fn paint_bar(
    cr: &Context,
    pango_layout: &pango::Layout,
    layout: &ChartLayout,
    chart: &Chart,
    theme: &Theme,
) {
    let pa = &layout.plot_area;
    let px = pa.x as f64;
    let py = pa.y as f64;
    let pw = pa.width as f64;
    let ph = pa.height as f64;

    // Background.
    set_source(cr, theme.background);
    cr.rectangle(px, py, pw, ph);
    cr.fill().ok();

    if pw <= 0.0 || ph <= 0.0 {
        return;
    }

    let n = chart.max_data_len();
    if n > 0 {
        let slot_w = pw / n as f64;
        let gap = (slot_w * 0.15).max(1.0);
        let bar_w = (slot_w - gap).max(1.0);
        let stacked = chart.kind.is_stacked_bar();
        let series_count = chart.series.len().max(1) as f64;
        let baseline = py + ph;

        for (i, column) in chart.bar_column_spans_all().into_iter().enumerate() {
            let slot_x = px + i as f64 * slot_w + gap / 2.0;
            for (si, bottom, top) in column {
                // Stacked segments span the slot; grouped series split it.
                let (bx, seg_w) = if stacked {
                    (slot_x, bar_w)
                } else {
                    let sub_w = (bar_w / series_count).max(1.0);
                    (slot_x + si as f64 * sub_w, sub_w)
                };
                let seg_h = (top - bottom) * ph;
                if seg_h <= 0.0 {
                    continue;
                }
                let by = baseline - top * ph;
                set_source(cr, series_color(chart, si));
                cr.rectangle(bx, by, seg_w, seg_h);
                cr.fill().ok();
            }
        }

        // Bottom axis.
        set_source(cr, theme.muted_fg);
        cr.set_line_width(1.0);
        cr.move_to(px, baseline);
        cr.line_to(px + pw, baseline);
        cr.stroke().ok();
    }

    paint_legend_gtk(cr, pango_layout, layout, chart, theme);
    paint_axis_labels_gtk(cr, pango_layout, layout, chart, theme);
}

fn paint_legend_gtk(
    cr: &Context,
    pango_layout: &pango::Layout,
    layout: &ChartLayout,
    chart: &Chart,
    theme: &Theme,
) {
    if let Some(lb) = &layout.legend_bounds {
        let lx = lb.x as f64;
        let ly = lb.y as f64;
        let lw = lb.width as f64;
        let lh = lb.height as f64;

        // Clear legend area.
        set_source(cr, theme.background);
        cr.rectangle(lx, ly, lw, lh);
        cr.fill().ok();

        let mut cx = lx + 2.0;
        for (i, s) in chart.series.iter().enumerate() {
            let color = series_color(chart, i);
            let swatch_size = lh * 0.6;
            let swatch_y = ly + (lh - swatch_size) / 2.0;

            set_source(cr, color);
            cr.rectangle(cx, swatch_y, swatch_size, swatch_size);
            cr.fill().ok();
            cx += swatch_size + 4.0;

            pango_layout.set_text(&s.label);
            pango_layout.set_attributes(None);
            set_source(cr, theme.foreground);
            cr.move_to(cx, ly);
            super::painted_text::show_layout(cr, pango_layout);
            let text_w = pango_layout.pixel_size().0 as f64;
            cx += text_w + 12.0;
        }
    }
}

fn paint_axis_labels_gtk(
    cr: &Context,
    pango_layout: &pango::Layout,
    layout: &ChartLayout,
    chart: &Chart,
    theme: &Theme,
) {
    let pa = &layout.plot_area;

    // Y-axis tick labels + grid lines.
    for &(sy, val) in &layout.y_tick_positions {
        let label = crate::primitives::chart::format_tick_value(val);
        pango_layout.set_text(&label);
        pango_layout.set_attributes(None);
        let text_w = pango_layout.pixel_size().0 as f64;
        set_source(cr, theme.muted_fg);
        cr.move_to(pa.x as f64 - text_w - 4.0, sy as f64 - 6.0);
        super::painted_text::show_layout(cr, pango_layout);

        if chart.show_grid && sy > pa.y && sy < pa.y + pa.height {
            let (r, g, b) = super::cairo_rgb(theme.muted_fg);
            cr.set_source_rgba(r, g, b, 0.2);
            cr.set_line_width(0.5);
            cr.move_to(pa.x as f64, sy as f64);
            cr.line_to((pa.x + pa.width) as f64, sy as f64);
            cr.stroke().ok();
        }
    }

    set_source(cr, theme.foreground);

    if let Some(label) = &chart.x_label {
        pango_layout.set_text(label);
        pango_layout.set_attributes(None);
        let text_w = pango_layout.pixel_size().0 as f64;
        let cx = pa.x as f64 + (pa.width as f64 - text_w) / 2.0;
        let cy = (pa.y + pa.height) as f64;
        cr.move_to(cx, cy);
        super::painted_text::show_layout(cr, pango_layout);
    }

    if let Some(label) = &chart.y_label {
        pango_layout.set_text(label);
        pango_layout.set_attributes(None);
        let lx = layout.bounds.x as f64;
        let ly = pa.y as f64;
        cr.move_to(lx, ly);
        super::painted_text::show_layout(cr, pango_layout);
    }
}

fn paint_crosshair_gtk(
    cr: &Context,
    pango_layout: &pango::Layout,
    layout: &ChartLayout,
    chart: &Chart,
    theme: &Theme,
    data_x: f64,
) {
    let data_len = chart.max_data_len();
    let screen_x = layout.data_to_screen_x(data_x, data_len) as f64;
    let pa = &layout.plot_area;

    if screen_x <= pa.x as f64 || screen_x >= (pa.x + pa.width) as f64 {
        return;
    }

    let (r, g, b) = super::cairo_rgb(theme.muted_fg);
    cr.set_source_rgba(r, g, b, 0.5);
    cr.set_line_width(1.0);
    let dashes = [4.0, 4.0];
    cr.set_dash(&dashes, 0.0);
    cr.move_to(screen_x, pa.y as f64);
    cr.line_to(screen_x, (pa.y + pa.height) as f64);
    cr.stroke().ok();
    cr.set_dash(&[], 0.0);

    let (y_min, y_max) = chart.effective_y_range();
    let range = y_max - y_min;
    for (si, s) in chart.series.iter().enumerate() {
        if s.data.is_empty() {
            continue;
        }
        let idx = data_x.round() as usize;
        if idx >= s.data.len() {
            continue;
        }
        let val = s.data[idx];
        let label = crate::primitives::chart::format_tick_value(val);
        let color = series_color(chart, si);
        set_source(cr, color);
        pango_layout.set_text(&label);
        pango_layout.set_attributes(None);
        let norm = if range > 0.0 {
            ((val - y_min) / range).clamp(0.0, 1.0)
        } else {
            0.5
        };
        let sy = pa.y as f64 + pa.height as f64 - norm * pa.height as f64;
        cr.move_to(screen_x + 4.0, sy - 8.0);
        super::painted_text::show_layout(cr, pango_layout);
    }

    let _ = theme;
}

fn paint_hover_marker_gtk(
    cr: &Context,
    layout: &ChartLayout,
    series_idx: usize,
    data_idx: usize,
    chart: &Chart,
) {
    for &(si, di, sx, sy) in &layout.data_point_positions {
        if si == series_idx && di == data_idx {
            let color = series_color(chart, si);
            set_source(cr, color);
            cr.arc(sx as f64, sy as f64, 5.0, 0.0, 2.0 * std::f64::consts::PI);
            cr.fill().ok();
            let (r, g, b) = super::cairo_rgb(color);
            cr.set_source_rgba(r, g, b, 0.3);
            cr.arc(sx as f64, sy as f64, 8.0, 0.0, 2.0 * std::f64::consts::PI);
            cr.fill().ok();
            return;
        }
    }
}

// Headless paint tests for the stacked/grouped bar geometry (#584 review —
// GTK had no chart coverage at all). Uses a Cairo `ImageSurface` and reads
// back pixel data directly, mirroring the pattern in `gtk::terminal`.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::chart::Series;
    use crate::types::WidgetId;
    use pangocairo::cairo::{Format, ImageSurface};

    const W: i32 = 30;
    const H: i32 = 30;

    /// Read an RGB triple from an ARgb32 surface at pixel (x, y).
    fn pixel(data: &[u8], stride: usize, x: i32, y: i32) -> (u8, u8, u8) {
        let off = y as usize * stride + x as usize * 4;
        (data[off + 2], data[off + 1], data[off])
    }

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

    fn paint(chart: &Chart) -> ImageSurface {
        let surface = ImageSurface::create(Format::ARgb32, W, H).expect("create ImageSurface");
        {
            let cr = Context::new(&surface).expect("Context::new");
            let pango_layout = pangocairo::functions::create_layout(&cr);
            let theme = Theme::default();
            draw_chart(
                &cr,
                &pango_layout,
                0.0,
                0.0,
                W as f64,
                H as f64,
                chart,
                &theme,
                12.0,
                8.0,
                None,
                None,
            );
        }
        surface
    }

    #[test]
    fn stacked_bar_paints_each_series_in_its_own_color() {
        // Three equal-value series stacked in a single column, with an
        // explicit y_range so the geometry is exact: (0, 1/3), (1/3, 2/3),
        // (2/3, 1) bottom to top.
        let chart = bar_chart(
            ChartKind::Bar,
            vec![
                series("a", vec![1.0]),
                series("b", vec![1.0]),
                series("c", vec![1.0]),
            ],
            (0.0, 3.0),
        );
        let mut s = paint(&chart);
        s.flush();
        let stride = s.stride() as usize;
        let data = s.data().expect("surface data");

        // slot_w = 30, gap = 4.5, bar_w = 25.5, slot_x = 2.25 → mid-x ≈ 15.
        let mid_x = 15;
        let bottom_y = 25; // mid of series 0's span (screen bottom third)
        let mid_y = 15; // mid of series 1's span
        let top_y = 5; // mid of series 2's span (screen top third)

        assert_eq!(
            pixel(&data, stride, mid_x, bottom_y),
            (SERIES_COLORS[0].r, SERIES_COLORS[0].g, SERIES_COLORS[0].b),
            "bottom segment should be series 0's colour"
        );
        assert_eq!(
            pixel(&data, stride, mid_x, mid_y),
            (SERIES_COLORS[1].r, SERIES_COLORS[1].g, SERIES_COLORS[1].b),
            "middle segment should be series 1's colour"
        );
        assert_eq!(
            pixel(&data, stride, mid_x, top_y),
            (SERIES_COLORS[2].r, SERIES_COLORS[2].g, SERIES_COLORS[2].b),
            "top segment should be series 2's colour"
        );
    }

    #[test]
    fn stacked_bar_all_zero_series_does_not_shift_the_others() {
        // series[1] and series[3] are all-zero; series[0] and series[2]
        // must still occupy exactly half the stack each, with no gap or
        // shift introduced by the zero-height segments between them.
        let chart = bar_chart(
            ChartKind::Bar,
            vec![
                series("a", vec![1.0]),
                series("zero1", vec![0.0]),
                series("b", vec![1.0]),
                series("zero2", vec![0.0]),
            ],
            (0.0, 2.0),
        );
        let mut s = paint(&chart);
        s.flush();
        let stride = s.stride() as usize;
        let data = s.data().expect("surface data");

        let mid_x = 15;
        // Bottom half (series 0) mid ≈ y 22-23; top half (series 2) mid ≈ y 7-8.
        assert_eq!(
            pixel(&data, stride, mid_x, 22),
            (SERIES_COLORS[0].r, SERIES_COLORS[0].g, SERIES_COLORS[0].b),
            "bottom half should stay series 0's colour, unshifted by the zero series"
        );
        assert_eq!(
            pixel(&data, stride, mid_x, 8),
            (SERIES_COLORS[2].r, SERIES_COLORS[2].g, SERIES_COLORS[2].b),
            "top half should be series 2's colour, immediately above series 0"
        );
    }

    #[test]
    fn grouped_bar_paints_series_side_by_side() {
        let chart = bar_chart(
            ChartKind::BarGrouped,
            vec![
                series("a", vec![1.0]),
                series("b", vec![2.0]),
                series("c", vec![3.0]),
            ],
            (0.0, 3.0),
        );
        let mut s = paint(&chart);
        s.flush();
        let stride = s.stride() as usize;
        let data = s.data().expect("surface data");

        // All three sub-bars reach down to a shared row near the baseline
        // (every grouped span starts at the floor), so one horizontal
        // probe row distinguishes them purely by x position.
        let probe_y = 27;
        assert_eq!(
            pixel(&data, stride, 6, probe_y),
            (SERIES_COLORS[0].r, SERIES_COLORS[0].g, SERIES_COLORS[0].b),
            "leftmost sub-bar should be series 0"
        );
        assert_eq!(
            pixel(&data, stride, 15, probe_y),
            (SERIES_COLORS[1].r, SERIES_COLORS[1].g, SERIES_COLORS[1].b),
            "middle sub-bar should be series 1"
        );
        assert_eq!(
            pixel(&data, stride, 23, probe_y),
            (SERIES_COLORS[2].r, SERIES_COLORS[2].g, SERIES_COLORS[2].b),
            "rightmost sub-bar should be series 2"
        );
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
