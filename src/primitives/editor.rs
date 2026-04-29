//! `Editor` primitive — code-editor viewport with gutter, syntax-
//! highlighted text, cursor, selections, diagnostics, and overlays.
//!
//! Lifted from vimcode's `render::RenderedWindow` / `RenderedLine` in
//! Phase C Stage 1 (#276). The data shape mirrors those types
//! field-for-field (verbatim port). Two rasterisers consume the
//! primitive: `quadraui::tui::draw_editor` and
//! `quadraui::gtk::draw_editor`.
//!
//! ## Module shape
//!
//! - **Stage 1A** (this commit) lifts only the *supporting types*:
//!   `DiagnosticSeverity`, `GitLineStatus`, `DiffLine`, `CursorShape`,
//!   `SelectionKind`, `CursorPos`, `EditorSelection`, `EditorCursor`,
//!   `Style`, `StyledSpan`, `DiagnosticMark`, `SpellMark`. This unblocks
//!   the `quadraui::Theme` editor-field additions without yet
//!   introducing the `Editor` / `EditorLine` data structs (those land
//!   in Stage 1B).
//! - **Stage 1B** adds `Editor` + `EditorLine`.
//! - **Stage 1C / 1D** add the TUI / GTK rasterisers.
//!
//! ## Naming clash with `quadraui::StyledSpan`
//!
//! `quadraui::types::StyledSpan` is owned-text and serde-friendly —
//! used by Lua-pluggable surfaces that need text the plugin can read.
//! This module's [`StyledSpan`] is byte-range based (offsets into the
//! line's `raw_text`) — necessary for the editor paint where
//! tree-sitter / LSP / search highlighting all produce byte ranges.
//! Both shapes coexist; this one is reachable as
//! `quadraui::primitives::editor::StyledSpan` to avoid the clash at
//! the crate root.

use crate::types::Color;
use serde::{Deserialize, Serialize};

// ─── DiagnosticSeverity ─────────────────────────────────────────────────────

/// LSP diagnostic severity, mirrored from `vimcode::core::lsp::DiagnosticSeverity`.
///
/// Values match the LSP wire format (1–4). Vimcode-side adapter at the
/// rasteriser boundary maps `lsp::DiagnosticSeverity` ↔ this enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum DiagnosticSeverity {
    Error = 1,
    Warning = 2,
    Information = 3,
    Hint = 4,
}

// ─── GitLineStatus ──────────────────────────────────────────────────────────

/// Per-line git diff status, mirrored from `vimcode::core::git::GitLineStatus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum GitLineStatus {
    Added,
    Modified,
    /// Lines were deleted at this position (rasteriser draws `▾`).
    Deleted,
}

// ─── DiffLine ───────────────────────────────────────────────────────────────

/// Per-line two-way-diff status, mirrored from
/// `vimcode::core::engine::DiffLine`. `None` on `EditorLine.diff_status`
/// means diff mode is not active for the line's window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DiffLine {
    Same,
    Added,
    Removed,
    /// Filler line inserted for alignment — no buffer content.
    Padding,
}

// ─── CursorShape ────────────────────────────────────────────────────────────

/// Shape of the editor cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CursorShape {
    /// Filled block (Normal / Visual modes).
    Block,
    /// Thin vertical bar (Insert mode).
    Bar,
    /// Underline (pending replace-char `r` command).
    Underline,
}

// ─── SelectionKind ──────────────────────────────────────────────────────────

/// Visual-selection mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SelectionKind {
    Char,
    Line,
    Block,
}

// ─── CursorPos ──────────────────────────────────────────────────────────────

/// Cursor position within the visible window area (view-line + char column).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CursorPos {
    /// Index into `Editor.lines` (0 = topmost visible line).
    pub view_line: usize,
    /// Column (character index within the line).
    pub col: usize,
}

// ─── EditorCursor ───────────────────────────────────────────────────────────

/// Primary cursor: position plus shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EditorCursor {
    pub pos: CursorPos,
    pub shape: CursorShape,
}

// ─── EditorSelection ────────────────────────────────────────────────────────

/// A normalised visual-selection range (start ≤ end) in buffer coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EditorSelection {
    pub kind: SelectionKind,
    /// First selected buffer line.
    pub start_line: usize,
    /// First selected column (Char / Block; ignored for Line).
    pub start_col: usize,
    /// Last selected buffer line (inclusive).
    pub end_line: usize,
    /// Last selected column (Char / Block; ignored for Line).
    pub end_col: usize,
}

// ─── Style ──────────────────────────────────────────────────────────────────

/// Per-span text style used by the editor's byte-range
/// [`StyledSpan`]. Mirrors `vimcode::render::Style`, with
/// `font_scale` narrowed from `f64` → `f32` so derives of
/// `PartialEq` / `Serialize` work without `Eq` blocking us.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Style {
    pub fg: Color,
    pub bg: Option<Color>,
    #[serde(default)]
    pub bold: bool,
    #[serde(default)]
    pub italic: bool,
    /// Font scale factor (1.0 = normal). Used by GTK for markdown
    /// headings; TUI ignores. Cast to `f64` at the Pango call site.
    pub font_scale: f32,
}

// ─── StyledSpan (byte-range) ────────────────────────────────────────────────

/// A styled byte-range within a line's text.
///
/// `start_byte` and `end_byte` are offsets into the
/// `EditorLine.raw_text` of the line that owns the span. The byte-range
/// form is necessary because tree-sitter / LSP / search highlighting
/// all produce byte ranges; converting to owned-text spans (the shape
/// of `quadraui::types::StyledSpan`) would lose alignment with these
/// upstream sources.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StyledSpan {
    pub start_byte: usize,
    pub end_byte: usize,
    pub style: Style,
}

// ─── DiagnosticMark ─────────────────────────────────────────────────────────

/// LSP diagnostic span on one line — drives inline underline / squiggle
/// painting plus tooltip-on-hover. `start_col` / `end_col` are character
/// indices within the line's `raw_text`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DiagnosticMark {
    pub start_col: usize,
    pub end_col: usize,
    pub severity: DiagnosticSeverity,
    pub message: String,
}

// ─── SpellMark ──────────────────────────────────────────────────────────────

/// Misspelled-word span on one line — drives spell-check underline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SpellMark {
    pub start_col: usize,
    pub end_col: usize,
}
