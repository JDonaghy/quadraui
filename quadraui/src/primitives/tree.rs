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
//! [`TreeStyle::row_height`](crate::types::TreeStyle::row_height) lets a
//! host pin that cadence to a fixed value instead of letting it float
//! with the editor font — see #623.
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
    /// When `Some`, backends render an inline text input in place of
    /// `text` and `badge`. The row's indent, icon, and chevron are
    /// still rendered normally.
    #[serde(default)]
    pub edit: Option<TreeRowEditState>,
}

/// Inline editing state for a tree row. When present on a `TreeRow`,
/// backends render a text input in place of the normal row label.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TreeRowEditState {
    pub text: String,
    /// Cursor position as a byte offset into `text`.
    pub cursor: usize,
    /// Selection anchor as a byte offset. When `Some(n)` and `n != cursor`,
    /// the range between anchor and cursor is selected.
    pub selection_anchor: Option<usize>,
    /// Shown in muted style when `text` is empty (e.g. "New file name...").
    #[serde(default)]
    pub placeholder: Option<String>,
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
///
/// `chevron_end_x` — when `Some(w)` and the row has `is_expanded.is_some()`,
/// [`TreeView::layout`] splits the row's hit region into a
/// [`TreeViewHit::Chevron`] zone for `x ∈ [0, w)` and a
/// [`TreeViewHit::Row`] zone for the remainder. Backends set this to the
/// x coordinate (in tree-local units) where the painted chevron ends.
/// `None` means no chevron split (leaf rows, or `show_chevrons = false`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TreeRowMeasure {
    pub height: f32,
    pub chevron_end_x: Option<f32>,
}

impl TreeRowMeasure {
    pub fn new(height: f32) -> Self {
        Self {
            height,
            chevron_end_x: None,
        }
    }

