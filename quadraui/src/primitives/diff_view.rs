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
//!
//! # Shared layout (issue #737)
//!
//! Row/pane geometry — pane widths, the divider position, the optional
//! header strip, and the scroll-clamped visible-line window — used to be
//! re-derived independently by every backend's `draw_diff_view` (and the
//! `row kind → colour` tables three times over). [`DiffView::layout`] is
//! now the single source of that geometry, and [`row_colors`] /
//! [`unified_row_style`] / [`unified_row_text`] / [`unified_hunk_header`]
//! are the single source of colour/text selection. Every backend (gtk,
//! macos, tui, win) calls these instead of carrying its own copy.

use crate::event::Rect;
use crate::theme::Theme;
use crate::types::{Color, WidgetId};
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

/// The `row kind → colour` table for **side-by-side** mode — shared by
/// every backend's rasteriser (gtk, macos, tui, win). This table existed
/// three times over (`gtk::diff_view::row_colors_gtk`,
/// `macos::diff_view::row_colors`, `tui::diff_view::row_colors`) before
/// issue #737; #713's primitive-first rule forbids a fourth copy, so this
/// single function is the only definition and every backend converts its
/// `Color` output to its own native colour type at the call site (`qc` on
/// TUI, `cairo_rgb` on GTK, a passthrough on macOS/Win).
///
/// Returns `(left_fg, left_bg, right_fg, right_bg)`.
pub fn row_colors(kind: DiffRowKind, theme: &Theme) -> (Color, Color, Color, Color) {
    match kind {
        DiffRowKind::Same => (
            theme.muted_fg,
            theme.background,
            theme.muted_fg,
            theme.background,
        ),
        DiffRowKind::Changed => (
            theme.git_deleted,
            theme.diff_removed_bg,
            theme.git_added,
            theme.diff_added_bg,
        ),
        DiffRowKind::Removed => (
            theme.git_deleted,
            theme.diff_removed_bg,
            theme.muted_fg,
            theme.diff_padding_bg,
        ),
        DiffRowKind::Added => (
            theme.muted_fg,
            theme.diff_padding_bg,
            theme.git_added,
            theme.diff_added_bg,
        ),
    }
}

/// The `row kind → (prefix, colour)` table for **unified** mode — the
/// single-column twin of [`row_colors`], likewise lifted out of three
/// backend copies (#737).
///
/// Returns `(prefix_char, fg, bg)`.
pub fn unified_row_style(kind: DiffRowKind, theme: &Theme) -> (char, Color, Color) {
    match kind {
        DiffRowKind::Same => (' ', theme.muted_fg, theme.background),
        DiffRowKind::Removed | DiffRowKind::Changed => {
            ('-', theme.git_deleted, theme.diff_removed_bg)
        }
        DiffRowKind::Added => ('+', theme.git_added, theme.diff_added_bg),
    }
}

/// The unified-mode content text for a [`DiffRow`]: the left text for a
/// `Removed` row, otherwise the right text falling back to the left
/// (`Changed`/`Added`/`Same`). Lifted out of three identical `match`
/// arms (#737).
pub fn unified_row_text(row: &DiffRow) -> &str {
    match row.kind {
        DiffRowKind::Removed => row.left.as_deref().unwrap_or(""),
        _ => row.right.as_deref().or(row.left.as_deref()).unwrap_or(""),
    }
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

/// Render a hunk's `@@ -l,n +r,m @@` unified-diff header line.
///
/// `-n` counts rows sourced from the LEFT file (`row.left.is_some()`),
/// `+m` from the RIGHT (`row.right.is_some()`) — these differ from
/// `hunk.rows.len()` whenever a change run produces padding rows (unequal
/// removed/added counts). Lifted out of three identical copies (#737).
pub fn unified_hunk_header(hunk: &DiffHunk) -> String {
    let left_count = hunk.rows.iter().filter(|r| r.left.is_some()).count();
    let right_count = hunk.rows.iter().filter(|r| r.right.is_some()).count();
    format!(
        "@@ -{},{} +{},{} @@",
        hunk.left_start, left_count, hunk.right_start, right_count
    )
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

/// Divider thickness between the left and right panes, in the backend's
/// native units (1 cell on TUI, 1 px/DIP everywhere else — every backend
/// used the same numeric value before #737, so it is a shared constant
/// rather than a fourth per-backend copy).
pub const DIFF_DIVIDER_W: f32 = 1.0;

/// Header strip geometry for **side-by-side** mode (only present when
/// `left_label`/`right_label` is set). Resolved once by [`DiffView::layout`]
/// rather than re-derived by each backend.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DiffHeaderGeometry {
    /// Bounds of the left label strip.
    pub left: Rect,
    /// Bounds of the right label strip.
    pub right: Rect,
    /// Bounds of the 1-unit divider segment within the header row.
    pub divider: Rect,
}

/// What a [`DiffDisplayLine`] renders: either a synthesized unified `@@ …
/// @@` hunk header, or a real diff row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffLineContent {
    /// A unified-mode hunk header. `hunk_idx` indexes [`DiffView::hunks`];
    /// render its text with [`unified_hunk_header`].
    UnifiedHeader {
        /// Index into [`DiffView::hunks`].
        hunk_idx: usize,
    },
    /// A real diff row. `row_idx` indexes [`DiffView::flat_rows`] — stable
    /// across both display modes, since row *content* doesn't change
    /// between side-by-side and unified, only how headers interleave.
    Row {
        /// Index into [`DiffView::flat_rows`].
        row_idx: usize,
    },
}

