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
