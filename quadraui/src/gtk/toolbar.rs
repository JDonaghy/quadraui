//! GTK rasteriser for [`crate::primitives::toolbar::Toolbar`].
//!
//! Paints a horizontal strip of clickable action buttons using Cairo +
//! Pango. Each [`crate::ToolbarButton::Action`] becomes a pill-shaped
//! cell with optional icon glyph, label, and key hint. Separators
//! render as a thin vertical rule between groups; labels paint as
//! plain text in `theme.muted_fg` (or their `fg` override).
//!
//! ## Per-state colouring
//!
//! | State              | Foreground             | Background           |
//! |--------------------|------------------------|----------------------|
//! | Action, enabled    | `theme.foreground`     | `bar_bg`             |
//! | Action, disabled   | `theme.muted_fg`       | `bar_bg`             |
//! | Action, is_active  | `theme.foreground`     | `theme.selected_bg`  |
//! | Action, hovered    | `theme.hover_fg`       | `theme.hover_bg`     |
//! | Action, pressed    | `theme.foreground`     | `theme.selected_bg`  |
//! | Separator          | `theme.muted_fg`       | `bar_bg`             |
//! | Label              | `Label.fg` or `muted`  | `bar_bg`             |
//!
//! `bar_bg` is `Toolbar.bg.unwrap_or(theme.header_bg)`.

use gtk4::cairo::Context;
use gtk4::pango;
use pangocairo::functions as pcfn;

use super::{rounded_rect_path, set_source};
use crate::primitives::toolbar::{Toolbar, ToolbarButton, ToolbarItemMeasure, ToolbarLayout};
use crate::theme::Theme;
use crate::types::WidgetId;

/// Horizontal padding inside each action button, in px.
const ACTION_H_PAD: f64 = 8.0;
/// Width of a separator slot in px.
const SEPARATOR_PX: f64 = 12.0;
/// Corner radius for action button highlight backgrounds.
const CORNER_RADIUS: f64 = 4.0;

/// Format an Action's rendered text: `"{icon} {label} ({hint})"` with
/// optional sections trimmed when absent.
fn action_text(label: &str, icon: Option<&str>, key_hint: Option<&str>) -> String {
    let mut s = String::new();
    if let Some(icon) = icon {
        s.push_str(icon);
        s.push(' ');
    }
    s.push_str(label);
    if let Some(hint) = key_hint {
        s.push_str(" (");
        s.push_str(hint);
        s.push(')');
    }
    s
}

/// Pixel width of `text` using `pango_layout` when available, or a
/// `char_width`-based fallback otherwise. Mirrors the
/// `pango_str_width` shape `GtkBackend` uses for status / tab bar
/// fallback layouts.
fn text_width_px(pango_layout: Option<&pango::Layout>, text: &str, char_width: f64) -> f64 {
    if let Some(pl) = pango_layout {
        pl.set_text(text);
        pl.pixel_size().0.max(0) as f64
    } else {
        (text.chars().count() as f64 * char_width).ceil()
    }
}

/// Compute the pixel width of a single toolbar item.
///
/// Exposed as `pub(crate)` so `gtk::backend`'s `form_layout` measurer
/// can reuse the same widths for `FieldKind::Toolbar` fields, guaranteeing
/// that form paint and hit-test agree on item positions.
pub(crate) fn measure_item(
    pango_layout: Option<&pango::Layout>,
    char_width: f64,
    btn: &ToolbarButton,
) -> f32 {
    match btn {
        ToolbarButton::Action {
            label,
            icon,
            key_hint,
            ..
        } => {
            let text = action_text(label, icon.as_deref(), key_hint.as_deref());
            let text_w = text_width_px(pango_layout, &text, char_width);
            (text_w + 2.0 * ACTION_H_PAD) as f32
        }
        ToolbarButton::Separator => SEPARATOR_PX as f32,
        ToolbarButton::Label { text, .. } => text_width_px(pango_layout, text, char_width) as f32,
    }
}

/// Compute the GTK pixel-unit layout for a [`Toolbar`] without painting.
///
/// `pango_layout` is `Some` inside a draw frame (Pango can measure
/// accurately) and `None` from layout-only paths called between
/// frames — in that case a `char_width`-based fallback is used.
pub fn gtk_toolbar_layout(
    bar: &Toolbar,
    pango_layout: Option<&pango::Layout>,
    char_width: f64,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
) -> ToolbarLayout {
    bar.layout(x as f32, y as f32, w as f32, h as f32, |btn| {
        ToolbarItemMeasure::new(measure_item(pango_layout, char_width, btn))
    })
}

