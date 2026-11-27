//! The shared pixel painter for every [`Chart`] kind, written against
//! [`PaintSurface`] so GTK, macOS and Win paint a chart from this one
//! implementation.
//!
//! Layering, back to front: the plot background, the grid, area fills,
//! axes, series (polylines or bars), legend and tick labels, then the
//! crosshair and the hover marker. The whole paint is clipped to the
//! chart's own bounds so no overlay bleeds past them.
//!
//! - **Area fill ([`super::Series::fill`]).** A filled line series paints
//!   the polygon between its polyline and the plot floor as one-pixel
//!   scanline bands ([`polygon_fill_spans`], the scanline fill the canvas
//!   polygon op uses) at [`FILL_ALPHA`] of the series colour, under the
//!   line itself.
//! - **The sparkline's data cadence.** Data points stretch evenly across
//!   the plot's full width, not windowed to the most recent
//!   `plot_area.width` points. A hovered sparkline point's marker still
//!   reads its position from `layout.data_point_positions` (matching the
//!   windowed cadence), so a data point outside the window can show a
//!   hover marker slightly off the re-stretched line.
//! - **The crosshair.** A solid line with per-series value labels,
//!   approximating a 50%-alpha tint with `blend` against
//!   `theme.background`.
//! - **The grid.** Horizontal rules at the interior y ticks in
//!   `theme.separator`, painted under the data so a series is never cut
//!   by a rule.
//! - **The hover marker.** Two nested squares (the surface has no circle
//!   verb): an outer halo of the series colour blended against the live
//!   `theme.background`, then the solid inner square on top of it.
//!
//! [`SERIES_COLORS`] is an alias for the crate's one palette,
//! `theme::CHART_SERIES`; painting resolves colours through
//! [`Chart::series_color`], so the pixel and cell painters cannot drift
//! apart on them.

use super::{Chart, ChartKind, ChartLayout};
use crate::event::Point;
use crate::paint_surface::PaintSurface;
use crate::primitives::canvas::native_surface_paint::stroke_polyline;
use crate::primitives::canvas::polygon_fill_spans;
use crate::theme::Theme;
use crate::Rect;

/// The crate's one series palette as a `const`, for the backend
/// rasteriser tests that compare painted pixels against it. Painting
/// itself goes through [`Theme::chart_series`].
#[cfg(test)]
pub(crate) const SERIES_COLORS: [crate::types::Color; 6] = crate::theme::CHART_SERIES;

/// Opacity of a [`super::Series::fill`] area under its line.
pub(crate) const FILL_ALPHA: f32 = 0.25;

/// Paint a [`Chart`] (already resolved into `layout`) onto `surface`,
/// with `hovered_point` marked and a crosshair at data index
/// `crosshair_x`.
///
/// `layout` must be the same [`ChartLayout`] the caller hit-tests
/// against: geometry is read only from it, never recomputed.
pub(crate) fn paint(
    chart: &Chart,
    layout: &ChartLayout,
    surface: &mut dyn PaintSurface,
    theme: &Theme,
    hovered_point: Option<(usize, usize)>,
    crosshair_x: Option<f64>,
) {
    let b = layout.bounds;
    if b.width <= 0.0 || b.height <= 0.0 {
        return;
    }

    surface.surface_push_clip(b);

    match chart.kind {
        ChartKind::Sparkline => paint_sparkline(surface, layout, chart, theme),
        ChartKind::Line => paint_line(surface, layout, chart, theme),
        ChartKind::Bar | ChartKind::BarGrouped => paint_bar(surface, layout, chart, theme),
    }

    if let Some(data_x) = crosshair_x {
        paint_crosshair(surface, layout, chart, theme, data_x);
    }
    if let Some((si, di)) = hovered_point {
        paint_hover_marker(surface, layout, si, di, chart, theme);
    }

    surface.surface_pop_clip();
}

