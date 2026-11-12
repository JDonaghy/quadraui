//! The retained layout-tree descriptor model a binding's render callback
//! decodes into: `LayoutNode` and its `Row`/`Column`/`Leaf`/`Split`/`Tabs`/
//! `Panel`/`SidebarPanel`/`SplitTree` shapes, each paired with the
//! `quadraui` descriptor(s) it carries. A binding owns decoding its own
//! wire format (a Python `dict`, a JS object, ...) into these types; this
//! module owns what the decoded shape looks like and how it paints
//! ([`crate::render::paint_layout`]).

use std::collections::HashSet;
use std::rc::Rc;

// `Panel` is `#[deprecated]` (use `SidebarPanel` + `Toolbar` instead);
// `LayoutNode::Panel`/`LayoutPanel` are this type's own wire-decode plumbing
// — the `"panel"` wire node itself is a separate breaking change.
#[allow(deprecated)]
use quadraui::{
    Button, DataTable, ListView, Panel, Rect, SectionSize, SidebarPanel, Split, SplitTree, TabBar,
    TextDisplay, TreeView,
};

use crate::extra_paint::ExtraPaint;
use crate::DecodeError;

/// One node of a render callback's layout tree: a `Row`/`Column` container
/// holding child nodes, a `Leaf` wrapping one id-addressable primitive
/// descriptor, a `Split` holding exactly two named child nodes divided by a
/// draggable ratio, a `Tabs` holding a tab strip plus one content subtree
/// per tab (of which only the selected one paints), a `Panel` or
/// `SidebarPanel` holding one chrome descriptor plus a single content
/// subtree, or a `SplitTree` holding an arbitrarily nested divider tree of
/// content subtrees.
pub enum LayoutNode {
    Row(LayoutContainer),
    Column(LayoutContainer),
    Leaf(LayoutLeaf),
    Split(LayoutSplit),
    Tabs(LayoutTabs),
    Panel(LayoutPanel),
    SidebarPanel(LayoutSidebarPanel),
    SplitTree(LayoutSplitTree),
}

/// A [`LayoutNode::Row`]/[`LayoutNode::Column`]'s decoded shape: its child
/// slots ([`LayoutChild`], each pairing one child node with its own
/// main-axis size/min/max) paired with the [`Spacing`] (`padding`/`margin`/
/// `gap`) and the main-axis [`Justify`]/cross-axis [`Align`] the render
/// callback's `"row"`/`"column"` dict declared alongside them — see
/// `crate::wire::layout_node_from_value`'s decode for the wire shape.
/// `spacing`/`justify`/`align` each default to no inset/no gap/`Start`/
/// `Stretch`, and every child's own size defaults to
/// [`SectionSize::EqualShare`] with no min/max, so an existing tree with
/// none of these keys paints the same shares as before they existed:
/// every child gets the same share of the full container, the way
/// [`crate::render::split_rects_with_gap`] (this container's own
/// predecessor) always divided it — down to floating-point tolerance, not
/// necessarily the identical bit pattern, since the two now reach that
/// share by different arithmetic (see
/// `crate::render::layout_container_rects`).
pub struct LayoutContainer {
    pub children: Vec<LayoutChild>,
    pub spacing: Spacing,
    pub justify: Justify,
    pub align: Align,
}

impl LayoutContainer {
    /// Builds a container whose every child uses the default
    /// [`LayoutChild::new`] sizing (`EqualShare`, no min/max) and whose
    /// `justify`/`align` are both their defaults — the equal-share-only
    /// shape every `"row"`/`"column"` node had before per-child sizing
    /// existed. Convenient for a call site that only ever deals in bare
    /// [`LayoutNode`]s (most of this crate's own tests, and any decode
    /// path that hasn't been taught the new per-child keys).
    pub fn new(children: Vec<LayoutNode>, spacing: Spacing) -> Self {
        Self {
            children: children.into_iter().map(LayoutChild::new).collect(),
            spacing,
            justify: Justify::default(),
            align: Align::default(),
        }
    }
}

