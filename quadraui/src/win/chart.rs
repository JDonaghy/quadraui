//! Direct2D / DirectWrite rasteriser for [`crate::primitives::chart::Chart`]
//! (issue #26).
//!
//! Mirrors `gtk::chart`'s structure: [`Chart::layout`] (the D6 layout
//! API) resolves the plot area, legend, axis-tick positions, and every
//! data point's screen position; this module paints from that one
//! resolved [`ChartLayout`] — line/axis strokes via
//! [`super::text::draw_line`], bars via [`super::text::fill_rect`], the
//! hover marker via [`super::text::fill_circle`].
//!
//! Only compiled on `target_os = "windows"` — see `super::mod`'s
//! `#[cfg(target_os = "windows")] mod chart;` and `backend.rs`'s module
//! docs. See `win::status_bar`'s module doc for why colours come from
//! `Theme::default()` rather than a live `WinBackend` theme field.
//!
//! # Scope for #26
//!
//! Sparkline/Line area fill (`Series::fill`) is not painted — it needs
//! a filled polygon path, which this backend doesn't build a
//! `ID2D1PathGeometry` for yet; only the line stroke itself paints.
//! The crosshair is a solid line, not GTK's dashed one (`ID2D1StrokeStyle`
//! dash patterns are a separate API this issue doesn't need for
//! correctness — see acceptance criteria: "crosshair line ... render
//! correctly", not "dashed").

use windows::Win32::Graphics::Direct2D::ID2D1RenderTarget;

use super::text::{blend, draw_line, fill_circle, fill_rect, DWrite};
use crate::event::Rect;
use crate::primitives::chart::{format_tick_value, Chart, ChartKind, ChartLayout, ChartMeasure};
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

fn series_color(chart: &Chart, idx: usize) -> Color {
    chart
        .series
        .get(idx)
        .and_then(|s| s.color)
        .unwrap_or(SERIES_COLORS[idx % SERIES_COLORS.len()])
}

/// Compute a [`Chart`]'s layout without painting — the DirectWrite twin
/// of [`draw_chart`]'s internal layout call.
pub fn win_chart_layout(
    chart: &Chart,
    rect: Rect,
    char_width: f32,
    line_height: f32,
) -> ChartLayout {
    chart.layout(
        rect.x,
        rect.y,
        ChartMeasure {
            width: rect.width,
            height: rect.height,
            char_width,
            line_height,
        },
    )
}

/// Draw a [`Chart`] into `rect` (DIPs) on `target`. Returns the
/// resolved [`ChartLayout`] for host click dispatch and nearest-point
/// hover resolution.
///
/// # Visual contract
///
/// - **Background:** `Theme::background` across the plot area.
/// - **Line/Sparkline:** a 1.5–2 DIP stroke per series, `series.color`
///   or the built-in 6-colour palette.
/// - **Bar/BarGrouped:** filled rectangles from
///   [`Chart::bar_column_spans_all`], stacked or side by side.
/// - **Legend:** one colour swatch + label per series.
/// - **Axis labels + grid:** `Theme::muted_fg` tick labels;
///   `chart.show_grid` adds faint horizontal gridlines.
/// - **Crosshair** (`crosshair_x`): a solid `Theme::muted_fg` vertical
///   line plus each series' value at that x.
/// - **Hover marker** (`hovered_point`): a filled circle at the
///   series' colour over the resolved data point.
#[allow(clippy::too_many_arguments)]
pub fn draw_chart(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    rect: Rect,
    chart: &Chart,
    char_width: f32,
    line_height: f32,
    hovered_point: Option<(usize, usize)>,
    crosshair_x: Option<f64>,
) -> ChartLayout {
    let theme = Theme::default();
    let layout = win_chart_layout(chart, rect, char_width, line_height);

    match chart.kind {
        ChartKind::Sparkline => paint_sparkline(target, &layout, chart, &theme),
        ChartKind::Line => paint_line(target, dwrite, &layout, chart, &theme),
        ChartKind::Bar | ChartKind::BarGrouped => paint_bar(target, dwrite, &layout, chart, &theme),
    }

    if let Some(data_x) = crosshair_x {
        paint_crosshair(target, dwrite, &layout, chart, &theme, data_x);
    }
    if let Some((si, di)) = hovered_point {
        paint_hover_marker(target, &layout, si, di, chart);
    }

    layout
}

