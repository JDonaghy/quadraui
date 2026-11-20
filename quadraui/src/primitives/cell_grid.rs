//! `CellGrid` primitive: a spreadsheet-style grid of cells with letter
//! column headers, numbered row headers, frozen leading rows/columns, and
//! a rectangular cell selection — descriptor, layout, hit-testing, plus
//! the [`crate::primitive::Primitive`] impl near the bottom of this
//! module that wires it through [`crate::backend::DrawsPrimitives::draw_primitive`]:
//! TUI paints real content via [`crate::primitive::Primitive::paint_cells`]
//! → [`crate::tui::draw_cell_grid`], and GTK/macOS/Win paint through
//! [`crate::primitive::Primitive::paint`] → this module's own
//! `native_surface_paint`, reusing
//! [`crate::primitives::data_table::DataTable`]'s own alignment/ellipsis
//! text helpers rather than forking them — the same "extend, don't fork"
//! rule [`crate::primitives::tree_table::TreeTable`] already follows for
//! its own cell text.
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
//!
//! # Two-axis virtualisation: `cells_window` + `CellsRequested`
//!
//! [`CellGrid::cells_window`] reports exactly what absolute block of the
//! sheet the scrolled (non-frozen) portion of [`CellGrid::cells`]
//! currently covers, as a [`CellsWindow`] — computed from
//! [`CellGrid::frozen_rows`]/[`CellGrid::frozen_cols`]/
//! [`CellGrid::loaded_row_start`]/[`CellGrid::loaded_col_start`] plus
//! `cells`' own shape. `loaded_row_start`/`loaded_col_start` are set by
//! whichever caller filled `cells` and are deliberately **independent**
//! of [`CellGrid::row_scroll`]/[`CellGrid::col_scroll`]: they record
//! where the supplied payload came from, not where the viewport
//! currently is, so a scroll that outruns what's loaded is still
//! detectable (see [`CellGrid::cells_requested`]'s doc for why collapsing
//! the two into one scroll-derived quantity defeats the whole check).
//! [`CellGrid::cells_requested`] compares that window against a resolved
//! [`CellGridLayout`]'s own visible rows/columns and, when the loaded
//! window doesn't cover what the viewport needs (a fresh scroll, a jump,
//! or a resize that grew the viewport), returns a
//! [`CellGridEvent::CellsRequested`] naming the exact absolute block —
//! `row_start`/`col_start`/`rows`/`cols` — the app should supply next.
//! [`CellGrid::row_count`]/[`CellGrid::col_count`] (the sheet totals)
//! never change size with the loaded block, so the header strips and any
//! scrollbar built from [`CellGrid::layout`] stay anchored to the real
//! sheet extent regardless of how small a window the app currently has
//! loaded.

use crate::event::Rect;
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
///     loaded_row_start: 0,
///     loaded_col_start: 0,
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
    /// How many rows after the frozen block are skipped before the body
    /// starts painting — the body's first visible row is
    /// `frozen_rows + row_scroll`, not `row_scroll` itself.
    #[serde(default)]
    pub row_scroll: usize,
    /// How many columns after the frozen block are skipped before the
    /// body starts painting — the body's first visible column is
    /// `frozen_cols + col_scroll`, not `col_scroll` itself.
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
    /// Absolute row the scrolled (non-frozen) portion of [`Self::cells`]
    /// starts at — set by whichever caller filled `cells`, independent of
    /// [`Self::row_scroll`]. See the module doc's "Two-axis
    /// virtualisation" section: this is what lets [`Self::cells_requested`]
    /// notice that `row_scroll` moved on without a matching reload, which
    /// deriving the loaded window's origin from `row_scroll` itself can
    /// never detect.
    #[serde(default)]
    pub loaded_row_start: usize,
    /// Absolute column the scrolled (non-frozen) portion of
    /// [`Self::cells`] starts at — see [`Self::loaded_row_start`].
    #[serde(default)]
    pub loaded_col_start: usize,
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
/// would start at or past `end` — first the leading `frozen` indices (each
/// included only while `pos` stays below `end`, so `frozen_count` can come
/// back smaller than the requested `frozen` once enough of them are
/// clipped), then, immediately following them, indices from
/// `frozen + scroll` up to `total` (the scrolled body). Shared by
/// [`CellGrid::layout`] for both the row (y) and column (x) axis, since the
/// arithmetic — a running position plus a per-index size lookup — is
/// identical either way.
///
/// A zero, negative or NaN size (an override or `default_size` of `0.0`,
/// say) advances `pos` by nothing, so it is skipped rather than pushed —
/// otherwise `pos >= end` would never trip and the full `0..total` range
/// would each get a `ResolvedSpan`, which for a real sheet (`total` in the
/// hundreds of thousands) means one zero-size allocation per row/column
/// instead of an empty viewport.
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
        if size > 0.0 {
            out.push(ResolvedSpan {
                index: i,
                start: pos,
                size,
            });
        }
        pos += size;
    }
    let frozen_count = out.len();
    let scroll = scroll.min(total.saturating_sub(frozen));
    for i in (frozen + scroll)..total {
        if pos >= end {
            break;
        }
        let size = overrides.get(&i).copied().unwrap_or(default_size).max(0.0);
        if size > 0.0 {
            out.push(ResolvedSpan {
                index: i,
                start: pos,
                size,
            });
        }
        pos += size;
    }
    (out, frozen_count)
}

/// Hit-test result for a `CellGrid`.
///
/// `#[non_exhaustive]` so a future variant can land without breaking an
/// external exhaustive `match` — see `CLAUDE.md`'s *Downstream consumers*
/// section.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
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

/// Events a `CellGrid` emits back to the app.
///
/// [`Self::CursorMoved`] is not produced by [`CellGrid::layout`]/
/// [`CellGridLayout::hit_test`] themselves — a host derives it from
/// [`crate::UiEvent`] plus `CellGridPrimitiveLayout::hit_test`, the same
/// convention every other row-family primitive's `*Event` enum follows
/// (e.g. [`crate::primitives::icon_grid::IconGridEvent`]).
/// [`Self::CellsRequested`] is different: it has no input event to derive
/// from (no click or key fired it), so a host computes it directly via
/// [`CellGrid::cells_requested`] once per frame after resolving layout.
///
/// `#[non_exhaustive]` so a future variant can land without breaking an
/// external exhaustive `match` — see `CLAUDE.md`'s *Downstream consumers*
/// section (same convention as [`CellGridHit`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum CellGridEvent {
    /// The active cell moved to a new absolute `(row, col)`.
    CursorMoved { row: usize, col: usize },
    /// The viewport needs the absolute sheet block `[row_start, row_start
    /// + rows)` × `[col_start, col_start + cols)` painted this frame, and
    /// [`CellGrid::cells_window`] says it isn't loaded yet — see
    /// [`CellGrid::cells_requested`]. This block excludes the frozen
    /// leading rows/columns, which stay loaded unconditionally: `cells`
    /// is indexed positionally against
    /// [`CellGridLayout::rows`]/[`CellGridLayout::cols`] with the frozen
    /// entries listed first, so a host that fills [`CellGrid::cells`]
    /// from this event alone, without re-prepending the frozen prefix,
    /// shifts body data into the frozen slots. Set
    /// [`CellGrid::loaded_row_start`]/[`CellGrid::loaded_col_start`] to
    /// `row_start`/`col_start` once the requested block is supplied.
    CellsRequested {
        row_start: usize,
        col_start: usize,
        rows: usize,
        cols: usize,
    },
}

/// The absolute sheet block the scrolled (non-frozen) portion of
/// [`CellGrid::cells`] currently covers, reported by
/// [`CellGrid::cells_window`]. The leading frozen rows/columns are not
/// part of this window — they never scroll, so a caller is expected to
/// keep them loaded unconditionally (see the module doc).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellsWindow {
    /// Absolute row the loaded scrolled body starts at.
    pub row_start: usize,
    /// Absolute column the loaded scrolled body starts at.
    pub col_start: usize,
    /// How many scrolled-body rows are loaded, starting at `row_start`.
    pub rows: usize,
    /// How many scrolled-body columns are loaded, starting at
    /// `col_start`.
    pub cols: usize,
    /// [`CellGrid::row_count`] — the sheet's full row extent, independent
    /// of how much is actually loaded.
    pub total_rows: usize,
    /// [`CellGrid::col_count`] — the sheet's full column extent.
    pub total_cols: usize,
}

