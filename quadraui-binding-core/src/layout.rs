//! The retained layout-tree descriptor model a binding's render callback
//! decodes into: `LayoutNode` and its `Row`/`Column`/`Leaf`/`Split`/`Tabs`/
//! `Panel`/`SidebarPanel`/`SplitTree` shapes, each paired with the
//! `quadraui` descriptor(s) it carries. A binding owns decoding its own
//! wire format (a Python `dict`, a JS object, ...) into these types; this
//! module owns what the decoded shape looks like and how it paints
//! ([`crate::render::paint_layout`]).

use std::collections::HashSet;
use std::rc::Rc;

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

    /// Shrinks `rect` by these insets, each independently cell-quantised
    /// via [`quantise_padding`] first — floored at zero width/height so an
    /// inset wider than `rect` never produces a negative size.
    pub fn inset(&self, rect: Rect) -> Rect {
        let top = quantise_padding(self.top);
        let right = quantise_padding(self.right);
        let bottom = quantise_padding(self.bottom);
        let left = quantise_padding(self.left);
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
/// quadraui's binding-core coordinate space is unit-agnostic (`Rect` is
/// plain `f32` everywhere — see `quadraui::flex`'s module doc): a Python/JS
/// render callback picks numbers in the active backend's own native
/// units, exactly like every other `Rect`-valued quantity in this tree. On
/// a pixel backend one native unit is one surface pixel, so a sub-pixel
/// spacing value is visually negligible; on `TuiBackend` one native unit
/// is exactly one terminal cell (`Backend::char_width`/`line_height` both
/// return `1.0`), so a fractional cell has no grid position to paint into.
/// [`EdgeInsets::inset`]/[`quantise_padding`]/[`quantise_margin`] round
/// every spacing value up to the next whole native unit before using it —
/// a no-op rounding on a pixel backend (sub-pixel noise only), the exact
/// "ceil to the next whole cell" rule on TUI. A `margin` under one unit
/// (so under one cell on TUI) drops to `0.0` outright rather than rounding
/// up to a whole cell of margin where the caller asked for none/a sliver —
/// see [`quantise_margin`]. `quadraui::style` documents the identical rule
/// for `quadraui::Style::padding`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Spacing {
    pub padding: EdgeInsets,
    pub margin: f32,
    pub gap: f32,
}

/// Rounds `value` up to the next whole native unit, floored at `0.0` —
/// [`Spacing`]'s doc explains why this is the shared TUI/pixel-backend
/// degrade rule for every padding/gap edge.
pub fn quantise_padding(value: f32) -> f32 {
    value.max(0.0).ceil()
}

/// [`quantise_padding`]'s rule, plus: a margin under one whole native unit
/// drops to `0.0` instead of rounding up — seeing a fractional margin
/// round *up* into a full cell of space on TUI when the caller meant
/// "basically none" would be more surprising than dropping it.
pub fn quantise_margin(value: f32) -> f32 {
    if value < 1.0 {
        0.0
    } else {
        value.ceil()
    }
}

/// A [`LayoutNode::Split`]'s decoded shape: the `quadraui::Split` descriptor
/// (its own `id`, `direction`, `ratio`, pane minimums) paired with the two
/// named child nodes it divides. `first`/`second` are full [`LayoutNode`]s,
/// not leaf-only slots, so a pane may itself hold a nested row/column/split.
pub struct LayoutSplit {
    pub split: Split,
    pub first: Box<LayoutNode>,
    pub second: Box<LayoutNode>,
}

/// A [`LayoutNode::Tabs`]'s decoded shape: a `quadraui::TabBar` descriptor
/// built from each tab's title (with `selected`'s item marked active) for
/// painting the strip, paired with `entries` holding the full content
/// subtree behind each tab. A tab group is a `Split` whose `first`/`second`
/// are each a `Tabs` — no separate "group" concept.
pub struct LayoutTabs {
    pub tab_bar: TabBar,
    pub entries: Vec<LayoutTabEntry>,
    pub selected: usize,
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
pub struct LayoutPanel {
    pub panel: Panel,
    pub content: Box<LayoutNode>,
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
