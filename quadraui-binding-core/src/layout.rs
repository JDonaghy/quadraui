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
    Button, DataTable, ListView, Panel, Rect, SidebarPanel, Split, SplitTree, TabBar, TextDisplay,
    TreeView,
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
/// nodes paired with the [`Spacing`] (`padding`/`margin`/`gap`) the render
/// callback's `"row"`/`"column"` dict declared alongside them — see
/// `crate::wire::layout_node_from_value`'s decode for the wire shape.
/// `spacing` defaults to [`Spacing::default`] (no inset, no gap), so an
/// existing tree with no spacing keys paints bit-for-bit identical to
/// before this field existed.
pub struct LayoutContainer {
    pub children: Vec<LayoutNode>,
    pub spacing: Spacing,
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
                    walk(what, child, seen)?;
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
}