/// One [`LayoutContainer`] child slot: the child [`LayoutNode`] itself,
/// paired with the main-axis [`SectionSize`] strategy it resolves under
/// (default [`SectionSize::EqualShare`], the only strategy that existed
/// before per-child sizing), and an optional `min`/`max` clamp in the same
/// portable surface-native-pixel unit [`Spacing`]'s fields use (quantised
/// to a whole native unit at paint time, same as `padding`/`margin`/`gap`
/// — see `crate::render::layout_container_rects`).
pub struct LayoutChild {
    pub node: LayoutNode,
    pub size: SectionSize,
    pub min: Option<f32>,
    pub max: Option<f32>,
}

impl LayoutChild {
    /// A child with the pre-existing default sizing: `EqualShare`, no
    /// min/max.
    pub fn new(node: LayoutNode) -> Self {
        Self {
            node,
            size: SectionSize::default(),
            min: None,
            max: None,
        }
    }
}

/// A [`LayoutContainer`]'s main-axis distribution of any leftover space
/// once every child's own size is resolved — CSS flexbox's
/// `justify-content`, restricted to the four values this layout tree
/// supports. Only visible when the children's resolved sizes sum to less
/// than the container's own main-axis extent (the common case — all
/// `EqualShare`, or a `Weight`/`EqualShare` mix with no min/max clamp —
/// always consumes the whole extent, so `justify` is a no-op there; it
/// matters once every child is `Fixed`/`Percent`/`Content`/clamped and
/// together they fall short of the container).
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

/// A [`LayoutContainer`]'s cross-axis treatment of each child — CSS
/// flexbox's `align-items`, restricted to the four values this layout tree
/// supports. `Stretch` (the default) gives every child the container's
/// full cross-axis extent, exactly like every `Row`/`Column` child before
/// `align` existed. `Start`/`Center`/`End` instead size a child to its own
/// natural cross-axis extent (see `crate::render::natural_size`) and
/// position it within the container's cross-axis extent; a child with no
/// cheap natural-size measurement on that axis (anything but a `Button`/
/// `TextDisplay`/`TabBar` leaf) degrades to `Stretch` instead of collapsing
/// to zero.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Align {
    Start,
    Center,
    End,
    #[default]
    Stretch,
}

/// One [`LayoutContainer`] child's main-axis sizing input to
/// [`resolve_main_axis_sizes`]: its [`SectionSize`] strategy, optional
/// `min`/`max` clamp (already converted to native units — see
/// `crate::render::layout_container_rects`), and `content` — the child's
/// own natural main-axis extent, consulted only when `size` is
/// [`SectionSize::Content`] (`0.0` for every other strategy, including a
/// `Content` child with no cheap natural-size measurement available).
#[derive(Clone, Copy, Debug)]
pub struct MainAxisSpec {
    pub size: SectionSize,
    pub min: Option<f32>,
    pub max: Option<f32>,
    pub content: f32,
}

/// Clamps `raw` between `spec.min`/`spec.max` (either bound optional),
/// then floors at `0.0` — a negative resolved size never reaches a
/// [`Rect`], regardless of how aggressively a caller-supplied `max` or a
/// `Percent`/`Weight` overflow squeezed it. `max` is applied first, then
/// `min` — so a child declaring a conflicting `min > max` resolves to
/// `min`, the same precedence CSS's own `min-width`/`max-width` conflict
/// resolution uses.
fn apply_min_max(spec: &MainAxisSpec, raw: f32) -> f32 {
    let mut v = raw;
    if let Some(max) = spec.max {
        v = v.min(max);
    }
    if let Some(min) = spec.min {
        v = v.max(min);
    }
    v.max(0.0)
}

