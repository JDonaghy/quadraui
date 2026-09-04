//! Public GTK (Cairo + Pango) rasterisers for `quadraui` primitives.
//!
//! Enabled via the `gtk` Cargo feature. Apps depend on `quadraui` with
//! `features = ["gtk"]` and call these `draw_*` functions to paint
//! primitives onto a [`gtk4::cairo::Context`] using a
//! [`pango::Layout`] for text measurement.
//!
//! Per D6 (see `docs/BACKEND_TRAIT_PROPOSAL.md` §9): primitives own
//! layout, backends rasterise. Most GTK rasterisers in this module
//! compute the primitive's `*Layout` internally because Pango
//! measurement requires the live `pango::Layout` — taking the layout
//! pre-computed would force callers to share that handle through two
//! separate phases. The result the layout would have produced is
//! returned alongside any per-frame hit regions so callers can dispatch
//! clicks.
//!
//! This module is the destination of issue #223 — the per-primitive
//! rasterisers are being lifted out of vimcode (`src/gtk/quadraui_gtk.rs`)
//! and kubeui (private `draw_status_bar` in `kubeui-gtk/src/main.rs`)
//! one primitive at a time. StatusBar is the pilot.

use gtk4::cairo::Context;
use gtk4::pango;

use crate::types::Color;

mod activity_bar;
pub mod backend;
mod board;
mod chart;
mod command_center;
mod command_line;
mod completions;
mod context_menu;
mod data_table;
mod dialog;
mod diff_view;
mod drop_overlay;
mod editor;
pub mod events;
mod find_replace;
mod form;
mod image;
mod list;
mod menu_bar;
pub mod menu_overlay;
mod message_list;
mod minimap;
mod multi_section_view;
mod painted_text;
mod palette;
mod panel;
mod pipeline_view;
mod progress;
mod rich_text_popup;
mod run;
mod scrollbar;
pub mod services;
pub mod shell_runner;
mod sidebar_panel;
mod spinner;
mod split;
mod split_tree;
mod status_bar;
mod tab_bar;
mod terminal;
pub mod testing;
mod text_display;
mod text_input;
mod toast;
mod toolbar;
mod tooltip;
mod tree;

pub use crate::primitives::tab_bar::TabBarHits;
pub use activity_bar::{draw_activity_bar, draw_activity_bar_with_style, ACTIVITY_ROW_PX};
pub use backend::GtkBackend;
pub use board::{draw_board, gtk_board_layout};
pub use chart::{draw_chart, gtk_chart_layout};
pub use command_center::{draw_command_center, gtk_command_center_layout};
pub use completions::draw_completions;
pub use context_menu::draw_context_menu;
pub use data_table::{draw_data_table, gtk_data_table_layout};
pub use dialog::draw_dialog;
pub use diff_view::draw_diff_view;
pub use drop_overlay::draw_drop_overlay;
pub use editor::{draw_editor, editor_col_at_x};
pub use events::{wire_da_events, wire_da_events_with_scroll_direction};
pub use find_replace::draw_find_replace;
pub use form::{draw_form, draw_settings_chrome};
pub use image::draw_image;
pub use list::{draw_list, gtk_list_layout};
pub use menu_bar::{draw_menu_bar, gtk_menu_bar_layout};
pub use menu_overlay::MenuOverlay;
pub use message_list::draw_message_list;
// #738: the legibility/render-mode thresholds (`is_legible`, `render_mode`,
// `minimap_font_px`, `MinimapRenderMode`) and the shared `ROW_PITCH_PX` /
// `COLUMN_CAPACITY` geometry constants moved to `primitives::minimap` so
// `win::minimap` (and any future macOS rasteriser) consume the exact same
// decision logic — see that module's docs. Zero hits for any of those
// names in `~/src/coord-tui/src` or `~/src/vimcode/src`, so they're
// dropped from this re-export list rather than kept as a stale alias
// (CLAUDE.md's "Downstream consumers" rule 8: zero hits ⇒ free to move).
pub use minimap::{draw_minimap, gtk_minimap_layout};
pub use multi_section_view::{
    draw_multi_section_view, gtk_msv_layout, metrics_for as multi_section_view_metrics,
};
pub use palette::draw_palette;
pub use panel::{draw_panel, gtk_panel_layout};
pub use pipeline_view::{draw_pipeline_view, gtk_pipeline_view_layout};
pub use progress::{draw_progress, gtk_progress_layout};
pub use rich_text_popup::{
    draw_rich_text_popup, RICH_TEXT_POPUP_SB_INSET, RICH_TEXT_POPUP_SB_WIDTH,
};
pub use run::{run, run_with, RunConfig};
pub use scrollbar::draw_scrollbar;
pub use sidebar_panel::{draw_sidebar_panel, gtk_sidebar_panel_layout};
pub use spinner::{draw_spinner, gtk_spinner_layout};
pub use split::{draw_split, gtk_split_layout};
pub use split_tree::{draw_split_tree, gtk_split_tree_layout};
pub use status_bar::{draw_status_bar, MIN_GAP_PX};
pub use tab_bar::{
    draw_tab_bar, draw_tab_bar_icons, draw_tab_bar_icons_with_chrome, draw_tab_bar_with_chrome,
};
pub use terminal::{draw_terminal_cells, draw_terminal_divider};
pub use text_display::{draw_text_display, gtk_text_display_layout};
pub use text_input::{draw_text_input, gtk_text_input_layout};
pub use toast::{draw_toast_stack, gtk_toast_stack_layout};
pub use toolbar::{draw_toolbar, gtk_toolbar_layout};
pub use tooltip::{draw_tooltip, draw_tooltip_with_chrome};
pub use tree::{draw_tree, gtk_tree_layout};

