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
