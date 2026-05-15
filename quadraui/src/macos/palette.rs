//! macOS rasteriser for [`crate::Palette`].
//!
//! Modal-style fuzzy picker. Mirrors
//! [`crate::gtk::palette::draw_palette`]: bordered box, title row,
//! query input with cursor, separator, scrollable item list with
//! selection highlight, optional pinned create-action row, optional
//! preview pane, optional scrollbar.
//!
//! ## Scope omissions (follow-up)
//!
//! - **Match-position highlighting** — `PaletteItem.match_positions`
//!   per-character fg highlights are deferred with the unified
//!   text-attribute pass. Items render in plain fg today.
//! - **Preview pane content** — preview lines render as plain text;
//!   syntax-highlighted spans land with the same pass.

use core_graphics::geometry::CGRect;
use core_graphics::sys::CGContextRef;
use core_text::font::CTFont;

use super::text::{draw_text, measure_text};
use crate::primitives::palette::{Palette, PaletteItemMeasure};
use crate::theme::Theme;
use crate::types::Color;

const SCROLLBAR_PX: f32 = 8.0;

/// Compute the macOS palette layout (used by both paint and host
/// hit-testing).
pub fn mac_palette_layout(
    palette: &Palette,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    line_height: f64,
) -> crate::primitives::palette::PaletteLayout {
    let title_h = line_height as f32;
    let query_h = if palette.show_query {
        line_height as f32
    } else {
        0.0
    };
    let mut layout = palette.layout(
        w as f32,
        h as f32,
        title_h,
        query_h,
        SCROLLBAR_PX,
        SCROLLBAR_PX,
        |_| PaletteItemMeasure::new(line_height as f32),
    );
    // Translate bounds from palette-local to surface coords.
    let (dx, dy) = (x as f32, y as f32);
    if dx != 0.0 || dy != 0.0 {
        if let Some(tb) = layout.title_bounds.as_mut() {
            tb.x += dx;
            tb.y += dy;
        }
        if let Some(qb) = layout.query_bounds.as_mut() {
            qb.x += dx;
            qb.y += dy;
        }
        for vi in &mut layout.visible_items {
            vi.bounds.x += dx;
            vi.bounds.y += dy;
        }
        for (rect, _) in &mut layout.hit_regions {
            rect.x += dx;
            rect.y += dy;
        }
        if let Some(cb) = layout.create_bounds.as_mut() {
            cb.x += dx;
            cb.y += dy;
        }
        if let Some(pb) = layout.preview_bounds.as_mut() {
            pb.x += dx;
            pb.y += dy;
        }
        if let Some(sb) = layout.scrollbar.as_mut() {
            sb.track.x += dx;
            sb.track.y += dy;
            sb.thumb.x += dx;
            sb.thumb.y += dy;
        }
    }
    layout
}

