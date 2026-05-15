//! macOS rasteriser for [`crate::Chart`].
//!
//! Mirrors [`crate::gtk::chart::draw_chart`] for the three chart kinds:
//!
//! - **Sparkline** — single-series polyline (1.5pt) inside the plot area.
//! - **Line** — multi-series polyline with optional area fill at 20%
//!   alpha.
//! - **Bar** — vertical rectangles per data point.
//!
//! Hover marker and crosshair overlays are honoured. Axis labels,
//! ticks, grid, and legend chrome use Core Text.

use core_graphics::base::CGFloat;
use core_graphics::geometry::CGRect;
use core_graphics::sys::CGContextRef;
use core_text::font::CTFont;

use super::text::{draw_text, measure_text};
use crate::primitives::chart::{Chart, ChartKind, ChartLayout, ChartMeasure};
use crate::theme::Theme;
use crate::types::Color;

/// Default series palette — matches GTK's `SERIES_COLORS`.
const SERIES_COLORS: [Color; 6] = [
    Color::rgb(80, 160, 255),
    Color::rgb(255, 120, 80),
    Color::rgb(80, 220, 120),
    Color::rgb(220, 180, 60),
    Color::rgb(180, 100, 240),
    Color::rgb(240, 100, 180),
];

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

/// Draw `chart` onto `ctx`. Returns the layout used for painting.
///
/// # Safety
///
/// `ctx` must be a valid `CGContextRef` borrowed for the duration of
/// the call.
#[allow(clippy::too_many_arguments)]
pub unsafe fn draw_chart(
    ctx: CGContextRef,
    font: &CTFont,
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
    let layout = mac_chart_layout(chart, x, y, w, h, line_height, char_width);

    if w <= 0.0 || h <= 0.0 {
        return layout;
    }

    CGContextSaveGState(ctx);
    CGContextClipToRect(ctx, CGRect::new_xywh(x, y, w, h));

    match chart.kind {
        ChartKind::Sparkline => paint_sparkline(ctx, &layout, chart, theme),
        ChartKind::Line => paint_line(ctx, font, &layout, chart, theme),
        ChartKind::Bar => paint_bar(ctx, font, &layout, chart, theme),
    }

    if let Some(data_x) = crosshair_x {
        paint_crosshair(ctx, &layout, chart, theme, data_x);
    }
    if let Some((si, di)) = hovered_point {
        paint_hover_marker(ctx, &layout, si, di, chart);
    }

    CGContextRestoreGState(ctx);
    layout
}

fn series_color(chart: &Chart, idx: usize) -> Color {
    chart
        .series
        .get(idx)
        .and_then(|s| s.color)
        .unwrap_or(SERIES_COLORS[idx % SERIES_COLORS.len()])
}

unsafe fn paint_sparkline(ctx: CGContextRef, layout: &ChartLayout, chart: &Chart, theme: &Theme) {
    let pa = &layout.plot_area;
    let px = pa.x as f64;
    let py = pa.y as f64;
    let pw = pa.width as f64;
    let ph = pa.height as f64;
    fill_rect(ctx, px, py, pw, ph, theme.background);

    let Some(series) = chart.series.first() else {
        return;
    };
    if series.data.is_empty() || pw <= 0.0 || ph <= 0.0 {
        return;
    }
    // The primitive layout pre-stamps Sparkline data positions at
    // 1-pixel cadence (TUI-cell convention bleeding into pixel
    // backends). For a GUI rasteriser we want the polyline stretched
    // across the full plot width — same approach GTK uses.
    let (y_min, y_max) = chart.effective_y_range();
    let range = y_max - y_min;
    let n = series.data.len();
    let mut pts = Vec::with_capacity(n);
    for (i, &val) in series.data.iter().enumerate() {
        let norm = if range > 0.0 {
            ((val - y_min) / range).clamp(0.0, 1.0)
        } else {
            0.5
        };
        let sx = if n <= 1 {
            px
        } else {
            px + (i as f64 / (n - 1) as f64) * pw
        };
        let sy = py + ph - norm * ph;
        pts.push((sx, sy));
    }
    stroke_path(ctx, &pts, series_color(chart, 0), 1.5);
}

