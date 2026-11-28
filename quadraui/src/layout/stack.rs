//! The one stack layout solver: sizes and positions a run of children
//! along a main axis (a row or a column), plus the spacing and
//! native-unit quantisation every stacked layout shares.
//!
//! - [`resolve_main_axis_sizes`] is the three-pass `Fixed`/`Content` →
//!   `Percent` → `Weight`/`EqualShare` size resolution over the
//!   [`SectionSize`] vocabulary. A binding's `"row"`/`"column"` container,
//!   [`crate::MultiSectionView`]'s per-section stack and
//!   [`super::resolve_grid_tracks`]'s two track axes all resolve through it.
//! - [`justify_starts`] and [`align_cross`] place the resolved sizes on the
//!   main and cross axis.
//! - [`stack_rects`] runs the whole pipeline for a container of
//!   [`StackChild`]ren, including the `wrap` multi-line flow, against
//!   natural sizes the caller measures on demand.
//! - [`split_rects`]/[`split_rects_with_gap`] divide an area into equal
//!   shares; [`shrink_main_axis`]/[`apply_split_gap`] carve a gap out of a
//!   [`crate::Split`] and reinsert it after the split is laid out.
//! - [`Spacing`]/[`EdgeInsets`] and the `quantise_*` helpers convert
//!   portable surface-native-pixel spacing into a backend's native units.
//!
//! Everything here is pure geometry over [`Rect`] and `f32`: no backend,
//! no descriptor tree.

use super::NaturalSize;
use crate::event::Rect;
use crate::primitives::multi_section_view::SectionSize;
use crate::primitives::split::{SplitHit, SplitLayout};

// ── Main-axis distribution and cross-axis alignment ────────────────────────

/// A stack's main-axis distribution of any leftover space once every
/// child's own size is resolved — CSS flexbox's `justify-content`,
/// restricted to four values. Only visible when the children's resolved
/// sizes sum to less than the container's main-axis extent: a container
/// with any unclamped `Weight`/`EqualShare` child always consumes the
/// whole extent, so `justify` is a no-op there.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Justify {
    /// Leftover space goes after the last child.
    #[default]
    Start,
    /// Leftover space is split evenly before the first and after the
    /// last child.
    Center,
    /// Leftover space goes before the first child.
    End,
    /// No leading/trailing leftover; it is split evenly *between*
    /// children instead (a no-op with zero or one child).
    SpaceBetween,
}

/// A stack's cross-axis treatment of each child — CSS flexbox's
/// `align-items`, restricted to four values. `Stretch` (the default) gives
/// every child the container's full cross-axis extent. `Start`/`Center`/
/// `End` size a child to its own natural cross-axis extent and position it
/// within the container's; a child with no natural measurement on that
/// axis degrades to `Stretch` instead of collapsing to zero.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Align {
    Start,
    Center,
    End,
    #[default]
    Stretch,
}

/// One child's main-axis sizing input to [`resolve_main_axis_sizes`].
///
/// - `size`: its [`SectionSize`] strategy.
/// - `min`/`max`: optional clamp, already in native units.
/// - `content`: the child's natural main-axis extent, consulted only when
///   `size` is [`SectionSize::Content`].
/// - `floor`: the least raw size a `Percent`/`Weight`/`EqualShare` child
///   gets before its `min`/`max` clamp — [`crate::MultiSectionView`]
///   passes each section's header chrome cost so a flexible section never
///   resolves smaller than its own header; a plain row/column child
///   passes `0.0`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MainAxisSpec {
    pub size: SectionSize,
    pub min: Option<f32>,
    pub max: Option<f32>,
    pub content: f32,
    pub floor: f32,
}

impl MainAxisSpec {
    /// A spec with no clamp, no content size and no floor.
    pub const fn new(size: SectionSize) -> Self {
        Self {
            size,
            min: None,
            max: None,
            content: 0.0,
            floor: 0.0,
        }
    }
}

/// Clamps `raw` between `spec.min`/`spec.max` (either bound optional),
/// then floors at `0.0` so a negative resolved size never reaches a
/// [`Rect`]. `max` is applied first, then `min`, so a conflicting
/// `min > max` resolves to `min` — CSS's `min-width`/`max-width`
/// precedence.
pub fn apply_min_max(spec: &MainAxisSpec, raw: f32) -> f32 {
    let mut v = raw;
    if let Some(max) = spec.max {
        v = v.min(max);
    }
    if let Some(min) = spec.min {
        v = v.max(min);
    }
    v.max(0.0)
}

/// Three-pass main-axis size resolution.
///
/// Pass 1 resolves every `Fixed`/`Content` child exactly (clamped by its
/// own min/max). Pass 2 resolves every `Percent` child against
/// `container_main` (the full main-axis extent, before any child consumed
/// part of it); if the percentages overflow what is left of `usable_main`,
/// every `Percent` child scales down proportionally. Pass 3 splits
/// whatever remains of `usable_main` across every `Weight`/`EqualShare`
/// child by weight ratio (`EqualShare` is `Weight(1.0)`); with no such
/// child the remainder goes unused. `usable_main` is `container_main` minus
/// whatever gap the caller reserved between children. Each `Percent`/
/// `Weight`/`EqualShare` child's raw share is raised to its
/// [`MainAxisSpec::floor`] before the clamp.
pub fn resolve_main_axis_sizes(
    specs: &[MainAxisSpec],
    container_main: f32,
    usable_main: f32,
) -> Vec<f32> {
    let n = specs.len();
    let mut out = vec![0.0_f32; n];
    let mut done = vec![false; n];
    let mut consumed = 0.0_f32;

    // Pass 1: Fixed + Content.
    for (i, spec) in specs.iter().enumerate() {
        let raw = match spec.size {
            SectionSize::Fixed(v) => v as f32,
            SectionSize::Content => spec.content,
            _ => continue,
        };
        let v = apply_min_max(spec, raw);
        out[i] = v;
        consumed += v;
        done[i] = true;
    }

    // Pass 2: Percent, against the original `container_main`.
    let mut percent_indices = Vec::new();
    let mut percent_raw = Vec::new();
    for (i, spec) in specs.iter().enumerate() {
        if done[i] {
            continue;
        }
        if let SectionSize::Percent(p) = spec.size {
            percent_indices.push(i);
            percent_raw.push(p.clamp(0.0, 1.0) * container_main);
        }
    }
    let percent_total: f32 = percent_raw.iter().sum();
    let percent_budget = (usable_main - consumed).max(0.0);
    let percent_scale = if percent_total > percent_budget && percent_total > 0.0 {
        percent_budget / percent_total
    } else {
        1.0
    };
    for (k, &i) in percent_indices.iter().enumerate() {
        let raw = (percent_raw[k] * percent_scale).max(specs[i].floor);
        let v = apply_min_max(&specs[i], raw);
        out[i] = v;
        consumed += v;
        done[i] = true;
    }

    // Pass 3: Weight + EqualShare share whatever remains.
    let remainder = (usable_main - consumed).max(0.0);
    let mut flex_indices = Vec::new();
    let mut weights = Vec::new();
    for (i, spec) in specs.iter().enumerate() {
        if done[i] {
            continue;
        }
        match spec.size {
            SectionSize::Weight(w) => {
                flex_indices.push(i);
                weights.push(w.max(0.0));
            }
            SectionSize::EqualShare => {
                flex_indices.push(i);
                weights.push(1.0);
            }
            SectionSize::Fixed(_) | SectionSize::Content | SectionSize::Percent(_) => {
                // Every `done[i]` slot from pass 1/2 is skipped above, so
                // this arm is unreachable. `debug_assert!` rather than
                // `unreachable!` keeps a release build's per-frame layout
                // path panic-free: an unhandled variant degrades to an
                // `EqualShare`-like flex share.
                debug_assert!(false, "already resolved in pass 1/2");
                flex_indices.push(i);
                weights.push(1.0);
            }
        }
    }
    let weight_sum: f32 = weights.iter().sum();
    if !flex_indices.is_empty() && weight_sum > 0.0 {
        for (k, &i) in flex_indices.iter().enumerate() {
            let raw = (remainder * (weights[k] / weight_sum)).max(specs[i].floor);
            out[i] = apply_min_max(&specs[i], raw);
        }
    } else {
        for &i in &flex_indices {
            out[i] = apply_min_max(&specs[i], specs[i].floor);
        }
    }

    out
}

