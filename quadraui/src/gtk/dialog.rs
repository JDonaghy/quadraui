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
            crate::primitives::toolbar::ToolbarPaintOptions::default(),
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

#[cfg(test)]
mod tests {
    use gtk4::cairo::{Context, Format, ImageSurface};

    use super::*;
    use crate::event::Rect as QRect;
    use crate::primitives::dialog::{DialogButton, DialogMeasure};
    use crate::types::{Color, StyledText, WidgetId};

    const W: i32 = 320;
    const H: i32 = 240;

    /// Same byte layout as `gtk/tooltip.rs`'s test helper and
    /// `GtkDriver::pixel`.
    fn pixel(data: &[u8], stride: usize, x: i32, y: i32) -> (u8, u8, u8) {
        let off = y as usize * stride + x as usize * 4;
        (data[off + 2], data[off + 1], data[off])
    }

    fn sample_dialog() -> Dialog {
        Dialog {
            id: WidgetId::new("dlg"),
            title: StyledText::plain("Confirm"),
            body: vec![StyledText::plain("Delete this file?")],
            buttons: vec![
                DialogButton {
                    id: WidgetId::new("cancel"),
                    label: "Cancel".into(),
                    is_default: false,
                    is_cancel: true,
                    tint: None,
                },
                DialogButton {
                    id: WidgetId::new("ok"),
                    label: "Delete".into(),
                    is_default: true,
                    is_cancel: false,
                    tint: None,
                },
            ],
            severity: None,
            vertical_buttons: false,
            table: None,
            input: None,
        }
    }

    fn layout_for(dialog: &Dialog) -> DialogLayout {
        layout_for_button_row(dialog, 20.0, 80.0)
    }

    /// Like [`layout_for`], but with an explicit button row height/width
    /// — used by tests that paint labels in a larger font and need a
    /// button box tall enough to contain a full-coverage (non-antialiased
    /// edge) glyph pixel.
    fn layout_for_button_row(
        dialog: &Dialog,
        button_row_height: f32,
        button_width: f32,
    ) -> DialogLayout {
        let viewport = QRect::new(0.0, 0.0, W as f32, H as f32);
        let measure = DialogMeasure {
            width: 240.0,
            title_height: 16.0,
            body_height: 16.0,
            table_height: 0.0,
            input_height: 0.0,
            button_row_height,
            button_width,
            button_gap: 8.0,
            padding: 8.0,
        };
        dialog.layout(viewport, measure, |_| {
            crate::primitives::toolbar::ToolbarItemMeasure::new(0.0)
        })
    }

    /// Paint `dialog` via [`draw_dialog`] onto a fresh surface and return
    /// the raw pixel buffer alongside the stride. `preset_font`, when
    /// given, is set on the layout as the "ambient" font *before*
    /// `draw_dialog` is called — simulating whatever font a real caller
    /// would have had active for the editor content (mirroring `gtk::
    /// dialog::draw_dialog`'s own doc: the editor's monospace layout
    /// font). Used to pin the #1077 "Font role" drift: pre-migration,
    /// only title + buttons used `ui_font_desc` — body/table/input
    /// stayed in whatever font was already active.
    fn paint_with_preset(
        dialog: &Dialog,
        layout: &DialogLayout,
        ui_font_desc: &pango::FontDescription,
        preset_font: Option<&pango::FontDescription>,
    ) -> (Vec<u8>, usize) {
        let mut surface = ImageSurface::create(Format::ARgb32, W, H).expect("create ImageSurface");
        {
            let cr = Context::new(&surface).expect("Context::new");
            let pango_layout = pangocairo::functions::create_layout(&cr);
            if let Some(preset) = preset_font {
                pango_layout.set_font_description(Some(preset));
            }
            draw_dialog(
                &cr,
                &pango_layout,
                ui_font_desc,
                dialog,
                layout,
                16.0,
                &Theme::default(),
            );
        }
        surface.flush();
        let stride = surface.stride() as usize;
        let data = surface.data().expect("surface data").to_vec();
        (data, stride)
    }

    /// #1077: every button now gets a stroked border, not just the
    /// default button's selected-bg fill — pre-migration GTK drew no
    /// button border at all (this test would have failed against the
    /// old per-backend rasteriser: the edge pixel equalled the plain
    /// surface bg, same as the equivalent `macos::dialog` pin).
    #[test]
    fn every_button_paints_a_border_stroke() {
        let dialog = sample_dialog();
        let layout = layout_for(&dialog);
        let ui_font = pango::FontDescription::from_string("Sans 10");
        let (data, stride) = paint_with_preset(&dialog, &layout, &ui_font, None);

        // Cancel (button_idx 0) is NOT the default button, so its
        // interior is plain surface_bg — its top edge should still
        // differ (border ink) from a point a few pixels further in.
        let btn = layout
            .visible_buttons
            .iter()
            .find(|v| v.button_idx == 0)
            .expect("cancel button visible");
        let edge = pixel(
            &data,
            stride,
            (btn.bounds.x + btn.bounds.width / 2.0) as i32,
            btn.bounds.y as i32,
        );
        let inner = pixel(
            &data,
            stride,
            (btn.bounds.x + btn.bounds.width / 2.0) as i32,
            (btn.bounds.y + 3.0) as i32,
        );
        assert_ne!(
            edge, inner,
            "button top edge should show border ink, distinct from the button's own \
             interior (edge={edge:?}, inner={inner:?})"
        );
    }