unsafe fn paint_line(
    ctx: CGContextRef,
    font: &CTFont,
    layout: &ChartLayout,
    chart: &Chart,
    theme: &Theme,
) {
    let pa = &layout.plot_area;
    fill_rect(
        ctx,
        pa.x as f64,
        pa.y as f64,
        pa.width as f64,
        pa.height as f64,
        theme.background,
    );

    // Grid lines.
    if chart.show_grid {
        for &(sy, _) in &layout.y_tick_positions {
            fill_rect(
                ctx,
                pa.x as f64,
                sy as f64,
                pa.width as f64,
                1.0,
                theme.separator,
            );
        }
    }

    // Per-series strokes.
    let baseline = pa.y as f64 + pa.height as f64;
    for (si, s) in chart.series.iter().enumerate() {
        let pts: Vec<(f64, f64)> = layout
            .data_point_positions
            .iter()
            .filter_map(|(s_idx, _, x, y)| {
                if *s_idx == si {
                    Some((*x as f64, *y as f64))
                } else {
                    None
                }
            })
            .collect();
        if pts.is_empty() {
            continue;
        }
        let color = series_color(chart, si);
        // Area fill (under-line polygon) painted before the stroke so
        // the line reads on top.
        if s.fill && pts.len() > 1 {
            let alpha_color = Color {
                r: color.r,
                g: color.g,
                b: color.b,
                a: 38, // ~15% alpha
            };
            let mut poly = pts.clone();
            // Close the polygon down to the baseline.
            poly.push((pts.last().unwrap().0, baseline));
            poly.push((pts.first().unwrap().0, baseline));
            fill_polygon(ctx, &poly, alpha_color);
        }
        stroke_path(ctx, &pts, color, 1.5);
    }

    paint_axis_labels(ctx, font, layout, chart, theme);
    paint_legend(ctx, font, layout, chart, theme);
}

unsafe fn paint_bar(
    ctx: CGContextRef,
    font: &CTFont,
    layout: &ChartLayout,
    chart: &Chart,
    theme: &Theme,
) {
    let pa = &layout.plot_area;
    fill_rect(
        ctx,
        pa.x as f64,
        pa.y as f64,
        pa.width as f64,
        pa.height as f64,
        theme.background,
    );

    let Some(series) = chart.series.first() else {
        return;
    };
    if series.data.is_empty() {
        return;
    }
    let (y_min, y_max) = chart.effective_y_range();
    let range = y_max - y_min;
    let pw = pa.width as f64;
    let ph = pa.height as f64;
    let px = pa.x as f64;
    let py = pa.y as f64;
    let n = series.data.len() as f64;
    // Slot-based positioning: each bar gets a full slot of width
    // `pw / n`, with a 15% gap split between left/right edges.
    let slot_w = pw / n;
    let gap = (slot_w * 0.15).max(1.0);
    let effective_bar_w = (slot_w - gap).max(1.0);
    let baseline = py + ph;
    let color = series_color(chart, 0);
    for (i, &val) in series.data.iter().enumerate() {
        let norm = if range > 0.0 {
            ((val - y_min) / range).clamp(0.0, 1.0)
        } else {
            0.5
        };
        let bar_h = norm * ph;
        let bx = px + i as f64 * slot_w + gap / 2.0;
        let by = baseline - bar_h;
        if bar_h > 0.0 {
            fill_rect(ctx, bx, by, effective_bar_w, bar_h, color);
        }
    }

    paint_axis_labels(ctx, font, layout, chart, theme);
    paint_legend(ctx, font, layout, chart, theme);
}

unsafe fn paint_axis_labels(
    ctx: CGContextRef,
    font: &CTFont,
    layout: &ChartLayout,
    _chart: &Chart,
    theme: &Theme,
) {
    // Y-axis tick labels: render to the left of the plot area.
    for &(sy, val) in &layout.y_tick_positions {
        let label = format_tick(val);
        let (tw, th) = measure_text(font, &label);
        let lx = (layout.plot_area.x as f64 - tw - 4.0).max(0.0);
        draw_text(
            ctx,
            font,
            &label,
            lx,
            sy as f64 - th / 2.0,
            color_to_cg(theme.muted_fg),
        );
    }
}

unsafe fn paint_legend(
    ctx: CGContextRef,
    font: &CTFont,
    layout: &ChartLayout,
    chart: &Chart,
    _theme: &Theme,
) {
    let Some(lb) = layout.legend_bounds else {
        return;
    };
    if chart.series.is_empty() {
        return;
    }
    let entry_w = (lb.width / chart.series.len() as f32).max(1.0);
    for (i, s) in chart.series.iter().enumerate() {
        let color = series_color(chart, i);
        let ex = lb.x + entry_w * i as f32;
        // Colour swatch.
        let sw = 10.0_f64;
        fill_rect(
            ctx,
            ex as f64,
            lb.y as f64 + 2.0,
            sw,
            lb.height as f64 - 4.0,
            color,
        );
        // Series label.
        let (_, th) = measure_text(font, &s.label);
        draw_text(
            ctx,
            font,
            &s.label,
            ex as f64 + sw + 4.0,
            lb.y as f64 + (lb.height as f64 - th) / 2.0,
            color_to_cg(color),
        );
    }
}

