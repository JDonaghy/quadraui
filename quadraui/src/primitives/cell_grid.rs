//! `CellGrid` primitive: a spreadsheet-style grid of cells with letter
//! column headers, numbered row headers, frozen leading rows/columns, and
//! a rectangular cell selection — descriptor, layout and hit-testing only.
//! No paint path exists yet: a backend rasteriser (TUI first, per the
//! repo's *One backend per issue* rule) is a follow-up issue in the
//! `CellGrid` epic, built on top of this module exactly as
//! [`crate::primitives::tree_table::TreeTable`] was built on
//! [`crate::primitives::data_table::DataTable`] rather than as a parallel
//! primitive.
//!
//! # The grid only presents
//!
//! Cell text arrives already formatted from the app or the engine behind
//! it (e.g. `"$1,200.50"`) — [`CellGrid`] never parses or computes a
//! value. It owns geometry, the cursor, selection and (once a backend
//! paints it) editing UI, and reports what the user did; formatting and
//! computation stay the app's job.
//!
//! # `cells` is a viewport-sized window, not the whole sheet
//!
//! [`CellGrid::row_count`]/[`CellGrid::col_count`] describe the full
//! sheet — they drive row-number/column-letter headers and the
//! scrollbar's content extent — but [`CellGrid::cells`] only ever holds
//! the cells a caller actually needs to paint this frame: the frozen
//! rows/columns plus whatever the current scroll position brings into
//! view. A caller first resolves [`CellGrid::layout`] (which needs none
//! of `cells`' contents, only the row/column counts and sizes) to learn
//! exactly which absolute rows and columns are visible — [`CellGridLayout::rows`]
//! and [`CellGridLayout::cols`], in the same frozen-then-scrolled order
//! `cells` is indexed in — and then fills `cells[r][c]` from
//! `rows[r]`/`cols[c]`'s absolute indices. This mirrors
//! [`crate::primitives::tree_table::TreeTable::tree`] being parallel to
//! (rather than a copy of) its row data, just windowed instead of
//! flattened.

use crate::primitives::data_table::ColumnAlign;
use crate::types::{Color, WidgetId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

fn default_true() -> bool {
    true
}

/// Horizontal alignment for one cell's text.
///
/// `#[non_exhaustive]` so a future variant can land without breaking an
/// external exhaustive `match` — see `CLAUDE.md`'s *Downstream consumers*
/// section.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[non_exhaustive]
pub enum CellAlign {
    Left,
    Center,
    Right,
    /// Right-align text that [`looks_numeric`] — the spreadsheet default
    /// for an unstyled cell. See [`CellAlign::resolve`].
    #[default]
    General,
}

impl CellAlign {
    /// Resolve to a concrete [`ColumnAlign`] (reused rather than forked —
    /// it already has exactly the three concrete alignments a painter
    /// needs) given this cell's own text. `Left`/`Center`/`Right` pass
    /// straight through; `General` right-aligns when [`looks_numeric`]
    /// and left-aligns otherwise.
    #[must_use]
    pub fn resolve(self, text: &str) -> ColumnAlign {
        match self {
            CellAlign::Left => ColumnAlign::Left,
            CellAlign::Center => ColumnAlign::Center,
            CellAlign::Right => ColumnAlign::Right,
            CellAlign::General => {
                if looks_numeric(text) {
                    ColumnAlign::Right
                } else {
                    ColumnAlign::Left
                }
            }
        }
    }
}

/// Loose "does this already-formatted text read as a number" heuristic
/// for [`CellAlign::General`] — not a parser: the grid never computes a
/// value from `text`, it only decides which way to lean the glyphs.
/// Strips a single optional currency symbol, thousands separators, a
/// trailing `%`, and `(...)` accounting-negative parens, then requires
/// what's left to be digits with at most one decimal point and an
/// optional leading `-`.
#[must_use]
fn looks_numeric(text: &str) -> bool {
    let t = text.trim();
    if t.is_empty() {
        return false;
    }
    let t = t.strip_prefix(['$', '€', '£', '¥']).unwrap_or(t);
    let t = t.strip_suffix('%').unwrap_or(t);
    let t = t
        .strip_prefix('(')
        .and_then(|s| s.strip_suffix(')'))
        .unwrap_or(t);
    let t = t.strip_prefix('-').unwrap_or(t);
    let t = t.replace(',', "");
    !t.is_empty()
        && t.matches('.').count() <= 1
        && t.chars().all(|c| c.is_ascii_digit() || c == '.')
}

/// Optional visual overrides for one [`Cell`]. Every field defaults to
/// "inherit the theme/column default" when `None`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct CellStyle {
    #[serde(default)]
    pub bold: Option<bool>,
    #[serde(default)]
    pub italic: Option<bool>,
    #[serde(default)]
    pub underline: Option<bool>,
    #[serde(default)]
    pub fg: Option<Color>,
    #[serde(default)]
    pub bg: Option<Color>,
    /// `None` behaves exactly like `Some(CellAlign::General)` — see
    /// [`CellAlign::resolve`].
    #[serde(default)]
    pub align: Option<CellAlign>,
}