// ── Accessibility ────────────────────────────────────────────────────
//
// `CellGrid`'s `A11yPrimitive` impl lives here rather than in
// `crate::a11y::tree` (where `DataTable`/`TreeView`'s impls live) — the
// same "new primitive owns its own a11y impl" precedent
// `IconGrid::a11y_info`'s doc describes, since the private per-row a11y
// helpers in `crate::a11y::tree` aren't reachable from this module
// either. Unlike `IconGrid`, `A11yRole::Table`/`Row`/`Cell` already fit
// exactly, the same roles `crate::a11y::tree::data_table_row` reports
// for `DataTable`.
//
// Reported children only ever cover `Self::cells`' painted window (see
// the module doc's "`cells` is a viewport-sized window") — the full
// sheet behind `row_count`/`col_count` has no accessibility presence
// beyond that.

/// Absolute sheet index for windowed position `pos` along one axis —
/// the same frozen-then-scrolled order [`resolve_spans`] resolves a
/// pixel [`CellGridLayout`] in, but viewport-agnostic: unlike
/// [`CellGrid::layout`]'s caller, [`CellGrid::a11y_children`] has no
/// pixel rect to resolve a real layout from, only [`CellGrid::cells`]'
/// windowed shape plus the frozen/scroll counts that produced it. Never
/// clips past `total` the way [`resolve_spans`]'s own early-break does —
/// a caller is expected to pass a `pos` already within `cells`' bounds.
fn windowed_abs_index(frozen: usize, scroll: usize, total: usize, pos: usize) -> usize {
    let frozen = frozen.min(total);
    if pos < frozen {
        pos
    } else {
        frozen + scroll.min(total.saturating_sub(frozen)) + (pos - frozen)
    }
}

impl CellGrid {
    /// The absolute row index [`Self::cells`]' windowed row `pos` maps
    /// onto — see [`windowed_abs_index`].
    #[must_use]
    fn windowed_row(&self, pos: usize) -> usize {
        windowed_abs_index(self.frozen_rows, self.row_scroll, self.row_count, pos)
    }

    /// The absolute column index [`Self::cells`]' windowed column `pos`
    /// maps onto — see [`windowed_abs_index`].
    #[must_use]
    fn windowed_col(&self, pos: usize) -> usize {
        windowed_abs_index(self.frozen_cols, self.col_scroll, self.col_count, pos)
    }

    /// The absolute sheet block the scrolled (non-frozen) portion of
    /// [`Self::cells`] currently covers — see [`CellsWindow`]'s own doc
    /// for why the frozen rows/columns are excluded. The block's origin
    /// comes from [`Self::loaded_row_start`]/[`Self::loaded_col_start`]
    /// (clamped into `[frozen_rows, row_count]`/`[frozen_cols,
    /// col_count]`), **not** from [`Self::row_scroll`]/[`Self::col_scroll`]
    /// — those two pairs must stay independent, or [`Self::cells_requested`]
    /// degenerates into "is `cells` big enough?" and can never notice that
    /// the scroll position moved on without a matching reload (see the
    /// module doc's "Two-axis virtualisation" section).
    ///
    /// `rows`/`cols` take the *minimum* loaded extent across every
    /// scrolled-body row rather than the first or the maximum — a jagged
    /// `cells` (one row shorter than the rest) must never read as fully
    /// covering a column that row doesn't actually have, which an
    /// optimistic max would.
    #[must_use]
    pub fn cells_window(&self) -> CellsWindow {
        let frozen_rows = self.frozen_rows.min(self.row_count);
        let frozen_cols = self.frozen_cols.min(self.col_count);
        let row_start = self.loaded_row_start.clamp(frozen_rows, self.row_count);
        let col_start = self.loaded_col_start.clamp(frozen_cols, self.col_count);
        let rows = self.cells.len().saturating_sub(frozen_rows);
        let cols = self
            .cells
            .iter()
            .skip(frozen_rows)
            .map(|row| row.len().saturating_sub(frozen_cols))
            .min()
            .unwrap_or(0);
        CellsWindow {
            row_start,
            col_start,
            rows,
            cols,
            total_rows: self.row_count,
            total_cols: self.col_count,
        }
    }

    /// Whether [`Self::cells_window`] already covers every row/column
    /// `layout` says is visible and, if not, the
    /// [`CellGridEvent::CellsRequested`] naming the exact absolute block
    /// the app should supply next — the frozen rows/columns are excluded
    /// from both sides of this check since they're expected to be loaded
    /// unconditionally (see the module doc).
    ///
    /// Returns `None` once the loaded window covers the visible range —
    /// a caller that already sent the right block gets no repeat
    /// request. There is no prefetch margin (unlike
    /// `quadraui-binding-core`'s wire-level `rows_requested_for_window`):
    /// the returned block is exactly the viewport's own visible
    /// scrolled-body rows/columns, since that's the acceptance contract
    /// this method exists to satisfy.
    #[must_use]
    pub fn cells_requested(&self, layout: &CellGridLayout) -> Option<CellGridEvent> {
        let body_rows = &layout.rows[layout.frozen_rows.min(layout.rows.len())..];
        let body_cols = &layout.cols[layout.frozen_cols.min(layout.cols.len())..];
        let (Some(first_row), Some(first_col)) = (body_rows.first(), body_cols.first()) else {
            return None;
        };
        let row_start = first_row.index;
        let col_start = first_col.index;
        let rows = body_rows.len();
        let cols = body_cols.len();

        let window = self.cells_window();
        // `cells` is indexed *positionally* against `layout.rows`/`layout.cols`
        // (see the module doc's "fills `cells[r][c]` from `rows[r]`/`cols[c]`'s
        // absolute indices"), so a loaded window whose origin merely *contains*
        // the viewport — but starts earlier — still paints the wrong rows/columns
        // under the viewport's headers. The only self-consistent check is exact
        // origin equality plus enough loaded extent.
        let covers = window.row_start == row_start
            && window.rows >= rows
            && window.col_start == col_start
            && window.cols >= cols;
        if covers {
            None
        } else {
            Some(CellGridEvent::CellsRequested {
                row_start,
                col_start,
                rows,
                cols,
            })
        }
    }
}

impl crate::a11y::A11yPrimitive for CellGrid {
    fn a11y_info(&self) -> crate::a11y::A11yInfo {
        crate::a11y::A11yInfo::new()
            .with_role(crate::a11y::A11yRole::Table)
            .with_label("Cell grid")
            .with_set_size(self.row_count as u32)
    }

    fn a11y_children(&self) -> Vec<crate::a11y::A11yChild> {
        let col_count = self.cells.iter().map(|row| row.len()).max().unwrap_or(0);
        let mut children = Vec::new();
        if self.show_col_headers && col_count > 0 {
            children.push(cell_grid_header_row(self, col_count));
        }
        children.extend(
            self.cells
                .iter()
                .enumerate()
                .map(|(r_pos, row)| cell_grid_row(self, r_pos, row)),
        );
        children
    }

    fn a11y_focus_key(&self) -> Option<String> {
        Some(format!("cell.{}.{}", self.cursor.0, self.cursor.1))
    }
}

/// The column-letter header row: one [`crate::a11y::A11yRole::Cell`] per
/// visible column (`0..col_count` of [`CellGrid::cells`]), labelled with
/// [`CellGrid::col_label`] — the same `Row`-of-`Cell`s shape
/// [`crate::a11y::tree::data_table_header`] reports for [`crate::DataTable`].
fn cell_grid_header_row(grid: &CellGrid, col_count: usize) -> crate::a11y::A11yChild {
    let cells = (0..col_count)
        .map(|c_pos| {
            let col = grid.windowed_col(c_pos);
            let info = crate::a11y::A11yInfo::new()
                .with_role(crate::a11y::A11yRole::Cell)
                .with_label(grid.col_label(col));
            crate::a11y::A11yChild::new(format!("colheader.{col}"), info)
        })
        .collect();
    crate::a11y::A11yChild::new(
        "header".to_string(),
        crate::a11y::A11yInfo::new()
            .with_role(crate::a11y::A11yRole::Row)
            .with_label("Header"),
    )
    .with_children(cells)
}

