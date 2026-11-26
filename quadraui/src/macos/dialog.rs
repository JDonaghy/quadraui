//! macOS rasteriser for [`crate::Dialog`].
//!
//! Content painting uses the shared
//! [`crate::primitives::dialog::native_surface_paint::paint`] function.
//! macOS's font-role behaviour (the whole dialog paints in `self.chrome_font`)
//! is unchanged by this — it was already the answer the shared `paint` adopted
//! for every backend.
//!
//! `DialogInput::Toolbar` renders through
//! `primitives::toolbar::native_surface_paint::paint_embedded` after the
//! shared `paint` returns, on the same chrome-font surface.
//!
//! Returns the per-button bounds as `Vec<Rect>` so the caller's click
//! handler can resolve button hits without re-running layout.

use core_graphics::sys::CGContextRef;
use core_text::font::CTFont;

use crate::event::Rect as QRect;
use crate::primitives::dialog::{native_surface_paint, Dialog, DialogInput, DialogLayout};
use crate::theme::Theme;

/// Draw a [`Dialog`] at its resolved layout. Returns per-button bounds.
///
/// # Safety
///
/// `ctx` must be a valid `CGContextRef` borrowed for the duration of
/// the call.
pub unsafe fn draw_dialog(
    ctx: CGContextRef,
    font: &CTFont,
    dialog: &Dialog,
    dialog_layout: &DialogLayout,
    line_height: f64,
    theme: &Theme,
) -> Vec<QRect> {
    let mut surface = super::surface::CgSurface {
        ctx,
        font: Some(font),
    };
    let button_rects = native_surface_paint::paint(
        dialog,
        dialog_layout,
        line_height as f32,
        &mut surface,
        theme,
        &crate::style::Style::default(),
    );

    // DialogInput::Toolbar isn't painted by the shared `paint` — see
    // that fn's module doc — so render it here.
    if let (Some(input_b), Some(DialogInput::Toolbar(toolbar))) =
        (dialog_layout.input_bounds, dialog.input.as_ref())
    {
        crate::primitives::toolbar::native_surface_paint::paint_embedded(
            toolbar,
            input_b,
            &mut surface,
            theme,
        );
    }

    button_rects
}

#[cfg(test)]
mod tests {
    use super::super::headless::BitmapSurface;
    use super::super::text::make_font;
    use super::super::MacBackend;
    use super::*;
    use crate::backend::DrawsPrimitives;
    use crate::event::Viewport;
    use crate::primitives::dialog::DialogButton;
    use crate::types::{Color, StyledText, WidgetId};
    use crate::Backend;

    const W: u32 = 320;
    const H: u32 = 200;

    fn font() -> CTFont {
        make_font("Menlo", 14.0).expect("Menlo installed")
    }

