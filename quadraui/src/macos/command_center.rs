//! macOS rasteriser for [`crate::CommandCenter`].
//!
//! Mirrors [`crate::gtk::command_center`]'s geometry and hit regions:
//! back/forward chevrons and a rounded-rect search box, centred within
//! the given area. Returns [`CommandCenterLayout`] for caller click
//! dispatch.
//!
//! Styling leans deliberately subtle, matching VS Code's title-bar
//! command centre rather than a high-contrast form control: the search
//! box is a low-alpha tint of [`Theme::separator`] with a thin,
//! low-alpha border of the same colour (both painted through
//! [`super::backend::ns_fill_rounded_rect`] /
//! [`super::backend::ns_stroke_rounded_rect`]), a small dim codicon
//! magnifier ([`crate::codicon::SEARCH`]) sits ahead of the label, and
//! the back/forward glyphs are codicon chevrons
//! ([`crate::codicon::CHEVRON_LEFT`] / [`crate::codicon::CHEVRON_RIGHT`])
//! rather than full-weight Unicode triangles — `font` is always built
//! through [`super::text::font_with_builtin_fallback`], so its cascade
//! list already carries the bundled codicon font, and these glyphs
//! resolve the same way [`super::tab_bar`]'s close/dirty glyphs do.

use core_graphics::geometry::CGRect;
use core_graphics::sys::CGContextRef;
use core_text::font::CTFont;

use super::backend::{ns_fill_rounded_rect, ns_stroke_rounded_rect};
use super::text::{draw_text, measure_text};
use crate::codicon;
use crate::event::Rect as QRect;
use crate::primitives::command_center::{CommandCenter, CommandCenterLayout, CommandCenterMeasure};
use crate::theme::Theme;
use crate::types::Color;

const ARROW_WIDTH_PX: f32 = 24.0;
const GAP_PX: f32 = 8.0;
const SEARCH_PAD_PX: f64 = 8.0;
const SEARCH_MIN_WIDTH: f32 = 280.0;

/// Corner radius of the search box's rounded rect — matches
/// `gtk::command_center::GTK_CORNER_RADIUS` so the box reads the same
/// size on both pixel backends.
const SEARCH_CORNER_RADIUS: f32 = 4.0;

/// Alpha (of 255) the search box's fill uses, applied to
/// [`Theme::separator`]. Low enough that the box reads as a faint tint
/// of the bar background rather than a distinct form control — VS
/// Code's own title-bar search box has the same barely-there fill.
const SEARCH_FILL_ALPHA: u8 = 22;

/// Alpha (of 255) the search box's border uses, applied to
/// [`Theme::separator`]. Thin and low-contrast rather than the bar's
/// full-opacity separator colour.
const SEARCH_BORDER_ALPHA: u8 = 130;

