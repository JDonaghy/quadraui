//! macOS rasteriser for [`crate::RichTextPopup`].
//!
//! Mirrors [`crate::gtk::rich_text_popup::draw_rich_text_popup`]:
//! bordered popup, per-line styled-text rendering (per-span fg via
//! `draw_text` calls), focused-link underline, optional scrollbar.
//!
//! ## Scope omissions (follow-up)
//!
//! - **Selection bg + inverted fg** — the GTK rasteriser paints a
//!   single Cairo rect under selected characters then inverts fg
//!   per-character via Pango attrs. macOS deferred with the unified
//!   text-attribute pass.
//! - **Bold / italic span attributes** — same as above.
//! - **Per-line font scale** (markdown heading rows) — needs
//!   `CTFontCreateCopyWithSymbolicTraits` or per-line CTFont swap.
//! - **Focused-link underline** — needs `kCTUnderlineStyleAttributeName`.

use core_graphics::geometry::CGRect;
use core_graphics::sys::CGContextRef;
use core_text::font::CTFont;

use super::text::{draw_text, measure_text};
use crate::primitives::rich_text_popup::{RichTextPopup, RichTextPopupLayout};
use crate::theme::Theme;
use crate::types::Color;

pub const RICH_TEXT_POPUP_SB_WIDTH: f64 = 8.0;
pub const RICH_TEXT_POPUP_SB_INSET: f64 = 1.0;

