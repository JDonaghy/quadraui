//! Frame → accessibility tree: the backend-independent builder that
//! turns an owned frame snapshot plus the current focus into a tree an
//! assistive technology can read.
//!
//! # Shape
//!
//! [`A11yTreeBuilder`] consumes a [`crate::frame::OwnedFrame`] (the
//! serde-capable per-surface snapshot) and a [`FocusManager`], and
//! produces either
//!
//! - a whole [`A11yTree`] — every node, in document order, root first —
//!   which is what tests and the snapshot seam assert against, or
//! - an [`A11yUpdate`] — the first call carries the full tree, every
//!   later call carries **only the nodes whose role, state, bounds or
//!   child list changed**. Under `--features a11y` an `A11yUpdate`
//!   converts to an [`accesskit::TreeUpdate`] via
//!   [`A11yUpdate::to_tree_update`].
//!
//! Nothing here touches a backend. The same tree comes out of the same
//! frame on every platform, which is why the test seam
//! ([`crate::testing::a11y`]) can build one with no window, no GTK, and
//! no terminal.
//!
//! # Node ids are stable across frames
//!
//! Every surface that carries a [`WidgetId`] gets
//! [`A11yNodeId::for_widget`] — a hash of the widget id's interned
//! string. Re-render the same widget at a different position, in a
//! different z-order, in a frame with different siblings, and its node
//! id is unchanged, which is exactly what the incremental diff and the
//! platform adapters need. Sub-nodes (list rows, toolbar buttons) hang
//! off their parent with [`A11yNodeId::child`], so a row's id depends on
//! its parent widget and its own key, not on its position in the frame.
//!
//! A surface with no `WidgetId` falls back to a key derived from its
//! index in the frame (`surface.3`). That id is stable only while the
//! frame's surface order is; primitives that want a durable id should
//! carry a `WidgetId`.
//!
//! # Bounds
//!
//! Surface bounds come straight from [`crate::frame::OwnedNode::rect`]
//! and are in **logical pixels** — the same coordinate space the frame
//! was laid out in. Sub-node bounds are `None`: a list row's or a
//! toolbar button's rect is only known after a backend measures text, so
//! the builder deliberately reports no rect for them rather than
//! inventing one. A backend that has metrics can fill them in by
//! supplying bounds on the [`A11yChild`]s it builds.
//!
//! # Per-primitive hooks, and the fallback
//!
//! A primitive opts into a real mapping by implementing
//! [`A11yPrimitive`] — role plus state for itself, plus the child nodes
//! it contains. Collections — [`ListView`], [`crate::primitives::tree::TreeView`],
//! [`crate::primitives::data_table::DataTable`], `MultiSectionView`,
//! `MessageList`, `Completions`, `Palette`, `Board`, `PipelineView` (mapped
//! below) — the application chrome — tab bars, activity bars, menus, status
//! bars, dialogs, forms, toasts, tooltips, progress indicators, scrollbars,
//! splits and panels (mapped in [`super::chrome`]) — and the content
//! primitives — editor, text input, text display, terminal, diff view,
//! chart, image, minimap and canvas (mapped in [`super::content`]) — are
//! mapped fully. Every other primitive (currently `Float` and the opaque
//! consumer-owned content kind) goes through a generic fallback: it still
//! appears, as a [`A11yRole::Group`] labelled with its kind ("Floating
//! panel", "Content", …), so an unmapped primitive reads as an unhelpful
//! group rather than vanishing from the tree.
//!
//! Two kinds of node are deliberate rather than accidental:
//! a [`A11yRole::Presentation`] node marks a part with no semantics of
//! its own (a scrollbar thumb, a split divider, a spinner glyph) — it is
//! still in the tree, so a snapshot shows the real structure; and
//! [`A11yLive::Polite`] on a toast overlay makes it a live region an
//! assistive technology announces without taking focus.
//!
//! A consumer can override any of this per node:
//! [`crate::frame::OwnedNode::a11y`] is merged over the primitive's own
//! info field by field, so setting just a label keeps the derived role
//! and state. [`A11yTreeBuilder::with_children`] is the matching override
//! for children: it replaces a widget's derived children wholesale,
//! for shapes — markdown headings and links, in particular — a
//! primitive's own [`A11yPrimitive::a11y_children`] has no way to produce
//! because the data it would need (a heading level, a link target) does
//! not live on the primitive's own descriptor.
//!
//! # TUI emits no tree
//!
//! The TUI backend deliberately publishes **no** accessibility tree. A
//! terminal application is already read by the screen reader attached to
//! the terminal emulator, and a second, parallel tree would make the
//! same content speak twice. The builder is still fully exercised under
//! the TUI feature set: it is backend-independent, always compiled, and
//! the headless snapshot seam runs it in tests. GUI backends (GTK,
//! macOS, Win) are the ones that hand an `accesskit::TreeUpdate` to a
//! platform adapter.
//!
//! # Feature gating
//!
//! The tree model, the per-primitive hooks and the snapshot form are
//! **always compiled** — they are plain data and carry no dependency on
//! accesskit. Only the conversions into `accesskit` types sit behind
//! `feature = "a11y"`.
//!
//! # Virtualisation: off-screen rows report true position/size
//!
//! A collection with far more rows than fit on screen (a 100k-row
//! [`crate::primitives::data_table::DataTable`]) must not materialise an
//! [`A11yChild`] per row — that would mean walking and allocating 100k
//! nodes on every frame just to describe the handful actually visible.
//! [`a11y_window`] caps how many rows around the current scroll offset
//! get a real child node, but every row's `position_in_set`/`set_size`
//! (computed before windowing, from the primitive's true row count) stays
//! correct regardless: row 50,000 of a windowed 100k-row table still
//! reports "row 50,000 of 100,000", not "of 200".

use std::collections::HashMap;

use crate::a11y::{A11yChecked, A11yInfo, A11yLive, A11yRole};
use crate::event::Rect;
use crate::focus::FocusManager;
use crate::frame::{OwnedContent, OwnedFrame, OwnedNode};
use crate::primitives::board::{BoardColumn, BoardModel};
use crate::primitives::completions::Completions;
use crate::primitives::data_table::{DataRow, DataTable, SortDirection};
use crate::primitives::list::ListView;
use crate::primitives::message_list::MessageList;
use crate::primitives::multi_section_view::{MultiSectionView, Section, SectionBody};
use crate::primitives::palette::Palette;
use crate::primitives::pipeline_view::{PipelineStage, PipelineView, StageStatus};
use crate::primitives::toolbar::{Toolbar, ToolbarButton};
use crate::primitives::tree::{TreeRow, TreeView};
use crate::types::{StyledText, WidgetId};

/// How many rows around the scroll offset get a real [`A11yChild`] node.
/// See the module doc's "Virtualisation" section.
const A11Y_WINDOW: usize = 200;

/// The slice of indices `[0, total)` that should get real `A11yChild`
/// nodes, anchored at `scroll_offset` as the first visible row. Callers
/// must still compute `position_in_set`/`set_size` against the true
/// `total`, not the returned range's length — see the module doc.
fn a11y_window(scroll_offset: usize, total: usize) -> std::ops::Range<usize> {
    if total <= A11Y_WINDOW {
        return 0..total;
    }
    let start = scroll_offset.min(total);
    let end = (start + A11Y_WINDOW).min(total);
    start..end
}

/// Default label for the synthetic root node.
const DEFAULT_ROOT_LABEL: &str = "quadraui";

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// FNV-1a over `bytes`, continuing from `seed`.
const fn fnv1a(bytes: &[u8], seed: u64) -> u64 {
    let mut hash = seed;
    let mut i = 0;
    while i < bytes.len() {
        hash ^= bytes[i] as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
        i += 1;
    }
    hash
}

/// Identity of a node in the accessibility tree, stable across frames.
///
/// Derived from a [`WidgetId`]'s interned string rather than from its
/// internal index, so the same widget id always produces the same node
/// id — in this process and the next one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct A11yNodeId(u64);

impl A11yNodeId {
    /// The synthetic root node every frame hangs off.
    pub const ROOT: A11yNodeId = A11yNodeId(fnv1a(b"quadraui.a11y.root", FNV_OFFSET));

    /// Node id for a widget, derived from its interned id string.
    pub fn for_widget(id: &WidgetId) -> Self {
        Self::for_key(id.as_str())
    }

    /// Node id for a surface that carries no [`WidgetId`], derived from
    /// an arbitrary key.
    pub fn for_key(key: &str) -> Self {
        Self(fnv1a(
            key.as_bytes(),
            fnv1a(b"quadraui.a11y.node.", FNV_OFFSET),
        ))
    }

    /// Node id for a sub-node of `self`, identified by `key` within its
    /// parent (e.g. `"item.3"`, `"button.run"`).
    pub fn child(self, key: &str) -> Self {
        Self(fnv1a(
            key.as_bytes(),
            fnv1a(&self.0.to_le_bytes(), FNV_OFFSET),
        ))
    }

    /// The raw identity, for backends and adapters that need a number.
    pub fn as_u64(self) -> u64 {
        self.0
    }

    /// The matching accesskit node id.
    #[cfg(feature = "a11y")]
    #[cfg_attr(docsrs, doc(cfg(feature = "a11y")))]
    pub fn to_accesskit(self) -> accesskit::NodeId {
        accesskit::NodeId(self.0)
    }
}

/// A sub-node a primitive contributes to the tree: a list row, a toolbar
/// button, a tree item.
///
/// `key` identifies the child **within its parent** and must be stable
/// for the same logical child across frames — prefer a widget id's
/// string over a positional index where the primitive has one. The
/// child's node id is `parent.child(&key)`.
#[derive(Debug, Clone, PartialEq)]
pub struct A11yChild {
    /// Stable per-parent key; combined with the parent's id to form the
    /// child's node id.
    pub key: String,
    /// Role, label and state for this child.
    pub info: A11yInfo,
    /// Bounds in logical pixels, when the primitive knows them without
    /// backend text metrics. See the module doc.
    pub bounds: Option<Rect>,
    /// Nested children, if any.
    pub children: Vec<A11yChild>,
}

impl A11yChild {
    /// A leaf child with the given key and info.
    pub fn new(key: impl Into<String>, info: A11yInfo) -> Self {
        Self {
            key: key.into(),
            info,
            bounds: None,
            children: Vec::new(),
        }
    }

    /// Attaches logical-pixel bounds.
    pub fn with_bounds(mut self, bounds: Rect) -> Self {
        self.bounds = Some(bounds);
        self
    }

    /// Attaches nested children.
    pub fn with_children(mut self, children: Vec<A11yChild>) -> Self {
        self.children = children;
        self
    }
}

/// The per-primitive hook: how a primitive describes itself to an
/// assistive technology.
///
/// Implement this on a primitive to give it a real role, state and
/// children. Primitives that don't implement it still appear in the
/// tree, as a labelled [`A11yRole::Group`] — see the module doc.
pub trait A11yPrimitive {
    /// This primitive's own role, accessible name and state.
    fn a11y_info(&self) -> A11yInfo;

    /// The sub-nodes this primitive contains, in reading order.
    /// Defaults to none.
    fn a11y_children(&self) -> Vec<A11yChild> {
        Vec::new()
    }

    /// The key of the child that should hold accessibility focus while
    /// *this* primitive is the focused widget — a listbox points at its
    /// active row, a toolbar at its focused button. `None` keeps focus
    /// on the primitive itself.
    fn a11y_focus_key(&self) -> Option<String> {
        None
    }
}

/// One node of a built accessibility tree.
#[derive(Debug, Clone, PartialEq)]
pub struct A11yTreeNode {
    /// This node's stable identity.
    pub id: A11yNodeId,
    /// Role, accessible name and state.
    pub info: A11yInfo,
    /// Bounds in logical pixels, when known. See the module doc for why
    /// sub-nodes usually have none.
    pub bounds: Option<Rect>,
    /// Child node ids, in reading order.
    pub children: Vec<A11yNodeId>,
}

impl A11yTreeNode {
    /// This node's role, defaulting to [`A11yRole::Group`] when the
    /// primitive supplied none.
    pub fn role(&self) -> A11yRole {
        self.info.a11y_role.unwrap_or(A11yRole::Group)
    }

    /// This node's accessible name, if it has one.
    pub fn label(&self) -> Option<&str> {
        self.info.a11y_label.as_deref()
    }

