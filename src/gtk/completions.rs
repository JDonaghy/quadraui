//! GTK rasteriser for [`crate::Completions`] (#285).
//!
//! Verbatim port of vimcode's `src/gtk/draw::draw_completion_popup`
//! body. Paints a Cairo-rendered autocomplete popup: full-bounds
//! background fill, full-bounds 1 px border stroke, per-item
//! selected-row highlight, candidate label rendered as `" {label}"`
//! via Pango.
//!
//! Per D6 (`docs/BACKEND_TRAIT_PROPOSAL.md` §9): the host invokes
//! `Completions::layout(...)` with the cursor anchor, viewport,
//! popup width / max height, and a per-item measure closure; this
//! rasteriser then paints the resolved [`crate::CompletionsLayout`]
//! verbatim.
//!
//! ## Border vs TUI
//!
//! GTK strokes a full 4-side border around the popup (Cairo
//! `rectangle` + `stroke`). The TUI rasteriser draws side `│`
//! borders only — its row geometry has no top/bottom rows for the
//! chrome. This divergence is intrinsic to the surfaces (TUI cells
//! coalesce; Cairo paints arbitrary geometry). The data primitive is
//! shared; the chrome paint approach is not.

use gtk4::cairo::Context;
use gtk4::pango;
use pangocairo::functions as pcfn;

use super::cairo_rgb;
use crate::primitives::completions::{Completions, CompletionsLayout};
use crate::theme::Theme;

/// Paint a [`Completions`] popup at its resolved [`CompletionsLayout`].
///
/// Background uses [`Theme::completion_bg`], selected row uses
/// [`Theme::completion_selected_bg`], item text uses
/// [`Theme::completion_fg`], and the popup border uses
/// [`Theme::completion_border`].
pub fn draw_completions(
    cr: &Context,
    pango_layout: &pango::Layout,
    completions: &Completions,
    layout: &CompletionsLayout,
    theme: &Theme,
) {
    let bounds = layout.bounds;
    if bounds.width <= 0.0 || bounds.height <= 0.0 {
        return;
    }
    let popup_x = bounds.x as f64;
    let popup_y = bounds.y as f64;
    let popup_w = bounds.width as f64;
    let popup_h = bounds.height as f64;

    // Background fill.
    let (r, g, b) = cairo_rgb(theme.completion_bg);
    cr.set_source_rgb(r, g, b);
    cr.rectangle(popup_x, popup_y, popup_w, popup_h);
    cr.fill().ok();

    // Border (full 4-side, matches pre-lift GTK chrome — see module doc
    // for the divergence-vs-TUI rationale).
    let (r, g, b) = cairo_rgb(theme.completion_border);
    cr.set_source_rgb(r, g, b);
    cr.set_line_width(1.0);
    cr.rectangle(popup_x, popup_y, popup_w, popup_h);
    cr.stroke().ok();

    // Items.
    for vis in &layout.visible_items {
        let Some(item) = completions.items.get(vis.item_idx) else {
            continue;
        };
        let item_x = vis.bounds.x as f64;
        let item_y = vis.bounds.y as f64;
        let item_w = vis.bounds.width as f64;
        let item_h = vis.bounds.height as f64;

        if vis.item_idx == completions.selected_idx {
            let (r, g, b) = cairo_rgb(theme.completion_selected_bg);
            cr.set_source_rgb(r, g, b);
            cr.rectangle(item_x, item_y, item_w, item_h);
            cr.fill().ok();
        }

        let (r, g, b) = cairo_rgb(theme.completion_fg);
        cr.set_source_rgb(r, g, b);
        let label = item
            .label
            .spans
            .first()
            .map(|s| s.text.as_str())
            .unwrap_or("");
        let display = format!(" {label}");
        pango_layout.set_text(&display);
        pango_layout.set_attributes(None);
        cr.move_to(item_x, item_y);
        pcfn::show_layout(cr, pango_layout);
    }
}