/// Three-pass main-axis size resolution for a [`LayoutContainer`]'s
/// children — the same `Fixed` → `Percent` → `Weight`/`EqualShare` pass
/// order `quadraui::MultiSectionView`'s own per-section layout uses for
/// the identical [`SectionSize`] vocabulary, adapted for a plain
/// `Row`/`Column` child (no header chrome cost, no collapsed state).
///
/// Pass 1 resolves every `Fixed`/`Content` child exactly (clamped by its
/// own min/max). Pass 2 resolves every `Percent` child against
/// `container_main` (the container's full main-axis extent, before any
/// child consumed part of it); if the percentages overflow what's left of
/// `usable_main`, every `Percent` child scales down proportionally. Pass 3
/// splits whatever remains of `usable_main` across every `Weight`/
/// `EqualShare` child by weight ratio (`EqualShare` is `Weight(1.0)`); with
/// no such child, the remainder goes unused (same as before per-child
/// sizing existed only when every child was `EqualShare` and consumed the
/// whole extent by construction). `usable_main` is `container_main` minus
/// whatever gap the caller already reserved between children.
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
        let v = apply_min_max(&specs[i], percent_raw[k] * percent_scale);
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
                // this arm is unreachable today. `debug_assert!` instead
                // of `unreachable!` so a future `SectionSize` variant
                // this match hasn't been taught yet degrades to an
                // `EqualShare`-like flex share in a release build rather
                // than panicking the per-frame layout path.
                debug_assert!(false, "already resolved in pass 1/2");
                flex_indices.push(i);
                weights.push(1.0);
            }
        }
    }
    let weight_sum: f32 = weights.iter().sum();
    if !flex_indices.is_empty() && weight_sum > 0.0 {
        for (k, &i) in flex_indices.iter().enumerate() {
            let raw = remainder * (weights[k] / weight_sum);
            out[i] = apply_min_max(&specs[i], raw);
        }
    } else {
        for &i in &flex_indices {
            out[i] = apply_min_max(&specs[i], 0.0);
        }
    }

    out
}

/// Main-axis starting offset of each child, relative to the container's
/// own main-axis origin, once [`resolve_main_axis_sizes`] has resolved
/// every `sizes[i]`. `gap` (already in native units) is inserted between
/// every pair of adjacent children regardless of `justify`; `justify`
/// only disposes of `usable_main - (sum(sizes) + total gap)` — the
/// leftover once sizing and the declared gap are both accounted for. A
/// negative leftover (children overflow `usable_main`) clamps to `0.0`,
/// so `justify` never pushes a child to a negative offset.
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
/// own natural cross-axis extent (`None` when no cheap measurement exists
/// — see [`Align`]'s own doc) and the container's [`Align`]. `Stretch`
/// ignores `natural_cross` entirely and always returns `(0.0,
/// cross_extent)` — the pre-`align` behaviour. Every other value sizes the
/// child to `natural_cross` (falling back to `cross_extent` — i.e.
/// degrading to `Stretch` — when `None`), clamped to fit inside
/// `cross_extent`, then positions it at the start/middle/end of whatever
/// cross-axis space that size didn't use.
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

/// One edge inset a `padding` spacing key decodes into: all four sides
/// individually, so a layout dict can express a uniform inset (one
/// number, same on every side), an `{"x": .., "y": ..}` axis pair, or a
/// full `{"top": .., "right": .., "bottom": .., "left": ..}` shape — see
/// `crate::wire::edge_insets_from_value` for the decode. Each side
/// defaults to `0.0`.
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

    /// `x` on the left/right edges, `y` on the top/bottom ones — the
    /// `{"x": .., "y": ..}` wire shape.
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
    /// [`quantise_horizontal`] against `char_width` — the same pair of
    /// per-axis native-unit metrics `Backend::char_width`/
    /// `Backend::line_height` report, so a caller painting into a real
    /// backend passes those two numbers straight through (see
    /// `crate::render::inset_for_spacing`). Floored at zero width/height
    /// so an inset wider than `rect` never produces a negative size.
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

