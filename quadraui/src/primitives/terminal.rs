//! `Terminal` primitive: a 2D grid of styled cells, used as the rendering
//! surface for VT100-compatible terminal emulators.
//!
//! The primitive is a *snapshot* — the source-of-truth (vimcode's
//! `vt100::Parser` + `TerminalPane` history ring buffer) lives in the
//! engine. Each frame the engine builds a fresh `Terminal` describing
//! the cells visible at the current scroll offset, including any
//! selection / cursor / find-match overlays.
//!
//! Per-cell foreground/background are explicit `Color` values rather than
//! palette indices — vimcode resolves vt100 palette colors through the
//! active theme before populating the cell grid, so the primitive is
//! palette-agnostic.
//!
//! # Backend contract
//!
//! **Purely declarative.** Iterate `cells[row][col]` and rasterise each
//! cell at its grid position. Per-cell `bold` / `italic` / `underline`
//! flags map to the backend's font/attr system. The `selected` /
//! `is_cursor` / `is_find_match` overlays use theme colours; backends
//! typically invert fg/bg for cursor cells and apply a colour
//! highlight for selection / find matches.
//!
//! Mouse interaction (selection drag, click-to-position) and keyboard
//! input (forward to PTY) live **outside** the primitive — they're the
//! app/backend's responsibility. The primitive is a paint snapshot,
//! not an interactive widget.
//!
//! For high-FPS terminals (60+ fps), backends may compare consecutive
//! `Terminal` snapshots and only repaint changed cells. Reference
//! implementations currently repaint the whole grid each frame — fine
//! for typical workloads, optimise when profiling shows it's hot.

use crate::event::Rect;
use crate::types::{Color, Modifiers, WidgetId};
use serde::{Deserialize, Serialize};

/// Declarative description of a terminal cell grid.
///
/// `cells[row][col]` — outer Vec is rows top-to-bottom, inner Vec is
/// columns left-to-right. Rows may be ragged (different lengths) but
/// backends should treat missing trailing cells as blank.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Terminal {
    pub id: WidgetId,
    pub cells: Vec<Vec<TerminalCell>>,
    /// When present, the rasteriser draws a themed scrollbar using the
    /// same style as editor scrollbars (`Backend::draw_scrollbar`).
    #[serde(default)]
    pub scrollbar: Option<TerminalScrollbar>,
}

/// Scrollbar state for a `Terminal`. The rasteriser constructs a
/// [`crate::Scrollbar::vertical`] from these values and the terminal's
/// rect, then delegates to `Backend::draw_scrollbar` for themed
/// rendering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalScrollbar {
    pub total_lines: usize,
    pub visible_lines: usize,
    pub scroll_offset: usize,
    /// When `true`, `scroll_offset = 0` means "at the bottom" (live
    /// view) and increasing offset scrolls up into history. The
    /// rasteriser flips the offset before constructing the visual
    /// scrollbar so the thumb sits at the bottom when offset is 0.
    #[serde(default)]
    pub inverted: bool,
    /// Scrollbar width in surface-native units (pixels for GTK, cells
    /// for TUI). When `None`, the rasteriser picks a default (8px GTK,
    /// 1 cell TUI).
    #[serde(default)]
    pub width: Option<u16>,
}

impl TerminalScrollbar {
    /// Scroll offset suitable for `Scrollbar::vertical`. When
    /// `inverted`, flips so offset 0 maps to the track bottom.
    pub fn effective_scroll_offset(&self) -> usize {
        if self.inverted {
            self.total_lines
                .saturating_sub(self.visible_lines)
                .saturating_sub(self.scroll_offset)
        } else {
            self.scroll_offset
        }
    }
}

/// One styled cell in a `Terminal`. Carries the rendered character, RGB
/// foreground/background, attributes, and overlay flags (cursor /
/// selection / find match) that the backend interprets visually.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalCell {
    pub ch: char,
    pub fg: Color,
    pub bg: Color,
    #[serde(default)]
    pub bold: bool,
    #[serde(default)]
    pub italic: bool,
    #[serde(default)]
    pub underline: bool,
    /// Cell is part of the user's mouse selection.
    #[serde(default)]
    pub selected: bool,
    /// Cell holds the VT100 cursor position. Backends typically render
    /// this with inverted colours.
    #[serde(default)]
    pub is_cursor: bool,
    /// Cell is part of a non-active find match (dim highlight).
    #[serde(default)]
    pub is_find_match: bool,
    /// Cell is part of the currently-selected find match (bright highlight).
    #[serde(default)]
    pub is_find_active: bool,
}

