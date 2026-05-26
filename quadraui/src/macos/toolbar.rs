//! macOS rasteriser for [`crate::primitives::toolbar::Toolbar`].
//!
//! Paints a horizontal strip of clickable action buttons using Core
//! Graphics + Core Text. Mirrors the TUI / GTK rasterisers' look:
//! enabled actions render in `theme.foreground`, disabled in
//! `theme.muted_fg`, hovered actions get a `theme.hover_bg` tint, and
//! active / pressed actions get `theme.selected_bg`. Separators draw
//! as a thin vertical line; labels paint as plain text.
//!
//! Per D6: layout policy lives in [`crate::primitives::toolbar::Toolbar::layout`];
//! this rasteriser paints what that returns and provides the
//! [`ToolbarLayout`] for click dispatch.

use core_graphics::geometry::CGRect;
use core_graphics::sys::CGContextRef;
use core_text::font::CTFont;

use super::text::{draw_text, measure_text};
use crate::primitives::toolbar::{Toolbar, ToolbarButton, ToolbarItemMeasure, ToolbarLayout};
use crate::theme::Theme;
use crate::types::{Color, WidgetId};

/// Horizontal padding inside each action button, in px.
const ACTION_H_PAD: f64 = 8.0;
/// Width of a separator slot in px.
const SEPARATOR_PX: f64 = 12.0;

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

fn measure_item(font: &CTFont, btn: &ToolbarButton) -> f32 {
    match btn {
        ToolbarButton::Action {
            label,
            icon,
            key_hint,
            ..
        } => {
            let text = action_text(label, icon.as_deref(), key_hint.as_deref());
            let (w, _) = measure_text(font, &text);
            (w + 2.0 * ACTION_H_PAD) as f32
        }
        ToolbarButton::Separator => SEPARATOR_PX as f32,
        ToolbarButton::Label { text, .. } => {
            let (w, _) = measure_text(font, text);
            w as f32
        }
    }
}

/// Compute the macOS pixel-unit layout for a [`Toolbar`] without
/// painting. `font` is required for accurate text measurement.
pub fn mac_toolbar_layout(
    bar: &Toolbar,
    font: &CTFont,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
) -> ToolbarLayout {
    bar.layout(x as f32, y as f32, w as f32, h as f32, |btn| {
        ToolbarItemMeasure::new(measure_item(font, btn))
    })
}

/// Paint `bar` into `(x, y, w, h)` on `ctx`. Returns the resolved
/// layout for host click dispatch.
///
/// # Safety
///
/// `ctx` must be a valid `CGContextRef` borrowed for the duration of
/// the call (typical: the frame-scope pointer stashed on
/// [`super::MacBackend`]).
#[allow(clippy::too_many_arguments)]
pub unsafe fn draw_toolbar(
    ctx: CGContextRef,
    font: &CTFont,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    bar: &Toolbar,
    theme: &Theme,
    hovered_id: Option<&WidgetId>,
    pressed_id: Option<&WidgetId>,
) -> ToolbarLayout {
    let layout = mac_toolbar_layout(bar, font, x, y, w, h);

    if w <= 0.0 || h <= 0.0 {
        return layout;
    }

    CGContextSaveGState(ctx);
    CGContextClipToRect(ctx, cgrect(x, y, w, h));

    let bar_bg = bar.bg.unwrap_or(theme.header_bg);
    fill_rect(ctx, x, y, w, h, bar_bg);

    for vis in &layout.visible_items {
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

                if is_pressed || *is_active {
                    fill_rect(
                        ctx,
                        item_x + 2.0,
                        item_y + 2.0,
                        item_w - 4.0,
                        item_h - 4.0,
                        theme.selected_bg,
                    );
                } else if is_hovered {
                    fill_rect(
                        ctx,
                        item_x + 2.0,
                        item_y + 2.0,
                        item_w - 4.0,
                        item_h - 4.0,
                        theme.hover_bg,
                    );
                }

                let text = action_text(label, icon.as_deref(), key_hint.as_deref());
                let (tw, th) = measure_text(font, &text);
                let tx = item_x + (item_w - tw) / 2.0;
                let ty = item_y + (item_h - th) / 2.0;
                let fg = if !*enabled {
                    theme.muted_fg
                } else if is_hovered {
                    theme.hover_fg
                } else {
                    theme.foreground
                };
                draw_text(ctx, font, &text, tx, ty, color_to_cg(fg));
            }
            ToolbarButton::Separator => {
                let mid_x = item_x + item_w / 2.0;
                let pad_y = (item_h * 0.2).max(2.0);
                set_stroke_color(ctx, theme.muted_fg);
                CGContextSetLineWidth(ctx, 1.0);
                CGContextMoveToPoint(ctx, mid_x, item_y + pad_y);
                CGContextAddLineToPoint(ctx, mid_x, item_y + item_h - pad_y);
                CGContextStrokePath(ctx);
            }
            ToolbarButton::Label { text, fg } => {
                let color = fg.unwrap_or(theme.muted_fg);
                let (_, th) = measure_text(font, text);
                let ty = item_y + (item_h - th) / 2.0;
                draw_text(ctx, font, text, item_x, ty, color_to_cg(color));
            }
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
    CGContextFillRect(ctx, cgrect(x, y, w, h));
}

unsafe fn set_stroke_color(ctx: CGContextRef, c: Color) {
    let (r, g, b, a) = color_to_cg(c);
    CGContextSetRGBStrokeColor(ctx, r, g, b, a);
}

fn cgrect(x: f64, y: f64, w: f64, h: f64) -> CGRect {
    use core_graphics::geometry::{CGPoint, CGSize};
    CGRect::new(&CGPoint::new(x, y), &CGSize::new(w, h))
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
    fn CGContextSetRGBStrokeColor(
        c: CGContextRef,
        red: core_graphics::base::CGFloat,
        green: core_graphics::base::CGFloat,
        blue: core_graphics::base::CGFloat,
        alpha: core_graphics::base::CGFloat,
    );
    fn CGContextSetLineWidth(c: CGContextRef, width: core_graphics::base::CGFloat);
    fn CGContextFillRect(c: CGContextRef, rect: CGRect);
    fn CGContextMoveToPoint(
        c: CGContextRef,
        x: core_graphics::base::CGFloat,
        y: core_graphics::base::CGFloat,
    );
    fn CGContextAddLineToPoint(
        c: CGContextRef,
        x: core_graphics::base::CGFloat,
        y: core_graphics::base::CGFloat,
    );
    fn CGContextStrokePath(c: CGContextRef);
}
