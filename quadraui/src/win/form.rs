//! Direct2D / DirectWrite rasteriser for [`crate::Form`] (issue #26).
//!
//! Unlike `gtk::form` (which paints from an ad-hoc running `y_off` /
//! `ix` cursor, independent of the D6 [`Form::layout`] call used only
//! for hit-testing), this rasteriser computes one [`crate::FormLayout`]
//! via [`win_form_layout`] and paints *from* its `visible_fields` /
//! `item_bounds` — the same "one layout, paint and hit-test both
//! consume it" contract `win::tab_bar` / `win::status_bar` established.
//! `ToggleGroup` / `ButtonRow` / `SegmentedControl` / an embedded
//! [`crate::primitives::toolbar::Toolbar`] all paint at their resolved
//! `item_bounds` rather than a hand-tracked running x.
//!
//! Per-field row height is `(line_height * 1.4).round()` — the
//! established GTK/TreeView/ListView convention.
//!
//! Only compiled on `target_os = "windows"` — see `super::mod`'s
//! `#[cfg(target_os = "windows")] mod form;` and `backend.rs`'s module
//! docs. See `win::status_bar`'s module doc for why colours come from
//! `Theme::default()` rather than a live `WinBackend` theme field.
//!
//! # Scope for #26
//!
//! `SegmentedControl` paints each option as plain highlighted text
//! (no bracket/pipe punctuation) rather than GTK/TUI's `[a|b|c]`
//! rendering — a deliberate simplification so paint and hit-test share
//! one measurer without splitting a shared bracket run's width across
//! items that don't own it. An embedded [`crate::primitives::toolbar::Toolbar`]
//! field paints each button as plain text (no full toolbar chrome —
//! `win::toolbar` doesn't exist yet); `Slider` / `ColorPicker` get a
//! plain track / swatch, not GTK's (currently blank) treatment.

use windows::Win32::Graphics::Direct2D::ID2D1RenderTarget;

use super::text::{fill_rect, DWrite};
use crate::event::Rect;
use crate::primitives::form::{
    FieldKind, Form, FormFieldMeasure, FormItemMeasure, FormLayout, ValidationState,
};
use crate::primitives::toolbar::ToolbarButton;
use crate::theme::Theme;
use crate::types::WidgetId;

fn plain(text: &crate::types::StyledText) -> String {
    text.spans.iter().map(|s| s.text.as_str()).collect()
}

fn row_height(line_height: f32) -> f32 {
    (line_height * 1.4).round()
}

fn toolbar_item(field_id: &WidgetId, btn: &ToolbarButton) -> Option<(WidgetId, String)> {
    match btn {
        ToolbarButton::Action { id, label, .. } => Some((id.clone(), label.clone())),
        ToolbarButton::Label { text, .. } => Some((field_id.clone(), text.clone())),
        ToolbarButton::Separator => None,
    }
}