/// One visible body row: an optional leading row-number header cell
/// (when [`CellGrid::show_row_headers`]), then one
/// [`crate::a11y::A11yRole::Cell`] per visible column, carrying its text
/// and `with_selected` whenever `(row, col)` falls inside
/// [`CellGrid::selection_bounds`]. The *focused* cell (the cursor) is
/// named separately via [`CellGrid::a11y_focus_key`] rather than a
/// per-cell flag — the same split [`crate::a11y::tree::data_table_row`]
/// draws between `Row`'s `with_selected` and `DataTable::a11y_focus_key`.
fn cell_grid_row(grid: &CellGrid, r_pos: usize, row: &[Cell]) -> crate::a11y::A11yChild {
    let abs_row = grid.windowed_row(r_pos);
    let (sr0, sc0, sr1, sc1) = grid.selection_bounds();
    let mut cells = Vec::new();
    if grid.show_row_headers {
        let info = crate::a11y::A11yInfo::new()
            .with_role(crate::a11y::A11yRole::Cell)
            .with_label(grid.row_label(abs_row));
        cells.push(crate::a11y::A11yChild::new(
            format!("rowheader.{abs_row}"),
            info,
        ));
    }
    for (c_pos, cell) in row.iter().enumerate() {
        let abs_col = grid.windowed_col(c_pos);
        let selected = (sr0..=sr1).contains(&abs_row) && (sc0..=sc1).contains(&abs_col);
        let mut info = crate::a11y::A11yInfo::new()
            .with_role(crate::a11y::A11yRole::Cell)
            .with_selected(selected);
        if !cell.text.is_empty() {
            info = info.with_label(cell.text.clone());
        }
        cells.push(crate::a11y::A11yChild::new(
            format!("cell.{abs_row}.{abs_col}"),
            info,
        ));
    }
    crate::a11y::A11yChild::new(
        format!("row.{abs_row}"),
        crate::a11y::A11yInfo::new().with_role(crate::a11y::A11yRole::Row),
    )
    .with_children(cells)
}

// ── GUI paint: headers, gridlines, cursor ring, selection, cell text ──
//
// `CellGrid` has no bespoke `Backend::draw_cell_grid` method — like
// [`crate::primitives::icon_grid::IconGrid`], it paints exclusively
// through [`crate::backend::DrawsPrimitives::draw_primitive`] (see
// [`Primitive::paint`][crate::primitive::Primitive::paint] below), so
// this module is the only pixel rasteriser it has.
#[cfg(any(
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
pub(crate) mod native_surface_paint {
    use super::{Cell, CellGrid, CellGridLayout};
    use crate::event::Rect;
    use crate::paint_surface::{fit_text_prefix, PaintSurface};
    use crate::primitives::data_table::ColumnAlign;
    use crate::style::Style;
    use crate::theme::Theme;

    /// `col_x`/`col_w`'s text origin for a `text_w`-wide run under
    /// `align` — the same three-way split
    /// [`crate::primitives::data_table::native_surface_paint`]'s own
    /// (private) `align_text_x` computes, duplicated here rather than
    /// exposed from there since it's a single `match` over an already
    /// shared [`ColumnAlign`].
    fn align_text_x(col_x: f32, col_w: f32, text_w: f32, align: ColumnAlign) -> f32 {
        match align {
            ColumnAlign::Left => col_x,
            ColumnAlign::Center => col_x + (col_w - text_w) / 2.0,
            ColumnAlign::Right => col_x + col_w - text_w,
        }
    }

    /// Paint one already-formatted header label, bold and centred in
    /// `rect` — the same bold convention
    /// [`crate::primitives::data_table::native_surface_paint::paint_header`]
    /// uses for column titles.
    fn paint_header_label(surface: &mut dyn PaintSurface, label: &str, rect: Rect, theme: &Theme) {
        if rect.width <= 0.0 || rect.height <= 0.0 {
            return;
        }
        surface.surface_push_clip(rect);
        let (raw_w, _) = surface.surface_measure_text_styled(label, true);
        let (text, tw) = if raw_w <= rect.width {
            (label.to_string(), raw_w)
        } else {
            fit_text_prefix(surface, label, true, rect.width)
        };
        let (_, th) = surface.surface_measure_text_styled(&text, true);
        let x = rect.x + (rect.width - tw).max(0.0) / 2.0;
        let y = rect.y + (rect.height - th).max(0.0) / 2.0;
        surface.surface_draw_text_run_styled(
            Rect::new(x, y, tw.max(1.0), th.max(1.0)),
            &text,
            theme.foreground,
            true,
            false,
            false,
            1.0,
        );
        surface.surface_pop_clip();
    }

    /// Paint one already-formatted body cell's text, honouring its
    /// [`crate::CellStyle`] — bold/italic/underline plus an optional
    /// foreground override — via [`PaintSurface::surface_draw_text_run_styled`].
    /// Ellipsis-truncated via the same shared [`fit_text_prefix`]
    /// [`crate::primitives::data_table::native_surface_paint::paint_cell`]
    /// uses, since a per-cell style flag has no parallel in
    /// `DataTable`'s own plain [`crate::StyledText`] cells.
    fn paint_cell_text(surface: &mut dyn PaintSurface, cell: &Cell, rect: Rect, theme: &Theme) {
        if cell.text.is_empty() || rect.width <= 0.0 || rect.height <= 0.0 {
            return;
        }
        let bold = cell.style.bold.unwrap_or(false);
        surface.surface_push_clip(rect);
        let (raw_w, _) = surface.surface_measure_text_styled(&cell.text, bold);
        let (text, tw) = if raw_w <= rect.width {
            (cell.text.clone(), raw_w)
        } else {
            fit_text_prefix(surface, &cell.text, bold, rect.width)
        };
        let (_, th) = surface.surface_measure_text_styled(&text, bold);
        let align = cell.style.effective_align(&cell.text);
        let x = align_text_x(rect.x, rect.width, tw, align);
        let y = rect.y + (rect.height - th).max(0.0) / 2.0;
        let fg = cell.style.fg.unwrap_or(theme.foreground);
        surface.surface_draw_text_run_styled(
            Rect::new(x, y, tw.max(1.0), th.max(1.0)),
            &text,
            fg,
            bold,
            cell.style.italic.unwrap_or(false),
            cell.style.underline.unwrap_or(false),
            1.0,
        );
        surface.surface_pop_clip();
    }

    /// The bounding rect of the resolved rectangular selection
    /// `(top, left, bottom, right)` — all inclusive absolute indices,
    /// see [`CellGrid::selection_bounds`] — over whichever of `layout`'s
    /// visible rows/columns fall inside it, or `None` when none do
    /// (the whole selection has scrolled out of view).
    ///
    /// Takes the bounding box of the matching spans rather than summing
    /// only contiguous ones, so a selection straddling the
    /// frozen/scrolled boundary (two separate runs in `layout.rows`/
    /// `layout.cols`) paints as one solid rect spanning the gap between
    /// them — a deliberate simplification a future issue can tighten to
    /// per-run rects if that gap ever reads as visually wrong.
    fn selection_rect(
        area: Rect,
        layout: &CellGridLayout,
        top: usize,
        left: usize,
        bottom: usize,
        right: usize,
    ) -> Option<Rect> {
        let mut y_bounds: Option<(f32, f32)> = None;
        for row in &layout.rows {
            if row.index >= top && row.index <= bottom {
                let (y0, y1) = (row.start, row.start + row.size);
                y_bounds = Some(match y_bounds {
                    Some((lo, hi)) => (lo.min(y0), hi.max(y1)),
                    None => (y0, y1),
                });
            }
        }
        let mut x_bounds: Option<(f32, f32)> = None;
        for col in &layout.cols {
            if col.index >= left && col.index <= right {
                let (x0, x1) = (col.start, col.start + col.size);
                x_bounds = Some(match x_bounds {
                    Some((lo, hi)) => (lo.min(x0), hi.max(x1)),
                    None => (x0, x1),
                });
            }
        }
        match (y_bounds, x_bounds) {
            (Some((yt, yb)), Some((xl, xr))) if yb > yt && xr > xl => {
                Some(Rect::new(area.x + xl, area.y + yt, xr - xl, yb - yt))
            }
            _ => None,
        }
    }

