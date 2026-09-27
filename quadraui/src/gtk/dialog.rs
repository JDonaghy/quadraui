//! GTK rasteriser for [`crate::Dialog`].
//!
//! Content painting (background, border, title, body, table, text-input
//! slot, buttons) moved to the shared
//! [`crate::primitives::dialog::native_surface_paint::paint`] (#1077,
//! `NativeSurface` Phase 4 slice 4/8) — see that fn's module doc for the
//! substantial drift it resolved (font role, per-button border, button
//! tint, label/input vertical centring, table `column_widths` +
//! separator sizing).
//!
//! **Font role**: the whole dialog now paints in the chrome/UI font —
//! previously only the title + buttons did, with body/table/input in
//! the editor's monospace layout font (see the shared `paint`'s doc for
//! why). This function saves the layout's existing (editor / mono) font
//! description on entry and swaps in `ui_font_desc` for the entire
//! paint, restoring it before returning so subsequent paints in the same
//! frame keep rendering in the editor font (#247) — same save/restore
//! shape as before, just applied once instead of per-region.
//!
//! `DialogInput::Toolbar` still renders through
//! [`super::toolbar::draw_toolbar`] after the shared `paint` returns,
//! now in the chrome font left active by that swap (a welcome side
//! effect: [`crate::ChromePrimitive::Toolbar`] classifies the toolbar
//! itself as chrome too, so an embedded dialog toolbar moving off the
//! editor font is a second drift this same change happens to close, not
//! a separate deliberate one).
//!
//! Returns the per-button hit rectangles `(x, y, w, h)` so the caller's
//! click handler can resolve a click to a button without re-running the
//! layout.

use gtk4::cairo::Context;
use gtk4::pango;

use crate::primitives::dialog::{native_surface_paint, Dialog, DialogInput, DialogLayout};
use crate::theme::Theme;

/// Draw a [`Dialog`] at its resolved layout. Returns
/// `Vec<(x, y, w, h)>` per visible button.
///
/// `pango_layout` is the editor's monospace Pango layout — the
/// rasteriser temporarily swaps in `ui_font_desc` for the entire dialog
/// (title, body, table, input, buttons — see this module's doc), then
/// restores the layout's original font description before returning.
pub fn draw_dialog(
    cr: &Context,
    pango_layout: &pango::Layout,
    ui_font_desc: &pango::FontDescription,
    dialog: &Dialog,
    dialog_layout: &DialogLayout,
    line_height: f64,
    theme: &Theme,
) -> Vec<(f64, f64, f64, f64)> {
    let bounds = dialog_layout.bounds;
    if bounds.width <= 0.0 || bounds.height <= 0.0 {
        return Vec::new();
    }

    // Save the layout's existing (editor / mono) font description so we
    // can swap to `ui_font_desc` for the whole dialog and restore at the
    // end. Without this, the UI font would leak into subsequent draw
    // calls in the same frame (#247).
    let saved_font = pango_layout.font_description();
    pango_layout.set_font_description(Some(ui_font_desc));

    let mut surface = super::surface::CairoSurface {
        cr,
        layout: Some(pango_layout),
        translucent_fill: true,
    };
    let button_rects = native_surface_paint::paint(
        dialog,
        dialog_layout,
        line_height as f32,
        &mut surface,
        theme,
    );

    // DialogInput::Toolbar isn't painted by the shared `paint` — the
    // toolbar rasteriser hasn't moved onto `NativeSurface` yet (out of
    // #1077's scope; see the shared `paint`'s module doc) — so render it
    // here, exactly as before this migration.
    if let (Some(input_b), Some(DialogInput::Toolbar(toolbar))) =
        (dialog_layout.input_bounds, dialog.input.as_ref())
    {
        super::toolbar::draw_toolbar(
            cr,
            pango_layout,
            input_b.x as f64,
            input_b.y as f64,
            input_b.width as f64,
            input_b.height as f64,
            toolbar,
            theme,
            None,
            None,
        );
    }

    // Restore the layout's font_description so subsequent paints in the
    // same frame use the editor font, not the chrome font we left active
    // (#247).
    pango_layout.set_font_description(saved_font.as_ref());

    button_rects
        .into_iter()
        .map(|r| (r.x as f64, r.y as f64, r.width as f64, r.height as f64))
        .collect()
}