/// Spacing a [`LayoutNode::Row`]/[`LayoutNode::Column`] may declare: the
/// space inset from its own edges before laying out children (`padding`),
/// the space its parent leaves around it (`margin`, one value on every
/// side), and the space inserted between adjacent children on the
/// container's main axis (`gap`). All three default to `0.0` — no
/// spacing — matching a tree with no spacing keys at all.
///
/// # TUI story
///
/// Every value here is in **surface-native pixels**, exactly like
/// `quadraui::Style::padding` — a Python/JS render callback picks the same
/// number regardless of which backend ends up painting it. On a pixel
/// backend (GTK/macOS/Win) that number already IS the native unit, so
/// [`quantise_horizontal`]/[`quantise_vertical`] only round away sub-pixel
/// noise. On `TuiBackend` the native unit is one terminal cell, and
/// `Backend::char_width`/`line_height` both report `1.0` there — the
/// backend's own native unit already *is* one cell, architecturally, not a
/// real font pixel measurement — so dividing by either one can't supply
/// the pixel-to-cell conversion a TUI rasteriser needs. Instead,
/// [`quantise_horizontal`]/[`quantise_vertical`] fall back to
/// [`NOMINAL_CELL_PX_WIDTH`]/[`NOMINAL_CELL_PX_HEIGHT`] — the same "8×16
/// nominal terminal cell" estimate this tree's own monospace-width
/// heuristics already lean on (see `gtk::backend`'s tab-bar-layout twin) —
/// and round `ceil(px / nominal_cell_px)` up to the next whole cell. A
/// `margin` that rounds to under one native unit drops to `0.0` outright
/// rather than rounding up to a whole unit of margin where the caller
/// asked for none/a sliver — see [`quantise_margin_horizontal`]/
/// [`quantise_margin_vertical`]. `quadraui::style` documents the identical
/// rule for `quadraui::Style::padding`.
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
    /// the box) — the same order CSS's box model uses, and the shared
    /// geometry every spacing-aware layout node applies to its own area
    /// before laying out whatever is inside it (children, pane content,
    /// panel chrome, tab strip). `gap` plays no part here — it only
    /// matters between siblings/panes, handled by each node kind's own
    /// paint logic in `crate::render::paint_layout`.
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
}

/// Nominal pixel width of one terminal cell — the pixel reference
/// [`quantise_horizontal`] falls back to on a cell-grid backend, whose own
/// `Backend::char_width()` reports `1.0` rather than a real font
/// measurement. See [`Spacing`]'s doc for why a real per-backend metric
/// can't supply this.
const NOMINAL_CELL_PX_WIDTH: f32 = 8.0;

/// Nominal pixel height of one terminal cell — [`NOMINAL_CELL_PX_WIDTH`]'s
/// vertical counterpart, consumed by [`quantise_vertical`].
const NOMINAL_CELL_PX_HEIGHT: f32 = 16.0;

/// Converts a raw spacing value (surface-native pixels) into native units
/// on one axis, without rounding: `value` unchanged on a pixel backend
/// (`metric > 1.0`, a real font measurement), or `value / nominal_cell_px`
/// on a cell-grid backend (`metric <= 1.0` — architecturally always
/// exactly `1.0`; see [`Spacing`]'s doc for why that can't itself be the
/// pixel reference). Floored at `0.0` first so a spacing value below zero
/// (not reachable from today's wire decode, but defensive) never produces
/// a negative inset.
fn native_units(value: f32, metric: f32, nominal_cell_px: f32) -> f32 {
    let value = value.max(0.0);
    if metric <= 1.0 {
        value / nominal_cell_px
    } else {
        value
    }
}

/// Rounds a horizontal spacing value up to the next whole horizontal
/// native unit — [`Spacing`]'s doc explains the shared TUI/pixel-backend
/// degrade rule this implements for every padding/gap horizontal edge.
/// `char_width` is the painting backend's own `Backend::char_width()`.
pub fn quantise_horizontal(value: f32, char_width: f32) -> f32 {
    native_units(value, char_width, NOMINAL_CELL_PX_WIDTH).ceil()
}

