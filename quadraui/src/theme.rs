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
        }
    }
}
