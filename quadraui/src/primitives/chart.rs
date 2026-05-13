//! `Chart` primitive: sparkline, line, and bar chart visualisations.
//!
//! Three chart kinds serve different data visualisation needs:
//!
//! - [`ChartKind::Sparkline`] — single-row inline chart for embedding
//!   in status bars or table cells. No axes, no labels.
//! - [`ChartKind::Line`] — multi-series line/area chart with optional
//!   axis labels and legend. Set [`Series::fill`] for area charts.
//! - [`ChartKind::Bar`] — vertical bar chart with category labels.
//!
//! Each [`Series`] carries a `Vec<f64>` of y-values evenly spaced along
//! the x-axis. The y-range auto-derives from data when
//! [`Chart::y_range`] is `None`.

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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum ChartKind {
    /// Single-row inline chart (no axes, no labels).
    Sparkline,
    /// Multi-series line chart with axes. Per-series `fill` enables area fill.
    #[default]
    Line,
    /// Vertical bar chart.
    Bar,
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
}

impl Chart {
    /// Resolve the effective y-range from explicit range or data min/max.
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
            ChartKind::Line | ChartKind::Bar => {
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
}
