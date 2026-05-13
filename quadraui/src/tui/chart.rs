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
        ChartKind::Bar => paint_bar(buf, &layout, chart, theme),
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

    if let Some(s) = chart.series.first() {
        if s.data.is_empty() {
            return;
        }

        let (y_min, y_max) = chart.effective_y_range();
        let range = y_max - y_min;
        let n = s.data.len();
        let bar_w = ((pw as usize) / n.max(1)).max(1);
        let fg = ratatui_color(series_color(chart, 0));
        let plot_h = ph.saturating_sub(1) as usize;

        for (i, &val) in s.data.iter().enumerate() {
            let norm = if range > 0.0 {
                ((val - y_min) / range).clamp(0.0, 1.0)
            } else {
                0.5
            };
            let fill_rows = (norm * plot_h as f64).round() as usize;
            let bx = px + (i * bar_w) as u16;

            for r in 0..fill_rows {
                let by = py + ph - 2 - r as u16;
                if by >= py {
                    for c in 0..bar_w.min((pw as usize).saturating_sub(i * bar_w)) {
                        set_cell(buf, bx + c as u16, by, '█', fg, bg);
                    }
                }
            }
        }

        // Bottom axis.
        for col in px..px + pw {
            set_cell(buf, col, py + ph - 1, '─', dim, bg);
        }
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

fn paint_axis_labels(buf: &mut Buffer, layout: &ChartLayout, chart: &Chart, theme: &Theme) {
    let bg = ratatui_color(theme.background);
    let fg = ratatui_color(theme.foreground);
    let dim = ratatui_color(theme.muted_fg);
    let pa = &layout.plot_area;
    let px = pa.x.round() as u16;
    let pw = pa.width.round() as u16;

    for &(sy, val) in &layout.y_tick_positions {
        let row = sy.round() as u16;
        let label = crate::primitives::chart::format_tick_value(val);
        let gutter_end = px.saturating_sub(1);
        let label_start = gutter_end.saturating_sub(label.len() as u16);
        for (i, ch) in label.chars().enumerate() {
            let col = label_start + i as u16;
            if col < gutter_end {
                set_cell(buf, col, row, ch, dim, bg);
            }
        }
        if chart.show_grid && row > pa.y.round() as u16 && row < (pa.y + pa.height).round() as u16 {
            for col in (px + 1)..(px + pw) {
                set_cell(buf, col, row, '┄', dim, bg);
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
        ChartKind::Line | ChartKind::Bar => {
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

    #[test]
    fn sparkline_paint_and_click_round_trip() {
        let area = Rect::new(0, 0, 5, 1);
        let mut buf = Buffer::empty(area);
        let chart = spark(vec![0.0, 0.25, 0.5, 0.75, 1.0]);
        let layout = draw_chart(&mut buf, area, &chart, &Theme::default(), None, None);

        assert_eq!(cell_char(&buf, 0, 0), '▁');
        assert_eq!(cell_char(&buf, 4, 0), '█');

        assert_eq!(
            layout.hit_test(2.5, 0.5),
            ChartHit::Body(WidgetId::new("c"))
        );
        assert_eq!(layout.hit_test(10.0, 0.5), ChartHit::Empty);
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

    #[test]
    fn bar_chart_paint_and_click_round_trip() {
        let area = Rect::new(0, 0, 10, 5);
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

        assert_eq!(
            layout.hit_test(5.0, 2.0),
            ChartHit::Body(WidgetId::new("c"))
        );
    }

    #[test]
    fn legend_paint_and_click_round_trip() {
        let area = Rect::new(0, 0, 30, 10);
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
    fn zero_size_is_no_op() {
        let buf_area = Rect::new(0, 0, 10, 1);
        let mut buf = Buffer::empty(buf_area);
        let area = Rect::new(0, 0, 0, 0);
        let chart = spark(vec![1.0, 2.0]);
        let _layout = draw_chart(&mut buf, area, &chart, &Theme::default(), None, None);
        assert_eq!(cell_char(&buf, 0, 0), ' ');
    }
}