/// [`quantise_horizontal`]'s vertical counterpart. `line_height` is the
/// painting backend's own `Backend::line_height()`.
pub fn quantise_vertical(value: f32, line_height: f32) -> f32 {
    native_units(value, line_height, NOMINAL_CELL_PX_HEIGHT).ceil()
}

/// Drops `native` to `0.0` when it is under one whole native unit, else
/// rounds it up — seeing a fractional margin round *up* into a full cell
/// of space on TUI when the caller meant "basically none" would be more
/// surprising than dropping it.
fn quantise_margin(native: f32) -> f32 {
    if native < 1.0 {
        0.0
    } else {
        native.ceil()
    }
}

/// [`quantise_horizontal`]'s rule, plus the [`quantise_margin`] drop —
/// margin's own degrade rule for a horizontal edge.
pub fn quantise_margin_horizontal(value: f32, char_width: f32) -> f32 {
    quantise_margin(native_units(value, char_width, NOMINAL_CELL_PX_WIDTH))
}

/// [`quantise_margin_horizontal`]'s vertical counterpart.
pub fn quantise_margin_vertical(value: f32, line_height: f32) -> f32 {
    quantise_margin(native_units(value, line_height, NOMINAL_CELL_PX_HEIGHT))
}

/// A [`LayoutNode::Split`]'s decoded shape: the `quadraui::Split` descriptor
/// (its own `id`, `direction`, `ratio`, pane minimums) paired with the two
/// named child nodes it divides. `first`/`second` are full [`LayoutNode`]s,
/// not leaf-only slots, so a pane may itself hold a nested row/column/split.
/// `spacing` is this node's own `padding`/`margin`/`gap` — `gap` is the
/// space between `first` and `second`, inserted after `Split::first_min`/
/// `second_min` are evaluated (see `crate::render::paint_layout`'s
/// `LayoutNode::Split` arm), so a minimum applies to the pane after the
/// gap is already carved out of the available space.
pub struct LayoutSplit {
    pub split: Split,
    pub first: Box<LayoutNode>,
    pub second: Box<LayoutNode>,
    pub spacing: Spacing,
}

/// A [`LayoutNode::Tabs`]'s decoded shape: a `quadraui::TabBar` descriptor
/// built from each tab's title (with `selected`'s item marked active) for
/// painting the strip, paired with `entries` holding the full content
/// subtree behind each tab. A tab group is a `Split` whose `first`/`second`
/// are each a `Tabs` — no separate "group" concept. `spacing` is this
/// node's own `padding`/`margin` (its `gap` is always `0.0` — a `"tabs"`
/// node decodes `"gap"` as an error instead, see
/// `crate::wire::spacing_from_value`), inset around the whole node
/// (strip and content both) before the strip/content split.
pub struct LayoutTabs {
    pub tab_bar: TabBar,
    pub entries: Vec<LayoutTabEntry>,
    pub selected: usize,
    pub spacing: Spacing,
    /// Index of the keyboard-focused tab, decoded from the `"tabs"`
    /// dict's optional `"focused_index"` key. Threaded into a
    /// [`quadraui::TabChrome`] rather than onto `tab_bar` directly — see
    /// that field's doc for why `TabBar` itself stays a plain field.
    pub focused_index: Option<usize>,
}

/// One tab inside a [`LayoutTabs`]: the id that addresses it, and the full
/// [`LayoutNode`] subtree painted beneath the strip while it is selected.
pub struct LayoutTabEntry {
    pub id: String,
    pub content: Box<LayoutNode>,
}