    /// Convenience constructor: row with an explicit chevron boundary.
    pub fn with_chevron(height: f32, chevron_end_x: f32) -> Self {
        Self {
            height,
            chevron_end_x: Some(chevron_end_x),
        }
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

/// Classification of a hit-test result.
///
/// When [`TreeRowMeasure::chevron_end_x`] is set for a branch row the layout
/// emits two adjacent hit regions for that row: a [`Chevron`] region on the
/// left (covering the painted expand/collapse glyph) and a [`Row`] region for
/// the remainder. Backends that do not distinguish the two leave
/// `chevron_end_x` as `None`; clicking anywhere on the row returns `Row`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TreeViewHit {
    /// Click landed on the body portion of a row.
    /// Carries the `row_idx` into `TreeView.rows`.
    Row(usize),
    /// Click landed on the expand/collapse chevron of a branch row.
    /// Carries the same `row_idx` as the companion `Row` region.
    Chevron(usize),
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

        // Clamp scroll_offset to a valid starting index; we still report
        // the clamped value so the app can write it back and self-correct.
        let resolved_scroll_offset =
            crate::primitives::scrollbar::clamp_scroll_offset(self.scroll_offset, self.rows.len());

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
            // Split the hit region into Chevron + Row when the backend
            // supplied a chevron boundary for this branch row.
            let row = &self.rows[i];
            if let (Some(_), Some(chev_x)) = (row.is_expanded, m.chevron_end_x) {
                let chev_x = chev_x.clamp(0.0, viewport_width);
                if chev_x > 0.0 {
                    hit_regions.push((Rect::new(0.0, y, chev_x, height), TreeViewHit::Chevron(i)));
                }
                let body_w = (viewport_width - chev_x).max(0.0);
                if body_w > 0.0 {
                    hit_regions.push((Rect::new(chev_x, y, body_w, height), TreeViewHit::Row(i)));
                }
            } else {
                hit_regions.push((bounds, TreeViewHit::Row(i)));
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tree_view_roundtrip_serde() {
        let tree = TreeView {
            id: WidgetId::new("sc"),
            rows: vec![TreeRow {
                path: vec![0],
                indent: 0,
                icon: None,
                text: StyledText::plain("Staged Changes"),
                badge: Some(Badge::plain("3")),
                is_expanded: Some(true),
                decoration: Decoration::Normal,
                edit: None,
            }],
            selection_mode: SelectionMode::Single,
            selected_path: Some(vec![0]),
            scroll_offset: 0,
            style: TreeStyle::default(),
            has_focus: true,
        };
        let json = serde_json::to_string(&tree).unwrap();
        let back: TreeView = serde_json::from_str(&json).unwrap();
        assert_eq!(tree, back);
    }

    /// #623: `TreeStyle::row_height` is additive, not a required field.
    ///
    /// A `TreeStyle` serialised before #623 (or hand-written by a Lua
    /// plugin, per the `types.rs` design invariant that every primitive
    /// is plugin-constructible from JSON) carries no `row_height` key at
    /// all. `#[serde(default)]` must decode that as `None` — i.e. "keep
    /// deriving the row pitch from `line_height`" — rather than failing
    /// the whole `TreeView` deserialisation with a missing-field error.
    #[test]
    fn tree_style_row_height_defaults_to_none_when_absent_from_json() {
        let legacy = r#"{
            "indent": 2,
            "show_chevrons": true,
            "chevron_expanded": "▾",
            "chevron_collapsed": "▸"
        }"#;
        let style: TreeStyle =
            serde_json::from_str(legacy).expect("pre-#623 TreeStyle must decode");
        assert_eq!(
            style.row_height, None,
            "absent row_height means 'derive from line_height', not a decode error"
        );
        assert_eq!(style, TreeStyle::default());
    }

    /// #623: an explicit override survives a serde round-trip, so a host
    /// that pins the row pitch keeps it across a save/restore of its UI
    /// state (and across the plugin JSON boundary).
    #[test]
    fn tree_style_row_height_override_roundtrips_serde() {
        let style = TreeStyle {
            row_height: Some(22),
            ..TreeStyle::default()
        };
        let json = serde_json::to_string(&style).unwrap();
        let back: TreeStyle = serde_json::from_str(&json).unwrap();
        assert_eq!(back.row_height, Some(22));
        assert_eq!(style, back);
    }

    // ── D6 TreeView layout API tests ──────────────────────────────────

    fn make_tree_row(path: &[u16], indent: u16, label: &str) -> TreeRow {
        TreeRow {
            path: path.to_vec(),
            indent,
            icon: None,
            text: StyledText::plain(label),
            badge: None,
            is_expanded: None,
            decoration: Decoration::Normal,
            edit: None,
        }
    }

    fn make_tree(rows: Vec<TreeRow>, scroll: usize) -> TreeView {
        TreeView {
            id: WidgetId::new("t"),
            rows,
            selection_mode: SelectionMode::Single,
            selected_path: None,
            scroll_offset: scroll,
            style: TreeStyle::default(),
            has_focus: true,
        }
    }

    #[test]
    fn tree_view_layout_empty() {
        let tree = make_tree(vec![], 0);
        let layout = tree.layout(40.0, 20.0, |_| TreeRowMeasure::new(1.0));
        assert_eq!(layout.visible_rows.len(), 0);
        assert_eq!(layout.hit_regions.len(), 0);
        assert_eq!(layout.resolved_scroll_offset, 0);
        assert_eq!(layout.hit_test(5.0, 5.0), TreeViewHit::Empty);
    }

    #[test]
    fn tree_view_layout_all_rows_fit() {
        let tree = make_tree(
            (0..3)
                .map(|i| make_tree_row(&[i], 0, &format!("row{i}")))
                .collect(),
            0,
        );
        let layout = tree.layout(40.0, 10.0, |_| TreeRowMeasure::new(1.0));
        assert_eq!(layout.visible_rows.len(), 3);
        assert_eq!(layout.visible_rows[0].bounds.y, 0.0);
        assert_eq!(layout.visible_rows[1].bounds.y, 1.0);
        assert_eq!(layout.visible_rows[2].bounds.y, 2.0);
        // Hit-test each row by y coord.
        assert_eq!(layout.hit_test(10.0, 0.5), TreeViewHit::Row(0));
        assert_eq!(layout.hit_test(10.0, 1.5), TreeViewHit::Row(1));
        assert_eq!(layout.hit_test(10.0, 2.5), TreeViewHit::Row(2));
        // Below last row → Empty.
        assert_eq!(layout.hit_test(10.0, 5.0), TreeViewHit::Empty);
    }

    #[test]
    fn tree_view_layout_scroll_offset_applies() {
        let tree = make_tree(
            (0..5)
                .map(|i| make_tree_row(&[i], 0, &format!("row{i}")))
                .collect(),
            2, // skip first 2
        );
        let layout = tree.layout(40.0, 10.0, |_| TreeRowMeasure::new(1.0));
        assert_eq!(layout.resolved_scroll_offset, 2);
        assert_eq!(layout.visible_rows.len(), 3);
        assert_eq!(layout.visible_rows[0].row_idx, 2);
        assert_eq!(layout.visible_rows[1].row_idx, 3);
        assert_eq!(layout.visible_rows[2].row_idx, 4);
    }

    #[test]
    fn tree_view_layout_viewport_overflow_clips() {
        // 10 rows of height 2.0 each; viewport 5.0 tall → only 3 rows
        // fit (one partially clipped).
        let tree = make_tree(
            (0..10)
                .map(|i| make_tree_row(&[i], 0, &format!("row{i}")))
                .collect(),
            0,
        );
        let layout = tree.layout(40.0, 5.0, |_| TreeRowMeasure::new(2.0));
        // Rows 0 (y=0..2), 1 (y=2..4), 2 (y=4..5 clipped) — three visible.
        assert_eq!(layout.visible_rows.len(), 3);
        // Last row clipped to height 1.0 (remaining = 5 - 4 = 1).
        assert_eq!(layout.visible_rows[2].bounds.height, 1.0);
        // A click below all visible rows returns Empty.
        assert_eq!(layout.hit_test(10.0, 5.0), TreeViewHit::Empty);
    }

    #[test]
    fn tree_view_layout_varying_row_heights() {
        // Branches (is_expanded != None) get height 1.4 * base; leaves get
        // 1.0. Proves the measurer can consult row state.
        let mut rows = vec![
            make_tree_row(&[0], 0, "branch"),
            make_tree_row(&[0, 0], 1, "leaf0"),
            make_tree_row(&[0, 1], 1, "leaf1"),
        ];
        rows[0].is_expanded = Some(true);
        let tree = make_tree(rows.clone(), 0);
        let layout = tree.layout(40.0, 10.0, |i| {
            let h = if rows[i].is_expanded.is_some() {
                1.4
            } else {
                1.0
            };
            TreeRowMeasure::new(h)
        });
        assert_eq!(layout.visible_rows.len(), 3);
        assert_eq!(layout.visible_rows[0].bounds.height, 1.4);
        assert_eq!(layout.visible_rows[1].bounds.y, 1.4);
        assert_eq!(layout.visible_rows[1].bounds.height, 1.0);
        assert!((layout.visible_rows[2].bounds.y - 2.4).abs() < 0.001);
    }

    #[test]
    fn tree_view_layout_pixel_units_fractional() {
        // GTK-style: line_height = 18.5 px leaf, 25.9 px branch. Proves
        // fractional row heights flow through correctly.
        let tree = make_tree(
            (0..5)
                .map(|i| make_tree_row(&[i], 0, &format!("r{i}")))
                .collect(),
            0,
        );
        let layout = tree.layout(300.0, 60.0, |_| TreeRowMeasure::new(18.5));
        // 3 full rows fit (55.5), 4th starts at y=55.5 and gets clipped to 4.5 px.
        assert_eq!(layout.visible_rows.len(), 4);
        assert!((layout.visible_rows[3].bounds.height - 4.5).abs() < 0.001);
    }

    #[test]
    fn tree_view_layout_scroll_offset_clamped() {
        // scroll_offset beyond rows.len() — resolved to rows.len()-1 so
        // the single remaining row is visible.
        let tree = make_tree(
            (0..3)
                .map(|i| make_tree_row(&[i], 0, &format!("r{i}")))
                .collect(),
            99,
        );
        let layout = tree.layout(40.0, 10.0, |_| TreeRowMeasure::new(1.0));
        assert_eq!(layout.resolved_scroll_offset, 2);
        assert_eq!(layout.visible_rows.len(), 1);
        assert_eq!(layout.visible_rows[0].row_idx, 2);
    }
}
