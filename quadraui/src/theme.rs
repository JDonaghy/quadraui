//! Backend-agnostic [`Theme`] struct consumed by the per-backend rasterisers
//! in [`crate::tui`] and [`crate::gtk`].
//!
//! `Theme` is intentionally small. Apps with rich theme systems (vimcode's
//! `render::Theme` carries dozens of LSP / git / markdown colours; kubeui
//! has its own palette) build a `quadraui::Theme` at the call site by
//! picking the relevant fields out of their app-specific theme. Adding a
//! field here means every primitive rasteriser can read it; field count
//! grows as more primitives migrate from vimcode-private rasterisers into
//! `quadraui::tui` / `quadraui::gtk`.
//!
//! This is the **first** field set — driven by the StatusBar pilot
//! (#223). Future migrations (TabBar, ListView, TreeView, …) will
//! extend the struct.

use crate::types::Color;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Minimal backend-agnostic colour palette consumed by the public
/// `quadraui::tui` / `quadraui::gtk` rasterisers.
///
/// Apps that want the rasterisers to draw with their own colours
/// build a `Theme` at the call site (vimcode does this from
/// `render::Theme`; kubeui does it from its own palette). All fields
/// are required so every rasteriser has a reasonable fallback for
/// regions a primitive doesn't fully cover (e.g. the `StatusBar`
/// background fill when no segments are present).
///
/// **Field set is incremental.** Each migrated primitive adds the
/// fields it needs. The `Default` impl keeps a coherent dark palette
/// so apps can spread `..Default::default()` after specifying the
/// fields they care about.
///
/// # Adding a field here is a BREAKING change — prefer a method
///
/// The paragraph above describes how in-tree callers *should* build a
/// `Theme`; it is not what the downstream consumers actually do.
/// `Theme` is a plain struct (not `#[non_exhaustive]`, and it must stay
/// that way — `#[non_exhaustive]` would forbid struct-literal syntax
/// downstream outright, breaking both consumers harder than any field
/// ever could), and `coord-tui` builds three of its four palettes —
/// `light_palette`, `high_contrast_palette`, `solarized_palette` in
/// `tui/src/settings.rs` — with **exhaustive literals and no
/// `..Default::default()` spread**. A new field therefore lands as
/// `error[E0063]: missing field` in their next build, and unlike a
/// rename there is no `#[deprecated]` shim that can soften a field
/// *addition* (CLAUDE.md's *Downstream consumers* section,
/// `docs/PRIMITIVE_RULES.md` rule 8). #620 learned this the expensive
/// way: `tab_active_border_top` shipped as a field, reddened the
/// `downstream consumers (compile truth)` gate, and came back as
/// [`Theme::tab_active_border_top`] — a method.
///
/// So, before adding a `pub` field here:
///
/// 1. Can the value be **derived from an existing field**? Then add an
///    inherent method (`fn tab_active_border_top(&self) -> Color`),
///    which is purely additive and still themeable, since the field it
///    derives from is one the app already sets.
/// 2. If it genuinely needs independent storage, it is a real breaking
///    change: one field per PR, with a `## Downstream impact` section
///    naming each consumer literal that must gain the field, and the
///    consumer migration issues opened in the same session.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Theme {
    // ── StatusBar pilot (#223 slice 1) ─────────────────────────────────
    /// Default surface background. Used as a fallback fill when the
    /// primitive has no opinion (e.g. an empty `StatusBar`).
    pub background: Color,
    /// Default surface foreground. Available for primitives that need
    /// a generic text colour; consumed by `TabBar` for the dirty-tab
    /// `●` glyph.
    pub foreground: Color,

    // ── TabBar pilot (#223 slice 2) ────────────────────────────────────
    /// Tab bar row background — also reused by inactive tab rows.
    pub tab_bar_bg: Color,
    /// Active tab background.
    pub tab_active_bg: Color,
    /// Active tab text colour.
    pub tab_active_fg: Color,
    /// Inactive tab text colour. Also used for right-segment text when
    /// the segment isn't `is_active`.
    pub tab_inactive_fg: Color,
    /// Active *preview* tab text colour (italicised in TUI).
    pub tab_preview_active_fg: Color,
    /// Inactive *preview* tab text colour.
    pub tab_preview_inactive_fg: Color,
    /// Window / panel separator colour. Used by `TabBar` for the
    /// close-button `×` on inactive tabs.
    pub separator: Color,

    // ── ListView pilot (#223 slice 3) ──────────────────────────────────
    /// Background fill for surfaces drawn with a border (e.g. a
    /// bordered `ListView` modal). Distinct from `background` because
    /// modal-style panels typically tint slightly off the editor bg.
    pub surface_bg: Color,
    /// Default text colour on `surface_bg`.
    pub surface_fg: Color,
    /// Background of the selected row in a `ListView`.
    pub selected_bg: Color,
    /// Dimmed selection background for unfocused trees / lists. Indicates
    /// the selected row without implying keyboard focus (e.g. explorer
    /// tree highlighting the file matching the active editor tab).
    pub inactive_selected_bg: Color,
    /// Border-glyph colour for bordered surfaces.
    pub border_fg: Color,
    /// Title text colour drawn over a top border (bordered `ListView`).
    pub title_fg: Color,
    /// Background of a flat (non-bordered) header strip.
    pub header_bg: Color,
    /// Foreground of a flat (non-bordered) header strip.
    pub header_fg: Color,
    /// Dim / muted foreground for less-important text (line numbers,
    /// detail columns, `Decoration::Muted`).
    pub muted_fg: Color,
    /// Error / `Decoration::Error` foreground.
    pub error_fg: Color,
    /// Warning / `Decoration::Warning` foreground.
    pub warning_fg: Color,

    // ── Palette pilot (#223 slice 5) ───────────────────────────────────
    /// Query-input text colour and cursor block fg in a `Palette`
    /// modal. Distinct from `surface_fg` — the query line is
    /// emphasised, item rows are not.
    pub query_fg: Color,
    /// Per-character highlight colour for fuzzy-match positions in
    /// `Palette` items.
    pub match_fg: Color,

    // ── Form pilot (#223 slice 6) ──────────────────────────────────────
    /// Accent foreground used by `Form` for active-state visual cues
    /// (toggle "[x]" when on, slider filled track, button frame when
    /// focused). Typically the editor cursor / caret colour.
    pub accent_fg: Color,

    // ── Tooltip pilot (#223 slice 7) ───────────────────────────────────
    /// Background fill for `Tooltip` popups (LSP hover, signature help,
    /// diff peek). Distinct from `surface_bg` so apps can tint hover
    /// popups differently from modal lists.
    pub hover_bg: Color,
    /// Default text colour for `Tooltip` popups.
    pub hover_fg: Color,
    /// Border-glyph / stroke colour for `Tooltip` popups.
    pub hover_border: Color,

    // ── Dialog pilot (#223 slice 8) ────────────────────────────────────
    /// Background of the input field in a `Dialog` (e.g. the rename
    /// prompt's text entry). Distinct from `surface_bg` so the input
    /// reads as a separate sub-region.
    pub input_bg: Color,

    // ── ActivityBar / Terminal lift (B5c.5) ────────────────────────────
    /// Foreground for inactive entries in dim-out-when-not-focused
    /// chrome (activity-bar inactive icons, similar surfaces). Distinct
    /// from `muted_fg` because activity-bar icons typically use the
    /// status-bar's inactive colour, not the line-number / detail tone.
    pub inactive_fg: Color,
    /// Selection-region background. Used by the `Terminal` rasteriser
    /// to highlight selected cells. Distinct from `selected_bg` (which
    /// is the listview row highlight).
    pub selection_bg: Color,

    // ── RichTextPopup / Completions lift (#266) ────────────────────────
    /// Link / focus accent foreground. Used by `RichTextPopup` for the
    /// focused-popup border colour and the scrollbar thumb when the
    /// popup has keyboard focus. Typically the editor's markdown link
    /// colour.
    pub link_fg: Color,
    /// Background fill for the `Completions` popup (and similar typeahead
    /// menus). Distinct from `surface_bg` so apps can tint completion
    /// menus differently from modal lists.
    pub completion_bg: Color,
    /// Default text colour for completion items.
    pub completion_fg: Color,
    /// Border-glyph / stroke colour for completion popup chrome.
    pub completion_border: Color,
    /// Background of the selected row in a completion popup.
    pub completion_selected_bg: Color,

    // ── FindReplace lift (#271) ────────────────────────────────────────
    /// Accent background used for "this toggle button is on" states
    /// (e.g. case-sensitive / regex / preserve-case toggles in the
    /// find-replace overlay). Typically the editor's tab-active-accent
    /// colour. Distinct from `selected_bg` (list highlight) and
    /// `accent_fg` (text-cursor accent).
    pub accent_bg: Color,

    // ── Scrollbar lift (#277) ──────────────────────────────────────────
    /// Track colour for `Scrollbar`. The TUI rasteriser draws this on
    /// the entire track (typically a dim shade visible against the
    /// editor background); the GTK rasteriser uses it with a low alpha
    /// for the overlay-style track on the right/bottom of the editor.
    pub scrollbar_track: Color,
    /// Thumb colour for `Scrollbar`. Both rasterisers use this for the
    /// thumb glyph / rectangle, with backend-specific brightness
    /// modulation when the scrollbar is hovered or being dragged.
    pub scrollbar_thumb: Color,

    // ── Editor lift (#276 Phase C Stage 1) ─────────────────────────────
    // Backgrounds.
    /// Slightly tinted background of the focused editor window when
    /// multiple windows are visible. Distinct from `background` so the
    /// active pane is visually distinguishable. Vimcode's
    /// `RenderedWindow.show_active_bg` selects between this and
    /// `background`.
    pub editor_active_background: Color,
    /// Background tint of the cursor's current line when the
    /// `cursorline` setting is on. Lower priority than diff backgrounds
    /// and the DAP-stopped highlight.
    pub cursorline_bg: Color,
    /// Background highlight of the line where the DAP adapter is
    /// currently stopped. Highest priority — overrides cursorline +
    /// diff backgrounds.
    pub dap_stopped_bg: Color,
    /// Background tint applied at colorcolumn positions
    /// (`settings.colorcolumn`).
    pub colorcolumn_bg: Color,

    // Diff backgrounds (two-way `:diffthis` mode).
    pub diff_added_bg: Color,
    pub diff_removed_bg: Color,
    /// Background of synthetic alignment-padding rows (no buffer
    /// content) inserted to keep diff panes line-aligned.
    pub diff_padding_bg: Color,

    // Gutter line numbers.
    /// Foreground of inactive line numbers in the gutter.
    pub line_number_fg: Color,
    /// Foreground of the line number on the cursor's current line.
    pub line_number_active_fg: Color,

    // Diagnostic foregrounds (drive squiggle / underline + gutter icon).
    pub diagnostic_error: Color,
    pub diagnostic_warning: Color,
    pub diagnostic_info: Color,
    pub diagnostic_hint: Color,

    // Git diff gutter markers.
    pub git_added: Color,
    pub git_modified: Color,
    pub git_deleted: Color,

    // Code-action lightbulb + spell-checker.
    /// Foreground of the code-action lightbulb gutter glyph.
    pub lightbulb: Color,
    /// Foreground of the spell-checker underline.
    pub spell_error: Color,

    // Cursor / selection / yank flash.
    /// Editor cursor base colour. TUI inverts fg/bg at the cell using
    /// this; GTK paints a rect with `cursor_normal_alpha`.
    pub cursor: Color,
    /// Alpha (0.0..1.0) applied to the GTK cursor rectangle in Normal
    /// mode. TUI ignores (no alpha on cells).
    pub cursor_normal_alpha: f32,
    /// Selection background colour. Both backends mix this with the
    /// underlying line bg (TUI via cell bg overwrite, GTK via alpha
    /// rect).
    pub selection: Color,
    /// Alpha (0.0..1.0) applied to the GTK selection rectangles.
    pub selection_alpha: f32,
    /// Background flash painted briefly after a yank.
    pub yank_highlight_bg: Color,
    /// Alpha (0.0..1.0) applied to the GTK yank-flash rectangles.
    pub yank_highlight_alpha: f32,

    // Bracket-match + indent-guides.
    /// Background highlight on the cursor's bracket and its match.
    pub bracket_match_bg: Color,
    /// Foreground of inactive indent-guide rules.
    pub indent_guide_fg: Color,
    /// Foreground of the active indent-guide column (cursor's scope).
    pub indent_guide_active_fg: Color,

    // Inline annotations + AI ghost text.
    /// Foreground of inline annotations (Lua-plugin virtual text, git
    /// blame). Muted by convention.
    pub annotation_fg: Color,
    /// Foreground of AI-completion ghost text. Muted by convention.
    pub ghost_text_fg: Color,

    // Command line (`:`, `/`, `?` prompt + message output).
    /// Background of the command line bar.
    pub command_line_bg: Color,
    /// Text colour of the command line bar.
    pub command_line_fg: Color,

    // ── Board / kanban (#362) ──────────────────────────────────────────
    /// Background of the selected card in a `Board`.
    pub board_selected_card_bg: Color,
    /// Background of the focused column header in a `Board`.
    pub board_col_header_bg: Color,
    /// `BadgeStatus::Running` colour (typically accent yellow / orange).
    pub badge_running: Color,
    /// `BadgeStatus::Passed` colour (green).
    pub badge_passed: Color,
    /// `BadgeStatus::Warning` colour (warning orange).
    pub badge_warning: Color,
    /// `BadgeStatus::Blocked` colour (red).
    pub badge_blocked: Color,
    /// Background of the `BoardCard::hint` callout strip at the bottom of a card.
    pub card_hint_bg: Color,
    /// Text colour of the `BoardCard::hint` callout.
    pub card_hint_fg: Color,
}

