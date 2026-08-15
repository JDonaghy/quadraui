//! `Chart` primitive: sparkline, line, and bar chart visualisations.
//!
//! Three chart kinds serve different data visualisation needs:
//!
//! - [`ChartKind::Sparkline`] — single-row inline chart for embedding
//!   in status bars or table cells. No axes, no labels.
//! - [`ChartKind::Line`] — multi-series line/area chart with optional
//!   axis labels and legend. Set [`Series::fill`] for area charts.
//! - [`ChartKind::Bar`] — vertical bar chart with category labels.
//!   Multiple series **stack** within each x-position.
//! - [`ChartKind::BarGrouped`] — the same data drawn **side by side**
//!   within each x-position instead of stacked.
//!
//! Each [`Series`] carries a `Vec<f64>` of y-values evenly spaced along
//! the x-axis. The y-range auto-derives from data when
//! [`Chart::y_range`] is `None` — for stacked bars that ceiling is the
//! largest *column total*, so a stack never clips (#584).
//!
//! Every backend paints bars from the shared geometry helpers
//! [`Chart::bar_column_spans`] and [`Chart::column_totals`], so stacked
//! and grouped layouts stay identical across TUI, GTK and macOS.

use crate::event::Rect;
use crate::types::{Color, WidgetId};
use serde::{Deserialize, Serialize};

/// Declarative description of a chart widget.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Chart {
    pub id: WidgetId,
    pub kind: ChartKind,
    pub series: Vec<Series>,
    #[serde(default)]
    pub x_label: Option<String>,
    #[serde(default)]
    pub y_label: Option<String>,
    /// Explicit y-axis range. `None` = auto-derived from data min/max.
    #[serde(default)]
    pub y_range: Option<(f64, f64)>,
    /// Explicit x-axis range. `None` = `0..series.data.len()`.
    #[serde(default)]
    pub x_range: Option<(f64, f64)>,
    #[serde(default)]
    pub show_legend: bool,
    /// Number of y-axis tick marks. `None` = auto (5).
    #[serde(default)]
    pub y_ticks: Option<usize>,
    /// Number of x-axis tick marks. `None` = auto.
    #[serde(default)]
    pub x_ticks: Option<usize>,
    /// Show horizontal grid lines at y-tick positions.
    #[serde(default)]
    pub show_grid: bool,
}

/// Chart visualisation kind.
///
/// `#[non_exhaustive]`: per PRIMITIVE_RULES rule 8, a downstream `match`
/// on this enum needs a wildcard arm so later kinds (#584 added
/// [`ChartKind::BarGrouped`]) stay additive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[non_exhaustive]
pub enum ChartKind {
    /// Single-row inline chart (no axes, no labels).
    Sparkline,
    /// Multi-series line chart with axes. Per-series `fill` enables area fill.
    #[default]
    Line,
    /// Vertical bar chart with category labels.
    ///
    /// Multiple series **stack**: each x-position paints one coloured
    /// segment per series, bottom-up in `series` order, and the bar's
    /// total height is the column sum. A single-series chart is exactly
    /// a plain bar chart — stacking is a no-op there.
    ///
    /// Segment heights are proportional to the values only when the
    /// y-axis floor is `0.0`. With an auto-derived range the floor is
    /// `min(data)`, so the *bottom* segment absorbs that offset (the
    /// same baseline behaviour a single-series bar chart has always
    /// had). Set `y_range: Some((0.0, …))` for exact proportions.
    Bar,
    /// Vertical bar chart whose series are drawn **side by side**
    /// within each x-position, for comparing magnitudes rather than
    /// composition. Use [`ChartKind::Bar`] to compare totals instead.
    ///
    /// Each slot is split into `series.len()` sub-bars. When a slot is
    /// too narrow to give every series at least one device unit, the
    /// trailing series are clipped — widen the chart or drop a series.
    BarGrouped,
}

impl ChartKind {
    /// True for every bar-family kind ([`ChartKind::Bar`],
    /// [`ChartKind::BarGrouped`]).
    pub fn is_bar(self) -> bool {
        matches!(self, ChartKind::Bar | ChartKind::BarGrouped)
    }

    /// True when bar segments accumulate per x-position rather than
    /// sitting side by side.
    pub fn is_stacked_bar(self) -> bool {
        matches!(self, ChartKind::Bar)
    }
}

