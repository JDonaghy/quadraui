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
//! Stage 1A landed the *supporting types* (`DiagnosticSeverity`,
//! `GitLineStatus`, `DiffLine`, `CursorShape`, `SelectionKind`,
//! `CursorPos`, `EditorSelection`, `EditorCursor`, `Style`,
//! `StyledSpan`, `DiagnosticMark`, `SpellMark`) and grew
//! `quadraui::Theme` with ~29 editor-paint fields. Stage 1B (this
//! commit) adds the `Editor` + `EditorLine` data structs — the
//! primitive's data layer is now complete; rasterisers land in
//! Stages 1C (TUI) and 1D (GTK).
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

use crate::event::Rect;
use crate::types::{Color, WidgetId};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

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

// ─── EditorLine ─────────────────────────────────────────────────────────────

/// One visible line in an [`Editor`] viewport, ready for the rasteriser.
///
/// Field shapes mirror `vimcode::render::RenderedLine` field-for-field
/// (verbatim port). The vimcode-side adapter at the rasteriser
/// boundary builds an `EditorLine` from a `RenderedLine` per frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EditorLine {
    /// Raw UTF-8 text (may include a trailing `\n`).
    pub raw_text: String,
    /// Pre-formatted gutter text (e.g. `"  42"` / `"   3"`).
    /// Empty when line numbers are disabled.
    pub gutter_text: String,
    /// Syntax-highlight + search-match spans (byte-offset based, into
    /// `raw_text`).
    pub spans: Vec<StyledSpan>,
    /// Buffer line index this rendered row corresponds to. Used by
    /// click handlers to map screen row → buffer line.
    pub line_idx: usize,
    /// True when the cursor is on this line (drives gutter highlight).
    #[serde(default)]
    pub is_current_line: bool,
    /// True when this is the header of a closed fold.
    #[serde(default)]
    pub is_fold_header: bool,
    /// Number of lines hidden in the fold (0 unless `is_fold_header`).
    #[serde(default)]
    pub folded_line_count: usize,
    /// Git diff status (`Added` / `Modified` / `Deleted`). `None` when
    /// the buffer is not git-tracked or the line is unchanged.
    #[serde(default)]
    pub git_diff: Option<GitLineStatus>,
    /// Two-way diff status. `None` when diff mode is off.
    #[serde(default)]
    pub diff_status: Option<DiffLine>,
    /// LSP diagnostic spans on this line (may be empty).
    #[serde(default)]
    pub diagnostics: Vec<DiagnosticMark>,
    /// Spell-check error spans on this line (may be empty).
    #[serde(default)]
    pub spell_errors: Vec<SpellMark>,
    /// True when there is a DAP breakpoint set on this line.
    #[serde(default)]
    pub is_breakpoint: bool,
    /// True when the breakpoint has a condition / hit count / log
    /// message (rasteriser draws ◆ instead of ●).
    #[serde(default)]
    pub is_conditional_bp: bool,
    /// True when the DAP adapter is currently stopped at this line.
    #[serde(default)]
    pub is_dap_current: bool,
    /// True when this row is a wrap-continuation (the 2nd+ visual row
    /// of a long buffer line). The line number belongs to the
    /// preceding non-continuation row; `gutter_text` is blank.
    #[serde(default)]
    pub is_wrap_continuation: bool,
    /// Character offset within the buffer line where this visual
    /// segment begins. 0 for non-wrapped lines and the first visual
    /// segment of a wrapped line.
    #[serde(default)]
    pub segment_col_offset: usize,
    /// Inline annotation (virtual text — git blame, plugin output,
    /// etc.) shown after line content in `annotation_fg`.
    #[serde(default)]
    pub annotation: Option<String>,
    /// AI ghost text shown after the cursor on this line (Insert
    /// mode). Rendered in `ghost_text_fg`.
    #[serde(default)]
    pub ghost_suffix: Option<String>,
    /// True for virtual rows showing AI completion continuation lines.
    /// `raw_text` is empty; the full text is in `ghost_suffix` and
    /// rasterisers paint it from the left edge of the content area.
    #[serde(default)]
    pub is_ghost_continuation: bool,
    /// Column positions where indent-guide rules should be drawn.
    /// Empty when the `indent_guides` setting is off.
    #[serde(default)]
    pub indent_guides: Vec<usize>,
    /// Column positions where the colorcolumn background should be
    /// painted.
    #[serde(default)]
    pub colorcolumns: Vec<usize>,
}