/// One on-screen line resolved by [`DiffView::layout`] — already
/// scroll-clamped, so `DiffView::layout(..).lines` is exactly the visible
/// window in paint order.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DiffDisplayLine {
    /// Full-width bounds of this line (used directly in unified mode; in
    /// side-by-side mode `left`/`right`/`divider` subdivide it instead).
    pub bounds: Rect,
    /// Left-pane bounds. `Some` only for [`DiffMode::SideBySide`] rows.
    pub left: Option<Rect>,
    /// Right-pane bounds. `Some` only for [`DiffMode::SideBySide`] rows.
    pub right: Option<Rect>,
    /// Pane-divider bounds. `Some` only for [`DiffMode::SideBySide`] rows.
    pub divider: Option<Rect>,
    /// What to render at this line.
    pub content: DiffLineContent,
}

/// Top-level pane geometry for **side-by-side** mode — present even when
/// `lines` is empty (e.g. an empty diff), so a backend painting trailing
/// blank rows or an empty-pane divider never needs to re-derive
/// `left_w`/`right_w`/`divider_x` by hand.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DiffPaneGeometry {
    /// Left-pane width.
    pub left_w: f32,
    /// Right-pane width.
    pub right_w: f32,
    /// Absolute x of the 1-unit divider column.
    pub divider_x: f32,
    /// Top of the content area (below the header strip, if any).
    pub content_y: f32,
    /// Height of the content area (viewport height minus the header strip).
    pub content_h: f32,
}

/// Fully-resolved `DiffView` geometry — the row-position/colour-agnostic
/// half of what every backend's `draw_diff_view` used to re-derive from
/// scratch (issue #737). Backends still own colour selection
/// ([`row_colors`] / [`unified_row_style`]) and text painting; this answers
/// only "where".
///
/// `lines` already reflects `scroll_offset` — it is the visible window,
/// not the full row list — so callers never re-implement the scroll-clamp
/// arithmetic.
#[derive(Debug, Clone, PartialEq)]
pub struct DiffViewGeometry {
    /// The viewport this geometry was computed for.
    pub bounds: Rect,
    /// Side-by-side header strip, when present.
    pub header: Option<DiffHeaderGeometry>,
    /// Side-by-side pane widths/divider position. `None` in
    /// [`DiffMode::Unified`], where there is only one column.
    pub panes: Option<DiffPaneGeometry>,
    /// Number of content rows that fit in the viewport.
    pub visible_rows: usize,
    /// Total number of display lines — same contract as
    /// [`DiffViewLayout::total_rows`] (content rows in side-by-side mode,
    /// `+ hunk_count` in unified mode).
    pub total_rows: usize,
    /// The visible window of display lines, top to bottom.
    pub lines: Vec<DiffDisplayLine>,
}

impl DiffViewGeometry {
    /// The [`DiffViewLayout`] scroll-clamp summary for this geometry —
    /// every backend's `draw_diff_view` returns this.
    pub fn as_layout(&self) -> DiffViewLayout {
        DiffViewLayout {
            visible_rows: self.visible_rows,
            total_rows: self.total_rows,
        }
    }
}

