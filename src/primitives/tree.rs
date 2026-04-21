//! `TreeView` primitive: hierarchical rows with expand/collapse, optional
//! icons, styled text, badges, and keyboard-driven selection.
//!
//! Trees are pre-flattened by the app: each `TreeRow` carries its
//! `TreePath`, visual `indent`, and an `is_expanded` flag (`None` for
//! leaves). Backends iterate `rows` in order; the primitive does not store
//! tree structure of its own. This keeps the data model plain and
//! plugin-friendly while letting apps control exactly which rows are
//! visible at any given frame.
//!
//! # Backend contract
//!
//! **Purely declarative** — render `rows[scroll_offset..]` until the
//! viewport is full. Click → row index → emit `TreeEvent::RowActivated`
//! with the row's `path`. Keyboard navigation (`j`/`k`/`h`/`l`/`Enter`)
//! emits the corresponding event; the *app* updates `selected_path` and
//! `scroll_offset` for the next frame.
//!
//! No measurement-dependent state — backends pick a uniform row height
//! (often `line_height` for leaves, `line_height * 1.4` for branches in
//! GUI backends, exactly `1` cell for TUI). Per-backend row cadence is
//! allowed; the primitive only constrains data shape.
//!
//! Apps that need "scroll selection into view" do it themselves by
//! adjusting `scroll_offset` based on the selected row's flat index and
//! the viewport row count.

use crate::types::{
    Badge, Decoration, Icon, Modifiers, SelectionMode, StyledText, TreePath, TreeStyle, WidgetId,
};
use serde::{Deserialize, Serialize};

/// Declarative description of a `TreeView` widget.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TreeView {
    pub id: WidgetId,
    /// Pre-flattened, pre-expanded rows in visual order.
    pub rows: Vec<TreeRow>,
    pub selection_mode: SelectionMode,
    pub selected_path: Option<TreePath>,
    /// How many rows have been scrolled past (app-owned in v1; primitive-owned
    /// scroll state with `ScrollState::id(widget_id)` is a later stage per
    /// `docs/UI_CRATE_DESIGN.md` §3.1).
    #[serde(default)]
    pub scroll_offset: usize,
    pub style: TreeStyle,
    #[serde(default)]
    pub has_focus: bool,
}

/// One visible row in a `TreeView`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TreeRow {
    pub path: TreePath,
    /// Visual indent level in `style.indent` units. Usually equals
    /// `path.len() - 1` but apps may flatten (e.g. show a child as indent 0
    /// when rendering a subtree in isolation).
    pub indent: u16,
    pub icon: Option<Icon>,
    pub text: StyledText,
    /// Right-aligned status indicator (e.g. git status letter, item count).
    pub badge: Option<Badge>,
    /// `None` marks a leaf; `Some(true)` marks an expanded branch;
    /// `Some(false)` marks a collapsed branch.
    pub is_expanded: Option<bool>,
    #[serde(default)]
    pub decoration: Decoration,
}

/// Events a `TreeView` emits back to the app.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TreeEvent {
    /// Single-click (or Enter on the keyboard) on a row.
    RowClicked {
        path: TreePath,
        modifiers: Modifiers,
    },
    /// Double-click on a row (typically "open" / "activate").
    RowDoubleClicked { path: TreePath },
    /// The chevron was clicked, or Space/arrow-keys expanded/collapsed a branch.
    RowToggleExpand { path: TreePath },
    /// Keyboard selection moved to a new row.
    SelectionChanged { path: TreePath },
    /// A key was pressed while the tree had focus and the primitive did not
    /// consume it. App may interpret it (e.g. `s` stages a file).
    KeyPressed { key: String, modifiers: Modifiers },
}
