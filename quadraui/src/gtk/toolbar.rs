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
//! Priority (highest first): pressed → hovered → focused → is_active → enabled.
//!
//! | State              | Foreground             | Background           |
//! |--------------------|------------------------|----------------------|
//! | Action, enabled    | `theme.foreground`     | `bar_bg`             |
//! | Action, disabled   | `theme.muted_fg`       | `bar_bg`             |
//! | Action, is_active  | `theme.foreground`     | `theme.selected_bg`  |
//! | Action, focused    | `theme.foreground`     | `bar_bg` + ring      |
//! | Action, hovered    | `theme.hover_fg`       | `theme.hover_bg`     |
//! | Action, pressed    | `theme.foreground`     | `theme.selected_bg`  |
//! | Separator          | `theme.muted_fg`       | `bar_bg`             |
//! | Label              | `Label.fg` or `muted`  | `bar_bg`             |
//!
//! Keyboard-focused buttons (via [`crate::primitives::toolbar::Toolbar::focused_index`])
//! receive a `theme.accent_fg`-coloured rounded-rect stroke drawn on top of the
//! button background. Hover / pressed still take visual priority over focus.
//!
//! `bar_bg` is `Toolbar.bg.unwrap_or(theme.header_bg)`.

use gtk4::cairo::Context;
use gtk4::pango;

use super::{rounded_rect_path, set_source};
use crate::primitives::layout_metrics::TextMeasure;
use crate::primitives::toolbar::{
    action_text, measure_button, Toolbar, ToolbarButton, ToolbarItemMeasure, ToolbarLayout,
};
use crate::theme::Theme;
use crate::types::WidgetId;

/// Corner radius for action button highlight backgrounds.
const CORNER_RADIUS: f64 = 4.0;

/// Adapts a live `pango::Layout` (falling back to a `char_width`-based
/// estimate when none is available, e.g. from a layout-only call between
/// paint frames) to the shared [`TextMeasure`] trait so
/// [`crate::primitives::toolbar::measure_button`] never has to name a
/// Pango type — mirrors `macos::toolbar`'s / `win::toolbar`'s twin
/// adapters (#730).
///
/// `pub(crate)` so `gtk::sidebar_panel`'s embedded toolbar header can
/// build the exact same adapter for its own `measure_button` calls,
/// guaranteeing paint and hit-test agree on item positions everywhere a
/// `Toolbar` appears — with no separate per-caller measurer function.
pub(crate) struct PangoMeasure<'a> {
    pub(crate) pango_layout: Option<&'a pango::Layout>,
    pub(crate) char_width: f64,
}

impl TextMeasure for PangoMeasure<'_> {
    fn width_of(&self, text: &str) -> f32 {
        if let Some(pl) = self.pango_layout {
            pl.set_text(text);
            pl.pixel_size().0.max(0) as f32
        } else {
            (text.chars().count() as f64 * self.char_width).ceil() as f32
        }
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
    let measure = PangoMeasure {
        pango_layout,
        char_width,
    };
    bar.layout(x as f32, y as f32, w as f32, h as f32, |btn| {
        ToolbarItemMeasure::new(measure_button(&measure, btn))
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
                let is_focused = *enabled && bar.focused_index == Some(vis.item_idx);

                // Highlight background for hover/pressed/active states.
                // Priority: pressed > hovered > focused > is_active.
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

                // Focus ring: drawn when focused and not already
                // visually dominated by hover or pressed highlight.
                if is_focused && !is_hovered && !is_pressed && !*is_active {
                    set_source(cr, theme.accent_fg);
                    cr.set_line_width(1.0);
                    rounded_rect_path(
                        cr,
                        item_x + 1.5,
                        item_y + 1.5,
                        item_w - 3.0,
                        item_h - 3.0,
                        CORNER_RADIUS,
                    );
                    cr.stroke().ok();
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
                super::painted_text::show_layout(cr, pango_layout);
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
                super::painted_text::show_layout(cr, pango_layout);
            }
        }
    }

    cr.restore().ok();
    toolbar_layout
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::toolbar::ToolbarHit;
    use crate::types::WidgetId;

    fn test_toolbar() -> Toolbar {
        Toolbar {
            id: WidgetId::new("tb"),
            buttons: vec![ToolbarButton::Action {
                id: WidgetId::new("tb:action"),
                label: "Refine".into(),
                icon: None,
                key_hint: None,
                enabled: true,
                is_active: false,
                tooltip: String::new(),
            }],
            bg: None,
            focused_index: None,
        }
    }

    /// `toolbar_layout` is documented **ABSOLUTE** (issue #505):
    /// `gtk_toolbar_layout` forwards `x`/`y` straight through to
    /// `Toolbar::layout` as `origin_x`/`origin_y`, which folds them into
    /// every visible item's `bounds` — so a non-zero origin must shift
    /// bounds by exactly `(x, y)`, unlike the LOCAL primitives
    /// (`status_bar_layout`, `data_table_layout`, `form_layout`) that
    /// ignore it entirely. `pango_layout: None` exercises the
    /// `char_width` fallback measurer, the same path a click-time
    /// (outside-frame) hit test uses.
    fn round_trip_at(x: f64, y: f64) {
        let bar = test_toolbar();
        let layout = gtk_toolbar_layout(&bar, None, 8.0, x, y, 100.0, 20.0);

        let vis = &layout.visible_items[0];
        assert_eq!(vis.bounds.x as f64, x);
        assert_eq!(vis.bounds.y as f64, y);

        let ccx = vis.bounds.x + 1.0;
        let ccy = vis.bounds.y + 1.0;
        assert_eq!(
            layout.hit_test(ccx, ccy),
            ToolbarHit::Button(WidgetId::new("tb:action"))
        );
    }

    #[test]
    fn paint_and_click_round_trip() {
        round_trip_at(0.0, 0.0);
    }

    /// Non-zero-origin regression guard (issue #505 / LESSONS.md).
    #[test]
    fn paint_and_click_round_trip_at_nonzero_origin() {
        round_trip_at(7.0, 13.0);
    }
}
