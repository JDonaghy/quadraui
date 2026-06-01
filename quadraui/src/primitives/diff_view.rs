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
//! `DiffEditability::RightEditable` is defined in the API but renders
//! as read-only on GTK v1. Full in-widget editing is a follow-up story.
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
/// `RightEditable` is defined here for future use but all v1 backends
/// render it as read-only — full in-widget editing is a follow-up story.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum DiffEditability {
    /// Both panes are read-only (default).
    #[default]
    ReadOnly,
    /// Right pane accepts edits (reserved for v2; currently rendered
    /// read-only by all backends).
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

/// Events emitted by a [`DiffView`] interaction.
///
/// Keyboard events are handled by `AppLogic::handle`; these events are
/// produced when the app logic mutates the view and wants to notify
/// observers (e.g. a scroll event triggers a parent layout recalculation).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffViewEvent {
    /// The scroll position changed.
    Scrolled { offset: usize },
    /// The focused pane changed.
    PaneSwitched { pane: DiffPane },
    /// Text was copied from the view.
    Copied { text: String },
}

/// Layout information returned by `draw_diff_view`.
///
/// Used by `AppLogic` to clamp scroll offsets after a resize.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiffViewLayout {
    /// Number of content rows that fit on screen (after the optional header row).
    pub visible_rows: usize,
    /// Total number of display rows across all hunks.
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
    /// Optional label shown above the left pane (e.g. file path / branch name).
    pub left_label: Option<String>,
    /// Optional label shown above the right pane.
    pub right_label: Option<String>,
    /// Pre-computed diff hunks. Set by calling
    /// `quadraui::diff::compute_hunks(left, right)`.
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
}
