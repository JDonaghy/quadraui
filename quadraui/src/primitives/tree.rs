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

use crate::event::Rect;
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

// ── D6 Layout API ───────────────────────────────────────────────────────────
//
// Per Decision D6 in `docs/BACKEND_TRAIT_PROPOSAL.md` §9: primitives return
// fully-resolved `Layout` structs; backends rasterise verbatim. Third
// primitive to gain the new shape after `TabBar` and `StatusBar`. TreeView
// is purely vertical — rows stack from `scroll_offset` until the viewport
// fills. Sub-row layout (chevron / icon / text / badge positions within a
// row) is still backend-owned in v1 because each backend has native
// conventions for those elements (see the A.1c lesson in PLAN.md: "When
// porting a primitive's draw function to a new backend, match the new
// backend's pre-migration row cadence, not the other backend's").

/// Per-row measurement supplied by the backend.
///
/// `height` is the row's height in the backend's native unit — 1 cell for
/// TUI, `line_height` or `line_height * 1.4` for GTK (leaves vs branches),
/// similar for other native backends.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TreeRowMeasure {
    pub height: f32,
}

impl TreeRowMeasure {
    pub fn new(height: f32) -> Self {
        Self { height }
    }
}

/// Resolved position of one visible tree row after layout.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VisibleTreeRow {
    /// Index into the original `TreeView.rows` Vec (absolute, not visible).
    pub row_idx: usize,
    /// Full row bounds. `height` is clipped to the viewport if the row
    /// would extend past the bottom edge.
    pub bounds: Rect,
}

/// Classification of a hit-test result. Chevron vs row-body is intentionally
/// not split in v1 — TUI doesn't distinguish (clicking anywhere on a branch
/// row toggles it), and GTK's chevron hit-test can be derived from the row's
/// `bounds` + `indent` levels when it migrates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TreeViewHit {
    /// Click landed on a row. Carries the `row_idx` for `TreeView.rows`.
    Row(usize),
    /// Click landed in the viewport's empty region (below the last row).
    Empty,
}

/// Fully-resolved tree-view layout. Backends iterate `visible_rows` for
/// painting and call [`Self::hit_test`] for clicks.
#[derive(Debug, Clone, PartialEq)]
pub struct TreeViewLayout {
    /// Viewport width in the measurer's unit.
    pub viewport_width: f32,
    /// Viewport height in the measurer's unit.
    pub viewport_height: f32,
    /// Rows that are at least partially visible, top to bottom.
    pub visible_rows: Vec<VisibleTreeRow>,
    /// Ordered hit-region list. One region per visible row.
    pub hit_regions: Vec<(Rect, TreeViewHit)>,
    /// Scroll offset actually used. Clamped to `[0, rows.len())` so the
    /// backend never iterates past the end of the row slice.
    pub resolved_scroll_offset: usize,
}

impl TreeViewLayout {
    /// Test which row (if any) contains point `(x, y)`. Returns
    /// `TreeViewHit::Empty` when the point is below the last visible row.
    pub fn hit_test(&self, x: f32, y: f32) -> TreeViewHit {
        for (rect, hit) in &self.hit_regions {
            if x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height {
                return hit.clone();
            }
        }
        TreeViewHit::Empty
    }
}

impl TreeView {
    /// Compute the full rendering + hit-test layout for this tree.
    ///
    /// Per D6: layout decisions live here; backends consume the returned
    /// `TreeViewLayout` verbatim — iterate `visible_rows` for painting;
    /// call `hit_test` for clicks. Sub-row elements (chevron, icon, text,
    /// badge) are still backend-owned in v1 because their positions
    /// depend heavily on native conventions (TUI char cells vs GTK Pango
    /// pixel metrics).
    ///
    /// # Arguments
    ///
    /// - `viewport_width`, `viewport_height` — available area in the
    ///   measurer's unit.
    /// - `measure_row(i)` — height for row `i` (index into `self.rows`).
    ///   Receives the row index (not the row itself) so backends can
    ///   vary height by decoration, indent, or other row state they know
    ///   about via their copy of `self.rows`.
    ///
    /// # Row clipping
    ///
    /// The last visible row's `bounds.height` is clipped to whatever
    /// fits inside the viewport. Backends that want to skip partially-
    /// visible rows can check `row.bounds.height < measure_row(row.row_idx).height`.
    pub fn layout<F>(
        &self,
        viewport_width: f32,
        viewport_height: f32,
        measure_row: F,
    ) -> TreeViewLayout
    where
        F: Fn(usize) -> TreeRowMeasure,
    {
        let mut visible_rows: Vec<VisibleTreeRow> = Vec::new();
        let mut hit_regions: Vec<(Rect, TreeViewHit)> = Vec::new();

        // Clamp scroll_offset to a valid starting index. If scroll_offset
        // >= rows.len(), the loop below yields nothing — which is fine,
        // but we still report the clamped value so the app can write it
        // back and self-correct.
        let resolved_scroll_offset = if self.rows.is_empty() {
            0
        } else {
            self.scroll_offset.min(self.rows.len() - 1)
        };

        let mut y = 0.0_f32;
        for i in resolved_scroll_offset..self.rows.len() {
            if y >= viewport_height {
                break;
            }
            let m = measure_row(i);
            // Clip the last row's height to fit inside the viewport.
            let remaining = viewport_height - y;
            let height = m.height.min(remaining).max(0.0);
            if height <= 0.0 {
                break;
            }
            let bounds = Rect::new(0.0, y, viewport_width, height);
            visible_rows.push(VisibleTreeRow { row_idx: i, bounds });
            hit_regions.push((bounds, TreeViewHit::Row(i)));
            y += m.height;
        }

        TreeViewLayout {
            viewport_width,
            viewport_height,
            visible_rows,
            hit_regions,
            resolved_scroll_offset,
        }
    }
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
