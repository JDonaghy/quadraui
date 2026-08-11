//! macOS rasteriser for [`crate::primitives::toolbar::Toolbar`].
//!
//! Paints a horizontal strip of clickable action buttons using Core
//! Graphics + Core Text. Mirrors the TUI / GTK rasterisers' look:
//! enabled actions render in `theme.foreground`, disabled in
//! `theme.muted_fg`, hovered actions get a `theme.hover_bg` tint,
//! active / pressed actions get `theme.selected_bg`, and
//! keyboard-focused actions (via `Toolbar::focused_index`) receive a
//! `theme.accent_fg`-coloured rounded-rect focus ring. Separators
//! draw as a thin vertical line; labels paint as plain text.
//!
//! Priority (highest first): pressed → hovered → focused → is_active → enabled.
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
                let is_focused = *enabled && bar.focused_index == Some(vis.item_idx);

                // Background fill: pressed/active > hovered > no fill.
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

                // Focus ring: drawn when focused and not already
                // dominated by hover / pressed highlight.
                if is_focused && !is_hovered && !is_pressed && !*is_active {
                    set_stroke_color(ctx, theme.accent_fg);
                    CGContextSetLineWidth(ctx, 1.0);
                    // Simple rectangle focus ring (2 px inset on each side).
                    CGContextStrokeRect(
                        ctx,
                        cgrect(item_x + 2.0, item_y + 2.0, item_w - 4.0, item_h - 4.0),
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
    fn CGContextStrokeRect(c: CGContextRef, rect: CGRect);
}

#[cfg(test)]
mod tests {
    use super::super::headless::BitmapSurface;
    use super::super::text::make_font;
    use super::super::MacBackend;
    use super::*;
    use crate::event::{Rect as QRect, Viewport};
    use crate::primitives::toolbar::ToolbarHit;
    use crate::types::WidgetId;
    use crate::Backend;

    const W: u32 = 240;
    const H: u32 = 40;

    fn font() -> CTFont {
        make_font("Menlo", 14.0).expect("Menlo installed on every macOS host")
    }

    fn mk_action(id: &str, label: &str, enabled: bool) -> ToolbarButton {
        ToolbarButton::Action {
            id: WidgetId::new(id),
            label: label.into(),
            icon: None,
            key_hint: None,
            enabled,
            is_active: false,
            tooltip: String::new(),
        }
    }

    /// Two-button bar, both enabled, no separators/labels — so
    /// `visible_items` order matches `buttons` order 1:1 and index 0
    /// is always the "Refine" action.
    fn sample_bar() -> Toolbar {
        Toolbar {
            id: WidgetId::new("tb"),
            buttons: vec![
                mk_action("tb:refine", "Refine", true),
                mk_action("tb:drop", "Drop", true),
            ],
            bg: None,
            focused_index: None,
        }
    }

    /// Paint a bar through the full `MacBackend::draw_toolbar` path at
    /// origin `(0, 0)` and return both the surface and the resolved
    /// layout (for hit_test). Establishes the harness shape every
    /// chrome rasteriser test in this crate follows (see e.g.
    /// `macos::panel::tests::paint_via_backend`).
    fn paint_via_backend(bar: &Toolbar) -> (BitmapSurface, ToolbarLayout) {
        paint_via_backend_at(bar, 0.0, 0.0)
    }

    /// Like [`paint_via_backend`] but paints at an arbitrary `(x, y)`
    /// origin. Unlike `StatusBar`/`TextDisplay` in this same module
    /// family (bar/body-LOCAL layouts — see their doc comments),
    /// `Toolbar::layout` bakes `x`/`y` straight into each item's
    /// `bounds` (ABSOLUTE frame, matching `mac_toolbar_layout`'s doc
    /// comment). This lets tests confirm painted buttons and the
    /// returned layout still agree at a non-zero origin.
    fn paint_via_backend_at(bar: &Toolbar, x: f32, y: f32) -> (BitmapSurface, ToolbarLayout) {
        let surface = BitmapSurface::new(W, H);
        surface.fill(0.0, 0.0, 0.0, 0.0);

        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));

        let layout = std::cell::RefCell::new(None);
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            let l = b.draw_toolbar(
                QRect::new(x, y, W as f32 - x, H as f32 - y),
                bar,
                None,
                None,
            );
            *layout.borrow_mut() = Some(l);
        });
        backend.end_frame();
        (surface, layout.into_inner().unwrap())
    }

    #[test]
    fn round_trip_click_hits_enabled_button() {
        // Paint at origin (0, 0), then hit-test the centre of the
        // first visible ("Refine") button's painted bounds. Assert the
        // layout reports a hit on that button's id.
        let bar = sample_bar();
        let (_surface, layout) = paint_via_backend(&bar);
        assert_eq!(layout.visible_items.len(), 2);

        let refine = &layout.visible_items[0];
        let hit = layout.hit_test(
            refine.bounds.x + refine.bounds.width * 0.5,
            refine.bounds.y + refine.bounds.height * 0.5,
        );
        assert_eq!(
            hit,
            ToolbarHit::Button(WidgetId::new("tb:refine")),
            "expected Refine button hit",
        );
    }

    #[test]
    fn round_trip_click_hits_enabled_button_at_nonzero_origin() {
        // Regression for quadraui#494 / LESSONS.md "Layout helpers must
        // return coords in the same frame across backends": the
        // origin-(0,0) test above can't distinguish a `Toolbar::layout`
        // that (correctly) bakes `x`/`y` into `bounds` from one that
        // (incorrectly) ignores them — both look identical when
        // x == y == 0. Paint at a non-zero origin and confirm both the
        // painted button position and the `hit_test` round trip agree
        // on the same absolute frame. This file has no `#[cfg(test)]`
        // module prior to quadraui#494; unlike sibling files' nonzero-
        // origin regressions this isn't a refactor of a pre-existing
        // test, so it's written fresh per LESSONS.md's guidance and is
        // unverified by the compiler in this sandbox (no macOS target).
        let bar = sample_bar();
        let origin_x = 7.0_f32;
        let origin_y = 13.0_f32;
        let (_surface, layout) = paint_via_backend_at(&bar, origin_x, origin_y);

        let refine = &layout.visible_items[0];
        assert!(
            (refine.bounds.x - origin_x).abs() < 0.01,
            "first button should start at origin_x={}, got {}",
            origin_x,
            refine.bounds.x,
        );
        assert!(
            (refine.bounds.y - origin_y).abs() < 0.01,
            "first button should start at origin_y={}, got {}",
            origin_y,
            refine.bounds.y,
        );

        let hit = layout.hit_test(
            refine.bounds.x + refine.bounds.width * 0.5,
            refine.bounds.y + refine.bounds.height * 0.5,
        );
        assert_eq!(
            hit,
            ToolbarHit::Button(WidgetId::new("tb:refine")),
            "expected Refine button hit at non-zero origin",
        );
    }
}