/// Strip `//` and `/* */` comments from JSON-with-comments (JSONC), as
/// used by VS Code theme files. Preserves newlines inside block comments
/// so that any later parse-error line/column reporting on the stripped
/// string still lines up with the original file.
///
/// Lifted from vimcode's `render::strip_json_comments` (#775) — a pure
/// text transform with nothing editor-specific about it.
fn strip_json_comments(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let bytes = input.as_bytes();
    let len = bytes.len();
    let mut i = 0;
    while i < len {
        if bytes[i] == b'"' {
            // String literal — copy verbatim until the closing quote, so a
            // `//` or `/*` inside a JSON string value is never mistaken
            // for a comment.
            out.push('"');
            i += 1;
            while i < len {
                if bytes[i] == b'\\' && i + 1 < len {
                    out.push(bytes[i] as char);
                    out.push(bytes[i + 1] as char);
                    i += 2;
                } else if bytes[i] == b'"' {
                    out.push('"');
                    i += 1;
                    break;
                } else {
                    out.push(bytes[i] as char);
                    i += 1;
                }
            }
        } else if i + 1 < len && bytes[i] == b'/' && bytes[i + 1] == b'/' {
            // Line comment — skip until newline.
            i += 2;
            while i < len && bytes[i] != b'\n' {
                i += 1;
            }
        } else if i + 1 < len && bytes[i] == b'/' && bytes[i + 1] == b'*' {
            // Block comment — skip until `*/`, preserving newlines.
            i += 2;
            while i + 1 < len && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                if bytes[i] == b'\n' {
                    out.push('\n');
                }
                i += 1;
            }
            i += 2; // skip `*/`
        } else {
            out.push(bytes[i] as char);
            i += 1;
        }
    }
    out
}

