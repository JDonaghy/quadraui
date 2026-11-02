//! The retained layout-tree descriptor model a binding's render callback
//! decodes into: `LayoutNode` and its `Row`/`Column`/`Leaf`/`Split`/`Tabs`/
//! `Panel`/`SidebarPanel`/`SplitTree` shapes, each paired with the
//! `quadraui` descriptor(s) it carries. A binding owns decoding its own
//! wire format (a Python `dict`, a JS object, ...) into these types; this
//! module owns what the decoded shape looks like and how it paints
//! ([`crate::render::paint_layout`]).

use std::rc::Rc;

use quadraui::{
    Button, DataTable, ListView, Panel, SidebarPanel, Split, SplitTree, TabBar, TextDisplay,
    TreeView,
};

use crate::extra_paint::ExtraPaint;

/// One node of a render callback's layout tree: a `Row`/`Column` container
/// holding child nodes, a `Leaf` wrapping one id-addressable primitive
/// descriptor, a `Split` holding exactly two named child nodes divided by a
/// draggable ratio, a `Tabs` holding a tab strip plus one content subtree
/// per tab (of which only the selected one paints), a `Panel` or
/// `SidebarPanel` holding one chrome descriptor plus a single content
/// subtree, or a `SplitTree` holding an arbitrarily nested divider tree of
/// content subtrees.
pub enum LayoutNode {
    Row(Vec<LayoutNode>),
    Column(Vec<LayoutNode>),
    Leaf(LayoutLeaf),
    Split(LayoutSplit),
    Tabs(LayoutTabs),
    Panel(LayoutPanel),
    SidebarPanel(LayoutSidebarPanel),
    SplitTree(LayoutSplitTree),
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
/// produce. [`paint_layout`]'s `LayoutNode::SplitTree` arm zips
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
/// whole tree ([`validate_unique_leaf_ids`]), so a kind may appear more
/// than once (e.g. two `data_table`s side by side) with each instance
/// still addressable on its own.
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
    /// Any additive primitive kind registered in [`EXTRA_PRIMITIVES`].
    /// Unlike the six variants above, a new additive kind never adds a new
    /// `LeafDescriptor` variant — it routes through this one.
    Extra(Rc<dyn ExtraPaint>),
}

/// The descriptor keys a layout tree leaf dict may carry, one per
/// [`LeafDescriptor`] variant — the same names [`RenderFrame::from_dict`]
/// uses for its flat slots.
pub const LEAF_KIND_KEYS: [&str; 6] = [
    "button",
    "text_display",
    "list",
    "tree",
    "data_table",
    "tab_bar",
];