impl CellStyle {
    /// This style's effective alignment for `text` — `align`'s own
    /// [`CellAlign::resolve`] when set, [`CellAlign::General`]'s when not.
    #[must_use]
    pub fn effective_align(&self, text: &str) -> ColumnAlign {
        self.align.unwrap_or_default().resolve(text)
    }
}

/// One cell's already-formatted display text plus its optional style.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Cell {
    pub text: String,
    #[serde(default)]
    pub style: CellStyle,
}

impl Cell {
    /// A plain cell with default style.
    #[must_use]
    pub fn plain(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            style: CellStyle::default(),
        }
    }
}

/// Spreadsheet-style column letters: `0` → `"A"`, `25` → `"Z"`, `26` →
/// `"AA"`, `27` → `"AB"` — the standard base-26 "bijective numeral"
/// scheme (no digit stands for zero, so `"Z"` is immediately followed by
/// `"AA"` rather than wrapping like an ordinary base-26 number would).
#[must_use]
pub fn column_letters(index: usize) -> String {
    let mut n = index;
    let mut bytes = Vec::new();
    loop {
        let rem = (n % 26) as u8;
        bytes.push(b'A' + rem);
        if n < 26 {
            break;
        }
        n = n / 26 - 1;
    }
    bytes.reverse();
    String::from_utf8(bytes).expect("ASCII A-Z bytes are always valid UTF-8")
}

