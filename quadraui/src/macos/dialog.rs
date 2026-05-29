//! macOS rasteriser for [`crate::Dialog`].
//!
//! Mirrors [`crate::gtk::dialog::draw_dialog`]: bordered box, title
//! row, body lines, optional input row, button row at the bottom.
//! Returns the per-button bounds as `Vec<Rect>` so the caller's click
//! handler can resolve button hits without re-running layout.

use core_graphics::geometry::CGRect;
use core_graphics::sys::CGContextRef;
use core_text::font::CTFont;

use super::text::{draw_text, measure_text};
use crate::event::Rect as QRect;
use crate::primitives::dialog::{Dialog, DialogInput, DialogLayout};
use crate::theme::Theme;
use crate::types::{Color, StyledText};

fn flatten(text: &StyledText) -> String {
    text.spans.iter().map(|s| s.text.as_str()).collect()
}

/// Draw a [`Dialog`] at its resolved layout. Returns per-button bounds.
///
/// # Safety
///
/// `ctx` must be a valid `CGContextRef` borrowed for the duration of
/// the call.
#[allow(clippy::too_many_arguments)]
pub unsafe fn draw_dialog(
    ctx: CGContextRef,
    font: &CTFont,
    dialog: &Dialog,
    dialog_layout: &DialogLayout,
    line_height: f64,
    theme: &Theme,
) -> Vec<QRect> {
    let bounds = dialog_layout.bounds;
    if bounds.width <= 0.0 || bounds.height <= 0.0 {
        return Vec::new();
    }

    fill_rect(
        ctx,
        bounds.x as f64,
        bounds.y as f64,
        bounds.width as f64,
        bounds.height as f64,
        theme.surface_bg,
    );
    stroke_rect(
        ctx,
        bounds.x as f64,
        bounds.y as f64,
        bounds.width as f64,
        bounds.height as f64,
        theme.border_fg,
        1.0,
    );

    if let Some(title_rect) = dialog_layout.title_bounds {
        draw_text(
            ctx,
            font,
            &flatten(&dialog.title),
            title_rect.x as f64,
            title_rect.y as f64,
            color_to_cg(theme.title_fg),
        );
    }

    let body_b = dialog_layout.body_bounds;
    for (i, line) in dialog.body.iter().enumerate() {
        let row_y = body_b.y as f64 + i as f64 * line_height;
        if row_y + line_height > body_b.y as f64 + body_b.height as f64 {
            break;
        }
        draw_text(
            ctx,
            font,
            &flatten(line),
            body_b.x as f64,
            row_y,
            color_to_cg(theme.surface_fg),
        );
    }

    if let (Some(input_b), Some(input_kind)) = (dialog_layout.input_bounds, dialog.input.as_ref()) {
        match input_kind {
            DialogInput::TextInput(input) => {
                fill_rect(
                    ctx,
                    input_b.x as f64,
                    input_b.y as f64,
                    input_b.width as f64,
                    input_b.height as f64,
                    theme.input_bg,
                );
                stroke_rect(
                    ctx,
                    input_b.x as f64,
                    input_b.y as f64,
                    input_b.width as f64,
                    input_b.height as f64,
                    theme.border_fg,
                    1.0,
                );
                let display = if input.value.is_empty() {
                    format!(" {}", input.placeholder)
                } else {
                    format!(" {}", input.value)
                };
                let (_, lh) = measure_text(font, &display);
                draw_text(
                    ctx,
                    font,
                    &display,
                    input_b.x as f64 + 2.0,
                    input_b.y as f64 + (input_b.height as f64 - lh) / 2.0,
                    color_to_cg(theme.surface_fg),
                );
            }
            DialogInput::Toolbar(toolbar) => {
                // Render the embedded toolbar using the macOS toolbar
                // rasteriser.
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
    }

    let mut rects: Vec<QRect> = Vec::with_capacity(dialog_layout.visible_buttons.len());
    for vis in &dialog_layout.visible_buttons {
        let btn = &dialog.buttons[vis.button_idx];
        rects.push(vis.bounds);

        if btn.is_default {
            fill_rect(
                ctx,
                vis.bounds.x as f64,
                vis.bounds.y as f64,
                vis.bounds.width as f64,
                vis.bounds.height as f64,
                theme.selected_bg,
            );
        }

        let label = if dialog.vertical_buttons {
            let prefix = if btn.is_default { "▸ " } else { "  " };
            format!("{}{}", prefix, btn.label)
        } else {
            format!("  {}  ", btn.label)
        };
        let (lw, lh) = measure_text(font, &label);
        let label_x = if dialog.vertical_buttons {
            vis.bounds.x as f64 + 4.0
        } else {
            vis.bounds.x as f64 + (vis.bounds.width as f64 - lw) / 2.0
        };
        let label_y = vis.bounds.y as f64 + (vis.bounds.height as f64 - lh) / 2.0;
        draw_text(
            ctx,
            font,
            &label,
            label_x,
            label_y,
            color_to_cg(theme.surface_fg),
        );
    }

    rects
}

fn color_to_cg(c: Color) -> (f64, f64, f64, f64) {
    (
        c.r as f64 / 255.0,
        c.g as f64 / 255.0,
        c.b as f64 / 255.0,
        c.a as f64 / 255.0,
    )
}

unsafe fn fill_rect(ctx: CGContextRef, x: f64, y: f64, w: f64, h: f64, c: Color) {
    let (r, g, b, a) = color_to_cg(c);
    CGContextSetRGBFillColor(ctx, r, g, b, a);
    use core_graphics::geometry::{CGPoint, CGSize};
    CGContextFillRect(ctx, CGRect::new(&CGPoint::new(x, y), &CGSize::new(w, h)));
}

unsafe fn stroke_rect(
    ctx: CGContextRef,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    c: Color,
    line_width: f64,
) {
    let (r, g, b, a) = color_to_cg(c);
    CGContextSetRGBStrokeColor(ctx, r, g, b, a);
    CGContextSetLineWidth(ctx, line_width);
    use core_graphics::geometry::{CGPoint, CGSize};
    CGContextStrokeRect(ctx, CGRect::new(&CGPoint::new(x, y), &CGSize::new(w, h)));
}

extern "C" {
    fn CGContextSetRGBFillColor(
        c: CGContextRef,
        red: core_graphics::base::CGFloat,
        green: core_graphics::base::CGFloat,
        blue: core_graphics::base::CGFloat,
        alpha: core_graphics::base::CGFloat,
    );
    fn CGContextSetRGBStrokeColor(
        c: CGContextRef,
        red: core_graphics::base::CGFloat,
        green: core_graphics::base::CGFloat,
        blue: core_graphics::base::CGFloat,
        alpha: core_graphics::base::CGFloat,
    );
    fn CGContextSetLineWidth(c: CGContextRef, w: core_graphics::base::CGFloat);
    fn CGContextFillRect(c: CGContextRef, rect: CGRect);
    fn CGContextStrokeRect(c: CGContextRef, rect: CGRect);
}

#[cfg(test)]
mod tests {
    use super::super::headless::BitmapSurface;
    use super::super::text::make_font;
    use super::super::MacBackend;
    use super::*;
    use crate::event::Viewport;
    use crate::primitives::dialog::{DialogButton, DialogMeasure};
    use crate::types::WidgetId;
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
            input: None,
        }
    }

    fn layout_for(dialog: &Dialog, viewport: QRect, line_height: f32) -> DialogLayout {
        let measure = DialogMeasure {
            width: 240.0,
            title_height: line_height,
            body_height: line_height,
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
}