    /// Paint `grid`'s visible headers, cell backgrounds/text, gridlines,
    /// frozen-pane dividers, selection fill and cursor ring into `area`
    /// on `surface` — the entry point every GUI backend's
    /// [`crate::primitive::Primitive::paint`] goes through (`CellGrid`
    /// has no bespoke `Backend::draw_cell_grid` method, so this is its
    /// only pixel paint path).
    ///
    /// No scrollbar thumb is painted even when
    /// [`CellGrid::show_scrollbar`] reserves a strip for one — a visual
    /// scrollbar thumb is a follow-up, not part of this issue's
    /// acceptance criteria (gridlines/headers/cursor/selection/text).
    pub(crate) fn paint(
        grid: &CellGrid,
        area: Rect,
        layout: &CellGridLayout,
        surface: &mut dyn PaintSurface,
        theme: &Theme,
        style: &Style,
    ) {
        if area.width <= 0.0 || area.height <= 0.0 {
            return;
        }
        surface.surface_push_clip(area);
        surface.surface_fill_rect(area, theme.background);

        let body_left = area.x + layout.row_header_width;
        let body_top = area.y + layout.col_header_height;
        let body_right = area.x + layout.viewport_width - layout.scrollbar_width;
        let body_bottom = area.y + layout.viewport_height - layout.scrollbar_height;

        // ── Headers ──────────────────────────────────────────────────
        if layout.col_header_height > 0.0 {
            surface.surface_fill_rect(
                Rect::new(
                    area.x,
                    area.y,
                    layout.viewport_width,
                    layout.col_header_height,
                ),
                theme.tab_bar_bg,
            );
            for col in &layout.cols {
                let x = area.x + col.start;
                if col.size <= 0.0 || x + col.size <= body_left || x >= body_right {
                    continue;
                }
                let rect = Rect::new(x, area.y, col.size, layout.col_header_height);
                paint_header_label(surface, &grid.col_label(col.index), rect, theme);
            }
        }
        if layout.row_header_width > 0.0 {
            surface.surface_fill_rect(
                Rect::new(
                    area.x,
                    area.y,
                    layout.row_header_width,
                    layout.viewport_height,
                ),
                theme.tab_bar_bg,
            );
            for row in &layout.rows {
                let y = area.y + row.start;
                if row.size <= 0.0 || y + row.size <= body_top || y >= body_bottom {
                    continue;
                }
                let rect = Rect::new(area.x, y, layout.row_header_width, row.size);
                paint_header_label(surface, &grid.row_label(row.index), rect, theme);
            }
        }

        // ── Selection fill ───────────────────────────────────────────
        // Painted *before* cell backgrounds/text, the same solid-fill-
        // first convention `DataTable::paint_row` uses for its own
        // selection/hover bands: a cell's own background (an opaque
        // `surface_fill_rect`) and its text then composite on top of
        // this alpha wash rather than the wash landing over already-
        // painted glyphs and half-washing them out.
        let (sr0, sc0, sr1, sc1) = grid.selection_bounds();
        if let Some(rect) = selection_rect(area, layout, sr0, sc0, sr1, sc1) {
            surface.surface_fill_rect_alpha(rect, theme.selection_bg, theme.selection_alpha);
        }

        // ── Cell backgrounds + text ──────────────────────────────────
        for (r_pos, row_span) in layout.rows.iter().enumerate() {
            if row_span.size <= 0.0 {
                continue;
            }
            let Some(row_cells) = grid.cells.get(r_pos) else {
                continue;
            };
            let y = area.y + row_span.start;
            for (c_pos, col_span) in layout.cols.iter().enumerate() {
                if col_span.size <= 0.0 {
                    continue;
                }
                let Some(cell) = row_cells.get(c_pos) else {
                    continue;
                };
                let rect = Rect::new(area.x + col_span.start, y, col_span.size, row_span.size);
                if let Some(bg) = cell.style.bg {
                    surface.surface_fill_rect(rect, bg);
                }
                paint_cell_text(surface, cell, rect, theme);
            }
        }

        // ── Gridlines ────────────────────────────────────────────────
        let body_w = (body_right - body_left).max(0.0);
        let body_h = (body_bottom - body_top).max(0.0);
        for row in &layout.rows {
            let y = area.y + row.start + row.size;
            if y < body_top || y > body_bottom {
                continue;
            }
            surface.surface_fill_rect(Rect::new(body_left, y - 0.5, body_w, 1.0), theme.separator);
        }
        for col in &layout.cols {
            let x = area.x + col.start + col.size;
            if x < body_left || x > body_right {
                continue;
            }
            surface.surface_fill_rect(Rect::new(x - 0.5, body_top, 1.0, body_h), theme.separator);
        }

        // ── Frozen-pane dividers ─────────────────────────────────────
        // A thicker (2px) rule right after the last frozen row/column —
        // distinct from the 1px gridlines above so the frozen boundary
        // reads as structural, the same way a spreadsheet's own "freeze
        // panes" divider does.
        if layout.frozen_rows > 0 && layout.frozen_rows < layout.rows.len() {
            let last = &layout.rows[layout.frozen_rows - 1];
            let y = area.y + last.start + last.size;
            surface.surface_fill_rect(Rect::new(body_left, y - 1.0, body_w, 2.0), theme.separator);
        }
        if layout.frozen_cols > 0 && layout.frozen_cols < layout.cols.len() {
            let last = &layout.cols[layout.frozen_cols - 1];
            let x = area.x + last.start + last.size;
            surface.surface_fill_rect(Rect::new(x - 1.0, body_top, 2.0, body_h), theme.separator);
        }

        // ── Cursor ring ──────────────────────────────────────────────
        if grid.has_focus {
            let cursor_rect = layout
                .rows
                .iter()
                .find(|r| r.index == grid.cursor.0)
                .zip(layout.cols.iter().find(|c| c.index == grid.cursor.1))
                .map(|(r, c)| Rect::new(area.x + c.start, area.y + r.start, c.size, r.size));
            if let Some(rect) = cursor_rect {
                surface.surface_stroke_rect(
                    rect,
                    theme.focus_ring_color(),
                    style.focus_ring_width.max(1.0),
                );
            }
        }

        surface.surface_pop_clip();
    }
}

/// [`crate::primitive::Primitive::Layout`] for [`CellGrid`]: the
/// absolute `area` [`CellGrid::layout`] was resolved for, plus the
/// resolved [`CellGridLayout`] itself.
///
/// `#[non_exhaustive]`: backend-returned, never constructed by a
/// consumer via struct literal, same convention as [`CellGridLayout`]
/// itself.
///
/// Unlike [`crate::DataTable`]/[`crate::ListView`]/[`crate::IconGrid`]'s
/// own `Primitive::Layout` types, not feature-gated: `CellGrid` has no
/// bespoke `Backend::draw_cell_grid` method, so [`crate::frame::Surface::CellGrid`]
/// paints exclusively through [`crate::backend::DrawsPrimitives::draw_primitive`],
/// which `ScreenLayout::draw` calls unconditionally — gating this type
/// behind a backend feature would make `frame.rs` fail to compile in a
/// zero-backend build.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct CellGridPrimitiveLayout {
    pub area: Rect,
    /// The grid-local layout itself.
    pub layout: CellGridLayout,
}

impl CellGridPrimitiveLayout {
    /// Hit-test at an absolute `(x, y)` — subtracts [`Self::area`]'s
    /// origin before delegating to [`CellGridLayout::hit_test`], whose
    /// own coordinates are grid-local.
    pub fn hit_test(&self, x: f32, y: f32) -> CellGridHit {
        self.layout.hit_test(x - self.area.x, y - self.area.y)
    }
}

impl crate::primitive::Primitive for CellGrid {
    type Layout = CellGridPrimitiveLayout;
    type Event = CellGridEvent;