    /// #1077: `DialogButton::tint` is now honoured on every backend —
    /// pre-migration GTK always painted every label in
    /// `theme.surface_fg`, silently dropping a caller's tint (same drift
    /// pinned for macOS in `macos::dialog::tests`).
    #[test]
    fn tinted_button_label_paints_in_the_tint_colour() {
        let mut dialog = sample_dialog();
        let tint = Color::rgb(220, 40, 40);
        dialog.buttons[1].tint = Some(tint); // "Delete" — the default button
                                             // A larger box + font than the default `layout_for` gives Cairo's
                                             // antialiased glyph rendering room for at least one fully-covered
                                             // (non-edge) interior pixel to actually equal the tint colour —
                                             // at the default small test font every pixel sampled some blend
                                             // with `theme.selected_bg` (button fill) instead.
        let layout = layout_for_button_row(&dialog, 40.0, 140.0);
        let ui_font = pango::FontDescription::from_string("Sans 24");
        let (data, stride) = paint_with_preset(&dialog, &layout, &ui_font, None);

        let btn = layout
            .visible_buttons
            .iter()
            .find(|v| v.button_idx == 1)
            .expect("default button visible");
        // Scan the whole button box, not one fixed row — the label is
        // centred both ways, and Cairo/Pango's antialiasing means a
        // single-row probe can land between a glyph's vertical stems
        // even though the label paints correctly (same reasoning as
        // `win::dialog::tests::tinted_button_label_paints_in_the_tint_colour`).
        let found = (btn.bounds.y as i32..(btn.bounds.y + btn.bounds.height) as i32)
            .flat_map(|y| {
                (btn.bounds.x as i32..(btn.bounds.x + btn.bounds.width) as i32).map(move |x| (x, y))
            })
            .map(|(x, y)| pixel(&data, stride, x, y))
            .any(|(r, g, b)| (r, g, b) == (tint.r, tint.g, tint.b));
        assert!(
            found,
            "tinted button label should paint at least one pixel in the tint colour \
             somewhere within its bounds"
        );
    }

    /// #1077 "Font role" drift: pre-migration GTK swapped fonts
    /// mid-paint — title + buttons rendered in `ui_font_desc`, but
    /// body/table/input rendered in whatever font was already active on
    /// `pango_layout` (the editor's monospace layout font). The shared
    /// `native_surface_paint::paint` now paints the *whole* dialog in
    /// `ui_font_desc`, so the body text pixels must come out identical
    /// regardless of what font was active before `draw_dialog` was
    /// called. Pinned by painting the same dialog twice with two wildly
    /// different "ambient" preset fonts (a huge one and a tiny one) and
    /// asserting the output is byte-identical — against the
    /// pre-migration rasteriser this would fail, since the huge/tiny
    /// preset font is exactly what body text used to render in.
    #[test]
    fn body_text_paints_in_the_chrome_font_regardless_of_the_layouts_prior_font() {
        let dialog = sample_dialog();
        let layout = layout_for(&dialog);
        let ui_font = pango::FontDescription::from_string("Sans 10");

        let huge_preset = pango::FontDescription::from_string("Monospace 40");
        let tiny_preset = pango::FontDescription::from_string("Monospace 6");

        let (data_huge, stride) = paint_with_preset(&dialog, &layout, &ui_font, Some(&huge_preset));
        let (data_tiny, stride_tiny) =
            paint_with_preset(&dialog, &layout, &ui_font, Some(&tiny_preset));

        assert_eq!(stride, stride_tiny);
        assert_eq!(
            data_huge, data_tiny,
            "dialog paint should be identical regardless of the pre-existing (editor) \
             font on the layout — the whole dialog now paints in ui_font_desc"
        );
    }

    /// The font swap must not leak: the layout's font description
    /// before `draw_dialog` is called should be restored afterwards
    /// (#247), so later paints in the same frame keep rendering in
    /// whatever font (editor) was active.
    #[test]
    fn draw_dialog_restores_the_layouts_original_font_description_afterwards() {
        let dialog = sample_dialog();
        let layout = layout_for(&dialog);
        let ui_font = pango::FontDescription::from_string("Sans 10");
        let editor_font = pango::FontDescription::from_string("Monospace 12");

        let surface = ImageSurface::create(Format::ARgb32, W, H).expect("create ImageSurface");
        let cr = Context::new(&surface).expect("Context::new");
        let pango_layout = pangocairo::functions::create_layout(&cr);
        pango_layout.set_font_description(Some(&editor_font));

        draw_dialog(
            &cr,
            &pango_layout,
            &ui_font,
            &dialog,
            &layout,
            16.0,
            &Theme::default(),
        );

        let restored = pango_layout.font_description().expect("font restored");
        assert_eq!(
            restored.to_str(),
            editor_font.to_str(),
            "layout's font description should be restored to the editor font after \
             draw_dialog returns, not left on ui_font_desc (#247)"
        );
    }
}