/// Compute a [`Form`]'s layout without painting — the DirectWrite twin
/// of [`draw_form`]'s internal layout call.
pub fn win_form_layout(dwrite: &DWrite, rect: Rect, form: &Form, line_height: f32) -> FormLayout {
    let row_h = row_height(line_height);
    form.layout(rect.width, rect.height, |i| {
        let field = &form.fields[i];
        let label_w = dwrite
            .measure_text(&plain(&field.label))
            .map(|(w, _)| w)
            .unwrap_or(0.0);
        let group_start_x = if label_w > 0.0 {
            6.0 + label_w + 12.0
        } else {
            6.0
        };

        match &field.kind {
            FieldKind::ToggleGroup { toggles } => {
                let items = toggles
                    .iter()
                    .map(|t| FormItemMeasure {
                        id: t.id.clone(),
                        width: dwrite.measure_text(&t.label).map(|(w, _)| w).unwrap_or(0.0),
                    })
                    .collect();
                FormFieldMeasure::with_items(row_h, group_start_x, 12.0, items)
            }
            FieldKind::ButtonRow { buttons } => {
                let items = buttons
                    .iter()
                    .map(|b| {
                        let icon_w = b
                            .icon
                            .as_ref()
                            .map(|i| {
                                dwrite
                                    .measure_text(&i.fallback)
                                    .map(|(w, _)| w)
                                    .unwrap_or(0.0)
                                    + 4.0
                            })
                            .unwrap_or(0.0);
                        let label_w = dwrite.measure_text(&b.label).map(|(w, _)| w).unwrap_or(0.0);
                        FormItemMeasure {
                            id: b.id.clone(),
                            width: icon_w + label_w + 16.0,
                        }
                    })
                    .collect();
                FormFieldMeasure::with_items(row_h, group_start_x, 8.0, items)
            }
            FieldKind::SegmentedControl { options, .. } => {
                let items = options
                    .iter()
                    .enumerate()
                    .map(|(idx, opt)| FormItemMeasure {
                        id: WidgetId::new(format!("{}__seg_{idx}", field.id.as_str())),
                        width: dwrite.measure_text(opt).map(|(w, _)| w).unwrap_or(0.0) + 16.0,
                    })
                    .collect();
                FormFieldMeasure::with_items(row_h, group_start_x, 4.0, items)
            }
            FieldKind::TextArea { visible_rows, .. } => {
                FormFieldMeasure::new(row_h * *visible_rows as f32)
            }
            FieldKind::Toolbar(toolbar) => {
                let items = toolbar
                    .buttons
                    .iter()
                    .filter_map(|btn| toolbar_item(&field.id, btn))
                    .map(|(id, text)| FormItemMeasure {
                        id,
                        width: dwrite.measure_text(&text).map(|(w, _)| w).unwrap_or(0.0) + 16.0,
                    })
                    .collect();
                FormFieldMeasure::with_items(row_h, group_start_x, 8.0, items)
            }
            _ => FormFieldMeasure::new(row_h),
        }
    })
}