// ─── Editor ─────────────────────────────────────────────────────────────────

/// Declarative description of one editor viewport (window / pane).
///
/// Mirrors `vimcode::render::RenderedWindow` field-for-field, with the
/// per-window status-line painted separately by the caller (the status
/// line was lifted to `quadraui::WindowStatusLine` in Session 241 —
/// Stage 1 of #276 does not touch that surface).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Editor {
    pub id: WidgetId,
    /// Pixel-space rectangle (used by GTK / Win-GUI; TUI ignores).
    pub rect: Rect,
    /// Visible lines, one per row.
    pub lines: Vec<EditorLine>,
    /// Primary cursor (position + shape). `None` when scrolled
    /// off-screen.
    #[serde(default)]
    pub cursor: Option<EditorCursor>,
    /// Secondary cursor positions for multi-cursor (Alt-D). Drawn as
    /// dimmed blocks.
    #[serde(default)]
    pub extra_cursors: Vec<CursorPos>,
    /// Active visual selection.
    #[serde(default)]
    pub selection: Option<EditorSelection>,
    /// Extra selections for Ctrl+D word-multi-cursor.
    #[serde(default)]
    pub extra_selections: Vec<EditorSelection>,
    /// Transient post-yank flash region. `None` when no flash active.
    #[serde(default)]
    pub yank_highlight: Option<EditorSelection>,
    /// Index of the first visible buffer line.
    pub scroll_top: usize,
    /// Number of character columns scrolled horizontally.
    #[serde(default)]
    pub scroll_left: usize,
    /// Total lines in the buffer (drives vertical scrollbar geometry).
    pub total_lines: usize,
    /// Max line length in the buffer (character cells, excl. trailing
    /// newline). Drives horizontal scrollbar.
    #[serde(default)]
    pub max_col: usize,
    /// Width of the line-number gutter in *character cells* (0 = no
    /// gutter). GTK multiplies by `char_width` to get pixels.
    #[serde(default)]
    pub gutter_char_width: usize,
    /// Whether this is the focused window.
    pub is_active: bool,
    /// Whether to render with the slightly-different active-window
    /// background — only true when `is_active` AND there are multiple
    /// windows. Drives the rasteriser's choice between
    /// `theme.editor_active_background` and `theme.background`.
    pub show_active_bg: bool,
    /// Whether the buffer has git diff data (controls git column).
    pub has_git_diff: bool,
    /// Whether to show the breakpoint gutter column.
    pub has_breakpoints: bool,
    /// Per-line worst diagnostic severity. Used for gutter icons.
    #[serde(default)]
    pub diagnostic_gutter: HashMap<usize, DiagnosticSeverity>,
    /// Lines with available LSP code actions (lightbulb gutter glyph).
    #[serde(default)]
    pub code_action_lines: HashSet<usize>,
    /// Bracket-pair positions to highlight (cursor bracket + match).
    /// Each `(view_line, col)`. Up to 2 entries.
    #[serde(default)]
    pub bracket_match_positions: Vec<(usize, usize)>,
    /// Indent-guide column highlighted as "active" (cursor's scope).
    #[serde(default)]
    pub active_indent_col: Option<usize>,
    /// Tab stop width for expanding `\t` to spaces (TUI rendering).
    pub tabstop: usize,
    /// Whether to draw cursorline highlight.
    pub cursorline: bool,
    /// Glyph the rasteriser draws in the gutter for lines with an
    /// available LSP code action. The host computes this from its
    /// icon registry per frame so nerd-font / fallback toggles
    /// propagate without the rasteriser depending on a host icon
    /// module. Apps without code-action icons can leave this at
    /// `'\0'` and the rasteriser skips painting.
    #[serde(default = "default_lightbulb_glyph")]
    pub lightbulb_glyph: char,
}