/// A [`LayoutNode::Panel`]'s decoded shape: the `quadraui::Panel` chrome
/// descriptor (title/actions/accent/collapsed) paired with the single
/// content subtree it frames. `content` is a full [`LayoutNode`], not a
/// leaf-only slot, so a panel may itself hold a nested row/column/split.
/// `spacing` is this node's own `padding`/`margin` (its `gap` is always
/// `0.0` — a `"panel"` node decodes `"gap"` as an error instead, see
/// `crate::wire::spacing_from_value`), inset around the whole panel
/// (chrome and content both) before the chrome is painted.
#[allow(deprecated)] // `Panel` is `#[deprecated]`; this struct is its own plumbing.
pub struct LayoutPanel {
    pub panel: Panel,
    pub content: Box<LayoutNode>,
    pub spacing: Spacing,
}

/// A [`LayoutNode::SidebarPanel`]'s decoded shape: the `quadraui::SidebarPanel`
/// chrome descriptor (optional toolbar) paired with the single content
/// subtree painted beneath the toolbar slot.
pub struct LayoutSidebarPanel {
    pub sidebar_panel: SidebarPanel,
    pub content: Box<LayoutNode>,
}

/// A [`LayoutNode::SplitTree`]'s decoded shape: the `quadraui::SplitTree`
/// geometry tree (every leaf position holding a synthetic, internal-only
/// `WidgetId`) paired with `leaves`, the matching content subtree for each
/// leaf position in the same pre-order [`SplitTree::leaf_ids`] would
/// produce. [`crate::render::paint_layout`]'s `LayoutNode::SplitTree` arm zips
/// `SplitTreeLayout::leaves`' resolved rects back against `leaves` by
/// position — the synthetic ids in `tree` are never user-facing and never
/// checked for uniqueness; `id` is the node's own user-supplied id instead.
pub struct LayoutSplitTree {
    pub id: String,
    pub tree: SplitTree,
    pub leaves: Vec<LayoutNode>,
}

/// One id-addressable primitive inside a layout tree, decoded from a
/// `{"id": ..., "<kind>": <descriptor>}` dict. `id` is unique across the
/// whole tree ([`validate_unique_leaf_ids`] checks this, called by
/// [`crate::wire::decode_render_output`] after decoding a tree), so a kind
/// may appear more than once (e.g. two `data_table`s side by side) with
/// each instance still addressable on its own.
pub struct LayoutLeaf {
    pub id: String,
    pub descriptor: LeafDescriptor,
    /// Optional pixel-precise scroll position lifted out of this leaf's
    /// raw `"list"`/`"tree"`/`"data_table"` descriptor dict by
    /// [`crate::scroll_px_from_value`] — `None` unless the dict set a
    /// numeric `"scroll_px"` key, and always `None` for a `descriptor`
    /// that isn't one of those three kinds. [`crate::render::paint_leaf`]
    /// passes it to the matching `Backend::draw_*_with_state`; a dict
    /// that never sets the key paints exactly as before.
    pub scroll_px: Option<f32>,
}

/// The decoded descriptor carried by a [`LayoutLeaf`] — one variant per
/// primitive kind a layout tree leaf may wrap.
pub enum LeafDescriptor {
    Button(Button),
    TextDisplay(TextDisplay),
    List(ListView),
    Tree(TreeView),
    DataTable(DataTable),
    TabBar(TabBar),
    /// Any additive primitive kind registered in a binding's own
    /// `EXTRA_PRIMITIVES` decode table. Unlike the six variants above, a
    /// new additive kind never adds a new `LeafDescriptor` variant — it
    /// routes through this one.
    Extra(Rc<dyn ExtraPaint>),
}

/// The descriptor keys a layout tree leaf dict may carry, one per
/// [`LeafDescriptor`] variant — the same names [`crate::render::RenderFrame`]'s
/// own flat-slot `Deserialize` impl uses for its slots.
pub const LEAF_KIND_KEYS: [&str; 6] = [
    "button",
    "text_display",
    "list",
    "tree",
    "data_table",
    "tab_bar",
];