/// One data series in a chart.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Series {
    pub label: String,
    /// Y-values, evenly spaced along the x-axis.
    pub data: Vec<f64>,
    /// Override colour. `None` = backend picks from a default palette.
    #[serde(default)]
    pub color: Option<Color>,
    /// Fill the area under the line (Line kind only). Ignored for
    /// Sparkline and Bar.
    #[serde(default)]
    pub fill: bool,
}

/// Events a `Chart` emits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChartEvent {
    /// User clicked the chart body.
    Clicked { id: WidgetId },
    /// User clicked a specific data point.
    DataPointClicked {
        id: WidgetId,
        series_idx: usize,
        data_idx: usize,
    },
    /// User clicked a legend entry.
    LegendClicked { id: WidgetId, series_idx: usize },
}

// ── Layout API ──────────────────────────────────────────────────────────────

/// Backend-supplied measurements for chart layout.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChartMeasure {
    pub width: f32,
    pub height: f32,
    /// Approximate monospace character width (for axis label sizing).
    pub char_width: f32,
    /// Line height (for axis label rows).
    pub line_height: f32,
}

/// Classification of a hit-test result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChartHit {
    /// Click landed on a specific data point.
    DataPoint(WidgetId, usize, usize),
    /// Click landed on the plot area (no specific point nearby).
    Body(WidgetId),
    /// Click landed on a legend entry (series index).
    Legend(WidgetId, usize),
    /// Click landed outside the chart.
    Empty,
}

/// Fully-resolved chart layout.
#[derive(Debug, Clone, PartialEq)]
pub struct ChartLayout {
    pub bounds: Rect,
    /// The data-plotting region (inside axes/labels).
    pub plot_area: Rect,
    pub legend_bounds: Option<Rect>,
    pub hit_regions: Vec<(Rect, ChartHit)>,
    /// Screen positions of data points: (series_idx, data_idx, x, y).
    /// Apps use these to anchor tooltips and resolve nearest-point from
    /// MouseMoved events.
    pub data_point_positions: Vec<(usize, usize, f32, f32)>,
    /// Y-axis tick positions: (screen_y, data_value).
    pub y_tick_positions: Vec<(f32, f64)>,
    /// X-axis tick positions: (screen_x, data_value).
    pub x_tick_positions: Vec<(f32, f64)>,
}

impl ChartLayout {
    pub fn hit_test(&self, x: f32, y: f32) -> ChartHit {
        for (rect, hit) in &self.hit_regions {
            if x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height {
                return hit.clone();
            }
        }
        ChartHit::Empty
    }

    /// Find the nearest data point to (x, y) within `snap_distance`.
    /// Returns `(series_idx, data_idx)`.
    pub fn nearest_point(&self, x: f32, y: f32, snap_distance: f32) -> Option<(usize, usize)> {
        let mut best: Option<(usize, usize, f32)> = None;
        let snap_sq = snap_distance * snap_distance;
        for &(si, di, px, py) in &self.data_point_positions {
            let dx = x - px;
            let dy = y - py;
            let dist_sq = dx * dx + dy * dy;
            if dist_sq <= snap_sq && (best.is_none() || dist_sq < best.unwrap().2) {
                best = Some((si, di, dist_sq));
            }
        }
        best.map(|(si, di, _)| (si, di))
    }

    /// Convert a screen x-coordinate to a data-space x index (fractional).
    pub fn screen_to_data_x(&self, screen_x: f32, data_len: usize) -> f64 {
        if data_len <= 1 || self.plot_area.width <= 0.0 {
            return 0.0;
        }
        let frac = ((screen_x - self.plot_area.x) / self.plot_area.width).clamp(0.0, 1.0);
        frac as f64 * (data_len - 1) as f64
    }

    /// Convert a data-space x index to a screen x-coordinate.
    pub fn data_to_screen_x(&self, data_x: f64, data_len: usize) -> f32 {
        if data_len <= 1 {
            return self.plot_area.x;
        }
        let frac = (data_x / (data_len - 1) as f64).clamp(0.0, 1.0) as f32;
        self.plot_area.x + frac * self.plot_area.width
    }
}

