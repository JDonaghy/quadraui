//! macOS rasteriser for [`crate::Form`].
//!
//! Mirrors [`crate::gtk::form::draw_form`] for the basic field kinds
//! needed by current consumers: `Label`, `Toggle`, `TextInput`,
//! `Button`, `ReadOnly`. Per-row height is `(line_height * 1.4).round()`
//! to match GTK row pitch.
//!
//! ## Scope omissions (follow-up)
//!
//! - **Rich field kinds** — `Slider`, `ColorPicker`, `Dropdown`,
//!   `SegmentedControl`, `ToggleGroup`, `ButtonRow`, `TextArea`,
//!   `Password`, `Number`, `DateInput`, `FilePicker` render as
//!   label-only rows for now. Layout still produces full hit regions
//!   so click routing keeps working; pretty rendering lands when a
//!   consumer needs it (parity check vs GTK before each add).
//! - **Selection highlight** inside `TextInput` — deferred with the
//!   unified text-attribute pass that also unlocks editor selection.
//! - **Validation indicators** — `ValidationState` is honoured for
//!   colour (error_fg / warning_fg on the field label) but the hint
//!   strip below the row is not yet rendered.

use core_graphics::geometry::CGRect;
use core_graphics::sys::CGContextRef;
use core_text::font::CTFont;

use super::text::{draw_text, measure_text};
use crate::event::Rect as QRect;
use crate::primitives::form::{FieldKind, Form, FormFieldMeasure, FormLayout, ValidationState};
use crate::theme::Theme;
use crate::types::Color;

/// Compute the layout the macOS rasteriser would produce for `form`
/// in `area` at `line_height`. Uniform `(line_height * 1.4).round()`
/// rows.
pub fn mac_form_layout(form: &Form, area: QRect, line_height: f64) -> FormLayout {
    let row_h = (line_height * 1.4).round() as f32;
    let mut layout = form.layout(area.width, area.height, |_| FormFieldMeasure::new(row_h));
    let (dx, dy) = (area.x, area.y);
    if dx != 0.0 || dy != 0.0 {
        for vf in &mut layout.visible_fields {
            vf.bounds.x += dx;
            vf.bounds.y += dy;
            for (_, ib) in &mut vf.item_bounds {
                ib.x += dx;
                ib.y += dy;
            }
        }
        for (rect, _) in &mut layout.hit_regions {
            rect.x += dx;
            rect.y += dy;
        }
    }
    layout
}

