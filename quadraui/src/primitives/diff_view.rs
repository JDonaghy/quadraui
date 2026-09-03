//! `DiffView` primitive: a two-pane side-by-side or unified diff viewer.
//!
//! # Backend contract
//!
//! **Purely declarative** — hunks are app-computed via
//! `quadraui::diff::compute_hunks`; backends only render. The app calls
//! `compute_hunks(left, right)` whenever the inputs change, stores the
//! result in `DiffView::hunks`, and passes the struct to the backend for
//! rasterisation. Backends never recompute the diff.
//!
//! # Scroll model
//!
//! A single `scroll_offset` drives lock-step scrolling of both panes.
//! The offset counts display rows from the beginning of the first hunk.
//! Per-pane independent scrolling is out of scope for v1; the planned
//! additive field is `right_scroll_offset: Option<usize>`.
//!
//! # Editability
//!
//! `DiffEditability::RightEditable` signals to consumers that the right
//! pane should accept edits on TUI. Events in this architecture flow
//! from `AppLogic::handle`, not from rasterisers — the app processes key
//! input, mutates `DiffView::right`, and recomputes hunks itself. Full
//! text-input machinery (cursor, insertion, deletion) is a follow-up
//! story; on GTK v1 the right pane remains read-only regardless of this
//! setting.
//!
//! # Distinct from `Editor` with `diff_status`
//!
//! `DiffView` renders a self-contained diff primitive from pre-computed
//! hunks. The `Editor` primitive decorates live buffer content with
//! per-line gutter colours. They are not interchangeable.

use crate::types::WidgetId;
use serde::{Deserialize, Serialize};

/// The kind of change a [`DiffRow`] represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiffRowKind {
    /// Line is identical on both sides.
    Same,
    /// Left side has a line, right side has a different line (modified).
    Changed,
    /// Left side has a line, right side is empty (deleted from right).
    Removed,
    /// Right side has a line, left side is empty (added on right).
    Added,
}

/// A single display row in a [`DiffHunk`].
///
/// Either side may be `None` (padding row) when one side has more lines
/// in a change run than the other.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffRow {
    /// Content from the left (original) file, or `None` for padding.
    pub left: Option<String>,
    /// Content from the right (modified) file, or `None` for padding.
    pub right: Option<String>,
    /// Change kind — drives background / foreground colour selection.
    pub kind: DiffRowKind,
}

/// A contiguous group of rows to display, bracketed by context lines.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffHunk {
    /// 1-based line number of the first row's left content in the original
    /// left text. Used to render `@@ -left_start,N +right_start,M @@` headers
    /// in unified mode.
    pub left_start: usize,
    /// 1-based line number of the first row's right content in the right text.
    pub right_start: usize,
    /// Ordered display rows for this hunk (context + changed lines).
    pub rows: Vec<DiffRow>,
}

/// Display mode for the diff widget.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum DiffMode {
    /// Two-column layout: left (original) on the left, right (modified) on
    /// the right, separated by a centre divider.
    #[default]
    SideBySide,
    /// Single-column layout with `+`/`-`/` ` prefixes. Hunk headers are
    /// rendered as `@@ -l,n +r,m @@` lines.
    Unified,
}

/// Whether the right pane of the diff is editable.
///
/// `RightEditable` signals to consumers that the right pane should accept
/// edits on TUI. Events in this architecture flow from `AppLogic::handle`,
/// not from rasterisers — the app processes key input and mutates the
/// right text itself. Full text-input machinery (cursor, insertion,
/// deletion) ships in a follow-up. On GTK v1 this setting is accepted but
/// the right pane renders read-only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum DiffEditability {
    /// Both panes are read-only (default).
    #[default]
    ReadOnly,
    /// Right pane accepts edits on TUI; the app mutates
    /// [`DiffView::right`] from `AppLogic::handle` after processing
    /// each edit key. GTK v1 renders as read-only; full editing
    /// machinery is a follow-up.
    RightEditable,
}

/// Which pane has keyboard focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum DiffPane {
    /// Left (original) pane.
    #[default]
    Left,
    /// Right (modified) pane.
    Right,
}

/// Layout information returned by `draw_diff_view`.
///
/// Used by `AppLogic` to clamp scroll offsets after a resize.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiffViewLayout {
    /// Number of content rows that fit on screen (after the optional header row).
    pub visible_rows: usize,
    /// Total number of display lines the backend used for rendering.
    ///
    /// In **side-by-side** mode this equals `DiffView::total_rows()` (one
    /// display line per `DiffRow`).
    ///
    /// In **unified** mode each hunk contributes an extra `@@ … @@` header
    /// line, so `total_rows = view.total_rows() + hunk_count`.  Always use
    /// this field — not `DiffView::total_rows()` — when clamping scroll:
    /// ```ignore
    /// view.scroll_offset = view.scroll_offset
    ///     .min(layout.total_rows.saturating_sub(layout.visible_rows));
    /// ```
    pub total_rows: usize,
}