    fn layout(&self, ctx: &crate::primitive::LayoutCtx, rect: Rect) -> Self::Layout {
        // Same runtime discriminator `ListView`/`IconGrid`'s own
        // `Primitive::layout` use: a terminal cell's `line_height` is
        // architecturally always exactly `1.0`, so this tells a TUI
        // caller apart from a pixel backend even when both features are
        // compiled into the same binary.
        #[cfg(feature = "tui")]
        if ctx.line_height <= 1.0 {
            // Route through the exact resolver `crate::tui::draw_cell_grid`
            // itself calls (`tui_cell_grid_layout`), so this layout can
            // never drift from what TUI actually paints —
            // `Self::paint_cells` below makes the matching paint-side
            // guarantee.
            let area = crate::tui::backend::q_rect_to_ratatui(rect);
            let layout = crate::tui::tui_cell_grid_layout(area, self);
            return CellGridPrimitiveLayout { area: rect, layout };
        }

        // No live text measurer reaches `Primitive::layout` on a pixel
        // backend (its signature carries only `LayoutCtx`'s flat
        // `char_width`), so the row-header gutter's width is estimated
        // from the sheet's own digit count rather than any backend's
        // real glyph metrics — the same no-live-font fallback
        // [`crate::primitives::layout_metrics::pixel_tree_table_layout`]
        // already applies for a pixel backend with no live glyph
        // measurer reachable.
        let col_header_height = if self.show_col_headers {
            ctx.line_height
        } else {
            0.0
        };
        let row_header_width = if self.show_row_headers {
            let digits = if self.row_count == 0 {
                1
            } else {
                self.row_count.to_string().chars().count()
            };
            (digits as f32 + 1.0) * ctx.char_width
        } else {
            0.0
        };
        let (scrollbar_w, scrollbar_h) = if self.show_scrollbar {
            (ctx.char_width, ctx.line_height)
        } else {
            (0.0, 0.0)
        };
        let layout = self.layout(
            rect.width,
            rect.height,
            col_header_height,
            row_header_width,
            scrollbar_w,
            scrollbar_h,
        );
        CellGridPrimitiveLayout { area: rect, layout }
    }