    /// Which [`crate::a11y::A11yAction`]s this node currently supports —
    /// derived purely from its role and state, so assistive technology
    /// only ever offers an action [`crate::a11y::route_action`] can
    /// actually turn into a `UiEvent`. A disabled node supports none.
    pub fn supported_actions(&self) -> Vec<crate::a11y::A11yAction> {
        use crate::a11y::A11yAction;

        if self.info.disabled == Some(true) {
            return Vec::new();
        }

        // `ScrollIntoView` is deliberately not declared here:
        // `route_action` does not yet translate it into a `UiEvent` (see
        // its doc), and the whole point of this method is to only offer
        // actions `route_action` can actually act on.
        let mut actions = vec![A11yAction::Focus];

        if matches!(
            self.role(),
            A11yRole::Button
                | A11yRole::TreeItem
                | A11yRole::ListItem
                | A11yRole::MenuItem
                | A11yRole::Tab
                | A11yRole::CheckBox
                | A11yRole::RadioButton
                | A11yRole::Row
        ) {
            actions.push(A11yAction::Click);
        }

        match self.info.expanded {
            Some(true) => actions.push(A11yAction::Collapse),
            Some(false) => actions.push(A11yAction::Expand),
            None => {}
        }

        let read_only = self.info.read_only == Some(true);
        // Both value actions below are gated on the role, not merely on a
        // populated value field: `A11yInfo::value` and
        // `A11yInfo::value_range` are this model's generic *readable*
        // value and range, not editable ones. `value` holds a link's URL,
        // a list item's detail text or a dropdown's selected option;
        // `value_range` holds a progress bar's fraction, a scrollbar's
        // position and a splitter's ratio. Declaring an edit affordance on
        // any of those offers assistive technology a control that does not
        // exist, so each action is advertised only for the role
        // `route_action` actually translates it for.
        if matches!(self.role(), A11yRole::TextInput) && !read_only {
            actions.push(A11yAction::SetValue);
        }
        if matches!(self.role(), A11yRole::Slider) && self.info.value_range.is_some() && !read_only
        {
            actions.push(A11yAction::Increment);
            actions.push(A11yAction::Decrement);
        }

        actions
    }

    /// The matching accesskit node.
    ///
    /// ARIA's one-based `level` and `position_in_set` are converted to
    /// accesskit's zero-based equivalents.
    #[cfg(feature = "a11y")]
    #[cfg_attr(docsrs, doc(cfg(feature = "a11y")))]
    pub fn to_accesskit(&self) -> accesskit::Node {
        let mut node = accesskit::Node::new(self.role().into());
        if let Some(label) = self.label() {
            node.set_label(label.to_string());
        }
        if let Some(rect) = self.bounds {
            node.set_bounds(accesskit::Rect::new(
                rect.x as f64,
                rect.y as f64,
                (rect.x + rect.width) as f64,
                (rect.y + rect.height) as f64,
            ));
        }
        if let Some(selected) = self.info.selected {
            node.set_selected(selected);
        }
        if let Some(expanded) = self.info.expanded {
            node.set_expanded(expanded);
        }
        if let Some(checked) = self.info.checked {
            node.set_toggled(checked.into());
        }
        if self.info.disabled == Some(true) {
            node.set_disabled();
        }
        if self.info.read_only == Some(true) {
            node.set_read_only();
        }
        if let Some(value) = &self.info.value {
            node.set_value(value.clone());
        }
        if let Some(range) = self.info.value_range {
            node.set_numeric_value(range.current);
            node.set_min_numeric_value(range.min);
            node.set_max_numeric_value(range.max);
        }
        if let Some(level) = self.info.level {
            node.set_level(level.saturating_sub(1) as usize);
        }
        if let Some(pos) = self.info.position_in_set {
            node.set_position_in_set(pos.saturating_sub(1) as usize);
        }
        if let Some(size) = self.info.set_size {
            node.set_size_of_set(size as usize);
        }
        if let Some(live) = self.info.live {
            node.set_live(live.into());
        }
        if self.info.hidden == Some(true) {
            node.set_hidden();
        }
        if self.info.modal == Some(true) {
            node.set_modal();
        }
        if !self.children.is_empty() {
            node.set_children(
                self.children
                    .iter()
                    .map(|id| id.to_accesskit())
                    .collect::<Vec<_>>(),
            );
        }
        for action in self.supported_actions() {
            node.add_action(action.into());
        }
        node
    }
}

/// A whole accessibility tree for one frame: every node, in document
/// order, root first.
#[derive(Debug, Clone, PartialEq)]
pub struct A11yTree {
    root: A11yNodeId,
    focus: A11yNodeId,
    nodes: Vec<A11yTreeNode>,
    owners: HashMap<A11yNodeId, A11yOwner>,
}

impl A11yTree {
    /// The root node's id.
    pub fn root(&self) -> A11yNodeId {
        self.root
    }

    /// The node that holds accessibility focus. Falls back to the root
    /// when nothing is focused — a tree must always name a focus.
    pub fn focus(&self) -> A11yNodeId {
        self.focus
    }

    /// Every node, in document order, root first.
    pub fn nodes(&self) -> &[A11yTreeNode] {
        &self.nodes
    }

    /// Looks a node up by id.
    pub fn node(&self, id: A11yNodeId) -> Option<&A11yTreeNode> {
        self.nodes.iter().find(|node| node.id == id)
    }

    /// The first node whose accessible name is exactly `label`, in
    /// document order.
    pub fn find(&self, label: &str) -> Option<&A11yTreeNode> {
        self.nodes.iter().find(|node| node.label() == Some(label))
    }

    /// The children of `id`, in reading order. Unknown child ids are
    /// skipped.
    pub fn children_of(&self, id: A11yNodeId) -> Vec<&A11yTreeNode> {
        match self.node(id) {
            Some(parent) => parent
                .children
                .iter()
                .filter_map(|child| self.node(*child))
                .collect(),
            None => Vec::new(),
        }
    }

    /// The focused node, if the tree still contains it.
    pub fn focused(&self) -> Option<&A11yTreeNode> {
        self.node(self.focus)
    }

    /// The widget that produced `id`, and the key identifying it within
    /// that widget (empty for the widget's own top-level node, non-empty
    /// for one of its direct children) — the side-table
    /// [`crate::a11y::route_action`] uses to turn an assistive-technology
    /// action back into the `UiEvent` the owning widget understands. Only
    /// widgets whose content kind supports action routing are present.
    pub(crate) fn owner(&self, id: A11yNodeId) -> Option<(&WidgetId, &str, A11yOwnerKind)> {
        self.owners
            .get(&id)
            .map(|owner| (&owner.widget, owner.key.as_str(), owner.kind))
    }

    /// A stable, assertable text rendering of the whole tree: one line
    /// per node, two spaces of indent per level, `role "label" [state]`.
    /// Both the quoted label and a `value=` token are escaped — `\n`
    /// becomes the two characters `\` `n`, a literal `"` becomes `\"`,
    /// and a literal `\` is doubled — so the node a multi-line editor
    /// buffer, terminal grid or diff pane belongs to still occupies
    /// exactly one line, and a quote inside the text never looks like
    /// the end of it.
    ///
    /// Bounds are deliberately **not** included — they depend on the
    /// frame's layout and would make every snapshot brittle. Assert on
    /// [`Self::nodes`] for geometry.
    pub fn to_snapshot(&self) -> String {
        let mut out = String::new();
        self.write_snapshot(self.root, 0, &mut out);
        out
    }

    fn write_snapshot(&self, id: A11yNodeId, depth: usize, out: &mut String) {
        let Some(node) = self.node(id) else {
            return;
        };
        for _ in 0..depth {
            out.push_str("  ");
        }
        out.push_str(role_name(node.role()));
        if let Some(label) = node.label() {
            out.push_str(" \"");
            out.push_str(&escape_snapshot_text(label));
            out.push('"');
        }
        let states = state_tokens(node, self.focus == node.id);
        if !states.is_empty() {
            out.push_str(" [");
            out.push_str(&states.join(" "));
            out.push(']');
        }
        out.push('\n');
        for child in &node.children {
            self.write_snapshot(*child, depth + 1, out);
        }
    }
}

/// An incremental tree update: the nodes that changed since the last
/// one, plus the focus (always) and the root (first update only).
#[derive(Debug, Clone, PartialEq)]
pub struct A11yUpdate {
    /// New or changed nodes. Unchanged nodes are omitted; a node whose
    /// child list changed is included, which is what lets an adapter
    /// re-parent or drop the children it no longer names.
    pub nodes: Vec<A11yTreeNode>,
    /// The tree root, set only on the first update after a
    /// [`A11yTreeBuilder::reset`] (or on a brand-new builder).
    pub root: Option<A11yNodeId>,
    /// The node holding accessibility focus. Always set — an update must
    /// name a focus even when nothing changed.
    pub focus: A11yNodeId,
    /// Whether this update is a full tree rather than a diff.
    pub full: bool,
}

#[cfg(feature = "a11y")]
#[cfg_attr(docsrs, doc(cfg(feature = "a11y")))]
impl A11yUpdate {
    /// The matching accesskit tree update, ready to hand to a platform
    /// adapter.
    pub fn to_tree_update(&self) -> accesskit::TreeUpdate {
        accesskit::TreeUpdate {
            nodes: self
                .nodes
                .iter()
                .map(|node| (node.id.to_accesskit(), node.to_accesskit()))
                .collect(),
            tree: self
                .root
                .map(|root| accesskit::TreeInfo::new(root.to_accesskit())),
            tree_id: accesskit::TreeId::ROOT,
            focus: self.focus.to_accesskit(),
        }
    }
}

/// Builds accessibility trees from frames, and diffs consecutive frames
/// so only changed nodes are emitted.
///
/// One builder belongs to one surface/window: it remembers the tree it
/// last emitted. [`Self::tree`] is stateless and always returns the full
/// tree; [`Self::update`] is the incremental path.
#[derive(Debug, Clone, Default)]
pub struct A11yTreeBuilder {
    prev: HashMap<A11yNodeId, A11yTreeNode>,
    sent_tree: bool,
    root_label: Option<String>,
    modal_top: Option<WidgetId>,
    children_overrides: HashMap<WidgetId, Vec<A11yChild>>,
}

impl A11yTreeBuilder {
    /// A builder that has emitted nothing yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the accessible name of the synthetic root node (typically
    /// the window title). Defaults to `"quadraui"`.
    pub fn with_root_label(mut self, label: impl Into<String>) -> Self {
        self.root_label = Some(label.into());
        self
    }

    /// Replaces the children a primitive would otherwise derive for
    /// `widget`, so an app can substitute a richer structure than the
    /// primitive's own [`A11yPrimitive::a11y_children`] without a bespoke
    /// descriptor field. [`super::content::markdown_a11y_children`] is the
    /// motivating case: a `TextDisplay` rendering
    /// [`crate::compose::markdown::RenderedMarkdown`] has no slot of its
    /// own for heading levels or link targets, so an app that rendered the
    /// markdown attaches that function's output here, keyed by the
    /// `TextDisplay`'s own [`WidgetId`], and the builder uses it in place of
    /// the default line-flattened children on every subsequent [`Self::tree`]
    /// / [`Self::update`] call.
    ///
    /// Calling this again for the same `widget` replaces the previous
    /// override; there is no way to clear one short of a new builder.
    pub fn with_children(mut self, widget: WidgetId, children: Vec<A11yChild>) -> Self {
        self.children_overrides.insert(widget, children);
        self
    }

    /// Names the widget on top of the backend's
    /// [`ModalStack`](crate::ModalStack) — the authoritative source of
    /// modality. While it is set and the matching surface is present,
    /// that surface is marked modal and every other top-level surface is
    /// hidden from assistive technology. `None` falls back to treating
    /// the topmost dialog surface in the frame as the modal one.
    pub fn with_modal_top(mut self, modal_top: impl Into<Option<WidgetId>>) -> Self {
        self.modal_top = modal_top.into();
        self
    }

    /// Updates the modal top between frames — the stateful form of
    /// [`Self::with_modal_top`].
    pub fn set_modal_top(&mut self, modal_top: impl Into<Option<WidgetId>>) {
        self.modal_top = modal_top.into();
    }

    /// The full tree for this frame. Does not touch the diff state, so
    /// it can be called freely from tests and debug tooling.
    pub fn tree(&self, frame: &OwnedFrame, focus: &FocusManager) -> A11yTree {
        build_tree(
            frame,
            focus,
            self.root_label.as_deref().unwrap_or(DEFAULT_ROOT_LABEL),
            self.modal_top.as_ref(),
            &self.children_overrides,
        )
    }

    /// The next update for this frame: the whole tree the first time,
    /// only the changed nodes afterwards.
    pub fn update(&mut self, frame: &OwnedFrame, focus: &FocusManager) -> A11yUpdate {
        let tree = self.tree(frame, focus);
        let full = !self.sent_tree;
        let nodes: Vec<A11yTreeNode> = if full {
            tree.nodes().to_vec()
        } else {
            tree.nodes()
                .iter()
                .filter(|node| self.prev.get(&node.id) != Some(*node))
                .cloned()
                .collect()
        };
        self.prev = tree
            .nodes()
            .iter()
            .map(|node| (node.id, node.clone()))
            .collect();
        self.sent_tree = true;
        A11yUpdate {
            nodes,
            root: if full { Some(tree.root()) } else { None },
            focus: tree.focus(),
            full,
        }
    }

    /// Forgets the emitted tree, so the next [`Self::update`] is a full
    /// one again. Call this when the platform adapter is recreated.
    pub fn reset(&mut self) {
        self.prev.clear();
        self.sent_tree = false;
    }

    /// The next update as an `accesskit::TreeUpdate`.
    #[cfg(feature = "a11y")]
    #[cfg_attr(docsrs, doc(cfg(feature = "a11y")))]
    pub fn tree_update(
        &mut self,
        frame: &OwnedFrame,
        focus: &FocusManager,
    ) -> accesskit::TreeUpdate {
        self.update(frame, focus).to_tree_update()
    }
}