/// Draw a [`Form`] into `rect` (DIPs) on `target`. Returns the resolved
/// [`FormLayout`] for host click dispatch.
///
/// # Visual contract
///
/// - **Background:** `Theme::tab_bar_bg`.
/// - **Focused row:** `Theme::selected_bg`.
/// - **`Label` field:** `Theme::header_bg` / `header_fg`.
/// - **Disabled field:** `muted_fg` text.
/// - **`TextInput` / `PasswordInput` / `TextArea`:** bracketed
///   `[value]`, with a `selection_bg` highlight and an `accent_fg`
///   caret when the field carries a cursor.
/// - **Validation:** a small `error_fg` / `warning_fg` square at the
///   row's left edge, plus the message text in the same colour.
pub fn draw_form(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    rect: Rect,
    form: &Form,
    line_height: f32,
) -> FormLayout {
    let theme = Theme::default();
    let _ = fill_rect(target, rect, theme.tab_bar_bg);

    let layout = win_form_layout(dwrite, rect, form, line_height);

    for vf in &layout.visible_fields {
        let field = &form.fields[vf.field_idx];
        let row_rect = Rect::new(
            rect.x + vf.bounds.x,
            rect.y + vf.bounds.y,
            vf.bounds.width,
            vf.bounds.height,
        );
        let row_h = row_rect.height;

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
        let _ = fill_rect(target, row_rect, row_bg);

        let field_fg = if field.disabled {
            theme.muted_fg
        } else {
            default_fg
        };

        let label_text = plain(&field.label);
        let (label_w, label_h) = dwrite.measure_text(&label_text).unwrap_or((0.0, 0.0));
        let label_x = row_rect.x + 6.0;
        let label_y = row_rect.y + (row_h - label_h) / 2.0;
        let _ = dwrite.draw_text(
            target,
            &label_text,
            Rect::new(label_x, label_y, label_w, label_h),
            field_fg,
        );
        let label_right = label_x + label_w;
        let no_label = label_text.is_empty();
        let input_right = row_rect.x + row_rect.width - 8.0;

        match &field.kind {
            FieldKind::Label => {}
            FieldKind::Toggle { value } => {
                let glyph = if *value { "[x]" } else { "[ ]" };
                let fg = if *value && !field.disabled {
                    theme.accent_fg
                } else {
                    field_fg
                };
                let (w, h) = dwrite.measure_text(glyph).unwrap_or((0.0, 0.0));
                let ix = if no_label { label_x } else { input_right - w };
                if no_label || ix > label_right + 8.0 {
                    let iy = row_rect.y + (row_h - h) / 2.0;
                    let _ = dwrite.draw_text(target, glyph, Rect::new(ix, iy, w, h), fg);
                }
            }
            FieldKind::TextInput {
                value,
                placeholder,
                cursor,
                selection_anchor,
            } => {
                draw_bracketed_text(
                    target,
                    dwrite,
                    row_rect,
                    label_right,
                    no_label,
                    input_right,
                    value,
                    placeholder,
                    *cursor,
                    *selection_anchor,
                    field_fg,
                    theme.muted_fg,
                    theme.selection_bg,
                    theme.accent_fg,
                    false,
                    '\0',
                );
            }
            FieldKind::PasswordInput {
                value,
                placeholder,
                cursor,
                mask_char,
            } => {
                draw_bracketed_text(
                    target,
                    dwrite,
                    row_rect,
                    label_right,
                    no_label,
                    input_right,
                    value,
                    placeholder,
                    *cursor,
                    None,
                    field_fg,
                    theme.muted_fg,
                    theme.selection_bg,
                    theme.accent_fg,
                    true,
                    *mask_char,
                );
            }
            FieldKind::TextArea {
                value,
                placeholder,
                cursor,
                ..
            } => {
                let first_line = value.lines().next().unwrap_or("");
                draw_bracketed_text(
                    target,
                    dwrite,
                    row_rect,
                    label_right,
                    no_label,
                    input_right,
                    first_line,
                    placeholder,
                    *cursor,
                    None,
                    field_fg,
                    theme.muted_fg,
                    theme.selection_bg,
                    theme.accent_fg,
                    false,
                    '\0',
                );
            }
            FieldKind::Button => {
                let cap = label_text.clone();
                let total_w = label_w + 24.0;
                let ix = if no_label {
                    label_x
                } else {
                    input_right - total_w
                };
                if no_label || ix > row_rect.x + 8.0 {
                    let brk = if is_focused {
                        theme.accent_fg
                    } else {
                        theme.muted_fg
                    };
                    let (_, h) = dwrite.measure_text("<").unwrap_or((0.0, label_h));
                    let y = row_rect.y + (row_h - h) / 2.0;
                    let _ = dwrite.draw_text(target, "<", Rect::new(ix, y, 12.0, h), brk);
                    let text_fg = if field.disabled {
                        theme.muted_fg
                    } else {
                        field_fg
                    };
                    let _ = dwrite.draw_text(
                        target,
                        &cap,
                        Rect::new(ix + 12.0, y, label_w, h),
                        text_fg,
                    );
                    let _ = dwrite.draw_text(
                        target,
                        ">",
                        Rect::new(ix + 12.0 + label_w + 4.0, y, 12.0, h),
                        brk,
                    );
                }
            }
            FieldKind::ReadOnly { value } => {
                let text = plain(value);
                let (w, h) = dwrite.measure_text(&text).unwrap_or((0.0, 0.0));
                let ix = if no_label { label_x } else { input_right - w };
                if no_label || ix > label_right + 8.0 {
                    let iy = row_rect.y + (row_h - h) / 2.0;
                    let _ =
                        dwrite.draw_text(target, &text, Rect::new(ix, iy, w, h), theme.muted_fg);
                }
            }
            FieldKind::Slider {
                value, min, max, ..
            } => {
                let track_w = 80.0_f32;
                let range = (max - min).max(f32::EPSILON);
                let frac = ((value - min) / range).clamp(0.0, 1.0);
                let value_str = format!("{value:.2}");
                let (value_w, _) = dwrite.measure_text(&value_str).unwrap_or((0.0, 0.0));
                let total = track_w + 8.0 + value_w;
                let ix = input_right - total;
                if ix > label_right + 8.0 {
                    let track_y = row_rect.y + row_h / 2.0 - 2.0;
                    let _ = fill_rect(target, Rect::new(ix, track_y, track_w, 4.0), theme.muted_fg);
                    let _ = fill_rect(
                        target,
                        Rect::new(ix, track_y, track_w * frac, 4.0),
                        theme.accent_fg,
                    );
                    let (_, vh) = dwrite.measure_text(&value_str).unwrap_or((0.0, 0.0));
                    let vy = row_rect.y + (row_h - vh) / 2.0;
                    let _ = dwrite.draw_text(
                        target,
                        &value_str,
                        Rect::new(ix + track_w + 8.0, vy, value_w, vh),
                        field_fg,
                    );
                }
            }
            FieldKind::ColorPicker { value } => {
                let hex = format!("#{:02x}{:02x}{:02x}", value.r, value.g, value.b);
                let (hw, hh) = dwrite.measure_text(&hex).unwrap_or((0.0, 0.0));
                let swatch = 12.0_f32;
                let total = swatch + 6.0 + hw;
                let ix = input_right - total;
                if ix > label_right + 8.0 {
                    let sy = row_rect.y + (row_h - swatch) / 2.0;
                    let _ = fill_rect(target, Rect::new(ix, sy, swatch, swatch), *value);
                    let ty = row_rect.y + (row_h - hh) / 2.0;
                    let _ = dwrite.draw_text(
                        target,
                        &hex,
                        Rect::new(ix + swatch + 6.0, ty, hw, hh),
                        field_fg,
                    );
                }
            }
            FieldKind::Dropdown {
                options,
                selected_idx,
            } => {
                let chosen = options.get(*selected_idx).map(plain).unwrap_or_default();
                let (cw, ch) = dwrite.measure_text(&chosen).unwrap_or((0.0, 0.0));
                let (chev_w, _) = dwrite.measure_text("\u{25BE}").unwrap_or((10.0, ch));
                let total = cw + 4.0 + chev_w;
                let ix = input_right - total;
                if ix > label_right + 8.0 {
                    let ty = row_rect.y + (row_h - ch) / 2.0;
                    let _ = dwrite.draw_text(target, &chosen, Rect::new(ix, ty, cw, ch), field_fg);
                    let _ = dwrite.draw_text(
                        target,
                        "\u{25BE}",
                        Rect::new(ix + cw + 4.0, ty, chev_w, ch),
                        theme.muted_fg,
                    );
                }
            }
            FieldKind::ToggleGroup { toggles } => {
                for (item_id, item_rect) in &vf.item_bounds {
                    if let Some(t) = toggles.iter().find(|t| &t.id == item_id) {
                        let fg = if t.value && !field.disabled {
                            theme.accent_fg
                        } else {
                            theme.muted_fg
                        };
                        let r = shift(row_rect, item_rect);
                        let _ = dwrite.draw_text(target, &t.label, r, fg);
                    }
                }
            }
            FieldKind::ButtonRow { buttons } => {
                for (item_id, item_rect) in &vf.item_bounds {
                    if let Some(b) = buttons.iter().find(|b| &b.id == item_id) {
                        let r = shift(row_rect, item_rect);
                        let disabled = b.disabled || field.disabled;
                        let bg = if disabled { row_bg } else { theme.hover_bg };
                        let _ = fill_rect(target, r, bg);
                        let fg = if disabled { theme.muted_fg } else { field_fg };
                        let mut tx = r.x + 4.0;
                        if let Some(ref icon) = b.icon {
                            let (iw, ih) =
                                dwrite.measure_text(&icon.fallback).unwrap_or((0.0, 0.0));
                            let iy = r.y + (r.height - ih) / 2.0;
                            let _ = dwrite.draw_text(
                                target,
                                &icon.fallback,
                                Rect::new(tx, iy, iw, ih),
                                fg,
                            );
                            tx += iw + 4.0;
                        }
                        let (lw, lh) = dwrite.measure_text(&b.label).unwrap_or((0.0, 0.0));
                        let ly = r.y + (r.height - lh) / 2.0;
                        let _ = dwrite.draw_text(target, &b.label, Rect::new(tx, ly, lw, lh), fg);
                    }
                }
            }
            FieldKind::SegmentedControl {
                options,
                selected_idx,
            } => {
                for (i, (_item_id, item_rect)) in vf.item_bounds.iter().enumerate() {
                    let opt = options.get(i).map(|s| s.as_str()).unwrap_or("");
                    let fg = if i == *selected_idx {
                        theme.accent_fg
                    } else {
                        theme.muted_fg
                    };
                    let r = shift(row_rect, item_rect);
                    if i == *selected_idx {
                        let _ = fill_rect(target, r, theme.hover_bg);
                    }
                    let (tw, th) = dwrite.measure_text(opt).unwrap_or((0.0, 0.0));
                    let ty = r.y + (r.height - th) / 2.0;
                    let _ = dwrite.draw_text(target, opt, Rect::new(r.x, ty, tw, th), fg);
                }
            }
            FieldKind::Toolbar(toolbar) => {
                for (item_id, item_rect) in &vf.item_bounds {
                    let btn = toolbar.buttons.iter().find_map(|b| {
                        toolbar_item(&field.id, b)
                            .filter(|(id, _)| id == item_id)
                            .map(|_| b)
                    });
                    if let Some(ToolbarButton::Action { label, enabled, .. }) = btn {
                        let fg = if *enabled { field_fg } else { theme.muted_fg };
                        let r = shift(row_rect, item_rect);
                        let (tw, th) = dwrite.measure_text(label).unwrap_or((0.0, 0.0));
                        let ty = r.y + (r.height - th) / 2.0;
                        let _ = dwrite.draw_text(target, label, Rect::new(r.x, ty, tw, th), fg);
                    } else if let Some(ToolbarButton::Label { text, fg }) = btn {
                        let color = fg.unwrap_or(field_fg);
                        let r = shift(row_rect, item_rect);
                        let (tw, th) = dwrite.measure_text(text).unwrap_or((0.0, 0.0));
                        let ty = r.y + (r.height - th) / 2.0;
                        let _ = dwrite.draw_text(target, text, Rect::new(r.x, ty, tw, th), color);
                    }
                }
            }
        }

        if let Some(ref vs) = field.validation {
            let (color, msg) = match vs {
                ValidationState::Error(m) => (theme.error_fg, m.as_str()),
                ValidationState::Warning(m) => (theme.warning_fg, m.as_str()),
            };
            let _ = fill_rect(
                target,
                Rect::new(row_rect.x + 2.0, row_rect.y + (row_h - 3.0) / 2.0, 3.0, 3.0),
                color,
            );
            if !msg.is_empty() {
                let (mw, mh) = dwrite.measure_text(msg).unwrap_or((0.0, 0.0));
                let my = row_rect.y + (row_h - mh) / 2.0;
                let _ =
                    dwrite.draw_text(target, msg, Rect::new(row_rect.x + 8.0, my, mw, mh), color);
            }
        }
    }

    layout
}