/// Declarative description of a `CellGrid` widget.
///
/// # Examples
///
/// ```
/// use quadraui::{Cell, CellGrid, CellGridHit, WidgetId};
///
/// let grid = CellGrid {
///     id: WidgetId::new("grid:sheet1"),
///     row_count: 100,
///     col_count: 30,
///     frozen_rows: 1,
///     frozen_cols: 1,
///     row_scroll: 0,
///     col_scroll: 0,
///     default_row_height: 1.0,
///     default_col_width: 8.0,
///     row_heights: Default::default(),
///     col_widths: Default::default(),
///     show_row_headers: true,
///     show_col_headers: true,
///     row_labels: Default::default(),
///     col_labels: Default::default(),
///     cursor: (0, 0),
///     selection_anchor: None,
///     cells: vec![vec![Cell::plain("A1")]],
///     has_focus: true,
///     show_scrollbar: false,
/// };
///
/// let layout = grid.layout(80.0, 24.0, 1.0, 4.0, 1.0, 1.0);
/// assert_eq!(layout.hit_test(0.0, 0.0), CellGridHit::Corner);
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CellGrid {
    pub id: WidgetId,
    /// Total rows in the sheet — drives row-number headers and the
    /// vertical scrollbar's content extent. Usually far larger than
    /// `cells`' own row count; see the module doc.
    pub row_count: usize,
    /// Total columns in the sheet — drives column-letter headers and the
    /// horizontal scrollbar's content extent.
    pub col_count: usize,
    /// Leading rows that never scroll, clamped to `row_count` by
    /// [`Self::layout`].
    #[serde(default)]
    pub frozen_rows: usize,
    /// Leading columns that never scroll, clamped to `col_count`.
    #[serde(default)]
    pub frozen_cols: usize,
    /// First non-frozen row currently scrolled into view.
    #[serde(default)]
    pub row_scroll: usize,
    /// First non-frozen column currently scrolled into view.
    #[serde(default)]
    pub col_scroll: usize,
    /// Row height in surface-native units (cells on TUI, pixels on a
    /// pixel backend) for any row with no [`Self::row_heights`] entry.
    pub default_row_height: f32,
    /// Column width in surface-native units for any column with no
    /// [`Self::col_widths`] entry.
    pub default_col_width: f32,
    /// Per-row height overrides, keyed by absolute row index. Sparse —
    /// most rows use `default_row_height`.
    #[serde(default)]
    pub row_heights: BTreeMap<usize, f32>,
    /// Per-column width overrides, keyed by absolute column index.
    #[serde(default)]
    pub col_widths: BTreeMap<usize, f32>,
    /// Paint the row-number header strip at all.
    #[serde(default = "default_true")]
    pub show_row_headers: bool,
    /// Paint the column-letter header strip at all.
    #[serde(default = "default_true")]
    pub show_col_headers: bool,
    /// Custom row header labels, keyed by absolute row index. A row with
    /// no entry falls back to its 1-based row number — see
    /// [`Self::row_label`].
    #[serde(default)]
    pub row_labels: BTreeMap<usize, String>,
    /// Custom column header labels, keyed by absolute column index. A
    /// column with no entry falls back to [`column_letters`] — see
    /// [`Self::col_label`].
    #[serde(default)]
    pub col_labels: BTreeMap<usize, String>,
    /// The active cell, as an absolute `(row, col)`.
    #[serde(default)]
    pub cursor: (usize, usize),
    /// The other corner of a rectangular selection, as an absolute
    /// `(row, col)`. `None` means no selection beyond the cursor cell
    /// itself. `Self::cursor` is always the selection's *active* corner
    /// — the one that moves under Shift+arrow.
    #[serde(default)]
    pub selection_anchor: Option<(usize, usize)>,
    /// The cells to paint this frame — see the module doc for how a
    /// `(row, col)` here maps to an absolute sheet position via
    /// [`CellGridLayout::rows`]/[`CellGridLayout::cols`].
    #[serde(default)]
    pub cells: Vec<Vec<Cell>>,
    #[serde(default)]
    pub has_focus: bool,
    /// Reserve a strip on the right/bottom edges for vertical/horizontal
    /// scrollbars.
    #[serde(default)]
    pub show_scrollbar: bool,
}

impl CellGrid {
    /// This row's header label: its [`Self::row_labels`] override if
    /// set, otherwise its 1-based row number.
    #[must_use]
    pub fn row_label(&self, row: usize) -> String {
        self.row_labels
            .get(&row)
            .cloned()
            .unwrap_or_else(|| (row + 1).to_string())
    }

    /// This column's header label: its [`Self::col_labels`] override if
    /// set, otherwise [`column_letters`].
    #[must_use]
    pub fn col_label(&self, col: usize) -> String {
        self.col_labels
            .get(&col)
            .cloned()
            .unwrap_or_else(|| column_letters(col))
    }

    /// The resolved rectangular selection as `(top, left, bottom, right)`
    /// — all inclusive — covering just [`Self::cursor`] when
    /// [`Self::selection_anchor`] is `None`.
    #[must_use]
    pub fn selection_bounds(&self) -> (usize, usize, usize, usize) {
        let (cr, cc) = self.cursor;
        let (ar, ac) = self.selection_anchor.unwrap_or(self.cursor);
        (cr.min(ar), cc.min(ac), cr.max(ar), cc.max(ac))
    }