unsafe fn paint_crosshair(
    ctx: CGContextRef,
    layout: &ChartLayout,
    chart: &Chart,
    theme: &Theme,
    data_x: f64,
) {
    let n = chart.max_data_len();
    if n == 0 {
        return;
    }
    let sx = layout.data_to_screen_x(data_x, n);
    fill_rect(
        ctx,
        sx as f64 - 0.5,
        layout.plot_area.y as f64,
        1.0,
        layout.plot_area.height as f64,
        theme.accent_fg,
    );
}

unsafe fn paint_hover_marker(
    ctx: CGContextRef,
    layout: &ChartLayout,
    si: usize,
    di: usize,
    chart: &Chart,
) {
    let Some(&(_, _, sx, sy)) = layout
        .data_point_positions
        .iter()
        .find(|(s, d, _, _)| *s == si && *d == di)
    else {
        return;
    };
    let color = series_color(chart, si);
    let radius = 4.0_f64;
    fill_rect(
        ctx,
        sx as f64 - radius,
        sy as f64 - radius,
        radius * 2.0,
        radius * 2.0,
        color,
    );
}

/// Stroke a polyline through `pts` using the CG path API.
unsafe fn stroke_path(ctx: CGContextRef, pts: &[(f64, f64)], color: Color, line_width: f64) {
    if pts.len() < 2 {
        return;
    }
    let (r, g, b, a) = color_to_cg(color);
    CGContextSetRGBStrokeColor(ctx, r, g, b, a);
    CGContextSetLineWidth(ctx, line_width);
    CGContextBeginPath(ctx);
    CGContextMoveToPoint(ctx, pts[0].0, pts[0].1);
    for &(px, py) in &pts[1..] {
        CGContextAddLineToPoint(ctx, px, py);
    }
    CGContextStrokePath(ctx);
}

/// Fill a closed polygon defined by `pts`.
unsafe fn fill_polygon(ctx: CGContextRef, pts: &[(f64, f64)], color: Color) {
    if pts.len() < 3 {
        return;
    }
    let (r, g, b, a) = color_to_cg(color);
    CGContextSetRGBFillColor(ctx, r, g, b, a);
    CGContextBeginPath(ctx);
    CGContextMoveToPoint(ctx, pts[0].0, pts[0].1);
    for &(px, py) in &pts[1..] {
        CGContextAddLineToPoint(ctx, px, py);
    }
    CGContextClosePath(ctx);
    CGContextFillPath(ctx);
}

fn format_tick(val: f64) -> String {
    if val.fract().abs() < 1e-9 {
        format!("{:.0}", val)
    } else {
        format!("{:.1}", val)
    }
}

fn color_to_cg(c: Color) -> (f64, f64, f64, f64) {
    (
        c.r as f64 / 255.0,
        c.g as f64 / 255.0,
        c.b as f64 / 255.0,
        c.a as f64 / 255.0,
    )
}

unsafe fn fill_rect(ctx: CGContextRef, x: f64, y: f64, w: f64, h: f64, c: Color) {
    let (r, g, b, a) = color_to_cg(c);
    CGContextSetRGBFillColor(ctx, r, g, b, a);
    CGContextFillRect(ctx, CGRect::new_xywh(x, y, w, h));
}

trait CGRectExt {
    fn new_xywh(x: f64, y: f64, w: f64, h: f64) -> Self;
}
impl CGRectExt for CGRect {
    fn new_xywh(x: f64, y: f64, w: f64, h: f64) -> Self {
        use core_graphics::geometry::{CGPoint, CGSize};
        CGRect::new(&CGPoint::new(x, y), &CGSize::new(w, h))
    }
}