/// Shift an item-local `Rect` (relative to the field row's own bounds)
/// into surface-absolute coordinates via `row_rect`'s origin.
fn shift(row_rect: Rect, item_rect: &Rect) -> Rect {
    Rect::new(
        row_rect.x + item_rect.x,
        row_rect.y + item_rect.y,
        item_rect.width,
        item_rect.height,
    )
}

/// Shared bracketed `[value]` painter for `TextInput` / `PasswordInput`
/// / the single-line `TextArea` preview. `mask` replaces every
/// character with `mask_char` when `masked` is true.
#[allow(clippy::too_many_arguments)]
fn draw_bracketed_text(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    row_rect: Rect,
    label_right: f32,
    no_label: bool,
    input_right: f32,
    value: &str,
    placeholder: &str,
    cursor: Option<usize>,
    selection_anchor: Option<usize>,
    field_fg: crate::Color,
    dim_fg: crate::Color,
    sel_bg: crate::Color,
    accent_fg: crate::Color,
    masked: bool,
    mask_char: char,
) {
    let masked_value: String;
    let shown: &str = if value.is_empty() {
        placeholder
    } else if masked {
        masked_value = value.chars().map(|_| mask_char).collect();
        &masked_value
    } else {
        value
    };
    let input_fg = if value.is_empty() { dim_fg } else { field_fg };
    let row_h = row_rect.height;
    let (shown_w, shown_h) = dwrite.measure_text(shown).unwrap_or((0.0, 0.0));

    let (ix, dw, bracket_right) = if no_label {
        let ix = row_rect.x + 6.0;
        let bracket_r = input_right - 4.0;
        let avail = (bracket_r - ix - 8.0).max(0.0);
        (ix, shown_w.min(avail), bracket_r)
    } else {
        let max_width = (row_rect.width * 0.6).max(80.0);
        let dw = shown_w.min(max_width);
        let ix = input_right - dw - 14.0;
        (ix, dw, ix + 8.0 + dw + 2.0)
    };
    let _ = dw;
    if !(no_label || ix > label_right + 8.0) {
        return;
    }

    let y = row_rect.y + (row_h - shown_h) / 2.0;
    let _ = dwrite.draw_text(target, "[", Rect::new(ix, y, 8.0, shown_h), dim_fg);

    let has_sel = !masked
        && matches!((cursor, selection_anchor), (Some(c), Some(a)) if c != a && !value.is_empty());
    if has_sel {
        let (c, a) = (cursor.unwrap(), selection_anchor.unwrap());
        let (lo, hi) = (c.min(a), c.max(a));
        let lo = snap(shown, lo);
        let hi = snap(shown, hi);
        let prefix = &shown[..lo];
        let sel_text = &shown[lo..hi];
        let suffix = &shown[hi..];
        let (pw, _) = dwrite.measure_text(prefix).unwrap_or((0.0, 0.0));
        let (sw, _) = dwrite.measure_text(sel_text).unwrap_or((0.0, 0.0));
        let _ = fill_rect(
            target,
            Rect::new(ix + 8.0 + pw, row_rect.y + 2.0, sw, row_h - 4.0),
            sel_bg,
        );
        let _ = dwrite.draw_text(
            target,
            prefix,
            Rect::new(ix + 8.0, y, pw, shown_h),
            input_fg,
        );
        let _ = dwrite.draw_text(
            target,
            sel_text,
            Rect::new(ix + 8.0 + pw, y, sw, shown_h),
            field_fg,
        );
        let _ = dwrite.draw_text(
            target,
            suffix,
            Rect::new(ix + 8.0 + pw + sw, y, shown_w - pw - sw, shown_h),
            input_fg,
        );
    } else {
        let _ = dwrite.draw_text(
            target,
            shown,
            Rect::new(ix + 8.0, y, shown_w, shown_h),
            input_fg,
        );
    }

    let _ = dwrite.draw_text(
        target,
        "]",
        Rect::new(bracket_right, y, 8.0, shown_h),
        dim_fg,
    );

    if let (Some(cur), false) = (cursor, masked) {
        if !value.is_empty() {
            let prefix = &shown[..snap(shown, cur)];
            let (pw, _) = dwrite.measure_text(prefix).unwrap_or((0.0, 0.0));
            let cx = ix + 8.0 + pw;
            let _ = fill_rect(
                target,
                Rect::new(cx, row_rect.y + 3.0, 1.5, row_h - 6.0),
                accent_fg,
            );
        }
    } else if let (Some(cur), true) = (cursor, masked) {
        if !value.is_empty() {
            let char_pos = value[..snap(value, cur)].chars().count();
            let prefix: String = shown.chars().take(char_pos).collect();
            let (pw, _) = dwrite.measure_text(&prefix).unwrap_or((0.0, 0.0));
            let cx = ix + 8.0 + pw;
            let _ = fill_rect(
                target,
                Rect::new(cx, row_rect.y + 3.0, 1.5, row_h - 6.0),
                accent_fg,
            );
        }
    }
}