    /// Paints through [`native_surface_paint::paint`] on every pixel
    /// backend (GTK/macOS/Win); a `tui`-only build has no
    /// [`crate::paint_surface::PaintSurface`] to paint into at all, so
    /// `self` reaching the screen there goes through [`Self::paint_cells`]
    /// below instead, mirroring
    /// [`crate::primitives::icon_grid::IconGrid::paint`]'s identical
    /// split.
    fn paint(
        &self,
        layout: &Self::Layout,
        surface: &mut dyn crate::paint_surface::PaintSurface,
        theme: &crate::theme::Theme,
        style: &crate::style::Style,
    ) {
        #[cfg(any(
            feature = "gtk",
            feature = "win",
            all(feature = "macos", target_os = "macos")
        ))]
        {
            native_surface_paint::paint(self, layout.area, &layout.layout, surface, theme, style);
        }
        #[cfg(not(any(
            feature = "gtk",
            feature = "win",
            all(feature = "macos", target_os = "macos")
        )))]
        {
            let _ = (layout, surface, theme, style);
        }
    }

    /// Delegates straight to [`crate::tui::draw_cell_grid`] — the exact
    /// function `TuiBackend` would call for a bespoke `draw_cell_grid`
    /// `Backend` method, so painting `CellGrid` through
    /// [`crate::primitive::Primitive`] on TUI is byte-for-byte identical
    /// by construction.
    #[cfg(feature = "tui")]
    fn paint_cells(
        &self,
        layout: &Self::Layout,
        buf: &mut ratatui::buffer::Buffer,
        theme: &crate::theme::Theme,
    ) {
        let area = crate::tui::backend::q_rect_to_ratatui(layout.area);
        crate::tui::draw_cell_grid(buf, area, self, theme);
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
            loaded_row_start: 0,
            loaded_col_start: 0,
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

    #[test]
    fn row_heights_and_col_widths_overrides_resize_only_their_own_absolute_index() {
        let mut g = grid();
        g.row_scroll = 10;
        // Absolute row 11 (the scrolled body's first row, per the
        // boundary test above) is 3 units tall instead of the default 1;
        // every other row keeps the default.
        g.row_heights.insert(11, 3.0);
        // Absolute column 0 (the frozen column) is 9 units wide instead
        // of the default 4.
        g.col_widths.insert(0, 9.0);
        let layout = g.layout(100.0, 50.0, 1.0, 4.0, 1.0, 1.0);

        assert_eq!(layout.rows[1].index, 11);
        assert_eq!(layout.rows[1].size, 3.0);
        // The next row after the overridden one starts 3 units later,
        // not 1 — the override shifted every following `pos`, not just
        // its own span.
        assert_eq!(layout.rows[2].index, 12);
        assert_eq!(layout.rows[2].start, layout.rows[1].start + 3.0);

        assert_eq!(layout.cols[0].index, 0);
        assert_eq!(layout.cols[0].size, 9.0);
        assert_eq!(layout.cols[1].start, layout.cols[0].start + 9.0);

        // A click inside the tall row's extra height (2 units into its
        // 3-unit band, past where the old 1-unit height would have ended)
        // still resolves to that row, not the one after it.
        assert_eq!(
            layout.hit_test(layout.cols[0].start, layout.rows[1].start + 2.5),
            CellGridHit::Cell(11, 0)
        );
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

    // ── cells_window / cells_requested — two-axis virtualisation ────

    /// A `row_count`×`col_count` sheet with no frozen rows/columns and
    /// nothing loaded into `cells` yet.
    fn virtual_grid(row_count: usize, col_count: usize) -> CellGrid {
        let mut g = grid();
        g.row_count = row_count;
        g.col_count = col_count;
        g.frozen_rows = 0;
        g.frozen_cols = 0;
        g.row_scroll = 0;
        g.col_scroll = 0;
        g.cells = vec![];
        g
    }

    /// Fill `g.cells` with a `rows`×`cols` block of blank cells, so
    /// `g.cells_window()` reports exactly that much loaded. Does **not**
    /// set `loaded_row_start`/`loaded_col_start` — callers that care where
    /// the block starts must set those explicitly, since (per the module
    /// doc) they are independent of `row_scroll`/`col_scroll`.
    fn load_cells(g: &mut CellGrid, rows: usize, cols: usize) {
        g.cells = (0..rows)
            .map(|_| (0..cols).map(|_| Cell::plain("")).collect())
            .collect();
    }

    /// [`load_cells`] plus setting `loaded_row_start`/`loaded_col_start` to
    /// `row_start`/`col_start` — simulates an app that filled `cells` for
    /// exactly the block [`CellGrid::cells_requested`] would name, the way
    /// a real caller reacts to [`CellGridEvent::CellsRequested`].
    fn load_cells_at(
        g: &mut CellGrid,
        row_start: usize,
        col_start: usize,
        rows: usize,
        cols: usize,
    ) {
        load_cells(g, rows, cols);
        g.loaded_row_start = row_start;
        g.loaded_col_start = col_start;
    }

    #[test]
    fn cells_window_excludes_the_frozen_leading_rows_and_columns() {
        let mut g = grid();
        // One frozen row/column plus 4 scrolled-body rows/columns: 5
        // total along each axis. `loaded_row_start`/`loaded_col_start`
        // name where the scrolled body starts — independent of
        // `row_scroll`/`col_scroll`, which only drive `layout`.
        let (row_start, col_start) = (g.frozen_rows + 50, g.frozen_cols + 20);
        load_cells_at(&mut g, row_start, col_start, 5, 5);
        let window = g.cells_window();
        assert_eq!(window.row_start, g.frozen_rows + 50);
        assert_eq!(window.col_start, g.frozen_cols + 20);
        assert_eq!(window.rows, 4);
        assert_eq!(window.cols, 4);
        assert_eq!(window.total_rows, 1000);
        assert_eq!(window.total_cols, 1000);
    }

    #[test]
    fn cells_window_reports_the_loaded_block_at_a_million_row_scale() {
        let mut g = virtual_grid(1_000_000, 16_384);
        load_cells_at(&mut g, 500_000, 8_000, 20, 30);
        let window = g.cells_window();
        assert_eq!(window.row_start, 500_000);
        assert_eq!(window.col_start, 8_000);
        assert_eq!(window.rows, 20);
        assert_eq!(window.cols, 30);
        assert_eq!(window.total_rows, 1_000_000);
        assert_eq!(window.total_cols, 16_384);
    }

    #[test]
    fn cells_window_is_independent_of_row_scroll_and_col_scroll() {
        // `loaded_row_start`/`loaded_col_start` name where `cells` was
        // filled from; `row_scroll`/`col_scroll` only drive `layout`.
        // Moving the scroll position must not move the reported window.
        let mut g = virtual_grid(1_000_000, 16_384);
        load_cells_at(&mut g, 10, 10, 20, 30);
        let before = g.cells_window();
        g.row_scroll = 999_000;
        g.col_scroll = 16_000;
        let after = g.cells_window();
        assert_eq!(before, after);
    }

    #[test]
    fn cells_requested_is_none_once_the_loaded_window_covers_the_viewport() {
        let mut g = virtual_grid(1_000_000, 16_384);
        g.row_scroll = 500_000;
        g.col_scroll = 8_000;
        let layout = g.layout(100.0, 50.0, 1.0, 8.0, 0.0, 0.0);
        load_cells_at(
            &mut g,
            layout.rows[0].index,
            layout.cols[0].index,
            layout.rows.len(),
            layout.cols.len(),
        );
        assert_eq!(g.cells_requested(&layout), None);
    }

    #[test]
    fn cells_requested_fires_when_scrolling_past_an_already_loaded_window() {
        // Load exactly the viewport at one scroll position, then scroll
        // away *without* touching `cells` or `loaded_row_start`/
        // `loaded_col_start`. A loaded block that's still the right size
        // but now sits at the wrong place must still trigger a request —
        // this is the steady-state scroll the acceptance bar names: "a
        // 1,000,000 × 16,384 virtual grid scrolls with the app supplying
        // only the visible block".
        let mut g = virtual_grid(1_000_000, 16_384);
        g.row_scroll = 0;
        g.col_scroll = 0;
        let first = g.layout(100.0, 50.0, 1.0, 8.0, 0.0, 0.0);
        load_cells_at(
            &mut g,
            first.rows[0].index,
            first.cols[0].index,
            first.rows.len(),
            first.cols.len(),
        );
        assert_eq!(g.cells_requested(&first), None);

        g.row_scroll = 500_000;
        g.col_scroll = 8_000;
        let scrolled = g.layout(100.0, 50.0, 1.0, 8.0, 0.0, 0.0);
        // `cells`/`loaded_row_start`/`loaded_col_start` are unchanged —
        // only the viewport moved.
        assert_eq!(g.cells.len(), first.rows.len());

        let event = g
            .cells_requested(&scrolled)
            .expect("the loaded window is stale even though it's the right size");
        assert_eq!(
            event,
            CellGridEvent::CellsRequested {
                row_start: scrolled.rows[0].index,
                col_start: scrolled.cols[0].index,
                rows: scrolled.rows.len(),
                cols: scrolled.cols.len(),
            }
        );
    }

    #[test]
    fn cells_requested_fires_when_jumping_to_the_far_corner_with_a_stale_window() {
        let mut g = virtual_grid(1_000_000, 16_384);
        g.row_scroll = 0;
        g.col_scroll = 0;
        let first = g.layout(100.0, 50.0, 1.0, 8.0, 0.0, 0.0);
        load_cells_at(
            &mut g,
            first.rows[0].index,
            first.cols[0].index,
            first.rows.len(),
            first.cols.len(),
        );
        assert_eq!(g.cells_requested(&first), None);

        // Jump straight to the far corner without reloading `cells`.
        g.row_scroll = 1_000_000 - 5;
        g.col_scroll = 16_384 - 5;
        let far = g.layout(100.0, 50.0, 1.0, 8.0, 0.0, 0.0);
        let last_row = far.rows.last().expect("far corner must have a row").index;
        let last_col = far.cols.last().expect("far corner must have a col").index;
        assert_eq!(last_row, 1_000_000 - 1);
        assert_eq!(last_col, 16_384 - 1);

        let event = g
            .cells_requested(&far)
            .expect("jumping past a stale loaded window must request the new block");
        let CellGridEvent::CellsRequested {
            row_start,
            col_start,
            rows,
            cols,
        } = event
        else {
            panic!("expected CellsRequested, got {event:?}");
        };
        assert_eq!(row_start, far.rows[0].index);
        assert_eq!(col_start, far.cols[0].index);
        assert_eq!(row_start + rows - 1, last_row);
        assert_eq!(col_start + cols - 1, last_col);
    }

    #[test]
    fn cells_requested_names_the_visible_block_after_scrolling_with_nothing_loaded() {
        let mut g = virtual_grid(1_000_000, 16_384);
        g.row_scroll = 500_000;
        g.col_scroll = 8_000;
        let layout = g.layout(100.0, 50.0, 1.0, 8.0, 0.0, 0.0);
        let expected_row_start = layout.rows[0].index;
        let expected_col_start = layout.cols[0].index;
        let expected_rows = layout.rows.len();
        let expected_cols = layout.cols.len();

        let event = g.cells_requested(&layout).expect("nothing loaded yet");
        assert_eq!(
            event,
            CellGridEvent::CellsRequested {
                row_start: expected_row_start,
                col_start: expected_col_start,
                rows: expected_rows,
                cols: expected_cols,
            }
        );
    }

    #[test]
    fn cells_requested_matches_the_viewport_at_the_far_corner() {
        let mut g = virtual_grid(1_000_000, 16_384);
        g.row_scroll = 1_000_000 - 5;
        g.col_scroll = 16_384 - 5;
        let layout = g.layout(100.0, 50.0, 1.0, 8.0, 0.0, 0.0);
        let last_row = layout
            .rows
            .last()
            .expect("far corner must still have a row")
            .index;
        let last_col = layout
            .cols
            .last()
            .expect("far corner must still have a col")
            .index;
        assert_eq!(last_row, 1_000_000 - 1);
        assert_eq!(last_col, 16_384 - 1);

        let event = g
            .cells_requested(&layout)
            .expect("nothing loaded at the far corner");
        let CellGridEvent::CellsRequested {
            row_start,
            col_start,
            rows,
            cols,
        } = event
        else {
            panic!("expected CellsRequested, got {event:?}");
        };
        assert_eq!(row_start, layout.rows[0].index);
        assert_eq!(col_start, layout.cols[0].index);
        assert_eq!(row_start + rows - 1, last_row);
        assert_eq!(col_start + cols - 1, last_col);
    }

    #[test]
    fn cells_requested_grows_with_a_larger_viewport_after_resizing() {
        let mut g = virtual_grid(1_000_000, 16_384);
        g.row_scroll = 100;
        g.col_scroll = 100;
        let small = g.layout(40.0, 20.0, 1.0, 8.0, 0.0, 0.0);
        load_cells_at(
            &mut g,
            small.rows[0].index,
            small.cols[0].index,
            small.rows.len(),
            small.cols.len(),
        );
        assert_eq!(g.cells_requested(&small), None);

        // Resize to a much larger viewport without reloading `cells` —
        // the loaded window does not cover every visible row/column, so
        // a fresh request names the bigger block the new viewport needs.
        let big = g.layout(400.0, 200.0, 1.0, 8.0, 0.0, 0.0);
        let event = g
            .cells_requested(&big)
            .expect("a grown viewport must request more cells");
        let CellGridEvent::CellsRequested { rows, cols, .. } = event else {
            panic!("expected CellsRequested, got {event:?}");
        };
        assert!(rows > small.rows.len());
        assert!(cols > small.cols.len());
    }

    #[test]
    fn cells_requested_fires_on_a_shrink_then_scroll_even_though_the_stale_window_contains_the_viewport(
    ) {
        // `cells` is indexed *positionally* against `layout.rows`/`layout.cols`
        // (see the module doc), so a loaded window merely *containing* the
        // viewport isn't enough — it must start at the same absolute row/col or
        // the body paints shifted. A viewport shrink alone must stay `None`
        // (the app legitimately kept its larger block and the origin still
        // matches), but scrolling inside that stale block afterwards must
        // fire a fresh request.
        let mut g = virtual_grid(1_000_000, 16_384);
        g.row_scroll = 500;
        let tall = g.layout(400.0, 50.0, 0.0, 0.0, 0.0, 0.0);
        load_cells_at(&mut g, 500, 0, tall.rows.len(), tall.cols.len());
        assert_eq!(g.cells_requested(&tall), None);

        // Viewport shrinks to 40 rows; the loaded 50-row block still starts
        // at the same origin, so no request fires and `cells` stays as-is.
        let short = g.layout(400.0, 40.0, 0.0, 0.0, 0.0, 0.0);
        assert_eq!(g.cells_requested(&short), None);
        assert_eq!(g.cells.len(), tall.rows.len());

        // Scroll ten rows inside the stale (larger) loaded block. The old
        // containment check would call this covered (500 <= 510 && 550 >=
        // 550); the origin no longer matches the viewport's first row, so a
        // fresh request must fire naming the new block.
        g.row_scroll = 510;
        let after = g.layout(400.0, 40.0, 0.0, 0.0, 0.0, 0.0);
        let event = g
            .cells_requested(&after)
            .expect("a stale loaded window whose origin has drifted must be requested again");
        assert_eq!(
            event,
            CellGridEvent::CellsRequested {
                row_start: after.rows[0].index,
                col_start: after.cols[0].index,
                rows: after.rows.len(),
                cols: after.cols.len(),
            }
        );
    }
}

#[cfg(test)]
mod a11y_tests {
    use super::*;
    use crate::a11y::{A11yInfo, A11yPrimitive, A11yRole};

    /// A 3×3 grid (no frozen panes, no scroll) with a 2×2 selection
    /// anchored at `(0, 0)` and cursor at `(1, 1)` — small enough that
    /// every windowed position's absolute index is just itself, so the
    /// assertions below can use plain `0`/`1`/`2` without first
    /// reasoning about `windowed_row`/`windowed_col`.
    fn grid_3x3_with_selection() -> CellGrid {
        let row = |label: &str| {
            (0..3)
                .map(|c| Cell::plain(format!("{label}{c}")))
                .collect::<Vec<_>>()
        };
        CellGrid {
            id: WidgetId::new("grid:a11y"),
            row_count: 3,
            col_count: 3,
            frozen_rows: 0,
            frozen_cols: 0,
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
            cursor: (1, 1),
            selection_anchor: Some((0, 0)),
            cells: vec![row("r0"), row("r1"), row("r2")],
            loaded_row_start: 0,
            loaded_col_start: 0,
            has_focus: true,
            show_scrollbar: false,
        }
    }

    #[test]
    fn a11y_info_reports_a_table_role() {
        let grid = grid_3x3_with_selection();
        let info = grid.a11y_info();
        assert_eq!(
            info,
            A11yInfo::new()
                .with_role(A11yRole::Table)
                .with_label("Cell grid")
                .with_set_size(3)
        );
    }

    #[test]
    fn a11y_children_include_a_header_row_of_column_letters() {
        let grid = grid_3x3_with_selection();
        let children = grid.a11y_children();
        let header = children
            .iter()
            .find(|c| c.key == "header")
            .expect("a column-header row child");
        assert_eq!(
            header.info,
            A11yInfo::new()
                .with_role(A11yRole::Row)
                .with_label("Header")
        );
        let labels: Vec<_> = header
            .children
            .iter()
            .map(|c| c.info.a11y_label.clone().unwrap_or_default())
            .collect();
        assert_eq!(labels, vec!["A", "B", "C"]);
    }

    #[test]
    fn a11y_children_carry_row_header_and_cell_text_with_absolute_indices() {
        let grid = grid_3x3_with_selection();
        let children = grid.a11y_children();
        let row1 = children
            .iter()
            .find(|c| c.key == "row.1")
            .expect("row 1's child");
        assert_eq!(row1.info, A11yInfo::new().with_role(A11yRole::Row));

        let row_header = row1
            .children
            .iter()
            .find(|c| c.key == "rowheader.1")
            .expect("row 1's row-header cell");
        assert_eq!(row_header.info.a11y_label.as_deref(), Some("2"));

        let cell = row1
            .children
            .iter()
            .find(|c| c.key == "cell.1.1")
            .expect("absolute cell (1, 1)");
        assert_eq!(cell.info.a11y_label.as_deref(), Some("r11"));
    }

    #[test]
    fn a11y_children_mark_cells_inside_the_selection_rectangle_selected() {
        let grid = grid_3x3_with_selection();
        let children = grid.a11y_children();
        let cell_at = |row_key: &str, cell_key: &str| {
            children
                .iter()
                .find(|c| c.key == row_key)
                .and_then(|r| r.children.iter().find(|c| c.key == cell_key))
                .unwrap_or_else(|| panic!("missing {row_key}/{cell_key}"))
        };

        // Selection spans (0, 0)..=(1, 1) — every cell inside that
        // rectangle reads selected, and (2, 2), well outside it, does
        // not.
        assert_eq!(cell_at("row.0", "cell.0.0").info.selected, Some(true));
        assert_eq!(cell_at("row.1", "cell.1.1").info.selected, Some(true));
        assert_eq!(cell_at("row.2", "cell.2.2").info.selected, Some(false));
    }

    #[test]
    fn a11y_focus_key_names_the_cursor_cell() {
        let grid = grid_3x3_with_selection();
        assert_eq!(grid.a11y_focus_key(), Some("cell.1.1".to_string()));
    }

    #[test]
    fn windowed_index_passes_through_the_frozen_block_then_adds_scroll() {
        let mut g = grid_3x3_with_selection();
        g.frozen_rows = 1;
        g.row_scroll = 5;
        g.row_count = 20;
        // Position 0 is still the frozen row (absolute index 0);
        // position 1 is the scrolled body's first row, which is
        // `frozen_rows + row_scroll` (1 + 5 = 6), not `row_scroll` alone.
        assert_eq!(g.windowed_row(0), 0);
        assert_eq!(g.windowed_row(1), 6);
        assert_eq!(g.windowed_row(2), 7);
    }
}

#[cfg(all(
    test,
    any(
        feature = "gtk",
        feature = "win",
        all(feature = "macos", target_os = "macos")
    )
))]
mod native_surface_paint_tests {
    use super::native_surface_paint::paint;
    use super::*;
    use crate::backend::ImagePaintResult;
    use crate::paint_surface::PaintSurface;
    use crate::style::Style;
    use crate::theme::Theme;
    use crate::Image;