impl Chart {
    /// Resolve the effective y-range from explicit range or data min/max.
    ///
    /// For a stacked bar chart ([`ChartKind::Bar`]) the ceiling is the
    /// largest **column total**, not the largest single value, so a
    /// stack can never clip. Column totals equal the data itself for a
    /// single-series chart, so single-series output is unaffected.
    pub fn effective_y_range(&self) -> (f64, f64) {
        if let Some(range) = self.y_range {
            return range;
        }
        let mut min = f64::INFINITY;
        let mut max = f64::NEG_INFINITY;
        for s in &self.series {
            for &v in &s.data {
                if v < min {
                    min = v;
                }
                if v > max {
                    max = v;
                }
            }
        }
        if self.kind.is_stacked_bar() {
            // Stacks are painted to the column total; widen the range so
            // the tallest one still fits (negative values likewise pull
            // the floor down rather than clipping below the axis).
            for total in self.column_totals() {
                if total < min {
                    min = total;
                }
                if total > max {
                    max = total;
                }
            }
        }
        if min > max {
            (0.0, 1.0)
        } else if (max - min).abs() < f64::EPSILON {
            (min - 1.0, max + 1.0)
        } else {
            (min, max)
        }
    }

    /// Maximum data length across all series.
    pub fn max_data_len(&self) -> usize {
        self.series.iter().map(|s| s.data.len()).max().unwrap_or(0)
    }

    /// Sum of every series' value at each x-position, i.e. the height a
    /// stacked bar reaches. Series shorter than [`Chart::max_data_len`]
    /// contribute `0.0` for the missing positions.
    pub fn column_totals(&self) -> Vec<f64> {
        (0..self.max_data_len())
            .map(|i| {
                self.series
                    .iter()
                    .map(|s| s.data.get(i).copied().unwrap_or(0.0))
                    .sum()
            })
            .collect()
    }

    /// Vertical extent of every bar segment in the `data_idx` column,
    /// as `(series_idx, bottom, top)` fractions of the plot height
    /// measured **up from the plot floor**, in bottom-to-top paint
    /// order. This is the one source of truth every backend's bar
    /// painter and [`Chart::layout`] share (#584).
    ///
    /// - [`ChartKind::Bar`] — spans accumulate: each segment starts
    ///   where the previous one ended, so an all-zero series occupies
    ///   an empty span and does **not** shift the series above it.
    /// - [`ChartKind::BarGrouped`] — every span starts at the floor;
    ///   the caller lays the series out side by side horizontally.
    ///
    /// Returns one entry per series (zero-height for missing data), so
    /// callers can index by series without re-checking lengths. Empty
    /// for non-bar kinds.
    pub fn bar_column_spans(&self, data_idx: usize) -> Vec<(usize, f64, f64)> {
        if !self.kind.is_bar() {
            return Vec::new();
        }
        let (y_min, y_max) = self.effective_y_range();
        let range = y_max - y_min;
        let norm = |v: f64| {
            if range > 0.0 {
                ((v - y_min) / range).clamp(0.0, 1.0)
            } else {
                0.5
            }
        };

        let mut spans = Vec::with_capacity(self.series.len());
        if self.kind.is_stacked_bar() {
            let mut cum = 0.0;
            let mut bottom = 0.0;
            for (si, s) in self.series.iter().enumerate() {
                cum += s.data.get(data_idx).copied().unwrap_or(0.0);
                // `max(bottom)` keeps spans monotonic if a negative
                // value would otherwise invert the segment.
                let top = norm(cum).max(bottom);
                spans.push((si, bottom, top));
                bottom = top;
            }
        } else {
            for (si, s) in self.series.iter().enumerate() {
                let top = s.data.get(data_idx).map(|&v| norm(v)).unwrap_or(0.0);
                spans.push((si, 0.0, top));
            }
        }
        spans
    }