/// Main-axis starting offset of each child, relative to the container's
/// main-axis origin, once [`resolve_main_axis_sizes`] has resolved every
/// `sizes[i]`. `gap` (native units) is inserted between every pair of
/// adjacent children regardless of `justify`; `justify` only disposes of
/// `usable_main - (sum(sizes) + total gap)`. A negative leftover (children
/// overflow `usable_main`) clamps to `0.0`, so `justify` never pushes a
/// child to a negative offset.
pub fn justify_starts(sizes: &[f32], usable_main: f32, gap: f32, justify: Justify) -> Vec<f32> {
    let n = sizes.len();
    if n == 0 {
        return Vec::new();
    }
    let total: f32 = sizes.iter().sum::<f32>() + gap * (n - 1) as f32;
    let leftover = (usable_main - total).max(0.0);
    let (lead, extra) = match justify {
        Justify::Start => (0.0, 0.0),
        Justify::Center => (leftover / 2.0, 0.0),
        Justify::End => (leftover, 0.0),
        Justify::SpaceBetween => {
            if n > 1 {
                (0.0, leftover / (n - 1) as f32)
            } else {
                (0.0, 0.0)
            }
        }
    };
    let mut starts = Vec::with_capacity(n);
    let mut cursor = lead;
    for (i, &size) in sizes.iter().enumerate() {
        starts.push(cursor);
        cursor += size;
        if i + 1 < n {
            cursor += gap + extra;
        }
    }
    starts
}

/// A child's cross-axis `(offset, size)` within `cross_extent`, given its
/// natural cross-axis extent (`None` when no measurement exists) and the
/// container's [`Align`]. `Stretch` ignores `natural_cross` and returns
/// `(0.0, cross_extent)`. Every other value sizes the child to
/// `natural_cross` (falling back to `cross_extent` when `None`), clamped to
/// fit inside `cross_extent`, then positions it at the start/middle/end of
/// the cross-axis space that size leaves.
pub fn align_cross(cross_extent: f32, natural_cross: Option<f32>, align: Align) -> (f32, f32) {
    if align == Align::Stretch {
        return (0.0, cross_extent.max(0.0));
    }
    let size = natural_cross
        .unwrap_or(cross_extent)
        .clamp(0.0, cross_extent.max(0.0));
    let offset = match align {
        Align::Start => 0.0,
        Align::Center => ((cross_extent - size) / 2.0).max(0.0),
        Align::End => (cross_extent - size).max(0.0),
        Align::Stretch => 0.0,
    };
    (offset, size)
}

// ── Spacing and native-unit quantisation ───────────────────────────────────

/// One edge inset a `padding` spacing value decodes into: all four sides
/// individually, so a uniform inset, an `x`/`y` axis pair and a full
/// per-side shape share one type. Each side defaults to `0.0`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct EdgeInsets {
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
}

impl EdgeInsets {
    /// The same inset on every side.
    pub fn uniform(v: f32) -> Self {
        Self {
            top: v,
            right: v,
            bottom: v,
            left: v,
        }
    }

    /// `x` on the left/right edges, `y` on the top/bottom ones.
    pub fn xy(x: f32, y: f32) -> Self {
        Self {
            top: y,
            right: x,
            bottom: y,
            left: x,
        }
    }

    /// Shrinks `rect` by these insets: `top`/`bottom` quantised via
    /// [`quantise_vertical`] against `line_height`, `left`/`right` via
    /// [`quantise_horizontal`] against `char_width` — the per-axis
    /// native-unit metrics `Backend::char_width`/`Backend::line_height`
    /// report. Floored at zero width/height so an inset wider than `rect`
    /// never produces a negative size.
    pub fn inset(&self, rect: Rect, char_width: f32, line_height: f32) -> Rect {
        let top = quantise_vertical(self.top, line_height);
        let bottom = quantise_vertical(self.bottom, line_height);
        let left = quantise_horizontal(self.left, char_width);
        let right = quantise_horizontal(self.right, char_width);
        let width = (rect.width - left - right).max(0.0);
        let height = (rect.height - top - bottom).max(0.0);
        Rect::new(rect.x + left, rect.y + top, width, height)
    }
}

/// Spacing a stacked layout node may declare: the space inset from its own
/// edges before laying out children (`padding`), the space its parent
/// leaves around it (`margin`, one value on every side), and the space
/// inserted between adjacent children on the main axis (`gap`). All three
/// default to `0.0`.
///
/// # TUI story
///
/// Every value here is in **surface-native pixels**, exactly like
/// [`crate::Style::padding`] — a caller picks the same number regardless of
/// which backend paints it. On a pixel backend (GTK/macOS/Win) that number
/// already is the native unit, so [`quantise_horizontal`]/
/// [`quantise_vertical`] only round away sub-pixel noise. On `TuiBackend`
/// the native unit is one terminal cell and `Backend::char_width`/
/// `line_height` both report `1.0`, which cannot supply a pixel-to-cell
/// conversion; the quantisers fall back to [`NOMINAL_CELL_PX_WIDTH`]/
/// [`NOMINAL_CELL_PX_HEIGHT`] (a nominal 8×16 terminal cell) and round
/// `ceil(px / nominal_cell_px)` up to the next whole cell. A `margin` that
/// rounds to under one native unit drops to `0.0` rather than rounding up
/// to a whole unit — see [`quantise_margin_horizontal`]/
/// [`quantise_margin_vertical`].
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Spacing {
    pub padding: EdgeInsets,
    pub margin: f32,
    pub gap: f32,
}