/// Walks `node` collecting every leaf and split `id` into one namespace,
/// returning a [`DecodeError`] on the first repeat. A unique id per
/// leaf/split lets host code address one instance unambiguously even when a
/// kind appears more than once, and keeps a split's id from silently
/// shadowing a leaf's. `what` labels the error message the same way every
/// other decoder in this crate does — [`crate::wire::decode_render_output`]
/// passes its own caller-supplied label through unchanged.
pub fn validate_unique_leaf_ids(what: &str, node: &LayoutNode) -> Result<(), DecodeError> {
    fn walk(what: &str, node: &LayoutNode, seen: &mut HashSet<String>) -> Result<(), DecodeError> {
        match node {
            LayoutNode::Row(container) | LayoutNode::Column(container) => {
                for child in &container.children {
                    walk(what, &child.node, seen)?;
                }
                Ok(())
            }
            LayoutNode::Leaf(leaf) => {
                if !seen.insert(leaf.id.clone()) {
                    return Err(DecodeError(format!(
                        "{what}: duplicate leaf id {:?}",
                        leaf.id
                    )));
                }
                Ok(())
            }
            LayoutNode::Split(split) => {
                let id = split.split.id.as_str().to_owned();
                if !seen.insert(id.clone()) {
                    return Err(DecodeError(format!("{what}: duplicate leaf id {id:?}")));
                }
                walk(what, &split.first, seen)?;
                walk(what, &split.second, seen)
            }
            LayoutNode::Tabs(tabs) => {
                let id = tabs.tab_bar.id.as_str().to_owned();
                if !seen.insert(id.clone()) {
                    return Err(DecodeError(format!("{what}: duplicate leaf id {id:?}")));
                }
                for entry in &tabs.entries {
                    if !seen.insert(entry.id.clone()) {
                        return Err(DecodeError(format!(
                            "{what}: duplicate leaf id {:?}",
                            entry.id
                        )));
                    }
                    walk(what, &entry.content, seen)?;
                }
                Ok(())
            }
            #[allow(deprecated)] // `Panel` is `#[deprecated]`.
            LayoutNode::Panel(panel_node) => {
                let id = panel_node.panel.id.as_str().to_owned();
                if !seen.insert(id.clone()) {
                    return Err(DecodeError(format!("{what}: duplicate leaf id {id:?}")));
                }
                walk(what, &panel_node.content, seen)
            }
            LayoutNode::SidebarPanel(sidebar_node) => {
                let id = sidebar_node.sidebar_panel.id.as_str().to_owned();
                if !seen.insert(id.clone()) {
                    return Err(DecodeError(format!("{what}: duplicate leaf id {id:?}")));
                }
                walk(what, &sidebar_node.content, seen)
            }
            LayoutNode::SplitTree(split_tree) => {
                if !seen.insert(split_tree.id.clone()) {
                    return Err(DecodeError(format!(
                        "{what}: duplicate leaf id {:?}",
                        split_tree.id
                    )));
                }
                for leaf in &split_tree.leaves {
                    walk(what, leaf, seen)?;
                }
                Ok(())
            }
        }
    }
    let mut seen = HashSet::new();
    walk(what, node, &mut seen)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `TuiBackend::char_width`/`line_height`'s own values — the
    /// architectural `1.0` that flags a cell-grid backend.
    const TUI_CHAR_WIDTH: f32 = 1.0;
    const TUI_LINE_HEIGHT: f32 = 1.0;

    /// A real font's reported char/line metrics (GTK/macOS/Win) — any
    /// value `> 1.0` takes the pixel-backend passthrough branch, so these
    /// particular numbers aren't load-bearing, just plausible ones.
    const PIXEL_CHAR_WIDTH: f32 = 8.0;
    const PIXEL_LINE_HEIGHT: f32 = 16.0;

    #[test]
    fn quantise_horizontal_passes_a_pixel_backend_value_through() {
        assert_eq!(quantise_horizontal(12.0, PIXEL_CHAR_WIDTH), 12.0);
    }

    #[test]
    fn quantise_horizontal_shrinks_pixels_into_cells_on_a_cell_grid_backend() {
        // ceil(12 / 8) = 2 -- not "12 cells", the bug this function exists
        // to fix.
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

    fn spec(size: SectionSize) -> MainAxisSpec {
        MainAxisSpec {
            size,
            min: None,
            max: None,
            content: 0.0,
        }
    }

    #[test]
    fn resolve_main_axis_sizes_all_equal_share_divides_evenly() {
        let specs = vec![
            spec(SectionSize::EqualShare),
            spec(SectionSize::EqualShare),
            spec(SectionSize::EqualShare),
        ];
        assert_eq!(resolve_main_axis_sizes(&specs, 90.0, 90.0), vec![30.0; 3]);
    }

    #[test]
    fn resolve_main_axis_sizes_fixed_plus_weight_mix() {
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
    fn resolve_main_axis_sizes_percent_is_against_the_original_container() {
        let specs = vec![
            spec(SectionSize::Percent(0.25)),
            spec(SectionSize::EqualShare),
        ];
        let sizes = resolve_main_axis_sizes(&specs, 200.0, 200.0);
        assert_eq!(sizes, vec![50.0, 150.0]);
    }

    #[test]
    fn resolve_main_axis_sizes_percent_overflow_scales_down_proportionally() {
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
    fn resolve_main_axis_sizes_content_uses_the_supplied_content_size() {
        let specs = vec![
            MainAxisSpec {
                size: SectionSize::Content,
                min: None,
                max: None,
                content: 15.0,
            },
            spec(SectionSize::EqualShare),
        ];
        let sizes = resolve_main_axis_sizes(&specs, 100.0, 100.0);
        assert_eq!(sizes, vec![15.0, 85.0]);
    }

    #[test]
    fn resolve_main_axis_sizes_min_floors_a_fixed_child() {
        let specs = vec![MainAxisSpec {
            size: SectionSize::Fixed(5),
            min: Some(20.0),
            max: None,
            content: 0.0,
        }];
        assert_eq!(resolve_main_axis_sizes(&specs, 100.0, 100.0), vec![20.0]);
    }

    #[test]
    fn resolve_main_axis_sizes_max_caps_an_equal_share_child() {
        let specs = vec![
            MainAxisSpec {
                size: SectionSize::EqualShare,
                min: None,
                max: Some(10.0),
                content: 0.0,
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
    fn resolve_main_axis_sizes_with_no_flex_child_leaves_the_remainder_unused() {
        let specs = vec![spec(SectionSize::Fixed(30))];
        assert_eq!(resolve_main_axis_sizes(&specs, 100.0, 100.0), vec![30.0]);
    }

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
    /// fractional (half-cell) offset -- unlike every resolved *size*,
    /// which is `ceil`-quantised to a whole native unit before either
    /// function ever sees it (see `crate::render::layout_container_rects`).
    /// A real TUI paint still renders at an integer cell, so a half-cell
    /// `x`/`y` origin biases one way or the other when it's eventually
    /// floored/rounded -- a known rounding hazard pinned here rather than
    /// fixed, since hit-testing reads the same geometry and stays
    /// internally consistent either way.
    #[test]
    fn align_cross_with_an_odd_leftover_yields_a_fractional_offset() {
        assert_eq!(align_cross(41.0, Some(10.0), Align::Center), (15.5, 10.0));
    }

    #[test]
    fn layout_child_new_defaults_to_equal_share_with_no_min_max() {
        let child = LayoutChild::new(LayoutNode::Row(LayoutContainer::new(
            Vec::new(),
            Spacing::default(),
        )));
        assert_eq!(child.size, SectionSize::EqualShare);
        assert_eq!(child.min, None);
        assert_eq!(child.max, None);
    }
}
