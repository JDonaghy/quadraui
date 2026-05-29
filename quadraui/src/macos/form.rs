//! macOS rasteriser for [`crate::Form`].
//!
//! Mirrors [`crate::gtk::form::draw_form`] for the field kinds needed
//! by current consumers: `Label`, `Toggle`, `TextInput`, `Button`,
//! `ReadOnly`, `ToggleGroup`, `SegmentedControl`, `ButtonRow`,
//! `PasswordInput`. Per-row height is `(line_height * 1.4).round()`
//! to match GTK row pitch.
//!
//! ## Scope omissions (follow-up)
//!
//! - **Rich field kinds still missing** — `Slider`, `ColorPicker`,
//!   `Dropdown`, `TextArea`, `Number`, `DateInput`, `FilePicker`
//!   render as label-only rows for now. Layout still produces full
//!   hit regions so click routing keeps working; pretty rendering
//!   lands when a consumer needs it (parity check vs GTK before each
//!   add).
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
use crate::primitives::form::{
    FieldKind, Form, FormFieldMeasure, FormItemMeasure, FormLayout, ValidationState,
};
use crate::theme::Theme;
use crate::types::{Color, WidgetId};

/// Compute the layout the macOS rasteriser would produce for `form`
/// in `area` at `line_height`. Rows are `(line_height * 1.4).round()`
/// tall.
///
/// Per-item measurement (`ToggleGroup`, `SegmentedControl`,
/// `ButtonRow`) uses `font` so paint and hit-test agree on each
/// item's x position. Mirrors the per-`FieldKind` measurement in
/// [`crate::gtk::backend`]'s `form_layout` impl.
///
/// Coordinate frame: `visible_fields.bounds`, `item_bounds`, and
/// `hit_regions` are in **form-local** coords (origin at 0, 0),
/// matching `gtk_form_layout` and the `tree_layout` contract. Hosts
/// (e.g. `SidebarSystem`, `FormController`) subtract `area.x`/`area.y`
/// from absolute click coords before calling
/// [`FormLayout::hit_test`].
pub fn mac_form_layout(form: &Form, area: QRect, line_height: f64, font: &CTFont) -> FormLayout {
    let row_h = (line_height * 1.4).round() as f32;
    let gap = 8.0_f32;
    form.layout(area.width, area.height, |i| {
        let field = &form.fields[i];
        match &field.kind {
            FieldKind::ToggleGroup { toggles } => {
                let start_x = items_start_x(font, field);
                let items = toggles
                    .iter()
                    .map(|t| FormItemMeasure {
                        id: t.id.clone(),
                        width: measure_text(font, &t.label).0 as f32,
                    })
                    .collect();
                FormFieldMeasure::with_items(row_h, start_x, gap, items)
            }
            FieldKind::ButtonRow { buttons } => {
                let start_x = items_start_x(font, field);
                let items = buttons
                    .iter()
                    .map(|b| FormItemMeasure {
                        id: b.id.clone(),
                        width: measure_text(font, &format!("[{}]", b.label)).0 as f32,
                    })
                    .collect();
                FormFieldMeasure::with_items(row_h, start_x, gap, items)
            }
            FieldKind::SegmentedControl { options, .. } => {
                let start_x = items_start_x(font, field);
                let items = options
                    .iter()
                    .enumerate()
                    .map(|(idx, opt)| FormItemMeasure {
                        id: WidgetId::new(format!("{}__seg_{idx}", field.id.as_str())),
                        width: measure_text(font, &format!("[{opt}]")).0 as f32,
                    })
                    .collect();
                // Segments butt up against each other — no inter-item gap.
                FormFieldMeasure::with_items(row_h, start_x, 0.0, items)
            }
            FieldKind::Toolbar(toolbar) => {
                use crate::primitives::toolbar::ToolbarButton;
                let start_x = {
                    let label_text: String =
                        field.label.spans.iter().map(|s| s.text.as_str()).collect();
                    if label_text.is_empty() {
                        6.0_f32
                    } else {
                        let (lw, _) = measure_text(font, &label_text);
                        6.0 + lw as f32 + 12.0
                    }
                };
                let items = toolbar
                    .buttons
                    .iter()
                    .map(|btn| {
                        let id = match btn {
                            ToolbarButton::Action { id, .. } => id.clone(),
                            _ => field.id.clone(),
                        };
                        let width = match btn {
                            ToolbarButton::Action {
                                label,
                                icon,
                                key_hint,
                                ..
                            } => {
                                let mut text = String::new();
                                if let Some(ic) = icon {
                                    text.push_str(ic);
                                    text.push(' ');
                                }
                                text.push_str(label);
                                if let Some(hint) = key_hint {
                                    text.push_str(" (");
                                    text.push_str(hint);
                                    text.push(')');
                                }
                                (measure_text(font, &text).0 as f32) + 16.0
                            }
                            ToolbarButton::Separator => 12.0,
                            ToolbarButton::Label { text, .. } => measure_text(font, text).0 as f32,
                        };
                        FormItemMeasure { id, width }
                    })
                    .collect();
                FormFieldMeasure::with_items(row_h, start_x, 0.0, items)
            }
            _ => FormFieldMeasure::new(row_h),
        }
    })
}