    /// Records every surface verb this module's paint uses — mirrors
    /// [`crate::primitives::tree_table::native_surface_paint`]'s own test
    /// double, so this test runs on any host without Core
    /// Graphics/Direct2D/Pango.
    #[derive(Default)]
    struct RecordingSurface {
        fills: Vec<(Rect, Color)>,
        fill_alphas: Vec<(Rect, Color, f32)>,
        strokes: Vec<(Rect, Color, f32)>,
        text_runs: Vec<(Rect, String, Color, bool)>,
        /// One tag per paint verb call, in call order, so a test can
        /// assert *ordering* across the otherwise-separate `fills`/
        /// `fill_alphas`/`text_runs` vecs above — in particular that the
        /// selection's `fill_alpha` precedes the cell `text_run`s it
        /// should composite underneath, not over.
        calls: Vec<&'static str>,
    }

    impl PaintSurface for RecordingSurface {
        fn surface_begin_frame(&mut self, _viewport: crate::Viewport) {}
        fn surface_end_frame(&mut self) {}
        fn surface_viewport(&self) -> crate::Viewport {
            crate::Viewport::new(300.0, 200.0, 1.0)
        }
        fn surface_line_height(&self) -> f32 {
            16.0
        }
        fn surface_char_width(&self) -> f32 {
            8.0
        }
        fn surface_measure_text(&self, text: &str) -> (f32, f32) {
            (text.chars().count() as f32 * 8.0, 16.0)
        }
        fn surface_fill_rect(&mut self, rect: Rect, color: Color) {
            self.fills.push((rect, color));
            self.calls.push("fill");
        }
        fn surface_fill_rounded_rect(&mut self, _rect: Rect, _radius: f32, _color: Color) {}
        fn surface_fill_rect_alpha(&mut self, rect: Rect, color: Color, alpha: f32) {
            self.fill_alphas.push((rect, color, alpha));
            self.calls.push("fill_alpha");
        }
        fn surface_stroke_rect(&mut self, rect: Rect, color: Color, stroke_width: f32) {
            self.strokes.push((rect, color, stroke_width));
            self.calls.push("stroke");
        }
        fn surface_draw_text_run(&mut self, rect: Rect, text: &str, color: Color) {
            self.text_runs.push((rect, text.to_string(), color, false));
            self.calls.push("text_run");
        }
        fn surface_draw_text_run_styled(
            &mut self,
            rect: Rect,
            text: &str,
            color: Color,
            bold: bool,
            _italic: bool,
            _underline: bool,
            _scale_x: f32,
        ) {
            self.text_runs.push((rect, text.to_string(), color, bold));
            self.calls.push("text_run");
        }
        fn surface_draw_line(
            &mut self,
            _from: crate::Point,
            _to: crate::Point,
            _color: Color,
            _stroke_width: f32,
        ) {
        }
        fn surface_push_clip(&mut self, _rect: Rect) {}
        fn surface_pop_clip(&mut self) {}
        fn surface_draw_image(&mut self, _rect: Rect, _image: &Image) -> ImagePaintResult {
            ImagePaintResult::Unsupported
        }
    }