extern "C" {
    fn CGContextSaveGState(c: CGContextRef);
    fn CGContextRestoreGState(c: CGContextRef);
    fn CGContextClipToRect(c: CGContextRef, rect: CGRect);
    fn CGContextSetRGBFillColor(c: CGContextRef, r: CGFloat, g: CGFloat, b: CGFloat, a: CGFloat);
    fn CGContextSetRGBStrokeColor(c: CGContextRef, r: CGFloat, g: CGFloat, b: CGFloat, a: CGFloat);
    fn CGContextSetLineWidth(c: CGContextRef, w: CGFloat);
    fn CGContextFillRect(c: CGContextRef, rect: CGRect);
    fn CGContextBeginPath(c: CGContextRef);
    fn CGContextMoveToPoint(c: CGContextRef, x: CGFloat, y: CGFloat);
    fn CGContextAddLineToPoint(c: CGContextRef, x: CGFloat, y: CGFloat);
    fn CGContextClosePath(c: CGContextRef);
    fn CGContextStrokePath(c: CGContextRef);
    fn CGContextFillPath(c: CGContextRef);
}

#[cfg(test)]
mod tests {
    use super::super::headless::BitmapSurface;
    use super::super::text::make_font;
    use super::super::MacBackend;
    use super::*;
    use crate::event::{Rect as QRect, Viewport};
    use crate::primitives::chart::{ChartHit, ChartKind, Series};
    use crate::theme::Theme;
    use crate::types::WidgetId;
    use crate::Backend;

    const W: u32 = 200;
    const H: u32 = 120;

    fn font() -> CTFont {
        make_font("Menlo", 14.0).expect("Menlo installed")
    }

    fn sample_line() -> Chart {
        Chart {
            id: WidgetId::new("ch"),
            kind: ChartKind::Line,
            series: vec![Series {
                label: "x".into(),
                data: vec![0.0, 1.0, 0.5, 2.0, 1.0],
                color: Some(Color::rgb(80, 160, 255)),
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

    fn paint_via_backend(
        chart: &Chart,
        hovered: Option<(usize, usize)>,
    ) -> (BitmapSurface, ChartLayout) {
        let surface = BitmapSurface::new(W, H);
        surface.fill(0.0, 0.0, 0.0, 0.0);
        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        let layout = std::cell::RefCell::new(None);
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            let l = b.draw_chart(
                QRect::new(0.0, 0.0, W as f32, H as f32),
                chart,
                hovered,
                None,
            );
            *layout.borrow_mut() = Some(l);
        });
        backend.end_frame();
        (surface, layout.into_inner().unwrap())
    }

    #[test]
    fn plot_area_paints_background() {
        let chart = sample_line();
        let (surface, layout) = paint_via_backend(&chart, None);
        let theme = Theme::default();
        // Probe a corner of the plot area.
        let px = (layout.plot_area.x + layout.plot_area.width - 2.0) as u32;
        let py = (layout.plot_area.y + 2.0) as u32;
        let (r, g, b, _) = surface.pixel(px, py);
        assert_eq!(
            (r, g, b),
            (theme.background.r, theme.background.g, theme.background.b),
        );
    }

    #[test]
    fn line_stroke_paints_series_color() {
        let chart = sample_line();
        let (surface, layout) = paint_via_backend(&chart, None);
        // Sample at data point index 1 (value 1.0 in [0..2] range = mid
        // plot, comfortably inside bounds — first and last points sit
        // on the plot edges).
        let (_, _, sx, sy) = layout.data_point_positions[1];
        let (r, g, b, _) = surface.pixel(sx as u32, sy as u32);
        // Series colour blue → blue channel dominant.
        assert!(
            b > r,
            "expected blue dominant at data point 1 ({}, {}), got ({}, {}, {})",
            sx as u32,
            sy as u32,
            r,
            g,
            b,
        );
    }

    #[test]
    fn hit_test_inside_plot_area_returns_body() {
        let chart = sample_line();
        let (_surface, layout) = paint_via_backend(&chart, None);
        let cx = layout.plot_area.x + layout.plot_area.width * 0.5;
        let cy = layout.plot_area.y + layout.plot_area.height * 0.5;
        assert_eq!(layout.hit_test(cx, cy), ChartHit::Body(WidgetId::new("ch")),);
    }

    #[test]
    fn hover_marker_painted_when_set() {
        let chart = sample_line();
        let (surface, layout) = paint_via_backend(&chart, Some((0, 2)));
        // Probe at the centre of the hover marker (data point 2).
        let (_, _, sx, sy) = layout.data_point_positions[2];
        let (r, _g, b, _) = surface.pixel(sx as u32, sy as u32);
        // Marker is the series colour, solid — blue should dominate.
        assert!(b > r);
    }
}