    /// Compute layout from viewport dimensions, header strip sizes and
    /// scrollbar sizes, all in surface-native units.
    ///
    /// `col_header_height`/`row_header_width` are ignored (treated as
    /// `0.0`) when [`Self::show_col_headers`]/[`Self::show_row_headers`]
    /// is `false`; likewise `scrollbar_width`/`scrollbar_height` when
    /// [`Self::show_scrollbar`] is `false`.
    pub fn layout(
        &self,
        viewport_width: f32,
        viewport_height: f32,
        col_header_height: f32,
        row_header_width: f32,
        scrollbar_width: f32,
        scrollbar_height: f32,
    ) -> CellGridLayout {
        let col_header_height = if self.show_col_headers {
            col_header_height.max(0.0)
        } else {
            0.0
        };
        let row_header_width = if self.show_row_headers {
            row_header_width.max(0.0)
        } else {
            0.0
        };
        let scrollbar_width = if self.show_scrollbar {
            scrollbar_width.max(0.0)
        } else {
            0.0
        };
        let scrollbar_height = if self.show_scrollbar {
            scrollbar_height.max(0.0)
        } else {
            0.0
        };

        let body_x_start = row_header_width;
        let body_y_start = col_header_height;
        let body_x_end = (viewport_width - scrollbar_width).max(body_x_start);
        let body_y_end = (viewport_height - scrollbar_height).max(body_y_start);

        let (rows, frozen_rows) = resolve_spans(
            self.frozen_rows,
            self.row_scroll,
            self.row_count,
            body_y_start,
            body_y_end,
            self.default_row_height,
            &self.row_heights,
        );
        let (cols, frozen_cols) = resolve_spans(
            self.frozen_cols,
            self.col_scroll,
            self.col_count,
            body_x_start,
            body_x_end,
            self.default_col_width,
            &self.col_widths,
        );

        CellGridLayout {
            viewport_width,
            viewport_height,
            col_header_height,
            row_header_width,
            scrollbar_width,
            scrollbar_height,
            rows,
            frozen_rows,
            cols,
            frozen_cols,
        }
    }
}

/// One resolved row or column: its absolute index into the sheet, and
/// its position/size along the relevant axis (`y`/`height` for a row,
/// `x`/`width` for a column) in surface-native units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResolvedSpan {
    pub index: usize,
    pub start: f32,
    pub size: f32,
}

/// Stack `total` 0-based indices into spans of `overrides.get(i)` (falling
/// back to `default_size`), starting at `start` and stopping once a span
/// would start at or past `end` — first the leading `frozen` indices
/// (always included, even if some end up clipped by `end`), then,
/// immediately following them, indices from `frozen + scroll` up to
/// `total` (the scrolled body). Shared by [`CellGrid::layout`] for both
/// the row (y) and column (x) axis, since the arithmetic — a running
/// position plus a per-index size lookup — is identical either way.
///
/// Returns the resolved spans in that frozen-then-scrolled paint order,
/// plus how many of the leading entries are the frozen ones.
fn resolve_spans(
    frozen: usize,
    scroll: usize,
    total: usize,
    start: f32,
    end: f32,
    default_size: f32,
    overrides: &BTreeMap<usize, f32>,
) -> (Vec<ResolvedSpan>, usize) {
    let frozen = frozen.min(total);
    let mut out = Vec::new();
    let mut pos = start;
    for i in 0..frozen {
        if pos >= end {
            break;
        }
        let size = overrides.get(&i).copied().unwrap_or(default_size).max(0.0);
        out.push(ResolvedSpan {
            index: i,
            start: pos,
            size,
        });
        pos += size;
    }
    let frozen_count = out.len();
    let scroll = scroll.min(total.saturating_sub(frozen));
    for i in (frozen + scroll)..total {
        if pos >= end {
            break;
        }
        let size = overrides.get(&i).copied().unwrap_or(default_size).max(0.0);
        out.push(ResolvedSpan {
            index: i,
            start: pos,
            size,
        });
        pos += size;
    }
    (out, frozen_count)
}

/// Hit-test result for a `CellGrid`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellGridHit {
    /// Click on the top-left box where the row and column header strips
    /// meet — conventionally "select everything".
    Corner,
    /// Click on a column header, by absolute column index.
    ColumnHeader(usize),
    /// Click on a row header, by absolute row index.
    RowHeader(usize),
    /// Click on a body cell, by absolute `(row, col)`.
    Cell(usize, usize),
    /// Click on the vertical scrollbar's strip.
    VScrollbar,
    /// Click on the horizontal scrollbar's strip.
    HScrollbar,
    /// Click outside every region above — empty space, or the
    /// bottom-right square where both scrollbars would otherwise meet.
    Empty,
}

/// Fully-resolved `CellGrid` layout.
///
/// `#[non_exhaustive]`: never constructed by a consumer via struct
/// literal, only ever received from [`CellGrid::layout`] — same
/// convention as [`crate::DataTableLayout`].
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct CellGridLayout {
    pub viewport_width: f32,
    pub viewport_height: f32,
    pub col_header_height: f32,
    pub row_header_width: f32,
    pub scrollbar_width: f32,
    pub scrollbar_height: f32,
    /// Visible rows in paint order: `rows[..frozen_rows]` first, then the
    /// scrolled body rows that fit the remaining viewport height. Also
    /// the order [`CellGrid::cells`] is expected to be indexed in.
    pub rows: Vec<ResolvedSpan>,
    /// How many of `rows`' leading entries are frozen.
    pub frozen_rows: usize,
    /// Visible columns, in the same frozen-then-scrolled paint order.
    pub cols: Vec<ResolvedSpan>,
    /// How many of `cols`' leading entries are frozen.
    pub frozen_cols: usize,
}

