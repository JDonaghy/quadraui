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
//! [`Chart::y_range`] is `None` — for a multi-series stacked bar chart
//! the range is anchored at `0.0` (widened to fit the tallest column
//! total and any negative excursion), so segments stay proportional
//! and a stack never clips (#584).
//!
//! Every backend paints bars from the shared geometry helper
//! [`Chart::bar_column_spans_all`] (or [`Chart::bar_column_spans`] for
//! a single column), which itself derives from
//! [`Chart::effective_y_range`], so stacked and grouped layouts stay
//! identical across TUI, GTK and macOS. [`Chart::column_totals`] is a
//! separate, independently-useful helper (e.g. for labelling a stack's
//! grand total) — it isn't on the geometry path itself.

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
    /// With two or more series and an auto-derived range (no explicit
    /// `y_range`), the floor is anchored at `0.0` specifically so
    /// segment proportions are correct — see [`Chart::effective_y_range`].
    /// An *explicit* `y_range` whose floor isn't `0.0` still skews every
    /// segment's proportion relative to the others, since the geometry
    /// has no way to know which value the caller intends as "empty";
    /// pass `y_range: Some((0.0, …))` if you need one. If the explicit
    /// range excludes `0.0` altogether (e.g. `Some((-10.0, -2.0))`) the
    /// stack's baseline lands on whichever plot edge is nearer zero and
    /// segments are clipped to the range rather than vanishing, but the
    /// proportions on screen are then the caller's to justify.
    Bar,
    /// Vertical bar chart whose series are drawn **side by side**
    /// within each x-position, for comparing magnitudes rather than
    /// composition. Use [`ChartKind::Bar`] to compare totals instead.
    ///
    /// Each slot is split into `series.len()` sub-bars. When a slot is
    /// too narrow to give every series at least one device unit, the
    /// trailing series are clipped — widen the chart or drop a series.
    /// (The TUI backend clips exactly this way; GTK and macOS instead
    /// floor each sub-bar to 1px and let them overlap rather than
    /// dropping data — both are acceptable outcomes for a pathologically
    /// narrow chart, but they render differently from each other.)
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
    /// For a stacked bar chart ([`ChartKind::Bar`]) with more than one
    /// series, the range is anchored so that segment proportions are
    /// correct: the floor is `min(0.0, ...)` and the ceiling is
    /// `max(0.0, ...)` over every *partial sum* reached while
    /// accumulating each column (not just each column's final total),
    /// so a stack can never clip and — critically — never has its
    /// segments' relative proportions skewed by a nonzero floor (#584
    /// review). Using raw individual values as the floor here would
    /// under-draw the first series in every column by a constant
    /// offset whenever the smallest single value anywhere is `> 0`.
    ///
    /// A single-series `Bar` chart has nothing to compose against, so
    /// it keeps the plain min/max-of-data behaviour every other chart
    /// kind uses — single-series output is unaffected by stacking.
    pub fn effective_y_range(&self) -> (f64, f64) {
        if let Some(range) = self.y_range {
            return range;
        }
        if self.kind.is_stacked_bar() && self.series.len() > 1 {
            let mut min = 0.0_f64;
            let mut max = 0.0_f64;
            for data_idx in 0..self.max_data_len() {
                let mut cum = 0.0;
                for s in &self.series {
                    cum += s.data.get(data_idx).copied().unwrap_or(0.0);
                    if cum < min {
                        min = cum;
                    }
                    if cum > max {
                        max = cum;
                    }
                }
            }
            return if (max - min).abs() < f64::EPSILON {
                (min - 1.0, max + 1.0)
            } else {
                (min, max)
            };
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
    /// - [`ChartKind::Bar`] with two or more series — spans accumulate:
    ///   each segment covers the gap between the running total before
    ///   and after that series, so an all-zero series occupies an empty
    ///   span and does **not** shift the series above it. A negative
    ///   value walks the stack back *down*, and its segment is returned
    ///   bottom-first (`bottom <= top` always holds).
    /// - [`ChartKind::Bar`] with a single series, and
    ///   [`ChartKind::BarGrouped`] — every span starts at the plot
    ///   floor. Grouped callers lay the series out side by side
    ///   horizontally; a single-series stack has nothing to compose
    ///   against, so it stays a plain bar chart (#584).
    ///
    /// Returns one entry per series (zero-height for missing data), so
    /// callers can index by series without re-checking lengths. Empty
    /// for non-bar kinds.
    ///
    /// This resolves [`Chart::effective_y_range`] on every call, which
    /// for a stacked chart is itself an O(series × columns) scan. Paint
    /// loops that walk every column should call
    /// [`Chart::bar_column_spans_all`] instead, which resolves the range
    /// once for the whole chart.
    pub fn bar_column_spans(&self, data_idx: usize) -> Vec<(usize, f64, f64)> {
        if !self.kind.is_bar() {
            return Vec::new();
        }
        self.bar_column_spans_in(data_idx, self.effective_y_range())
    }

    /// [`Chart::bar_column_spans`] for every column, outer index =
    /// column, resolving the y-range once instead of once per column.
    ///
    /// Backends paint whole charts, so this is the form their paint
    /// loops want: it turns the bar pass from O(columns² × series) into
    /// O(columns × series). Empty for non-bar kinds.
    pub fn bar_column_spans_all(&self) -> Vec<Vec<(usize, f64, f64)>> {
        if !self.kind.is_bar() {
            return Vec::new();
        }
        let y_range = self.effective_y_range();
        (0..self.max_data_len())
            .map(|di| self.bar_column_spans_in(di, y_range))
            .collect()
    }

    /// Shared body of [`Chart::bar_column_spans`] and
    /// [`Chart::bar_column_spans_all`], with the y-range already
    /// resolved by the caller.
    fn bar_column_spans_in(
        &self,
        data_idx: usize,
        (y_min, y_max): (f64, f64),
    ) -> Vec<(usize, f64, f64)> {
        let range = y_max - y_min;
        let norm = |v: f64| {
            if range > 0.0 {
                ((v - y_min) / range).clamp(0.0, 1.0)
            } else {
                0.5
            }
        };

        let mut spans = Vec::with_capacity(self.series.len());
        // Gated on `series.len() > 1` to match [`Chart::effective_y_range`]'s
        // zero-anchoring exactly. A *single*-series `Bar` keeps the plain
        // min/max-of-data range, so `norm(0.0)` there is not the plot floor —
        // for all-negative data (`[-5, -3, -1]` → range `(-5, -1)`) it clamps
        // to `1.0` and every bar would collapse to zero height. Single-series
        // bars take the `else` branch and stay byte-identical to pre-#584.
        if self.kind.is_stacked_bar() && self.series.len() > 1 {
            // Each segment spans the gap between two consecutive cumulative
            // levels, both mapped through `norm`. Deriving `bottom` from the
            // *previous cumulative value* rather than carrying the previous
            // `top` forward means a segment stays visible even when zero
            // falls outside the range entirely (an explicit all-negative
            // `y_range` used to collapse the whole stack to zero height), and
            // `min`/`max` keeps `bottom <= top` when a negative value walks
            // the stack back down.
            let mut cum = 0.0;
            for (si, s) in self.series.iter().enumerate() {
                let next = cum + s.data.get(data_idx).copied().unwrap_or(0.0);
                let (a, b) = (norm(cum), norm(next));
                spans.push((si, a.min(b), a.max(b)));
                cum = next;
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
                let range = y_max - y_min;
                let y_tick_count = self.y_ticks.unwrap_or(5);
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
                // Independent of `y_label_width` (it only subtracts the
                // x-label row and legend row), so it's safe to compute
                // ahead of the gutter to know whether ticks will actually
                // be painted into it.
                let plot_h_avail = (measure.height - x_label_height - legend_height).max(0.0);

                // Size the gutter from the labels that will actually be
                // painted — the interior tick values, not just the
                // endpoints (#647). `format_tick_value` emits a decimal
                // place for any non-integral value, so an interior tick
                // like `14.4` between endpoints `0` and `18` is routinely
                // longer than either end.
                let y_label_width = if y_tick_count > 0 || self.y_label.is_some() {
                    let mut max_label_len = if y_tick_count > 0 && plot_h_avail > 0.0 && range > 0.0
                    {
                        (0..=y_tick_count)
                            .map(|i| {
                                let frac = i as f64 / y_tick_count as f64;
                                format_tick_value(y_min + frac * range).len()
                            })
                            .max()
                            .unwrap_or(0)
                    } else {
                        format_tick_value(y_max)
                            .len()
                            .max(format_tick_value(y_min).len())
                    };
                    if let Some(label) = &self.y_label {
                        max_label_len = max_label_len.max(label.len());
                    }
                    measure.char_width * (max_label_len as f32 + 1.0)
                } else {
                    0.0
                };

                let plot_x = origin_x + y_label_width;
                let plot_y = origin_y + legend_height;
                let plot_w = (measure.width - y_label_width).max(0.0);
                let plot_h = plot_h_avail;
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
                    for (di, column) in self.bar_column_spans_all().into_iter().enumerate() {
                        for (si, bottom, top) in column {
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

// ── NativeSurface paint (#810, Phase 2c of the NativeSurface milestone) ────
//
// Before this, `gtk::chart::draw_chart`, `macos::chart::draw_chart` and
// `win::chart::draw_chart` each independently painted every `ChartKind`
// with their own Cairo / CoreGraphics / Direct2D calls (quadraui#785
// child #810, `docs/SMELL_AUDIT_2026-07.md` §5). `paint` below is the one
// shared implementation, written against
// [`crate::native_surface::NativeSurface`] (#807, Phase 1) instead of any
// one backend's drawing API.
//
// Behavioural divergences found while unifying (not resolved silently,
// per this issue's acceptance bar):
//
//   - **The clip (quadraui#791).** GTK and macOS both bracketed their
//     whole paint in a save/clip/restore to the chart's own rect, so the
//     legend/crosshair/hover-marker overlays (painted outside
//     `plot_area` on purpose) couldn't bleed past the chart's own
//     bounds. `win::chart::draw_chart` had **no clip at all**. `paint`
//     always brackets its body in `surface_push_clip(layout.bounds)` /
//     `surface_pop_clip()`, so Windows gains the clip GTK and macOS
//     already had, for free — see the driver-tier tests in each
//     backend's own `backend.rs` test module.
//   - **Area fill (`Series::fill`) is dropped.** GTK and macOS filled
//     the polygon under a Sparkline/Line series with the pre-#810
//     Cairo/CoreGraphics path APIs; `win::chart` never painted it at
//     all — its own (now-deleted) module doc read: "it needs a filled
//     polygon path, which this backend doesn't build a
//     `ID2D1PathGeometry` for yet; only the line stroke itself paints."
//     `NativeSurface` has no polygon-fill verb (only axis-aligned
//     `surface_fill_rect`), so there is no way to reproduce an
//     arbitrary under-the-line fill through it. `paint` adopts
//     Windows's pre-existing, documented scope limitation instead of
//     inventing a fill_rect-based approximation: `Series::fill` no
//     longer paints an area on any pixel backend. TUI's own
//     `tui::chart` is untouched and keeps its own fill rendering.
//   - **The sparkline's data cadence.** GTK and macOS stretch every
//     data point evenly across the plot's full width — macOS's own
//     pre-unification module doc: "we want the polyline stretched
//     across the full plot width — same approach GTK uses." Windows
//     instead painted only the most recent `plot_area.width` points at
//     one pixel each (the same windowed cadence `Chart::layout` itself
//     uses for `ChartKind::Sparkline`'s `data_point_positions` — a
//     TUI-cell convention bleeding into the pixel backends, per that
//     same macOS comment). `paint` adopts the two-out-of-three
//     full-width stretch. A hovered sparkline point's marker still
//     reads its position from `layout.data_point_positions` (matching
//     what GTK and macOS already did), so a data point outside the
//     windowed cadence can show a hover marker slightly off the
//     re-stretched line — a pre-existing mismatch carried forward
//     rather than fixed here, since it's outside this issue's scope.
//   - **The crosshair.** GTK drew a dashed, 50%-alpha line plus each
//     series' value at the crosshair position. Windows drew the same
//     per-series labels but a solid, fully opaque line. macOS drew only
//     a plain solid line, no labels at all. `NativeSurface` has no
//     dashed-line verb, so `paint` adopts the two-out-of-three shape —
//     solid line, per-series value labels — approximating GTK's
//     50%-alpha tint with `blend` against `theme.background` instead of
//     dropping it outright.
//   - **Axis lines.** GTK and Windows both stroke the plot's left/bottom
//     axis lines for `ChartKind::Line` and a bottom baseline for
//     `ChartKind::Bar`/`BarGrouped`; macOS drew neither. `paint` adopts
//     the two-out-of-three shape and always strokes them.
//   - **The grid line color.** GTK used `theme.muted_fg` at ~20% alpha;
//     Windows approximated the same intent by blending `theme.muted_fg`
//     into `theme.background` at 35%; macOS used `theme.separator` at
//     full opacity. `paint` adopts macOS's shape — `theme.separator` is
//     the theme's own semantic "chrome line" color, so painting it
//     opaque needs no alpha approximation on any backend.
//   - **The hover marker.** GTK and Windows drew a real two-ring circle
//     (`cr.arc`/`fill_circle`); macOS approximated it with a single
//     filled square, since its own private rasteriser never grew a
//     circle helper. `NativeSurface` has no circle verb either (see its
//     module doc's "~15 drawing verbs" — a circle isn't one of them),
//     so `paint` adopts macOS's square approximation, sized to the same
//     footprint GTK/Windows already used (radius 5 / radius 8 rings →
//     10×10 / 16×16 squares).
//   - **The chart's theme, on Windows, is left as-is.**
//     `win::chart::draw_chart` was called with a hardcoded
//     `Theme::default()` rather than `self.current_theme`
//     (`win::status_bar` got this same fix under quadraui#789, but that
//     was its own dedicated issue). `WinBackend::draw_form`'s own #808
//     migration kept `Theme::default()` rather than folding in that
//     fix; `WinBackend::draw_chart` does the same here, for the same
//     reason — a live-theme fix is a separate, one-issue-at-a-time
//     change, not something to bundle incidentally into a paint-code
//     unification.
//
// `SERIES_COLORS`/`series_color` are lifted here verbatim (same six
// literal colors every deleted per-backend copy used) — per this
// issue's acceptance bar, the color-table *abstraction* is a separate,
// sibling issue; this phase only collapses three copies of the same
// literal array into one.
//
// `#[allow(dead_code)]`: see `primitives::form`'s identical note (#808)
// — only *called* once a real pixel backend is compiled in, exercised by
// each backend's own `Backend::draw_chart` call site plus this module's
// own `RecordingSurface` tests on every leg that enables one of the
// three cfg'd features.
#[cfg(any(
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
#[allow(dead_code)]
mod native_surface_paint {
    use super::{Chart, ChartKind, ChartLayout};
    use crate::native_surface::NativeSurface;
    use crate::theme::Theme;
    use crate::types::Color;
    use crate::Rect;

    /// Default series palette — see this module's doc for why this is a
    /// fourth (not shared-with-TUI) copy.
    pub(crate) const SERIES_COLORS: [Color; 6] = [
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

    /// CPU-side alpha pre-mix — see this module's doc for why: two of
    /// the three pixel backends' `NativeSurface::surface_fill_rect` /
    /// `surface_draw_line` implementations don't honour `Color::a`
    /// (mirrors the now-deleted `win::text::blend`, lifted here since
    /// it's needed by every backend now, not just Windows).
    fn blend(base: Color, over: Color, alpha: f32) -> Color {
        let alpha = alpha.clamp(0.0, 1.0);
        let mix =
            |b: u8, o: u8| -> u8 { (b as f32 * (1.0 - alpha) + o as f32 * alpha).round() as u8 };
        Color::rgb(
            mix(base.r, over.r),
            mix(base.g, over.g),
            mix(base.b, over.b),
        )
    }

    /// Stroke a polyline through `points` as `points.len() - 1` separate
    /// segments — `NativeSurface::surface_draw_line` only draws one
    /// segment at a time (mirrors how `win::chart`'s pre-#810 rasteriser
    /// already built every polyline, one `draw_line` call per segment,
    /// since Direct2D's `ID2D1RenderTarget` has no multi-segment stroke
    /// helper here either).
    fn stroke_polyline(
        surface: &mut dyn NativeSurface,
        points: &[(f32, f32)],
        color: Color,
        stroke_width: f32,
    ) {
        for pair in points.windows(2) {
            let (from, to) = (pair[0], pair[1]);
            surface.surface_draw_line(
                crate::Point::new(from.0, from.1),
                crate::Point::new(to.0, to.1),
                color,
                stroke_width,
            );
        }
    }

    /// Paint a [`Chart`] (already resolved into `layout`) onto `surface`.
    /// See this module's doc for the divergences resolved while
    /// unifying three per-backend copies into this one.
    ///
    /// `layout` must be the same [`ChartLayout`] the caller uses for
    /// hit-testing/hover resolution — mirrors `primitives::form::paint`'s
    /// contract of reading geometry only from the caller-supplied
    /// layout, never recomputing it.
    pub(crate) fn paint(
        chart: &Chart,
        layout: &ChartLayout,
        surface: &mut dyn NativeSurface,
        theme: &Theme,
        hovered_point: Option<(usize, usize)>,
        crosshair_x: Option<f64>,
    ) {
        let b = layout.bounds;
        if b.width <= 0.0 || b.height <= 0.0 {
            return;
        }

        // Clip to the chart's own rect (quadraui#791) — see this
        // module's doc for why Windows didn't have this before.
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
            paint_hover_marker(surface, layout, si, di, chart);
        }

        surface.surface_pop_clip();
    }

    fn paint_sparkline(
        surface: &mut dyn NativeSurface,
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
        let color = series_color(chart, 0);
        let n = s.data.len();
        let points: Vec<(f32, f32)> = s
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
                (sx, sy)
            })
            .collect();
        stroke_polyline(surface, &points, color, 1.5);
    }

    fn paint_line(
        surface: &mut dyn NativeSurface,
        layout: &ChartLayout,
        chart: &Chart,
        theme: &Theme,
    ) {
        let pa = layout.plot_area;
        surface.surface_fill_rect(pa, theme.background);
        if pa.width <= 0.0 || pa.height <= 0.0 {
            return;
        }

        // Axes.
        surface.surface_draw_line(
            crate::Point::new(pa.x, pa.y),
            crate::Point::new(pa.x, pa.y + pa.height),
            theme.muted_fg,
            1.0,
        );
        surface.surface_draw_line(
            crate::Point::new(pa.x, pa.y + pa.height),
            crate::Point::new(pa.x + pa.width, pa.y + pa.height),
            theme.muted_fg,
            1.0,
        );

        for (si, s) in chart.series.iter().enumerate() {
            if s.data.is_empty() {
                continue;
            }
            let color = series_color(chart, si);
            let points: Vec<(f32, f32)> = layout
                .data_point_positions
                .iter()
                .filter_map(|&(pt_si, _, x, y)| if pt_si == si { Some((x, y)) } else { None })
                .collect();
            stroke_polyline(surface, &points, color, 2.0);
        }

        paint_legend(surface, layout, chart, theme);
        paint_axis_labels(surface, layout, chart, theme);
    }

    fn paint_bar(
        surface: &mut dyn NativeSurface,
        layout: &ChartLayout,
        chart: &Chart,
        theme: &Theme,
    ) {
        let pa = layout.plot_area;
        surface.surface_fill_rect(pa, theme.background);
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
                    surface.surface_fill_rect(
                        Rect::new(bx, by, seg_w, seg_h),
                        series_color(chart, si),
                    );
                }
            }

            surface.surface_draw_line(
                crate::Point::new(pa.x, baseline),
                crate::Point::new(pa.x + pa.width, baseline),
                theme.muted_fg,
                1.0,
            );
        }

        paint_legend(surface, layout, chart, theme);
        paint_axis_labels(surface, layout, chart, theme);
    }

    fn paint_legend(
        surface: &mut dyn NativeSurface,
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
            let color = series_color(chart, i);
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

    fn paint_axis_labels(
        surface: &mut dyn NativeSurface,
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

            if chart.show_grid && sy > pa.y && sy < pa.y + pa.height {
                surface.surface_draw_line(
                    crate::Point::new(pa.x, sy),
                    crate::Point::new(pa.x + pa.width, sy),
                    theme.separator,
                    0.5,
                );
            }
        }

        if let Some(label) = &chart.x_label {
            let (tw, th) = surface.surface_measure_text(label);
            let cx = pa.x + (pa.width - tw) / 2.0;
            let cy = pa.y + pa.height;
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
        surface: &mut dyn NativeSurface,
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

        let line_color = blend(theme.background, theme.muted_fg, 0.5);
        surface.surface_draw_line(
            crate::Point::new(screen_x, pa.y),
            crate::Point::new(screen_x, pa.y + pa.height),
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
            let color = series_color(chart, si);
            let norm = if range > 0.0 {
                ((val - y_min) / range).clamp(0.0, 1.0)
            } else {
                0.5
            };
            let sy = pa.y + pa.height - norm as f32 * pa.height;
            let (tw, th) = surface.surface_measure_text(&label);
            surface.surface_draw_text_run(
                Rect::new(screen_x + 4.0, sy - 8.0, tw, th),
                &label,
                color,
            );
        }
    }

    fn paint_hover_marker(
        surface: &mut dyn NativeSurface,
        layout: &ChartLayout,
        series_idx: usize,
        data_idx: usize,
        chart: &Chart,
    ) {
        let Some(&(_, _, sx, sy)) = layout
            .data_point_positions
            .iter()
            .find(|&&(si, di, _, _)| si == series_idx && di == data_idx)
        else {
            return;
        };
        let color = series_color(chart, series_idx);
        let inner = 5.0_f32;
        surface.surface_fill_rect(
            Rect::new(sx - inner, sy - inner, inner * 2.0, inner * 2.0),
            color,
        );
        let outer_color = blend(Theme::default().background, color, 0.3);
        let outer = 8.0_f32;
        surface.surface_fill_rect(
            Rect::new(sx - outer, sy - outer, outer * 2.0, outer * 2.0),
            outer_color,
        );
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::backend::ImagePaintResult;
        use crate::event::Viewport;
        use crate::primitives::chart::Series;
        use crate::types::WidgetId;
        use crate::Image;

        /// Records every drawing verb `paint` issues, so a paint
        /// assertion can run on any host — no Cairo, Core Graphics or
        /// Direct2D needed. Mirrors `primitives::find_replace`'s own
        /// `RecordingSurface` (#809): the pixel backends' real
        /// `ImageSurface`/`BitmapSurface`/`HeadlessSurface` driver tests
        /// still cover "the verb reached real pixels"; this covers "the
        /// shared painter emits the right verb at all", on every leg of
        /// the quality gate that enables gtk, win, or macos.
        #[derive(Default)]
        struct RecordingSurface {
            fills: Vec<(Rect, Color)>,
            lines: Vec<(crate::Point, crate::Point, Color, f32)>,
            clip_pushes: Vec<Rect>,
            clip_pops: usize,
        }

        impl NativeSurface for RecordingSurface {
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
            fn surface_stroke_rect(&mut self, _rect: Rect, _color: Color, _stroke_width: f32) {}
            fn surface_draw_text_run(&mut self, _rect: Rect, _text: &str, _color: Color) {}
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

        /// Regression for quadraui#791/#810: every pixel backend must
        /// clip a chart's paint to its own bounds — see this module's
        /// doc for why Windows had no clip at all before this phase.
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
    }
}

#[cfg(any(
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
#[allow(unused_imports)]
pub(crate) use native_surface_paint::{paint, SERIES_COLORS};

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

    // ── Y-axis gutter sizing (#647) ──────────────────────────────────────

    /// A `Line` chart with an explicit y-range/tick-count and no legend or
    /// x-label, for exercising gutter sizing in isolation.
    fn line_chart_with_ticks(y_range: (f64, f64), y_ticks: usize) -> Chart {
        Chart {
            id: WidgetId::new("chart"),
            kind: ChartKind::Line,
            series: vec![Series {
                label: "S".into(),
                data: vec![1.0, 2.0],
                color: None,
                fill: false,
            }],
            x_label: None,
            y_label: None,
            y_range: Some(y_range),
            x_range: None,
            show_legend: false,
            y_ticks: Some(y_ticks),
            x_ticks: None,
            show_grid: false,
        }
    }

    #[test]
    fn y_gutter_sizes_from_interior_tick_labels_not_just_endpoints() {
        // 0..18 over 5 ticks: the interior ticks format as "3.6", "7.2",
        // "10.8", "14.4" — 4 characters, longer than either endpoint
        // ("0"/"18", 1-2 chars). Pre-fix the gutter was sized from the
        // endpoints alone and the interior labels spilled outside the
        // chart's own bounds.
        let chart = line_chart_with_ticks((0.0, 18.0), 5);
        let m = ChartMeasure {
            width: 40.0,
            height: 20.0,
            char_width: 1.0,
            line_height: 1.0,
        };
        let bounds_x = 2.0;
        let layout = chart.layout(bounds_x, 0.0, m);
        assert!(
            layout.plot_area.x - bounds_x >= 5.0,
            "gutter should reserve the 4-char interior label + 1 padding: \
             plot_area.x={}, bounds.x={bounds_x}",
            layout.plot_area.x
        );
    }

    #[test]
    fn y_label_longer_than_tick_labels_widens_the_gutter() {
        // A `y_label` longer than the widest tick label must not be
        // silently truncated: the gutter has to widen to fit it too.
        let mut chart = line_chart_with_ticks((0.0, 18.0), 5);
        let long_label = "Merges per bucket"; // 18 chars vs. "14.4"'s 4.
        chart.y_label = Some(long_label.into());
        let m = ChartMeasure {
            width: 60.0,
            height: 20.0,
            char_width: 1.0,
            line_height: 1.0,
        };
        let layout = chart.layout(0.0, 0.0, m);
        let expected_min = long_label.len() as f32 + 1.0;
        assert!(
            layout.plot_area.x >= expected_min,
            "gutter should widen to fit the y_label in full: \
             plot_area.x={}, expected >= {expected_min}",
            layout.plot_area.x
        );
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
    fn stacked_bar_floor_anchors_at_zero_when_min_value_is_nonzero() {
        // Regression for the review finding on #584: two series, one
        // point each, neither containing a literal 0.0 anywhere. The
        // auto-derived floor must still be 0.0 (not 5.0, the smallest
        // raw value in the chart) or the segments below render
        // out-of-proportion to their true values.
        let chart = bar_chart(ChartKind::Bar, vec![vec![10.0], vec![5.0]]);
        assert_eq!(chart.effective_y_range(), (0.0, 15.0));

        let spans = chart.bar_column_spans(0);
        // A = 10 of 15 total → 2/3 of the stack; B = 5 of 15 → 1/3.
        // Pre-fix this rendered as an exact 50/50 split instead.
        assert!((spans[0].1 - 0.0).abs() < 1e-9);
        assert!((spans[0].2 - 2.0 / 3.0).abs() < 1e-9, "A span: {spans:?}");
        assert!((spans[1].1 - 2.0 / 3.0).abs() < 1e-9);
        assert!((spans[1].2 - 1.0).abs() < 1e-9, "B span: {spans:?}");
    }

    #[test]
    fn stacked_bar_floor_tracks_negative_partial_sums_not_just_totals() {
        // A column total alone can hide a dip below zero mid-stack:
        // 10 + (-15) + 20 sums to 15, but the running sum touches -5
        // partway through. The floor must cover that dip.
        let chart = bar_chart(ChartKind::Bar, vec![vec![10.0], vec![-15.0], vec![20.0]]);
        assert_eq!(chart.effective_y_range(), (-5.0, 15.0));
    }

    #[test]
    fn bar_column_spans_baseline_tracks_a_nonzero_explicit_floor() {
        // With an explicit y_range whose floor isn't 0.0, the stack's
        // baseline (where the bottom segment starts) must track
        // wherever zero maps to under that range, not the plot's
        // literal bottom edge.
        let mut chart = bar_chart(ChartKind::Bar, vec![vec![10.0], vec![10.0]]);
        chart.y_range = Some((-10.0, 30.0));
        let spans = chart.bar_column_spans(0);
        // norm(0.0) = (0 - -10) / 40 = 0.25.
        assert!((spans[0].1 - 0.25).abs() < 1e-9, "spans: {spans:?}");
    }

    #[test]
    fn single_series_bar_spans_rise_from_the_plot_floor_for_negative_data() {
        // Regression for review round 2 on #584: `bar_column_spans`'
        // stacked baseline used to be `norm(0.0)` for *any* `Bar` chart,
        // but a single-series chart keeps the plain min/max auto-range,
        // where 0.0 sits outside the data. Here the range is (-5, -1),
        // so `norm(0.0)` clamped to 1.0 and every bar collapsed to zero
        // height. Pre-#584 these rendered 0% / 50% / 100%.
        let chart = bar_chart(ChartKind::Bar, vec![vec![-5.0, -3.0, -1.0]]);
        assert_eq!(chart.effective_y_range(), (-5.0, -1.0));
        for (di, expected_top) in [0.0, 0.5, 1.0].into_iter().enumerate() {
            let spans = chart.bar_column_spans(di);
            assert_eq!(spans.len(), 1);
            assert!((spans[0].1 - 0.0).abs() < 1e-9, "col {di}: {spans:?}");
            assert!(
                (spans[0].2 - expected_top).abs() < 1e-9,
                "col {di}: {spans:?}"
            );
        }
    }

    #[test]
    fn single_series_bar_spans_are_unshifted_for_mixed_sign_data() {
        // Same root cause, milder symptom: with range (-2, 3) the old
        // `norm(0.0)` baseline of 0.4 made the tallest bar span
        // 40%–100% instead of the full plot height.
        let chart = bar_chart(ChartKind::Bar, vec![vec![-2.0, 3.0]]);
        assert_eq!(chart.effective_y_range(), (-2.0, 3.0));
        assert_eq!(chart.bar_column_spans(0), vec![(0, 0.0, 0.0)]);
        assert_eq!(chart.bar_column_spans(1), vec![(0, 0.0, 1.0)]);
    }

    #[test]
    fn stacked_spans_stay_visible_when_an_explicit_range_excludes_zero() {
        // Zero is above the whole range, so the stack's baseline clamps
        // to the plot ceiling and the segments hang below it. Deriving
        // each segment from consecutive cumulative levels keeps them
        // visible; carrying `top` forward used to collapse every
        // segment to zero height (review round 2, non-blocking note).
        let mut chart = bar_chart(ChartKind::Bar, vec![vec![-3.0], vec![-4.0]]);
        chart.y_range = Some((-10.0, -2.0));
        let spans = chart.bar_column_spans(0);
        // -3 maps to 0.875 and -7 to 0.375 over the 8-unit range.
        assert!((spans[0].1 - 0.875).abs() < 1e-9, "spans: {spans:?}");
        assert!((spans[0].2 - 1.0).abs() < 1e-9, "spans: {spans:?}");
        assert!((spans[1].1 - 0.375).abs() < 1e-9, "spans: {spans:?}");
        assert!((spans[1].2 - 0.875).abs() < 1e-9, "spans: {spans:?}");
        assert!(spans.iter().all(|(_, b, t)| t > b), "spans: {spans:?}");
    }

    #[test]
    fn stacked_span_of_a_negative_series_walks_the_stack_back_down() {
        // 10 then -15 then +20 over the auto range (-5, 15): the middle
        // series' segment spans downward from 10 to -5 and is still
        // returned bottom-first.
        let chart = bar_chart(ChartKind::Bar, vec![vec![10.0], vec![-15.0], vec![20.0]]);
        assert_eq!(chart.effective_y_range(), (-5.0, 15.0));
        let spans = chart.bar_column_spans(0);
        assert_eq!(
            spans,
            vec![(0, 0.25, 0.75), (1, 0.0, 0.75), (2, 0.0, 1.0)],
            "spans: {spans:?}"
        );
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
        assert!(chart.bar_column_spans_all().is_empty());
    }

    #[test]
    fn bar_column_spans_all_matches_the_per_column_helper() {
        // The bulk form exists only to resolve the y-range once for the
        // whole chart; it must agree with the single-column form column
        // for column, for both bar kinds.
        for kind in [ChartKind::Bar, ChartKind::BarGrouped] {
            let chart = bar_chart(kind, vec![vec![1.0, 5.0, -2.0], vec![2.0, 0.0, 3.0]]);
            let all = chart.bar_column_spans_all();
            assert_eq!(all.len(), chart.max_data_len(), "{kind:?}");
            for (di, column) in all.iter().enumerate() {
                assert_eq!(*column, chart.bar_column_spans(di), "{kind:?} col {di}");
            }
        }
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