fn paint_sparkline(
    surface: &mut dyn PaintSurface,
    layout: &ChartLayout,
    chart: &Chart,
    theme: &Theme,
) {
    let pa = layout.plot_area;
    surface.surface_fill_rect(pa, theme.background);

    let Some(s) = chart.series.first() else {
        return;
    };
    if s.data.is_empty() || pa.width <= 0.0 || pa.height <= 0.0 {
        return;
    }
    let (y_min, y_max) = chart.effective_y_range();
    let range = y_max - y_min;
    let color = chart.series_color(0, theme);
    let n = s.data.len();
    let points: Vec<Point> = s
        .data
        .iter()
        .enumerate()
        .map(|(i, &val)| {
            let norm = if range > 0.0 {
                ((val - y_min) / range).clamp(0.0, 1.0)
            } else {
                0.5
            };
            let sx = pa.x
                + if n <= 1 {
                    0.0
                } else {
                    (i as f32 / (n - 1) as f32) * pa.width
                };
            let sy = pa.y + pa.height - norm as f32 * pa.height;
            Point::new(sx, sy)
        })
        .collect();
    stroke_polyline(surface, &points, color, 1.5);
}

/// The plot's left and bottom axis lines.
fn paint_axes(surface: &mut dyn PaintSurface, pa: Rect, theme: &Theme) {
    surface.surface_draw_line(
        Point::new(pa.x, pa.y),
        Point::new(pa.x, pa.y + pa.height),
        theme.muted_fg,
        1.0,
    );
    surface.surface_draw_line(
        Point::new(pa.x, pa.y + pa.height),
        Point::new(pa.x + pa.width, pa.y + pa.height),
        theme.muted_fg,
        1.0,
    );
}

fn paint_line(surface: &mut dyn PaintSurface, layout: &ChartLayout, chart: &Chart, theme: &Theme) {
    let pa = layout.plot_area;
    surface.surface_fill_rect(pa, theme.background);
    if pa.width <= 0.0 || pa.height <= 0.0 {
        return;
    }

    paint_grid(surface, layout, chart, theme);

    let series_points = |si: usize| -> Vec<Point> {
        layout
            .data_point_positions
            .iter()
            .filter(|&&(pt_si, ..)| pt_si == si)
            .map(|&(_, _, x, y)| Point::new(x, y))
            .collect()
    };

    for (si, s) in chart.series.iter().enumerate() {
        if !s.fill {
            continue;
        }
        let mut polygon = series_points(si);
        let (Some(first), Some(last)) = (polygon.first().copied(), polygon.last().copied()) else {
            continue;
        };
        let floor = pa.y + pa.height;
        polygon.push(Point::new(last.x, floor));
        polygon.push(Point::new(first.x, floor));
        let color = chart.series_color(si, theme);
        for band in polygon_fill_spans(&polygon, 1.0) {
            surface.surface_fill_rect_alpha(band, color, FILL_ALPHA);
        }
    }

    paint_axes(surface, pa, theme);

    for (si, s) in chart.series.iter().enumerate() {
        if s.data.is_empty() {
            continue;
        }
        stroke_polyline(
            surface,
            &series_points(si),
            chart.series_color(si, theme),
            2.0,
        );
    }

    paint_legend(surface, layout, chart, theme);
    paint_axis_labels(surface, layout, chart, theme);
}

fn paint_bar(surface: &mut dyn PaintSurface, layout: &ChartLayout, chart: &Chart, theme: &Theme) {
    let pa = layout.plot_area;
    surface.surface_fill_rect(pa, theme.background);
    if pa.width <= 0.0 || pa.height <= 0.0 {
        return;
    }

    paint_grid(surface, layout, chart, theme);

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
                surface.surface_fill_rect(
                    Rect::new(bx, by, seg_w, seg_h),
                    chart.series_color(si, theme),
                );
            }
        }

        surface.surface_draw_line(
            Point::new(pa.x, baseline),
            Point::new(pa.x + pa.width, baseline),
            theme.muted_fg,
            1.0,
        );
    }

    paint_legend(surface, layout, chart, theme);
    paint_axis_labels(surface, layout, chart, theme);
}

fn paint_legend(
    surface: &mut dyn PaintSurface,
    layout: &ChartLayout,
    chart: &Chart,
    theme: &Theme,
) {
    let Some(lb) = layout.legend_bounds else {
        return;
    };
    surface.surface_fill_rect(lb, theme.background);

    let mut cx = lb.x + 2.0;
    for (i, s) in chart.series.iter().enumerate() {
        let color = chart.series_color(i, theme);
        let swatch = lb.height * 0.6;
        let sy = lb.y + (lb.height - swatch) / 2.0;
        surface.surface_fill_rect(Rect::new(cx, sy, swatch, swatch), color);
        cx += swatch + 4.0;

        let (tw, th) = surface.surface_measure_text(&s.label);
        surface.surface_draw_text_run(
            Rect::new(cx, lb.y, tw, th.max(lb.height)),
            &s.label,
            theme.foreground,
        );
        cx += tw + 12.0;
    }
}