/// The full accessibility tree for one frame, with the default root
/// label — the stateless convenience form of [`A11yTreeBuilder::tree`].
pub fn frame_to_tree(frame: &OwnedFrame, focus: &FocusManager) -> A11yTree {
    build_tree(frame, focus, DEFAULT_ROOT_LABEL, None, &HashMap::new())
}

/// The full accessibility tree for one frame, with the widget on top of
/// the backend's [`ModalStack`](crate::ModalStack) named explicitly.
///
/// The modal stack is held by the backend rather than captured in the
/// frame, so a caller that has it passes the top entry's id here: that
/// surface is marked modal and every other top-level surface is hidden
/// from assistive technology, so the AT cursor cannot escape the dialog.
/// `None` is identical to [`frame_to_tree`], which falls back to the
/// topmost dialog surface in the frame.
pub fn frame_to_tree_with_modal(
    frame: &OwnedFrame,
    focus: &FocusManager,
    modal_top: Option<&WidgetId>,
) -> A11yTree {
    build_tree(frame, focus, DEFAULT_ROOT_LABEL, modal_top, &HashMap::new())
}

/// The identity a modal stack entry is matched against. Overlay
/// surfaces are captured without a hit-test widget id, so the
/// descriptor's own id is the only thing a modal stack can name.
fn modal_identity(node: &OwnedNode) -> Option<WidgetId> {
    node.widget_id.or(match &node.content {
        OwnedContent::Dialog(dialog) => Some(dialog.id),
        OwnedContent::ContextMenu(menu) => Some(menu.id),
        _ => None,
    })
}

/// Which top-level surface, if any, holds a modal dialog: the one named
/// by the modal stack, else the topmost dialog painted this frame.
fn modal_surface(frame: &OwnedFrame, modal_top: Option<&WidgetId>) -> Option<usize> {
    match modal_top {
        Some(id) => frame
            .nodes()
            .iter()
            .position(|node| modal_identity(node).as_ref() == Some(id)),
        None => frame
            .nodes()
            .iter()
            .rposition(|node| matches!(node.content, OwnedContent::Dialog(_))),
    }
}

fn build_tree(
    frame: &OwnedFrame,
    focus: &FocusManager,
    root_label: &str,
    modal_top: Option<&WidgetId>,
    children_overrides: &HashMap<WidgetId, Vec<A11yChild>>,
) -> A11yTree {
    let modal_idx = modal_surface(frame, modal_top);
    let mut nodes: Vec<A11yTreeNode> = Vec::with_capacity(frame.nodes().len() + 1);
    let mut root_children: Vec<A11yNodeId> = Vec::new();
    let mut root_bounds: Option<Rect> = None;
    let mut focus_id = A11yNodeId::ROOT;
    let mut owners: HashMap<A11yNodeId, A11yOwner> = HashMap::new();

    // Reserve the root's slot; its children and bounds are only known
    // once every surface has been walked.
    nodes.push(A11yTreeNode {
        id: A11yNodeId::ROOT,
        info: A11yInfo::new()
            .with_role(A11yRole::Window)
            .with_label(root_label),
        bounds: None,
        children: Vec::new(),
    });

    for (pos, owned) in frame.nodes().iter().enumerate() {
        let id = match &owned.widget_id {
            Some(widget) => A11yNodeId::for_widget(widget),
            None => A11yNodeId::for_key(&format!("surface.{}", owned.idx)),
        };
        let described = describe(&owned.content);
        let mut info = match &owned.a11y {
            Some(override_info) => merge(described.info, override_info),
            None => described.info,
        };
        // An explicit children override (keyed by widget id, set via
        // `A11yTreeBuilder::with_children`) replaces the primitive's own
        // derived children wholesale — there is no per-child merge, since
        // the override exists precisely for shapes (markdown headings and
        // links) the primitive cannot derive at all.
        let children: &[A11yChild] = match &owned.widget_id {
            Some(widget) => children_overrides
                .get(widget)
                .map(Vec::as_slice)
                .unwrap_or(&described.children),
            None => &described.children,
        };
        // An explicit override still wins: modality is derived state,
        // and a consumer that asserted it knows better.
        if let Some(modal_idx) = modal_idx {
            if pos == modal_idx {
                info.modal.get_or_insert(true);
            } else {
                info.hidden.get_or_insert(true);
            }
        }
        let child_ids: Vec<A11yNodeId> =
            children.iter().map(|child| id.child(&child.key)).collect();

        if focus.focused() == owned.widget_id.as_ref() && owned.widget_id.is_some() {
            focus_id = match &described.focus_key {
                Some(key) if child_ids.contains(&id.child(key)) => id.child(key),
                _ => id,
            };
        }

        root_children.push(id);
        root_bounds = Some(match root_bounds {
            Some(acc) => union(acc, owned.rect),
            None => owned.rect,
        });
        nodes.push(A11yTreeNode {
            id,
            info,
            bounds: Some(owned.rect),
            children: child_ids,
        });
        for child in children {
            push_child(&mut nodes, id, child);
        }

        // Every top-level widget gets an owner-table entry so actions
        // that only need a `WidgetId` (`Focus`, `ScrollUp`, `ScrollDown`)
        // can resolve against any widget, not just the richly-mapped ones.
        // Child-node entries stay scoped to kinds `route_action` knows
        // how to translate into a child-level event (list item, tree
        // row, form field); every other primitive's children are left
        // out so routing against them is a clean "unsupported" lookup
        // miss rather than a wrong guess.
        let kind = content_route_kind(&owned.content);
        if let Some(widget) = &owned.widget_id {
            owners.insert(
                id,
                A11yOwner {
                    widget: *widget,
                    key: String::new(),
                    kind,
                },
            );
            if kind != A11yOwnerKind::Unsupported {
                for child in &described.children {
                    owners.insert(
                        id.child(&child.key),
                        A11yOwner {
                            widget: *widget,
                            key: child.key.clone(),
                            kind,
                        },
                    );
                }
            }
        }
    }

    nodes[0].children = root_children;
    nodes[0].bounds = root_bounds;

    A11yTree {
        root: A11yNodeId::ROOT,
        focus: focus_id,
        nodes,
        owners,
    }
}

fn push_child(nodes: &mut Vec<A11yTreeNode>, parent: A11yNodeId, child: &A11yChild) {
    let id = parent.child(&child.key);
    let grandchildren: Vec<A11yNodeId> = child
        .children
        .iter()
        .map(|grandchild| id.child(&grandchild.key))
        .collect();
    nodes.push(A11yTreeNode {
        id,
        info: child.info.clone(),
        bounds: child.bounds,
        children: grandchildren,
    });
    for grandchild in &child.children {
        push_child(nodes, id, grandchild);
    }
}

fn union(a: Rect, b: Rect) -> Rect {
    let x = a.x.min(b.x);
    let y = a.y.min(b.y);
    let right = (a.x + a.width).max(b.x + b.width);
    let bottom = (a.y + a.height).max(b.y + b.height);
    Rect::new(x, y, right - x, bottom - y)
}

/// Which family of [`crate::event::UiEvent`] a widget's content kind
/// routes an assistive-technology action onto. Only kinds
/// [`crate::a11y::route_action`] knows how to translate are named here;
/// every other primitive is [`Self::Unsupported`] and is left out of the
/// owner side-table entirely.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum A11yOwnerKind {
    List,
    Tree,
    Form,
    Unsupported,
}

/// Which [`A11yOwnerKind`] a node's content maps onto, for owner
/// side-table population in [`build_tree`].
fn content_route_kind(content: &OwnedContent) -> A11yOwnerKind {
    match content {
        OwnedContent::List(_) => A11yOwnerKind::List,
        OwnedContent::Tree(_) => A11yOwnerKind::Tree,
        OwnedContent::Form(_) => A11yOwnerKind::Form,
        _ => A11yOwnerKind::Unsupported,
    }
}

/// One entry in [`A11yTree`]'s owner side-table: which widget produced a
/// node, and the key identifying it within that widget's own
/// [`A11yPrimitive::a11y_children`] (empty for the widget's own
/// top-level node).
#[derive(Debug, Clone, PartialEq)]
struct A11yOwner {
    widget: WidgetId,
    key: String,
    kind: A11yOwnerKind,
}

/// Encodes a [`crate::primitives::tree::TreePath`] as a dot-joined
/// decimal string, so it survives as an [`A11yChild`] key and round-trips
/// back through [`decode_tree_path`] when an assistive-technology action
/// targets a tree row — the row's position in [`crate::primitives::tree::TreeView::rows`]
/// is not itself stable enough to recover, since only the frozen
/// [`A11yTree`] is available at routing time.
pub(crate) fn encode_tree_path(path: &[u16]) -> String {
    path.iter()
        .map(|segment| segment.to_string())
        .collect::<Vec<_>>()
        .join(".")
}

/// The inverse of [`encode_tree_path`]. `None` when `encoded` is not a
/// well-formed dot-joined path.
pub(crate) fn decode_tree_path(encoded: &str) -> Option<Vec<u16>> {
    encoded.split('.').map(|part| part.parse().ok()).collect()
}

/// What a primitive contributes to the tree.
struct Described {
    info: A11yInfo,
    children: Vec<A11yChild>,
    focus_key: Option<String>,
}

impl Described {
    fn from_primitive(primitive: &dyn A11yPrimitive) -> Self {
        Self {
            info: primitive.a11y_info(),
            children: primitive.a11y_children(),
            focus_key: primitive.a11y_focus_key(),
        }
    }

    /// The generic fallback: a group labelled with the primitive's kind,
    /// so an unmapped primitive still reads as *something*.
    fn fallback(content: &OwnedContent) -> Self {
        Self {
            info: A11yInfo::new()
                .with_role(A11yRole::Group)
                .with_label(content_kind(content)),
            children: Vec::new(),
            focus_key: None,
        }
    }

    /// A surface whose content is not captured in the frame: it still gets
    /// a real role and name, just no children.
    fn labelled(role: A11yRole, label: &str) -> Self {
        Self {
            info: A11yInfo::new().with_role(role).with_label(label),
            children: Vec::new(),
            focus_key: None,
        }
    }
}

fn describe(content: &OwnedContent) -> Described {
    match content {
        OwnedContent::List(list) => Described::from_primitive(list),
        OwnedContent::Toolbar(bar) => Described::from_primitive(bar),
        OwnedContent::TabBar(bar) => Described::from_primitive(bar),
        OwnedContent::ActivityBar(bar) => Described::from_primitive(bar),
        OwnedContent::MenuBar(bar) => Described::from_primitive(bar),
        OwnedContent::ContextMenu(menu) => Described::from_primitive(menu),
        OwnedContent::StatusBar(bar) => Described::from_primitive(bar),
        OwnedContent::CommandCenter(cc) => Described::from_primitive(cc),
        OwnedContent::CommandLine(cmd) => Described::from_primitive(cmd),
        OwnedContent::Dialog(dialog) => Described::from_primitive(dialog),
        OwnedContent::Form(form) => Described::from_primitive(form),
        OwnedContent::Toast(stack) => Described::from_primitive(stack),
        OwnedContent::Tooltip(tooltip) => Described::from_primitive(tooltip),
        OwnedContent::RichTextPopup(popup) => Described::from_primitive(popup),
        OwnedContent::Progress(bar) => Described::from_primitive(bar),
        OwnedContent::Spinner(spinner) => Described::from_primitive(spinner),
        OwnedContent::Scrollbar(sb) => Described::from_primitive(sb),
        OwnedContent::Split(split) => Described::from_primitive(split),
        OwnedContent::SplitTree(tree) => Described::from_primitive(tree),
        OwnedContent::Panel(panel) => Described::from_primitive(panel),
        OwnedContent::SidebarPanel(panel) => Described::from_primitive(panel),
        OwnedContent::Editor(editor) => Described::from_primitive(editor),
        OwnedContent::TextInput(ti) => Described::from_primitive(ti),
        OwnedContent::TextDisplay(td) => Described::from_primitive(td),
        OwnedContent::Terminal(term) => Described::from_primitive(term),
        OwnedContent::DiffView(view) => Described::from_primitive(view),
        OwnedContent::Chart(chart) => Described::from_primitive(chart),
        OwnedContent::Image(image) => Described::from_primitive(image),
        OwnedContent::Minimap(minimap) => Described::from_primitive(minimap),
        OwnedContent::Canvas(canvas) => Described::from_primitive(canvas),
        OwnedContent::Tree(tree) => Described::from_primitive(tree),
        OwnedContent::DataTable(table) => Described::from_primitive(table),
        OwnedContent::MultiSectionView(view) => Described::from_primitive(view),
        OwnedContent::Palette(palette) => Described::from_primitive(palette),
        OwnedContent::Completions(completions) => Described::from_primitive(completions),
        OwnedContent::Board(model) => Described::from_primitive(model),
        OwnedContent::MessageList(list) => Described::from_primitive(list),
        OwnedContent::PipelineView(view) => Described::from_primitive(view),
        // Unit variants: the source surface carries no serializable
        // content, so only the role and name survive the frame capture.
        OwnedContent::FindReplace => Described::labelled(A11yRole::Form, "Find and replace"),
        OwnedContent::DropOverlay => Described::labelled(A11yRole::Presentation, "Drop overlay"),
        other => Described::fallback(other),
    }
}