/// Convert a `quadraui::Color` (0-255 RGBA) into Cairo's normalised
/// `(r, g, b)` tuple. Alpha is dropped — Cairo supports
/// `set_source_rgba` if a future primitive needs it.
pub fn cairo_rgb(c: Color) -> (f64, f64, f64) {
    (c.r as f64 / 255.0, c.g as f64 / 255.0, c.b as f64 / 255.0)
}

/// `set_source_rgb` shortcut used internally by the rasterisers and
/// available to apps that want their auxiliary draws to colour-match.
pub fn set_source(cr: &Context, c: Color) {
    let (r, g, b) = cairo_rgb(c);
    cr.set_source_rgb(r, g, b);
}

/// Build a closed rounded-rectangle Cairo path with corner radius `r`.
/// Used by rasterisers that need bordered/clipped rounded rects
/// (context menu, list view, command center).
pub(crate) fn rounded_rect_path(cr: &Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    use std::f64::consts::{FRAC_PI_2, PI};
    cr.new_path();
    cr.arc(x + w - r, y + r, r, -FRAC_PI_2, 0.0);
    cr.arc(x + w - r, y + h - r, r, 0.0, FRAC_PI_2);
    cr.arc(x + r, y + h - r, r, FRAC_PI_2, PI);
    cr.arc(x + r, y + r, r, PI, 3.0 * FRAC_PI_2);
    cr.close_path();
}

/// Re-export so apps can name the Pango layout type without depending
/// on `gtk4::pango` directly.
pub use pango::Layout as PangoLayout;

/// Font family GTK chrome text falls back to for Nerd-Font glyphs.
///
/// Matches the family [`activity_bar::ICON_FONT_DESC`] and
/// [`tab_bar::tab_icon_font`] already pin for the activity bar and tab
/// icons (#620) — kept as one named constant so retargeting to a
/// differently-named Nerd Font patch (or a consumer's fontconfig alias)
/// is a one-line change instead of hunting every rasteriser that
/// references the literal.
pub const NERD_FONT_FALLBACK_FAMILY: &str = "Symbols Nerd Font";