/// Draw a [`Form`] into `(x, y, w, h)` on `ctx`. Returns the same
/// layout `mac_form_layout` would produce.
///
/// # Safety
///
/// `ctx` must be a valid `CGContextRef` borrowed for the duration of
/// the call.
#[allow(clippy::too_many_arguments)]
pub unsafe fn draw_form(
    ctx: CGContextRef,
    font: &CTFont,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    form: &Form,
    theme: &Theme,
    line_height: f64,
) -> FormLayout {
    let area = QRect::new(x as f32, y as f32, w as f32, h as f32);
    if w <= 0.0 || h <= 0.0 {
        return mac_form_layout(form, area, line_height);
    }

    let layout = mac_form_layout(form, area, line_height);

    CGContextSaveGState(ctx);
    CGContextClipToRect(ctx, CGRect::new_xywh(x, y, w, h));
    fill_rect(ctx, x, y, w, h, theme.tab_bar_bg);

    for vis in &layout.visible_fields {
        let field = &form.fields[vis.field_idx];
        let row_x = vis.bounds.x as f64;
        let row_y = vis.bounds.y as f64;
        let row_w = vis.bounds.width as f64;
        let row_h = vis.bounds.height as f64;

        let is_focused = form.has_focus
            && form
                .focused_field
                .as_ref()
                .is_some_and(|id| id == &field.id);
        let is_header = matches!(field.kind, FieldKind::Label);

        let (default_fg, row_bg) = if is_focused {
            (theme.foreground, theme.selected_bg)
        } else if is_header {
            (theme.header_fg, theme.header_bg)
        } else {
            (theme.foreground, theme.tab_bar_bg)
        };

        fill_rect(ctx, row_x, row_y, row_w, row_h, row_bg);

        let validation_fg = field.validation.as_ref().map(|v| match v {
            ValidationState::Error(_) => theme.error_fg,
            ValidationState::Warning(_) => theme.warning_fg,
        });
        let field_fg = if field.disabled {
            theme.muted_fg
        } else {
            validation_fg.unwrap_or(default_fg)
        };

        let label_text: String = field.label.spans.iter().map(|s| s.text.as_str()).collect();
        let (label_w, label_h) = measure_text(font, &label_text);
        let label_x = row_x + 6.0;
        let text_y = (row_y + (row_h - label_h) / 2.0).round();
        draw_text(
            ctx,
            font,
            &label_text,
            label_x,
            text_y,
            color_to_cg(field_fg),
        );
        let label_right = label_x + label_w;
        let no_label = label_text.is_empty();

        let input_right = row_x + row_w - 8.0;
        match &field.kind {
            FieldKind::Label => {}
            FieldKind::Toggle { value } => {
                let glyph = if *value { "[x]" } else { "[ ]" };
                let fg_color = if *value && !field.disabled {
                    theme.accent_fg
                } else {
                    field_fg
                };
                let (iw, _) = measure_text(font, glyph);
                let ix = if no_label { label_x } else { input_right - iw };
                if no_label || ix > label_right + 8.0 {
                    draw_text(ctx, font, glyph, ix, text_y, color_to_cg(fg_color));
                }
            }
            FieldKind::TextInput {
                value,
                placeholder,
                cursor,
                selection_anchor: _,
            } => {
                let shown = if value.is_empty() {
                    placeholder.as_str()
                } else {
                    value.as_str()
                };
                let input_fg = if value.is_empty() {
                    theme.muted_fg
                } else {
                    field_fg
                };
                let (shown_w, _) = measure_text(font, shown);

                let (ix, bracket_right) = if no_label {
                    (label_x, input_right - 4.0)
                } else {
                    let max_width = (row_w * 0.6).max(80.0);
                    let dw = shown_w.min(max_width);
                    let ix = input_right - dw - 14.0;
                    (ix, ix + 8.0 + dw + 2.0)
                };
                if no_label || ix > label_right + 8.0 {
                    draw_text(ctx, font, "[", ix, text_y, color_to_cg(theme.muted_fg));
                    draw_text(ctx, font, shown, ix + 8.0, text_y, color_to_cg(input_fg));
                    draw_text(
                        ctx,
                        font,
                        "]",
                        bracket_right,
                        text_y,
                        color_to_cg(theme.muted_fg),
                    );

                    // Caret — thin 1.5pt bar at the cursor's byte offset.
                    if let Some(cur) = cursor {
                        if is_focused {
                            let prefix_byte = (*cur).min(shown.len());
                            let prefix = &shown[..prefix_byte];
                            let (prefix_w, _) = measure_text(font, prefix);
                            let caret_x = ix + 8.0 + prefix_w;
                            fill_rect(
                                ctx,
                                caret_x,
                                row_y + 3.0,
                                1.5,
                                row_h - 6.0,
                                theme.foreground,
                            );
                        }
                    }
                }
            }
            FieldKind::Button => {
                // Label already painted above; nothing further to draw.
            }
            FieldKind::ReadOnly { value } => {
                let value_text: String = value.spans.iter().map(|s| s.text.as_str()).collect();
                let (vw, _) = measure_text(font, &value_text);
                let vx = input_right - vw;
                if vx > label_right + 8.0 {
                    draw_text(
                        ctx,
                        font,
                        &value_text,
                        vx,
                        text_y,
                        color_to_cg(theme.muted_fg),
                    );
                }
            }
            // Rich field kinds — label-only fallback. Tracked in module
            // header.
            _ => {}
        }
    }

    CGContextRestoreGState(ctx);
    layout
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
    CGContextFillRect(ctx, CGRect::new_xywh(x, y, w, h));
}

trait CGRectExt {
    fn new_xywh(x: f64, y: f64, w: f64, h: f64) -> Self;
}
impl CGRectExt for CGRect {
    fn new_xywh(x: f64, y: f64, w: f64, h: f64) -> Self {
        use core_graphics::geometry::{CGPoint, CGSize};
        CGRect::new(&CGPoint::new(x, y), &CGSize::new(w, h))
    }
}

extern "C" {
    fn CGContextSaveGState(c: CGContextRef);
    fn CGContextRestoreGState(c: CGContextRef);
    fn CGContextClipToRect(c: CGContextRef, rect: CGRect);
    fn CGContextSetRGBFillColor(
        c: CGContextRef,
        red: core_graphics::base::CGFloat,
        green: core_graphics::base::CGFloat,
        blue: core_graphics::base::CGFloat,
        alpha: core_graphics::base::CGFloat,
    );
    fn CGContextFillRect(c: CGContextRef, rect: CGRect);
}