    /// Compute layout and hit regions.
    ///
    /// Backends call this with their native measurements. The returned
    /// [`ChartLayout`] is consumed by both paint and hit_test — one
    /// source of truth.
    pub fn layout(&self, origin_x: f32, origin_y: f32, measure: ChartMeasure) -> ChartLayout {
        let bounds = Rect::new(origin_x, origin_y, measure.width, measure.height);

        match self.kind {
            ChartKind::Sparkline => {
                let hit_regions = vec![(bounds, ChartHit::Body(self.id.clone()))];
                let mut data_point_positions = Vec::new();
                if let Some(s) = self.series.first() {
                    let (y_min, y_max) = self.effective_y_range();
                    let range = y_max - y_min;
                    let pw = measure.width;
                    let n = s.data.len();
                    let start = n.saturating_sub(pw as usize);
                    for (i, &val) in s.data[start..].iter().enumerate() {
                        let norm = if range > 0.0 {
                            ((val - y_min) / range).clamp(0.0, 1.0)
                        } else {
                            0.5
                        };
                        let sx = origin_x + i as f32;
                        let sy = origin_y + (1.0 - norm as f32) * measure.height;
                        data_point_positions.push((0, start + i, sx, sy));
                    }
                }
                ChartLayout {
                    bounds,
                    plot_area: bounds,
                    legend_bounds: None,
                    hit_regions,
                    data_point_positions,
                    y_tick_positions: Vec::new(),
                    x_tick_positions: Vec::new(),
                }
            }
            ChartKind::Line | ChartKind::Bar | ChartKind::BarGrouped => {
                let (y_min, y_max) = self.effective_y_range();
                let y_tick_count = self.y_ticks.unwrap_or(5);
                let y_label_width = if y_tick_count > 0 || self.y_label.is_some() {
                    let max_label_len = format_tick_value(y_max)
                        .len()
                        .max(format_tick_value(y_min).len());
                    measure.char_width * (max_label_len as f32 + 1.0)
                } else {
                    0.0
                };
                let x_label_height = if self.x_label.is_some() {
                    measure.line_height
                } else {
                    0.0
                };
                let legend_height = if self.show_legend && !self.series.is_empty() {
                    measure.line_height
                } else {
                    0.0
                };

                let plot_x = origin_x + y_label_width;
                let plot_y = origin_y + legend_height;
                let plot_w = (measure.width - y_label_width).max(0.0);
                let plot_h = (measure.height - x_label_height - legend_height).max(0.0);
                let plot_area = Rect::new(plot_x, plot_y, plot_w, plot_h);

                let legend_bounds = if legend_height > 0.0 {
                    Some(Rect::new(plot_x, origin_y, plot_w, legend_height))
                } else {
                    None
                };

                let mut hit_regions = Vec::new();
                if let Some(lb) = legend_bounds {
                    let entry_w = if self.series.is_empty() {
                        0.0
                    } else {
                        (plot_w / self.series.len() as f32).max(1.0)
                    };
                    for (i, _) in self.series.iter().enumerate() {
                        let ex = lb.x + entry_w * i as f32;
                        let ew = if i + 1 == self.series.len() {
                            lb.x + lb.width - ex
                        } else {
                            entry_w
                        };
                        hit_regions.push((
                            Rect::new(ex, lb.y, ew, lb.height),
                            ChartHit::Legend(self.id.clone(), i),
                        ));
                    }
                }
                hit_regions.push((plot_area, ChartHit::Body(self.id.clone())));

                let range = y_max - y_min;
                let mut data_point_positions = Vec::new();
                if self.kind.is_bar() {
                    // Bars own a slot, not a point: anchor each segment
                    // at its own rectangle so `nearest_point` resolves
                    // to the (series, index) under the cursor — which is
                    // what a stacked-segment tooltip needs (#584).
                    let n = self.max_data_len();
                    let slot_w = if n == 0 { 0.0 } else { plot_w / n as f32 };
                    let series_count = self.series.len().max(1) as f32;
                    let stacked = self.kind.is_stacked_bar();
                    for di in 0..n {
                        for (si, bottom, top) in self.bar_column_spans(di) {
                            let (sx, sy) = if stacked {
                                let mid = (bottom + top) / 2.0;
                                (
                                    plot_x + slot_w * (di as f32 + 0.5),
                                    plot_y + plot_h - mid as f32 * plot_h,
                                )
                            } else {
                                let sub_w = slot_w / series_count;
                                (
                                    plot_x + slot_w * di as f32 + sub_w * (si as f32 + 0.5),
                                    plot_y + plot_h - top as f32 * plot_h,
                                )
                            };
                            data_point_positions.push((si, di, sx, sy));
                        }
                    }
                } else {
                    for (si, s) in self.series.iter().enumerate() {
                        let n = s.data.len();
                        for (di, &val) in s.data.iter().enumerate() {
                            let norm = if range > 0.0 {
                                ((val - y_min) / range).clamp(0.0, 1.0)
                            } else {
                                0.5
                            };
                            let sx = if n <= 1 {
                                plot_x
                            } else {
                                plot_x + (di as f32 / (n - 1) as f32) * plot_w
                            };
                            let sy = plot_y + plot_h - norm as f32 * plot_h;
                            data_point_positions.push((si, di, sx, sy));
                        }
                    }
                }

                let mut y_tick_positions = Vec::new();
                if y_tick_count > 0 && plot_h > 0.0 && range > 0.0 {
                    for i in 0..=y_tick_count {
                        let frac = i as f64 / y_tick_count as f64;
                        let val = y_min + frac * range;
                        let sy = plot_y + plot_h - frac as f32 * plot_h;
                        y_tick_positions.push((sy, val));
                    }
                }

                let x_tick_count = self.x_ticks.unwrap_or(0);
                let data_len = self.max_data_len();
                let mut x_tick_positions = Vec::new();
                if x_tick_count > 0 && plot_w > 0.0 && data_len > 1 {
                    for i in 0..=x_tick_count {
                        let frac = i as f64 / x_tick_count as f64;
                        let val = frac * (data_len - 1) as f64;
                        let sx = plot_x + frac as f32 * plot_w;
                        x_tick_positions.push((sx, val));
                    }
                }

                ChartLayout {
                    bounds,
                    plot_area,
                    legend_bounds,
                    hit_regions,
                    data_point_positions,
                    y_tick_positions,
                    x_tick_positions,
                }
            }
        }
    }
}