impl Theme {
    /// Parse a VS Code theme JSON (or JSONC — VS Code theme files commonly
    /// carry `//` / `/* */` comments) file and map its `colors` object
    /// onto a [`Theme`]. Keys the file doesn't set leave the corresponding
    /// [`Theme::default`] field untouched, so even a minimal theme (a
    /// handful of `colors` keys) produces a fully-populated, usable
    /// palette rather than a partially-black one. Returns `None` if the
    /// file cannot be read or does not parse as JSON.
    ///
    /// Lifted from vimcode's `render::Theme::from_vscode_json` (#775).
    /// vimcode's own theme carries dozens of syntax/LSP/semantic-token
    /// fields (`keyword`, `string_lit`, `semantic_namespace`, …) that
    /// this crate's [`Theme`] has no equivalent for — those stay
    /// app-side, built by app-specific code from the same theme file's
    /// `tokenColors` array, which this function does not touch.
    ///
    /// Colours that VS Code themes commonly express as a translucent
    /// `#rrggbbaa` overlay (e.g. `editor.lineHighlightBackground`, diff
    /// backgrounds, bracket-match highlight) are alpha-composited over
    /// `editor.background` via [`Color::try_from_hex_over`] rather than
    /// taken at face value, so a rasteriser that draws an opaque rect
    /// still gets the intended blended colour.
    pub fn from_vscode_json(path: &Path) -> Option<Self> {
        let data = std::fs::read_to_string(path).ok()?;
        let data = strip_json_comments(&data);
        let val: serde_json::Value = serde_json::from_str(&data).ok()?;
        let colors = val.get("colors");

        let mut theme = Self::default();

        // Plain (non-composited) colour lookup.
        let color =
            |key: &str| -> Option<Color> { colors?.get(key)?.as_str().and_then(Color::from_hex) };
        // Alpha-composited lookup, blended over `bg`.
        let color_over = |key: &str, bg: Color| -> Option<Color> {
            colors?
                .get(key)?
                .as_str()
                .and_then(|s| Color::try_from_hex_over(s, bg))
        };

        // ── Editor core ──────────────────────────────────────────────
        if let Some(c) = color("editor.background") {
            theme.background = c;
            theme.editor_active_background = c.lighten(0.02);
            theme.scrollbar_track = c;
            theme.command_line_bg = c;
        }
        if let Some(c) = color("editor.foreground") {
            theme.foreground = c;
            theme.command_line_fg = c;
        }

        // ── Selection / cursor ───────────────────────────────────────
        if let Some(c) = color("editor.selectionBackground") {
            theme.selection_bg = c;
            theme.selection = c;
        }
        if let Some(c) = color("editorCursor.foreground") {
            theme.cursor = c;
        }
        if let Some(c) = color_over("editor.lineHighlightBackground", theme.background) {
            theme.cursorline_bg = c;
        }
        if let Some(c) = color_over("editor.stackFrameHighlightBackground", theme.background) {
            theme.dap_stopped_bg = c;
        }

        // ── Tab bar ──────────────────────────────────────────────────
        if let Some(c) = color("editorGroupHeader.tabsBackground") {
            theme.tab_bar_bg = c;
        }
        if let Some(c) = color("tab.activeBackground") {
            theme.tab_active_bg = c;
        }
        if let Some(c) = color("tab.activeForeground") {
            theme.tab_active_fg = c;
        }
        if let Some(c) = color("tab.inactiveForeground") {
            theme.tab_inactive_fg = c;
            theme.tab_preview_active_fg = c.lighten(0.2);
            theme.tab_preview_inactive_fg = c.darken(0.3);
        }
        if let Some(c) = color("editorGroup.border") {
            theme.separator = c;
        }

        // ── Surfaces / lists (Palette, ListView) ────────────────────
        if let Some(c) = color("editorWidget.background") {
            theme.surface_bg = c;
        }
        if let Some(c) = color("editorWidget.foreground").or_else(|| color("editor.foreground")) {
            theme.surface_fg = c;
        }
        if let Some(c) = color("list.activeSelectionBackground") {
            theme.selected_bg = c;
        }
        if let Some(c) = color("list.inactiveSelectionBackground") {
            theme.inactive_selected_bg = c;
        }
        if let Some(c) = color("editorWidget.border") {
            theme.border_fg = c;
        }
        if let Some(c) = color("titleBar.activeForeground") {
            theme.title_fg = c;
        }
        if let Some(c) = color("sideBarSectionHeader.background") {
            theme.header_bg = c;
        }
        if let Some(c) = color("sideBarSectionHeader.foreground") {
            theme.header_fg = c;
        }
        if let Some(c) = color("descriptionForeground") {
            theme.muted_fg = c;
        }
        if let Some(c) = color("editorError.foreground") {
            theme.error_fg = c;
            theme.diagnostic_error = c;
        }
        if let Some(c) = color("editorWarning.foreground") {
            theme.warning_fg = c;
            theme.diagnostic_warning = c;
        }
        if let Some(c) = color("editorInfo.foreground") {
            theme.diagnostic_info = c;
        }
        if let Some(c) = color("editorHint.foreground") {
            theme.diagnostic_hint = c;
        }

        // ── Palette ──────────────────────────────────────────────────
        if let Some(c) = color("input.foreground") {
            theme.query_fg = c;
        }
        if let Some(c) = color("list.highlightForeground")
            .or_else(|| color("editorSuggestWidget.highlightForeground"))
        {
            theme.match_fg = c;
        }

        // ── Form / accent ────────────────────────────────────────────
        if let Some(c) = color("focusBorder") {
            theme.accent_fg = c;
        }
        if let Some(c) = color("button.background") {
            theme.accent_bg = c;
        }

        // ── Tooltip ──────────────────────────────────────────────────
        if let Some(c) =
            color("editorHoverWidget.background").or_else(|| color("editorWidget.background"))
        {
            theme.hover_bg = c;
        }
        if let Some(c) =
            color("editorHoverWidget.foreground").or_else(|| color("editor.foreground"))
        {
            theme.hover_fg = c;
        }
        if let Some(c) = color("editorHoverWidget.border").or_else(|| color("editorWidget.border"))
        {
            theme.hover_border = c;
        }

        // ── Dialog ───────────────────────────────────────────────────
        if let Some(c) = color("input.background") {
            theme.input_bg = c;
        }

        // ── ActivityBar ──────────────────────────────────────────────
        if let Some(c) =
            color("activityBar.inactiveForeground").or_else(|| color("descriptionForeground"))
        {
            theme.inactive_fg = c;
        }

        // ── Completions / links ──────────────────────────────────────
        if let Some(c) = color("textLink.foreground") {
            theme.link_fg = c;
        }
        if let Some(c) =
            color("editorSuggestWidget.background").or_else(|| color("editorWidget.background"))
        {
            theme.completion_bg = c;
        }
        if let Some(c) =
            color("editorSuggestWidget.foreground").or_else(|| color("editor.foreground"))
        {
            theme.completion_fg = c;
        }
        if let Some(c) =
            color("editorSuggestWidget.border").or_else(|| color("editorWidget.border"))
        {
            theme.completion_border = c;
        }
        if let Some(c) = color("editorSuggestWidget.selectedBackground") {
            theme.completion_selected_bg = c;
        }

        // ── Scrollbar ────────────────────────────────────────────────
        // VS Code doesn't have a separate track colour — the slider
        // itself is drawn translucent over the editor background, so
        // composite it the same way and reuse `editor.background` (set
        // above) as the track.
        if let Some(c) = color_over("scrollbarSlider.background", theme.background) {
            theme.scrollbar_thumb = c;
        }

        // ── Gutter line numbers ──────────────────────────────────────
        if let Some(c) = color("editorLineNumber.foreground") {
            theme.line_number_fg = c;
        }
        if let Some(c) = color("editorLineNumber.activeForeground") {
            theme.line_number_active_fg = c;
        }

        // ── Git gutter ───────────────────────────────────────────────
        if let Some(c) = color("gitDecoration.addedResourceForeground")
            .or_else(|| color("editorGutter.addedBackground"))
        {
            theme.git_added = c;
        }
        if let Some(c) = color("gitDecoration.modifiedResourceForeground")
            .or_else(|| color("editorGutter.modifiedBackground"))
        {
            theme.git_modified = c;
        }
        if let Some(c) = color("gitDecoration.deletedResourceForeground")
            .or_else(|| color("editorGutter.deletedBackground"))
        {
            theme.git_deleted = c;
        }

        // ── Code-action lightbulb ────────────────────────────────────
        if let Some(c) = color("editorLightBulb.foreground") {
            theme.lightbulb = c;
        }

        // ── Diff ─────────────────────────────────────────────────────
        if let Some(c) = color_over("diffEditor.insertedTextBackground", theme.background) {
            theme.diff_added_bg = c;
        }
        if let Some(c) = color_over("diffEditor.removedTextBackground", theme.background) {
            theme.diff_removed_bg = c;
        }

        // ── Bracket-match + indent guides ────────────────────────────
        if let Some(c) = color_over("editorBracketMatch.background", theme.background) {
            theme.bracket_match_bg = c;
        }
        if let Some(c) = color("editorIndentGuide.background") {
            theme.indent_guide_fg = c;
        }
        if let Some(c) = color("editorIndentGuide.activeBackground") {
            theme.indent_guide_active_fg = c;
        }

        // ── Annotations / ghost text ─────────────────────────────────
        if let Some(c) = color("editorCodeLens.foreground") {
            theme.annotation_fg = c;
        }
        if let Some(c) = color("editorGhostText.foreground") {
            theme.ghost_text_fg = c;
        }

        Some(theme)
    }