    fn sample_dialog() -> Dialog {
        Dialog {
            id: WidgetId::new("dlg"),
            title: StyledText::plain("Confirm"),
            body: vec![StyledText::plain("Delete this file?")],
            buttons: vec![
                DialogButton {
                    spec: None,
                    id: WidgetId::new("cancel"),
                    label: "Cancel".into(),
                    is_default: false,
                    is_cancel: true,
                    tint: None,
                },
                DialogButton {
                    spec: None,
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

    /// Paint `dialog` centred in the full viewport through the
    /// widget-contract path and hand back both the pixels and the layout
    /// the backend resolved, so every probe below reads the geometry that
    /// was actually painted.
    fn paint_via_backend(dialog: &Dialog) -> (BitmapSurface, DialogLayout) {
        let viewport = QRect::new(0.0, 0.0, W as f32, H as f32);
        let surface = BitmapSurface::new(W, H);
        surface.fill(0.0, 0.0, 0.0, 0.0);
        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        let layout = backend.enter_frame_scope(surface.context_ptr(), |b| {
            b.draw_primitive(viewport, dialog).layout
        });
        backend.end_frame();
        (surface, layout)
    }

    #[test]
    fn dialog_paints_surface_bg() {
        let dialog = sample_dialog();
        let (surface, layout) = paint_via_backend(&dialog);
        let theme = Theme::default();
        let b = layout.bounds;
        // Probe near the right edge, away from the body text.
        let (r, g, bp, _) =
            surface.pixel((b.x + b.width - 8.0) as u32, (b.y + b.height - 4.0) as u32);
        assert_eq!(
            (r, g, bp),
            (theme.surface_bg.r, theme.surface_bg.g, theme.surface_bg.b),
        );
    }

    #[test]
    fn default_button_paints_selected_bg() {
        let dialog = sample_dialog();
        let (surface, layout) = paint_via_backend(&dialog);
        let theme = Theme::default();
        // "Delete" is the default button — second in visible_buttons.
        let btn = layout
            .visible_buttons
            .iter()
            .find(|v| v.button_idx == 1)
            .expect("default button visible");
        // Probe near top edge of the button (away from label glyphs).
        let px = (btn.bounds.x + btn.bounds.width / 2.0) as u32;
        let py = (btn.bounds.y + 1.0) as u32;
        let (r, g, b, _) = surface.pixel(px, py);
        assert_eq!(
            (r, g, b),
            (
                theme.selected_bg.r,
                theme.selected_bg.g,
                theme.selected_bg.b
            ),
        );
    }

    #[test]
    fn button_rects_resolved_for_each_visible_button() {
        let dialog = sample_dialog();
        let (_surface, layout) = paint_via_backend(&dialog);
        assert_eq!(layout.visible_buttons.len(), 2);
    }

    /// Every button now gets a stroked border, not just the
    /// default button's selected-bg fill — pre-migration macOS drew no
    /// button border at all (this test would have failed against the
    /// old per-backend rasteriser: the edge pixel equalled the plain
    /// surface bg).
    #[test]
    fn every_button_paints_a_border_stroke() {
        let dialog = sample_dialog();
        let (surface, layout) = paint_via_backend(&dialog);
        // Cancel (button_idx 0) is NOT the default button, so its
        // interior is plain surface_bg — its top edge should still
        // differ (border ink) from a point 3px further in.
        let btn = layout
            .visible_buttons
            .iter()
            .find(|v| v.button_idx == 0)
            .expect("cancel button visible");
        let edge = surface.pixel(
            (btn.bounds.x + btn.bounds.width / 2.0) as u32,
            btn.bounds.y as u32,
        );
        let inner = surface.pixel(
            (btn.bounds.x + btn.bounds.width / 2.0) as u32,
            (btn.bounds.y + 3.0) as u32,
        );
        assert_ne!(
            edge, inner,
            "button top edge should show border ink, distinct from the button's own \
             interior (edge={edge:?}, inner={inner:?})"
        );
    }

    /// `DialogButton::tint` is now honoured on every backend —
    /// pre-migration macOS always painted every label in
    /// `theme.surface_fg`, silently dropping a caller's tint (the field's
    /// own doc says it's for destructive-action colouring, e.g. "Delete").
    #[test]
    fn tinted_button_label_paints_in_the_tint_colour() {
        let mut dialog = sample_dialog();
        let tint = Color::rgb(220, 40, 40);
        dialog.buttons[1].tint = Some(tint); // "Delete" — the default button
        let (surface, layout) = paint_via_backend(&dialog);
        let btn = layout
            .visible_buttons
            .iter()
            .find(|v| v.button_idx == 1)
            .expect("default button visible");
        // Scan the button row for the tint colour — the label is
        // centred, so a fixed-x probe risks landing between glyphs.
        let y = (btn.bounds.y + btn.bounds.height / 2.0) as u32;
        let found = (btn.bounds.x as u32..(btn.bounds.x + btn.bounds.width) as u32)
            .map(|x| surface.pixel(x, y))
            .any(|(r, g, b, _)| (r, g, b) == (tint.r, tint.g, tint.b));
        assert!(
            found,
            "tinted button label should paint at least one pixel in the tint colour \
             somewhere in its row"
        );
    }
}
