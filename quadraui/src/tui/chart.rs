//! TUI rasteriser for [`crate::Chart`].
//!
//! Sparklines use Unicode block elements (`▁▂▃▄▅▆▇█`). Line charts
//! use braille dots for sub-cell resolution. Bar charts use vertical
//! block stacking.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::{ratatui_color, set_cell};
use crate::primitives::chart::{Chart, ChartKind, ChartLayout, ChartMeasure};
use crate::theme::Theme;
use crate::types::Color;

const SPARK_BLOCKS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

const SERIES_COLORS: [Color; 6] = [
    Color::rgb(80, 160, 255),
    Color::rgb(255, 120, 80),
    Color::rgb(80, 220, 120),
    Color::rgb(220, 180, 60),
    Color::rgb(180, 100, 240),
    Color::rgb(240, 100, 180),
];

/// Compute the TUI cell-unit layout for a [`Chart`] without painting.
pub fn tui_chart_layout(chart: &Chart, area: Rect) -> ChartLayout {
    chart.layout(
        area.x as f32,
        area.y as f32,
        ChartMeasure {
            width: area.width as f32,
            height: area.height as f32,
            char_width: 1.0,
            line_height: 1.0,
        },
    )
}

/// Draw a [`Chart`] into `area` on `buf`. `hovered_point` carries
/// per-frame hover state so the rasteriser can highlight a data point.
/// Returns the layout for host click dispatch.
#[allow(clippy::too_many_arguments)]
pub fn draw_chart(
    buf: &mut Buffer,
    area: Rect,
    chart: &Chart,
    theme: &Theme,
    hovered_point: Option<(usize, usize)>,
    crosshair_x: Option<f64>,
) -> ChartLayout {
    let layout = tui_chart_layout(chart, area);

    if area.width == 0 || area.height == 0 {
        return layout;
    }

    match chart.kind {
        ChartKind::Sparkline => paint_sparkline(buf, &layout, chart, theme),
        ChartKind::Line => paint_line(buf, &layout, chart, theme),
        ChartKind::Bar | ChartKind::BarGrouped => paint_bar(buf, &layout, chart, theme),
    }

    if let Some(data_x) = crosshair_x {
        paint_crosshair(buf, &layout, chart, theme, data_x);
    }

    if let Some((si, di)) = hovered_point {
        paint_hover_marker(buf, &layout, si, di, chart, theme);
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

fn paint_sparkline(buf: &mut Buffer, layout: &ChartLayout, chart: &Chart, theme: &Theme) {
    let pa = &layout.plot_area;
    let px = pa.x.round() as u16;
    let py = pa.y.round() as u16;
    let pw = pa.width.round() as u16;

    let bg = ratatui_color(theme.background);

    if let Some(s) = chart.series.first() {
        if s.data.is_empty() || pw == 0 {
            return;
        }
        let (y_min, y_max) = chart.effective_y_range();
        let range = y_max - y_min;
        let fg = ratatui_color(series_color(chart, 0));
        let n = s.data.len();

        for col_idx in 0..pw as usize {
            let frac = col_idx as f64 / (pw as usize).saturating_sub(1).max(1) as f64;
            let data_pos = frac * (n - 1) as f64;
            let lo = (data_pos.floor() as usize).min(n - 1);
            let hi = (lo + 1).min(n - 1);
            let t = data_pos - lo as f64;
            let val = s.data[lo] * (1.0 - t) + s.data[hi] * t;

            let norm = if range > 0.0 {
                ((val - y_min) / range).clamp(0.0, 1.0)
            } else {
                0.5
            };
            let idx = ((norm * 7.0).round() as usize).min(7);
            set_cell(buf, px + col_idx as u16, py, SPARK_BLOCKS[idx], fg, bg);
        }
    }
}

fn paint_line(buf: &mut Buffer, layout: &ChartLayout, chart: &Chart, theme: &Theme) {
    let pa = &layout.plot_area;
    let px = pa.x.round() as u16;
    let py = pa.y.round() as u16;
    let pw = pa.width.round() as u16;
    let ph = pa.height.round() as u16;

    if pw == 0 || ph == 0 {
        return;
    }

    let bg = ratatui_color(theme.background);
    let dim = ratatui_color(theme.muted_fg);

    // Clear plot area.
    for row in py..py + ph {
        for col in px..px + pw {
            set_cell(buf, col, row, ' ', dim, bg);
        }
    }

    // Grid is background: paint it before axes and series so data and
    // axes are drawn on top of it, not cut by it (#648).
    paint_grid(buf, layout, chart, theme);

    // Axes: left edge and bottom edge.
    for row in py..py + ph {
        set_cell(buf, px, row, '│', dim, bg);
    }
    for col in px..px + pw {
        set_cell(buf, col, py + ph - 1, '─', dim, bg);
    }
    set_cell(buf, px, py + ph - 1, '└', dim, bg);

    let (y_min, y_max) = chart.effective_y_range();
    let range = y_max - y_min;
    let plot_cols = (pw.saturating_sub(1)) as usize;
    let plot_rows = (ph.saturating_sub(1)) as usize;

    if plot_cols == 0 || plot_rows == 0 {
        return;
    }

    // Braille plotting: each cell is 2 dots wide × 4 dots tall.
    let dot_w = plot_cols * 2;
    let dot_h = plot_rows * 4;

    for (si, s) in chart.series.iter().enumerate() {
        if s.data.is_empty() {
            continue;
        }
        let fg = ratatui_color(series_color(chart, si));

        let mut grid = vec![vec![false; dot_w]; dot_h];

        for (i, &val) in s.data.iter().enumerate() {
            let norm = if range > 0.0 {
                ((val - y_min) / range).clamp(0.0, 1.0)
            } else {
                0.5
            };
            let dx = if s.data.len() <= 1 {
                0
            } else {
                (i * (dot_w.saturating_sub(1))) / (s.data.len() - 1)
            };
            let dy = ((1.0 - norm) * (dot_h.saturating_sub(1)) as f64).round() as usize;
            let dx = dx.min(dot_w.saturating_sub(1));
            let dy = dy.min(dot_h.saturating_sub(1));
            grid[dy][dx] = true;

            // Connect consecutive points with intermediate dots.
            if i > 0 {
                let prev_norm = if range > 0.0 {
                    ((s.data[i - 1] - y_min) / range).clamp(0.0, 1.0)
                } else {
                    0.5
                };
                let prev_dx = if s.data.len() <= 1 {
                    0
                } else {
                    ((i - 1) * (dot_w.saturating_sub(1))) / (s.data.len() - 1)
                };
                let prev_dy =
                    ((1.0 - prev_norm) * (dot_h.saturating_sub(1)) as f64).round() as usize;
                interpolate_dots(&mut grid, prev_dx, prev_dy, dx, dy);
            }
        }

        // Render braille grid to buffer.
        for cell_row in 0..plot_rows {
            for cell_col in 0..plot_cols {
                let mut code: u32 = 0x2800;
                for (bit, &(dr, dc)) in BRAILLE_OFFSETS.iter().enumerate() {
                    let gy = cell_row * 4 + dr;
                    let gx = cell_col * 2 + dc;
                    if gy < dot_h && gx < dot_w && grid[gy][gx] {
                        code |= 1 << bit;
                    }
                }
                if code != 0x2800 {
                    let ch = char::from_u32(code).unwrap_or(' ');
                    let bx = px + 1 + cell_col as u16;
                    let by = py + cell_row as u16;
                    if bx < px + pw && by < py + ph - 1 {
                        set_cell(buf, bx, by, ch, fg, bg);
                    }
                }
            }
        }
    }

    paint_legend(buf, layout, chart, theme);
    paint_axis_labels(buf, layout, chart, theme);
}

// Braille dot offsets: (row_in_cell, col_in_cell) → bit index.
// Standard Unicode braille ordering.
const BRAILLE_OFFSETS: [(usize, usize); 8] = [
    (0, 0), // bit 0
    (1, 0), // bit 1
    (2, 0), // bit 2
    (0, 1), // bit 3
    (1, 1), // bit 4
    (2, 1), // bit 5
    (3, 0), // bit 6
    (3, 1), // bit 7
];

fn interpolate_dots(grid: &mut [Vec<bool>], x0: usize, y0: usize, x1: usize, y1: usize) {
    let dx = (x1 as isize - x0 as isize).abs();
    let dy = (y1 as isize - y0 as isize).abs();
    let steps = dx.max(dy);
    if steps == 0 {
        return;
    }
    for step in 0..=steps {
        let t = step as f64 / steps as f64;
        let ix = (x0 as f64 + t * (x1 as f64 - x0 as f64)).round() as usize;
        let iy = (y0 as f64 + t * (y1 as f64 - y0 as f64)).round() as usize;
        if iy < grid.len() && ix < grid[0].len() {
            grid[iy][ix] = true;
        }
    }
}

fn paint_bar(buf: &mut Buffer, layout: &ChartLayout, chart: &Chart, theme: &Theme) {
    let pa = &layout.plot_area;
    let px = pa.x.round() as u16;
    let py = pa.y.round() as u16;
    let pw = pa.width.round() as u16;
    let ph = pa.height.round() as u16;

    if pw == 0 || ph == 0 {
        return;
    }

    let bg = ratatui_color(theme.background);
    let dim = ratatui_color(theme.muted_fg);

    // Clear plot area.
    for row in py..py + ph {
        for col in px..px + pw {
            set_cell(buf, col, row, ' ', dim, bg);
        }
    }

    // Grid is background: paint it before any bars are drawn so bars sit
    // on top of it, not cut by it (#648). Painted even for an empty
    // chart (below) so the grid stays consistent regardless of data.
    paint_grid(buf, layout, chart, theme);

    let n = chart.max_data_len();
    if n == 0 {
        // Nothing to plot: still paint the legend and axis labels so an
        // empty bar chart reads as an empty chart, matching the GTK and
        // macOS painters.
        paint_legend(buf, layout, chart, theme);
        paint_axis_labels(buf, layout, chart, theme);
        return;
    }

    // Cell-quantised bar geometry. Slot width floors, exactly as the
    // single-series painter always did, so single-series output is
    // byte-identical to pre-#584.
    let slot_w = ((pw as usize) / n).max(1);
    let plot_h = ph.saturating_sub(1) as usize;
    let stacked = chart.kind.is_stacked_bar();
    let series_count = chart.series.len().max(1);
    // Row 0 of a bar is the row directly above the axis row.
    let base_row = (py + ph).saturating_sub(2);

    for (di, column) in chart.bar_column_spans_all().into_iter().enumerate() {
        let slot_x = di * slot_w;
        if slot_x >= pw as usize {
            break;
        }
        let slot_cells = slot_w.min(pw as usize - slot_x);

        for (si, bottom, top) in column {
            // Stacked segments span the whole slot; grouped series each
            // take a sub-slot beside the previous one.
            let (cell_off, cell_w) = if stacked {
                (0, slot_cells)
            } else {
                let sub_w = (slot_cells / series_count).max(1);
                let off = si * sub_w;
                if off >= slot_cells {
                    // Slot too narrow for the remaining series.
                    break;
                }
                (off, sub_w.min(slot_cells - off))
            };

            let row_bottom = (bottom * plot_h as f64).round() as usize;
            let row_top = (top * plot_h as f64).round() as usize;
            if row_top <= row_bottom || plot_h == 0 {
                continue;
            }

            let fg = ratatui_color(series_color(chart, si));
            for r in row_bottom..row_top.min(plot_h) {
                let by = base_row.saturating_sub(r as u16);
                if by < py {
                    break;
                }
                for c in 0..cell_w {
                    set_cell(buf, px + (slot_x + cell_off + c) as u16, by, '█', fg, bg);
                }
            }
        }
    }

    // Bottom axis.
    for col in px..px + pw {
        set_cell(buf, col, py + ph - 1, '─', dim, bg);
    }

    paint_legend(buf, layout, chart, theme);
    paint_axis_labels(buf, layout, chart, theme);
}

fn paint_legend(buf: &mut Buffer, layout: &ChartLayout, chart: &Chart, theme: &Theme) {
    if let Some(lb) = &layout.legend_bounds {
        let ly = lb.y.round() as u16;
        let lx = lb.x.round() as u16;
        let lw = lb.width.round() as u16;
        let bg = ratatui_color(theme.background);
        let fg = ratatui_color(theme.foreground);

        // Clear legend row.
        for col in lx..lx + lw {
            set_cell(buf, col, ly, ' ', fg, bg);
        }

        let mut col = lx;
        for (i, s) in chart.series.iter().enumerate() {
            if col >= lx + lw {
                break;
            }
            let sc = ratatui_color(series_color(chart, i));
            set_cell(buf, col, ly, '■', sc, bg);
            col += 1;

            for ch in s.label.chars() {
                if col >= lx + lw {
                    break;
                }
                set_cell(buf, col, ly, ch, fg, bg);
                col += 1;
            }
            col += 1; // gap between entries
        }
    }
}

/// Paint the horizontal grid rules at each y-tick row. Background layer:
/// must run *before* axes and series are painted so data sits on top of
/// the grid rather than being cut by it (#648). Mirrors the macOS
/// rasteriser's grid-then-series ordering (`macos/chart.rs`).
fn paint_grid(buf: &mut Buffer, layout: &ChartLayout, chart: &Chart, theme: &Theme) {
    if !chart.show_grid {
        return;
    }
    let bg = ratatui_color(theme.background);
    let dim = ratatui_color(theme.muted_fg);
    let pa = &layout.plot_area;
    let px = pa.x.round() as u16;
    let pw = pa.width.round() as u16;

    for &(sy, _val) in &layout.y_tick_positions {
        let row = sy.round() as u16;
        if row > pa.y.round() as u16 && row < (pa.y + pa.height).round() as u16 {
            for col in (px + 1)..(px + pw) {
                set_cell(buf, col, row, '┄', dim, bg);
            }
        }
    }
}

fn paint_axis_labels(buf: &mut Buffer, layout: &ChartLayout, chart: &Chart, theme: &Theme) {
    let bg = ratatui_color(theme.background);
    let fg = ratatui_color(theme.foreground);
    let dim = ratatui_color(theme.muted_fg);
    let pa = &layout.plot_area;
    let px = pa.x.round() as u16;
    let pw = pa.width.round() as u16;

    // Right-align each label against the gutter's right edge, but never
    // let it start left of the chart's own bounds (#647): a label wider
    // than the gutter has its leading characters dropped instead of
    // spilling onto whatever the enclosing panel painted there.
    let bounds_x = layout.bounds.x.round() as u16;
    for &(sy, val) in &layout.y_tick_positions {
        let row = sy.round() as u16;
        let label = crate::primitives::chart::format_tick_value(val);
        let gutter_end = px.saturating_sub(1);
        let desired_start = gutter_end.saturating_sub(label.len() as u16);
        let label_start = desired_start.max(bounds_x);
        let skip = (label_start - desired_start) as usize;
        for (i, ch) in label.chars().skip(skip).enumerate() {
            let col = label_start + i as u16;
            if col < gutter_end {
                set_cell(buf, col, row, ch, dim, bg);
            }
        }
    }

    if let Some(label) = &chart.x_label {
        let label_y = (pa.y + pa.height).round() as u16;
        let label_x = px + pw.saturating_sub(label.len() as u16) / 2;
        for (i, ch) in label.chars().enumerate() {
            let col = label_x + i as u16;
            if col < (layout.bounds.x + layout.bounds.width).round() as u16 {
                set_cell(buf, col, label_y, ch, fg, bg);
            }
        }
    }

    if let Some(label) = &chart.y_label {
        let label_x = layout.bounds.x.round() as u16;
        let label_y = pa.y.round() as u16;
        for (i, ch) in label.chars().enumerate() {
            let col = label_x + i as u16;
            if col < px {
                set_cell(buf, col, label_y, ch, fg, bg);
            }
        }
    }
}

fn paint_crosshair(
    buf: &mut Buffer,
    layout: &ChartLayout,
    chart: &Chart,
    theme: &Theme,
    data_x: f64,
) {
    let data_len = chart.max_data_len();
    let screen_x = layout.data_to_screen_x(data_x, data_len);
    let col = screen_x.round() as u16;
    let pa = &layout.plot_area;
    let py = pa.y.round() as u16;
    let ph = pa.height.round() as u16;
    let px = pa.x.round() as u16;
    let pw = pa.width.round() as u16;

    if col <= px || col >= px + pw || ph == 0 {
        return;
    }

    let dim = ratatui_color(theme.muted_fg);
    let bg = ratatui_color(theme.background);
    for row in py..py + ph.saturating_sub(1) {
        set_cell(buf, col, row, '│', dim, bg);
    }
}

fn paint_hover_marker(
    buf: &mut Buffer,
    layout: &ChartLayout,
    series_idx: usize,
    data_idx: usize,
    chart: &Chart,
    theme: &Theme,
) {
    let pa = &layout.plot_area;
    let px = pa.x.round() as u16;
    let py = pa.y.round() as u16;
    let pw = pa.width.round() as u16;
    let ph = pa.height.round() as u16;

    let s = match chart.series.get(series_idx) {
        Some(s) if data_idx < s.data.len() => s,
        _ => return,
    };
    let val = s.data[data_idx];
    let (y_min, y_max) = chart.effective_y_range();
    let range = y_max - y_min;
    let norm = if range > 0.0 {
        ((val - y_min) / range).clamp(0.0, 1.0)
    } else {
        0.5
    };
    let n = s.data.len();

    let fg = ratatui_color(series_color(chart, series_idx));
    let bg = ratatui_color(theme.background);

    let (col, row) = match chart.kind {
        ChartKind::Sparkline => {
            let frac = if pw <= 1 {
                0.0
            } else {
                data_idx as f32 / (n - 1).max(1) as f32
            };
            (px + (frac * (pw - 1) as f32).round() as u16, py)
        }
        ChartKind::Bar | ChartKind::BarGrouped => {
            // Bars anchor on their own rectangle, so reuse the layout's
            // per-segment positions rather than the line/braille math —
            // the marker lands on the hovered stack segment (#584).
            let pos = layout
                .data_point_positions
                .iter()
                .find(|&&(s, d, _, _)| s == series_idx && d == data_idx);
            let &(_, _, sx, sy) = match pos {
                Some(p) => p,
                None => return,
            };
            if pw == 0 || ph == 0 {
                return;
            }
            let col = (sx.round() as u16).clamp(px, px + pw - 1);
            let row = (sy.round() as u16).clamp(py, py + ph.saturating_sub(2));
            (col, row)
        }
        ChartKind::Line => {
            let plot_cols = pw.saturating_sub(1) as usize;
            let plot_rows = ph.saturating_sub(1) as usize;
            if plot_cols == 0 || plot_rows == 0 {
                return;
            }
            let dot_w = plot_cols * 2;
            let dot_h = plot_rows * 4;
            let dx = if n <= 1 {
                0
            } else {
                (data_idx * dot_w.saturating_sub(1)) / (n - 1)
            };
            let dy = ((1.0 - norm) * dot_h.saturating_sub(1) as f64).round() as usize;
            let cell_col = dx / 2;
            let cell_row = dy / 4;
            (px + 1 + cell_col as u16, py + cell_row as u16)
        }
    };

    let buf_area = buf.area;
    if col < buf_area.x + buf_area.width && row < buf_area.y + buf_area.height {
        set_cell(buf, col, row, '●', fg, bg);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::chart::{ChartHit, Series};
    use crate::types::WidgetId;

    fn spark(data: Vec<f64>) -> Chart {
        Chart {
            id: WidgetId::new("c"),
            kind: ChartKind::Sparkline,
            series: vec![Series {
                label: String::new(),
                data,
                color: None,
                fill: false,
            }],
            x_label: None,
            y_label: None,
            y_range: None,
            x_range: None,
            show_legend: false,
            y_ticks: None,
            x_ticks: None,
            show_grid: false,
        }
    }

    fn cell_char(buf: &Buffer, x: u16, y: u16) -> char {
        buf[(x, y)].symbol().chars().next().unwrap_or(' ')
    }

    /// Shared body for `sparkline_paint_and_click_round_trip[_at_nonzero_origin]`:
    /// `tui_chart_layout` bakes `area.x`/`area.y` into `Chart::layout`'s
    /// `origin_x`/`origin_y` (absolute frame), so paint + click must agree
    /// at a non-zero origin too, per LESSONS.md's "layout helpers must
    /// return coords in the same frame across backends" (quadraui#494).
    fn sparkline_paint_and_click_round_trip_at(origin_x: u16, origin_y: u16) {
        let area = Rect::new(origin_x, origin_y, 5, 1);
        let mut buf = Buffer::empty(area);
        let chart = spark(vec![0.0, 0.25, 0.5, 0.75, 1.0]);
        let layout = draw_chart(&mut buf, area, &chart, &Theme::default(), None, None);

        assert_eq!(cell_char(&buf, origin_x, origin_y), '▁');
        assert_eq!(cell_char(&buf, origin_x + 4, origin_y), '█');

        let (ox, oy) = (origin_x as f32, origin_y as f32);
        assert_eq!(
            layout.hit_test(ox + 2.5, oy + 0.5),
            ChartHit::Body(WidgetId::new("c"))
        );
        assert_eq!(layout.hit_test(ox + 10.0, oy + 0.5), ChartHit::Empty);
    }

    #[test]
    fn sparkline_paint_and_click_round_trip() {
        sparkline_paint_and_click_round_trip_at(0, 0);
    }

    #[test]
    fn sparkline_paint_and_click_round_trip_at_nonzero_origin() {
        sparkline_paint_and_click_round_trip_at(7, 13);
    }

    #[test]
    fn sparkline_max_value_gets_full_block() {
        let area = Rect::new(0, 0, 3, 1);
        let mut buf = Buffer::empty(area);
        let chart = spark(vec![10.0, 10.0, 10.0]);
        let _layout = draw_chart(&mut buf, area, &chart, &Theme::default(), None, None);
        // Flat data: all mid-height.
        for col in 0..3 {
            assert_ne!(cell_char(&buf, col, 0), ' ');
        }
    }

    #[test]
    fn sparkline_empty_data_no_crash() {
        let area = Rect::new(0, 0, 10, 1);
        let mut buf = Buffer::empty(area);
        let chart = spark(vec![]);
        let _layout = draw_chart(&mut buf, area, &chart, &Theme::default(), None, None);
        assert_eq!(cell_char(&buf, 0, 0), ' ');
    }

    #[test]
    fn line_chart_paints_braille() {
        let area = Rect::new(0, 0, 20, 10);
        let mut buf = Buffer::empty(area);
        let chart = Chart {
            id: WidgetId::new("c"),
            kind: ChartKind::Line,
            series: vec![Series {
                label: "A".into(),
                data: vec![0.0, 5.0, 10.0, 5.0, 0.0],
                color: None,
                fill: false,
            }],
            x_label: None,
            y_label: None,
            y_range: None,
            x_range: None,
            show_legend: false,
            y_ticks: None,
            x_ticks: None,
            show_grid: false,
        };
        let layout = draw_chart(&mut buf, area, &chart, &Theme::default(), None, None);

        assert!(layout.plot_area.width > 0.0);
        assert!(layout.plot_area.height > 0.0);
        assert_eq!(
            layout.hit_test(5.0, 5.0),
            ChartHit::Body(WidgetId::new("c"))
        );
    }

    /// Shared body for `bar_chart_paint_and_click_round_trip[_at_nonzero_origin]`
    /// — see `sparkline_paint_and_click_round_trip_at` for the non-zero-origin
    /// rationale (quadraui#494).
    fn bar_chart_paint_and_click_round_trip_at(origin_x: u16, origin_y: u16) {
        let area = Rect::new(origin_x, origin_y, 10, 5);
        // A buffer exactly matching `area`: since #647, y-axis tick-label
        // painting is clamped to `bounds.x` and can no longer spill left
        // of the widget's own area, so this no longer needs the
        // full-screen buffer the pre-fix rasteriser required.
        let mut buf = Buffer::empty(area);
        let chart = Chart {
            id: WidgetId::new("c"),
            kind: ChartKind::Bar,
            series: vec![Series {
                label: "B".into(),
                data: vec![1.0, 3.0, 2.0],
                color: None,
                fill: false,
            }],
            x_label: None,
            y_label: None,
            y_range: None,
            x_range: None,
            show_legend: false,
            y_ticks: None,
            x_ticks: None,
            show_grid: false,
        };
        let layout = draw_chart(&mut buf, area, &chart, &Theme::default(), None, None);

        // Bar for max value (3.0) should have filled cells.
        let pa = &layout.plot_area;
        let bar_x = pa.x.round() as u16 + 3; // second bar starts around col 3
        let bar_y = pa.y.round() as u16 + (pa.height.round() as u16) - 2;
        assert_eq!(cell_char(&buf, bar_x, bar_y), '█');

        // No y_label/legend, so the plot area's origin equals `area`'s.
        let (ox, oy) = (origin_x as f32, origin_y as f32);
        assert_eq!(
            layout.hit_test(ox + 5.0, oy + 2.0),
            ChartHit::Body(WidgetId::new("c"))
        );
    }

    #[test]
    fn bar_chart_paint_and_click_round_trip() {
        bar_chart_paint_and_click_round_trip_at(0, 0);
    }

    #[test]
    fn bar_chart_paint_and_click_round_trip_at_nonzero_origin() {
        bar_chart_paint_and_click_round_trip_at(7, 13);
    }

    /// #647 regression: y-range `0..18` over the default 5 ticks formats
    /// the interior ticks as `3.6`, `7.2`, `10.8`, `14.4` — 4 characters,
    /// wider than either endpoint (`0`/`18`). Pre-fix, the gutter was
    /// sized from the endpoints alone and these interior labels spilled
    /// left of `area.x` onto whatever the host buffer held there. The
    /// buffer here starts at the screen's own origin (not `area`'s), the
    /// same shape the pre-fix `bar_chart_paint_and_click_round_trip_at`
    /// and `legend_paint_and_click_round_trip_at` needed to tolerate the
    /// spill — this is the direct inverse: it now asserts nothing left
    /// of `area.x` was ever painted.
    #[test]
    fn y_axis_tick_labels_never_paint_left_of_area_bounds() {
        let origin_x = 5u16;
        let origin_y = 2u16;
        let area = Rect::new(origin_x, origin_y, 20, 8);
        let mut buf = Buffer::empty(Rect::new(
            0,
            0,
            origin_x + area.width,
            origin_y + area.height,
        ));
        let chart = Chart {
            id: WidgetId::new("c"),
            kind: ChartKind::Bar,
            series: vec![Series {
                label: "B".into(),
                data: vec![1.0, 5.0, 9.0, 13.0, 17.0],
                color: None,
                fill: false,
            }],
            x_label: None,
            y_label: None,
            y_range: Some((0.0, 18.0)),
            x_range: None,
            show_legend: false,
            y_ticks: None, // default 5
            x_ticks: None,
            show_grid: false,
        };
        let _layout = draw_chart(&mut buf, area, &chart, &Theme::default(), None, None);

        for y in 0..buf.area.height {
            for x in 0..origin_x {
                assert_eq!(
                    cell_char(&buf, x, y),
                    ' ',
                    "cell ({x},{y}) left of area.x={origin_x} was painted"
                );
            }
        }
    }

    /// Shared body for `legend_paint_and_click_round_trip[_at_nonzero_origin]`
    /// — see `sparkline_paint_and_click_round_trip_at` for the non-zero-origin
    /// rationale (quadraui#494).
    fn legend_paint_and_click_round_trip_at(origin_x: u16, origin_y: u16) {
        let area = Rect::new(origin_x, origin_y, 30, 10);
        // See `bar_chart_paint_and_click_round_trip_at`: since #647
        // axis-label painting can no longer spill left of `area.x`, so a
        // buffer exactly matching `area` is enough.
        let mut buf = Buffer::empty(area);
        let chart = Chart {
            id: WidgetId::new("c"),
            kind: ChartKind::Line,
            series: vec![
                Series {
                    label: "CPU".into(),
                    data: vec![1.0, 2.0],
                    color: None,
                    fill: false,
                },
                Series {
                    label: "Mem".into(),
                    data: vec![3.0, 4.0],
                    color: None,
                    fill: false,
                },
            ],
            x_label: None,
            y_label: None,
            y_range: None,
            x_range: None,
            show_legend: true,
            y_ticks: None,
            x_ticks: None,
            show_grid: false,
        };
        let layout = draw_chart(&mut buf, area, &chart, &Theme::default(), None, None);

        let lb = layout.legend_bounds.unwrap();
        assert_eq!(
            cell_char(&buf, lb.x.round() as u16, lb.y.round() as u16),
            '■'
        );

        let mid = lb.x + lb.width / 4.0;
        assert_eq!(
            layout.hit_test(mid, lb.y + 0.5),
            ChartHit::Legend(WidgetId::new("c"), 0)
        );
    }

    #[test]
    fn legend_paint_and_click_round_trip() {
        legend_paint_and_click_round_trip_at(0, 0);
    }

    #[test]
    fn legend_paint_and_click_round_trip_at_nonzero_origin() {
        legend_paint_and_click_round_trip_at(7, 13);
    }

    // ── Multi-series bars (#584) ────────────────────────────────────────

    fn series_of(label: &str, data: Vec<f64>, color: Color) -> Series {
        Series {
            label: label.into(),
            data,
            color: Some(color),
            fill: false,
        }
    }

    fn bar_chart_of(kind: ChartKind, series: Vec<Series>, y_range: Option<(f64, f64)>) -> Chart {
        Chart {
            id: WidgetId::new("c"),
            kind,
            series,
            x_label: None,
            y_label: None,
            y_range,
            x_range: None,
            show_legend: false,
            y_ticks: None,
            x_ticks: None,
            show_grid: false,
        }
    }

    /// The painted grid as one string per row, `.` for blank cells.
    fn grid(buf: &Buffer, area: Rect) -> Vec<String> {
        (area.y..area.y + area.height)
            .map(|y| {
                (area.x..area.x + area.width)
                    .map(|x| match cell_char(buf, x, y) {
                        ' ' => '.',
                        c => c,
                    })
                    .collect()
            })
            .collect()
    }

    fn fg_at(buf: &Buffer, x: u16, y: u16) -> ratatui::style::Color {
        buf[(x, y)].fg
    }

    /// #584 is additive: a single-series `Bar` must paint exactly what
    /// it painted before stacking existed. The grid below (y-tick
    /// gutter on the left, bars floored at the auto-range minimum,
    /// slot width `pw / n` floored) is the pre-#584 output, pinned.
    #[test]
    fn single_series_bar_grid_is_pinned() {
        let area = Rect::new(0, 0, 10, 5);
        let mut buf = Buffer::empty(area);
        let chart = bar_chart_of(
            ChartKind::Bar,
            vec![series_of("B", vec![1.0, 3.0, 2.0], Color::rgb(1, 2, 3))],
            None,
        );
        let _ = draw_chart(&mut buf, area, &chart, &Theme::default(), None, None);
        // #647: the gutter is now sized from the widest *interior* tick
        // label ("1.4"/"1.8"/"2.2"/"2.6", 3 chars) rather than just the
        // endpoints ("1"/"3", 1 char), so it's a column wider than
        // pre-fix and every column below shifts right to match.
        assert_eq!(
            grid(&buf, area),
            vec![
                "..3...██..",
                "2.6...██..",
                "2.2...████",
                "1.8...████",
                "1.4.──────",
            ]
        );
    }

    /// Review round 2 on #584: a single-series `Bar` whose data is all
    /// negative auto-derives the range (-5, -1), where `norm(0.0)`
    /// clamps to the plot *ceiling*. The stacked baseline used to be
    /// applied here too, collapsing every bar to zero height — an empty
    /// plot area. Bars must still rise from the floor at 0% / 50% / 100%.
    #[test]
    fn single_series_bar_with_negative_data_still_paints() {
        let area = Rect::new(0, 0, 10, 5);
        let mut buf = Buffer::empty(area);
        let chart = bar_chart_of(
            ChartKind::Bar,
            vec![series_of("B", vec![-5.0, -3.0, -1.0], Color::rgb(1, 2, 3))],
            None,
        );
        let _ = draw_chart(&mut buf, area, &chart, &Theme::default(), None, None);
        // #647: the gutter now reserves room for the widest interior
        // tick label ("-4.2", 4 chars) rather than just the endpoints
        // ("-5"/"-1", 2 chars), shifting every column right by 1.
        assert_eq!(
            grid(&buf, area),
            vec![
                "..-1...█..",
                "-1.8...█..",
                "-2.6..██..",
                "-3.4..██..",
                "-4.2.─────",
            ]
        );
    }

    #[test]
    fn stacked_bar_paints_every_series_with_its_own_colour() {
        // 3 series × 1.0 over an explicit 0..3 range ⇒ each owns two of
        // the six plot rows, bottom-up in `series` order.
        let (red, green, blue) = (
            Color::rgb(255, 0, 0),
            Color::rgb(0, 255, 0),
            Color::rgb(0, 0, 255),
        );
        let area = Rect::new(0, 0, 12, 7);
        let mut buf = Buffer::empty(area);
        let chart = bar_chart_of(
            ChartKind::Bar,
            vec![
                series_of("a", vec![1.0], red),
                series_of("b", vec![1.0], green),
                series_of("c", vec![1.0], blue),
            ],
            Some((0.0, 3.0)),
        );
        let layout = draw_chart(&mut buf, area, &chart, &Theme::default(), None, None);
        let px = layout.plot_area.x.round() as u16;
        let py = layout.plot_area.y.round() as u16;
        let ph = layout.plot_area.height.round() as u16;

        // Six stacked rows above the axis row, two per series.
        let expected = [
            (py + ph - 2, red),
            (py + ph - 3, red),
            (py + ph - 4, green),
            (py + ph - 5, green),
            (py + ph - 6, blue),
            (py + ph - 7, blue),
        ];
        for (row, color) in expected {
            assert_eq!(
                cell_char(&buf, px, row),
                '█',
                "row {row} of the stack should be filled:\n{:?}",
                grid(&buf, area)
            );
            assert_eq!(
                fg_at(&buf, px, row),
                ratatui_color(color),
                "row {row} should carry its own series colour:\n{:?}",
                grid(&buf, area)
            );
        }
    }

    #[test]
    fn stacked_bar_ceiling_is_the_column_total() {
        // Auto-range: totals are 3.0, so the tallest stack fills the
        // plot and no segment is clipped off the top.
        let area = Rect::new(0, 0, 12, 7);
        let mut buf = Buffer::empty(area);
        let chart = bar_chart_of(
            ChartKind::Bar,
            vec![
                series_of("a", vec![1.0], Color::rgb(255, 0, 0)),
                series_of("b", vec![1.0], Color::rgb(0, 255, 0)),
                series_of("c", vec![1.0, 0.0], Color::rgb(0, 0, 255)),
            ],
            None,
        );
        assert_eq!(chart.effective_y_range(), (0.0, 3.0));
        let layout = draw_chart(&mut buf, area, &chart, &Theme::default(), None, None);
        let px = layout.plot_area.x.round() as u16;
        let py = layout.plot_area.y.round() as u16;
        // Topmost plot row of the first column is painted by series 2.
        assert_eq!(cell_char(&buf, px, py), '█');
        assert_eq!(fg_at(&buf, px, py), ratatui_color(Color::rgb(0, 0, 255)));
    }

    #[test]
    fn stacked_bar_all_zero_series_does_not_shift_the_others() {
        let (red, blue) = (Color::rgb(255, 0, 0), Color::rgb(0, 0, 255));
        let area = Rect::new(0, 0, 12, 7);
        let render = |series: Vec<Series>| {
            let mut buf = Buffer::empty(area);
            let chart = bar_chart_of(ChartKind::Bar, series, Some((0.0, 3.0)));
            let _ = draw_chart(&mut buf, area, &chart, &Theme::default(), None, None);
            grid(&buf, area)
        };

        let without_zeros = render(vec![
            series_of("a", vec![2.0, 1.0], red),
            series_of("c", vec![1.0, 2.0], blue),
        ]);
        let with_zeros = render(vec![
            series_of("a", vec![2.0, 1.0], red),
            series_of("zero", vec![0.0, 0.0], Color::rgb(9, 9, 9)),
            series_of("c", vec![1.0, 2.0], blue),
        ]);
        assert_eq!(
            with_zeros, without_zeros,
            "an all-zero series must occupy no rows and move nothing"
        );
    }

    #[test]
    fn grouped_bar_paints_series_side_by_side() {
        let (red, green, blue) = (
            Color::rgb(255, 0, 0),
            Color::rgb(0, 255, 0),
            Color::rgb(0, 0, 255),
        );
        // Plot is 12 wide → one 12-cell slot split into three 4-cell
        // sub-bars, each with its own height.
        let area = Rect::new(0, 0, 14, 7);
        let mut buf = Buffer::empty(area);
        let chart = bar_chart_of(
            ChartKind::BarGrouped,
            vec![
                series_of("a", vec![1.0], red),
                series_of("b", vec![2.0], green),
                series_of("c", vec![3.0], blue),
            ],
            Some((0.0, 3.0)),
        );
        // Grouped mode keeps the single-value ceiling.
        assert_eq!(chart.effective_y_range(), (0.0, 3.0));
        let layout = draw_chart(&mut buf, area, &chart, &Theme::default(), None, None);
        let px = layout.plot_area.x.round() as u16;
        let py = layout.plot_area.y.round() as u16;
        let ph = layout.plot_area.height.round() as u16;
        let pw = layout.plot_area.width.round() as u16;
        let sub_w = pw / 3;

        let bottom = py + ph - 2;
        for (i, color) in [red, green, blue].into_iter().enumerate() {
            let col = px + sub_w * i as u16;
            assert_eq!(
                fg_at(&buf, col, bottom),
                ratatui_color(color),
                "sub-bar {i} owns its own columns:\n{:?}",
                grid(&buf, area)
            );
        }
        // Heights rise 1 → 2 → 3 across the sub-bars: the shortest does
        // not reach the row the tallest does.
        let top = py;
        assert_eq!(cell_char(&buf, px + sub_w * 2, top), '█');
        assert_eq!(cell_char(&buf, px, top), ' ');
    }

    #[test]
    fn bar_hover_marker_lands_on_the_hovered_stack_segment() {
        let area = Rect::new(0, 0, 12, 7);
        let chart = bar_chart_of(
            ChartKind::Bar,
            vec![
                series_of("a", vec![1.0], Color::rgb(255, 0, 0)),
                series_of("b", vec![1.0], Color::rgb(0, 255, 0)),
                series_of("c", vec![1.0], Color::rgb(0, 0, 255)),
            ],
            Some((0.0, 3.0)),
        );
        let marker_row = |hovered: (usize, usize)| {
            let mut buf = Buffer::empty(area);
            let layout = draw_chart(
                &mut buf,
                area,
                &chart,
                &Theme::default(),
                Some(hovered),
                None,
            );
            let px = layout.plot_area.x.round() as u16;
            let pw = layout.plot_area.width.round() as u16;
            let py = layout.plot_area.y.round() as u16;
            let ph = layout.plot_area.height.round() as u16;
            let row = (py..py + ph)
                .find(|&row| (px..px + pw).any(|col| cell_char(&buf, col, row) == '●'))
                .unwrap_or_else(|| panic!("no hover marker painted:\n{:?}", grid(&buf, area)));
            (row, py, ph)
        };

        // Six plot rows, two per segment: hovering the bottom series
        // marks the bottom band, the top series the top band — not the
        // one spot the braille/line math used to pick for every series.
        let (bottom_row, py, ph) = marker_row((0, 0));
        assert!(
            bottom_row >= py + ph - 3,
            "bottom segment marker at {bottom_row}, plot rows {py}..{}",
            py + ph
        );
        let (top_row, py, ph) = marker_row((2, 0));
        assert!(
            top_row <= py + 1,
            "top segment marker at {top_row}, plot rows {py}..{}",
            py + ph
        );
    }

    #[test]
    fn multi_series_bar_legend_labels_every_series() {
        let area = Rect::new(0, 0, 30, 8);
        let mut buf = Buffer::empty(area);
        let mut chart = bar_chart_of(
            ChartKind::Bar,
            vec![
                series_of("OK", vec![1.0], Color::rgb(255, 0, 0)),
                series_of("Slow", vec![1.0], Color::rgb(0, 255, 0)),
                series_of("Fail", vec![1.0], Color::rgb(0, 0, 255)),
            ],
            Some((0.0, 3.0)),
        );
        chart.show_legend = true;
        let layout = draw_chart(&mut buf, area, &chart, &Theme::default(), None, None);
        let lb = layout.legend_bounds.expect("bar charts get a legend row");
        let row: String = (lb.x.round() as u16..(lb.x + lb.width).round() as u16)
            .map(|x| cell_char(&buf, x, lb.y.round() as u16))
            .collect();
        assert!(
            row.contains("OK") && row.contains("Slow") && row.contains("Fail"),
            "every bar series should be named in the legend: {row:?}"
        );
    }

    #[test]
    fn zero_size_is_no_op() {
        let buf_area = Rect::new(0, 0, 10, 1);
        let mut buf = Buffer::empty(buf_area);
        let area = Rect::new(0, 0, 0, 0);
        let chart = spark(vec![1.0, 2.0]);
        let _layout = draw_chart(&mut buf, area, &chart, &Theme::default(), None, None);
        assert_eq!(cell_char(&buf, 0, 0), ' ');
    }

    // ── Grid paints under data, not over it (#648) ──────────────────────

    /// A tall bar and an empty (zero-height) bar share a y-tick row: the
    /// tall bar's cell must stay `█` (data wins) while the empty bar's
    /// cell on the same row must still show the grid dash — proving the
    /// grid is painted *underneath* the data rather than deleted
    /// outright (deleting the grid would also make this pass if we only
    /// asserted the bar cell).
    #[test]
    fn bar_chart_grid_paints_under_bars_not_over_them() {
        let area = Rect::new(0, 0, 12, 9);
        let mut buf = Buffer::empty(area);
        let mut chart = bar_chart_of(
            ChartKind::Bar,
            vec![series_of("B", vec![3.0, 0.0], Color::rgb(1, 2, 3))],
            Some((0.0, 4.0)),
        );
        chart.show_grid = true;
        chart.y_ticks = Some(4);
        let layout = draw_chart(&mut buf, area, &chart, &Theme::default(), None, None);

        let pa = &layout.plot_area;
        let px = pa.x.round() as u16;
        let py = pa.y.round() as u16;
        let ph = pa.height.round() as u16;
        let pw = pa.width.round() as u16;
        let slot_w = (pw / 2).max(1);

        let bar_col = px; // first data point: value 3.0, a tall bar
        let empty_col = px + slot_w; // second data point: value 0.0, no bar

        let tick_row_in_bar = layout
            .y_tick_positions
            .iter()
            .map(|&(sy, _)| sy.round() as u16)
            .filter(|&row| row > py && row < py + ph)
            .find(|&row| cell_char(&buf, bar_col, row) == '█')
            .unwrap_or_else(|| {
                panic!(
                    "no tick row landed inside the bar for this fixture:\n{:?}",
                    grid(&buf, area)
                )
            });

        assert_eq!(cell_char(&buf, bar_col, tick_row_in_bar), '█');
        assert_eq!(
            cell_char(&buf, empty_col, tick_row_in_bar),
            '┄',
            "grid should still show through cells the data does not occupy:\n{:?}",
            grid(&buf, area)
        );
    }

    /// A line chart's braille line crosses several y-tick rows; the
    /// braille glyph at the crossing must survive, not be replaced by
    /// the grid dash (#648).
    #[test]
    fn line_chart_grid_paints_under_braille_not_over_it() {
        let area = Rect::new(0, 0, 20, 10);
        let mut buf = Buffer::empty(area);
        let chart = Chart {
            id: WidgetId::new("c"),
            kind: ChartKind::Line,
            series: vec![Series {
                label: "A".into(),
                data: vec![0.0, 2.5, 5.0, 7.5, 10.0, 7.5, 5.0, 2.5, 0.0],
                color: None,
                fill: false,
            }],
            x_label: None,
            y_label: None,
            y_range: Some((0.0, 10.0)),
            x_range: None,
            show_legend: false,
            y_ticks: Some(5),
            x_ticks: None,
            show_grid: true,
        };
        let layout = draw_chart(&mut buf, area, &chart, &Theme::default(), None, None);

        let pa = &layout.plot_area;
        let px = pa.x.round() as u16;
        let py = pa.y.round() as u16;
        let ph = pa.height.round() as u16;
        let pw = pa.width.round() as u16;

        // A tick row strictly inside the plot (excluding the axis border
        // rows) where the line's braille dots cross should still show a
        // braille glyph somewhere along the row.
        let tick_row = layout
            .y_tick_positions
            .iter()
            .map(|&(sy, _)| sy.round() as u16)
            .find(|&row| row > py && row < py + ph.saturating_sub(1))
            .expect("fixture should have an interior tick row");

        let braille_survives = (px + 1..px + pw).any(|col| {
            let ch = cell_char(&buf, col, tick_row);
            ('\u{2800}'..='\u{28ff}').contains(&ch)
        });
        assert!(
            braille_survives,
            "expected a braille dot to survive on tick row {tick_row}:\n{:?}",
            grid(&buf, area)
        );
    }
}