// ── D6 Layout API ───────────────────────────────────────────────────────────
//
// Per Decision D6: primitives return fully-resolved `Layout` structs.
// Ninth and last primitive on the new shape. Terminal is a uniform cell
// grid — layout here just resolves viewport dimensions to grid sizes
// and provides click-to-cell mapping. The cell contents are rendered
// directly from `cells[row][col]`; the layout method doesn't walk
// them.

/// Cell dimensions supplied by the backend. TUI passes `(1.0, 1.0)`
/// (char-cell units); native backends pass the font's advance width
/// and line height.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TerminalCellSize {
    pub width: f32,
    pub height: f32,
}

impl TerminalCellSize {
    pub fn new(width: f32, height: f32) -> Self {
        Self { width, height }
    }
}

/// Classification of a hit-test result. For terminals every click maps
/// to a grid cell (row, col) — except clicks outside the viewport or
/// below the last rendered row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalHit {
    Cell { row: u16, col: u16 },
    Empty,
}

/// Fully-resolved terminal layout. Because the cell grid is uniform,
/// there's no `visible_cells` list — backends iterate
/// `terminal.cells` directly at grid positions. The layout provides
/// the viewport → grid conversion and click hit-testing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TerminalLayout {
    pub viewport_width: f32,
    pub viewport_height: f32,
    pub cell_size: TerminalCellSize,
    /// Number of rows that fit in the viewport (may differ from
    /// `terminal.cells.len()`; use `grid_rows.min(cells.len())` when
    /// iterating).
    pub grid_rows: u16,
    /// Number of columns that fit in the viewport.
    pub grid_cols: u16,
}

impl TerminalLayout {
    /// Convert a point in the viewport to a grid cell, or `Empty` if
    /// the point is outside the rendered grid.
    pub fn hit_test(&self, x: f32, y: f32) -> TerminalHit {
        if x < 0.0
            || y < 0.0
            || x >= self.viewport_width
            || y >= self.viewport_height
            || self.cell_size.width <= 0.0
            || self.cell_size.height <= 0.0
        {
            return TerminalHit::Empty;
        }
        let col = (x / self.cell_size.width).floor() as u32;
        let row = (y / self.cell_size.height).floor() as u32;
        if row < self.grid_rows as u32 && col < self.grid_cols as u32 {
            TerminalHit::Cell {
                row: row as u16,
                col: col as u16,
            }
        } else {
            TerminalHit::Empty
        }
    }

    /// Rectangle occupied by cell `(row, col)`, or `None` if the cell
    /// is outside the grid.
    pub fn cell_bounds(&self, row: u16, col: u16) -> Option<Rect> {
        if row >= self.grid_rows || col >= self.grid_cols {
            return None;
        }
        Some(Rect::new(
            col as f32 * self.cell_size.width,
            row as f32 * self.cell_size.height,
            self.cell_size.width,
            self.cell_size.height,
        ))
    }
}

impl Terminal {
    /// Compare `self` against the previously-painted `prev` snapshot and
    /// return the indices of rows whose cell content differs.
    ///
    /// Returns `None` when a full repaint is required rather than a
    /// row-by-row diff: the two grids have different row counts, so
    /// `prev`'s row `i` doesn't necessarily correspond to `self`'s row
    /// `i` (a resize/reflow shifted everything). Returns `Some(rows)`
    /// otherwise — sorted ascending, deduplicated, and possibly empty
    /// (e.g. a cursor-blink tick that produced an identical grid).
    ///
    /// This backs the #417 dirty-row repaint optimisation: backends may
    /// call this to skip re-rasterising cells that haven't changed since
    /// the last frame (see the high-FPS note in this module's doc
    /// comment). `TerminalCell` compares by value, so any change to a
    /// cell's glyph, colour, or overlay flag (`selected` / `is_cursor` /
    /// `is_find_match` / `is_find_active`) — i.e. scroll, selection,
    /// cursor move, and find-highlight updates — registers the owning
    /// row as dirty; there is nothing overlay-specific for a caller to
    /// special-case.
    pub fn dirty_rows(&self, prev: &Terminal) -> Option<Vec<usize>> {
        if self.cells.len() != prev.cells.len() {
            return None;
        }
        Some(
            self.cells
                .iter()
                .zip(prev.cells.iter())
                .enumerate()
                .filter_map(|(i, (a, b))| (a != b).then_some(i))
                .collect(),
        )
    }