/// Draw a [`RichTextPopup`] at its resolved layout. Returns per-link
/// hit regions as `(rect, url)` tuples.
///
/// # Safety
///
/// `ctx` must be a valid `CGContextRef` borrowed for the duration of
/// the call.
pub unsafe fn draw_rich_text_popup(
    ctx: CGContextRef,
    font: &CTFont,
    popup: &RichTextPopup,
    layout: &RichTextPopupLayout,
    theme: &Theme,
) {
    let bounds = layout.bounds;
    if bounds.width <= 0.0 || bounds.height <= 0.0 {
        return;
    }
    let bx = bounds.x as f64;
    let by = bounds.y as f64;
    let bw = bounds.width as f64;
    let bh = bounds.height as f64;

    let bg = popup.bg.unwrap_or(theme.hover_bg);
    let fg = popup.fg.unwrap_or(theme.hover_fg);
    let border = if popup.has_focus {
        theme.link_fg
    } else {
        theme.hover_border
    };

    fill_rect(ctx, bx, by, bw, bh, bg);
    stroke_rect(ctx, bx, by, bw, bh, border, 1.0);

    let content = layout.content_bounds;

    CGContextSaveGState(ctx);
    CGContextClipToRect(
        ctx,
        CGRect::new_xywh(
            content.x as f64,
            content.y as f64,
            content.width as f64,
            content.height as f64,
        ),
    );

    for vis in &layout.visible_lines {
        let row_y = vis.bounds.y as f64;
        let line_x = vis.bounds.x as f64;
        let Some(styled) = popup.lines.get(vis.line_idx) else {
            continue;
        };
        // Per-span sequential render. Each span draws at its measured
        // x-offset using its own fg.
        let mut span_x = line_x;
        for span in &styled.spans {
            let span_fg = span.fg.unwrap_or(fg);
            draw_text(ctx, font, &span.text, span_x, row_y, color_to_cg(span_fg));
            let (sw, _) = measure_text(font, &span.text);
            span_x += sw;
        }
    }

    CGContextRestoreGState(ctx);

    // Scrollbar — track + thumb.
    if let Some(sb) = layout.scrollbar {
        let sb_w = RICH_TEXT_POPUP_SB_WIDTH;
        let sb_x = bx + bw - sb_w - RICH_TEXT_POPUP_SB_INSET;
        let track_y = sb.track.y as f64;
        let track_h = sb.track.height as f64;
        fill_rect(
            ctx,
            sb_x,
            track_y,
            sb_w,
            track_h,
            with_alpha(theme.muted_fg, 0.3),
        );
        let thumb_top_off = (sb.thumb.y - sb.track.y) as f64;
        let thumb_h = sb.thumb.height as f64;
        fill_rect(
            ctx,
            sb_x + 1.0,
            track_y + thumb_top_off,
            sb_w - 2.0,
            thumb_h,
            border,
        );
    }
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
    use crate::primitives::rich_text_popup::PopupPlacement;
    use crate::types::{StyledText, WidgetId};
    use crate::Backend;

    const W: u32 = 320;
    const H: u32 = 200;

    fn font() -> CTFont {
        make_font("Menlo", 14.0).expect("Menlo installed")
    }

    fn sample_popup() -> RichTextPopup {
        RichTextPopup {
            id: WidgetId::new("rtp"),
            lines: vec![
                StyledText::plain("fn map<U, F>("),
                StyledText::plain("    self,"),
                StyledText::plain("    f: F,"),
                StyledText::plain(") -> Option<U>"),
            ],
            line_text: vec![
                "fn map<U, F>(".into(),
                "    self,".into(),
                "    f: F,".into(),
                ") -> Option<U>".into(),
            ],
            line_scales: vec![],
            scroll_top: 0,
            max_visible_rows: 8,
            has_focus: false,
            selection: None,
            links: vec![],
            focused_link: None,
            placement: PopupPlacement::Above,
            padding: 1.0,
            fg: None,
            bg: None,
        }
    }

    fn layout_for(
        popup: &RichTextPopup,
        viewport: QRect,
        line_height: f32,
        char_width: f32,
    ) -> RichTextPopupLayout {
        let measure = crate::primitives::rich_text_popup::RichTextPopupMeasure::new(
            char_width * 30.0,
            line_height,
        );
        popup.layout(100.0, 150.0, viewport, measure, |_, _, _| 0.0)
    }

    fn paint(popup: &RichTextPopup, layout: &RichTextPopupLayout) -> BitmapSurface {
        let surface = BitmapSurface::new(W, H);
        surface.fill(0.0, 0.0, 0.0, 0.0);
        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            b.draw_rich_text_popup(popup, layout);
        });
        backend.end_frame();
        surface
    }

    #[test]
    fn popup_paints_hover_bg() {
        let popup = sample_popup();
        let viewport = QRect::new(0.0, 0.0, W as f32, H as f32);
        let layout = layout_for(&popup, viewport, 16.0, 8.4);
        let surface = paint(&popup, &layout);
        let theme = Theme::default();
        // Probe inside the content area, away from any line glyphs
        // (right edge of the popup).
        let b = layout.bounds;
        let (r, g, bp, _) =
            surface.pixel((b.x + b.width - 4.0) as u32, (b.y + b.height - 4.0) as u32);
        assert_eq!(
            (r, g, bp),
            (theme.hover_bg.r, theme.hover_bg.g, theme.hover_bg.b),
        );
    }

    #[test]
    fn focused_popup_border_paints_differently_than_unfocused() {
        // When `has_focus` is true the border should be drawn in
        // `theme.link_fg` instead of `theme.hover_border`. We probe
        // the same edge pixel in both states and assert the values
        // differ — sufficient to prove the conditional fires without
        // needing to disambiguate AA-blended pixel values.
        let viewport = QRect::new(0.0, 0.0, W as f32, H as f32);
        let mut unfocused = sample_popup();
        unfocused.has_focus = false;
        let mut focused = sample_popup();
        focused.has_focus = true;
        let layout_u = layout_for(&unfocused, viewport, 16.0, 8.4);
        let layout_f = layout_for(&focused, viewport, 16.0, 8.4);
        let surface_u = paint(&unfocused, &layout_u);
        let surface_f = paint(&focused, &layout_f);
        let bx = layout_u.bounds.x as u32;
        let by = layout_u.bounds.y as u32;
        let bw = layout_u.bounds.width as u32;
        // Right edge of the top border, away from line glyphs.
        let pu = surface_u.pixel(bx + bw - 4, by);
        let pf = surface_f.pixel(bx + bw - 4, by);
        assert_ne!(
            (pu.0, pu.1, pu.2),
            (pf.0, pf.1, pf.2),
            "focused vs unfocused border should paint different colours",
        );
    }
}