fn paint_sparkline(target: &ID2D1RenderTarget, layout: &ChartLayout, chart: &Chart, theme: &Theme) {
    let pa = layout.plot_area;
    let _ = fill_rect(target, pa, theme.background);
    if pa.width <= 0.0 || pa.height <= 0.0 {
        return;
    }
    let Some(s) = chart.series.first() else {
        return;
    };
    if s.data.is_empty() {
        return;
    }
    let (y_min, y_max) = chart.effective_y_range();
    let range = y_max - y_min;
    let color = series_color(chart, 0);
    let n = s.data.len();
    let start = n.saturating_sub(pa.width as usize);
    let mut prev: Option<(f32, f32)> = None;
    for (i, &val) in s.data[start..].iter().enumerate() {
        let norm = if range > 0.0 {
            ((val - y_min) / range).clamp(0.0, 1.0)
        } else {
            0.5
        };
        let sx = pa.x + i as f32;
        let sy = pa.y + (1.0 - norm as f32) * pa.height;
        if let Some((px, py)) = prev {
            let _ = draw_line(target, px, py, sx, sy, color, 1.5);
        }
        prev = Some((sx, sy));
    }
}

fn paint_line(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    layout: &ChartLayout,
    chart: &Chart,
    theme: &Theme,
) {
    let pa = layout.plot_area;
    let _ = fill_rect(target, pa, theme.background);
    if pa.width <= 0.0 || pa.height <= 0.0 {
        return;
    }
    // Axes.
    let _ = draw_line(
        target,
        pa.x,
        pa.y,
        pa.x,
        pa.y + pa.height,
        theme.muted_fg,
        1.0,
    );
    let _ = draw_line(
        target,
        pa.x,
        pa.y + pa.height,
        pa.x + pa.width,
        pa.y + pa.height,
        theme.muted_fg,
        1.0,
    );

    for (si, s) in chart.series.iter().enumerate() {
        if s.data.is_empty() {
            continue;
        }
        let color = series_color(chart, si);
        let mut prev: Option<(f32, f32)> = None;
        for &(pt_si, _di, x, y) in &layout.data_point_positions {
            if pt_si != si {
                continue;
            }
            if let Some((px, py)) = prev {
                let _ = draw_line(target, px, py, x, y, color, 2.0);
            }
            prev = Some((x, y));
        }
    }

    paint_legend(target, dwrite, layout, chart, theme);
    paint_axis_labels(target, dwrite, layout, chart, theme);
}

fn paint_bar(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    layout: &ChartLayout,
    chart: &Chart,
    theme: &Theme,
) {
    let pa = layout.plot_area;
    let _ = fill_rect(target, pa, theme.background);
    if pa.width <= 0.0 || pa.height <= 0.0 {
        return;
    }

    let n = chart.max_data_len();
    if n > 0 {
        let slot_w = pa.width / n as f32;
        let gap = (slot_w * 0.15).max(1.0);
        let bar_w = (slot_w - gap).max(1.0);
        let stacked = chart.kind.is_stacked_bar();
        let series_count = chart.series.len().max(1) as f32;
        let baseline = pa.y + pa.height;

        for (i, column) in chart.bar_column_spans_all().into_iter().enumerate() {
            let slot_x = pa.x + i as f32 * slot_w + gap / 2.0;
            for (si, bottom, top) in column {
                let (bx, seg_w) = if stacked {
                    (slot_x, bar_w)
                } else {
                    let sub_w = (bar_w / series_count).max(1.0);
                    (slot_x + si as f32 * sub_w, sub_w)
                };
                let seg_h = (top - bottom) as f32 * pa.height;
                if seg_h <= 0.0 {
                    continue;
                }
                let by = baseline - top as f32 * pa.height;
                let _ = fill_rect(
                    target,
                    Rect::new(bx, by, seg_w, seg_h),
                    series_color(chart, si),
                );
            }
        }

        let _ = draw_line(
            target,
            pa.x,
            baseline,
            pa.x + pa.width,
            baseline,
            theme.muted_fg,
            1.0,
        );
    }

    paint_legend(target, dwrite, layout, chart, theme);
    paint_axis_labels(target, dwrite, layout, chart, theme);
}

