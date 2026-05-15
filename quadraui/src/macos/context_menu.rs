//! macOS rasteriser for [`crate::ContextMenu`].
//!
//! Mirrors [`crate::gtk::context_menu::draw_context_menu`]: bordered
//! rectangle, per-item rows with selected-bg highlight, separator
//! lines, optional right-aligned detail text. Returns per-clickable
//! hit rectangles as `Vec<(Rect, WidgetId)>` so the caller's click
//! handler can resolve menu clicks without re-running layout.

use core_graphics::geometry::CGRect;
use core_graphics::sys::CGContextRef;
use core_text::font::CTFont;

use super::text::{draw_text, measure_text};
use crate::event::Rect as QRect;
use crate::primitives::context_menu::{ContextMenu, ContextMenuLayout};
use crate::theme::Theme;
use crate::types::{Color, WidgetId};

/// Draw a [`ContextMenu`] popup. Returns per-clickable hit
/// rectangles paired with their item IDs.
///
/// # Safety
///
/// `ctx` must be a valid `CGContextRef` borrowed for the duration of
/// the call.
#[allow(clippy::too_many_arguments)]
pub unsafe fn draw_context_menu(
    ctx: CGContextRef,
    font: &CTFont,
    menu: &ContextMenu,
    menu_layout: &ContextMenuLayout,
    theme: &Theme,
) -> Vec<(QRect, WidgetId)> {
    let bounds = menu_layout.bounds;
    if bounds.width <= 0.0 || bounds.height <= 0.0 {
        return Vec::new();
    }

    let bg = menu.bg.unwrap_or(theme.hover_bg);
    fill_rect(
        ctx,
        bounds.x as f64,
        bounds.y as f64,
        bounds.width as f64,
        bounds.height as f64,
        bg,
    );
    stroke_rect(
        ctx,
        bounds.x as f64 + 0.5,
        bounds.y as f64 + 0.5,
        bounds.width as f64 - 1.0,
        bounds.height as f64 - 1.0,
        theme.hover_border,
        1.0,
    );

    let mut hits: Vec<(QRect, WidgetId)> = Vec::new();

    // Pass 1: backgrounds (separator lines + selection highlights).
    for vis in &menu_layout.visible_items {
        let row_x = vis.bounds.x as f64;
        let row_y = vis.bounds.y as f64;
        let row_w = vis.bounds.width as f64;
        let row_h = vis.bounds.height as f64;

        if vis.is_separator {
            let sep_y = row_y + row_h * 0.5;
            fill_rect(ctx, row_x + 4.0, sep_y, row_w - 8.0, 1.0, theme.muted_fg);
            continue;
        }

        let is_selected = vis.item_idx == menu.selected_idx && vis.clickable;
        if is_selected {
            fill_rect(
                ctx,
                row_x + 1.0,
                row_y,
                row_w - 2.0,
                row_h,
                theme.selected_bg,
            );
        }

        if vis.clickable {
            if let Some(ref id) = menu.items[vis.item_idx].id {
                hits.push((vis.bounds, id.clone()));
            }
        }
    }

    // Pass 2: labels + detail text on top of backgrounds.
    for vis in &menu_layout.visible_items {
        if vis.is_separator {
            continue;
        }
        let item = &menu.items[vis.item_idx];
        let row_x = vis.bounds.x as f64;
        let row_y = vis.bounds.y as f64;
        let row_w = vis.bounds.width as f64;
        let row_h = vis.bounds.height as f64;

        let label_text: String = item.label.spans.iter().map(|s| s.text.as_str()).collect();
        let label_fg = if vis.clickable {
            theme.foreground
        } else {
            theme.muted_fg
        };
        let (_, lh) = measure_text(font, &label_text);
        let text_y = row_y + (row_h - lh) * 0.5;
        draw_text(
            ctx,
            font,
            &label_text,
            row_x + 8.0,
            text_y,
            color_to_cg(label_fg),
        );

        if let Some(ref det) = item.detail {
            let det_text: String = det.spans.iter().map(|s| s.text.as_str()).collect();
            if !det_text.is_empty() {
                let (sw, _) = measure_text(font, &det_text);
                draw_text(
                    ctx,
                    font,
                    &det_text,
                    row_x + row_w - sw - 8.0,
                    text_y,
                    color_to_cg(theme.muted_fg),
                );
            }
        }
    }

    hits
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
    use crate::primitives::context_menu::{
        ContextMenuItem, ContextMenuItemMeasure, ContextMenuPlacement,
    };
    use crate::types::StyledText;
    use crate::Backend;

    const W: u32 = 200;
    const H: u32 = 120;

    fn font() -> CTFont {
        make_font("Menlo", 14.0).expect("Menlo installed")
    }

    fn item(id: &str, label: &str) -> ContextMenuItem {
        ContextMenuItem {
            id: Some(WidgetId::new(id)),
            label: StyledText::plain(label),
            detail: None,
            disabled: false,
        }
    }

    fn separator() -> ContextMenuItem {
        ContextMenuItem {
            id: None,
            label: StyledText::default(),
            detail: None,
            disabled: false,
        }
    }

    fn sample_menu() -> ContextMenu {
        ContextMenu {
            id: WidgetId::new("menu"),
            items: vec![
                item("cut", "Cut"),
                item("copy", "Copy"),
                separator(),
                item("paste", "Paste"),
            ],
            selected_idx: 1,
            placement: ContextMenuPlacement::Below,
            bg: None,
        }
    }

    fn layout_for(menu: &ContextMenu, viewport: QRect, item_h: f32) -> ContextMenuLayout {
        menu.layout(20.0, 20.0, viewport, 120.0, |_| {
            ContextMenuItemMeasure::new(item_h)
        })
    }

    fn paint_via_backend(
        menu: &ContextMenu,
        layout: &ContextMenuLayout,
    ) -> (BitmapSurface, Vec<(QRect, WidgetId)>) {
        let surface = BitmapSurface::new(W, H);
        surface.fill(0.0, 0.0, 0.0, 0.0);
        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        let hits = std::cell::RefCell::new(Vec::new());
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            *hits.borrow_mut() = b.draw_context_menu(menu, layout);
        });
        backend.end_frame();
        (surface, hits.into_inner())
    }

    #[test]
    fn menu_paints_bg_inside_bounds() {
        let menu = sample_menu();
        let viewport = QRect::new(0.0, 0.0, W as f32, H as f32);
        let layout = layout_for(&menu, viewport, 20.0);
        let (surface, _hits) = paint_via_backend(&menu, &layout);
        let theme = Theme::default();
        let b = layout.bounds;
        // Probe just inside the bordered rect, away from item glyphs.
        let (r, g, bp, _) =
            surface.pixel((b.x + b.width - 8.0) as u32, (b.y + b.height - 8.0) as u32);
        assert_eq!(
            (r, g, bp),
            (theme.hover_bg.r, theme.hover_bg.g, theme.hover_bg.b),
        );
    }

    #[test]
    fn selected_row_paints_selected_bg() {
        let menu = sample_menu();
        let viewport = QRect::new(0.0, 0.0, W as f32, H as f32);
        let layout = layout_for(&menu, viewport, 20.0);
        let (surface, _hits) = paint_via_backend(&menu, &layout);
        let theme = Theme::default();
        // selected_idx = 1 → second row (Copy).
        let row = layout
            .visible_items
            .iter()
            .find(|v| v.item_idx == 1 && v.clickable)
            .expect("copy row visible");
        let px = (row.bounds.x + row.bounds.width - 4.0) as u32;
        let py = (row.bounds.y + row.bounds.height / 2.0) as u32;
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
    fn hits_returned_for_clickable_items_only() {
        let menu = sample_menu();
        let viewport = QRect::new(0.0, 0.0, W as f32, H as f32);
        let layout = layout_for(&menu, viewport, 20.0);
        let (_surface, hits) = paint_via_backend(&menu, &layout);
        // Three clickable items (cut, copy, paste); separator not in hits.
        assert_eq!(hits.len(), 3);
        let ids: Vec<&str> = hits.iter().map(|(_, id)| id.as_str()).collect();
        assert!(ids.contains(&"cut"));
        assert!(ids.contains(&"copy"));
        assert!(ids.contains(&"paste"));
    }
}