impl Spacing {
    /// Shrinks `rect` by this spacing's `margin` (quantised per-axis via
    /// [`quantise_margin_horizontal`]/[`quantise_margin_vertical`], outside
    /// the box) and then its `padding` (via [`EdgeInsets::inset`], inside
    /// the box) — CSS's box-model order. `gap` plays no part here; it only
    /// matters between siblings.
    pub fn inset(&self, rect: Rect, char_width: f32, line_height: f32) -> Rect {
        let margin_h = quantise_margin_horizontal(self.margin, char_width);
        let margin_v = quantise_margin_vertical(self.margin, line_height);
        let after_margin = Rect::new(
            rect.x + margin_h,
            rect.y + margin_v,
            (rect.width - margin_h * 2.0).max(0.0),
            (rect.height - margin_v * 2.0).max(0.0),
        );
        self.padding.inset(after_margin, char_width, line_height)
    }

    /// The natural size of a box whose content measures `content`: the
    /// content plus this spacing's padding and margin on each measured
    /// axis, quantised as [`Self::inset`] quantises them, so `inset` of a
    /// rect this size gives the content back exactly. An axis `content`
    /// leaves unmeasured stays `None`.
    pub fn outset(
        &self,
        content: super::NaturalSize,
        char_width: f32,
        line_height: f32,
    ) -> super::NaturalSize {
        let margin_h = quantise_margin_horizontal(self.margin, char_width) * 2.0;
        let margin_v = quantise_margin_vertical(self.margin, line_height) * 2.0;
        let pad = &self.padding;
        let pad_h =
            quantise_horizontal(pad.left, char_width) + quantise_horizontal(pad.right, char_width);
        let pad_v =
            quantise_vertical(pad.top, line_height) + quantise_vertical(pad.bottom, line_height);
        super::NaturalSize::new(
            content.width.map(|w| w + pad_h + margin_h),
            content.height.map(|h| h + pad_v + margin_v),
        )
    }
}

/// Nominal pixel width of one terminal cell — the pixel reference
/// [`quantise_horizontal`] falls back to on a cell-grid backend, whose
/// `Backend::char_width()` reports `1.0` rather than a real font
/// measurement.
pub const NOMINAL_CELL_PX_WIDTH: f32 = 8.0;

/// Nominal pixel height of one terminal cell — [`NOMINAL_CELL_PX_WIDTH`]'s
/// vertical counterpart, consumed by [`quantise_vertical`].
pub const NOMINAL_CELL_PX_HEIGHT: f32 = 16.0;

/// Converts a raw spacing value (surface-native pixels) into native units
/// on one axis, without rounding: `value` unchanged on a pixel backend
/// (`metric > 1.0`), or `value / nominal_cell_px` on a cell-grid backend
/// (`metric <= 1.0`). Floored at `0.0` first so a negative spacing value
/// never produces a negative inset.
fn native_units(value: f32, metric: f32, nominal_cell_px: f32) -> f32 {
    let value = value.max(0.0);
    if metric <= 1.0 {
        value / nominal_cell_px
    } else {
        value
    }
}

/// Rounds a horizontal spacing value up to the next whole horizontal
/// native unit — [`Spacing`]'s doc explains the TUI/pixel-backend rule.
/// `char_width` is the painting backend's `Backend::char_width()`.
pub fn quantise_horizontal(value: f32, char_width: f32) -> f32 {
    native_units(value, char_width, NOMINAL_CELL_PX_WIDTH).ceil()
}

/// [`quantise_horizontal`]'s vertical counterpart. `line_height` is the
/// painting backend's `Backend::line_height()`.
pub fn quantise_vertical(value: f32, line_height: f32) -> f32 {
    native_units(value, line_height, NOMINAL_CELL_PX_HEIGHT).ceil()
}

/// Drops `native` to `0.0` when it is under one whole native unit, else
/// rounds it up: a fractional margin meant as "basically none" does not
/// grow into a full cell of space on TUI.
fn quantise_margin(native: f32) -> f32 {
    if native < 1.0 {
        0.0
    } else {
        native.ceil()
    }
}

/// [`quantise_horizontal`]'s rule plus the sub-unit margin drop, for a
/// horizontal margin edge.
pub fn quantise_margin_horizontal(value: f32, char_width: f32) -> f32 {
    quantise_margin(native_units(value, char_width, NOMINAL_CELL_PX_WIDTH))
}

/// [`quantise_margin_horizontal`]'s vertical counterpart.
pub fn quantise_margin_vertical(value: f32, line_height: f32) -> f32 {
    quantise_margin(native_units(value, line_height, NOMINAL_CELL_PX_HEIGHT))
}

// ── A whole stack: sizes, positions, wrapping ──────────────────────────────

/// One stacked child's main-axis sizing: its [`SectionSize`] strategy and
/// an optional `min`/`max` clamp, all in portable surface-native pixels
/// ([`stack_rects`] quantises them into native units, the same way
/// [`Spacing`]'s fields are).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct StackChild {
    pub size: SectionSize,
    pub min: Option<f32>,
    pub max: Option<f32>,
}

/// A stack's container-level options: the `gap` between adjacent children
/// (surface-native pixels), [`Justify`], [`Align`], and `wrap` — CSS
/// flexbox's `flex-wrap: wrap` in one direction (no `wrap-reverse`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct StackStyle {
    pub gap: f32,
    pub justify: Justify,
    pub align: Align,
    pub wrap: bool,
}