/// Draw a [`Palette`] modal into `(x, y, w, h)` on `ctx`.
///
/// # Safety
///
/// `ctx` must be a valid `CGContextRef` borrowed for the duration of
/// the call.
#[allow(clippy::too_many_arguments)]
pub unsafe fn draw_palette(
    ctx: CGContextRef,
    font: &CTFont,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    palette: &Palette,
    theme: &Theme,
    line_height: f64,
) {
    if w < 20.0 || h < line_height * 4.0 {
        return;
    }

    CGContextSaveGState(ctx);
    CGContextClipToRect(ctx, CGRect::new_xywh(x, y, w, h));
    fill_rect(ctx, x, y, w, h, theme.surface_bg);
    stroke_rect(ctx, x, y, w, h, theme.border_fg, 1.0);

    let layout = mac_palette_layout(palette, x, y, w, h, line_height);

    // Title row.
    if let Some(tb) = layout.title_bounds {
        let title_text = if palette.total_count > 0 {
            format!(
                " {}  {}/{} ",
                palette.title,
                palette.items.len(),
                palette.total_count
            )
        } else {
            format!(" {} ", palette.title)
        };
        let (_, th) = measure_text(font, &title_text);
        draw_text(
            ctx,
            font,
            &title_text,
            tb.x as f64 + 8.0,
            tb.y as f64 + (tb.height as f64 - th) / 2.0,
            color_to_cg(theme.title_fg),
        );
    }

    // Query row + cursor.
    if let Some(qb) = layout.query_bounds {
        let prompt = "> ";
        let (pw, qh) = measure_text(font, prompt);
        let query_y = qb.y as f64;
        draw_text(
            ctx,
            font,
            prompt,
            qb.x as f64 + 8.0,
            query_y + (qb.height as f64 - qh) / 2.0,
            color_to_cg(theme.query_fg),
        );
        let query_text_x = qb.x as f64 + 8.0 + pw;
        draw_text(
            ctx,
            font,
            &palette.query,
            query_text_x,
            query_y + (qb.height as f64 - qh) / 2.0,
            color_to_cg(theme.query_fg),
        );
        let cursor_prefix: &str = if palette.query_cursor >= palette.query.len() {
            palette.query.as_str()
        } else {
            &palette.query[..palette.query_cursor]
        };
        let (cpw, _) = measure_text(font, cursor_prefix);
        let cursor_x = query_text_x + cpw;
        let cursor_w = (line_height * 0.45).max(2.0);
        fill_rect(
            ctx,
            cursor_x,
            query_y,
            cursor_w,
            qb.height as f64,
            theme.query_fg,
        );
    }

    // Separator row.
    if palette.show_query {
        if let Some(qb) = layout.query_bounds {
            let sep_y = qb.y as f64 + qb.height as f64;
            fill_rect(ctx, x, sep_y, w, 1.0, theme.border_fg);
        }
    }

    // Result rows.
    for vis in &layout.visible_items {
        let item = &palette.items[vis.item_idx];
        let row_x = vis.bounds.x as f64;
        let row_y = vis.bounds.y as f64;
        let row_w = vis.bounds.width as f64;
        let row_h = vis.bounds.height as f64;
        let is_selected = vis.item_idx == palette.selected_idx && palette.has_focus;

        if is_selected {
            fill_rect(ctx, row_x, row_y, row_w, row_h, theme.selected_bg);
        }

        let full_text: String = item.text.spans.iter().map(|s| s.text.as_str()).collect();
        let (_, lh) = measure_text(font, &full_text);
        let text_y = row_y + (row_h - lh) / 2.0;
        draw_text(
            ctx,
            font,
            &full_text,
            row_x + 8.0,
            text_y,
            color_to_cg(theme.surface_fg),
        );

        if let Some(ref det) = item.detail {
            let det_text: String = det.spans.iter().map(|s| s.text.as_str()).collect();
            if !det_text.is_empty() {
                let (dw, _) = measure_text(font, &det_text);
                draw_text(
                    ctx,
                    font,
                    &det_text,
                    row_x + row_w - dw - 8.0,
                    text_y,
                    color_to_cg(theme.muted_fg),
                );
            }
        }
    }

    // Pinned create-action row.
    if let (Some(cb), Some(label)) = (layout.create_bounds, palette.create_label.as_ref()) {
        fill_rect(
            ctx,
            cb.x as f64,
            cb.y as f64,
            cb.width as f64,
            cb.height as f64,
            theme.hover_bg,
        );
        let (_, th) = measure_text(font, label);
        draw_text(
            ctx,
            font,
            label,
            cb.x as f64 + 8.0,
            cb.y as f64 + (cb.height as f64 - th) / 2.0,
            color_to_cg(theme.accent_fg),
        );
    }

    // Scrollbar.
    if let Some(sb) = layout.scrollbar {
        fill_rect(
            ctx,
            sb.track.x as f64,
            sb.track.y as f64,
            sb.track.width as f64,
            sb.track.height as f64,
            with_alpha(theme.scrollbar_track, 0.4),
        );
        fill_rect(
            ctx,
            sb.thumb.x as f64,
            sb.thumb.y as f64,
            sb.thumb.width as f64,
            sb.thumb.height as f64,
            with_alpha(theme.scrollbar_thumb, 0.8),
        );
    }

    // Preview pane (basic — paints bg + plain text lines).
    if let (Some(pb), Some(preview)) = (layout.preview_bounds, palette.preview.as_ref()) {
        fill_rect(
            ctx,
            pb.x as f64,
            pb.y as f64,
            pb.width as f64,
            pb.height as f64,
            theme.background,
        );
        let mut py = pb.y as f64;
        if let Some(ref title) = preview.title {
            draw_text(
                ctx,
                font,
                title,
                pb.x as f64 + 8.0,
                py,
                color_to_cg(theme.muted_fg),
            );
            py += line_height;
        }
        for line in preview.lines.iter().skip(preview.scroll_offset) {
            if py + line_height > pb.y as f64 + pb.height as f64 {
                break;
            }
            let text: String = line.spans.iter().map(|s| s.text.as_str()).collect();
            draw_text(
                ctx,
                font,
                &text,
                pb.x as f64 + 8.0,
                py,
                color_to_cg(theme.foreground),
            );
            py += line_height;
        }
    }

    CGContextRestoreGState(ctx);
}