/// Draw a [`Toolbar`] into `(x, y, w, h)` on `cr`. Returns the layout
/// for host click dispatch.
#[allow(clippy::too_many_arguments)]
pub fn draw_toolbar(
    cr: &Context,
    pango_layout: &pango::Layout,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    bar: &Toolbar,
    theme: &Theme,
    hovered_id: Option<&WidgetId>,
    pressed_id: Option<&WidgetId>,
) -> ToolbarLayout {
    pango_layout.set_attributes(None);
    pango_layout.set_width(-1);
    pango_layout.set_ellipsize(pango::EllipsizeMode::None);

    // Inside a draw frame, prefer Pango measurement; `char_width` is
    // unused. We still pass a positive default so the fallback path
    // (which `draw_toolbar` itself never hits) remains well-defined.
    let toolbar_layout = gtk_toolbar_layout(bar, Some(pango_layout), 8.0, x, y, w, h);

    if w <= 0.0 || h <= 0.0 {
        return toolbar_layout;
    }

    // Clip to the bar's rect so anything painted by mistake doesn't
    // leak past the right edge.
    cr.save().ok();
    cr.rectangle(x, y, w, h);
    cr.clip();

    // Background fill.
    let bar_bg = bar.bg.unwrap_or(theme.header_bg);
    set_source(cr, bar_bg);
    cr.rectangle(x, y, w, h);
    cr.fill().ok();

    for vis in &toolbar_layout.visible_items {
        let item_x = vis.bounds.x as f64;
        let item_y = vis.bounds.y as f64;
        let item_w = vis.bounds.width as f64;
        let item_h = vis.bounds.height as f64;
        if item_w <= 0.0 || item_h <= 0.0 {
            continue;
        }

        let btn = &bar.buttons[vis.item_idx];

        match btn {
            ToolbarButton::Action {
                id,
                label,
                icon,
                key_hint,
                enabled,
                is_active,
                ..
            } => {
                let is_hovered = *enabled && hovered_id == Some(id);
                let is_pressed = *enabled && pressed_id == Some(id);

                // Highlight background for hover/pressed/active states.
                let highlight = if is_pressed || *is_active {
                    Some(theme.selected_bg)
                } else if is_hovered {
                    Some(theme.hover_bg)
                } else {
                    None
                };
                if let Some(bg) = highlight {
                    set_source(cr, bg);
                    rounded_rect_path(
                        cr,
                        item_x + 2.0,
                        item_y + 2.0,
                        item_w - 4.0,
                        item_h - 4.0,
                        CORNER_RADIUS,
                    );
                    cr.fill().ok();
                }

                // Foreground.
                let text_fg = if !*enabled {
                    theme.muted_fg
                } else if is_hovered {
                    theme.hover_fg
                } else {
                    theme.foreground
                };
                set_source(cr, text_fg);

                let text = action_text(label, icon.as_deref(), key_hint.as_deref());
                pango_layout.set_text(&text);
                let (tw, th) = pango_layout.pixel_size();
                let tx = item_x + (item_w - tw as f64) / 2.0;
                let ty = item_y + (item_h - th as f64) / 2.0;
                cr.move_to(tx, ty);
                pcfn::show_layout(cr, pango_layout);
            }
            ToolbarButton::Separator => {
                set_source(cr, theme.muted_fg);
                cr.set_line_width(1.0);
                let mid_x = item_x + item_w / 2.0;
                let pad_y = (item_h * 0.2).max(2.0);
                cr.move_to(mid_x, item_y + pad_y);
                cr.line_to(mid_x, item_y + item_h - pad_y);
                cr.stroke().ok();
            }
            ToolbarButton::Label { text, fg } => {
                let color = fg.unwrap_or(theme.muted_fg);
                set_source(cr, color);
                pango_layout.set_text(text);
                let (_tw, th) = pango_layout.pixel_size();
                let ty = item_y + (item_h - th as f64) / 2.0;
                cr.move_to(item_x, ty);
                pcfn::show_layout(cr, pango_layout);
            }
        }
    }

    cr.restore().ok();
    toolbar_layout
}