/// X offset where row items (ToggleGroup / SegmentedControl /
/// ButtonRow) start: 6px row inset + label width + 12px gap.
fn items_start_x(font: &CTFont, field: &crate::primitives::form::FormField) -> f32 {
    let label_text: String = field.label.spans.iter().map(|s| s.text.as_str()).collect();
    let (label_w, _) = measure_text(font, &label_text);
    6.0 + label_w as f32 + 12.0
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
        return mac_form_layout(form, area, line_height, font);
    }

    let layout = mac_form_layout(form, area, line_height, font);

    CGContextSaveGState(ctx);
    CGContextClipToRect(ctx, CGRect::new_xywh(x, y, w, h));
    fill_rect(ctx, x, y, w, h, theme.tab_bar_bg);

    for vis in &layout.visible_fields {
        let field = &form.fields[vis.field_idx];
        // Layout returns local coords; shift to absolute for paint.
        let row_x = vis.bounds.x as f64 + x;
        let row_y = vis.bounds.y as f64 + y;
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
            FieldKind::ToggleGroup { toggles } => {
                // Layout's item_bounds are local — shift by (x, y) for paint.
                for (toggle, (_id, item_rect)) in toggles.iter().zip(&vis.item_bounds) {
                    let on = toggle.value && !field.disabled;
                    let toggle_fg = if on { theme.accent_fg } else { theme.muted_fg };
                    let ix = item_rect.x as f64 + x;
                    let iy = item_rect.y as f64 + y;
                    let iw = item_rect.width as f64;
                    let ih = item_rect.height as f64;
                    // Subtle pill-style background on the "on" state so the
                    // toggled state is visible at a glance — GTK leans on
                    // fg color alone, but macOS needs the extra contrast
                    // since text rasterisation is lighter here.
                    if on {
                        fill_rect(ctx, ix, iy + 2.0, iw, ih - 4.0, theme.selected_bg);
                    }
                    let (_, th) = measure_text(font, &toggle.label);
                    let ty = (iy + (ih - th) / 2.0).round();
                    draw_text(ctx, font, &toggle.label, ix, ty, color_to_cg(toggle_fg));
                }
            }
            FieldKind::ButtonRow { buttons } => {
                for (button, (_id, item_rect)) in buttons.iter().zip(&vis.item_bounds) {
                    let disabled = button.disabled || field.disabled;
                    let btn_fg = if disabled { theme.muted_fg } else { field_fg };
                    let brk_fg = if disabled {
                        theme.muted_fg
                    } else {
                        theme.accent_fg
                    };
                    let ix = item_rect.x as f64 + x;
                    let iy = item_rect.y as f64 + y;
                    let ih = item_rect.height as f64;
                    let (_, bh) = measure_text(font, "[");
                    let ty = (iy + (ih - bh) / 2.0).round();
                    // [
                    let (bw, _) = measure_text(font, "[");
                    draw_text(ctx, font, "[", ix, ty, color_to_cg(brk_fg));
                    let mut cur = ix + bw;
                    // Optional icon (uses ASCII fallback — nerd-font handling
                    // lives at a higher layer).
                    if let Some(ref icon) = button.icon {
                        let glyph = icon.fallback.as_str();
                        let (iw, _) = measure_text(font, glyph);
                        draw_text(ctx, font, glyph, cur, ty, color_to_cg(btn_fg));
                        cur += iw;
                        if !button.label.is_empty() {
                            cur += 4.0;
                        }
                    }
                    // label
                    draw_text(ctx, font, &button.label, cur, ty, color_to_cg(btn_fg));
                    let (lw, _) = measure_text(font, &button.label);
                    cur += lw;
                    // ]
                    draw_text(ctx, font, "]", cur, ty, color_to_cg(brk_fg));
                }
            }
            FieldKind::SegmentedControl {
                options,
                selected_idx,
            } => {
                for (idx, (opt, (_id, item_rect))) in
                    options.iter().zip(&vis.item_bounds).enumerate()
                {
                    let ix = item_rect.x as f64 + x;
                    let iy = item_rect.y as f64 + y;
                    let iw = item_rect.width as f64;
                    let ih = item_rect.height as f64;
                    let is_selected = idx == *selected_idx;
                    let opt_fg = if is_selected {
                        theme.accent_fg
                    } else {
                        theme.muted_fg
                    };
                    if is_selected {
                        fill_rect(ctx, ix, iy + 2.0, iw, ih - 4.0, theme.selected_bg);
                    }
                    let bracketed = format!("[{opt}]");
                    let (_, th) = measure_text(font, &bracketed);
                    let ty = (iy + (ih - th) / 2.0).round();
                    draw_text(ctx, font, &bracketed, ix, ty, color_to_cg(opt_fg));
                }
            }
            FieldKind::PasswordInput {
                value,
                placeholder,
                cursor,
                mask_char,
            } => {
                let masked: String = value.chars().map(|_| *mask_char).collect();
                let shown = if value.is_empty() {
                    placeholder.as_str()
                } else {
                    masked.as_str()
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

                    if let Some(cur) = cursor {
                        if is_focused {
                            // Each plaintext char maps to one mask char, so
                            // measure the masked prefix up to the byte-cursor.
                            let char_pos = value[..(*cur).min(value.len())].chars().count();
                            let mask_prefix: String = masked.chars().take(char_pos).collect();
                            let (prefix_w, _) = measure_text(font, &mask_prefix);
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
            FieldKind::Toolbar(toolbar) => {
                // Delegate to the macOS toolbar rasteriser.
                let toolbar_x = if no_label {
                    label_x
                } else {
                    label_right + 12.0
                };
                let toolbar_w = row_w - (toolbar_x - x);
                if toolbar_w > 0.0 {
                    super::toolbar::draw_toolbar(
                        ctx, font, toolbar_x, row_y, toolbar_w, row_h, toolbar, theme, None, None,
                    );
                }
            }
            // Rich field kinds still without paint paths — label-only
            // fallback. Tracked in module header.
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
    use crate::primitives::form::{ButtonRowItem, FieldKind, FormField, FormHit, ToggleGroupItem};
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
                &font(),
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

    #[test]
    fn layout_returns_local_coords_when_area_offset() {
        // Cross-backend contract: visible_fields.bounds, item_bounds,
        // and hit_regions are in form-local coords (origin 0, 0),
        // regardless of where `area` lives. Hosts (SidebarSystem,
        // FormController, AppLogic) subtract area.x/area.y from
        // absolute click coords before hit_test. Matches the
        // `gtk_form_layout` and `mac_tree_layout` contract.
        //
        // Regression for #44 macos_sidebar_search "Find click selects
        // row above": prior to the fix, mac_form_layout shifted
        // hit_regions to absolute coords, causing AppLogic that
        // localised position (per the documented contract) to hit the
        // field at `position.y - 2*area.y` instead of the one under
        // the cursor.
        let form = sample_form();
        // Area offset by (0, 40) — typical when a form lives below an
        // MSV section header.
        let area = QRect::new(0.0, 40.0, 320.0, 120.0);
        let layout = mac_form_layout(&form, area, 16.0, &font());
        // Locality: first field's bounds.y must be 0, not 40.
        let first = &layout.visible_fields[0];
        assert_eq!(
            first.bounds.y, 0.0,
            "visible_fields.bounds.y must be local (0.0), got {}",
            first.bounds.y,
        );
        // Round-trip: simulate a click at the absolute centre of each
        // painted field, localise the way the AppLogic does, and assert
        // it hits the right field. Pre-fix this returned the field N
        // positions earlier.
        for vis in &layout.visible_fields {
            let abs_x = area.x + vis.bounds.x + vis.bounds.width * 0.5;
            let abs_y = area.y + vis.bounds.y + vis.bounds.height * 0.5;
            let local_x = abs_x - area.x;
            let local_y = abs_y - area.y;
            assert_eq!(
                layout.hit_test(local_x, local_y),
                FormHit::Field(vis.id.clone()),
                "field {:?} click → wrong hit (coord-frame drift)",
                vis.id,
            );
        }
    }

    // ── #189 ToggleGroup / SegmentedControl / ButtonRow / PasswordInput ──

    fn toggle_group_field(id: &str, toggles: Vec<ToggleGroupItem>) -> FormField {
        FormField {
            id: WidgetId::new(id),
            label: StyledText::plain(""),
            kind: FieldKind::ToggleGroup { toggles },
            hint: StyledText::default(),
            disabled: false,
            validation: None,
        }
    }

    fn search_flags_form() -> Form {
        // Mirrors the `macos_sidebar_search` shape — three toggles with
        // the middle one ON, no label so items start at the row's left.
        Form {
            id: WidgetId::new("search"),
            fields: vec![toggle_group_field(
                "flags",
                vec![
                    ToggleGroupItem {
                        id: WidgetId::new("case"),
                        label: "Aa".into(),
                        value: false,
                    },
                    ToggleGroupItem {
                        id: WidgetId::new("regex"),
                        label: ".*".into(),
                        value: true,
                    },
                    ToggleGroupItem {
                        id: WidgetId::new("word"),
                        label: "W".into(),
                        value: false,
                    },
                ],
            )],
            focused_field: None,
            scroll_offset: 0,
            has_focus: false,
        }
    }

    #[test]
    fn toggle_group_layout_resolves_per_item_bounds() {
        // ToggleGroup must populate item_bounds so per-toggle clicks
        // dispatch the right `ToggleGroupItem` id rather than the
        // parent field id.
        let form = search_flags_form();
        let (_surface, layout) = paint_via_backend(&form);
        let vis = &layout.visible_fields[0];
        assert_eq!(
            vis.item_bounds.len(),
            3,
            "ToggleGroup should resolve 3 item_bounds",
        );
        let ids: Vec<&WidgetId> = vis.item_bounds.iter().map(|(id, _)| id).collect();
        assert_eq!(
            ids,
            vec![
                &WidgetId::new("case"),
                &WidgetId::new("regex"),
                &WidgetId::new("word"),
            ],
        );
    }

    #[test]
    fn toggle_group_on_item_paints_selected_bg() {
        // The "on" toggle (regex / `.*`) must paint `selected_bg`
        // behind its rect so it is visually distinguishable from
        // the off toggles at a glance.
        let form = search_flags_form();
        let (surface, layout) = paint_via_backend(&form);
        let theme = Theme::default();
        let vis = &layout.visible_fields[0];
        let (_, on_rect) = vis
            .item_bounds
            .iter()
            .find(|(id, _)| id == &WidgetId::new("regex"))
            .expect("regex toggle in layout");
        // Probe a couple of pixels INSIDE the on-toggle's vertical
        // padded band where the bg is filled (rect inset by 2 on top
        // and bottom — see ToggleGroup paint path).
        let px = (on_rect.x + on_rect.width - 1.0) as u32;
        let py = (on_rect.y + on_rect.height / 2.0) as u32;
        let (r, g, b, _) = surface.pixel(px, py);
        assert_eq!(
            (r, g, b),
            (
                theme.selected_bg.r,
                theme.selected_bg.g,
                theme.selected_bg.b,
            ),
            "on-toggle should paint selected_bg behind its rect",
        );
    }

    #[test]
    fn toggle_group_click_dispatches_individual_toggle_id() {
        // Round-trip the acceptance criteria: click at a known
        // on-toggle's centre, assert hit_test returns the toggle's
        // own WidgetId (not the parent field's id).
        let form = search_flags_form();
        let (_surface, layout) = paint_via_backend(&form);
        for (id, rect) in &layout.visible_fields[0].item_bounds {
            let cx = rect.x + rect.width * 0.5;
            let cy = rect.y + rect.height * 0.5;
            assert_eq!(
                layout.hit_test(cx, cy),
                FormHit::Field(id.clone()),
                "click on toggle {id:?} should resolve to its own id",
            );
        }
    }

    #[test]
    fn segmented_control_selected_paints_selected_bg() {
        let form = Form {
            id: WidgetId::new("seg-form"),
            fields: vec![FormField {
                id: WidgetId::new("scope"),
                label: StyledText::plain(""),
                kind: FieldKind::SegmentedControl {
                    options: vec!["File".into(), "Folder".into(), "Project".into()],
                    selected_idx: 1,
                },
                hint: StyledText::default(),
                disabled: false,
                validation: None,
            }],
            focused_field: None,
            scroll_offset: 0,
            has_focus: false,
        };
        let (surface, layout) = paint_via_backend(&form);
        let theme = Theme::default();
        let vis = &layout.visible_fields[0];
        // Synthetic per-segment ids `<field>__seg_<idx>` — assert
        // the SELECTED segment paints selected_bg.
        let (_, selected) = vis
            .item_bounds
            .iter()
            .find(|(id, _)| id.as_str() == "scope__seg_1")
            .expect("segment 1 in layout");
        let px = (selected.x + selected.width - 1.0) as u32;
        let py = (selected.y + selected.height / 2.0) as u32;
        let (r, g, b, _) = surface.pixel(px, py);
        assert_eq!(
            (r, g, b),
            (
                theme.selected_bg.r,
                theme.selected_bg.g,
                theme.selected_bg.b,
            ),
            "selected segment should paint selected_bg behind its rect",
        );
    }

    #[test]
    fn button_row_layout_resolves_per_item_bounds() {
        let form = Form {
            id: WidgetId::new("actions-form"),
            fields: vec![FormField {
                id: WidgetId::new("actions"),
                label: StyledText::plain(""),
                kind: FieldKind::ButtonRow {
                    buttons: vec![
                        ButtonRowItem {
                            id: WidgetId::new("find"),
                            label: "Find".into(),
                            disabled: false,
                            icon: None,
                        },
                        ButtonRowItem {
                            id: WidgetId::new("replace"),
                            label: "Replace".into(),
                            disabled: false,
                            icon: None,
                        },
                    ],
                },
                hint: StyledText::default(),
                disabled: false,
                validation: None,
            }],
            focused_field: None,
            scroll_offset: 0,
            has_focus: false,
        };
        let (_surface, layout) = paint_via_backend(&form);
        let vis = &layout.visible_fields[0];
        assert_eq!(vis.item_bounds.len(), 2);
        // Each item rect should be wide enough to span the bracketed
        // label (e.g. "[Find]" is 6 monospace chars wide).
        for (id, rect) in &vis.item_bounds {
            assert!(
                rect.width >= 6.0 * 4.0,
                "button {id:?} rect width {} too narrow",
                rect.width,
            );
            let hit = layout.hit_test(rect.x + 1.0, rect.y + rect.height * 0.5);
            assert_eq!(
                hit,
                FormHit::Field(id.clone()),
                "click at button {id:?} should hit its own id",
            );
        }
    }

    #[test]
    fn password_input_paints_with_mask_char() {
        // PasswordInput should render `mask_char` repeated `value.chars().count()`
        // times, NOT the plaintext. Verify by computing the expected
        // masked-text width and asserting paint geometry agrees.
        let form = Form {
            id: WidgetId::new("auth-form"),
            fields: vec![FormField {
                id: WidgetId::new("pw"),
                label: StyledText::plain(""),
                kind: FieldKind::PasswordInput {
                    value: "hunter2".into(),
                    placeholder: String::new(),
                    cursor: Some(7),
                    mask_char: '•',
                },
                hint: StyledText::default(),
                disabled: false,
                validation: None,
            }],
            focused_field: Some(WidgetId::new("pw")),
            scroll_offset: 0,
            has_focus: true,
        };
        let (surface, layout) = paint_via_backend(&form);
        // Locate the field bounds.
        let vis = &layout.visible_fields[0];
        // Probe inside the field row at the very centre Y. Pre-fix
        // this pixel would have been background (label-only fallback);
        // post-fix the row contains painted `[`, masked text, `]`.
        // Cheap shape-check: at least ONE pixel inside the row should
        // be foreground (mask glyphs) rather than the focused-row bg.
        let theme = Theme::default();
        let mut saw_glyph = false;
        let y = (vis.bounds.y + vis.bounds.height / 2.0) as u32;
        for x in (vis.bounds.x as u32)..(vis.bounds.x + vis.bounds.width) as u32 {
            let (r, g, b, _) = surface.pixel(x, y);
            // Foreground-ish pixel: differs from both selected_bg (focused row)
            // and pure transparent surface clear.
            let is_bg = (r, g, b)
                == (
                    theme.selected_bg.r,
                    theme.selected_bg.g,
                    theme.selected_bg.b,
                );
            if !is_bg && !(r == 0 && g == 0 && b == 0) {
                saw_glyph = true;
                break;
            }
        }
        assert!(
            saw_glyph,
            "PasswordInput row should paint masked glyphs (none found)",
        );
    }
}
