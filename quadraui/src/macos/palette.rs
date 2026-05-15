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
///
/// Coordinate frame: all returned bounds (`title_bounds`,
/// `query_bounds`, `visible_items.bounds`, `create_bounds`,
/// `preview_bounds`, `scrollbar.{track,thumb}`, `hit_regions`) are in
/// **palette-local** coords (origin at 0, 0), matching
/// `tui_palette_layout` and `gtk_palette_layout`. Hosts must subtract
/// the palette's `area.x` / `area.y` from absolute click coords before
/// calling `PaletteLayout::hit_test`. The `x` / `y` params are kept in
/// the signature for symmetry with `draw_palette` but do not affect
/// output.
pub fn mac_palette_layout(
    palette: &Palette,
    _x: f64,
    _y: f64,
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
    palette.layout(
        w as f32,
        h as f32,
        title_h,
        query_h,
        SCROLLBAR_PX,
        SCROLLBAR_PX,
        |_| PaletteItemMeasure::new(line_height as f32),
    )
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

    // Title row. Layout returns local coords; shift to absolute for paint.
    if let Some(tb) = layout.title_bounds {
        let tb_x = tb.x as f64 + x;
        let tb_y = tb.y as f64 + y;
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
            tb_x + 8.0,
            tb_y + (tb.height as f64 - th) / 2.0,
            color_to_cg(theme.title_fg),
        );
    }

    // Query row + cursor. Layout returns local coords; shift to
    // absolute for paint.
    if let Some(qb) = layout.query_bounds {
        let qb_x = qb.x as f64 + x;
        let prompt = "> ";
        let (pw, qh) = measure_text(font, prompt);
        let query_y = qb.y as f64 + y;
        draw_text(
            ctx,
            font,
            prompt,
            qb_x + 8.0,
            query_y + (qb.height as f64 - qh) / 2.0,
            color_to_cg(theme.query_fg),
        );
        let query_text_x = qb_x + 8.0 + pw;
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
            let sep_y = qb.y as f64 + y + qb.height as f64;
            fill_rect(ctx, x, sep_y, w, 1.0, theme.border_fg);
        }
    }

    // Result rows. Layout returns local coords; shift to absolute for paint.
    for vis in &layout.visible_items {
        let item = &palette.items[vis.item_idx];
        let row_x = vis.bounds.x as f64 + x;
        let row_y = vis.bounds.y as f64 + y;
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

    // Pinned create-action row. Layout returns local coords; shift to
    // absolute for paint.
    if let (Some(cb), Some(label)) = (layout.create_bounds, palette.create_label.as_ref()) {
        let cb_x = cb.x as f64 + x;
        let cb_y = cb.y as f64 + y;
        fill_rect(
            ctx,
            cb_x,
            cb_y,
            cb.width as f64,
            cb.height as f64,
            theme.hover_bg,
        );
        let (_, th) = measure_text(font, label);
        draw_text(
            ctx,
            font,
            label,
            cb_x + 8.0,
            cb_y + (cb.height as f64 - th) / 2.0,
            color_to_cg(theme.accent_fg),
        );
    }

    // Scrollbar. Layout returns local coords; shift to absolute for paint.
    if let Some(sb) = layout.scrollbar {
        fill_rect(
            ctx,
            sb.track.x as f64 + x,
            sb.track.y as f64 + y,
            sb.track.width as f64,
            sb.track.height as f64,
            with_alpha(theme.scrollbar_track, 0.4),
        );
        fill_rect(
            ctx,
            sb.thumb.x as f64 + x,
            sb.thumb.y as f64 + y,
            sb.thumb.width as f64,
            sb.thumb.height as f64,
            with_alpha(theme.scrollbar_thumb, 0.8),
        );
    }

    // Preview pane (basic — paints bg + plain text lines). Layout
    // returns local coords; shift to absolute for paint.
    if let (Some(pb), Some(preview)) = (layout.preview_bounds, palette.preview.as_ref()) {
        let pb_x = pb.x as f64 + x;
        let pb_y = pb.y as f64 + y;
        fill_rect(
            ctx,
            pb_x,
            pb_y,
            pb.width as f64,
            pb.height as f64,
            theme.background,
        );
        let mut py = pb_y;
        if let Some(ref title) = preview.title {
            draw_text(
                ctx,
                font,
                title,
                pb_x + 8.0,
                py,
                color_to_cg(theme.muted_fg),
            );
            py += line_height;
        }
        for line in preview.lines.iter().skip(preview.scroll_offset) {
            if py + line_height > pb_y + pb.height as f64 {
                break;
            }
            let text: String = line.spans.iter().map(|s| s.text.as_str()).collect();
            draw_text(
                ctx,
                font,
                &text,
                pb_x + 8.0,
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

    #[test]
    fn layout_returns_local_coords_when_area_offset() {
        // Cross-backend contract: all bounds returned by
        // mac_palette_layout are in palette-local coords (origin
        // 0, 0), regardless of where the rasteriser paints, matching
        // `tui_palette_layout` and `gtk_palette_layout`. Hosts
        // subtract area.x/area.y from absolute click coords before
        // hit_test.
        //
        // Regression for #190: prior to the fix, mac_palette_layout
        // shifted hit_regions to absolute coords. Latent today (no
        // `Backend::palette_layout` trait method exposes the layout
        // to consumers), but ready to bite the moment one is added —
        // same shape as #44's tree/form click drift.
        let p = sample_palette();
        // Area offset by (40, 80) — palette modals are typically
        // centred / inset within a window.
        let area_x: f64 = 40.0;
        let area_y: f64 = 80.0;
        let layout = mac_palette_layout(&p, area_x, area_y, W as f64, H as f64, 16.0);
        // Locality: title_bounds.y must be 0, not area_y.
        let tb = layout.title_bounds.expect("title present");
        assert_eq!(
            tb.y, 0.0,
            "title_bounds.y must be local (0.0), got {}",
            tb.y,
        );
        let qb = layout.query_bounds.expect("query bounds present");
        assert!(
            qb.y < area_y as f32,
            "query_bounds.y must be local (< area_y), got {} vs area_y={}",
            qb.y,
            area_y,
        );
        // Round-trip: simulate a click at the absolute centre of the
        // query row, localise the way AppLogic does, and assert it
        // still hits Query. Pre-fix this returned the wrong region.
        let abs_y = area_y as f32 + qb.y + qb.height * 0.5;
        let abs_x = area_x as f32 + qb.x + 20.0;
        let local_x = abs_x - area_x as f32;
        let local_y = abs_y - area_y as f32;
        assert_eq!(
            layout.hit_test(local_x, local_y),
            PaletteHit::Query,
            "query click → wrong hit (coord-frame drift)",
        );
        // Round-trip each visible item the same way.
        for vi in &layout.visible_items {
            let abs_x = area_x as f32 + vi.bounds.x + vi.bounds.width * 0.5;
            let abs_y = area_y as f32 + vi.bounds.y + vi.bounds.height * 0.5;
            let local_x = abs_x - area_x as f32;
            let local_y = abs_y - area_y as f32;
            assert_eq!(
                layout.hit_test(local_x, local_y),
                PaletteHit::Item(vi.item_idx),
                "item {} click → wrong hit (coord-frame drift)",
                vi.item_idx,
            );
        }
    }
}