    /// Compute viewport → grid conversion. The layout is uniform-cell,
    /// so this is just division; there's no per-cell work.
    ///
    /// # Arguments
    ///
    /// - `viewport_width`, `viewport_height` — pane dimensions.
    /// - `cell_width`, `cell_height` — cell dimensions. TUI passes
    ///   `(1.0, 1.0)`; native backends pass the font's advance width
    ///   and line height.
    pub fn layout(
        &self,
        viewport_width: f32,
        viewport_height: f32,
        cell_width: f32,
        cell_height: f32,
    ) -> TerminalLayout {
        let grid_cols = if cell_width > 0.0 {
            (viewport_width / cell_width).floor().max(0.0) as u16
        } else {
            0
        };
        let grid_rows = if cell_height > 0.0 {
            (viewport_height / cell_height).floor().max(0.0) as u16
        } else {
            0
        };
        TerminalLayout {
            viewport_width,
            viewport_height,
            cell_size: TerminalCellSize::new(cell_width, cell_height),
            grid_rows,
            grid_cols,
        }
    }
}

// ── Split-pane layout ───────────────────────────────────────────────────────

/// Layout geometry for a side-by-side terminal split. The divider
/// occupies one `cell_width` column between the two panes. An optional
/// scrollbar strip is reserved at the right edge.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TerminalSplitLayout {
    /// Left pane region.
    pub left: Rect,
    /// Right pane region.
    pub right: Rect,
    /// X-coordinate of the divider column's left edge.
    pub divider_x: f32,
    /// Width of the divider column (one `cell_width`).
    pub divider_width: f32,
    /// Cell dimensions used for coordinate-to-cell conversion.
    pub cell_size: TerminalCellSize,
    /// Scrollbar strip bounds (right edge of area). `None` when
    /// `scrollbar_width` was 0.
    pub scrollbar: Option<Rect>,
}

impl TerminalSplitLayout {
    /// Compute a two-pane split layout from a containing area, left
    /// pane column count, cell dimensions, and optional scrollbar width.
    ///
    /// TUI callers pass `cell_width = 1.0, cell_height = 1.0`;
    /// GTK callers pass the font's advance width and line height.
    /// Pass `scrollbar_width = 0.0` to omit the scrollbar zone.
    pub fn new(
        area: Rect,
        left_cols: usize,
        cell_width: f32,
        cell_height: f32,
        scrollbar_width: f32,
    ) -> Self {
        let sb_w = if scrollbar_width > 0.0 {
            scrollbar_width.min(area.width)
        } else {
            0.0
        };
        let usable_w = (area.width - sb_w).max(0.0);
        let left_w = (left_cols as f32 * cell_width).min(usable_w);
        let divider_x = area.x + left_w;
        let divider_w = cell_width.min((usable_w - left_w).max(0.0));
        let right_x = divider_x + divider_w;
        let right_w = (usable_w - left_w - divider_w).max(0.0);
        let scrollbar = if sb_w > 0.0 {
            Some(Rect::new(area.x + usable_w, area.y, sb_w, area.height))
        } else {
            None
        };
        Self {
            left: Rect::new(area.x, area.y, left_w, area.height),
            right: Rect::new(right_x, area.y, right_w, area.height),
            divider_x,
            divider_width: divider_w,
            cell_size: TerminalCellSize::new(cell_width, cell_height),
            scrollbar,
        }
    }