#[cfg(test)]
mod tests {
    use super::super::headless::BitmapSurface;
    use super::super::text::make_font;
    use super::super::MacBackend;
    use super::*;
    use crate::event::Viewport;
    use crate::primitives::form::{FieldKind, FormField, FormHit};
    use crate::theme::Theme;
    use crate::types::{StyledText, WidgetId};
    use crate::Backend;

    const W: u32 = 320;
    const H: u32 = 160;

    fn font() -> CTFont {
        make_font("Menlo", 14.0).expect("Menlo installed")
    }

    fn label(id: &str, text: &str) -> FormField {
        FormField {
            id: WidgetId::new(id),
            label: StyledText::plain(text),
            kind: FieldKind::Label,
            hint: StyledText::default(),
            disabled: false,
            validation: None,
        }
    }

    fn text_input(id: &str, label_text: &str, value: &str) -> FormField {
        FormField {
            id: WidgetId::new(id),
            label: StyledText::plain(label_text),
            kind: FieldKind::TextInput {
                value: value.into(),
                placeholder: String::new(),
                cursor: Some(value.len()),
                selection_anchor: None,
            },
            hint: StyledText::default(),
            disabled: false,
            validation: None,
        }
    }

    fn toggle(id: &str, label_text: &str, value: bool) -> FormField {
        FormField {
            id: WidgetId::new(id),
            label: StyledText::plain(label_text),
            kind: FieldKind::Toggle { value },
            hint: StyledText::default(),
            disabled: false,
            validation: None,
        }
    }

    fn sample_form() -> Form {
        Form {
            id: WidgetId::new("settings"),
            fields: vec![
                label("hdr", "General"),
                text_input("name", "Name", "alice"),
                toggle("enabled", "Enabled", true),
            ],
            focused_field: Some(WidgetId::new("name")),
            scroll_offset: 0,
            has_focus: true,
        }
    }

    fn paint_via_backend(form: &Form) -> (BitmapSurface, FormLayout) {
        let surface = BitmapSurface::new(W, H);
        surface.fill(0.0, 0.0, 0.0, 0.0);
        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        let layout = std::cell::RefCell::new(None);
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            b.draw_form(QRect::new(0.0, 0.0, W as f32, H as f32), form);
            let l = super::mac_form_layout(
                form,
                QRect::new(0.0, 0.0, W as f32, H as f32),
                b.line_height() as f64,
            );
            *layout.borrow_mut() = Some(l);
        });
        backend.end_frame();
        (surface, layout.into_inner().unwrap())
    }

    #[test]
    fn focused_field_paints_selected_bg() {
        let form = sample_form();
        let (surface, layout) = paint_via_backend(&form);
        let theme = Theme::default();
        let name = layout
            .visible_fields
            .iter()
            .find(|v| v.id == WidgetId::new("name"))
            .expect("name field visible");
        // Probe near right edge to avoid the label glyph "Name".
        let px = (name.bounds.x + name.bounds.width - 4.0) as u32;
        let py = (name.bounds.y + name.bounds.height / 2.0) as u32;
        let (r, g, b, _) = surface.pixel(px, py);
        // Focused field row gets selected_bg.
        assert_eq!(
            (r, g, b),
            (
                theme.selected_bg.r,
                theme.selected_bg.g,
                theme.selected_bg.b
            ),
            "focused TextInput row should paint selected_bg",
        );
    }

    #[test]
    fn header_field_paints_header_bg() {
        let form = sample_form();
        let (surface, layout) = paint_via_backend(&form);
        let theme = Theme::default();
        let hdr = layout
            .visible_fields
            .iter()
            .find(|v| v.id == WidgetId::new("hdr"))
            .expect("header visible");
        let px = (hdr.bounds.x + hdr.bounds.width - 4.0) as u32;
        let py = (hdr.bounds.y + hdr.bounds.height / 2.0) as u32;
        let (r, g, b, _) = surface.pixel(px, py);
        assert_eq!(
            (r, g, b),
            (theme.header_bg.r, theme.header_bg.g, theme.header_bg.b),
        );
    }

    #[test]
    fn hit_test_resolves_field_at_painted_centre() {
        let form = sample_form();
        let (_surface, layout) = paint_via_backend(&form);
        for vis in &layout.visible_fields {
            let cx = vis.bounds.x + vis.bounds.width * 0.5;
            let cy = vis.bounds.y + vis.bounds.height * 0.5;
            assert_eq!(
                layout.hit_test(cx, cy),
                FormHit::Field(vis.id.clone()),
                "field {:?} hit-test",
                vis.id,
            );
        }
    }
}