/// The `DiffView` primitive: a two-pane or unified diff viewer.
///
/// Apps build this from pre-computed [`DiffHunk`]s (via
/// `quadraui::diff::compute_hunks`) and pass it to `backend.draw_diff_view`.
/// The backend never recomputes the diff — it only rasterises.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffView {
    /// Stable identifier for this widget instance.
    pub id: WidgetId,
    /// Original (left-side) text. Source for `compute_hunks`.
    ///
    /// Carried alongside the pre-computed `hunks` so the view is the
    /// canonical single-struct state: an edited-right consumer can
    /// reconstruct the new right buffer and re-diff against `left`
    /// without threading extra strings out-of-band, and a serialised
    /// `DiffView` can re-compute `hunks` if the cached copy is stale.
    pub left: String,
    /// Proposed (right-side) text. Source for `compute_hunks`. Mutated
    /// by the app when `editability == RightEditable`.
    pub right: String,
    /// Optional label shown above the left pane (e.g. file path / branch name).
    pub left_label: Option<String>,
    /// Optional label shown above the right pane.
    pub right_label: Option<String>,
    /// Pre-computed diff hunks. Set by calling
    /// `quadraui::diff::compute_hunks(&self.left, &self.right)`.
    pub hunks: Vec<DiffHunk>,
    /// Display mode — side-by-side (default) or unified.
    pub mode: DiffMode,
    /// Editability of the right pane. Currently all backends render as read-only.
    pub editability: DiffEditability,
    /// Row scroll offset (from the top of the first hunk).
    pub scroll_offset: usize,
    /// Which pane currently has keyboard focus.
    pub focused_pane: DiffPane,
    /// Whether the widget as a whole has keyboard focus.
    ///
    /// **Currently unused by all backends.** Reserved for a future focus
    /// border; setting this to `true` produces no visual change in v1.
    pub has_focus: bool,
}

impl DiffView {
    /// Total number of display rows across all hunks.
    ///
    /// Use this to clamp `scroll_offset` after the hunk list changes:
    /// `view.scroll_offset = view.scroll_offset.min(view.total_rows().saturating_sub(1))`.
    pub fn total_rows(&self) -> usize {
        self.hunks.iter().map(|h| h.rows.len()).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::WidgetId;

    /// Asserts the default field values for a freshly-constructed `DiffView`.
    #[test]
    fn diff_view_field_defaults() {
        let view = DiffView {
            id: WidgetId::new("test"),
            left: String::new(),
            right: String::new(),
            left_label: None,
            right_label: None,
            hunks: vec![],
            mode: DiffMode::default(),
            editability: DiffEditability::default(),
            scroll_offset: 0,
            focused_pane: DiffPane::default(),
            has_focus: false,
        };
        assert_eq!(view.mode, DiffMode::SideBySide);
        assert_eq!(view.editability, DiffEditability::ReadOnly);
        assert_eq!(view.focused_pane, DiffPane::Left);
        assert_eq!(view.scroll_offset, 0);
        assert!(!view.has_focus);
        assert_eq!(view.total_rows(), 0);
    }

    /// `DiffView::total_rows` correctly sums across multiple hunks.
    #[test]
    fn total_rows_sums_hunks() {
        let make_hunk = |n: usize| DiffHunk {
            left_start: 1,
            right_start: 1,
            rows: (0..n)
                .map(|_| DiffRow {
                    left: Some("x".into()),
                    right: Some("x".into()),
                    kind: DiffRowKind::Same,
                })
                .collect(),
        };
        let view = DiffView {
            id: WidgetId::new("t"),
            left: String::new(),
            right: String::new(),
            left_label: None,
            right_label: None,
            hunks: vec![make_hunk(3), make_hunk(5)],
            mode: DiffMode::default(),
            editability: DiffEditability::default(),
            scroll_offset: 0,
            focused_pane: DiffPane::default(),
            has_focus: false,
        };
        assert_eq!(view.total_rows(), 8);
    }

    /// The acceptance-criterion construction from issue #294 must compile
    /// and behave: the user writes `left` + `right` strings, calls
    /// `compute_hunks`, and the resulting struct carries everything.
    #[test]
    fn acceptance_criterion_constructor_compiles_and_runs() {
        use crate::diff::compute_hunks;
        let left = "a\nb\n";
        let right = "a\nc\n";
        let view = DiffView {
            id: WidgetId::new("acceptance"),
            left: left.into(),
            right: right.into(),
            left_label: None,
            right_label: None,
            hunks: compute_hunks(left, right),
            mode: DiffMode::SideBySide,
            editability: DiffEditability::ReadOnly,
            scroll_offset: 0,
            focused_pane: DiffPane::Left,
            has_focus: false,
        };
        // Sanity: hunks were computed and the source strings round-trip
        // through the struct without modification.
        assert!(!view.hunks.is_empty(), "expected at least one hunk");
        assert_eq!(view.left, "a\nb\n");
        assert_eq!(view.right, "a\nc\n");
    }
}