/// Field-by-field override: anything the consumer set on
/// [`crate::frame::OwnedNode::a11y`] wins, everything else keeps the
/// primitive's derived value.
fn merge(base: A11yInfo, over: &A11yInfo) -> A11yInfo {
    let mut merged = base;
    merged.a11y_role = over.a11y_role.or(merged.a11y_role);
    merged.a11y_label = over.a11y_label.clone().or(merged.a11y_label);
    merged.selected = over.selected.or(merged.selected);
    merged.expanded = over.expanded.or(merged.expanded);
    merged.checked = over.checked.or(merged.checked);
    merged.disabled = over.disabled.or(merged.disabled);
    merged.read_only = over.read_only.or(merged.read_only);
    merged.value = over.value.clone().or(merged.value);
    merged.value_range = over.value_range.or(merged.value_range);
    merged.level = over.level.or(merged.level);
    merged.position_in_set = over.position_in_set.or(merged.position_in_set);
    merged.set_size = over.set_size.or(merged.set_size);
    merged.live = over.live.or(merged.live);
    merged.hidden = over.hidden.or(merged.hidden);
    merged.modal = over.modal.or(merged.modal);
    merged
}

/// Human-readable kind name for the generic fallback label.
fn content_kind(content: &OwnedContent) -> &'static str {
    match content {
        OwnedContent::Editor(_) => "Editor",
        OwnedContent::TabBar(_) => "Tab bar",
        OwnedContent::StatusBar(_) => "Status bar",
        OwnedContent::ActivityBar(_) => "Activity bar",
        OwnedContent::CommandLine(_) => "Command line",
        OwnedContent::Terminal(_) => "Terminal",
        OwnedContent::TextDisplay(_) => "Text",
        OwnedContent::MultiSectionView(_) => "Multi-section view",
        OwnedContent::Tree(_) => "Tree",
        OwnedContent::List(_) => "List",
        OwnedContent::Form(_) => "Form",
        OwnedContent::MenuBar(_) => "Menu bar",
        OwnedContent::Split(_) => "Split",
        OwnedContent::Panel(_) => "Panel",
        OwnedContent::Scrollbar(_) => "Scrollbar",
        OwnedContent::Palette(_) => "Palette",
        OwnedContent::Tooltip(_) => "Tooltip",
        OwnedContent::ContextMenu(_) => "Context menu",
        OwnedContent::Dialog(_) => "Dialog",
        OwnedContent::Float(_) => "Floating panel",
        OwnedContent::Completions(_) => "Completions",
        OwnedContent::RichTextPopup(_) => "Rich text popup",
        OwnedContent::Toast(_) => "Notifications",
        OwnedContent::DataTable(_) => "Table",
        OwnedContent::Chart(_) => "Chart",
        OwnedContent::Canvas(_) => "Canvas",
        OwnedContent::Board(_) => "Board",
        OwnedContent::CommandCenter(_) => "Command center",
        OwnedContent::DiffView(_) => "Diff view",
        OwnedContent::Image(_) => "Image",
        OwnedContent::MessageList(_) => "Message list",
        OwnedContent::Minimap(_) => "Minimap",
        OwnedContent::PipelineView(_) => "Pipeline view",
        OwnedContent::Progress(_) => "Progress",
        OwnedContent::SidebarPanel(_) => "Sidebar panel",
        OwnedContent::Spinner(_) => "Spinner",
        OwnedContent::SplitTree(_) => "Split tree",
        OwnedContent::TextInput(_) => "Text input",
        OwnedContent::Toolbar(_) => "Toolbar",
        OwnedContent::DropOverlay => "Drop overlay",
        OwnedContent::FindReplace => "Find and replace",
        OwnedContent::Opaque => "Content",
    }
}

/// Snake-case name of a role, as it appears in a snapshot line.
fn role_name(role: A11yRole) -> &'static str {
    match role {
        A11yRole::Window => "window",
        A11yRole::Toolbar => "toolbar",
        A11yRole::Button => "button",
        A11yRole::Tab => "tab",
        A11yRole::TabList => "tab_list",
        A11yRole::Menu => "menu",
        A11yRole::MenuItem => "menu_item",
        A11yRole::MenuBar => "menu_bar",
        A11yRole::Tree => "tree",
        A11yRole::TreeItem => "tree_item",
        A11yRole::List => "list",
        A11yRole::ListItem => "list_item",
        A11yRole::Table => "table",
        A11yRole::Row => "row",
        A11yRole::Cell => "cell",
        A11yRole::TextInput => "text_input",
        A11yRole::CheckBox => "check_box",
        A11yRole::RadioGroup => "radio_group",
        A11yRole::RadioButton => "radio_button",
        A11yRole::Slider => "slider",
        A11yRole::ComboBox => "combo_box",
        A11yRole::Label => "label",
        A11yRole::Form => "form",
        A11yRole::Document => "document",
        A11yRole::Editor => "editor",
        A11yRole::Terminal => "terminal",
        A11yRole::Dialog => "dialog",
        A11yRole::Status => "status",
        A11yRole::Alert => "alert",
        A11yRole::Tooltip => "tooltip",
        A11yRole::ScrollBar => "scroll_bar",
        A11yRole::Splitter => "splitter",
        A11yRole::Progress => "progress",
        A11yRole::Image => "image",
        A11yRole::Chart => "chart",
        A11yRole::Canvas => "canvas",
        A11yRole::Group => "group",
        A11yRole::Presentation => "presentation",
        A11yRole::Heading => "heading",
        A11yRole::Link => "link",
    }
}

/// The state tokens for a snapshot line, in a fixed order so snapshots
/// are stable.
fn state_tokens(node: &A11yTreeNode, focused: bool) -> Vec<String> {
    let info = &node.info;
    let mut tokens = Vec::new();
    if focused {
        tokens.push("focused".to_string());
    }
    if info.selected == Some(true) {
        tokens.push("selected".to_string());
    }
    match info.expanded {
        Some(true) => tokens.push("expanded".to_string()),
        Some(false) => tokens.push("collapsed".to_string()),
        None => {}
    }
    match info.checked {
        Some(A11yChecked::True) => tokens.push("checked".to_string()),
        Some(A11yChecked::False) => tokens.push("unchecked".to_string()),
        Some(A11yChecked::Mixed) => tokens.push("mixed".to_string()),
        None => {}
    }
    if info.disabled == Some(true) {
        tokens.push("disabled".to_string());
    }
    if info.read_only == Some(true) {
        tokens.push("readonly".to_string());
    }
    if let Some(value) = &info.value {
        tokens.push(format!("value=\"{}\"", escape_snapshot_text(value)));
    }
    if let Some(range) = info.value_range {
        tokens.push(format!(
            "range={}/{}-{}",
            range.current, range.min, range.max
        ));
    }
    if let Some(level) = info.level {
        tokens.push(format!("level={level}"));
    }
    match (info.position_in_set, info.set_size) {
        (Some(pos), Some(size)) => tokens.push(format!("pos={pos}/{size}")),
        (Some(pos), None) => tokens.push(format!("pos={pos}")),
        (None, Some(size)) => tokens.push(format!("size={size}")),
        (None, None) => {}
    }
    match info.live {
        Some(A11yLive::Polite) => tokens.push("live=polite".to_string()),
        Some(A11yLive::Assertive) => tokens.push("live=assertive".to_string()),
        Some(A11yLive::Off) | None => {}
    }
    if info.modal == Some(true) {
        tokens.push("modal".to_string());
    }
    if info.hidden == Some(true) {
        tokens.push("hidden".to_string());
    }
    tokens
}

/// Escapes a quoted snapshot string — a node's label or its `value=`
/// token — so the line stays unambiguous: embedded newlines become `\n`
/// (keeping a multi-line editor buffer, terminal grid or diff pane on
/// the single line its node occupies), a literal `"` becomes `\"` so it
/// cannot be read as the closing delimiter, and a literal `\` is doubled
/// so neither escape is ambiguous. [`A11yTree::to_snapshot`] documents
/// one line per node; this is what keeps that true.
fn escape_snapshot_text(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('\n', "\\n")
        .replace('"', "\\\"")
}

/// Plain text of a styled run, for use as an accessible name.
pub(super) fn plain_text(text: &StyledText) -> String {
    text.spans.iter().map(|span| span.text.as_str()).collect()
}

/// `one of N` positioning, 1-based as ARIA defines it.
pub(super) fn in_set(info: A11yInfo, idx: usize, total: usize) -> A11yInfo {
    info.with_position_in_set(idx as u32 + 1)
        .with_set_size(total as u32)
}

pub(super) fn non_empty(text: String) -> Option<String> {
    if text.trim().is_empty() {
        None
    } else {
        Some(text)
    }
}

impl A11yPrimitive for ListView {
    fn a11y_info(&self) -> A11yInfo {
        let mut info = A11yInfo::new()
            .with_role(A11yRole::List)
            .with_set_size(self.items.len() as u32);
        if let Some(label) = self.title.as_ref().and_then(|t| non_empty(plain_text(t))) {
            info = info.with_label(label);
        }
        info
    }

    fn a11y_children(&self) -> Vec<A11yChild> {
        let set_size = self.items.len() as u32;
        a11y_window(self.scroll_offset, self.items.len())
            .map(|idx| {
                let item = &self.items[idx];
                let mut info = A11yInfo::new()
                    .with_role(A11yRole::ListItem)
                    .with_selected(idx == self.selected_idx)
                    .with_position_in_set(idx as u32 + 1)
                    .with_set_size(set_size);
                if let Some(label) = non_empty(plain_text(&item.text)) {
                    info = info.with_label(label);
                }
                if let Some(detail) = item.detail.as_ref().and_then(|d| non_empty(plain_text(d))) {
                    info = info.with_value(detail);
                }
                A11yChild::new(format!("item.{idx}"), info)
            })
            .collect()
    }

    fn a11y_focus_key(&self) -> Option<String> {
        if self.selected_idx < self.items.len() {
            Some(format!("item.{}", self.selected_idx))
        } else {
            None
        }
    }
}

impl A11yPrimitive for Toolbar {
    fn a11y_info(&self) -> A11yInfo {
        A11yInfo::new().with_role(A11yRole::Toolbar)
    }

    fn a11y_children(&self) -> Vec<A11yChild> {
        self.buttons
            .iter()
            .enumerate()
            .map(|(idx, button)| toolbar_child(idx, button))
            .collect()
    }

    fn a11y_focus_key(&self) -> Option<String> {
        let idx = self.focused_index?;
        let button = self.buttons.get(idx)?;
        Some(toolbar_child_key(idx, button))
    }
}

/// Stable per-toolbar key for a button: its widget id where it has one,
/// so reordering the bar doesn't rename it.
fn toolbar_child_key(idx: usize, button: &ToolbarButton) -> String {
    match button {
        ToolbarButton::Action { id, .. } => format!("button.{}", id.as_str()),
        ToolbarButton::Separator => format!("separator.{idx}"),
        ToolbarButton::Label { .. } => format!("label.{idx}"),
    }
}

fn toolbar_child(idx: usize, button: &ToolbarButton) -> A11yChild {
    let key = toolbar_child_key(idx, button);
    let info = match button {
        ToolbarButton::Action {
            label,
            enabled,
            is_active,
            tooltip,
            ..
        } => {
            let mut info = A11yInfo::new()
                .with_role(A11yRole::Button)
                .with_disabled(!*enabled);
            if let Some(label) = non_empty(label.clone()) {
                info = info.with_label(label);
            }
            if *is_active {
                info = info.with_checked(A11yChecked::True);
            }
            if let Some(tooltip) = non_empty(tooltip.clone()) {
                info = info.with_value(tooltip);
            }
            info
        }
        // A separator is pure visual grouping: an assistive technology
        // should step straight over it.
        ToolbarButton::Separator => A11yInfo::new().with_role(A11yRole::Presentation),
        // Static text in a toolbar has no interactive role; it reads as
        // a labelled group so its text is still announced.
        ToolbarButton::Label { text, .. } => {
            let mut info = A11yInfo::new().with_role(A11yRole::Group);
            if let Some(label) = non_empty(text.clone()) {
                info = info.with_label(label);
            }
            info
        }
    };
    A11yChild::new(key, info)
}

impl A11yPrimitive for DataTable {
    fn a11y_info(&self) -> A11yInfo {
        A11yInfo::new()
            .with_role(A11yRole::Table)
            .with_label("Table")
            .with_set_size(self.rows.len() as u32)
    }

    fn a11y_children(&self) -> Vec<A11yChild> {
        let total = self.rows.len();
        let mut children: Vec<A11yChild> = vec![data_table_header(self)];
        children.extend(
            a11y_window(self.scroll_offset, total).map(|idx| data_table_row(self, idx, total)),
        );
        if let Some(footer) = &self.footer {
            children.push(data_table_footer(footer));
        }
        children
    }

    fn a11y_focus_key(&self) -> Option<String> {
        self.selected_idx.map(|idx| format!("row.{idx}"))
    }
}