    /// Resolve an absolute point to a split zone with pane-relative cell
    /// coordinates. Converts pixel/cell positions to `(col, row)` within
    /// the hit pane using the stored `cell_size`.
    pub fn hit_test(&self, x: f32, y: f32) -> TerminalSplitHit {
        if y < self.left.y || y >= self.left.y + self.left.height {
            return TerminalSplitHit::Outside;
        }
        if let Some(sb) = self.scrollbar {
            if x >= sb.x && x < sb.x + sb.width {
                return TerminalSplitHit::Scrollbar;
            }
        }
        if x >= self.left.x && x < self.left.x + self.left.width {
            let col = ((x - self.left.x) / self.cell_size.width).floor() as u16;
            let row = ((y - self.left.y) / self.cell_size.height).floor() as u16;
            TerminalSplitHit::LeftPane { col, row }
        } else if x >= self.divider_x && x < self.divider_x + self.divider_width {
            TerminalSplitHit::Divider
        } else if x >= self.right.x && x < self.right.x + self.right.width {
            let col = ((x - self.right.x) / self.cell_size.width).floor() as u16;
            let row = ((y - self.right.y) / self.cell_size.height).floor() as u16;
            TerminalSplitHit::RightPane { col, row }
        } else {
            TerminalSplitHit::Outside
        }
    }
}

/// Result of [`TerminalSplitLayout::hit_test`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalSplitHit {
    LeftPane { col: u16, row: u16 },
    RightPane { col: u16, row: u16 },
    Divider,
    Scrollbar,
    Outside,
}