/// Format a tick value for axis labels. Uses integer format when the
/// value has no fractional part, otherwise one decimal place.
pub fn format_tick_value(v: f64) -> String {
    if (v - v.round()).abs() < 0.01 {
        format!("{}", v as i64)
    } else {
        format!("{:.1}", v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::WidgetId;

    fn sparkline_chart(data: Vec<f64>) -> Chart {
        Chart {
            id: WidgetId::new("chart"),
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

    fn line_chart(data: Vec<f64>) -> Chart {
        Chart {
            id: WidgetId::new("chart"),
            kind: ChartKind::Line,
            series: vec![Series {
                label: "Series A".into(),
                data,
                color: None,
                fill: false,
            }],
            x_label: Some("Time".into()),
            y_label: Some("Value".into()),
            y_range: None,
            x_range: None,
            show_legend: true,
            y_ticks: None,
            x_ticks: None,
            show_grid: false,
        }
    }

    #[test]
    fn sparkline_layout_fills_bounds() {
        let chart = sparkline_chart(vec![1.0, 2.0, 3.0]);
        let m = ChartMeasure {
            width: 20.0,
            height: 1.0,
            char_width: 1.0,
            line_height: 1.0,
        };
        let layout = chart.layout(0.0, 0.0, m);
        assert_eq!(layout.plot_area, layout.bounds);
        assert!(layout.legend_bounds.is_none());
    }

    #[test]
    fn sparkline_hit_test_body() {
        let chart = sparkline_chart(vec![1.0, 2.0]);
        let m = ChartMeasure {
            width: 10.0,
            height: 1.0,
            char_width: 1.0,
            line_height: 1.0,
        };
        let layout = chart.layout(0.0, 0.0, m);
        assert_eq!(
            layout.hit_test(5.0, 0.5),
            ChartHit::Body(WidgetId::new("chart"))
        );
        assert_eq!(layout.hit_test(15.0, 0.5), ChartHit::Empty);
    }

    #[test]
    fn line_layout_subtracts_axes_and_legend() {
        let chart = line_chart(vec![1.0, 2.0, 3.0]);
        let m = ChartMeasure {
            width: 40.0,
            height: 20.0,
            char_width: 1.0,
            line_height: 1.0,
        };
        let layout = chart.layout(0.0, 0.0, m);
        assert!(layout.plot_area.x > 0.0, "y-label shifts plot right");
        assert!(
            layout.plot_area.height < 20.0,
            "x-label + legend reduce height"
        );
        assert!(layout.legend_bounds.is_some());
    }

    #[test]
    fn line_legend_hit_test() {
        let mut chart = line_chart(vec![1.0, 2.0]);
        chart.series.push(Series {
            label: "Series B".into(),
            data: vec![3.0, 4.0],
            color: None,
            fill: false,
        });
        let m = ChartMeasure {
            width: 40.0,
            height: 20.0,
            char_width: 1.0,
            line_height: 1.0,
        };
        let layout = chart.layout(0.0, 0.0, m);
        let lb = layout.legend_bounds.unwrap();
        let mid_x = lb.x + lb.width / 4.0;
        assert_eq!(
            layout.hit_test(mid_x, lb.y + 0.5),
            ChartHit::Legend(WidgetId::new("chart"), 0)
        );
        let mid_x2 = lb.x + lb.width * 3.0 / 4.0;
        assert_eq!(
            layout.hit_test(mid_x2, lb.y + 0.5),
            ChartHit::Legend(WidgetId::new("chart"), 1)
        );
    }

    #[test]
    fn effective_y_range_auto() {
        let chart = sparkline_chart(vec![2.0, 5.0, 3.0]);
        assert_eq!(chart.effective_y_range(), (2.0, 5.0));
    }

    #[test]
    fn effective_y_range_explicit() {
        let mut chart = sparkline_chart(vec![2.0, 5.0]);
        chart.y_range = Some((0.0, 10.0));
        assert_eq!(chart.effective_y_range(), (0.0, 10.0));
    }

    #[test]
    fn effective_y_range_empty() {
        let chart = sparkline_chart(vec![]);
        assert_eq!(chart.effective_y_range(), (0.0, 1.0));
    }

    #[test]
    fn effective_y_range_flat() {
        let chart = sparkline_chart(vec![5.0, 5.0, 5.0]);
        let (lo, hi) = chart.effective_y_range();
        assert!(lo < 5.0 && hi > 5.0);
    }

    // ── Multi-series bars (#584) ────────────────────────────────────────

    fn bar_chart(kind: ChartKind, data: Vec<Vec<f64>>) -> Chart {
        Chart {
            id: WidgetId::new("chart"),
            kind,
            series: data
                .into_iter()
                .enumerate()
                .map(|(i, d)| Series {
                    label: format!("S{i}"),
                    data: d,
                    color: None,
                    fill: false,
                })
                .collect(),
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

    #[test]
    fn stacked_bar_y_ceiling_is_max_column_total() {
        let chart = bar_chart(
            ChartKind::Bar,
            vec![vec![1.0, 5.0], vec![2.0, 1.0], vec![3.0, 0.0]],
        );
        // Column totals are 6 and 6; the max single value is 5.
        assert_eq!(chart.column_totals(), vec![6.0, 6.0]);
        assert_eq!(chart.effective_y_range(), (0.0, 6.0));
    }

    #[test]
    fn grouped_bar_y_ceiling_is_max_single_value() {
        let chart = bar_chart(
            ChartKind::BarGrouped,
            vec![vec![1.0, 5.0], vec![2.0, 1.0], vec![3.0, 0.0]],
        );
        assert_eq!(chart.effective_y_range(), (0.0, 5.0));
    }

    #[test]
    fn single_series_bar_y_range_is_unchanged_by_stacking() {
        // Pin the pre-#584 behaviour: one series' column totals are the
        // data itself, so nothing about the auto-range moves.
        let chart = bar_chart(ChartKind::Bar, vec![vec![2.0, 5.0, 3.0]]);
        assert_eq!(chart.effective_y_range(), (2.0, 5.0));
    }

    #[test]
    fn stacked_spans_accumulate_bottom_up() {
        let mut chart = bar_chart(ChartKind::Bar, vec![vec![1.0], vec![1.0], vec![1.0]]);
        chart.y_range = Some((0.0, 3.0));
        let spans = chart.bar_column_spans(0);
        assert_eq!(spans.len(), 3);
        assert!((spans[0].1 - 0.0).abs() < 1e-9 && (spans[0].2 - 1.0 / 3.0).abs() < 1e-9);
        assert!((spans[1].1 - 1.0 / 3.0).abs() < 1e-9 && (spans[1].2 - 2.0 / 3.0).abs() < 1e-9);
        assert!((spans[2].1 - 2.0 / 3.0).abs() < 1e-9 && (spans[2].2 - 1.0).abs() < 1e-9);
    }

    #[test]
    fn stacked_all_zero_series_does_not_shift_the_others() {
        let mut with_zeros = bar_chart(
            ChartKind::Bar,
            vec![vec![1.0], vec![0.0], vec![1.0], vec![0.0]],
        );
        with_zeros.y_range = Some((0.0, 2.0));
        let spans = with_zeros.bar_column_spans(0);
        // The zero series occupies an empty span at the boundary…
        assert_eq!((spans[1].1, spans[1].2), (0.5, 0.5));
        assert_eq!((spans[3].1, spans[3].2), (1.0, 1.0));
        // …and the series above it keeps the span it would have had.
        assert_eq!((spans[0].1, spans[0].2), (0.0, 0.5));
        assert_eq!((spans[2].1, spans[2].2), (0.5, 1.0));
    }

    #[test]
    fn grouped_spans_all_start_at_the_floor() {
        let mut chart = bar_chart(ChartKind::BarGrouped, vec![vec![1.0], vec![2.0], vec![4.0]]);
        chart.y_range = Some((0.0, 4.0));
        let spans = chart.bar_column_spans(0);
        assert_eq!(spans, vec![(0, 0.0, 0.25), (1, 0.0, 0.5), (2, 0.0, 1.0)]);
    }

    #[test]
    fn bar_column_spans_empty_for_non_bar_kinds() {
        let chart = sparkline_chart(vec![1.0, 2.0]);
        assert!(chart.bar_column_spans(0).is_empty());
    }

    #[test]
    fn stacked_bar_nearest_point_resolves_series_and_index() {
        let mut chart = bar_chart(
            ChartKind::Bar,
            vec![vec![1.0, 1.0], vec![1.0, 1.0], vec![1.0, 1.0]],
        );
        chart.y_range = Some((0.0, 3.0));
        let m = ChartMeasure {
            width: 30.0,
            height: 30.0,
            char_width: 1.0,
            line_height: 1.0,
        };
        let layout = chart.layout(0.0, 0.0, m);
        // One anchor per (series, column), not one per column.
        assert_eq!(layout.data_point_positions.len(), 6);

        let pa = layout.plot_area;
        let slot_w = pa.width / 2.0;
        // Second column, top third of the stack → series 2, index 1.
        let x = pa.x + slot_w * 1.5;
        let y = pa.y + pa.height / 6.0;
        assert_eq!(layout.nearest_point(x, y, slot_w), Some((2, 1)));
        // Bottom third of the first column → series 0, index 0.
        let y_bottom = pa.y + pa.height * 5.0 / 6.0;
        assert_eq!(
            layout.nearest_point(pa.x + slot_w * 0.5, y_bottom, slot_w),
            Some((0, 0))
        );
    }

    #[test]
    fn grouped_bar_anchors_sit_side_by_side() {
        let mut chart = bar_chart(ChartKind::BarGrouped, vec![vec![1.0], vec![2.0], vec![3.0]]);
        chart.y_range = Some((0.0, 3.0));
        let m = ChartMeasure {
            width: 30.0,
            height: 30.0,
            char_width: 1.0,
            line_height: 1.0,
        };
        let layout = chart.layout(0.0, 0.0, m);
        let xs: Vec<f32> = layout.data_point_positions.iter().map(|p| p.2).collect();
        assert_eq!(xs.len(), 3);
        assert!(xs[0] < xs[1] && xs[1] < xs[2], "sub-bars advance: {xs:?}");
        // Taller value → higher anchor (smaller screen y).
        let ys: Vec<f32> = layout.data_point_positions.iter().map(|p| p.3).collect();
        assert!(ys[0] > ys[1] && ys[1] > ys[2], "bar tops rise: {ys:?}");
    }

    #[test]
    fn bar_kind_predicates() {
        assert!(ChartKind::Bar.is_bar() && ChartKind::Bar.is_stacked_bar());
        assert!(ChartKind::BarGrouped.is_bar() && !ChartKind::BarGrouped.is_stacked_bar());
        assert!(!ChartKind::Line.is_bar() && !ChartKind::Sparkline.is_bar());
    }

    #[test]
    fn bar_grouped_round_trips_through_serde() {
        let json = serde_json::to_string(&ChartKind::BarGrouped).unwrap();
        assert_eq!(json, "\"BarGrouped\"");
        let back: ChartKind = serde_json::from_str(&json).unwrap();
        assert_eq!(back, ChartKind::BarGrouped);
        // Pre-#584 payloads still deserialize.
        let old: ChartKind = serde_json::from_str("\"Bar\"").unwrap();
        assert_eq!(old, ChartKind::Bar);
    }
}
