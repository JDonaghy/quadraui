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
mod completions;
mod context_menu;
mod dialog;
mod editor;
pub mod events;
mod find_replace;
mod form;
mod list;
mod message_list;
mod multi_section_view;
mod palette;
mod rich_text_popup;
mod run;
mod scrollbar;
pub mod services;
mod status_bar;
mod tab_bar;
mod terminal;
mod text_display;
mod tooltip;
mod tree;

pub use crate::primitives::tab_bar::TabBarHits;
pub use activity_bar::{draw_activity_bar, ACTIVITY_ROW_PX};
pub use backend::GtkBackend;
pub use completions::draw_completions;
pub use context_menu::draw_context_menu;
pub use dialog::draw_dialog;
pub use editor::draw_editor;
pub use find_replace::draw_find_replace;
pub use form::{draw_form, draw_settings_chrome};
pub use list::draw_list;
pub use message_list::draw_message_list;
pub use multi_section_view::{
    draw_multi_section_view, layout_for as multi_section_view_layout,
    metrics_for as multi_section_view_metrics,
};
pub use palette::draw_palette;
pub use rich_text_popup::{
    draw_rich_text_popup, RICH_TEXT_POPUP_SB_INSET, RICH_TEXT_POPUP_SB_WIDTH,
};
pub use run::run;
pub use scrollbar::draw_scrollbar;
pub use status_bar::draw_status_bar;
pub use tab_bar::draw_tab_bar;
pub use terminal::draw_terminal_cells;
pub use text_display::draw_text_display;
pub use tooltip::draw_tooltip;
pub use tree::draw_tree;

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

/// Re-export so apps can name the Pango layout type without depending
/// on `gtk4::pango` directly.
pub use pango::Layout as PangoLayout;