/// Events a `Terminal` emits back to the app. Currently unused by vimcode
/// (the terminal panel handles its own input directly via the engine's
/// `terminal_*` methods), but defined for plugin invariants §10 — a
/// plugin-declared terminal would route events through this enum.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TerminalEvent {
    /// A key was pressed with the terminal focused. Routed to the
    /// underlying PTY by the app.
    KeyPressed { key: String, modifiers: Modifiers },
    /// User started selecting at `(row, col)` (0-based, content area).
    SelectStart { row: u16, col: u16 },
    /// User dragged the selection to a new endpoint.
    SelectExtend { row: u16, col: u16 },
    /// User released the mouse button — selection is finalised.
    SelectEnd,
    /// Mouse wheel scroll: positive = scroll content downward (toward
    /// live), negative = scroll backward into history.
    Scroll { delta: i32 },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_layout_basic() {
        let area = Rect::new(0.0, 0.0, 80.0, 24.0);
        let sl = TerminalSplitLayout::new(area, 40, 1.0, 1.0, 0.0);
        assert_eq!(sl.left, Rect::new(0.0, 0.0, 40.0, 24.0));
        assert_eq!(sl.divider_x, 40.0);
        assert_eq!(sl.divider_width, 1.0);
        assert_eq!(sl.right, Rect::new(41.0, 0.0, 39.0, 24.0));
        assert!(sl.scrollbar.is_none());
    }

    #[test]
    fn split_layout_pixel_units() {
        let area = Rect::new(10.0, 5.0, 800.0, 600.0);
        let sl = TerminalSplitLayout::new(area, 40, 8.0, 16.0, 0.0);
        assert_eq!(sl.left, Rect::new(10.0, 5.0, 320.0, 600.0));
        assert_eq!(sl.divider_x, 330.0);
        assert_eq!(sl.divider_width, 8.0);
        assert_eq!(sl.right, Rect::new(338.0, 5.0, 472.0, 600.0));
    }

    #[test]
    fn split_layout_left_cols_exceed_area() {
        let area = Rect::new(0.0, 0.0, 20.0, 10.0);
        let sl = TerminalSplitLayout::new(area, 30, 1.0, 1.0, 0.0);
        assert_eq!(sl.left.width, 20.0);
        assert_eq!(sl.divider_width, 0.0);
        assert_eq!(sl.right.width, 0.0);
    }

    #[test]
    fn split_layout_zero_left_cols() {
        let area = Rect::new(0.0, 0.0, 80.0, 24.0);
        let sl = TerminalSplitLayout::new(area, 0, 1.0, 1.0, 0.0);
        assert_eq!(sl.left.width, 0.0);
        assert_eq!(sl.divider_x, 0.0);
        assert_eq!(sl.divider_width, 1.0);
        assert_eq!(sl.right.width, 79.0);
    }

    #[test]
    fn split_hit_test_cell_coords() {
        let area = Rect::new(0.0, 0.0, 80.0, 24.0);
        let sl = TerminalSplitLayout::new(area, 40, 1.0, 1.0, 0.0);
        assert_eq!(
            sl.hit_test(10.0, 5.0),
            TerminalSplitHit::LeftPane { col: 10, row: 5 }
        );
        assert_eq!(sl.hit_test(40.0, 5.0), TerminalSplitHit::Divider);
        assert_eq!(
            sl.hit_test(50.0, 5.0),
            TerminalSplitHit::RightPane { col: 9, row: 5 }
        );
        assert_eq!(sl.hit_test(50.0, 30.0), TerminalSplitHit::Outside);
        assert_eq!(sl.hit_test(-1.0, 5.0), TerminalSplitHit::Outside);
    }

    #[test]
    fn split_hit_test_pixel_units() {
        let area = Rect::new(10.0, 5.0, 800.0, 600.0);
        let sl = TerminalSplitLayout::new(area, 40, 8.0, 16.0, 0.0);
        // Click at x=26 (offset 16 from left=10) → col 2
        assert_eq!(
            sl.hit_test(26.0, 21.0),
            TerminalSplitHit::LeftPane { col: 2, row: 1 }
        );
        // Right pane starts at 338.0; click at 354.0 → offset 16 → col 2
        assert_eq!(
            sl.hit_test(354.0, 37.0),
            TerminalSplitHit::RightPane { col: 2, row: 2 }
        );
    }

    #[test]
    fn split_layout_with_scrollbar() {
        let area = Rect::new(0.0, 0.0, 80.0, 24.0);
        let sl = TerminalSplitLayout::new(area, 40, 1.0, 1.0, 1.0);
        // Scrollbar takes 1 cell from the right edge.
        let sb = sl.scrollbar.unwrap();
        assert_eq!(sb.x, 79.0);
        assert_eq!(sb.width, 1.0);
        // Right pane is narrower by 1.
        assert_eq!(sl.right.width, 38.0);
    }

    #[test]
    fn split_hit_test_scrollbar() {
        let area = Rect::new(0.0, 0.0, 80.0, 24.0);
        let sl = TerminalSplitLayout::new(area, 40, 1.0, 1.0, 1.0);
        assert_eq!(sl.hit_test(79.0, 5.0), TerminalSplitHit::Scrollbar);
        // Just inside right pane (not scrollbar).
        assert_eq!(
            sl.hit_test(78.0, 5.0),
            TerminalSplitHit::RightPane { col: 37, row: 5 }
        );
    }

    #[test]
    fn effective_scroll_offset_non_inverted() {
        let sb = TerminalScrollbar {
            total_lines: 500,
            visible_lines: 24,
            scroll_offset: 100,
            inverted: false,
            width: None,
        };
        assert_eq!(sb.effective_scroll_offset(), 100);
    }

    #[test]
    fn effective_scroll_offset_inverted_at_bottom() {
        let sb = TerminalScrollbar {
            total_lines: 500,
            visible_lines: 24,
            scroll_offset: 0,
            inverted: true,
            width: None,
        };
        // offset 0 = live view → thumb at bottom → effective = max
        assert_eq!(sb.effective_scroll_offset(), 476);
    }

    #[test]
    fn effective_scroll_offset_inverted_at_top() {
        let sb = TerminalScrollbar {
            total_lines: 500,
            visible_lines: 24,
            scroll_offset: 476,
            inverted: true,
            width: None,
        };
        // fully scrolled into history → thumb at top → effective = 0
        assert_eq!(sb.effective_scroll_offset(), 0);
    }

    #[test]
    fn effective_scroll_offset_inverted_midpoint() {
        let sb = TerminalScrollbar {
            total_lines: 500,
            visible_lines: 24,
            scroll_offset: 200,
            inverted: true,
            width: None,
        };
        assert_eq!(sb.effective_scroll_offset(), 276);
    }

    #[test]
    fn effective_scroll_offset_inverted_no_overflow() {
        let sb = TerminalScrollbar {
            total_lines: 24,
            visible_lines: 24,
            scroll_offset: 0,
            inverted: true,
            width: None,
        };
        // total == visible → no scrollable range
        assert_eq!(sb.effective_scroll_offset(), 0);
    }

    // ── dirty_rows (#417) ────────────────────────────────────────────

    fn cell(ch: char) -> TerminalCell {
        TerminalCell {
            ch,
            fg: Color::rgb(255, 255, 255),
            bg: Color::rgb(0, 0, 0),
            bold: false,
            italic: false,
            underline: false,
            selected: false,
            is_cursor: false,
            is_find_match: false,
            is_find_active: false,
        }
    }

    fn grid(rows: &[&str]) -> Terminal {
        Terminal {
            id: WidgetId::new("term"),
            cells: rows
                .iter()
                .map(|row| row.chars().map(cell).collect())
                .collect(),
            scrollbar: None,
        }
    }

    #[test]
    fn dirty_rows_identical_grids_are_clean() {
        let prev = grid(&["abc", "def"]);
        let next = grid(&["abc", "def"]);
        assert_eq!(next.dirty_rows(&prev), Some(vec![]));
    }

    #[test]
    fn dirty_rows_flags_only_changed_rows() {
        let prev = grid(&["abc", "def", "ghi"]);
        let next = grid(&["abc", "XYZ", "ghi"]);
        assert_eq!(next.dirty_rows(&prev), Some(vec![1]));
    }

    #[test]
    fn dirty_rows_flags_multiple_changed_rows() {
        let prev = grid(&["abc", "def", "ghi"]);
        let next = grid(&["AAA", "def", "GGG"]);
        assert_eq!(next.dirty_rows(&prev), Some(vec![0, 2]));
    }

    #[test]
    fn dirty_rows_none_when_row_count_differs() {
        let prev = grid(&["abc", "def"]);
        let next = grid(&["abc", "def", "ghi"]);
        assert_eq!(next.dirty_rows(&prev), None);
    }

    #[test]
    fn dirty_rows_row_length_change_counts_as_dirty() {
        // Same row count, but one row grew (ragged rows) — the
        // `Vec<TerminalCell>` equality is length-sensitive, so this
        // must show up as dirty rather than silently comparing equal.
        let prev = grid(&["ab", "cd"]);
        let next = grid(&["abc", "cd"]);
        assert_eq!(next.dirty_rows(&prev), Some(vec![0]));
    }

    #[test]
    fn dirty_rows_overlay_flag_change_is_dirty() {
        // Cursor / selection / find-highlight overlays are plain
        // `TerminalCell` fields, so a flag-only change (same glyph and
        // colour) still registers the row as dirty.
        let prev = grid(&["abc"]);
        let mut next = grid(&["abc"]);
        next.cells[0][1].is_cursor = true;
        assert_eq!(next.dirty_rows(&prev), Some(vec![0]));
    }

    #[test]
    fn terminal_roundtrip_serde() {
        let term = Terminal {
            id: WidgetId::new("terminal-0"),
            cells: vec![
                vec![
                    TerminalCell {
                        ch: '$',
                        fg: Color::rgb(200, 200, 200),
                        bg: Color::rgb(20, 20, 20),
                        bold: true,
                        italic: false,
                        underline: false,
                        selected: false,
                        is_cursor: false,
                        is_find_match: false,
                        is_find_active: false,
                    },
                    TerminalCell {
                        ch: ' ',
                        fg: Color::rgb(200, 200, 200),
                        bg: Color::rgb(20, 20, 20),
                        bold: false,
                        italic: false,
                        underline: false,
                        selected: false,
                        is_cursor: true,
                        is_find_match: false,
                        is_find_active: false,
                    },
                ],
                vec![TerminalCell {
                    ch: 'm',
                    fg: Color::rgb(255, 100, 50),
                    bg: Color::rgb(20, 20, 20),
                    bold: false,
                    italic: false,
                    underline: true,
                    selected: true,
                    is_cursor: false,
                    is_find_match: true,
                    is_find_active: false,
                }],
            ],
            scrollbar: None,
        };
        let json = serde_json::to_string(&term).unwrap();
        let back: Terminal = serde_json::from_str(&json).unwrap();
        assert_eq!(term, back);
    }

    #[test]
    fn terminal_scrollbar_serde_round_trip() {
        let term = Terminal {
            id: WidgetId::new("t"),
            cells: vec![vec![TerminalCell {
                ch: 'x',
                fg: Color::rgb(200, 200, 200),
                bg: Color::rgb(20, 20, 20),
                bold: false,
                italic: false,
                underline: false,
                selected: false,
                is_cursor: false,
                is_find_match: false,
                is_find_active: false,
            }]],
            scrollbar: Some(TerminalScrollbar {
                total_lines: 500,
                visible_lines: 24,
                scroll_offset: 100,
                inverted: false,
                width: None,
            }),
        };
        let json = serde_json::to_string(&term).unwrap();
        let back: Terminal = serde_json::from_str(&json).unwrap();
        assert_eq!(term, back);
        let sb = back.scrollbar.unwrap();
        assert_eq!(sb.total_lines, 500);
        assert_eq!(sb.visible_lines, 24);
        assert_eq!(sb.scroll_offset, 100);
    }

    #[test]
    fn terminal_event_roundtrip_serde() {
        let events = vec![
            TerminalEvent::KeyPressed {
                key: "a".to_string(),
                modifiers: Modifiers::default(),
            },
            TerminalEvent::SelectStart { row: 5, col: 10 },
            TerminalEvent::SelectExtend { row: 6, col: 20 },
            TerminalEvent::SelectEnd,
            TerminalEvent::Scroll { delta: -3 },
        ];
        for event in &events {
            let json = serde_json::to_string(event).unwrap();
            let back: TerminalEvent = serde_json::from_str(&json).unwrap();
            assert_eq!(event, &back);
        }
    }

    // ── D6 Terminal layout API tests ──────────────────────────────────

    fn make_term(rows: usize, cols: usize) -> Terminal {
        let cell = TerminalCell {
            ch: ' ',
            fg: Color::rgb(200, 200, 200),
            bg: Color::rgb(20, 20, 20),
            bold: false,
            italic: false,
            underline: false,
            selected: false,
            is_cursor: false,
            is_find_match: false,
            is_find_active: false,
        };
        Terminal {
            id: WidgetId::new("term"),
            cells: (0..rows).map(|_| vec![cell.clone(); cols]).collect(),
            scrollbar: None,
        }
    }

    #[test]
    fn terminal_layout_tui_cells() {
        let term = make_term(24, 80);
        let layout = term.layout(80.0, 24.0, 1.0, 1.0);
        assert_eq!(layout.grid_rows, 24);
        assert_eq!(layout.grid_cols, 80);
        // Click at (5, 3) → cell (row=3, col=5).
        match layout.hit_test(5.5, 3.5) {
            TerminalHit::Cell { row, col } => {
                assert_eq!(row, 3);
                assert_eq!(col, 5);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn terminal_layout_pixel_cells() {
        // Native: 800x600 viewport, 8 px × 16 px cells = 100 cols × 37 rows.
        let term = make_term(37, 100);
        let layout = term.layout(800.0, 600.0, 8.0, 16.0);
        assert_eq!(layout.grid_cols, 100);
        assert_eq!(layout.grid_rows, 37); // 600/16 = 37.5 → 37
                                          // Click at (160, 48) → col=20, row=3.
        match layout.hit_test(160.0, 48.0) {
            TerminalHit::Cell { row, col } => {
                assert_eq!(col, 20);
                assert_eq!(row, 3);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn terminal_layout_hit_test_outside() {
        let term = make_term(10, 20);
        let layout = term.layout(20.0, 10.0, 1.0, 1.0);
        assert_eq!(layout.hit_test(-1.0, 5.0), TerminalHit::Empty);
        assert_eq!(layout.hit_test(5.0, -1.0), TerminalHit::Empty);
        assert_eq!(layout.hit_test(100.0, 5.0), TerminalHit::Empty);
    }

    #[test]
    fn terminal_layout_cell_bounds() {
        let term = make_term(10, 20);
        let layout = term.layout(20.0, 10.0, 1.0, 1.0);
        let r = layout.cell_bounds(3, 5).unwrap();
        assert_eq!(r.x, 5.0);
        assert_eq!(r.y, 3.0);
        assert_eq!(r.width, 1.0);
        assert_eq!(r.height, 1.0);
        // Out of range → None.
        assert!(layout.cell_bounds(99, 0).is_none());
        assert!(layout.cell_bounds(0, 99).is_none());
    }
}