/// Resolve `(start, end)` — the half-open range of the flat display-line
/// list that is on-screen — from a scroll offset. Shared by both
/// [`DiffView::layout`] modes; was duplicated inline in every backend's
/// `draw_diff_view` before #737.
fn scroll_window(scroll_offset: usize, visible_rows: usize, total: usize) -> (usize, usize) {
    let start = scroll_offset.min(total.saturating_sub(1));
    let end = (start + visible_rows).min(total);
    (start, end)
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

    /// Flatten every hunk's rows into one list, in display order. The
    /// resulting index is what [`DiffLineContent::Row::row_idx`] refers
    /// to, in both [`DiffMode::SideBySide`] and [`DiffMode::Unified`].
    pub fn flat_rows(&self) -> Vec<&DiffRow> {
        self.hunks.iter().flat_map(|h| h.rows.iter()).collect()
    }

    /// Compute this view's row/pane geometry for a `viewport` of the given
    /// `line_height` (backend-native units: 1.0 for TUI cells, font pixel
    /// height for GTK/macOS/Win).
    ///
    /// This is the shared "where" half of what every backend's
    /// `draw_diff_view` used to re-derive independently (issue #737) —
    /// pane widths, the divider position, the header strip, and the
    /// scroll-clamped visible-line window. Backends still own colour
    /// selection ([`row_colors`] / [`unified_row_style`]) and text
    /// painting.
    ///
    /// Returns an empty (zero-`visible_rows`) geometry without painting
    /// implications when `viewport` or `line_height` is non-positive —
    /// same guard every pre-#737 backend used.
    pub fn layout(&self, viewport: Rect, line_height: f32) -> DiffViewGeometry {
        if viewport.width <= 0.0 || viewport.height <= 0.0 || line_height <= 0.0 {
            return DiffViewGeometry {
                bounds: viewport,
                header: None,
                panes: None,
                visible_rows: 0,
                total_rows: self.total_rows(),
                lines: Vec::new(),
            };
        }

        match self.mode {
            DiffMode::SideBySide => self.layout_side_by_side(viewport, line_height),
            DiffMode::Unified => self.layout_unified(viewport, line_height),
        }
    }

    fn layout_side_by_side(&self, viewport: Rect, line_height: f32) -> DiffViewGeometry {
        let has_header = self.left_label.is_some() || self.right_label.is_some();
        let header_h = if has_header { line_height } else { 0.0 };

        let left_w = ((viewport.width - DIFF_DIVIDER_W) / 2.0).floor();
        let right_w = (viewport.width - DIFF_DIVIDER_W - left_w).max(0.0);
        let divider_x = viewport.x + left_w;

        let header = has_header.then(|| DiffHeaderGeometry {
            left: Rect::new(viewport.x, viewport.y, left_w, header_h),
            right: Rect::new(divider_x + DIFF_DIVIDER_W, viewport.y, right_w, header_h),
            divider: Rect::new(divider_x, viewport.y, DIFF_DIVIDER_W, header_h),
        });

        let content_y = viewport.y + header_h;
        let content_h = (viewport.height - header_h).max(0.0);
        let visible_rows = (content_h / line_height).floor() as usize;

        let total_rows = self.total_rows();
        let (start, end) = scroll_window(self.scroll_offset, visible_rows, total_rows);

        let mut lines = Vec::with_capacity(end.saturating_sub(start));
        for (i, row_idx) in (start..end).enumerate() {
            let row_y = content_y + i as f32 * line_height;
            lines.push(DiffDisplayLine {
                bounds: Rect::new(viewport.x, row_y, viewport.width, line_height),
                left: Some(Rect::new(viewport.x, row_y, left_w, line_height)),
                right: Some(Rect::new(
                    divider_x + DIFF_DIVIDER_W,
                    row_y,
                    right_w,
                    line_height,
                )),
                divider: Some(Rect::new(divider_x, row_y, DIFF_DIVIDER_W, line_height)),
                content: DiffLineContent::Row { row_idx },
            });
        }

        DiffViewGeometry {
            bounds: viewport,
            header,
            panes: Some(DiffPaneGeometry {
                left_w,
                right_w,
                divider_x,
                content_y,
                content_h,
            }),
            visible_rows,
            total_rows,
            lines,
        }
    }

    fn layout_unified(&self, viewport: Rect, line_height: f32) -> DiffViewGeometry {
        let mut sequence: Vec<DiffLineContent> = Vec::new();
        let mut row_idx = 0usize;
        for (hunk_idx, hunk) in self.hunks.iter().enumerate() {
            sequence.push(DiffLineContent::UnifiedHeader { hunk_idx });
            for _ in &hunk.rows {
                sequence.push(DiffLineContent::Row { row_idx });
                row_idx += 1;
            }
        }
        let total_display = sequence.len();

        let visible_rows = (viewport.height / line_height).floor() as usize;
        let (start, end) = scroll_window(self.scroll_offset, visible_rows, total_display);

        let mut lines = Vec::with_capacity(end.saturating_sub(start));
        for (i, content) in sequence[start..end].iter().enumerate() {
            let row_y = viewport.y + i as f32 * line_height;
            lines.push(DiffDisplayLine {
                bounds: Rect::new(viewport.x, row_y, viewport.width, line_height),
                left: None,
                right: None,
                divider: None,
                content: *content,
            });
        }

        DiffViewGeometry {
            bounds: viewport,
            header: None,
            panes: None,
            visible_rows,
            total_rows: total_display,
            lines,
        }
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

    // ── #737: shared layout geometry ────────────────────────────────────

    fn two_hunk_view(mode: DiffMode) -> DiffView {
        DiffView {
            id: WidgetId::new("layout-test"),
            left: String::new(),
            right: String::new(),
            left_label: None,
            right_label: None,
            hunks: vec![
                DiffHunk {
                    left_start: 1,
                    right_start: 1,
                    rows: vec![
                        DiffRow {
                            left: Some("alpha".into()),
                            right: Some("ALPHA".into()),
                            kind: DiffRowKind::Changed,
                        },
                        DiffRow {
                            left: Some("beta".into()),
                            right: Some("beta".into()),
                            kind: DiffRowKind::Same,
                        },
                    ],
                },
                DiffHunk {
                    left_start: 10,
                    right_start: 10,
                    rows: vec![
                        DiffRow {
                            left: Some("gamma".into()),
                            right: None,
                            kind: DiffRowKind::Removed,
                        },
                        DiffRow {
                            left: None,
                            right: Some("delta".into()),
                            kind: DiffRowKind::Added,
                        },
                    ],
                },
            ],
            mode,
            editability: DiffEditability::ReadOnly,
            scroll_offset: 0,
            focused_pane: DiffPane::Left,
            has_focus: false,
        }
    }

    /// `flat_rows` concatenates every hunk's rows, in order, regardless of
    /// mode — the index [`DiffLineContent::Row::row_idx`] refers to.
    #[test]
    fn flat_rows_concatenates_every_hunk() {
        let view = two_hunk_view(DiffMode::SideBySide);
        let flat = view.flat_rows();
        assert_eq!(flat.len(), 4);
        assert_eq!(flat[0].left.as_deref(), Some("alpha"));
        assert_eq!(flat[3].right.as_deref(), Some("delta"));
    }

    /// Non-positive viewport or line height returns an empty geometry
    /// without dividing by zero or underflowing — same guard every
    /// pre-#737 backend implemented independently.
    #[test]
    fn zero_size_viewport_returns_empty_geometry() {
        let view = two_hunk_view(DiffMode::SideBySide);
        let geometry = view.layout(Rect::new(0.0, 0.0, 0.0, 10.0), 1.0);
        assert_eq!(geometry.visible_rows, 0);
        assert_eq!(geometry.total_rows, view.total_rows());
        assert!(geometry.lines.is_empty());

        let geometry = view.layout(Rect::new(0.0, 0.0, 10.0, 10.0), 0.0);
        assert_eq!(geometry.visible_rows, 0);
    }

    /// Side-by-side pane geometry: `left_w + DIFF_DIVIDER_W + right_w`
    /// exactly reconstructs the viewport width, and the divider sits at
    /// `viewport.x + left_w`.
    #[test]
    fn side_by_side_panes_reconstruct_viewport_width() {
        let view = two_hunk_view(DiffMode::SideBySide);
        let viewport = Rect::new(5.0, 0.0, 41.0, 10.0);
        let geometry = view.layout(viewport, 1.0);
        let panes = geometry.panes.expect("side-by-side mode has panes");

        assert_eq!(
            panes.left_w + DIFF_DIVIDER_W + panes.right_w,
            viewport.width
        );
        assert_eq!(panes.divider_x, viewport.x + panes.left_w);
    }

    /// A header strip is present only when a label is set, and it
    /// reserves exactly one `line_height` band from the content area.
    #[test]
    fn header_present_only_when_a_label_is_set() {
        let mut view = two_hunk_view(DiffMode::SideBySide);
        let viewport = Rect::new(0.0, 0.0, 40.0, 10.0);

        let no_header = view.layout(viewport, 1.0);
        assert!(no_header.header.is_none());
        assert_eq!(no_header.visible_rows, 10);

        view.left_label = Some("a/main.rs".into());
        let with_header = view.layout(viewport, 1.0);
        let header = with_header.header.expect("label set implies a header");
        assert_eq!(header.left.height, 1.0);
        assert_eq!(with_header.visible_rows, 9, "header steals one row");
    }

    /// `lines` is exactly the scroll-clamped visible window: at
    /// `scroll_offset = 1` on a 2-row-tall viewport, the first display
    /// line is row 1, not row 0.
    #[test]
    fn side_by_side_lines_respect_scroll_offset() {
        let mut view = two_hunk_view(DiffMode::SideBySide);
        view.scroll_offset = 1;
        let geometry = view.layout(Rect::new(0.0, 0.0, 40.0, 2.0), 1.0);
        assert_eq!(geometry.lines.len(), 2);
        assert_eq!(
            geometry.lines[0].content,
            DiffLineContent::Row { row_idx: 1 }
        );
        assert_eq!(
            geometry.lines[1].content,
            DiffLineContent::Row { row_idx: 2 }
        );
    }

    /// Unified mode interleaves a `UnifiedHeader` before each hunk's rows,
    /// and `total_rows` counts both — the regression this file's own
    /// `DiffViewLayout::total_rows` doc has warned about since #506.
    #[test]
    fn unified_lines_interleave_headers_with_rows() {
        let view = two_hunk_view(DiffMode::Unified);
        let geometry = view.layout(Rect::new(0.0, 0.0, 40.0, 100.0), 1.0);
        // 2 headers + 4 rows = 6 display lines, all visible (viewport is
        // tall enough).
        assert_eq!(geometry.total_rows, 6);
        assert_eq!(geometry.lines.len(), 6);
        assert_eq!(
            geometry.lines[0].content,
            DiffLineContent::UnifiedHeader { hunk_idx: 0 }
        );
        assert_eq!(
            geometry.lines[1].content,
            DiffLineContent::Row { row_idx: 0 }
        );
        assert_eq!(
            geometry.lines[2].content,
            DiffLineContent::Row { row_idx: 1 }
        );
        assert_eq!(
            geometry.lines[3].content,
            DiffLineContent::UnifiedHeader { hunk_idx: 1 }
        );
        assert_eq!(
            geometry.lines[4].content,
            DiffLineContent::Row { row_idx: 2 }
        );
        assert_eq!(
            geometry.lines[5].content,
            DiffLineContent::Row { row_idx: 3 }
        );
    }

    /// `row_colors` covers every [`DiffRowKind`] without panicking, and
    /// `Same` uses the same background on both sides (no left/right tint).
    #[test]
    fn row_colors_same_uses_uniform_background() {
        let theme = Theme::default();
        let (_, left_bg, _, right_bg) = row_colors(DiffRowKind::Same, &theme);
        assert_eq!(left_bg, theme.background);
        assert_eq!(right_bg, theme.background);
    }

    /// `unified_row_style` assigns the conventional `-`/`+`/` ` prefixes.
    #[test]
    fn unified_row_style_prefixes() {
        let theme = Theme::default();
        assert_eq!(unified_row_style(DiffRowKind::Removed, &theme).0, '-');
        assert_eq!(unified_row_style(DiffRowKind::Changed, &theme).0, '-');
        assert_eq!(unified_row_style(DiffRowKind::Added, &theme).0, '+');
        assert_eq!(unified_row_style(DiffRowKind::Same, &theme).0, ' ');
    }

    /// `unified_row_text` prefers the left text for `Removed`, otherwise
    /// the right text falling back to the left.
    #[test]
    fn unified_row_text_prefers_left_only_for_removed() {
        let removed = DiffRow {
            left: Some("old".into()),
            right: None,
            kind: DiffRowKind::Removed,
        };
        assert_eq!(unified_row_text(&removed), "old");

        let changed = DiffRow {
            left: Some("old".into()),
            right: Some("new".into()),
            kind: DiffRowKind::Changed,
        };
        assert_eq!(unified_row_text(&changed), "new");
    }

    /// `unified_hunk_header` counts left/right rows independently, so a
    /// hunk with padding rows (unequal removed/added counts) doesn't
    /// inflate either number to `rows.len()`.
    #[test]
    fn unified_hunk_header_excludes_padding_from_counts() {
        let hunk = DiffHunk {
            left_start: 5,
            right_start: 7,
            rows: vec![
                DiffRow {
                    left: Some("old1".into()),
                    right: Some("new1".into()),
                    kind: DiffRowKind::Changed,
                },
                DiffRow {
                    left: Some("old2".into()),
                    right: None,
                    kind: DiffRowKind::Removed,
                },
                DiffRow {
                    left: None,
                    right: Some("new3".into()),
                    kind: DiffRowKind::Added,
                },
            ],
        };
        assert_eq!(unified_hunk_header(&hunk), "@@ -5,2 +7,2 @@");
    }
}