impl CellGridLayout {
    /// Resolve a click to a corner / header / cell / scrollbar strip.
    /// `x`/`y` are viewport-relative, as a caller's raw pointer position.
    pub fn hit_test(&self, x: f32, y: f32) -> CellGridHit {
        if x < 0.0 || y < 0.0 || x >= self.viewport_width || y >= self.viewport_height {
            return CellGridHit::Empty;
        }
        let body_right = self.viewport_width - self.scrollbar_width;
        let body_bottom = self.viewport_height - self.scrollbar_height;

        // The vertical scrollbar's strip runs the full height (including
        // alongside the column headers), except the bottom-right corner
        // square shared with the horizontal scrollbar.
        if self.scrollbar_width > 0.0 && x >= body_right {
            return if y < body_bottom {
                CellGridHit::VScrollbar
            } else {
                CellGridHit::Empty
            };
        }
        // Mirrors the vertical strip: full width, excluding the corner
        // square already handled above.
        if self.scrollbar_height > 0.0 && y >= body_bottom {
            return CellGridHit::HScrollbar;
        }

        let in_col_header = self.col_header_height > 0.0 && y < self.col_header_height;
        let in_row_header = self.row_header_width > 0.0 && x < self.row_header_width;
        if in_col_header && in_row_header {
            return CellGridHit::Corner;
        }
        if in_col_header {
            return match self
                .cols
                .iter()
                .find(|c| x >= c.start && x < c.start + c.size)
            {
                Some(c) => CellGridHit::ColumnHeader(c.index),
                None => CellGridHit::Empty,
            };
        }
        if in_row_header {
            return match self
                .rows
                .iter()
                .find(|r| y >= r.start && y < r.start + r.size)
            {
                Some(r) => CellGridHit::RowHeader(r.index),
                None => CellGridHit::Empty,
            };
        }
        let row = self
            .rows
            .iter()
            .find(|r| y >= r.start && y < r.start + r.size);
        let col = self
            .cols
            .iter()
            .find(|c| x >= c.start && x < c.start + c.size);
        match (row, col) {
            (Some(r), Some(c)) => CellGridHit::Cell(r.index, c.index),
            _ => CellGridHit::Empty,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid() -> CellGrid {
        CellGrid {
            id: WidgetId::new("grid:test"),
            row_count: 1000,
            col_count: 1000,
            frozen_rows: 1,
            frozen_cols: 1,
            row_scroll: 0,
            col_scroll: 0,
            default_row_height: 1.0,
            default_col_width: 4.0,
            row_heights: BTreeMap::new(),
            col_widths: BTreeMap::new(),
            show_row_headers: true,
            show_col_headers: true,
            row_labels: BTreeMap::new(),
            col_labels: BTreeMap::new(),
            cursor: (0, 0),
            selection_anchor: None,
            cells: vec![],
            has_focus: true,
            show_scrollbar: false,
        }
    }

    // ── column_letters ──────────────────────────────────────────────

    #[test]
    fn column_letters_goes_past_z_into_aa_ab() {
        assert_eq!(column_letters(0), "A");
        assert_eq!(column_letters(25), "Z");
        assert_eq!(column_letters(26), "AA");
        assert_eq!(column_letters(27), "AB");
        assert_eq!(column_letters(51), "AZ");
        assert_eq!(column_letters(52), "BA");
        // 26 + 26*26 = 702 is the first triple-letter column: "AAA".
        assert_eq!(column_letters(702), "AAA");
    }

    #[test]
    fn col_label_falls_back_to_letters_but_honours_an_override() {
        let mut g = grid();
        assert_eq!(g.col_label(26), "AA");
        g.col_labels.insert(26, "Total".to_string());
        assert_eq!(g.col_label(26), "Total");
        assert_eq!(g.col_label(0), "A");
    }

    #[test]
    fn row_label_is_one_based_but_honours_an_override() {
        let mut g = grid();
        assert_eq!(g.row_label(0), "1");
        assert_eq!(g.row_label(9), "10");
        g.row_labels.insert(0, "Header".to_string());
        assert_eq!(g.row_label(0), "Header");
    }

    // ── CellAlign::resolve / "general" numbers ──────────────────────

    #[test]
    fn general_alignment_right_aligns_numbers_and_left_aligns_text() {
        assert_eq!(CellAlign::General.resolve("1200.50"), ColumnAlign::Right);
        assert_eq!(CellAlign::General.resolve("$1,200.50"), ColumnAlign::Right);
        assert_eq!(CellAlign::General.resolve("-42"), ColumnAlign::Right);
        assert_eq!(CellAlign::General.resolve("(42)"), ColumnAlign::Right);
        assert_eq!(CellAlign::General.resolve("42%"), ColumnAlign::Right);
        assert_eq!(CellAlign::General.resolve("hello"), ColumnAlign::Left);
        assert_eq!(CellAlign::General.resolve(""), ColumnAlign::Left);
        assert_eq!(CellAlign::General.resolve("12.3.4"), ColumnAlign::Left);
    }

    #[test]
    fn explicit_alignment_is_never_overridden_by_the_general_heuristic() {
        assert_eq!(CellAlign::Left.resolve("1200.50"), ColumnAlign::Left);
        assert_eq!(CellAlign::Center.resolve("hello"), ColumnAlign::Center);
        assert_eq!(CellAlign::Right.resolve("hello"), ColumnAlign::Right);
    }

    #[test]
    fn cell_style_with_no_align_set_behaves_like_general() {
        let style = CellStyle::default();
        assert_eq!(style.effective_align("42"), ColumnAlign::Right);
        assert_eq!(style.effective_align("abc"), ColumnAlign::Left);
    }

    // ── frozen panes stay put while the body scrolls ────────────────

    #[test]
    fn frozen_row_and_column_stay_put_as_the_body_scrolls() {
        let mut g = grid();
        let unscrolled = g.layout(100.0, 50.0, 1.0, 4.0, 1.0, 1.0);
        assert_eq!(unscrolled.rows[0].index, 0);
        assert_eq!(unscrolled.cols[0].index, 0);
        let frozen_row0_y = unscrolled.rows[0].start;
        let frozen_col0_x = unscrolled.cols[0].start;

        g.row_scroll = 20;
        g.col_scroll = 15;
        let scrolled = g.layout(100.0, 50.0, 1.0, 4.0, 1.0, 1.0);

        // The frozen row/column are still the very first entries, at the
        // exact same position — scrolling never moves them.
        assert_eq!(scrolled.rows[0].index, 0);
        assert_eq!(scrolled.rows[0].start, frozen_row0_y);
        assert_eq!(scrolled.cols[0].index, 0);
        assert_eq!(scrolled.cols[0].start, frozen_col0_x);
        assert_eq!(scrolled.frozen_rows, 1);
        assert_eq!(scrolled.frozen_cols, 1);

        // But the body (second entry onward) now starts at the scrolled
        // absolute index, not row/col 1.
        assert_eq!(scrolled.rows[1].index, 1 + 20);
        assert_eq!(scrolled.cols[1].index, 1 + 15);
    }

    // ── hit_test at frozen/scrolled boundaries ──────────────────────

    #[test]
    fn hit_test_resolves_corner_headers_and_cells() {
        let g = grid();
        let layout = g.layout(100.0, 50.0, 1.0, 4.0, 1.0, 1.0);

        assert_eq!(layout.hit_test(0.0, 0.0), CellGridHit::Corner);
        // Just right of the row-header strip, still in the header band:
        // a column header.
        assert_eq!(layout.hit_test(4.5, 0.0), CellGridHit::ColumnHeader(0));
        // Just below the column-header strip, still in the row-header
        // band: a row header.
        assert_eq!(layout.hit_test(0.0, 1.5), CellGridHit::RowHeader(0));
        // Inside the body: the frozen (row 0, col 0) cell.
        assert_eq!(layout.hit_test(5.0, 1.5), CellGridHit::Cell(0, 0));
    }

    #[test]
    fn hit_test_at_the_frozen_scrolled_boundary_lands_on_the_right_cell() {
        let mut g = grid();
        g.row_scroll = 10;
        g.col_scroll = 5;
        let layout = g.layout(100.0, 50.0, 1.0, 4.0, 1.0, 1.0);

        // Row entry 0 is frozen row 0 at y in [1.0, 2.0); row entry 1 is
        // the scrolled body's first row (absolute index 11) at
        // y in [2.0, 3.0). A click right at that boundary must land on
        // the scrolled row, not the frozen one.
        assert_eq!(layout.rows[1].index, 11);
        assert_eq!(layout.rows[1].start, 2.0);
        assert_eq!(
            layout.hit_test(5.0, 2.0),
            CellGridHit::Cell(11, layout.cols[0].index)
        );
        // One unit above the boundary is still the frozen row.
        assert_eq!(
            layout.hit_test(5.0, 1.99),
            CellGridHit::Cell(0, layout.cols[0].index)
        );

        // Same check along the column axis, at a `y` inside the frozen
        // row's own band (`[1.0, 2.0)`) so only the column boundary is
        // under test: col entry 0 is frozen col 0 at x in [4.0, 8.0);
        // col entry 1 is the scrolled body's first column (absolute
        // index 6) at x in [8.0, 12.0).
        assert_eq!(layout.cols[1].index, 6);
        assert_eq!(layout.cols[1].start, 8.0);
        assert_eq!(
            layout.hit_test(8.0, 1.5),
            CellGridHit::Cell(layout.rows[0].index, 6)
        );
        assert_eq!(
            layout.hit_test(7.99, 1.5),
            CellGridHit::Cell(layout.rows[0].index, 0)
        );
    }

    #[test]
    fn hit_test_resolves_scrollbar_strips_and_the_dead_corner() {
        let mut g = grid();
        g.show_scrollbar = true;
        let layout = g.layout(100.0, 50.0, 1.0, 4.0, 2.0, 2.0);

        assert_eq!(layout.hit_test(99.0, 10.0), CellGridHit::VScrollbar);
        assert_eq!(layout.hit_test(10.0, 49.0), CellGridHit::HScrollbar);
        // The bottom-right square where both strips would overlap is a
        // dead corner, not either scrollbar.
        assert_eq!(layout.hit_test(99.0, 49.0), CellGridHit::Empty);
    }

    #[test]
    fn hit_test_outside_the_viewport_is_empty() {
        let g = grid();
        let layout = g.layout(100.0, 50.0, 1.0, 4.0, 1.0, 1.0);
        assert_eq!(layout.hit_test(-1.0, 0.0), CellGridHit::Empty);
        assert_eq!(layout.hit_test(0.0, -1.0), CellGridHit::Empty);
        assert_eq!(layout.hit_test(100.0, 0.0), CellGridHit::Empty);
        assert_eq!(layout.hit_test(0.0, 50.0), CellGridHit::Empty);
    }

    #[test]
    fn hidden_headers_shrink_their_strip_to_zero() {
        let mut g = grid();
        g.show_row_headers = false;
        g.show_col_headers = false;
        let layout = g.layout(100.0, 50.0, 1.0, 4.0, 1.0, 1.0);
        assert_eq!(layout.row_header_width, 0.0);
        assert_eq!(layout.col_header_height, 0.0);
        // With no header strips, a click at the origin lands directly on
        // the first cell rather than the corner.
        assert_eq!(layout.hit_test(0.0, 0.0), CellGridHit::Cell(0, 0));
    }

    // ── selection_bounds ─────────────────────────────────────────────

    #[test]
    fn selection_bounds_normalizes_cursor_and_anchor_either_direction() {
        let mut g = grid();
        g.cursor = (2, 1);
        g.selection_anchor = Some((5, 4));
        assert_eq!(g.selection_bounds(), (2, 1, 5, 4));

        // Dragging back past the anchor: cursor becomes the top-left
        // corner instead, and the bounds stay normalized.
        g.cursor = (0, 0);
        assert_eq!(g.selection_bounds(), (0, 0, 5, 4));

        // No anchor: the selection is just the cursor cell.
        g.selection_anchor = None;
        g.cursor = (3, 3);
        assert_eq!(g.selection_bounds(), (3, 3, 3, 3));
    }
}