/// Point size of the small magnifier glyph inside the search box,
/// deliberately smaller than `font`'s own size so it reads as an icon
/// rather than competing with the label text.
const SEARCH_ICON_SIZE_PT: f64 = 12.0;

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
        let glyph = codicon::CHEVRON_LEFT.to_string();
        let (tw, _) = measure_text(font, &glyph);
        draw_text(
            ctx,
            font,
            &glyph,
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
        let glyph = codicon::CHEVRON_RIGHT.to_string();
        let (tw, _) = measure_text(font, &glyph);
        draw_text(
            ctx,
            font,
            &glyph,
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
        let box_rect = QRect::new(bx as f32, by as f32, bw as f32, bh as f32);

        // Subtle rounded fill + thin rounded border, both a low-alpha
        // tint of the separator colour — a faint box rather than a
        // high-contrast form control.
        let fill = Color::rgba(
            theme.separator.r,
            theme.separator.g,
            theme.separator.b,
            SEARCH_FILL_ALPHA,
        );
        let border = Color::rgba(
            theme.separator.r,
            theme.separator.g,
            theme.separator.b,
            SEARCH_BORDER_ALPHA,
        );
        ns_fill_rounded_rect(ctx, box_rect, SEARCH_CORNER_RADIUS, fill);
        ns_stroke_rounded_rect(ctx, box_rect, SEARCH_CORNER_RADIUS, border, 1.0);

        // Small dim magnifier glyph ahead of the label. `clone_with_font_size`
        // keeps `font`'s own cascade list (including the bundled codicon
        // fallback), only the point size changes.
        let icon_font = font.clone_with_font_size(SEARCH_ICON_SIZE_PT);
        let icon_glyph = codicon::SEARCH.to_string();
        let (icon_w, _) = measure_text(&icon_font, &icon_glyph);
        let icon_x = bx + SEARCH_PAD_PX;
        let icon_y = by + (bh - SEARCH_ICON_SIZE_PT) / 2.0 + 1.0;
        draw_text(
            ctx,
            &icon_font,
            &icon_glyph,
            icon_x,
            icon_y,
            color_to_cg(disabled_fg),
        );

        // Search label.
        draw_text(
            ctx,
            font,
            &cc.search_label,
            icon_x + icon_w + SEARCH_PAD_PX,
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
    fn hit_test_resolves_back_forward_search_at_nonzero_origin() {
        // Layout helpers must return coords in the same frame across backends:
        // `mac_command_center_layout` bakes `x`/`y` straight into the bounds
        // (absolute frame, matching the GTK/TUI twins). Test non-zero origin.
        let cc = sample_cc();
        let f = font();
        let layout = mac_command_center_layout(&cc, &f, 7.0, 13.0, W as f64, H as f64);
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

    /// `cargo test -p quadraui --features macos -- --ignored --nocapture macos::command_center::tests::dump_smoke_ppm`
    ///
    /// Paints the sample CC (back enabled, forward disabled, "project"
    /// search) into a 480 × 32 surface and writes
    /// `/tmp/quadraui_command_center.ppm`. Open in Preview to confirm:
    /// - Back chevron is bright (tab_inactive_fg); forward chevron is
    ///   dim (muted_fg) — both codicon glyphs, not Unicode triangles.
    /// - Search box is a faint rounded rect (low-alpha separator fill
    ///   and border, not a hard-edged high-contrast outline) with a
    ///   small dim magnifier glyph ahead of the "project" label.
    /// - Whole strip is centred horizontally within the 480-wide bar.
    #[test]
    #[ignore = "writes /tmp/quadraui_command_center.ppm — opt in with --ignored"]
    fn dump_smoke_ppm() {
        let cc = sample_cc();
        let (surface, _) = paint_via_backend(&cc);
        surface.write_ppm_and_open("/tmp/quadraui_command_center.ppm");
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

    #[test]
    fn search_box_is_rounded_and_the_fill_is_a_subtle_tint() {
        let cc = sample_cc();
        let (surface, layout) = paint_via_backend(&cc);
        let theme = Theme::default();
        let search = layout.search_bounds.expect("search bounds present");

        // A corner of the box's bounding rect sits outside a radius-4
        // fillet (distance from the fillet centre is ~5.7px, past the
        // 4px radius) — background must show through there, or this is
        // just a plain rectangle under a new name. CoreGraphics
        // antialiases the fillet's curve against the sub-pixel box
        // coordinates, so the sampled corner can carry a faint AA fringe
        // (a couple of units) even when fully outside the fill; a corner
        // mistakenly inside a *plain* rectangle fill would instead land
        // near a full-alpha blend of the fill colour, many units away.
        // Tolerance distinguishes the two without hardcoding either
        // theme's exact RGB separation.
        let corner_x = search.x.round() as u32;
        let corner_y = (search.y + 2.0).round() as u32;
        let (r, g, b, _) = surface.pixel(corner_x, corner_y);
        let corner_dist = (r as i32 - theme.tab_bar_bg.r as i32).abs()
            + (g as i32 - theme.tab_bar_bg.g as i32).abs()
            + (b as i32 - theme.tab_bar_bg.b as i32).abs();
        assert!(
            corner_dist <= 4,
            "a corner outside the radius-4 fillet must stay (almost) untouched by the fill: \
             got {:?}, background is {:?} (dist={corner_dist})",
            (r, g, b),
            (theme.tab_bar_bg.r, theme.tab_bar_bg.g, theme.tab_bar_bg.b)
        );

        // The centre of the box is tinted, but only faintly: the issue
        // asks for a subtle, low-contrast box rather than a solid block
        // of the separator colour, so the blended pixel must land much
        // closer to the bar background than a full-opacity separator
        // fill would.
        let cx = (search.x + search.width / 2.0).round() as u32;
        let cy = (search.y + search.height / 2.0).round() as u32;
        let (r, g, b, _) = surface.pixel(cx, cy);
        let bg_dist = (r as i32 - theme.tab_bar_bg.r as i32).abs()
            + (g as i32 - theme.tab_bar_bg.g as i32).abs()
            + (b as i32 - theme.tab_bar_bg.b as i32).abs();
        let full_separator_dist = (theme.separator.r as i32 - theme.tab_bar_bg.r as i32).abs()
            + (theme.separator.g as i32 - theme.tab_bar_bg.g as i32).abs()
            + (theme.separator.b as i32 - theme.tab_bar_bg.b as i32).abs();
        assert!(
            bg_dist > 0 && bg_dist < full_separator_dist,
            "search box fill must be visibly tinted (bg_dist={bg_dist}) but far short of a \
             full-opacity separator block (full_separator_dist={full_separator_dist})"
        );
    }
}