/// Resolves a stack's children into their final rects inside `area`
/// (already inset by the container's own `padding`/`margin`).
///
/// `horizontal` is `true` for a row (main axis = width), `false` for a
/// column. Each child's [`SectionSize`] main-axis size is clamped by its
/// own `min`/`max`; [`Justify`] distributes leftover main-axis space and
/// [`Align`] sizes and positions each child on the cross axis.
/// `char_width`/`line_height` are the painting backend's native-unit
/// metrics; `gap`, a `Fixed` size and `min`/`max` are quantised against them.
///
/// `natural(i)` measures child `i`'s intrinsic size. It is consulted only
/// where a natural size matters — a `Content` child's main axis, a
/// non-`Stretch` [`Align`]'s cross axis, and every child when
/// `style.wrap` is set — so a default `Stretch` stack of non-`Content`
/// children never measures anything.
///
/// With `style.wrap`, children are packed greedily onto main-axis lines (a
/// line always takes at least one child, even one wider than the container
/// on its own), and each line resolves its own sizes and positions through
/// [`resolve_main_axis_sizes`]/[`justify_starts`] — `justify` distributes
/// each line's own leftover space. Lines stack along the cross axis, each
/// sized to the largest natural cross extent it holds, separated by `gap`
/// quantised by the cross-axis rule. A child's [`SectionSize`] still
/// governs its size within its line, but line assignment bins by each
/// child's natural extent: `Fixed`/`Percent`/`Content` use the value they
/// resolve to, `Weight`/`EqualShare` use the natural main size, falling
/// back to one native unit.
pub fn stack_rects(
    area: Rect,
    children: &[StackChild],
    style: StackStyle,
    horizontal: bool,
    char_width: f32,
    line_height: f32,
    natural: &mut dyn FnMut(usize) -> NaturalSize,
) -> Vec<Rect> {
    if children.is_empty() {
        return Vec::new();
    }
    if style.wrap {
        return stack_rects_wrapped(
            area,
            children,
            style,
            horizontal,
            char_width,
            line_height,
            natural,
        );
    }
    let n = children.len();
    let (main_metric, quantise_main): (f32, fn(f32, f32) -> f32) = if horizontal {
        (char_width, quantise_horizontal)
    } else {
        (line_height, quantise_vertical)
    };
    let gap = quantise_main(style.gap, main_metric);
    let container_main = if horizontal { area.width } else { area.height };
    let usable_main = (container_main - gap * (n - 1) as f32).max(0.0);

    let specs: Vec<MainAxisSpec> = children
        .iter()
        .enumerate()
        .map(|(i, child)| {
            let mut spec = native_spec(child, main_metric, quantise_main);
            if matches!(spec.size, SectionSize::Content) {
                spec.content = natural(i).main(horizontal).unwrap_or(0.0);
            }
            spec
        })
        .collect();

    let sizes = resolve_main_axis_sizes(&specs, container_main, usable_main);
    // `justify_starts` reinserts the full gap total itself, so it takes
    // the container's full main-axis extent rather than the gap-reduced
    // `usable_main`.
    let starts = justify_starts(&sizes, container_main, gap, style.justify);

    let cross_extent = if horizontal { area.height } else { area.width };
    sizes
        .iter()
        .zip(starts.iter())
        .enumerate()
        .map(|(i, (&size, &start))| {
            let natural_cross = if style.align == Align::Stretch {
                None
            } else {
                natural(i).cross(horizontal)
            };
            let (cross_offset, cross_size) = align_cross(cross_extent, natural_cross, style.align);
            if horizontal {
                Rect::new(area.x + start, area.y + cross_offset, size, cross_size)
            } else {
                Rect::new(area.x + cross_offset, area.y + start, cross_size, size)
            }
        })
        .collect()
}

/// `child`'s [`MainAxisSpec`] with its `Fixed` size and `min`/`max`
/// quantised into native units. `content` is left at `0.0` for the caller
/// to fill in for a `Content` child.
fn native_spec(
    child: &StackChild,
    main_metric: f32,
    quantise_main: fn(f32, f32) -> f32,
) -> MainAxisSpec {
    let size = match child.size {
        SectionSize::Fixed(raw) => {
            SectionSize::Fixed(quantise_main(raw as f32, main_metric).round() as u16)
        }
        other => other,
    };
    MainAxisSpec {
        size,
        min: child.min.map(|v| quantise_main(v, main_metric)),
        max: child.max.map(|v| quantise_main(v, main_metric)),
        content: 0.0,
        floor: 0.0,
    }
}

/// A per-axis rounding function: `quantise_horizontal` or `quantise_vertical`.
type Quantise = fn(f32, f32) -> f32;

/// [`stack_rects`]'s multi-line path for a `wrap` stack.
fn stack_rects_wrapped(
    area: Rect,
    children: &[StackChild],
    style: StackStyle,
    horizontal: bool,
    char_width: f32,
    line_height: f32,
    natural: &mut dyn FnMut(usize) -> NaturalSize,
) -> Vec<Rect> {
    let n = children.len();
    let (main_metric, cross_metric, quantise_main, quantise_cross): (f32, f32, Quantise, Quantise) =
        if horizontal {
            (
                char_width,
                line_height,
                quantise_horizontal,
                quantise_vertical,
            )
        } else {
            (
                line_height,
                char_width,
                quantise_vertical,
                quantise_horizontal,
            )
        };
    let gap = quantise_main(style.gap, main_metric);
    // The distance between lines runs along the cross axis, so it is
    // quantised by the cross-axis rule against the cross metric.
    let cross_gap = quantise_cross(style.gap, cross_metric);
    let container_main = if horizontal { area.width } else { area.height };

    let mut specs: Vec<MainAxisSpec> = Vec::with_capacity(n);
    let mut natural_cross: Vec<Option<f32>> = Vec::with_capacity(n);
    let mut bin_hints: Vec<f32> = Vec::with_capacity(n);
    for (i, child) in children.iter().enumerate() {
        let measured = natural(i);
        let mut spec = native_spec(child, main_metric, quantise_main);
        if matches!(spec.size, SectionSize::Content) {
            spec.content = measured.main(horizontal).unwrap_or(0.0);
        }
        let hint = match spec.size {
            SectionSize::Fixed(raw) => raw as f32,
            SectionSize::Content => spec.content,
            SectionSize::Percent(p) => p.clamp(0.0, 1.0) * container_main,
            SectionSize::Weight(_) | SectionSize::EqualShare => {
                measured.main(horizontal).unwrap_or(main_metric)
            }
        };
        bin_hints.push(apply_min_max(&spec, hint));
        natural_cross.push(measured.cross(horizontal));
        specs.push(spec);
    }

    // Greedy line assignment: a line always holds at least its first
    // child regardless of overflow.
    let mut lines: Vec<Vec<usize>> = Vec::new();
    let mut line_main = 0.0_f32;
    for (i, &hint) in bin_hints.iter().enumerate() {
        let starts_new_line = match lines.last() {
            Some(last) if !last.is_empty() => line_main + gap + hint > container_main,
            _ => true,
        };
        if starts_new_line {
            lines.push(Vec::new());
            line_main = 0.0;
        }
        let current = lines.last_mut().expect("a line was just pushed if empty");
        if !current.is_empty() {
            line_main += gap;
        }
        current.push(i);
        line_main += hint;
    }

    let mut rects = vec![Rect::new(0.0, 0.0, 0.0, 0.0); n];
    let mut cross_pos = 0.0_f32;
    for line in &lines {
        let line_usable_main = (container_main - gap * (line.len() - 1) as f32).max(0.0);
        let line_specs: Vec<MainAxisSpec> = line.iter().map(|&i| specs[i]).collect();
        let sizes = resolve_main_axis_sizes(&line_specs, container_main, line_usable_main);
        let starts = justify_starts(&sizes, container_main, gap, style.justify);
        let line_cross = line
            .iter()
            .map(|&i| natural_cross[i].unwrap_or(cross_metric))
            .fold(0.0_f32, f32::max);
        for (local, &i) in line.iter().enumerate() {
            let (cross_offset, cross_size) = align_cross(line_cross, natural_cross[i], style.align);
            let main_start = starts[local];
            let main_size = sizes[local];
            rects[i] = if horizontal {
                Rect::new(
                    area.x + main_start,
                    area.y + cross_pos + cross_offset,
                    main_size,
                    cross_size,
                )
            } else {
                Rect::new(
                    area.x + cross_pos + cross_offset,
                    area.y + main_start,
                    cross_size,
                    main_size,
                )
            };
        }
        cross_pos += line_cross + cross_gap;
    }

    rects
}