    /// Colour of the 1 px accent line along the top edge of the active
    /// tab — VS Code Dark Modern's `tab.activeBorderTop` (#620).
    ///
    /// Never applied automatically. A caller opts in per bar by passing
    /// `active_accent: Some(theme.tab_active_border_top())`, mirroring VS
    /// Code's own split between `tab.activeBorderTop` (focused group) and
    /// `tab.unfocusedActiveBorderTop` (typically unset). Leaving
    /// [`crate::TabBar::active_accent`] as `None` paints no accent at all
    /// on every backend (TUI, GTK, macOS) — e.g. for a bottom-panel tab
    /// strip, a terminal toolbar, or an unfocused split's tab bar.
    ///
    /// **This is a method, not a `Theme` field, on purpose** — see the
    /// "adding a field here is a breaking change" note on [`Theme`]
    /// itself. Deriving it from [`Self::accent_fg`] loses nothing: a
    /// palette that already sets its accent colour gets a matching tab
    /// accent for free, and the built-in dark default is unchanged
    /// (`accent_fg` defaults to the same `rgb(140, 200, 240)` this
    /// originally shipped as a field).
    pub fn tab_active_border_top(&self) -> Color {
        self.accent_fg
    }

    // ── Terminal find-highlight lift (#500) ─────────────────────────────
    //
    // `tui/terminal.rs`, `gtk/terminal.rs`, and `macos/terminal.rs` each
    // hardcoded the same two RGB literals for the find-match overlay
    // (`rgb(255, 165, 0)` active, `rgb(100, 80, 20)` dim) — see #500. The
    // shared ladder in `crate::terminal_style::resolve_cell_style` reads
    // them from here instead.
    //
    // **These are methods, not `Theme` fields, on purpose** — same
    // reasoning as [`Self::tab_active_border_top`] above. Unlike that
    // method, these don't derive from an existing field (there's no
    // "find highlight" colour anywhere else in `Theme` to reuse), but a
    // method with a hardcoded literal is still purely additive: it adds
    // no new field for `coord-tui`'s exhaustive palette literals
    // (`tui/src/settings.rs`) to miss, so it costs nothing downstream.
    // Promote to a real field only if a consumer asks to theme these
    // independently — that's rule 8's "if it must break" path, one field
    // per PR with a `## Downstream impact` section.