/// Build a Pango font description for GTK chrome text (list/tree/
/// palette rows, status bar segments, menu items, dialogs, ...) from a
/// `ui_font`-style description string (e.g. `"Sans 11"`), appending
/// [`NERD_FONT_FALLBACK_FAMILY`] to the family list.
///
/// Chrome primitives paint icon glyphs (`Icon::glyph`) and — on the
/// status bar — consumer-composed segment text that can itself embed a
/// raw Nerd-Font codepoint, inline with ordinary label text, all using
/// the *same* Pango layout and font. Most system UI fonts ("Sans",
/// "Cantarell", ...) have no coverage for the Private-Use-Area
/// codepoints Nerd Font glyphs live in, so without an explicit fallback
/// family in the description Pango's per-character font substitution is
/// left to guess at a system font that happens to cover that codepoint
/// range — unreliable, and prone to picking an unrelated font that
/// defines *something* there, rather than the intended glyph. Naming
/// the fallback family explicitly, in family-list order right after the
/// caller's own family, makes glyph resolution deterministic instead of
/// dependent on what else is installed (quadraui#416).
///
/// Harmless when no glyph is being painted: Pango only consults a later
/// family in the list for characters the earlier one doesn't cover, so
/// plain text keeps rendering in the caller's own `ui_font` family.
pub(crate) fn chrome_font_description(ui_font: &str) -> pango::FontDescription {
    with_nerd_font_fallback(&pango::FontDescription::from_string(ui_font))
}

/// Clone `base` with [`NERD_FONT_FALLBACK_FAMILY`] appended to its family
/// list. Used where a rasteriser can't build a fresh description from a
/// `ui_font` string (the caller only hands it an already-live
/// [`pango::FontDescription`] to paint one glyph with, e.g. an icon
/// inside an otherwise editor-font-painted row) — see
/// [`chrome_font_description`] for the string-based sibling and the full
/// rationale.
pub(crate) fn with_nerd_font_fallback(base: &pango::FontDescription) -> pango::FontDescription {
    let mut desc = base.clone();
    let family = desc.family().map(|f| f.to_string()).unwrap_or_default();
    let with_fallback = if family.is_empty() {
        NERD_FONT_FALLBACK_FAMILY.to_string()
    } else {
        format!("{family},{NERD_FONT_FALLBACK_FAMILY}")
    };
    desc.set_family(&with_fallback);
    desc
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The family list must lead with the caller's own `ui_font` family
    /// (so ordinary chrome text keeps rendering in the configured font)
    /// and include [`NERD_FONT_FALLBACK_FAMILY`] afterward (so a glyph
    /// the primary family can't cover — e.g. a Nerd-Font icon — still
    /// resolves instead of rendering tofu/blank).
    #[test]
    fn chrome_font_description_appends_nerd_font_fallback() {
        let desc = chrome_font_description("Sans 11");
        let family = desc.family().expect("family set").to_string();
        assert_eq!(family, "Sans,Symbols Nerd Font");
    }

    /// The point size from the input description string must survive —
    /// only the family list is touched.
    #[test]
    fn chrome_font_description_preserves_size() {
        let plain = pango::FontDescription::from_string("Sans 11");
        let desc = chrome_font_description("Sans 11");
        assert_eq!(desc.size(), plain.size());
    }

    /// A caller-supplied multi-family list (a consumer's own fallback
    /// chain) is preserved verbatim, with the Nerd Font family appended
    /// rather than replacing it.
    #[test]
    fn chrome_font_description_appends_after_existing_family_list() {
        let desc = chrome_font_description("Cantarell,DejaVu Sans 12");
        let family = desc.family().expect("family set").to_string();
        assert_eq!(family, "Cantarell,DejaVu Sans,Symbols Nerd Font");
    }

    /// [`with_nerd_font_fallback`] mirrors [`chrome_font_description`]
    /// for callers that only have a live `FontDescription` to hand (e.g.
    /// `gtk::palette::draw_palette`'s icon-glyph swap, which must not
    /// disturb the row's base font for the label text painted around
    /// it) — same family-list-append behaviour, size preserved.
    #[test]
    fn with_nerd_font_fallback_appends_to_an_existing_description() {
        let base = pango::FontDescription::from_string("Monospace 13");
        let desc = with_nerd_font_fallback(&base);
        let family = desc.family().expect("family set").to_string();
        assert_eq!(family, "Monospace,Symbols Nerd Font");
        assert_eq!(desc.size(), base.size());
    }
}
