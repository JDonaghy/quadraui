//! macOS rasteriser for [`crate::CommandCenter`].
//!
//! Mirrors [`crate::gtk::command_center`]: back/forward arrows
//! (`◀` / `▶`) and a rounded-rect bordered search box, centred within
//! the given area. Returns [`CommandCenterLayout`] for caller click
//! dispatch.
//!
//! ## Scope omissions (follow-up)
//!
//! - **Rounded search-box border** — GTK strokes a 4 px-radius
//!   rounded rect. macOS for #38 draws a 1-pt straight rectangle
//!   border (CG path API needed for proper rounded corners). Visual
//!   parity tracked separately; geometry + hit regions identical.

use core_graphics::geometry::CGRect;
use core_graphics::sys::CGContextRef;
use core_text::font::CTFont;

use super::text::{draw_text, measure_text};
use crate::event::Rect as QRect;
use crate::primitives::command_center::{CommandCenter, CommandCenterLayout, CommandCenterMeasure};
use crate::theme::Theme;
use crate::types::Color;

const ARROW_WIDTH_PX: f32 = 24.0;
const GAP_PX: f32 = 8.0;
const SEARCH_PAD_PX: f64 = 8.0;
const SEARCH_MIN_WIDTH: f32 = 280.0;

/// Compute the pixel-unit layout without painting. Caller routes
/// clicks against this when it needs the same layout the rasteriser
/// used.
pub fn mac_command_center_layout(
    cc: &CommandCenter,
    font: &CTFont,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
) -> CommandCenterLayout {
    let search_w = if cc.search_label.is_empty() {
        0.0
    } else {
        let (text_w, _) = measure_text(font, &cc.search_label);
        (text_w as f32 + SEARCH_PAD_PX as f32 * 2.0).max(SEARCH_MIN_WIDTH)
    };
    cc.layout(
        QRect::new(x as f32, y as f32, w as f32, h as f32),
        CommandCenterMeasure {
            arrow_width: ARROW_WIDTH_PX,
            gap: GAP_PX,
            search_box_width: search_w,
            height: h as f32,
        },
    )
}