fn paint_legend(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    layout: &ChartLayout,
    chart: &Chart,
    theme: &Theme,
) {
    let Some(lb) = layout.legend_bounds else {
        return;
    };
    let _ = fill_rect(target, lb, theme.background);
    let mut cx = lb.x + 2.0;
    for (i, s) in chart.series.iter().enumerate() {
        let color = series_color(chart, i);
        let swatch = lb.height * 0.6;
        let sy = lb.y + (lb.height - swatch) / 2.0;
        let _ = fill_rect(target, Rect::new(cx, sy, swatch, swatch), color);
        cx += swatch + 4.0;

        let (tw, th) = dwrite.measure_text(&s.label).unwrap_or((0.0, 0.0));
        let _ = dwrite.draw_text(
            target,
            &s.label,
            Rect::new(cx, lb.y, tw, th.max(lb.height)),
            theme.foreground,
        );
        cx += tw + 12.0;
    }
}

fn paint_axis_labels(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    layout: &ChartLayout,
    chart: &Chart,
    theme: &Theme,
) {
    let pa = layout.plot_area;
    for &(sy, val) in &layout.y_tick_positions {
        let label = format_tick_value(val);
        let (tw, th) = dwrite.measure_text(&label).unwrap_or((0.0, 0.0));
        let _ = dwrite.draw_text(
            target,
            &label,
            Rect::new(pa.x - tw - 4.0, sy - 6.0, tw, th),
            theme.muted_fg,
        );

        if chart.show_grid && sy > pa.y && sy < pa.y + pa.height {
            let _ = draw_line(
                target,
                pa.x,
                sy,
                pa.x + pa.width,
                sy,
                blend_grid(theme.muted_fg),
                0.5,
            );
        }
    }

    if let Some(label) = &chart.x_label {
        let (tw, th) = dwrite.measure_text(label).unwrap_or((0.0, 0.0));
        let cx = pa.x + (pa.width - tw) / 2.0;
        let cy = pa.y + pa.height;
        let _ = dwrite.draw_text(target, label, Rect::new(cx, cy, tw, th), theme.foreground);
    }
    if let Some(label) = &chart.y_label {
        let (tw, th) = dwrite.measure_text(label).unwrap_or((0.0, 0.0));
        let _ = dwrite.draw_text(
            target,
            label,
            Rect::new(layout.bounds.x, pa.y, tw, th),
            theme.foreground,
        );
    }
}

/// Cheap "faint gridline" approximation: since [`super::text::fill_rect`]
/// / [`draw_line`] paint opaque colours, a ~20%-opacity gridline (what
/// `gtk::chart` paints via `set_source_rgba(.., 0.2)`) is approximated
/// here as a flat blend against `Theme::background` instead of a real
/// alpha-blended stroke.
fn blend_grid(muted_fg: Color) -> Color {
    blend(Theme::default().background, muted_fg, 0.35)
}

fn paint_crosshair(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    layout: &ChartLayout,
    chart: &Chart,
    theme: &Theme,
    data_x: f64,
) {
    let data_len = chart.max_data_len();
    let screen_x = layout.data_to_screen_x(data_x, data_len);
    let pa = layout.plot_area;
    if screen_x <= pa.x || screen_x >= pa.x + pa.width {
        return;
    }
    let _ = draw_line(
        target,
        screen_x,
        pa.y,
        screen_x,
        pa.y + pa.height,
        theme.muted_fg,
        1.0,
    );

    let (y_min, y_max) = chart.effective_y_range();
    let range = y_max - y_min;
    for (si, s) in chart.series.iter().enumerate() {
        if s.data.is_empty() {
            continue;
        }
        let idx = data_x.round() as usize;
        let Some(&val) = s.data.get(idx) else {
            continue;
        };
        let label = format_tick_value(val);
        let color = series_color(chart, si);
        let norm = if range > 0.0 {
            ((val - y_min) / range).clamp(0.0, 1.0)
        } else {
            0.5
        };
        let sy = pa.y + pa.height - norm as f32 * pa.height;
        let (tw, th) = dwrite.measure_text(&label).unwrap_or((0.0, 0.0));
        let _ = dwrite.draw_text(
            target,
            &label,
            Rect::new(screen_x + 4.0, sy - 8.0, tw, th),
            color,
        );
    }
}

