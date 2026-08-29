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

impl Theme {
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