/// The column-header row: one `Cell` per [`Column`], carrying its title
/// and, when that column is the current sort column, a value describing
/// the direction — the same pair of signals `chrome.rs`'s `dialog_table`
/// gives a screen reader in table-navigation mode.
fn data_table_header(table: &DataTable) -> A11yChild {
    let total = table.columns.len();
    let cells = table
        .columns
        .iter()
        .enumerate()
        .map(|(col, column)| {
            let mut info = in_set(A11yInfo::new().with_role(A11yRole::Cell), col, total);
            if let Some(label) = non_empty(column.title.clone()) {
                info = info.with_label(label);
            }
            if let Some((sort_col, direction)) = table.sort {
                if sort_col == col {
                    info = info.with_value(match direction {
                        SortDirection::Ascending => "sorted ascending",
                        SortDirection::Descending => "sorted descending",
                    });
                }
            }
            A11yChild::new(format!("cell.{col}"), info)
        })
        .collect();
    A11yChild::new(
        "header".to_string(),
        A11yInfo::new()
            .with_role(A11yRole::Row)
            .with_label("Header"),
    )
    .with_children(cells)
}

fn row_cells(cells: &[StyledText]) -> Vec<A11yChild> {
    let total = cells.len();
    cells
        .iter()
        .enumerate()
        .map(|(col, text)| {
            let mut cell_info = in_set(A11yInfo::new().with_role(A11yRole::Cell), col, total);
            if let Some(label) = non_empty(plain_text(text)) {
                cell_info = cell_info.with_label(label);
            }
            A11yChild::new(format!("cell.{col}"), cell_info)
        })
        .collect()
}

fn data_table_row(table: &DataTable, idx: usize, total: usize) -> A11yChild {
    let row = &table.rows[idx];
    let info = in_set(
        A11yInfo::new()
            .with_role(A11yRole::Row)
            .with_selected(table.selected_idx == Some(idx)),
        idx,
        total,
    );
    A11yChild::new(format!("row.{idx}"), info).with_children(row_cells(&row.cells))
}

fn data_table_footer(footer: &DataRow) -> A11yChild {
    let info = A11yInfo::new().with_role(A11yRole::Row);
    A11yChild::new("footer".to_string(), info).with_children(row_cells(&footer.cells))
}

/// The path of a tree row's parent — everything but its own last
/// segment. The root's own parent path is empty.
fn parent_path(path: &[u16]) -> &[u16] {
    &path[..path.len().saturating_sub(1)]
}

/// `(position_in_set, set_size)` for every row in `rows`, grouped by
/// sibling (same parent path). Computed over the full, unwindowed row
/// list — see the module doc's "Virtualisation" section for why this
/// pass must not be limited to the visible window: siblings interleave
/// with other subtrees' descendants in the flattened row list, so a
/// window's start is not necessarily a sibling group's true start.
///
/// This is `O(rows)` on every call, unlike [`DataTable`]'s windowing,
/// which only ever touches the windowed slice: a tree's sibling groups
/// can only be found by scanning every row's parent path, since a
/// window's boundary can fall in the middle of a subtree. A tree deep
/// enough for this to matter in practice can revisit this with an
/// incrementally maintained index instead of a full rescan per frame.
fn tree_row_positions(rows: &[TreeRow]) -> Vec<(u32, u32)> {
    let mut sizes: HashMap<&[u16], u32> = HashMap::new();
    for row in rows {
        *sizes.entry(parent_path(&row.path)).or_insert(0) += 1;
    }
    let mut seen: HashMap<&[u16], u32> = HashMap::new();
    rows.iter()
        .map(|row| {
            let parent = parent_path(&row.path);
            let size = sizes[parent];
            let pos = {
                let entry = seen.entry(parent).or_insert(0);
                *entry += 1;
                *entry
            };
            (pos, size)
        })
        .collect()
}

impl A11yPrimitive for TreeView {
    fn a11y_info(&self) -> A11yInfo {
        // `set_size` on the container describes the root's own sibling
        // set — the count a user can actually traverse at the top
        // level — not every flattened row in the tree (which includes
        // descendants nested arbitrarily deep).
        let root_count = self.rows.iter().filter(|row| row.path.len() == 1).count() as u32;
        A11yInfo::new()
            .with_role(A11yRole::Tree)
            .with_set_size(root_count)
    }

    fn a11y_children(&self) -> Vec<A11yChild> {
        let positions = tree_row_positions(&self.rows);
        a11y_window(self.scroll_offset, self.rows.len())
            .map(|idx| {
                let (pos, size) = positions[idx];
                tree_row_child(self, idx, pos, size)
            })
            .collect()
    }

    fn a11y_focus_key(&self) -> Option<String> {
        let selected = self.selected_path.as_ref()?;
        Some(format!("row.{}", encode_tree_path(selected)))
    }
}

fn tree_row_child(tree: &TreeView, idx: usize, pos: u32, size: u32) -> A11yChild {
    let row = &tree.rows[idx];
    let mut info = A11yInfo::new()
        .with_role(A11yRole::TreeItem)
        .with_level(row.indent as u32 + 1)
        .with_position_in_set(pos)
        .with_set_size(size)
        .with_selected(tree.selected_path.as_deref() == Some(row.path.as_slice()));
    if let Some(expanded) = row.is_expanded {
        info = info.with_expanded(expanded);
    }
    if let Some(label) = non_empty(plain_text(&row.text)) {
        info = info.with_label(label);
    }
    // Encoded as the row's full path, not its flattened index — the
    // index shifts as siblings expand/collapse, but the path is what
    // `route_action` can recover a `TreePath` from.
    A11yChild::new(format!("row.{}", encode_tree_path(&row.path)), info)
}

impl A11yPrimitive for MessageList {
    fn a11y_info(&self) -> A11yInfo {
        A11yInfo::new()
            .with_role(A11yRole::List)
            .with_label("Message list")
            .with_set_size(self.rows.len() as u32)
    }

    fn a11y_children(&self) -> Vec<A11yChild> {
        let total = self.rows.len();
        a11y_window(self.scroll_top, total)
            .map(|idx| {
                let row = &self.rows[idx];
                let mut info = in_set(A11yInfo::new().with_role(A11yRole::ListItem), idx, total);
                if let Some(label) = non_empty(row.text.clone()) {
                    info = info.with_label(label);
                }
                A11yChild::new(format!("row.{idx}"), info)
            })
            .collect()
    }
}

impl A11yPrimitive for Completions {
    fn a11y_info(&self) -> A11yInfo {
        A11yInfo::new()
            .with_role(A11yRole::List)
            .with_label("Completions")
            .with_set_size(self.items.len() as u32)
    }

    fn a11y_children(&self) -> Vec<A11yChild> {
        let total = self.items.len();
        a11y_window(self.scroll_offset, total)
            .map(|idx| completion_child(self, idx, total))
            .collect()
    }

    fn a11y_focus_key(&self) -> Option<String> {
        if self.selected_idx < self.items.len() {
            Some(format!("item.{}", self.selected_idx))
        } else {
            None
        }
    }
}

fn completion_child(completions: &Completions, idx: usize, total: usize) -> A11yChild {
    let item = &completions.items[idx];
    let mut info = in_set(
        A11yInfo::new()
            .with_role(A11yRole::ListItem)
            .with_selected(idx == completions.selected_idx),
        idx,
        total,
    );
    if let Some(label) = non_empty(plain_text(&item.label)) {
        info = info.with_label(label);
    }
    if let Some(detail) = item.detail.as_ref().and_then(|d| non_empty(plain_text(d))) {
        info = info.with_value(detail);
    }
    A11yChild::new(format!("item.{idx}"), info)
}

impl A11yPrimitive for Palette {
    fn a11y_info(&self) -> A11yInfo {
        let mut info = A11yInfo::new()
            .with_role(A11yRole::List)
            .with_set_size(self.items.len() as u32);
        if let Some(label) = non_empty(self.title.clone()) {
            info = info.with_label(label);
        }
        info
    }

    fn a11y_children(&self) -> Vec<A11yChild> {
        let total = self.items.len();
        a11y_window(self.scroll_offset, total)
            .map(|idx| palette_child(self, idx, total))
            .collect()
    }

    fn a11y_focus_key(&self) -> Option<String> {
        if self.selected_idx < self.items.len() {
            Some(format!("item.{}", self.selected_idx))
        } else {
            None
        }
    }
}

fn palette_child(palette: &Palette, idx: usize, total: usize) -> A11yChild {
    let item = &palette.items[idx];
    let mut info = in_set(
        A11yInfo::new()
            .with_role(A11yRole::ListItem)
            .with_selected(idx == palette.selected_idx)
            .with_level(item.depth as u32 + 1),
        idx,
        total,
    );
    if item.expandable {
        info = info.with_expanded(item.expanded);
    }
    if let Some(label) = non_empty(plain_text(&item.text)) {
        info = info.with_label(label);
    }
    if let Some(detail) = item.detail.as_ref().and_then(|d| non_empty(plain_text(d))) {
        info = info.with_value(detail);
    }
    A11yChild::new(format!("item.{idx}"), info)
}

impl A11yPrimitive for BoardModel {
    fn a11y_info(&self) -> A11yInfo {
        A11yInfo::new()
            .with_role(A11yRole::Group)
            .with_label("Board")
            .with_set_size(self.columns.len() as u32)
    }

    fn a11y_children(&self) -> Vec<A11yChild> {
        let total = self.columns.len();
        self.columns
            .iter()
            .enumerate()
            .map(|(idx, column)| board_column_child(self, column, idx, total))
            .collect()
    }

    fn a11y_focus_key(&self) -> Option<String> {
        // `A11yChild`'s key identifies a child within its own parent, and
        // `build_tree` only resolves a focus key against the primitive's
        // direct (top-level) children (see `tree.rs`'s `build_tree`), so
        // the selected card's own key — nested two levels under the board
        // — can never match here. Point focus at its column instead,
        // which is a direct child and so does resolve.
        let selected = self.selected_card_id?;
        self.columns
            .iter()
            .position(|column| column.cards.iter().any(|card| card.id == selected))
            .map(|col_idx| format!("column.{col_idx}"))
    }
}

fn board_column_child(
    board: &BoardModel,
    column: &BoardColumn,
    idx: usize,
    total: usize,
) -> A11yChild {
    // `position_in_set`/`set_size` here describe the column's position
    // among sibling columns only — never overwritten with the card
    // count below, which is a different set entirely.
    let info = in_set(
        A11yInfo::new()
            .with_role(A11yRole::List)
            .with_label(column.title.clone()),
        idx,
        total,
    );
    let total_cards = column.cards.len();
    let cards = a11y_window(column.scroll_offset, total_cards)
        .map(|card_idx| board_card_child(board, column, card_idx, total_cards))
        .collect();
    A11yChild::new(format!("column.{idx}"), info).with_children(cards)
}

fn board_card_child(
    board: &BoardModel,
    column: &BoardColumn,
    card_idx: usize,
    total_cards: usize,
) -> A11yChild {
    let card = &column.cards[card_idx];
    let info = in_set(
        A11yInfo::new()
            .with_role(A11yRole::ListItem)
            .with_label(card.title.clone())
            .with_selected(board.selected_card_id == Some(card.id)),
        card_idx,
        total_cards,
    );
    A11yChild::new(format!("card.{card_idx}"), info)
}

impl A11yPrimitive for PipelineView {
    fn a11y_info(&self) -> A11yInfo {
        A11yInfo::new()
            .with_role(A11yRole::List)
            .with_label("Pipeline view")
            .with_set_size(self.stages.len() as u32)
    }

    fn a11y_children(&self) -> Vec<A11yChild> {
        let total = self.stages.len();
        self.stages
            .iter()
            .enumerate()
            .map(|(idx, stage)| pipeline_stage_child(stage, idx, total))
            .collect()
    }

    fn a11y_focus_key(&self) -> Option<String> {
        self.focused_stage.map(|idx| format!("stage.{idx}"))
    }
}

fn pipeline_stage_child(stage: &PipelineStage, idx: usize, total: usize) -> A11yChild {
    let mut info = in_set(
        A11yInfo::new()
            .with_role(A11yRole::ListItem)
            .with_label(stage.label.clone()),
        idx,
        total,
    );
    if let Some(value) = pipeline_stage_value(&stage.status) {
        info = info.with_value(value);
    }
    A11yChild::new(format!("stage.{idx}"), info)
}

fn pipeline_stage_value(status: &StageStatus) -> Option<&'static str> {
    match status {
        StageStatus::Pending => Some("Pending"),
        StageStatus::Active => Some("Active"),
        StageStatus::Done => Some("Done"),
        StageStatus::Failed => Some("Failed"),
        StageStatus::Skipped => Some("Skipped"),
        StageStatus::Stale => Some("Stale"),
    }
}

impl A11yPrimitive for MultiSectionView {
    fn a11y_info(&self) -> A11yInfo {
        A11yInfo::new()
            .with_role(A11yRole::Group)
            .with_label("Multi-section view")
            .with_set_size(self.sections.len() as u32)
    }

    fn a11y_children(&self) -> Vec<A11yChild> {
        let total = self.sections.len();
        self.sections
            .iter()
            .enumerate()
            .map(|(idx, section)| {
                section_child(section, idx, total, self.active_section == Some(idx))
            })
            .collect()
    }

    fn a11y_focus_key(&self) -> Option<String> {
        self.active_section.map(|idx| format!("section.{idx}"))
    }
}

fn section_child(section: &Section, idx: usize, total: usize, active: bool) -> A11yChild {
    let mut info = in_set(
        A11yInfo::new()
            .with_role(A11yRole::Group)
            .with_expanded(!section.collapsed)
            .with_selected(active),
        idx,
        total,
    );
    if let Some(label) = non_empty(plain_text(&section.header.title)) {
        info = info.with_label(label);
    }
    let children = if section.collapsed {
        Vec::new()
    } else {
        section_body_child(&section.body).into_iter().collect()
    };
    A11yChild::new(format!("section.{idx}"), info).with_children(children)
}

