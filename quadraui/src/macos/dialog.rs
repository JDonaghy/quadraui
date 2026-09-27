//! macOS rasteriser for [`crate::Dialog`].
//!
//! Content painting moved to the shared
//! [`crate::primitives::dialog::native_surface_paint::paint`] (#1077,
//! `NativeSurface` Phase 4 slice 4/8) — see that fn's module doc for the
//! drift it resolved. macOS's own font-role behaviour (the whole dialog
//! paints in `self.chrome_font`, per issue #1003) is unchanged by this
//! migration — it was already the answer the shared `paint` adopted for
//! every backend.
//!
//! `DialogInput::Toolbar` still renders through
//! [`super::toolbar::draw_toolbar`] after the shared `paint` returns,
//! using the same `font` (chrome) as before — unchanged.
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
    );

    // DialogInput::Toolbar isn't painted by the shared `paint` — see
    // that fn's module doc — so render it here, exactly as before this
    // migration.
    if let (Some(input_b), Some(DialogInput::Toolbar(toolbar))) =
        (dialog_layout.input_bounds, dialog.input.as_ref())
    {
        // SAFETY: `ctx` validity is this function's own caller contract,
        // forwarded unchanged.
        unsafe {
            super::toolbar::draw_toolbar(
                ctx,
                font,
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
    }

    button_rects
}

#[cfg(test)]
mod tests {
    use super::super::headless::BitmapSurface;
    use super::super::text::{make_font, measure_text};
    use super::super::MacBackend;
    use super::*;
    use crate::event::Viewport;
    use crate::primitives::dialog::{DialogButton, DialogMeasure};
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

    fn layout_for(dialog: &Dialog, viewport: QRect, line_height: f32) -> DialogLayout {
        let measure = DialogMeasure {
            width: 240.0,
            title_height: line_height,
            body_height: line_height,
            // Fixture dialogs carry no `table`, so the table slot is
            // zero-height. Field added to `DialogMeasure` after this
            // fixture was written; it never compiled until quadraui#484
            // first built the macOS test target.
            table_height: 0.0,
            input_height: if dialog.input.is_some() {
                line_height
            } else {
                0.0
            },
            button_row_height: line_height,
            button_width: 80.0,
            button_gap: 8.0,
            padding: 8.0,
        };
        dialog.layout(viewport, measure, |btn| {
            use crate::primitives::toolbar::ToolbarItemMeasure;
            let text_w = match btn {
                crate::primitives::toolbar::ToolbarButton::Action { label, .. } => {
                    let (w, _) = measure_text(&make_font("Menlo", 14.0).unwrap(), label);
                    w + 16.0
                }
                crate::primitives::toolbar::ToolbarButton::Separator => 12.0,
                crate::primitives::toolbar::ToolbarButton::Label { text, .. } => {
                    let (w, _) = measure_text(&make_font("Menlo", 14.0).unwrap(), text);
                    w
                }
            };
            ToolbarItemMeasure::new(text_w as f32)
        })
    }

    fn paint_via_backend(dialog: &Dialog, layout: &DialogLayout) -> (BitmapSurface, Vec<QRect>) {
        let surface = BitmapSurface::new(W, H);
        surface.fill(0.0, 0.0, 0.0, 0.0);
        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        let rects = std::cell::RefCell::new(Vec::new());
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            *rects.borrow_mut() = b.draw_dialog(dialog, layout);
        });
        backend.end_frame();
        (surface, rects.into_inner())
    }

    #[test]
    fn dialog_paints_surface_bg() {
        let dialog = sample_dialog();
        let viewport = QRect::new(0.0, 0.0, W as f32, H as f32);
        let layout = layout_for(&dialog, viewport, 16.0);
        let (surface, _) = paint_via_backend(&dialog, &layout);
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
        let viewport = QRect::new(0.0, 0.0, W as f32, H as f32);
        let layout = layout_for(&dialog, viewport, 16.0);
        let (surface, _) = paint_via_backend(&dialog, &layout);
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
    fn button_rects_returned_for_each_visible_button() {
        let dialog = sample_dialog();
        let viewport = QRect::new(0.0, 0.0, W as f32, H as f32);
        let layout = layout_for(&dialog, viewport, 16.0);
        let (_surface, rects) = paint_via_backend(&dialog, &layout);
        assert_eq!(rects.len(), 2);
    }

    /// #1077: every button now gets a stroked border, not just the
    /// default button's selected-bg fill — pre-migration macOS drew no
    /// button border at all (this test would have failed against the
    /// old per-backend rasteriser: the edge pixel equalled the plain
    /// surface bg).
    #[test]
    fn every_button_paints_a_border_stroke() {
        let dialog = sample_dialog();
        let viewport = QRect::new(0.0, 0.0, W as f32, H as f32);
        let layout = layout_for(&dialog, viewport, 16.0);
        let (surface, _) = paint_via_backend(&dialog, &layout);
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

    /// #1077: `DialogButton::tint` is now honoured on every backend —
    /// pre-migration macOS always painted every label in
    /// `theme.surface_fg`, silently dropping a caller's tint (the field's
    /// own doc says it's for destructive-action colouring, e.g. "Delete").
    #[test]
    fn tinted_button_label_paints_in_the_tint_colour() {
        let mut dialog = sample_dialog();
        let tint = Color::rgb(220, 40, 40);
        dialog.buttons[1].tint = Some(tint); // "Delete" — the default button
        let viewport = QRect::new(0.0, 0.0, W as f32, H as f32);
        let layout = layout_for(&dialog, viewport, 16.0);
        let (surface, _) = paint_via_backend(&dialog, &layout);
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