/// Horizontal rules at every interior y tick, when
/// [`Chart::show_grid`] is set. Painted right after the plot background
/// so fills, axes and series all sit on top of it.
fn paint_grid(surface: &mut dyn PaintSurface, layout: &ChartLayout, chart: &Chart, theme: &Theme) {
    if !chart.show_grid {
        return;
    }
    let pa = layout.plot_area;
    for &(sy, _) in &layout.y_tick_positions {
        if sy > pa.y && sy < pa.y + pa.height {
            surface.surface_draw_line(
                Point::new(pa.x, sy),
                Point::new(pa.x + pa.width, sy),
                theme.separator,
                0.5,
            );
        }
    }
}

fn paint_axis_labels(
    surface: &mut dyn PaintSurface,
    layout: &ChartLayout,
    chart: &Chart,
    theme: &Theme,
) {
    let pa = layout.plot_area;

    for &(sy, val) in &layout.y_tick_positions {
        let label = super::format_tick_value(val);
        let (tw, th) = surface.surface_measure_text(&label);
        surface.surface_draw_text_run(
            Rect::new(pa.x - tw - 4.0, sy - th / 2.0, tw, th),
            &label,
            theme.muted_fg,
        );
    }

    // X-axis ticks: centred under each tick's screen x but clamped to
    // stay inside the chart's own bounds — a tick at `frac == 1.0` sits
    // exactly on the bounds' right edge, and a naive centred label there
    // would always spill past it. A label that still collides with the
    // previous (already-clamped) one is dropped rather than painted
    // overlapping. `layout` only populates `x_tick_positions` when
    // `chart.x_ticks` is `Some(n > 0)`, which is exactly the condition
    // that reserved this row's height.
    let bounds = layout.bounds;
    let x_ticks_height = if !layout.x_tick_positions.is_empty() {
        surface.surface_line_height()
    } else {
        0.0
    };
    let mut prev_label_end: Option<f32> = None;
    for &(sx, val) in &layout.x_tick_positions {
        let label = super::format_tick_value(val);
        let (tw, th) = surface.surface_measure_text(&label);
        if tw > bounds.width {
            continue;
        }
        let max_x = bounds.x + bounds.width - tw;
        let lx = (sx - tw / 2.0).clamp(bounds.x, max_x);
        if let Some(prev_end) = prev_label_end {
            if lx < prev_end + 4.0 {
                continue;
            }
        }
        surface.surface_draw_text_run(
            Rect::new(lx, pa.y + pa.height, tw, th),
            &label,
            theme.muted_fg,
        );
        prev_label_end = Some(lx + tw);
    }

    if let Some(label) = &chart.x_label {
        let (tw, th) = surface.surface_measure_text(label);
        let cx = pa.x + (pa.width - tw) / 2.0;
        let cy = pa.y + pa.height + x_ticks_height;
        surface.surface_draw_text_run(Rect::new(cx, cy, tw, th), label, theme.foreground);
    }

    if let Some(label) = &chart.y_label {
        let (tw, th) = surface.surface_measure_text(label);
        surface.surface_draw_text_run(
            Rect::new(layout.bounds.x, pa.y, tw, th),
            label,
            theme.foreground,
        );
    }
}

fn paint_crosshair(
    surface: &mut dyn PaintSurface,
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

    let line_color = theme.background.blend(theme.muted_fg, 0.5);
    surface.surface_draw_line(
        Point::new(screen_x, pa.y),
        Point::new(screen_x, pa.y + pa.height),
        line_color,
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
        let label = super::format_tick_value(val);
        let color = chart.series_color(si, theme);
        let norm = if range > 0.0 {
            ((val - y_min) / range).clamp(0.0, 1.0)
        } else {
            0.5
        };
        let sy = pa.y + pa.height - norm as f32 * pa.height;
        let (tw, th) = surface.surface_measure_text(&label);
        surface.surface_draw_text_run(Rect::new(screen_x + 4.0, sy - 8.0, tw, th), &label, color);
    }
}