/// Snap a byte offset to the nearest character boundary at or before
/// it, so slicing `s[..n]` never panics mid-multibyte-character.
fn snap(s: &str, n: usize) -> usize {
    let n = n.min(s.len());
    (0..=n).rev().find(|&i| s.is_char_boundary(i)).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::form::{FormHit, ToggleGroupItem};
    use crate::types::StyledText;
    use crate::win::testing::HeadlessSurface;

    const W: f32 = 300.0;
    const H: f32 = 200.0;
    const LINE_HEIGHT: f32 = 14.0;

    fn field(id: &str, label: &str, kind: FieldKind) -> crate::primitives::form::FormField {
        crate::primitives::form::FormField {
            id: WidgetId::new(id),
            label: StyledText::plain(label.to_string()),
            kind,
            hint: StyledText::plain(""),
            disabled: false,
            validation: None,
        }
    }

    fn make_form(fields: Vec<crate::primitives::form::FormField>) -> Form {
        Form {
            id: WidgetId::new("form"),
            fields,
            focused_field: None,
            scroll_offset: 0,
            has_focus: true,
        }
    }

    /// Paint↔click round trip across a mix of field kinds, including a
    /// `ToggleGroup` whose per-item hit regions must land on the
    /// correct toggle id.
    #[test]
    fn paint_and_hit_test_round_trip() {
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
        let form = make_form(vec![
            field(
                "name",
                "Name",
                FieldKind::TextInput {
                    value: "quadraui".into(),
                    placeholder: String::new(),
                    cursor: Some(4),
                    selection_anchor: None,
                },
            ),
            field(
                "flags",
                "Flags",
                FieldKind::ToggleGroup {
                    toggles: vec![
                        ToggleGroupItem {
                            id: WidgetId::new("flags:a"),
                            label: "A".into(),
                            value: true,
                        },
                        ToggleGroupItem {
                            id: WidgetId::new("flags:b"),
                            label: "B".into(),
                            value: false,
                        },
                    ],
                },
            ),
        ]);
        let rect = Rect::new(0.0, 0.0, W, H);

        let layout = surface
            .paint(|target| {
                draw_form(target, &dwrite, rect, &form, LINE_HEIGHT);
            })
            .map(|_| win_form_layout(&dwrite, rect, &form, LINE_HEIGHT))
            .expect("paint form");

        assert_eq!(layout.visible_fields.len(), 2);
        for vf in &layout.visible_fields {
            let cx = vf.bounds.x + 1.0;
            let cy = vf.bounds.y + vf.bounds.height / 2.0;
            assert_eq!(layout.hit_test(cx, cy), FormHit::Field(vf.id.clone()));
        }

        let toggle_field = &layout.visible_fields[1];
        assert_eq!(toggle_field.item_bounds.len(), 2);
        for (id, item_rect) in &toggle_field.item_bounds {
            let cx = item_rect.x + item_rect.width / 2.0;
            let cy = item_rect.y + item_rect.height / 2.0;
            assert_eq!(layout.hit_test(cx, cy), FormHit::Field(id.clone()));
        }
    }

    /// The focused row paints `selected_bg` at its own bounds.
    #[test]
    fn focused_row_paints_selected_bg() {
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
        let mut form = make_form(vec![
            field("a", "A", FieldKind::Toggle { value: false }),
            field("b", "B", FieldKind::Toggle { value: true }),
        ]);
        form.focused_field = Some(WidgetId::new("b"));
        let rect = Rect::new(0.0, 0.0, W, H);

        let layout = surface
            .paint(|target| {
                draw_form(target, &dwrite, rect, &form, LINE_HEIGHT);
            })
            .map(|_| win_form_layout(&dwrite, rect, &form, LINE_HEIGHT))
            .expect("paint");

        let theme = Theme::default();
        let bounds = layout.visible_fields[1].bounds;
        let px = surface.pixel_at((bounds.x + 1.0) as u32, (bounds.y + 1.0) as u32);
        assert_eq!(
            (px.r, px.g, px.b),
            (
                theme.selected_bg.r,
                theme.selected_bg.g,
                theme.selected_bg.b
            )
        );
    }

    /// No-paint layout must agree byte-for-byte with what `draw_form`
    /// painted.
    #[test]
    fn no_paint_layout_matches_paint_layout() {
        let form = make_form(vec![
            field(
                "name",
                "Name",
                FieldKind::TextInput {
                    value: "hi".into(),
                    placeholder: String::new(),
                    cursor: None,
                    selection_anchor: None,
                },
            ),
            field(
                "pw",
                "Password",
                FieldKind::PasswordInput {
                    value: "secret".into(),
                    placeholder: String::new(),
                    cursor: Some(3),
                    mask_char: '*',
                },
            ),
        ]);
        let rect = Rect::new(0.0, 0.0, W, H);
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");

        let painted = surface
            .paint(|target| {
                draw_form(target, &dwrite, rect, &form, LINE_HEIGHT);
            })
            .map(|_| win_form_layout(&dwrite, rect, &form, LINE_HEIGHT))
            .expect("paint");
        let no_paint = win_form_layout(&dwrite, rect, &form, LINE_HEIGHT);
        assert_eq!(painted, no_paint);
    }
}