    /// Background painted over the currently-active find match —
    /// terminal find-highlight convention (bright orange).
    pub fn find_active_bg(&self) -> Color {
        Color::rgb(255, 165, 0)
    }

    /// Foreground painted over [`Self::find_active_bg`] (black, for
    /// contrast against the bright orange).
    pub fn find_active_fg(&self) -> Color {
        Color::rgb(0, 0, 0)
    }

    /// Background painted over non-active find matches (dim highlight).
    pub fn find_match_bg(&self) -> Color {
        Color::rgb(100, 80, 20)
    }
}

impl Default for Theme {
    /// Neutral dark palette so the rasterisers produce something visible
    /// when an app forgets to populate the theme. Apps almost always
    /// override this.
    fn default() -> Self {
        let bg = Color::rgb(20, 22, 30);
        let fg = Color::rgb(220, 220, 220);
        let muted = Color::rgb(120, 122, 135);
        Self {
            background: bg,
            foreground: fg,
            tab_bar_bg: bg,
            tab_active_bg: Color::rgb(40, 44, 56),
            tab_active_fg: fg,
            tab_inactive_fg: Color::rgb(140, 140, 150),
            tab_preview_active_fg: Color::rgb(180, 180, 200),
            tab_preview_inactive_fg: Color::rgb(110, 110, 125),
            separator: Color::rgb(60, 62, 72),
            surface_bg: Color::rgb(28, 32, 44),
            surface_fg: fg,
            selected_bg: Color::rgb(50, 60, 90),
            inactive_selected_bg: Color::rgb(35, 40, 58),
            border_fg: Color::rgb(120, 160, 200),
            title_fg: Color::rgb(180, 200, 230),
            header_bg: Color::rgb(40, 44, 56),
            header_fg: fg,
            muted_fg: muted,
            error_fg: Color::rgb(220, 80, 80),
            warning_fg: Color::rgb(220, 180, 80),
            query_fg: fg,
            match_fg: Color::rgb(255, 200, 80),
            accent_fg: Color::rgb(140, 200, 240),
            hover_bg: Color::rgb(36, 40, 52),
            hover_fg: fg,
            hover_border: Color::rgb(120, 140, 175),
            input_bg: Color::rgb(48, 52, 64),
            inactive_fg: Color::rgb(120, 122, 135),
            selection_bg: Color::rgb(60, 80, 120),
            link_fg: Color::rgb(110, 175, 230),
            completion_bg: Color::rgb(36, 40, 52),
            completion_fg: fg,
            completion_border: Color::rgb(120, 140, 175),
            completion_selected_bg: Color::rgb(50, 60, 90),
            accent_bg: Color::rgb(80, 160, 240),
            scrollbar_track: Color::rgb(40, 44, 56),
            scrollbar_thumb: Color::rgb(110, 115, 130),

            // Editor lift (#276 Phase C Stage 1) — neutral defaults.
            editor_active_background: bg,
            cursorline_bg: Color::rgb(30, 33, 45),
            dap_stopped_bg: Color::rgb(80, 70, 30),
            colorcolumn_bg: Color::rgb(30, 32, 42),
            diff_added_bg: Color::rgb(28, 50, 32),
            diff_removed_bg: Color::rgb(60, 30, 30),
            diff_padding_bg: Color::rgb(28, 30, 38),
            line_number_fg: muted,
            line_number_active_fg: Color::rgb(200, 200, 210),
            diagnostic_error: Color::rgb(220, 80, 80),
            diagnostic_warning: Color::rgb(220, 180, 80),
            diagnostic_info: Color::rgb(110, 175, 230),
            diagnostic_hint: Color::rgb(140, 200, 240),
            git_added: Color::rgb(120, 200, 120),
            git_modified: Color::rgb(220, 180, 80),
            git_deleted: Color::rgb(220, 80, 80),
            lightbulb: Color::rgb(220, 200, 80),
            spell_error: Color::rgb(110, 200, 200),
            cursor: Color::rgb(220, 220, 220),
            cursor_normal_alpha: 0.40,
            selection: Color::rgb(60, 80, 120),
            selection_alpha: 0.50,
            yank_highlight_bg: Color::rgb(220, 200, 80),
            yank_highlight_alpha: 0.30,
            bracket_match_bg: Color::rgb(80, 90, 110),
            indent_guide_fg: Color::rgb(50, 54, 66),
            indent_guide_active_fg: Color::rgb(110, 115, 130),
            annotation_fg: Color::rgb(110, 115, 130),
            ghost_text_fg: Color::rgb(110, 115, 130),
            command_line_bg: bg,
            command_line_fg: fg,

            // Board / kanban (#362)
            board_selected_card_bg: Color::rgb(50, 60, 90),
            board_col_header_bg: Color::rgb(40, 44, 56),
            badge_running: Color::rgb(220, 180, 80),
            badge_passed: Color::rgb(120, 200, 120),
            badge_warning: Color::rgb(220, 140, 60),
            badge_blocked: Color::rgb(220, 80, 80),
            card_hint_bg: Color::rgb(35, 40, 55),
            card_hint_fg: Color::rgb(180, 190, 210),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_theme_json(dir: &std::path::Path, name: &str, contents: &str) -> std::path::PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, contents).expect("write fixture theme json");
        path
    }

