//! GTK rasteriser for [`crate::Chart`].
//!
//! Sparklines render as Cairo polylines. Line charts use Cairo paths
//! with optional area fill. Bar charts use Cairo rectangles. Axis
//! labels and legends use Pango.

use gtk4::cairo::Context;
use gtk4::pango;
use pangocairo::functions as pcfn;

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
) -> ChartLayout {
    let layout = gtk_chart_layout(chart, x, y, w, h, line_height, char_width);

    match chart.kind {
        ChartKind::Sparkline => paint_sparkline(cr, &layout, chart, theme),
        ChartKind::Line => paint_line(cr, pango_layout, &layout, chart, theme),
        ChartKind::Bar => paint_bar(cr, pango_layout, &layout, chart, theme),
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

    if let Some(s) = chart.series.first() {
        if s.data.is_empty() {
            return;
        }
        let (y_min, y_max) = chart.effective_y_range();
        let range = y_max - y_min;
        let n = s.data.len();
        let bar_w = pw / n as f64;
        let gap = (bar_w * 0.15).max(1.0);
        let effective_bar_w = (bar_w - gap).max(1.0);
        let color = series_color(chart, 0);

        for (i, &val) in s.data.iter().enumerate() {
            let norm = if range > 0.0 {
                ((val - y_min) / range).clamp(0.0, 1.0)
            } else {
                0.5
            };
            let bar_h = norm * ph;
            let bx = px + i as f64 * bar_w + gap / 2.0;
            let by = py + ph - bar_h;

            set_source(cr, color);
            cr.rectangle(bx, by, effective_bar_w, bar_h);
            cr.fill().ok();
        }

        // Bottom axis.
        set_source(cr, theme.muted_fg);
        cr.set_line_width(1.0);
        cr.move_to(px, py + ph);
        cr.line_to(px + pw, py + ph);
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
            pcfn::show_layout(cr, pango_layout);
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
        pcfn::show_layout(cr, pango_layout);

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
        pcfn::show_layout(cr, pango_layout);
    }

    if let Some(label) = &chart.y_label {
        pango_layout.set_text(label);
        pango_layout.set_attributes(None);
        let lx = layout.bounds.x as f64;
        let ly = pa.y as f64;
        cr.move_to(lx, ly);
        pcfn::show_layout(cr, pango_layout);
    }
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