fn default_lightbulb_glyph() -> char {
    '!'
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editor_line_default_constructible_via_serde() {
        // A minimal-ish `EditorLine` round-trips through serde with
        // its #[serde(default)] fields populated by their natural
        // defaults — proves the field set is plugin-friendly.
        let json = r#"{ "raw_text": "fn main() {}",
                       "gutter_text": "  1",
                       "spans": [],
                       "line_idx": 0 }"#;
        let line: EditorLine = serde_json::from_str(json).expect("EditorLine deserialise");
        assert_eq!(line.raw_text, "fn main() {}");
        assert_eq!(line.line_idx, 0);
        assert!(!line.is_current_line);
        assert!(line.diagnostics.is_empty());
        assert!(line.indent_guides.is_empty());
    }

    #[test]
    fn editor_round_trips_through_serde() {
        let editor = Editor {
            id: "editor:0".into(),
            rect: Rect::new(0.0, 0.0, 800.0, 600.0),
            lines: vec![EditorLine {
                raw_text: "let x = 1;".into(),
                gutter_text: "  1".into(),
                spans: vec![StyledSpan {
                    start_byte: 0,
                    end_byte: 3,
                    style: Style {
                        fg: Color::rgb(200, 200, 200),
                        bg: None,
                        bold: false,
                        italic: false,
                        font_scale: 1.0,
                    },
                }],
                line_idx: 0,
                is_current_line: true,
                is_fold_header: false,
                folded_line_count: 0,
                git_diff: Some(GitLineStatus::Added),
                diff_status: None,
                diagnostics: vec![],
                spell_errors: vec![],
                is_breakpoint: false,
                is_conditional_bp: false,
                is_dap_current: false,
                is_wrap_continuation: false,
                segment_col_offset: 0,
                annotation: None,
                ghost_suffix: None,
                is_ghost_continuation: false,
                indent_guides: vec![],
                colorcolumns: vec![],
            }],
            cursor: Some(EditorCursor {
                pos: CursorPos {
                    view_line: 0,
                    col: 4,
                },
                shape: CursorShape::Block,
            }),
            extra_cursors: vec![],
            selection: None,
            extra_selections: vec![],
            yank_highlight: None,
            scroll_top: 0,
            scroll_left: 0,
            total_lines: 1,
            max_col: 10,
            gutter_char_width: 3,
            is_active: true,
            show_active_bg: false,
            has_git_diff: true,
            has_breakpoints: false,
            diagnostic_gutter: HashMap::new(),
            code_action_lines: HashSet::new(),
            bracket_match_positions: vec![],
            active_indent_col: None,
            tabstop: 4,
            cursorline: true,
            lightbulb_glyph: '!',
        };
        let json = serde_json::to_string(&editor).expect("Editor serialise");
        let back: Editor = serde_json::from_str(&json).expect("Editor deserialise");
        assert_eq!(back.lines.len(), 1);
        assert_eq!(back.lines[0].raw_text, "let x = 1;");
        assert!(matches!(back.lines[0].git_diff, Some(GitLineStatus::Added)));
        assert!(matches!(back.cursor.unwrap().shape, CursorShape::Block));
    }

    #[test]
    fn diagnostic_severity_lsp_order_preserved() {
        // Order Error < Warning < Information < Hint matches LSP
        // numeric ordering — important because vimcode picks the
        // worst severity per line by `min`.
        assert!(DiagnosticSeverity::Error < DiagnosticSeverity::Warning);
        assert!(DiagnosticSeverity::Warning < DiagnosticSeverity::Information);
        assert!(DiagnosticSeverity::Information < DiagnosticSeverity::Hint);
    }
}