    #[test]
    fn strip_json_comments_removes_line_and_block_comments() {
        let input = "{\n  // a line comment\n  \"a\": 1, /* inline block */\n  \"b\": \"has // not a comment\"\n}";
        let stripped = strip_json_comments(input);
        let val: serde_json::Value = serde_json::from_str(&stripped).unwrap();
        assert_eq!(val["a"], 1);
        assert_eq!(val["b"], "has // not a comment");
    }

    #[test]
    fn from_vscode_json_maps_colors_onto_theme_fields() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write_theme_json(
            dir.path(),
            "sample.json",
            r##"{
                "colors": {
                    "editor.background": "#101820",
                    "editor.foreground": "#f0f0f0",
                    "tab.activeBackground": "#202830",
                    "tab.activeForeground": "#ffffff",
                    "editorError.foreground": "#ff0000",
                    "editorLineNumber.foreground": "#808080"
                }
            }"##,
        );
        let theme = Theme::from_vscode_json(&path).expect("parse fixture theme");
        assert_eq!(theme.background, Color::from_hex("#101820").unwrap());
        assert_eq!(theme.foreground, Color::from_hex("#f0f0f0").unwrap());
        assert_eq!(theme.tab_active_bg, Color::from_hex("#202830").unwrap());
        assert_eq!(theme.tab_active_fg, Color::from_hex("#ffffff").unwrap());
        assert_eq!(theme.error_fg, Color::from_hex("#ff0000").unwrap());
        assert_eq!(theme.diagnostic_error, Color::from_hex("#ff0000").unwrap());
        assert_eq!(theme.line_number_fg, Color::from_hex("#808080").unwrap());
    }

    /// Keys the theme file doesn't set must leave `Theme::default()`'s
    /// value in place — a minimal theme should still be fully usable,
    /// not partially black/undefined.
    #[test]
    fn from_vscode_json_leaves_unset_fields_at_default() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write_theme_json(
            dir.path(),
            "minimal.json",
            r##"{ "colors": { "editor.background": "#000000" } }"##,
        );
        let theme = Theme::from_vscode_json(&path).expect("parse fixture theme");
        let default = Theme::default();
        assert_eq!(theme.background, Color::rgb(0, 0, 0));
        // Untouched field keeps the default's value.
        assert_eq!(theme.badge_passed, default.badge_passed);
        assert_eq!(theme.accent_fg, default.accent_fg);
    }

    /// VS Code theme files are JSONC — comments must not break parsing.
    #[test]
    fn from_vscode_json_strips_comments_before_parsing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write_theme_json(
            dir.path(),
            "commented.json",
            "{\n  // top-level comment\n  \"colors\": {\n    \"editor.background\": \"#123456\" /* trailing */\n  }\n}",
        );
        let theme = Theme::from_vscode_json(&path).expect("parse commented fixture theme");
        assert_eq!(theme.background, Color::from_hex("#123456").unwrap());
    }

    /// `editor.lineHighlightBackground` is commonly a translucent
    /// `#rrggbbaa` overlay in real VS Code themes — it must be
    /// alpha-composited over `editor.background`, not taken at face
    /// value with its original (translucent) alpha.
    #[test]
    fn from_vscode_json_composites_translucent_line_highlight_over_background() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write_theme_json(
            dir.path(),
            "translucent.json",
            r##"{
                "colors": {
                    "editor.background": "#000000",
                    "editor.lineHighlightBackground": "#ffffff80"
                }
            }"##,
        );
        let theme = Theme::from_vscode_json(&path).expect("parse fixture theme");
        let bg = Color::from_hex("#000000").unwrap();
        let expected = Color::try_from_hex_over("#ffffff80", bg).unwrap();
        assert_eq!(theme.cursorline_bg, expected);
        // Fully opaque against black should be pure white, and fully
        // transparent should stay black — sanity-check the direction of
        // the blend rather than just re-deriving the same computation.
        assert_ne!(theme.cursorline_bg, Color::rgb(255, 255, 255));
        assert_ne!(theme.cursorline_bg, Color::rgb(0, 0, 0));
    }

    #[test]
    fn from_vscode_json_returns_none_for_missing_file() {
        let path = std::path::Path::new("/nonexistent/path/does-not-exist.json");
        assert_eq!(Theme::from_vscode_json(path), None);
    }

    #[test]
    fn from_vscode_json_returns_none_for_malformed_json() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write_theme_json(dir.path(), "broken.json", "{ not json at all");
        assert_eq!(Theme::from_vscode_json(&path), None);
    }

    /// #620: the active-tab top accent is exposed as a *method* derived
    /// from `accent_fg`, not as its own `Theme` field — a field addition
    /// is `error[E0063]` in coord-tui's three exhaustive palette literals
    /// (`tui/src/settings.rs`), which is exactly what reddened the
    /// `downstream consumers (compile truth)` gate. The default value is
    /// unchanged from the field version it replaced.
    #[test]
    fn tab_active_border_top_defaults_to_the_accent_colour() {
        let theme = Theme::default();
        assert_eq!(
            theme.tab_active_border_top(),
            theme.accent_fg,
            "the tab accent line must track the palette's accent colour"
        );
        assert_eq!(
            theme.tab_active_border_top(),
            Color::rgb(140, 200, 240),
            "default accent must match the value #620 originally shipped as a field"
        );
    }

    /// #500: the terminal find-highlight colours are exposed as
    /// *methods*, not `Theme` fields, for the same reason as
    /// `tab_active_border_top` above — see
    /// `exhaustive_theme_literal_still_compiles` below.
    #[test]
    fn terminal_find_highlight_colours_match_the_pre_shared_literals() {
        let theme = Theme::default();
        // Same RGB literals `tui`/`gtk`/`macos::terminal` each hardcoded
        // independently before #500 — the shared ladder must not change
        // the shipped colours, only where they're defined.
        assert_eq!(theme.find_active_bg(), Color::rgb(255, 165, 0));
        assert_eq!(theme.find_active_fg(), Color::rgb(0, 0, 0));
        assert_eq!(theme.find_match_bg(), Color::rgb(100, 80, 20));
    }

    /// The accent stays *themeable* despite not having its own field: an
    /// app that recolours `accent_fg` recolours the tab accent with it,
    /// which is what makes the method a lossless replacement rather than
    /// a hardcoded constant.
    #[test]
    fn tab_active_border_top_follows_a_custom_accent() {
        let theme = Theme {
            accent_fg: Color::rgb(255, 100, 0),
            ..Theme::default()
        };
        assert_eq!(theme.tab_active_border_top(), Color::rgb(255, 100, 0));
    }

    /// The regression this whole change is about: a `Theme` built with an
    /// **exhaustive literal and no `..Default::default()` spread** — the
    /// shape coord-tui's `light_palette` / `high_contrast_palette` /
    /// `solarized_palette` (`tui/src/settings.rs`) use — must keep
    /// compiling. This test is that consumer shape in miniature, and it
    /// is deliberately verbose: the `..` spread is the one thing it must
    /// NOT have, because the spread is exactly what would let a new
    /// `pub` field slip through here and surface as `error[E0063]` in a
    /// consumer's CI instead.
    ///
    /// **If you added a field to `Theme` and this stopped compiling,
    /// that is the test working.** Re-read [`Theme`]'s "adding a field
    /// here is a BREAKING change" section before adding the field below:
    /// a value derivable from an existing field belongs in a method (see
    /// [`Theme::tab_active_border_top`]), and a field that genuinely
    /// needs its own storage needs consumer migrations landed alongside.
    #[test]
    fn exhaustive_theme_literal_still_compiles() {
        // Values come from `Default` so this asserts the struct's
        // *shape*, not ~80 hardcoded colours.
        let d = Theme::default();
        let exhaustive = Theme {
            background: d.background,
            foreground: d.foreground,
            tab_bar_bg: d.tab_bar_bg,
            tab_active_bg: d.tab_active_bg,
            tab_active_fg: d.tab_active_fg,
            tab_inactive_fg: d.tab_inactive_fg,
            tab_preview_active_fg: d.tab_preview_active_fg,
            tab_preview_inactive_fg: d.tab_preview_inactive_fg,
            separator: d.separator,
            surface_bg: d.surface_bg,
            surface_fg: d.surface_fg,
            selected_bg: d.selected_bg,
            inactive_selected_bg: d.inactive_selected_bg,
            border_fg: d.border_fg,
            title_fg: d.title_fg,
            header_bg: d.header_bg,
            header_fg: d.header_fg,
            muted_fg: d.muted_fg,
            error_fg: d.error_fg,
            warning_fg: d.warning_fg,
            query_fg: d.query_fg,
            match_fg: d.match_fg,
            accent_fg: d.accent_fg,
            hover_bg: d.hover_bg,
            hover_fg: d.hover_fg,
            hover_border: d.hover_border,
            input_bg: d.input_bg,
            inactive_fg: d.inactive_fg,
            selection_bg: d.selection_bg,
            link_fg: d.link_fg,
            completion_bg: d.completion_bg,
            completion_fg: d.completion_fg,
            completion_border: d.completion_border,
            completion_selected_bg: d.completion_selected_bg,
            accent_bg: d.accent_bg,
            scrollbar_track: d.scrollbar_track,
            scrollbar_thumb: d.scrollbar_thumb,
            editor_active_background: d.editor_active_background,
            cursorline_bg: d.cursorline_bg,
            dap_stopped_bg: d.dap_stopped_bg,
            colorcolumn_bg: d.colorcolumn_bg,
            diff_added_bg: d.diff_added_bg,
            diff_removed_bg: d.diff_removed_bg,
            diff_padding_bg: d.diff_padding_bg,
            line_number_fg: d.line_number_fg,
            line_number_active_fg: d.line_number_active_fg,
            diagnostic_error: d.diagnostic_error,
            diagnostic_warning: d.diagnostic_warning,
            diagnostic_info: d.diagnostic_info,
            diagnostic_hint: d.diagnostic_hint,
            git_added: d.git_added,
            git_modified: d.git_modified,
            git_deleted: d.git_deleted,
            lightbulb: d.lightbulb,
            spell_error: d.spell_error,
            cursor: d.cursor,
            cursor_normal_alpha: d.cursor_normal_alpha,
            selection: d.selection,
            selection_alpha: d.selection_alpha,
            yank_highlight_bg: d.yank_highlight_bg,
            yank_highlight_alpha: d.yank_highlight_alpha,
            bracket_match_bg: d.bracket_match_bg,
            indent_guide_fg: d.indent_guide_fg,
            indent_guide_active_fg: d.indent_guide_active_fg,
            annotation_fg: d.annotation_fg,
            ghost_text_fg: d.ghost_text_fg,
            command_line_bg: d.command_line_bg,
            command_line_fg: d.command_line_fg,
            board_selected_card_bg: d.board_selected_card_bg,
            board_col_header_bg: d.board_col_header_bg,
            badge_running: d.badge_running,
            badge_passed: d.badge_passed,
            badge_warning: d.badge_warning,
            badge_blocked: d.badge_blocked,
            card_hint_bg: d.card_hint_bg,
            card_hint_fg: d.card_hint_fg,
        };
        assert_eq!(exhaustive, d);
    }
}