// ── Equal splits and split gaps ────────────────────────────────────────────

/// Shrinks `area`'s main-axis extent by `gap`, anchored at the same
/// origin — the rect a [`crate::Split`] is laid out into so its ratio and
/// `first_min`/`second_min` clamping evaluate against the space left once
/// the gap between panes is carved out. [`apply_split_gap`] reinserts the
/// carved-out space afterwards.
pub fn shrink_main_axis(area: Rect, horizontal: bool, gap: f32) -> Rect {
    if horizontal {
        Rect::new(area.x, area.y, (area.width - gap).max(0.0), area.height)
    } else {
        Rect::new(area.x, area.y, area.width, (area.height - gap).max(0.0))
    }
}

/// Reinserts `gap` between a [`SplitLayout`]'s two panes after it was laid
/// out into [`shrink_main_axis`]'s gap-reduced rect: the divider and first
/// pane stay where they were, and the second pane — its `second_bounds`
/// plus its [`SplitHit::SecondPane`] hit region — shifts forward by `gap`
/// along the main axis. `bounds` widens back to the pre-shrink area. A
/// no-op when `gap` is `0.0`.
pub fn apply_split_gap(layout: &mut SplitLayout, horizontal: bool, gap: f32) {
    if gap <= 0.0 {
        return;
    }
    let shift = |rect: Rect| -> Rect {
        if horizontal {
            Rect::new(rect.x + gap, rect.y, rect.width, rect.height)
        } else {
            Rect::new(rect.x, rect.y + gap, rect.width, rect.height)
        }
    };
    layout.second_bounds = shift(layout.second_bounds);
    for (rect, hit) in layout.hit_regions.iter_mut() {
        if matches!(hit, SplitHit::SecondPane(_)) {
            *rect = shift(*rect);
        }
    }
    layout.bounds = if horizontal {
        Rect::new(
            layout.bounds.x,
            layout.bounds.y,
            layout.bounds.width + gap,
            layout.bounds.height,
        )
    } else {
        Rect::new(
            layout.bounds.x,
            layout.bounds.y,
            layout.bounds.width,
            layout.bounds.height + gap,
        )
    };
}

/// Divides `area` into `n` equal shares, side by side when `horizontal` or
/// stacked top-to-bottom otherwise, with `gap` between adjacent shares —
/// quantised via [`quantise_horizontal`] (against `char_width`) when
/// `horizontal`, or [`quantise_vertical`] (against `line_height`)
/// otherwise, so the distance between two shares is always a whole number
/// of TUI cells. The shares themselves stay fractional. Returns an empty
/// `Vec` for `n == 0`. When `gap` alone consumes the whole of `area`, every
/// share is zero-sized and its origin clamps to `area`'s far edge, so the
/// geometry never escapes `area`.
pub fn split_rects_with_gap(
    area: Rect,
    n: usize,
    horizontal: bool,
    gap: f32,
    char_width: f32,
    line_height: f32,
) -> Vec<Rect> {
    if n == 0 {
        return Vec::new();
    }
    let gap = if horizontal {
        quantise_horizontal(gap, char_width)
    } else {
        quantise_vertical(gap, line_height)
    };
    let n_f = n as f32;
    let total_gap = gap * (n.saturating_sub(1)) as f32;
    (0..n)
        .map(|i| {
            let i_f = i as f32;
            if horizontal {
                let share = ((area.width - total_gap) / n_f).max(0.0);
                let x = (area.x + (share + gap) * i_f).min(area.x + area.width);
                Rect::new(x, area.y, share, area.height)
            } else {
                let share = ((area.height - total_gap) / n_f).max(0.0);
                let y = (area.y + (share + gap) * i_f).min(area.y + area.height);
                Rect::new(area.x, y, area.width, share)
            }
        })
        .collect()
}