/// Paint `cc` into `(x, y, w, h)` on `ctx`. Returns the layout.
///
/// # Safety
///
/// `ctx` must be a valid `CGContextRef` borrowed for the duration of
/// the call.
#[allow(clippy::too_many_arguments)]
pub unsafe fn draw_command_center(
    ctx: CGContextRef,
    font: &CTFont,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    cc: &CommandCenter,
    theme: &Theme,
    line_height: f64,
) -> CommandCenterLayout {
    let layout = mac_command_center_layout(cc, font, x, y, w, h);

    CGContextSaveGState(ctx);
    fill_rect(ctx, x, y, w, h, theme.tab_bar_bg);

    let enabled_fg = theme.tab_inactive_fg;
    let disabled_fg = theme.muted_fg;
    let text_y = y + (h - line_height) / 2.0;

    if let Some(bb) = layout.back_bounds {
        let fg = if cc.back_enabled {
            enabled_fg
        } else {
            disabled_fg
        };
        let (tw, _) = measure_text(font, "◀");
        draw_text(
            ctx,
            font,
            "◀",
            bb.x as f64 + (bb.width as f64 - tw) / 2.0,
            text_y,
            color_to_cg(fg),
        );
    }
    if let Some(fb) = layout.forward_bounds {
        let fg = if cc.forward_enabled {
            enabled_fg
        } else {
            disabled_fg
        };
        let (tw, _) = measure_text(font, "▶");
        draw_text(
            ctx,
            font,
            "▶",
            fb.x as f64 + (fb.width as f64 - tw) / 2.0,
            text_y,
            color_to_cg(fg),
        );
    }

    if let Some(sb) = layout.search_bounds {
        let bx = sb.x as f64;
        let by = sb.y as f64 + 2.0;
        let bw = sb.width as f64;
        let bh = sb.height as f64 - 4.0;

        // Border — straight 1-pt outline. CG path API needed for
        // rounded corners; deferred (see module header).
        stroke_rect(ctx, bx, by, bw, bh, theme.separator, 1.0);

        // Search label.
        draw_text(
            ctx,
            font,
            &cc.search_label,
            bx + SEARCH_PAD_PX,
            text_y,
            color_to_cg(theme.tab_inactive_fg),
        );
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
    fn CGContextSaveGState(c: CGContextRef);
    fn CGContextRestoreGState(c: CGContextRef);
    fn CGContextSetRGBFillColor(
        c: CGContextRef,
        red: core_graphics::base::CGFloat,
        green: core_graphics::base::CGFloat,
        blue: core_graphics::base::CGFloat,
        alpha: core_graphics::base::CGFloat,
    );
    fn CGContextFillRect(c: CGContextRef, rect: CGRect);
    fn CGContextSetRGBStrokeColor(
        c: CGContextRef,
        red: core_graphics::base::CGFloat,
        green: core_graphics::base::CGFloat,
        blue: core_graphics::base::CGFloat,
        alpha: core_graphics::base::CGFloat,
    );
    fn CGContextSetLineWidth(c: CGContextRef, w: core_graphics::base::CGFloat);
    fn CGContextStrokeRect(c: CGContextRef, rect: CGRect);
}

#[cfg(test)]
mod tests {
    use super::super::headless::BitmapSurface;
    use super::super::text::make_font;
    use super::super::MacBackend;
    use super::*;
    use crate::event::Viewport;
    use crate::primitives::command_center::CommandCenterHit;
    use crate::theme::Theme;
    use crate::types::WidgetId;
    use crate::Backend;

    const W: u32 = 480;
    const H: u32 = 32;

    fn font() -> CTFont {
        make_font("Menlo", 14.0).expect("Menlo installed")
    }

    fn sample_cc() -> CommandCenter {
        CommandCenter {
            id: WidgetId::new("cc"),
            back_enabled: true,
            forward_enabled: false,
            search_label: "project".into(),
        }
    }

    fn paint_via_backend(cc: &CommandCenter) -> (BitmapSurface, CommandCenterLayout) {
        let surface = BitmapSurface::new(W, H);
        surface.fill(0.0, 0.0, 0.0, 0.0);
        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        let layout = std::cell::RefCell::new(None);
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            let l = b.draw_command_center(QRect::new(0.0, 0.0, W as f32, H as f32), cc);
            *layout.borrow_mut() = Some(l);
        });
        backend.end_frame();
        (surface, layout.into_inner().unwrap())
    }

    #[test]
    fn background_is_tab_bar_bg() {
        let cc = sample_cc();
        let (surface, _) = paint_via_backend(&cc);
        let theme = Theme::default();
        // Probe column 0 (well before any arrow) at mid-height.
        let (r, g, b, _) = surface.pixel(0, H / 2);
        assert_eq!(
            (r, g, b),
            (theme.tab_bar_bg.r, theme.tab_bar_bg.g, theme.tab_bar_bg.b),
        );
    }

    #[test]
    fn layout_centres_content_in_bounds() {
        let cc = sample_cc();
        let (_surface, layout) = paint_via_backend(&cc);
        let back = layout.back_bounds.expect("back arrow has bounds");
        let search = layout.search_bounds.expect("search bounds present");
        // Content_width = 24 + 8 + 24 + 8 + max(280, ...) ≥ 344. With
        // W=480 the centred left edge sits at (480 - content) / 2.
        // back.x must be > 0 and < W/2.
        assert!(back.x > 0.0 && back.x < (W as f32) / 2.0);
        // Search box ends at or before W.
        assert!(search.x + search.width <= W as f32);
    }

    #[test]
    fn hit_test_resolves_back_forward_search() {
        let cc = sample_cc();
        let (_surface, layout) = paint_via_backend(&cc);
        let back = layout.back_bounds.unwrap();
        let fwd = layout.forward_bounds.unwrap();
        let search = layout.search_bounds.unwrap();

        assert_eq!(
            layout.hit_test(back.x + 1.0, back.y + 1.0),
            CommandCenterHit::Back,
        );
        assert_eq!(
            layout.hit_test(fwd.x + 1.0, fwd.y + 1.0),
            CommandCenterHit::Forward,
        );
        assert_eq!(
            layout.hit_test(search.x + 5.0, search.y + 5.0),
            CommandCenterHit::SearchBox,
        );
    }

    #[test]
    fn empty_search_label_omits_search_bounds() {
        let cc = CommandCenter {
            search_label: "".into(),
            ..sample_cc()
        };
        let (_surface, layout) = paint_via_backend(&cc);
        assert!(layout.search_bounds.is_none());
        assert!(layout.back_bounds.is_some());
        assert!(layout.forward_bounds.is_some());
    }
}
