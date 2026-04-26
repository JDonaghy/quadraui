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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
        }
    }
}