/// Divides `area` into `n` equal shares with no gap —
/// [`split_rects_with_gap`] with `gap: 0.0`, which never consults either
/// metric.
pub fn split_rects(area: Rect, n: usize, horizontal: bool) -> Vec<Rect> {
    split_rects_with_gap(area, n, horizontal, 0.0, 1.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::split::{Split, SplitDirection, SplitMeasure};
    use crate::WidgetId;

    /// `TuiBackend::char_width`/`line_height`'s values — the architectural
    /// `1.0` that flags a cell-grid backend.
    const TUI_CHAR_WIDTH: f32 = 1.0;
    const TUI_LINE_HEIGHT: f32 = 1.0;

    /// A real font's reported char/line metrics (GTK/macOS/Win) — any
    /// value `> 1.0` takes the pixel-backend passthrough branch.
    const PIXEL_CHAR_WIDTH: f32 = 8.0;
    const PIXEL_LINE_HEIGHT: f32 = 16.0;

    // ── Quantisation and spacing ─────────────────────────────────────

    #[test]
    fn quantise_horizontal_passes_a_pixel_backend_value_through() {
        assert_eq!(quantise_horizontal(12.0, PIXEL_CHAR_WIDTH), 12.0);
    }

    #[test]
    fn quantise_horizontal_shrinks_pixels_into_cells_on_a_cell_grid_backend() {
        // ceil(12 / 8) = 2, not 12 cells.
        assert_eq!(quantise_horizontal(12.0, TUI_CHAR_WIDTH), 2.0);
    }

    #[test]
    fn quantise_vertical_shrinks_pixels_into_cells_on_a_cell_grid_backend() {
        // ceil(12 / 16) = 1.
        assert_eq!(quantise_vertical(12.0, TUI_LINE_HEIGHT), 1.0);
    }

    #[test]
    fn quantise_margin_horizontal_drops_a_sub_cell_margin_on_tui() {
        // 4px is under one nominal 8px-wide cell, so it drops to zero
        // rather than rounding up to a whole cell of margin.
        assert_eq!(quantise_margin_horizontal(4.0, TUI_CHAR_WIDTH), 0.0);
    }

    #[test]
    fn quantise_margin_horizontal_rounds_up_once_it_clears_one_cell() {
        // ceil(9 / 8) = 2.
        assert_eq!(quantise_margin_horizontal(9.0, TUI_CHAR_WIDTH), 2.0);
    }

    #[test]
    fn quantise_margin_vertical_drops_a_sub_cell_margin_on_a_pixel_backend() {
        assert_eq!(quantise_margin_vertical(0.5, PIXEL_LINE_HEIGHT), 0.0);
    }

    #[test]
    fn edge_insets_inset_uses_the_matching_axis_metric_per_side() {
        let rect = Rect::new(0.0, 0.0, 80.0, 24.0);
        let insets = EdgeInsets::uniform(12.0);
        let inset = insets.inset(rect, TUI_CHAR_WIDTH, TUI_LINE_HEIGHT);
        assert_eq!(inset, Rect::new(2.0, 1.0, 76.0, 22.0));
    }

    /// A split's `margin`/`padding` inset the whole split area before the
    /// split divides it; on a pixel backend the values pass straight
    /// through.
    #[test]
    fn spacing_insets_the_whole_area_on_a_pixel_backend() {
        let area = Rect::new(0.0, 0.0, 100.0, 50.0);
        let spacing = Spacing {
            padding: EdgeInsets::uniform(5.0),
            margin: 3.0,
            gap: 0.0,
        };
        let inset = spacing.inset(area, PIXEL_CHAR_WIDTH, PIXEL_LINE_HEIGHT);
        assert_eq!(inset, Rect::new(8.0, 8.0, 100.0 - 16.0, 50.0 - 16.0));
    }

    /// The same inset on TUI shrinks the pixel padding into whole cells.
    #[test]
    fn spacing_insets_the_whole_area_on_tui() {
        let area = Rect::new(0.0, 0.0, 80.0, 24.0);
        let spacing = Spacing {
            padding: EdgeInsets::uniform(12.0),
            margin: 0.0,
            gap: 0.0,
        };
        let inset = spacing.inset(area, TUI_CHAR_WIDTH, TUI_LINE_HEIGHT);
        // ceil(12 / 8) = 2 columns each side, ceil(12 / 16) = 1 row each side.
        assert_eq!(inset, Rect::new(2.0, 1.0, 80.0 - 4.0, 24.0 - 2.0));
    }

    // ── resolve_main_axis_sizes ──────────────────────────────────────

    fn spec(size: SectionSize) -> MainAxisSpec {
        MainAxisSpec::new(size)
    }

    #[test]
    fn main_axis_sizes_all_equal_share_divides_evenly() {
        let specs = vec![
            spec(SectionSize::EqualShare),
            spec(SectionSize::EqualShare),
            spec(SectionSize::EqualShare),
        ];
        assert_eq!(resolve_main_axis_sizes(&specs, 90.0, 90.0), vec![30.0; 3]);
    }

    #[test]
    fn main_axis_sizes_fixed_plus_weight_mix() {
        // Fixed(20) first, the 80 that's left split 2:1 between the two
        // Weight children -- 53.333.. and 26.666...
        let specs = vec![
            spec(SectionSize::Fixed(20)),
            spec(SectionSize::Weight(2.0)),
            spec(SectionSize::Weight(1.0)),
        ];
        let sizes = resolve_main_axis_sizes(&specs, 100.0, 100.0);
        assert_eq!(sizes[0], 20.0);
        assert!((sizes[1] - 53.333_332).abs() < 0.01);
        assert!((sizes[2] - 26.666_666).abs() < 0.01);
    }

    #[test]
    fn main_axis_sizes_percent_is_against_the_original_container() {
        let specs = vec![
            spec(SectionSize::Percent(0.25)),
            spec(SectionSize::EqualShare),
        ];
        let sizes = resolve_main_axis_sizes(&specs, 200.0, 200.0);
        assert_eq!(sizes, vec![50.0, 150.0]);
    }

    #[test]
    fn main_axis_sizes_percent_overflow_scales_down_proportionally() {
        // Two 70% children overflow the 100-wide container; both scale
        // down by the same factor so they still sum to the full width.
        let specs = vec![
            spec(SectionSize::Percent(0.7)),
            spec(SectionSize::Percent(0.7)),
        ];
        let sizes = resolve_main_axis_sizes(&specs, 100.0, 100.0);
        assert_eq!(sizes, vec![50.0, 50.0]);
    }

    #[test]
    fn main_axis_sizes_content_uses_the_supplied_content_size() {
        let specs = vec![
            MainAxisSpec {
                content: 15.0,
                ..spec(SectionSize::Content)
            },
            spec(SectionSize::EqualShare),
        ];
        let sizes = resolve_main_axis_sizes(&specs, 100.0, 100.0);
        assert_eq!(sizes, vec![15.0, 85.0]);
    }

    #[test]
    fn main_axis_sizes_min_floors_a_fixed_child() {
        let specs = vec![MainAxisSpec {
            min: Some(20.0),
            ..spec(SectionSize::Fixed(5))
        }];
        assert_eq!(resolve_main_axis_sizes(&specs, 100.0, 100.0), vec![20.0]);
    }

    #[test]
    fn main_axis_sizes_max_caps_an_equal_share_child() {
        let specs = vec![
            MainAxisSpec {
                max: Some(10.0),
                ..spec(SectionSize::EqualShare)
            },
            spec(SectionSize::EqualShare),
        ];
        // Each would get 50 by equal share; the first is capped to 10.
        assert_eq!(
            resolve_main_axis_sizes(&specs, 100.0, 100.0),
            vec![10.0, 50.0]
        );
    }

    #[test]
    fn main_axis_sizes_with_no_flex_child_leaves_the_remainder_unused() {
        let specs = vec![spec(SectionSize::Fixed(30))];
        assert_eq!(resolve_main_axis_sizes(&specs, 100.0, 100.0), vec![30.0]);
    }

    #[test]
    fn main_axis_sizes_raises_a_flex_share_to_its_floor() {
        // Three equal shares of 9 would each get 3; the first's floor of
        // 5 lifts it, the others keep their 3.
        let specs = vec![
            MainAxisSpec {
                floor: 5.0,
                ..spec(SectionSize::EqualShare)
            },
            spec(SectionSize::EqualShare),
            spec(SectionSize::EqualShare),
        ];
        assert_eq!(
            resolve_main_axis_sizes(&specs, 9.0, 9.0),
            vec![5.0, 3.0, 3.0]
        );
    }

    /// `MultiSectionView`'s per-section stack — a `Fixed` command row, a
    /// collapsed section reduced to its header, a `Percent` section and two
    /// `EqualShare` sections whose header chrome floors them — resolves
    /// through this solver to the same sizes its own three-pass resolution
    /// produced: Fixed(3), collapsed header 1, 40% of 50 = 20, and the 26
    /// remaining split 13/13.
    #[test]
    fn multi_section_view_three_pass_case_resolves_through_the_shared_solver() {
        use crate::primitives::multi_section_view::{
            Axis, EmptyBody, MsvLayoutMetrics, MultiSectionView, ScrollMode, Section, SectionBody,
            SectionHeader, SectionMeasure,
        };
        use crate::types::StyledText;

        fn section(id: &str, size: SectionSize, collapsed: bool) -> Section {
            Section {
                id: id.into(),
                header: SectionHeader {
                    title: StyledText::plain(id),
                    ..Default::default()
                },
                body: SectionBody::Empty(EmptyBody::default()),
                aux: None,
                size,
                collapsed,
                min_size: None,
                max_size: None,
            }
        }
        let view = MultiSectionView {
            id: WidgetId::new("view"),
            sections: vec![
                section("cmd", SectionSize::Fixed(3), false),
                section("gone", SectionSize::EqualShare, true),
                section("pct", SectionSize::Percent(0.4), false),
                section("a", SectionSize::EqualShare, false),
                section("b", SectionSize::EqualShare, false),
            ],
            active_section: None,
            axis: Axis::Vertical,
            allow_resize: false,
            allow_collapse: true,
            scroll_mode: ScrollMode::PerSection,
            has_focus: false,
            panel_scroll: 0.0,
        };
        let bounds = Rect::new(0.0, 0.0, 30.0, 50.0);
        let layout = view.layout(bounds, MsvLayoutMetrics::default(), |_| {
            SectionMeasure::default()
        });
        let msv_sizes: Vec<f32> = layout.sections.iter().map(|s| s.resolved_size).collect();

        let header = MsvLayoutMetrics::default().header_size;
        let specs: Vec<MainAxisSpec> = vec![
            spec(SectionSize::Fixed(3)),
            MainAxisSpec {
                content: header,
                ..spec(SectionSize::Content)
            },
            MainAxisSpec {
                floor: header,
                ..spec(SectionSize::Percent(0.4))
            },
            MainAxisSpec {
                floor: header,
                ..spec(SectionSize::EqualShare)
            },
            MainAxisSpec {
                floor: header,
                ..spec(SectionSize::EqualShare)
            },
        ];
        let shared = resolve_main_axis_sizes(&specs, 50.0, 50.0);
        assert_eq!(shared, vec![3.0, header, 20.0, 13.0, 13.0]);
        assert_eq!(msv_sizes, shared);
    }

    // ── justify_starts / align_cross ─────────────────────────────────

    #[test]
    fn justify_starts_start_puts_leftover_after_the_last_child() {
        assert_eq!(
            justify_starts(&[10.0, 10.0], 100.0, 0.0, Justify::Start),
            vec![0.0, 10.0]
        );
    }

    #[test]
    fn justify_starts_center_splits_leftover_before_and_after() {
        // 100 - 20 = 80 leftover, 40 leads.
        assert_eq!(
            justify_starts(&[10.0, 10.0], 100.0, 0.0, Justify::Center),
            vec![40.0, 50.0]
        );
    }

    #[test]
    fn justify_starts_end_puts_leftover_before_the_first_child() {
        assert_eq!(
            justify_starts(&[10.0, 10.0], 100.0, 0.0, Justify::End),
            vec![80.0, 90.0]
        );
    }

    #[test]
    fn justify_starts_space_between_spreads_leftover_between_children() {
        // 100 - 30 = 70 leftover split between the 2 gaps between 3
        // children -- 35 each.
        assert_eq!(
            justify_starts(&[10.0, 10.0, 10.0], 100.0, 0.0, Justify::SpaceBetween),
            vec![0.0, 45.0, 90.0]
        );
    }

    #[test]
    fn justify_starts_space_between_with_one_child_is_a_no_op() {
        assert_eq!(
            justify_starts(&[10.0], 100.0, 0.0, Justify::SpaceBetween),
            vec![0.0]
        );
    }

    #[test]
    fn justify_starts_respects_a_declared_gap_regardless_of_justify() {
        // 100 - (10 + 10 + 5 gap) = 75 leftover, split before/after by
        // Center.
        assert_eq!(
            justify_starts(&[10.0, 10.0], 100.0, 5.0, Justify::Center),
            vec![37.5, 52.5]
        );
    }

    #[test]
    fn align_cross_stretch_ignores_natural_size_and_fills_the_extent() {
        assert_eq!(align_cross(40.0, Some(10.0), Align::Stretch), (0.0, 40.0));
    }

    #[test]
    fn align_cross_start_sizes_to_natural_and_anchors_at_zero() {
        assert_eq!(align_cross(40.0, Some(10.0), Align::Start), (0.0, 10.0));
    }

    #[test]
    fn align_cross_center_sizes_to_natural_and_centers_it() {
        assert_eq!(align_cross(40.0, Some(10.0), Align::Center), (15.0, 10.0));
    }

    #[test]
    fn align_cross_end_sizes_to_natural_and_anchors_at_the_far_edge() {
        assert_eq!(align_cross(40.0, Some(10.0), Align::End), (30.0, 10.0));
    }

    #[test]
    fn align_cross_with_no_natural_size_degrades_to_stretch() {
        assert_eq!(align_cross(40.0, None, Align::Center), (0.0, 40.0));
    }

    /// `align_cross`/`justify_starts` dispose of an odd leftover with a
    /// fractional (half-cell) offset, unlike every resolved size, which is
    /// quantised to a whole native unit first. Hit-testing reads the same
    /// geometry, so it stays internally consistent.
    #[test]
    fn align_cross_with_an_odd_leftover_yields_a_fractional_offset() {
        assert_eq!(align_cross(41.0, Some(10.0), Align::Center), (15.5, 10.0));
    }

    // ── stack_rects ──────────────────────────────────────────────────

    fn no_natural(_: usize) -> NaturalSize {
        NaturalSize::NONE
    }

    #[test]
    fn stack_rects_default_style_splits_equal_shares() {
        let children = [StackChild::default(); 3];
        let rects = stack_rects(
            Rect::new(0.0, 0.0, 30.0, 4.0),
            &children,
            StackStyle::default(),
            true,
            TUI_CHAR_WIDTH,
            TUI_LINE_HEIGHT,
            &mut no_natural,
        );
        assert_eq!(
            rects,
            vec![
                Rect::new(0.0, 0.0, 10.0, 4.0),
                Rect::new(10.0, 0.0, 10.0, 4.0),
                Rect::new(20.0, 0.0, 10.0, 4.0),
            ]
        );
    }

    #[test]
    fn stack_rects_measures_only_where_a_natural_size_matters() {
        let children = [
            StackChild::default(),
            StackChild {
                size: SectionSize::Content,
                ..StackChild::default()
            },
        ];
        let mut measured = Vec::new();
        let rects = stack_rects(
            Rect::new(0.0, 0.0, 20.0, 4.0),
            &children,
            StackStyle::default(),
            true,
            TUI_CHAR_WIDTH,
            TUI_LINE_HEIGHT,
            &mut |i| {
                measured.push(i);
                NaturalSize::new(Some(6.0), Some(1.0))
            },
        );
        assert_eq!(measured, vec![1], "only the Content child is measured");
        assert_eq!(rects[0], Rect::new(0.0, 0.0, 14.0, 4.0));
        assert_eq!(rects[1], Rect::new(14.0, 0.0, 6.0, 4.0));
    }

    #[test]
    fn stack_rects_wrap_flows_overflow_onto_a_new_line() {
        let children = [StackChild {
            size: SectionSize::Content,
            ..StackChild::default()
        }; 3];
        let rects = stack_rects(
            Rect::new(0.0, 0.0, 10.0, 6.0),
            &children,
            StackStyle {
                wrap: true,
                ..StackStyle::default()
            },
            true,
            TUI_CHAR_WIDTH,
            TUI_LINE_HEIGHT,
            &mut |_| NaturalSize::new(Some(4.0), Some(2.0)),
        );
        assert_eq!(
            rects,
            vec![
                Rect::new(0.0, 0.0, 4.0, 2.0),
                Rect::new(4.0, 0.0, 4.0, 2.0),
                Rect::new(0.0, 2.0, 4.0, 2.0),
            ]
        );
    }

    // ── Equal splits and split gaps ──────────────────────────────────

    /// When `gap` alone consumes the whole area, every share clamps to
    /// `0.0` and the geometry still stays inside `area`.
    #[test]
    fn split_rects_with_gap_clamps_origins_when_gap_exceeds_the_area() {
        let area = Rect::new(0.0, 0.0, 10.0, 10.0);
        let rects = split_rects_with_gap(area, 5, true, 4.0, PIXEL_CHAR_WIDTH, PIXEL_LINE_HEIGHT);
        assert_eq!(rects.len(), 5);
        for rect in &rects {
            assert_eq!(rect.width, 0.0);
            assert!(
                rect.x <= area.x + area.width,
                "origin must not escape area: {rect:?}"
            );
        }
    }

    #[test]
    fn split_rects_with_gap_leaves_whole_unit_gaps_between_shares() {
        let area = Rect::new(0.0, 0.0, 33.0, 10.0);
        let rects = split_rects_with_gap(area, 3, true, 3.0, PIXEL_CHAR_WIDTH, PIXEL_LINE_HEIGHT);
        assert_eq!(rects.len(), 3);
        // 33 width, 2 gaps of 3 => 27 left for 3 equal shares of 9 each.
        assert_eq!(rects[0], Rect::new(0.0, 0.0, 9.0, 10.0));
        assert_eq!(rects[1], Rect::new(12.0, 0.0, 9.0, 10.0));
        assert_eq!(rects[2], Rect::new(24.0, 0.0, 9.0, 10.0));
    }

    #[test]
    fn split_rects_with_gap_zero_matches_the_plain_equal_split() {
        let area = Rect::new(0.0, 0.0, 30.0, 10.0);
        assert_eq!(
            split_rects_with_gap(area, 3, true, 0.0, PIXEL_CHAR_WIDTH, PIXEL_LINE_HEIGHT),
            split_rects(area, 3, true)
        );
    }

    #[test]
    fn split_rects_with_gap_quantises_a_fractional_gap_up() {
        let area = Rect::new(0.0, 0.0, 20.0, 10.0);
        let rects = split_rects_with_gap(area, 2, true, 0.4, PIXEL_CHAR_WIDTH, PIXEL_LINE_HEIGHT);
        // 0.4 rounds up to a whole unit of gap (1.0).
        assert_eq!(rects[1].x - (rects[0].x + rects[0].width), 1.0);
    }

    /// The same fractional gap from a cell-grid backend's metrics rounds
    /// up to exactly one whole TUI cell.
    #[test]
    fn split_rects_with_gap_quantises_a_fractional_gap_up_on_tui() {
        let area = Rect::new(0.0, 0.0, 20.0, 10.0);
        let rects = split_rects_with_gap(area, 2, true, 0.4, TUI_CHAR_WIDTH, TUI_LINE_HEIGHT);
        assert_eq!(rects[1].x - (rects[0].x + rects[0].width), 1.0);
    }

    fn half_split() -> Split {
        Split {
            id: WidgetId::new("s"),
            direction: SplitDirection::Horizontal,
            ratio: 0.5,
            first_min: 0.0,
            second_min: 0.0,
        }
    }

    /// [`shrink_main_axis`]/[`apply_split_gap`] together implement a
    /// split's `gap`: the space the split divides is reduced by the gap
    /// first, and the second pane shifts forward by exactly the gap
    /// afterwards, landing flush with the original area's far edge.
    #[test]
    fn split_gap_is_carved_out_before_dividing_then_reinserted() {
        let area = Rect::new(0.0, 0.0, 100.0, 40.0);
        let gap = 10.0;
        let draw_area = shrink_main_axis(area, true, gap);
        assert_eq!(draw_area, Rect::new(0.0, 0.0, 90.0, 40.0));

        let mut layout = half_split().layout(draw_area, SplitMeasure::new(1.0));
        let first_width_before = layout.first_bounds.width;
        let second_x_before = layout.second_bounds.x;

        apply_split_gap(&mut layout, true, gap);

        assert_eq!(layout.first_bounds.width, first_width_before);
        assert_eq!(layout.second_bounds.x, second_x_before + gap);
        assert_eq!(
            layout.second_bounds.x + layout.second_bounds.width,
            area.x + area.width
        );
        let second_hit = layout
            .hit_regions
            .iter()
            .find(|(_, hit)| matches!(hit, SplitHit::SecondPane(_)))
            .expect("a second-pane hit region");
        assert_eq!(second_hit.0.x, second_x_before + gap);
    }

    #[test]
    fn split_gap_zero_is_a_no_op() {
        let area = Rect::new(0.0, 0.0, 100.0, 40.0);
        let mut layout = half_split().layout(area, SplitMeasure::new(1.0));
        let before = layout.clone();
        apply_split_gap(&mut layout, true, 0.0);
        assert_eq!(layout, before);
    }
}