    /// A 4×4 grid, one frozen leading row/column, with a 2×2 selection
    /// and a cursor at `(1, 1)` — small enough that every visible span's
    /// absolute index matches [`CellGridLayout::rows`]/`cols` directly.
    fn grid() -> CellGrid {
        let row = |label: &str| {
            (0..4)
                .map(|c| Cell::plain(format!("{label}{c}")))
                .collect::<Vec<_>>()
        };
        CellGrid {
            id: WidgetId::new("grid:paint"),
            row_count: 4,
            col_count: 4,
            frozen_rows: 1,
            frozen_cols: 1,
            row_scroll: 0,
            col_scroll: 0,
            default_row_height: 10.0,
            default_col_width: 40.0,
            row_heights: BTreeMap::new(),
            col_widths: BTreeMap::new(),
            show_row_headers: true,
            show_col_headers: true,
            row_labels: BTreeMap::new(),
            col_labels: BTreeMap::new(),
            cursor: (1, 1),
            selection_anchor: Some((0, 0)),
            cells: vec![row("r0"), row("r1"), row("r2"), row("r3")],
            loaded_row_start: 0,
            loaded_col_start: 0,
            has_focus: true,
            show_scrollbar: false,
        }
    }

    #[test]
    fn column_and_row_headers_paint_their_labels_at_the_resolved_positions() {
        let grid = grid();
        let layout = grid.layout(200.0, 100.0, 10.0, 15.0, 0.0, 0.0);
        let area = Rect::new(0.0, 0.0, 200.0, 100.0);
        let mut surface = RecordingSurface::default();
        let theme = Theme::default();
        let style = Style::default();
        paint(&grid, area, &layout, &mut surface, &theme, &style);

        let col0 = layout.cols[0];
        let header = surface
            .text_runs
            .iter()
            .find(|(_, t, _, bold)| t == "A" && *bold)
            .expect("column header A must paint, bold");
        assert!(
            header.0.x >= col0.start && header.0.x < col0.start + col0.size,
            "column header A must paint within column 0's own span"
        );
        assert!(header.0.y < layout.col_header_height);

        let row0 = layout.rows[0];
        let row_header = surface
            .text_runs
            .iter()
            .find(|(_, t, _, bold)| t == "1" && *bold)
            .expect("row header 1 must paint, bold");
        assert!(
            row_header.0.y >= row0.start && row_header.0.y < row0.start + row0.size,
            "row header 1 must paint within row 0's own span"
        );
        assert!(row_header.0.x < layout.row_header_width);
    }

    #[test]
    fn cursor_ring_strokes_exactly_the_active_cells_resolved_rect() {
        let grid = grid();
        let layout = grid.layout(200.0, 100.0, 10.0, 15.0, 0.0, 0.0);
        let area = Rect::new(0.0, 0.0, 200.0, 100.0);
        let mut surface = RecordingSurface::default();
        let theme = Theme::default();
        let style = Style::default();
        paint(&grid, area, &layout, &mut surface, &theme, &style);

        let row = layout.rows.iter().find(|r| r.index == 1).unwrap();
        let col = layout.cols.iter().find(|c| c.index == 1).unwrap();
        let expected = Rect::new(col.start, row.start, col.size, row.size);
        assert_eq!(
            surface.strokes.len(),
            1,
            "exactly one cursor ring must be stroked"
        );
        assert_eq!(surface.strokes[0].0, expected);
    }

    #[test]
    fn no_cursor_ring_is_painted_when_the_grid_is_not_focused() {
        let mut grid = grid();
        grid.has_focus = false;
        let layout = grid.layout(200.0, 100.0, 10.0, 15.0, 0.0, 0.0);
        let area = Rect::new(0.0, 0.0, 200.0, 100.0);
        let mut surface = RecordingSurface::default();
        let theme = Theme::default();
        let style = Style::default();
        paint(&grid, area, &layout, &mut surface, &theme, &style);
        assert!(surface.strokes.is_empty());
    }

    #[test]
    fn selection_fill_covers_exactly_the_selected_rows_and_columns() {
        let grid = grid();
        let layout = grid.layout(200.0, 100.0, 10.0, 15.0, 0.0, 0.0);
        let area = Rect::new(0.0, 0.0, 200.0, 100.0);
        let mut surface = RecordingSurface::default();
        let theme = Theme::default();
        let style = Style::default();
        paint(&grid, area, &layout, &mut surface, &theme, &style);

        // Selection is (0, 0)..=(1, 1) — rows/cols 0 and 1.
        let r0 = layout.rows.iter().find(|r| r.index == 0).unwrap();
        let r1 = layout.rows.iter().find(|r| r.index == 1).unwrap();
        let c0 = layout.cols.iter().find(|c| c.index == 0).unwrap();
        let c1 = layout.cols.iter().find(|c| c.index == 1).unwrap();
        let expected = Rect::new(
            c0.start.min(c1.start),
            r0.start.min(r1.start),
            (c1.start + c1.size).max(c0.start + c0.size) - c0.start.min(c1.start),
            (r1.start + r1.size).max(r0.start + r0.size) - r0.start.min(r1.start),
        );
        assert_eq!(surface.fill_alphas.len(), 1);
        assert_eq!(surface.fill_alphas[0].0, expected);
        assert_eq!(surface.fill_alphas[0].1, theme.selection_bg);
        assert_eq!(surface.fill_alphas[0].2, theme.selection_alpha);

        // The selection wash must land *before* the selected cell's own
        // background/text, so it composites underneath them rather than
        // over already-painted glyphs — `DataTable::paint_row`'s own
        // solid-fill-first convention (`data_table.rs`). Pin this to
        // cell (0, 0)'s own text run specifically (not just the first
        // `text_run` of any kind), since the column/row headers paint
        // their own bold `text_run`s earlier still and would otherwise
        // make this assertion trivially pass regardless of ordering.
        let fill_alpha_pos = surface
            .calls
            .iter()
            .position(|c| *c == "fill_alpha")
            .expect("the selection fill must be recorded");
        let nth_text_run = surface
            .text_runs
            .iter()
            .position(|(_, t, _, _)| t == "r00")
            .expect("cell (0, 0)'s own text must have painted");
        let cell_text_pos = surface
            .calls
            .iter()
            .enumerate()
            .filter(|(_, c)| **c == "text_run")
            .nth(nth_text_run)
            .map(|(i, _)| i)
            .expect("as many text_run calls as text_runs entries");
        assert!(
            fill_alpha_pos < cell_text_pos,
            "selection fill (at {fill_alpha_pos}) must paint before the selected cell's own text (at {cell_text_pos})"
        );
    }

    #[test]
    fn cell_text_paints_with_its_own_style_fg_and_bold() {
        let mut grid = grid();
        grid.cells[2][2].style.fg = Some(Color::rgb(200, 10, 10));
        grid.cells[2][2].style.bold = Some(true);
        let layout = grid.layout(200.0, 100.0, 10.0, 15.0, 0.0, 0.0);
        let area = Rect::new(0.0, 0.0, 200.0, 100.0);
        let mut surface = RecordingSurface::default();
        let theme = Theme::default();
        let style = Style::default();
        paint(&grid, area, &layout, &mut surface, &theme, &style);

        let run = surface
            .text_runs
            .iter()
            .find(|(_, t, _, _)| t == "r22")
            .expect("cell (2, 2)'s text must paint");
        assert_eq!(run.2, Color::rgb(200, 10, 10));
        assert!(run.3, "the cell's own bold override must apply");
    }
}