fn with_alpha(c: Color, alpha: f64) -> Color {
    Color {
        r: c.r,
        g: c.g,
        b: c.b,
        a: (255.0 * alpha).round().clamp(0.0, 255.0) as u8,
    }
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
    CGContextStrokeRect(ctx, CGRect::new_xywh(x, y, w, h));
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
    use crate::event::{Rect as QRect, Viewport};
    use crate::primitives::palette::{PaletteHit, PaletteItem};
    use crate::types::{StyledText, WidgetId};
    use crate::Backend;

    const W: u32 = 400;
    const H: u32 = 240;

    fn font() -> CTFont {
        make_font("Menlo", 14.0).expect("Menlo installed")
    }

    fn sample_palette() -> Palette {
        Palette {
            id: WidgetId::new("pal"),
            title: "Commands".into(),
            query: "fo".into(),
            query_cursor: 2,
            items: vec![
                PaletteItem {
                    text: StyledText::plain("foo: open file"),
                    detail: Some(StyledText::plain("Ctrl+O")),
                    icon: None,
                    match_positions: vec![0, 1],
                    depth: 0,
                    expandable: false,
                    expanded: false,
                },
                PaletteItem {
                    text: StyledText::plain("foo: close tab"),
                    detail: None,
                    icon: None,
                    match_positions: vec![0, 1],
                    depth: 0,
                    expandable: false,
                    expanded: false,
                },
            ],
            selected_idx: 1,
            scroll_offset: 0,
            total_count: 25,
            has_focus: true,
            show_query: true,
            create_label: None,
            preview: None,
        }
    }

    fn paint_via_backend(palette: &Palette) -> BitmapSurface {
        let surface = BitmapSurface::new(W, H);
        surface.fill(0.0, 0.0, 0.0, 0.0);
        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            b.draw_palette(QRect::new(0.0, 0.0, W as f32, H as f32), palette);
        });
        backend.end_frame();
        surface
    }

    #[test]
    fn palette_paints_surface_bg() {
        let p = sample_palette();
        let surface = paint_via_backend(&p);
        let theme = Theme::default();
        // Probe right edge of the popup, well below all rows.
        let (r, g, b, _) = surface.pixel(W - 4, H - 4);
        assert_eq!(
            (r, g, b),
            (theme.surface_bg.r, theme.surface_bg.g, theme.surface_bg.b),
        );
    }

    #[test]
    fn selected_item_paints_selected_bg() {
        let p = sample_palette();
        let surface = paint_via_backend(&p);
        let theme = Theme::default();
        let layout = mac_palette_layout(&p, 0.0, 0.0, W as f64, H as f64, 16.0);
        let sel = layout
            .visible_items
            .iter()
            .find(|v| v.item_idx == 1)
            .expect("selected item visible");
        let px = (sel.bounds.x + sel.bounds.width - 4.0) as u32;
        let py = (sel.bounds.y + sel.bounds.height / 2.0) as u32;
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
    fn hit_test_resolves_query_row() {
        let p = sample_palette();
        let layout = mac_palette_layout(&p, 0.0, 0.0, W as f64, H as f64, 16.0);
        let qb = layout.query_bounds.expect("query bounds present");
        let hit = layout.hit_test(qb.x + 20.0, qb.y + qb.height * 0.5);
        assert_eq!(hit, PaletteHit::Query);
    }
}