/// Maps a section's body to a single `"body"` child that carries the body
/// primitive's own `a11y_info()` (role, label, set size) with its
/// `a11y_children()` nested underneath — mirroring `chrome.rs`'s
/// `A11yChild::new("input", bar.a11y_info()).with_children(bar.a11y_children())`
/// pattern. Dropping the body's own info and re-parenting only its children
/// directly under the section would strip the container role/label/set-size
/// (for `Tree`/`List`/`Form`/`MessageList`) and drop the body entirely for
/// `Terminal`/`Chart`, which expose no children of their own.
fn section_body_child(body: &SectionBody) -> Option<A11yChild> {
    match body {
        SectionBody::Tree(tree) => Some(
            A11yChild::new("body".to_string(), tree.a11y_info())
                .with_children(tree.a11y_children()),
        ),
        SectionBody::List(list) => Some(
            A11yChild::new("body".to_string(), list.a11y_info())
                .with_children(list.a11y_children()),
        ),
        SectionBody::Form(form) => Some(
            A11yChild::new("body".to_string(), form.a11y_info())
                .with_children(form.a11y_children()),
        ),
        SectionBody::Terminal(terminal) => {
            Some(A11yChild::new("body".to_string(), terminal.a11y_info()))
        }
        SectionBody::MessageList(list) => Some(
            A11yChild::new("body".to_string(), list.a11y_info())
                .with_children(list.a11y_children()),
        ),
        SectionBody::Chart(chart) => Some(A11yChild::new("body".to_string(), chart.a11y_info())),
        SectionBody::Text(lines) => {
            let label = lines.iter().map(plain_text).collect::<Vec<_>>().join("\n");
            let mut info = A11yInfo::new().with_role(A11yRole::Label);
            if let Some(label) = non_empty(label) {
                info = info.with_label(label);
            }
            Some(A11yChild::new("body".to_string(), info))
        }
        SectionBody::Empty(empty) => {
            let mut info = A11yInfo::new().with_role(A11yRole::Group);
            if let Some(label) = non_empty(plain_text(&empty.text)) {
                info = info.with_label(label);
            }
            Some(A11yChild::new("body".to_string(), info))
        }
        SectionBody::Custom(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::{ScreenLayout, Surface};
    use crate::layout::{Anchor, Side};
    use crate::primitives::board::{BoardCard, BoardColumn, BoardModel};
    use crate::primitives::canvas::Canvas;
    use crate::primitives::chart::{Chart, ChartKind, Series};
    use crate::primitives::completions::{
        CompletionItem, CompletionKind, CompletionsLayout, CompletionsPlacement,
    };
    use crate::primitives::data_table::{Column, ColumnAlign, ColumnWidth, DataRow};
    use crate::primitives::float::{Float, FloatMeasure};
    use crate::primitives::form::Form;
    use crate::primitives::list::ListItem;
    use crate::primitives::message_list::MessageRow;
    use crate::primitives::multi_section_view::{
        Axis, EmptyBody, ScrollMode, Section, SectionBody, SectionHeader, SectionSize,
    };
    use crate::primitives::palette::{PaletteItem, PaletteMode};
    use crate::primitives::pipeline_view::{PipelineStage, StageStatus};
    use crate::primitives::scrollbar::Scrollbar;
    use crate::primitives::terminal::{Terminal, TerminalCell};
    use crate::primitives::toolbar::ToolbarButton;
    use crate::types::{Color, Decoration, SelectionMode, TreeStyle, WidgetId};

    fn list(id: &str, title: Option<&str>, items: &[&str], selected: usize) -> ListView {
        ListView {
            id: WidgetId::new(id),
            title: title.map(StyledText::plain),
            items: items
                .iter()
                .map(|text| ListItem {
                    text: StyledText::plain(*text),
                    icon: None,
                    detail: None,
                    decoration: Decoration::default(),
                })
                .collect(),
            selected_idx: selected,
            scroll_offset: 0,
            has_focus: false,
            bordered: false,
            h_scroll: 0,
            max_content_width: None,
            show_v_scrollbar: false,
        }
    }

    fn toolbar(id: &str) -> Toolbar {
        Toolbar::new(WidgetId::new(id)).with_buttons(vec![
            ToolbarButton::Action {
                id: WidgetId::new("run"),
                label: "Run".to_string(),
                icon: None,
                key_hint: None,
                enabled: true,
                is_active: false,
                tooltip: String::new(),
            },
            ToolbarButton::Separator,
            ToolbarButton::Action {
                id: WidgetId::new("stop"),
                label: "Stop".to_string(),
                icon: None,
                key_hint: None,
                enabled: false,
                is_active: false,
                tooltip: String::new(),
            },
        ])
    }

    fn frame_with(surfaces: Vec<Surface>) -> OwnedFrame {
        let mut layout = ScreenLayout::new();
        for surface in surfaces {
            layout.push(surface);
        }
        OwnedFrame::from_layout(&layout)
    }

    fn list_surface() -> Surface {
        Surface::List {
            rect: Rect::new(0.0, 0.0, 30.0, 10.0),
            list: list("files", Some("Files"), &["a.rs", "b.rs", "c.rs"], 1),
        }
    }

    /// A surface whose capture carries no top-level `WidgetId` (the id
    /// lives on the primitive, not the `Surface` variant itself).
    fn canvas_surface() -> Surface {
        Surface::Canvas {
            rect: Rect::new(0.0, 0.0, 30.0, 10.0),
            canvas: Canvas {
                id: WidgetId::new("sketch"),
                ops: Vec::new(),
            },
        }
    }

    /// A board with two columns and a selected card, for exercising the
    /// `BoardModel` mapping (column/card roles and their independent
    /// position-in-set/set-size pairs).
    fn board_surface() -> Surface {
        Surface::Board {
            rect: Rect::new(0.0, 0.0, 30.0, 10.0),
            model: BoardModel {
                id: WidgetId::new("sprint"),
                columns: vec![
                    BoardColumn {
                        id: WidgetId::new("todo"),
                        title: "Todo".to_string(),
                        cards: vec![
                            BoardCard {
                                id: WidgetId::new("card-1"),
                                title: "Write docs".to_string(),
                                labels: Vec::new(),
                                badges: Vec::new(),
                                hint: None,
                            },
                            BoardCard {
                                id: WidgetId::new("card-2"),
                                title: "Fix bug".to_string(),
                                labels: Vec::new(),
                                badges: Vec::new(),
                                hint: None,
                            },
                            BoardCard {
                                id: WidgetId::new("card-4"),
                                title: "Plan sprint".to_string(),
                                labels: Vec::new(),
                                badges: Vec::new(),
                                hint: None,
                            },
                        ],
                        scroll_offset: 0,
                    },
                    BoardColumn {
                        id: WidgetId::new("done"),
                        title: "Done".to_string(),
                        cards: vec![BoardCard {
                            id: WidgetId::new("card-3"),
                            title: "Ship it".to_string(),
                            labels: Vec::new(),
                            badges: Vec::new(),
                            hint: None,
                        }],
                        scroll_offset: 0,
                    },
                ],
                selected_card_id: Some(WidgetId::new("card-2")),
                col_scroll_offset: 0,
            },
        }
    }

    /// A surface whose primitive has no `a11y_info` hook — the
    /// generic-fallback case.
    fn float_surface() -> Surface {
        let float = Float::new(
            WidgetId::new("f"),
            Anchor::new(Rect::new(0.0, 0.0, 5.0, 1.0), Side::Bottom),
        );
        let layout = float.layout(
            Rect::new(0.0, 0.0, 80.0, 24.0),
            FloatMeasure::new(10.0, 4.0),
        );
        Surface::Float { float, layout }
    }

    /// A mapped surface whose capture carries no `WidgetId`, and whose
    /// mapping emits a presentational child for the thumb.
    fn scrollbar_surface() -> Surface {
        let rect = Rect::new(29.0, 0.0, 1.0, 10.0);
        Surface::Scrollbar {
            rect,
            sb: Scrollbar::vertical(WidgetId::new("files.vscroll"), rect, 0.0, 30.0, 10.0, 1.0),
        }
    }

    fn toolbar_surface() -> Surface {
        Surface::Toolbar {
            rect: Rect::new(0.0, 10.0, 30.0, 2.0),
            bar: toolbar("tools"),
            hovered: None,
            pressed: None,
        }
    }

    #[test]
    fn root_is_a_window_covering_every_surface() {
        let frame = frame_with(vec![list_surface(), toolbar_surface()]);
        let tree = frame_to_tree(&frame, &FocusManager::new());
        let root = tree.node(tree.root()).expect("root node");
        assert_eq!(root.role(), A11yRole::Window);
        assert_eq!(root.label(), Some("quadraui"));
        assert_eq!(root.children.len(), 2, "one child per surface");
        assert_eq!(root.bounds, Some(Rect::new(0.0, 0.0, 30.0, 12.0)));
    }

    #[test]
    fn list_maps_to_a_list_with_item_children() {
        let frame = frame_with(vec![list_surface()]);
        let tree = frame_to_tree(&frame, &FocusManager::new());
        assert_eq!(
            tree.to_snapshot(),
            "window \"quadraui\" [focused]\n  \
             list \"Files\" [size=3]\n    \
             list_item \"a.rs\" [pos=1/3]\n    \
             list_item \"b.rs\" [selected pos=2/3]\n    \
             list_item \"c.rs\" [pos=3/3]\n"
        );
    }

    #[test]
    fn toolbar_maps_buttons_with_disabled_state() {
        let frame = frame_with(vec![toolbar_surface()]);
        let tree = frame_to_tree(&frame, &FocusManager::new());
        assert_eq!(
            tree.to_snapshot(),
            "window \"quadraui\" [focused]\n  \
             toolbar\n    \
             button \"Run\"\n    \
             presentation\n    \
             button \"Stop\" [disabled]\n"
        );
    }

    #[test]
    fn unmapped_primitive_is_a_labelled_group() {
        let frame = frame_with(vec![float_surface()]);
        let tree = frame_to_tree(&frame, &FocusManager::new());
        let node = tree
            .children_of(tree.root())
            .first()
            .copied()
            .expect("surface node");
        assert_eq!(node.role(), A11yRole::Group);
        assert_eq!(node.label(), Some("Floating panel"));
    }

    #[test]
    fn scrollbar_reports_its_axis_and_a_presentational_thumb() {
        let frame = frame_with(vec![scrollbar_surface()]);
        let tree = frame_to_tree(&frame, &FocusManager::new());
        assert_eq!(
            tree.to_snapshot(),
            "window \"quadraui\" [focused]\n  \
             scroll_bar \"Vertical scrollbar\" [range=0/0-1]\n    \
             presentation \"Scrollbar thumb\"\n"
        );
    }

    #[test]
    fn node_ids_are_stable_across_frames_and_positions() {
        let first = frame_with(vec![list_surface(), toolbar_surface()]);
        // Same widgets, reordered and moved.
        let second = frame_with(vec![
            Surface::Toolbar {
                rect: Rect::new(0.0, 0.0, 30.0, 2.0),
                bar: toolbar("tools"),
                hovered: None,
                pressed: None,
            },
            Surface::List {
                rect: Rect::new(0.0, 2.0, 30.0, 10.0),
                list: list("files", Some("Files"), &["a.rs", "b.rs", "c.rs"], 1),
            },
        ]);
        let fm = FocusManager::new();
        let a = frame_to_tree(&first, &fm);
        let b = frame_to_tree(&second, &fm);
        let list_id = A11yNodeId::for_widget(&WidgetId::new("files"));
        assert_eq!(a.node(list_id).map(|n| n.id), Some(list_id));
        assert_eq!(b.node(list_id).map(|n| n.id), Some(list_id));
        assert_eq!(
            a.find("b.rs").map(|n| n.id),
            b.find("b.rs").map(|n| n.id),
            "a row keeps its id when its parent moves"
        );
    }

    #[test]
    fn focus_follows_the_focus_manager_into_the_active_row() {
        let frame = frame_with(vec![list_surface(), toolbar_surface()]);
        let mut fm = FocusManager::new();
        let unfocused = frame_to_tree(&frame, &fm);
        assert_eq!(
            unfocused.focus(),
            unfocused.root(),
            "nothing focused parks focus on the root"
        );

        fm.set_focus(Some(WidgetId::new("files")));
        let tree = frame_to_tree(&frame, &fm);
        assert_eq!(
            tree.focused().and_then(|n| n.label()),
            Some("b.rs"),
            "a focused list points focus at its selected row"
        );
        assert!(tree.to_snapshot().contains("list_item \"b.rs\" [focused"));
    }

    #[test]
    fn focus_lands_on_the_focused_toolbar_button() {
        let mut bar = toolbar("tools");
        bar.focused_index = Some(2);
        let frame = frame_with(vec![Surface::Toolbar {
            rect: Rect::new(0.0, 0.0, 30.0, 2.0),
            bar,
            hovered: None,
            pressed: None,
        }]);
        let mut fm = FocusManager::new();
        fm.set_focus(Some(WidgetId::new("tools")));
        let tree = frame_to_tree(&frame, &fm);
        assert_eq!(tree.focused().and_then(|n| n.label()), Some("Stop"));
    }

    #[test]
    fn consumer_a11y_override_wins_field_by_field() {
        let mut frame = frame_with(vec![list_surface()]);
        frame.nodes_mut()[0].a11y = Some(A11yInfo::new().with_label("Open editors"));
        let tree = frame_to_tree(&frame, &FocusManager::new());
        let node = tree
            .children_of(tree.root())
            .first()
            .copied()
            .expect("surface node");
        assert_eq!(node.label(), Some("Open editors"), "label is overridden");
        assert_eq!(node.role(), A11yRole::List, "derived role survives");
    }

    #[test]
    fn first_update_is_full_then_only_changed_nodes_are_emitted() {
        let mut builder = A11yTreeBuilder::new();
        let frame = frame_with(vec![list_surface(), toolbar_surface()]);
        let fm = FocusManager::new();

        let first = builder.update(&frame, &fm);
        assert!(first.full);
        assert_eq!(first.root, Some(A11yNodeId::ROOT));
        assert_eq!(first.nodes.len(), frame_to_tree(&frame, &fm).nodes().len());

        let second = builder.update(&frame, &fm);
        assert!(!second.full);
        assert_eq!(second.root, None, "the tree is only declared once");
        assert!(
            second.nodes.is_empty(),
            "an unchanged frame emits no nodes, got {:?}",
            second.nodes
        );
        assert_eq!(second.focus, first.focus, "focus is always reported");
    }

    #[test]
    fn changed_selection_emits_only_the_affected_rows() {
        let mut builder = A11yTreeBuilder::new();
        let fm = FocusManager::new();
        builder.update(&frame_with(vec![list_surface()]), &fm);

        let moved = frame_with(vec![Surface::List {
            rect: Rect::new(0.0, 0.0, 30.0, 10.0),
            list: list("files", Some("Files"), &["a.rs", "b.rs", "c.rs"], 2),
        }]);
        let update = builder.update(&moved, &fm);
        let labels: Vec<_> = update.nodes.iter().filter_map(|n| n.label()).collect();
        assert_eq!(
            labels,
            vec!["b.rs", "c.rs"],
            "only the deselected and newly selected rows changed"
        );
    }

    #[test]
    fn removing_a_row_re_emits_the_parent_without_it() {
        let mut builder = A11yTreeBuilder::new();
        let fm = FocusManager::new();
        builder.update(&frame_with(vec![list_surface()]), &fm);

        let shorter = frame_with(vec![Surface::List {
            rect: Rect::new(0.0, 0.0, 30.0, 10.0),
            list: list("files", Some("Files"), &["a.rs", "b.rs"], 1),
        }]);
        let update = builder.update(&shorter, &fm);
        let list_id = A11yNodeId::for_widget(&WidgetId::new("files"));
        let parent = update
            .nodes
            .iter()
            .find(|n| n.id == list_id)
            .expect("parent is re-emitted when its child list shrinks");
        assert_eq!(parent.children.len(), 2);
        let gone = list_id.child("item.2");
        assert!(
            !update.nodes.iter().any(|n| n.id == gone),
            "the removed row is not part of the update"
        );
    }

    #[test]
    fn reset_forces_a_full_tree_again() {
        let mut builder = A11yTreeBuilder::new();
        let frame = frame_with(vec![list_surface()]);
        let fm = FocusManager::new();
        builder.update(&frame, &fm);
        builder.reset();
        let again = builder.update(&frame, &fm);
        assert!(again.full);
        assert_eq!(again.root, Some(A11yNodeId::ROOT));
    }

    #[test]
    fn root_label_is_overridable() {
        let builder = A11yTreeBuilder::new().with_root_label("kubeui");
        let tree = builder.tree(&frame_with(vec![list_surface()]), &FocusManager::new());
        assert_eq!(
            tree.node(tree.root()).and_then(|n| n.label()),
            Some("kubeui")
        );
    }

    #[test]
    fn surfaces_without_a_widget_id_still_appear() {
        let frame = frame_with(vec![canvas_surface()]);
        let tree = frame_to_tree(&frame, &FocusManager::new());
        assert_eq!(tree.nodes().len(), 2, "root plus the id-less surface");
        assert_eq!(
            tree.children_of(tree.root())[0].id,
            A11yNodeId::for_key("surface.0")
        );
    }

    #[cfg(feature = "a11y")]
    #[test]
    fn tree_update_carries_tree_info_focus_and_nodes() {
        let mut builder = A11yTreeBuilder::new();
        let frame = frame_with(vec![list_surface()]);
        let mut fm = FocusManager::new();
        fm.set_focus(Some(WidgetId::new("files")));

        let update = builder.tree_update(&frame, &fm);
        let tree_info = update.tree.expect("first update declares the tree");
        assert_eq!(tree_info.root, A11yNodeId::ROOT.to_accesskit());
        assert_eq!(update.tree_id, accesskit::TreeId::ROOT);
        let list_id = A11yNodeId::for_widget(&WidgetId::new("files"));
        assert_eq!(update.focus, list_id.child("item.1").to_accesskit());

        let (_, list_node) = update
            .nodes
            .iter()
            .find(|(id, _)| *id == list_id.to_accesskit())
            .expect("the list node is in the update");
        assert_eq!(list_node.role(), accesskit::Role::List);
        assert_eq!(list_node.label(), Some("Files"));
        assert_eq!(list_node.children().len(), 3);
        assert_eq!(
            list_node.bounds(),
            Some(accesskit::Rect::new(0.0, 0.0, 30.0, 10.0))
        );

        let (_, row) = update
            .nodes
            .iter()
            .find(|(id, _)| *id == list_id.child("item.1").to_accesskit())
            .expect("the selected row is in the update");
        assert_eq!(row.is_selected(), Some(true));
        assert_eq!(
            row.position_in_set(),
            Some(1),
            "accesskit's position_in_set is zero-based"
        );
        assert_eq!(row.size_of_set(), Some(3));

        let unchanged = builder.tree_update(&frame, &fm);
        assert!(unchanged.nodes.is_empty());
        assert!(unchanged.tree.is_none());
        assert_eq!(unchanged.focus, update.focus);
    }

    #[test]
    fn pipeline_view_maps_stages_with_status_value_and_position() {
        let view = PipelineView {
            id: WidgetId::new("deploy"),
            stages: vec![
                PipelineStage {
                    label: "Build".to_string(),
                    status: StageStatus::Pending,
                    action: None,
                },
                PipelineStage {
                    label: "Test".to_string(),
                    status: StageStatus::Active,
                    action: None,
                },
                PipelineStage {
                    label: "Lint".to_string(),
                    status: StageStatus::Done,
                    action: None,
                },
                PipelineStage {
                    label: "Package".to_string(),
                    status: StageStatus::Failed,
                    action: None,
                },
                PipelineStage {
                    label: "Docs".to_string(),
                    status: StageStatus::Skipped,
                    action: None,
                },
                PipelineStage {
                    label: "Release".to_string(),
                    status: StageStatus::Stale,
                    action: None,
                },
            ],
            focused_stage: None,
        };
        let frame = frame_with(vec![Surface::PipelineView {
            rect: Rect::new(0.0, 0.0, 60.0, 4.0),
            view,
        }]);
        let tree = frame_to_tree(&frame, &FocusManager::new());
        assert_eq!(
            tree.to_snapshot(),
            "window \"quadraui\" [focused]\n  \
             list \"Pipeline view\" [size=6]\n    \
             list_item \"Build\" [value=\"Pending\" pos=1/6]\n    \
             list_item \"Test\" [value=\"Active\" pos=2/6]\n    \
             list_item \"Lint\" [value=\"Done\" pos=3/6]\n    \
             list_item \"Package\" [value=\"Failed\" pos=4/6]\n    \
             list_item \"Docs\" [value=\"Skipped\" pos=5/6]\n    \
             list_item \"Release\" [value=\"Stale\" pos=6/6]\n"
        );
    }

    #[test]
    fn board_model_maps_columns_and_cards_with_independent_positions() {
        let frame = frame_with(vec![board_surface()]);
        let tree = frame_to_tree(&frame, &FocusManager::new());
        assert_eq!(
            tree.to_snapshot(),
            "window \"quadraui\" [focused]\n  \
             group \"Board\" [size=2]\n    \
             list \"Todo\" [pos=1/2]\n      \
             list_item \"Write docs\" [pos=1/3]\n      \
             list_item \"Fix bug\" [selected pos=2/3]\n      \
             list_item \"Plan sprint\" [pos=3/3]\n    \
             list \"Done\" [pos=2/2]\n      \
             list_item \"Ship it\" [pos=1/1]\n"
        );
    }

    #[test]
    fn data_table_maps_rows_cells_and_a_footer_with_no_position() {
        let table = DataTable {
            id: WidgetId::new("people"),
            columns: vec![
                Column {
                    title: "Name".to_string(),
                    width: ColumnWidth::default(),
                    align: ColumnAlign::default(),
                },
                Column {
                    title: "Age".to_string(),
                    width: ColumnWidth::default(),
                    align: ColumnAlign::default(),
                },
            ],
            rows: vec![
                DataRow {
                    cells: vec![StyledText::plain("Alice"), StyledText::plain("30")],
                    decoration: Decoration::default(),
                },
                DataRow {
                    cells: vec![StyledText::plain("Bob"), StyledText::plain("25")],
                    decoration: Decoration::default(),
                },
            ],
            selected_idx: Some(1),
            scroll_offset: 0,
            sort: None,
            has_focus: false,
            show_scrollbar: false,
            min_total_width: None,
            h_scroll: 0.0,
            column_overrides: Vec::new(),
            footer: Some(DataRow {
                cells: vec![StyledText::plain("Total"), StyledText::plain("2")],
                decoration: Decoration::default(),
            }),
        };
        let frame = frame_with(vec![Surface::DataTable {
            rect: Rect::new(0.0, 0.0, 30.0, 10.0),
            table,
            hovered: None,
        }]);
        let tree = frame_to_tree(&frame, &FocusManager::new());
        assert_eq!(
            tree.to_snapshot(),
            "window \"quadraui\" [focused]\n  \
             table \"Table\" [size=2]\n    \
             row \"Header\"\n      \
             cell \"Name\" [pos=1/2]\n      \
             cell \"Age\" [pos=2/2]\n    \
             row [pos=1/2]\n      \
             cell \"Alice\" [pos=1/2]\n      \
             cell \"30\" [pos=2/2]\n    \
             row [selected pos=2/2]\n      \
             cell \"Bob\" [pos=1/2]\n      \
             cell \"25\" [pos=2/2]\n    \
             row\n      \
             cell \"Total\" [pos=1/2]\n      \
             cell \"2\" [pos=2/2]\n"
        );
    }

    #[test]
    fn data_table_virtualisation_reports_true_set_size_under_a_100k_row_window() {
        let total = 100_000usize;
        let table = DataTable {
            id: WidgetId::new("big"),
            columns: vec![Column {
                title: "Value".to_string(),
                width: ColumnWidth::default(),
                align: ColumnAlign::default(),
            }],
            rows: (0..total)
                .map(|i| DataRow {
                    cells: vec![StyledText::plain(format!("row-{i}"))],
                    decoration: Decoration::default(),
                })
                .collect(),
            selected_idx: None,
            scroll_offset: 50_000,
            sort: None,
            has_focus: false,
            show_scrollbar: false,
            min_total_width: None,
            h_scroll: 0.0,
            column_overrides: Vec::new(),
            footer: None,
        };
        let frame = frame_with(vec![Surface::DataTable {
            rect: Rect::new(0.0, 0.0, 30.0, 10.0),
            table,
            hovered: None,
        }]);
        let tree = frame_to_tree(&frame, &FocusManager::new());

        let table_id = A11yNodeId::for_widget(&WidgetId::new("big"));
        let rows = tree.children_of(table_id);
        // The header row is always materialised, on top of the 200-row
        // window around `scroll_offset` — never the full 100,000 data rows.
        assert_eq!(rows.len(), 201);

        let data_rows = &rows[1..];
        let first = data_rows.first().expect("windowed row");
        assert_eq!(first.info.position_in_set, Some(50_001));
        assert_eq!(first.info.set_size, Some(100_000));

        let last = data_rows.last().expect("windowed row");
        assert_eq!(last.info.position_in_set, Some(50_200));
        assert_eq!(last.info.set_size, Some(100_000));
    }

    #[test]
    fn tree_view_maps_rows_flat_with_level_token_and_grouped_sibling_position() {
        let tree_view = TreeView {
            id: WidgetId::new("files"),
            rows: vec![
                TreeRow {
                    path: vec![0],
                    indent: 0,
                    icon: None,
                    text: StyledText::plain("src"),
                    badge: None,
                    is_expanded: Some(true),
                    decoration: Decoration::default(),
                    edit: None,
                },
                TreeRow {
                    path: vec![0, 0],
                    indent: 1,
                    icon: None,
                    text: StyledText::plain("main.rs"),
                    badge: None,
                    is_expanded: None,
                    decoration: Decoration::default(),
                    edit: None,
                },
                TreeRow {
                    path: vec![0, 1],
                    indent: 1,
                    icon: None,
                    text: StyledText::plain("lib.rs"),
                    badge: None,
                    is_expanded: None,
                    decoration: Decoration::default(),
                    edit: None,
                },
                TreeRow {
                    path: vec![1],
                    indent: 0,
                    icon: None,
                    text: StyledText::plain("docs"),
                    badge: None,
                    is_expanded: Some(false),
                    decoration: Decoration::default(),
                    edit: None,
                },
            ],
            selection_mode: SelectionMode::Single,
            selected_path: Some(vec![0, 1]),
            scroll_offset: 0,
            style: TreeStyle::default(),
            has_focus: false,
        };
        let frame = frame_with(vec![Surface::Tree {
            rect: Rect::new(0.0, 0.0, 30.0, 10.0),
            tree: tree_view,
        }]);
        let tree = frame_to_tree(&frame, &FocusManager::new());
        assert_eq!(
            tree.to_snapshot(),
            "window \"quadraui\" [focused]\n  \
             tree [size=2]\n    \
             tree_item \"src\" [expanded level=1 pos=1/2]\n    \
             tree_item \"main.rs\" [level=2 pos=1/2]\n    \
             tree_item \"lib.rs\" [selected level=2 pos=2/2]\n    \
             tree_item \"docs\" [collapsed level=1 pos=2/2]\n"
        );
    }

    #[test]
    fn message_list_maps_rows_as_list_items_with_position() {
        let list = MessageList {
            id: WidgetId::new("log"),
            rows: vec![
                MessageRow::new("Connected", Color::rgb(0, 200, 0), 0.0),
                MessageRow::new("Warning: low disk", Color::rgb(200, 200, 0), 0.0),
                MessageRow::new("Disconnected", Color::rgb(200, 0, 0), 0.0),
            ],
            scroll_top: 0,
        };
        let frame = frame_with(vec![Surface::MessageList {
            rect: Rect::new(0.0, 0.0, 40.0, 6.0),
            list,
        }]);
        let tree = frame_to_tree(&frame, &FocusManager::new());
        assert_eq!(
            tree.to_snapshot(),
            "window \"quadraui\" [focused]\n  \
             list \"Message list\" [size=3]\n    \
             list_item \"Connected\" [pos=1/3]\n    \
             list_item \"Warning: low disk\" [pos=2/3]\n    \
             list_item \"Disconnected\" [pos=3/3]\n"
        );
    }

    #[test]
    fn multi_section_view_maps_sections_as_groups_with_expanded_and_active_state() {
        let view = MultiSectionView {
            id: WidgetId::new("panels"),
            sections: vec![
                Section {
                    id: "logs".to_string(),
                    header: SectionHeader {
                        icon: None,
                        title: StyledText::plain("Logs"),
                        badge: None,
                        actions: Vec::new(),
                        show_chevron: true,
                    },
                    body: SectionBody::Text(vec![StyledText::plain("Build succeeded")]),
                    aux: None,
                    size: SectionSize::Weight(1.0),
                    collapsed: false,
                    min_size: None,
                    max_size: None,
                },
                Section {
                    id: "empty".to_string(),
                    header: SectionHeader {
                        icon: None,
                        title: StyledText::plain("Empty"),
                        badge: None,
                        actions: Vec::new(),
                        show_chevron: true,
                    },
                    body: SectionBody::Empty(EmptyBody {
                        icon: None,
                        text: StyledText::plain("Nothing here"),
                        hint: None,
                        action: None,
                    }),
                    aux: None,
                    size: SectionSize::Weight(1.0),
                    collapsed: true,
                    min_size: None,
                    max_size: None,
                },
            ],
            active_section: Some(0),
            axis: Axis::Vertical,
            allow_resize: false,
            allow_collapse: true,
            scroll_mode: ScrollMode::PerSection,
            has_focus: false,
            panel_scroll: 0.0,
        };
        let frame = frame_with(vec![Surface::MultiSectionView {
            rect: Rect::new(0.0, 0.0, 40.0, 20.0),
            view,
        }]);
        let tree = frame_to_tree(&frame, &FocusManager::new());
        assert_eq!(
            tree.to_snapshot(),
            "window \"quadraui\" [focused]\n  \
             group \"Multi-section view\" [size=2]\n    \
             group \"Logs\" [selected expanded pos=1/2]\n      \
             label \"Build succeeded\"\n    \
             group \"Empty\" [collapsed pos=2/2]\n"
        );
    }

    #[test]
    fn multi_section_view_body_delegates_container_role_label_and_children_for_every_variant() {
        let view = MultiSectionView {
            id: WidgetId::new("panels"),
            sections: vec![
                Section {
                    id: "tree".to_string(),
                    header: SectionHeader {
                        icon: None,
                        title: StyledText::plain("Tree"),
                        badge: None,
                        actions: Vec::new(),
                        show_chevron: true,
                    },
                    body: SectionBody::Tree(TreeView {
                        id: WidgetId::new("files"),
                        rows: vec![TreeRow {
                            path: vec![0],
                            indent: 0,
                            icon: None,
                            text: StyledText::plain("root"),
                            badge: None,
                            is_expanded: None,
                            decoration: Decoration::default(),
                            edit: None,
                        }],
                        selection_mode: SelectionMode::Single,
                        selected_path: None,
                        scroll_offset: 0,
                        style: TreeStyle::default(),
                        has_focus: false,
                    }),
                    aux: None,
                    size: SectionSize::Weight(1.0),
                    collapsed: false,
                    min_size: None,
                    max_size: None,
                },
                Section {
                    id: "list".to_string(),
                    header: SectionHeader {
                        icon: None,
                        title: StyledText::plain("List"),
                        badge: None,
                        actions: Vec::new(),
                        show_chevron: true,
                    },
                    body: SectionBody::List(list("notes", Some("Notes"), &["Buy milk"], 1)),
                    aux: None,
                    size: SectionSize::Weight(1.0),
                    collapsed: false,
                    min_size: None,
                    max_size: None,
                },
                Section {
                    id: "form".to_string(),
                    header: SectionHeader {
                        icon: None,
                        title: StyledText::plain("Form"),
                        badge: None,
                        actions: Vec::new(),
                        show_chevron: true,
                    },
                    body: SectionBody::Form(Form {
                        id: WidgetId::new("settings"),
                        fields: Vec::new(),
                        focused_field: None,
                        scroll_offset: 0,
                        has_focus: false,
                        label_position: Default::default(),
                        label_align: Default::default(),
                        label_width: None,
                    }),
                    aux: None,
                    size: SectionSize::Weight(1.0),
                    collapsed: false,
                    min_size: None,
                    max_size: None,
                },
                Section {
                    id: "terminal".to_string(),
                    header: SectionHeader {
                        icon: None,
                        title: StyledText::plain("Terminal"),
                        badge: None,
                        actions: Vec::new(),
                        show_chevron: true,
                    },
                    body: SectionBody::Terminal(Terminal {
                        id: WidgetId::new("shell"),
                        cells: vec![vec![TerminalCell {
                            text: "$".to_string(),
                            fg: Color::rgb(200, 200, 200),
                            bg: Color::rgb(0, 0, 0),
                            bold: false,
                            italic: false,
                            underline: false,
                            dim: false,
                            selected: false,
                            is_cursor: false,
                            cursor_shape: Default::default(),
                            cursor_blinking: false,
                            is_find_match: false,
                            is_find_active: false,
                        }]],
                        scrollbar: None,
                    }),
                    aux: None,
                    size: SectionSize::Weight(1.0),
                    collapsed: false,
                    min_size: None,
                    max_size: None,
                },
                Section {
                    id: "messages".to_string(),
                    header: SectionHeader {
                        icon: None,
                        title: StyledText::plain("Messages"),
                        badge: None,
                        actions: Vec::new(),
                        show_chevron: true,
                    },
                    body: SectionBody::MessageList(MessageList {
                        id: WidgetId::new("log"),
                        rows: vec![MessageRow::new("Connected", Color::rgb(0, 200, 0), 0.0)],
                        scroll_top: 0,
                    }),
                    aux: None,
                    size: SectionSize::Weight(1.0),
                    collapsed: false,
                    min_size: None,
                    max_size: None,
                },
                Section {
                    id: "chart".to_string(),
                    header: SectionHeader {
                        icon: None,
                        title: StyledText::plain("Chart"),
                        badge: None,
                        actions: Vec::new(),
                        show_chevron: true,
                    },
                    body: SectionBody::Chart(Chart {
                        id: WidgetId::new("cpu"),
                        kind: ChartKind::Sparkline,
                        series: vec![Series {
                            label: "cpu".to_string(),
                            data: vec![0.1, 0.4, 0.2],
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
                    }),
                    aux: None,
                    size: SectionSize::Weight(1.0),
                    collapsed: false,
                    min_size: None,
                    max_size: None,
                },
            ],
            active_section: None,
            axis: Axis::Vertical,
            allow_resize: false,
            allow_collapse: true,
            scroll_mode: ScrollMode::PerSection,
            has_focus: false,
            panel_scroll: 0.0,
        };
        let frame = frame_with(vec![Surface::MultiSectionView {
            rect: Rect::new(0.0, 0.0, 40.0, 30.0),
            view,
        }]);
        let tree = frame_to_tree(&frame, &FocusManager::new());
        assert_eq!(
            tree.to_snapshot(),
            "window \"quadraui\" [focused]\n  \
             group \"Multi-section view\" [size=6]\n    \
             group \"Tree\" [expanded pos=1/6]\n      \
             tree [size=1]\n        \
             tree_item \"root\" [level=1 pos=1/1]\n    \
             group \"List\" [expanded pos=2/6]\n      \
             list \"Notes\" [size=1]\n        \
             list_item \"Buy milk\" [pos=1/1]\n    \
             group \"Form\" [expanded pos=3/6]\n      \
             form \"Form\" [size=0]\n    \
             group \"Terminal\" [expanded pos=4/6]\n      \
             terminal [value=\"$\"]\n    \
             group \"Messages\" [expanded pos=5/6]\n      \
             list \"Message list\" [size=1]\n        \
             list_item \"Connected\" [pos=1/1]\n    \
             group \"Chart\" [expanded pos=6/6]\n      \
             chart\n"
        );
    }

    #[test]
    fn completions_maps_items_with_detail_value_and_position() {
        let completions = Completions {
            id: WidgetId::new("popup"),
            items: vec![
                CompletionItem {
                    label: StyledText::plain("println!"),
                    detail: Some(StyledText::plain("macro")),
                    documentation: None,
                    kind: CompletionKind::Snippet,
                    icon: None,
                },
                CompletionItem {
                    label: StyledText::plain("print"),
                    detail: None,
                    documentation: None,
                    kind: CompletionKind::Function,
                    icon: None,
                },
                CompletionItem {
                    label: StyledText::plain("panic!"),
                    detail: Some(StyledText::plain("macro")),
                    documentation: None,
                    kind: CompletionKind::Snippet,
                    icon: None,
                },
            ],
            selected_idx: 1,
            scroll_offset: 0,
            has_focus: false,
        };
        let frame = frame_with(vec![Surface::Completions {
            completions,
            layout: CompletionsLayout {
                bounds: Rect::new(0.0, 0.0, 30.0, 6.0),
                placement: CompletionsPlacement::Below,
                visible_items: Vec::new(),
                hit_regions: Vec::new(),
                resolved_scroll_offset: 0,
            },
        }]);
        let tree = frame_to_tree(&frame, &FocusManager::new());
        assert_eq!(
            tree.to_snapshot(),
            "window \"quadraui\" [focused]\n  \
             list \"Completions\" [size=3]\n    \
             list_item \"println!\" [value=\"macro\" pos=1/3]\n    \
             list_item \"print\" [selected pos=2/3]\n    \
             list_item \"panic!\" [value=\"macro\" pos=3/3]\n"
        );
    }

    #[test]
    fn palette_maps_items_with_level_expanded_and_position() {
        let palette = Palette {
            id: WidgetId::new("cmds"),
            title: "Commands".to_string(),
            query: String::new(),
            query_cursor: 0,
            items: vec![
                PaletteItem {
                    text: StyledText::plain("src"),
                    detail: None,
                    icon: None,
                    match_positions: Vec::new(),
                    depth: 0,
                    expandable: true,
                    expanded: true,
                },
                PaletteItem {
                    text: StyledText::plain("main.rs"),
                    detail: Some(StyledText::plain("12 lines")),
                    icon: None,
                    match_positions: Vec::new(),
                    depth: 1,
                    expandable: false,
                    expanded: false,
                },
            ],
            selected_idx: 1,
            scroll_offset: 0,
            total_count: 0,
            has_focus: false,
            show_query: true,
            create_label: None,
            preview: None,
            mode: PaletteMode::default(),
        };
        let frame = frame_with(vec![Surface::Palette {
            rect: Rect::new(0.0, 0.0, 30.0, 10.0),
            palette,
        }]);
        let tree = frame_to_tree(&frame, &FocusManager::new());
        assert_eq!(
            tree.to_snapshot(),
            "window \"quadraui\" [focused]\n  \
             list \"Commands\" [size=2]\n    \
             list_item \"src\" [expanded level=1 pos=1/2]\n    \
             list_item \"main.rs\" [selected value=\"12 lines\" level=2 pos=2/2]\n"
        );
    }
}