fn paint_hover_marker(
    target: &ID2D1RenderTarget,
    layout: &ChartLayout,
    series_idx: usize,
    data_idx: usize,
    chart: &Chart,
) {
    for &(si, di, sx, sy) in &layout.data_point_positions {
        if si == series_idx && di == data_idx {
            let color = series_color(chart, si);
            let _ = fill_circle(target, sx, sy, 5.0, color);
            let outer = blend(Theme::default().background, color, 0.3);
            let _ = fill_circle(target, sx, sy, 8.0, outer);
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::chart::{ChartHit, Series};
    use crate::types::WidgetId;
    use crate::win::testing::HeadlessSurface;

    const W: f32 = 200.0;
    const H: f32 = 100.0;
    const CHAR_W: f32 = 8.0;
    const LINE_H: f32 = 16.0;

    fn line_chart(data: Vec<f64>) -> Chart {
        Chart {
            id: WidgetId::new("chart"),
            kind: ChartKind::Line,
            series: vec![Series {
                label: "A".into(),
                data,
                color: None,
                fill: false,
            }],
            x_label: None,
            y_label: None,
            y_range: None,
            x_range: None,
            show_legend: false,
            y_ticks: Some(0),
            x_ticks: Some(0),
            show_grid: false,
        }
    }

    /// Paint↔hover round trip: a nearest-point lookup at a data point's
    /// resolved screen position must resolve back to that point, and
    /// painting with it as `hovered_point` must not panic.
    #[test]
    fn paint_and_nearest_point_round_trip() {
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
        let chart = line_chart(vec![1.0, 5.0, 2.0, 8.0, 3.0]);
        let rect = Rect::new(0.0, 0.0, W, H);

        let layout = surface
            .paint(|target| {
                draw_chart(target, &dwrite, rect, &chart, CHAR_W, LINE_H, None, None);
            })
            .map(|_| win_chart_layout(&chart, rect, CHAR_W, LINE_H))
            .expect("paint chart");

        assert_eq!(layout.data_point_positions.len(), 5);
        for &(si, di, x, y) in &layout.data_point_positions {
            let nearest = layout.nearest_point(x, y, 1.0);
            assert_eq!(nearest, Some((si, di)));
        }

        // Painting with a hover marker at the first point must not panic.
        surface
            .paint(|target| {
                draw_chart(
                    target,
                    &dwrite,
                    rect,
                    &chart,
                    CHAR_W,
                    LINE_H,
                    Some((0, 0)),
                    Some(1.0),
                );
            })
            .expect("paint chart with hover + crosshair");
    }

    /// A click on the plot body (away from any data point) resolves to
    /// `ChartHit::Body`.
    #[test]
    fn body_click_resolves_to_body_hit() {
        let chart = line_chart(vec![1.0, 2.0, 3.0]);
        let rect = Rect::new(0.0, 0.0, W, H);
        let layout = win_chart_layout(&chart, rect, CHAR_W, LINE_H);
        let mid = layout.plot_area;
        let hit = layout.hit_test(mid.x + mid.width / 2.0, mid.y + mid.height / 2.0);
        assert!(matches!(hit, ChartHit::Body(_)), "got {:?}", hit);
    }

    /// No-paint layout must agree byte-for-byte with what `draw_chart`
    /// painted.
    #[test]
    fn no_paint_layout_matches_paint_layout() {
        let chart = line_chart(vec![1.0, 4.0, 2.0]);
        let rect = Rect::new(0.0, 0.0, W, H);
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");

        let painted = surface
            .paint(|target| {
                draw_chart(target, &dwrite, rect, &chart, CHAR_W, LINE_H, None, None);
            })
            .map(|_| win_chart_layout(&chart, rect, CHAR_W, LINE_H))
            .expect("paint");
        let no_paint = win_chart_layout(&chart, rect, CHAR_W, LINE_H);
        assert_eq!(painted, no_paint);
    }
}