fn paint_hover_marker(
    surface: &mut dyn PaintSurface,
    layout: &ChartLayout,
    series_idx: usize,
    data_idx: usize,
    chart: &Chart,
    theme: &Theme,
) {
    let Some(&(_, _, sx, sy)) = layout
        .data_point_positions
        .iter()
        .find(|&&(si, di, _, _)| si == series_idx && di == data_idx)
    else {
        return;
    };
    let color = chart.series_color(series_idx, theme);
    let outer = 8.0_f32;
    surface.surface_fill_rect(
        Rect::new(sx - outer, sy - outer, outer * 2.0, outer * 2.0),
        theme.background.blend(color, 0.3),
    );
    let inner = 5.0_f32;
    surface.surface_fill_rect(
        Rect::new(sx - inner, sy - inner, inner * 2.0, inner * 2.0),
        color,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::ImagePaintResult;
    use crate::event::Viewport;
    use crate::primitives::chart::Series;
    use crate::types::Color;
    use crate::types::WidgetId;
    use crate::Image;

    /// Records every drawing verb `paint` issues, so a paint
    /// assertion can run on any host — no Cairo, Core Graphics or
    /// Direct2D needed. Mirrors `primitives::find_replace`'s own
    /// `RecordingSurface`: the pixel backends' real
    /// `ImageSurface`/`BitmapSurface`/`HeadlessSurface` driver tests
    /// still cover "the verb reached real pixels"; this covers "the
    /// shared painter emits the right verb at all", on every leg of
    /// the quality gate that enables gtk, win, or macos.
    #[derive(Default)]
    struct RecordingSurface {
        fills: Vec<(Rect, Color)>,
        lines: Vec<(crate::Point, crate::Point, Color, f32)>,
        texts: Vec<(Rect, String, Color)>,
        clip_pushes: Vec<Rect>,
        clip_pops: usize,
    }

    impl PaintSurface for RecordingSurface {
        fn surface_begin_frame(&mut self, _viewport: Viewport) {}
        fn surface_end_frame(&mut self) {}
        fn surface_viewport(&self) -> Viewport {
            Viewport::new(200.0, 100.0, 1.0)
        }
        fn surface_line_height(&self) -> f32 {
            16.0
        }
        fn surface_char_width(&self) -> f32 {
            8.0
        }
        fn surface_measure_text(&self, text: &str) -> (f32, f32) {
            (text.chars().count() as f32 * 8.0, 14.0)
        }
        fn surface_fill_rect(&mut self, rect: Rect, color: Color) {
            self.fills.push((rect, color));
        }
        /// Test-only recorder — `paint` never calls this verb
        /// (see this module's own doc for why no primitive here has been
        /// migrated onto it yet); this exists only so `RecordingSurface`
        /// satisfies the trait. Records into the same `fills` list as
        /// `surface_fill_rect` (radius dropped) — no test asserts on it
        /// today.
        fn surface_fill_rounded_rect(&mut self, rect: Rect, _radius: f32, color: Color) {
            self.fills.push((rect, color));
        }
        fn surface_stroke_rect(&mut self, _rect: Rect, _color: Color, _stroke_width: f32) {}
        fn surface_stroke_rounded_rect(
            &mut self,
            _rect: Rect,
            _radius: f32,
            _color: Color,
            _stroke_width: f32,
        ) {
        }
        fn surface_draw_text_run(&mut self, rect: Rect, text: &str, color: Color) {
            self.texts.push((rect, text.to_string(), color));
        }
        fn surface_draw_line(
            &mut self,
            from: crate::Point,
            to: crate::Point,
            color: Color,
            stroke_width: f32,
        ) {
            self.lines.push((from, to, color, stroke_width));
        }
        fn surface_push_clip(&mut self, rect: Rect) {
            self.clip_pushes.push(rect);
        }
        fn surface_pop_clip(&mut self) {
            self.clip_pops += 1;
        }
        fn surface_draw_image(&mut self, _rect: Rect, _image: &Image) -> ImagePaintResult {
            ImagePaintResult::Unsupported
        }
    }

    fn line_chart(data: Vec<f64>) -> Chart {
        Chart {
            id: WidgetId::new("chart"),
            kind: ChartKind::Line,
            series: vec![Series {
                label: "a".into(),
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
            hovered_point: None,
            crosshair_x: None,
        }
    }

    fn layout_for(chart: &Chart) -> ChartLayout {
        chart.layout(
            0.0,
            0.0,
            crate::primitives::chart::ChartMeasure {
                width: 100.0,
                height: 50.0,
                char_width: 8.0,
                line_height: 16.0,
            },
        )
    }

    /// Regression: every pixel backend must clip a chart's paint to its
    /// own bounds.
    #[test]
    fn paint_clips_to_the_charts_own_bounds() {
        let chart = line_chart(vec![1.0, 2.0, 3.0]);
        let layout = layout_for(&chart);
        let theme = Theme::default();
        let mut surface = RecordingSurface::default();

        paint(&chart, &layout, &mut surface, &theme, None, None);

        assert_eq!(
            surface.clip_pushes,
            vec![layout.bounds],
            "paint must push exactly one clip, at the chart's own bounds"
        );
        assert_eq!(
            surface.clip_pops, 1,
            "every surface_push_clip must be balanced by surface_pop_clip"
        );
    }

    #[test]
    fn zero_size_chart_paints_nothing_and_never_clips() {
        let chart = line_chart(vec![1.0, 2.0]);
        let layout = ChartLayout {
            bounds: Rect::new(0.0, 0.0, 0.0, 0.0),
            plot_area: Rect::new(0.0, 0.0, 0.0, 0.0),
            legend_bounds: None,
            hit_regions: Vec::new(),
            data_point_positions: Vec::new(),
            y_tick_positions: Vec::new(),
            x_tick_positions: Vec::new(),
        };
        let theme = Theme::default();
        let mut surface = RecordingSurface::default();

        paint(&chart, &layout, &mut surface, &theme, None, None);

        assert!(surface.clip_pushes.is_empty());
        assert_eq!(surface.clip_pops, 0);
        assert!(surface.fills.is_empty());
    }

    fn line_chart_with_x_ticks(
        data: Vec<f64>,
        x_range: Option<(f64, f64)>,
        x_ticks: usize,
    ) -> Chart {
        Chart {
            id: WidgetId::new("chart"),
            kind: ChartKind::Line,
            series: vec![Series {
                label: "a".into(),
                data,
                color: None,
                fill: false,
            }],
            x_label: None,
            y_label: None,
            y_range: None,
            x_range,
            show_legend: false,
            y_ticks: Some(0),
            x_ticks: Some(x_ticks),
            show_grid: false,
            hovered_point: None,
            crosshair_x: None,
        }
    }

    /// Pixel backends (GTK/win/macOS) share this `paint_axis_labels`, so
    /// a fix here covers all three at once.
    #[test]
    fn x_axis_ticks_paint_text_runs_with_their_values() {
        let chart = line_chart_with_x_ticks(vec![1.0, 2.0, 3.0], None, 1);
        let layout = layout_for(&chart);
        let theme = Theme::default();
        let mut surface = RecordingSurface::default();

        paint(&chart, &layout, &mut surface, &theme, None, None);

        let labels: Vec<&str> = surface.texts.iter().map(|(_, t, _)| t.as_str()).collect();
        assert!(
            labels.contains(&"0") && labels.contains(&"2"),
            "expected x-tick labels for data indices 0 and 2, got {labels:?}"
        );
    }

    /// An explicit `x_range` drives tick *values*, not the data index.
    #[test]
    fn x_axis_tick_values_honour_x_range() {
        let chart = line_chart_with_x_ticks(vec![1.0, 2.0, 3.0], Some((100.0, 300.0)), 1);
        let layout = layout_for(&chart);
        let theme = Theme::default();
        let mut surface = RecordingSurface::default();

        paint(&chart, &layout, &mut surface, &theme, None, None);

        let labels: Vec<&str> = surface.texts.iter().map(|(_, t, _)| t.as_str()).collect();
        assert!(
            labels.contains(&"100") && labels.contains(&"300"),
            "expected x_range-derived labels, got {labels:?}"
        );
        assert!(
            !labels.contains(&"0") && !labels.contains(&"2"),
            "index-based labels should not appear once x_range is set: {labels:?}"
        );
    }

    #[test]
    fn line_series_paints_with_its_resolved_color() {
        let chart = line_chart(vec![1.0, 4.0, 2.0]);
        let layout = layout_for(&chart);
        let theme = Theme::default();
        let mut surface = RecordingSurface::default();

        paint(&chart, &layout, &mut surface, &theme, None, None);

        assert!(
            surface
                .lines
                .iter()
                .any(|&(_, _, c, _)| c == SERIES_COLORS[0]),
            "expected at least one line segment in the series' resolved colour, got {:?}",
            surface.lines,
        );
    }

    /// Builds an n-series line chart where no series sets its own
    /// `color`, so every one of them falls through to the theme
    /// palette.
    fn uncoloured_multi_series_chart(n: usize) -> Chart {
        Chart {
            id: WidgetId::new("chart"),
            kind: ChartKind::Line,
            series: (0..n)
                .map(|i| Series {
                    label: format!("s{i}"),
                    data: vec![1.0, 4.0, 2.0],
                    color: None,
                    fill: false,
                })
                .collect(),
            x_label: None,
            y_label: None,
            y_range: None,
            x_range: None,
            show_legend: false,
            y_ticks: Some(0),
            x_ticks: Some(0),
            show_grid: false,
            hovered_point: None,
            crosshair_x: None,
        }
    }

    /// A series with no explicit `color` resolves through
    /// [`Theme::chart_series`], and more series than the palette has
    /// entries cycles back through it (`idx % 6`) rather than
    /// panicking or inventing an off-palette colour.
    #[test]
    fn uncoloured_series_cycle_through_the_themes_chart_series_palette() {
        let chart = uncoloured_multi_series_chart(8);
        let layout = layout_for(&chart);
        let theme = Theme::default();
        let palette = theme.chart_series();
        let mut surface = RecordingSurface::default();

        paint(&chart, &layout, &mut surface, &theme, None, None);

        // `surface.lines` also carries chrome (axes, grid) painted
        // in theme colours that aren't series colours, so assert on
        // palette coverage rather than on every recorded line.
        let painted: Vec<Color> = surface.lines.iter().map(|&(_, _, c, _)| c).collect();
        assert!(
            !painted.is_empty(),
            "fixture painted no line segments at all"
        );
        for expected in &palette {
            assert!(
                painted.contains(expected),
                "8 uncoloured series should cover all 6 palette entries; {expected:?} never appeared in {painted:?}",
            );
        }
    }

    /// A `Series` that *does* set its own `color` keeps it — the
    /// theme palette is only the fallback for series that don't
    #[test]
    fn explicit_series_color_wins_over_the_theme_palette() {
        let mut chart = uncoloured_multi_series_chart(1);
        let custom = Color::rgb(1, 2, 3);
        assert!(
            !Theme::default().chart_series().contains(&custom),
            "test fixture invalid: custom colour must not be in the palette"
        );
        chart.series[0].color = Some(custom);
        let layout = layout_for(&chart);
        let theme = Theme::default();
        let mut surface = RecordingSurface::default();

        paint(&chart, &layout, &mut surface, &theme, None, None);

        assert!(
            surface.lines.iter().any(|&(_, _, c, _)| c == custom),
            "expected the line to paint with the series' own colour, got {:?}",
            surface.lines,
        );
        assert!(
            surface
                .lines
                .iter()
                .all(|&(_, _, c, _)| c != theme.chart_series()[0]),
            "palette entry 0 must not be used once the series sets its own colour, got {:?}",
            surface.lines,
        );
    }

    #[test]
    fn hover_marker_paints_two_nested_fills_at_the_data_point() {
        let chart = line_chart(vec![1.0, 4.0, 2.0]);
        let layout = layout_for(&chart);
        let theme = Theme::default();
        let mut surface = RecordingSurface::default();

        paint(&chart, &layout, &mut surface, &theme, Some((0, 1)), None);

        let (_, _, sx, sy) = layout.data_point_positions[1];
        // Filter to small, marker-sized fills (<= the 16x16 outer
        // ring) centred on the data point — excludes the much
        // larger plot-area background fill, which also covers this
        // point.
        let fills_at_point = surface
            .fills
            .iter()
            .filter(|(r, _)| {
                r.width <= 16.0
                    && r.height <= 16.0
                    && r.x <= sx
                    && sx <= r.x + r.width
                    && r.y <= sy
                    && sy <= r.y + r.height
            })
            .count();
        assert_eq!(
            fills_at_point, 2,
            "hover marker should paint two nested fills (inner + outer ring) centred on the data point"
        );
    }

    /// Regression: the hover marker's outer-ring halo must blend against
    /// the *live* `theme.background` passed into `paint`, not a hardcoded
    /// default. Uses a theme whose background differs sharply from
    /// `Theme::default()`'s dark palette to verify the blend tracks the
    /// live theme.
    #[test]
    fn hover_marker_outer_ring_blends_against_the_live_theme_background() {
        let chart = line_chart(vec![1.0, 4.0, 2.0]);
        let layout = layout_for(&chart);
        let light_background = Color::rgb(240, 240, 240);
        let theme = Theme {
            background: light_background,
            ..Theme::default()
        };
        let mut surface = RecordingSurface::default();

        paint(&chart, &layout, &mut surface, &theme, Some((0, 1)), None);

        let (_, _, sx, sy) = layout.data_point_positions[1];
        let series_color = SERIES_COLORS[0];
        let expected_outer = light_background.blend(series_color, 0.3);
        let wrong_outer_from_default_theme = Theme::default().background.blend(series_color, 0.3);
        assert_ne!(
            expected_outer, wrong_outer_from_default_theme,
            "test fixture invalid: light and default-theme backgrounds must blend to different colours"
        );

        let outer_ring_fill = surface
            .fills
            .iter()
            .filter(|(r, _)| {
                r.width <= 16.0
                    && r.height <= 16.0
                    && r.width > 10.0
                    && r.x <= sx
                    && sx <= r.x + r.width
                    && r.y <= sy
                    && sy <= r.y + r.height
            })
            .map(|(_, c)| *c)
            .next();
        assert_eq!(
            outer_ring_fill,
            Some(expected_outer),
            "outer ring should blend the series colour against the live theme's background, not Theme::default()'s"
        );
    }

    /// The grid is background: every grid rule is stroked before the
    /// first series segment, so a series is never cut by a rule.
    #[test]
    fn grid_paints_under_the_series() {
        let mut chart = line_chart(vec![1.0, 4.0, 2.0]);
        chart.show_grid = true;
        chart.y_ticks = Some(4);
        let layout = layout_for(&chart);
        let theme = Theme::default();
        let mut surface = RecordingSurface::default();

        paint(&chart, &layout, &mut surface, &theme, None, None);

        let last_grid = surface
            .lines
            .iter()
            .rposition(|&(_, _, c, _)| c == theme.separator)
            .expect("show_grid paints rules");
        let first_series = surface
            .lines
            .iter()
            .position(|&(_, _, c, _)| c == SERIES_COLORS[0])
            .expect("the series paints");
        assert!(
            last_grid < first_series,
            "grid rule {last_grid} painted over series segment {first_series}"
        );
    }

    /// The hover marker's solid inner square is painted last, on top of
    /// its halo, so it stays visible.
    #[test]
    fn hover_marker_inner_square_paints_over_its_halo() {
        let chart = line_chart(vec![1.0, 4.0, 2.0]);
        let layout = layout_for(&chart);
        let theme = Theme::default();
        let mut surface = RecordingSurface::default();

        paint(&chart, &layout, &mut surface, &theme, Some((0, 1)), None);

        let (rect, color) = *surface.fills.last().expect("the marker paints");
        assert_eq!(color, SERIES_COLORS[0]);
        assert_eq!(rect.width, 10.0, "the last fill is the 10x10 inner square");
    }

    /// A `fill` series paints translucent bands of its colour between its
    /// line and the plot floor; an unfilled one paints none.
    #[test]
    fn fill_series_paints_translucent_bands_under_the_line() {
        let mut chart = line_chart(vec![1.0, 4.0, 2.0]);
        let layout = layout_for(&chart);
        let theme = Theme::default();
        let is_band = |&(_, c): &(Rect, Color)| c == SERIES_COLORS[0].with_alpha(FILL_ALPHA as f64);

        let mut unfilled = RecordingSurface::default();
        paint(&chart, &layout, &mut unfilled, &theme, None, None);
        assert!(!unfilled.fills.iter().any(is_band));

        chart.series[0].fill = true;
        let mut filled = RecordingSurface::default();
        paint(&chart, &layout, &mut filled, &theme, None, None);
        let bands: Vec<Rect> = filled
            .fills
            .iter()
            .filter(|f| is_band(f))
            .map(|(r, _)| *r)
            .collect();
        assert!(!bands.is_empty(), "a filled series paints its area");
        let floor = layout.plot_area.y + layout.plot_area.height;
        assert!(bands.iter().all(|r| r.y + r.height <= floor + 1.0));
        assert!(bands.iter().any(|r| (r.y + r.height - floor).abs() <= 1.0));
    }
}
