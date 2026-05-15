//! macOS rasteriser for [`crate::StatusBar`].
//!
//! Mirrors [`crate::gtk::status_bar::draw_status_bar`]: the rasteriser
//! computes the layout internally because Core Text measurement and
//! glyph rendering both require the same `CTFont` handle, so splitting
//! the work across the call boundary would force callers to plumb the
//! font through twice. The resolved [`StatusBarLayout`] is returned so
//! the host (or the [`Backend`] adapter on top) can dispatch clicks.
//!
//! Per D6: layout policy (priority drop, gap rules, …) lives in
//! [`StatusBar::layout`]; this rasteriser paints whatever that returns.
//!
//! ## Bold segments
//!
//! Tracked separately — `bold` on a segment is currently ignored. Bold
//! support requires materialising a bold variant of the active font
//! via `CTFontCreateCopyWithSymbolicTraits` and is out of scope for
//! #38; follow-up after the chrome batch lands.
//!
//! [`Backend`]: crate::Backend

use core_graphics::geometry::CGRect;
use core_graphics::sys::CGContextRef;
use core_text::font::CTFont;

use super::text::{draw_text, measure_text};
use crate::primitives::status_bar::{
    StatusBar, StatusBarLayout, StatusBarSegment, StatusSegmentMeasure, StatusSegmentSide,
};
use crate::theme::Theme;
use crate::types::{Color, WidgetId};

/// 16-point minimum gap between left and right segment groups. Matches
/// the GTK rasteriser and the vimcode native-backend behaviour.
const MIN_GAP_PX: f32 = 16.0;

/// Paint `bar` into the rect `(x, y, width, line_height)` on `ctx`
/// using `font` for text measurement + glyph rendering. Returns the
/// resolved layout — hit regions are in **bar-local coordinates**
/// (relative to `x`), matching `gtk::draw_status_bar`.
///
/// # Safety
///
/// `ctx` must be a valid `CGContextRef` borrowed for the duration of
/// the call (typical: the frame-scope pointer stashed on
/// [`super::MacBackend`]). Calling with a freed or null pointer is UB.
#[allow(clippy::too_many_arguments)]
pub unsafe fn draw_status_bar(
    ctx: CGContextRef,
    font: &CTFont,
    x: f64,
    y: f64,
    width: f64,
    line_height: f64,
    bar: &StatusBar,
    theme: &Theme,
    hovered_id: Option<&WidgetId>,
    pressed_id: Option<&WidgetId>,
) -> StatusBarLayout {
    // Empty rect — return the layout the primitive would have produced
    // for a zero-width bar so callers' hit dispatch behaves predictably.
    if width <= 0.0 || line_height <= 0.0 {
        return bar.layout(
            width.max(0.0) as f32,
            line_height.max(0.0) as f32,
            MIN_GAP_PX,
            |_| StatusSegmentMeasure::new(0.0),
        );
    }

    CGContextSaveGState(ctx);

    // Clip to the bar rect so right-aligned segments that overflow are
    // truncated cleanly at the right edge instead of painting past it.
    CGContextClipToRect(ctx, CGRect::new_xywh(x, y, width, line_height));

    // Background fill: first segment's bg, falling through to theme bg.
    let fill = bar
        .left_segments
        .first()
        .or(bar.right_segments.first())
        .map(|s| s.bg)
        .unwrap_or(theme.background);
    fill_rect(ctx, x, y, width, line_height, fill);

    // Measure each visible segment via Core Text. `measure_text` returns
    // (width, height) in points; the primitive only needs the width.
    let measure = |seg: &StatusBarSegment| -> StatusSegmentMeasure {
        let (w, _) = measure_text(font, &seg.text);
        StatusSegmentMeasure::new(w as f32)
    };
    let bar_layout = bar.layout(width as f32, line_height as f32, MIN_GAP_PX, measure);

    for vs in &bar_layout.visible_segments {
        let seg = match vs.side {
            StatusSegmentSide::Left => &bar.left_segments[vs.segment_idx],
            StatusSegmentSide::Right => &bar.right_segments[vs.segment_idx],
        };
        let seg_x = x + vs.bounds.x as f64;
        let seg_w = vs.bounds.width as f64;

        // Hover/press tint — applied only to interactive segments to
        // match the TUI + GTK convention. `action_id.is_some_and(...)`
        // is false for both non-clickable segments and segments whose
        // id doesn't match `hovered_id` / `pressed_id`.
        let effective_bg = if seg
            .action_id
            .as_ref()
            .is_some_and(|id| Some(id) == pressed_id)
        {
            seg.bg.darken(0.05)
        } else if seg
            .action_id
            .as_ref()
            .is_some_and(|id| Some(id) == hovered_id)
        {
            seg.bg.lighten(0.05)
        } else {
            seg.bg
        };
        fill_rect(ctx, seg_x, y, seg_w, line_height, effective_bg);

        let fg = color_to_cg(seg.fg);
        draw_text(ctx, font, &seg.text, seg_x, y, fg);
    }

    CGContextRestoreGState(ctx);

    bar_layout
}

/// Convert a `quadraui::Color` (0–255 RGBA) into CG's normalised
/// `(r, g, b, a)` tuple expected by [`super::text::draw_text`] and
/// the local `fill_rect`.
fn color_to_cg(c: Color) -> (f64, f64, f64, f64) {
    (
        c.r as f64 / 255.0,
        c.g as f64 / 255.0,
        c.b as f64 / 255.0,
        c.a as f64 / 255.0,
    )
}

/// Set the fill colour and emit a `CGContextFillRect`. Convenience for
/// the rasteriser's repeated background-fill pattern.
///
/// # Safety
///
/// Same contract as the caller — `ctx` must be a valid CG context
/// borrowed for the duration of the call.
unsafe fn fill_rect(ctx: CGContextRef, x: f64, y: f64, w: f64, h: f64, c: Color) {
    let (r, g, b, a) = color_to_cg(c);
    CGContextSetRGBFillColor(ctx, r, g, b, a);
    CGContextFillRect(ctx, CGRect::new_xywh(x, y, w, h));
}

// Small convenience extension for building `CGRect` in (x, y, w, h)
// form. Keeps call sites scannable.
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
    fn CGContextFillRect(c: CGContextRef, rect: CGRect);
}

#[cfg(test)]
mod tests {
    use super::super::headless::BitmapSurface;
    use super::super::text::make_font;
    use super::super::MacBackend;
    use super::*;
    use crate::event::{Rect as QRect, Viewport};
    use crate::primitives::status_bar::StatusBarHit;
    use crate::primitives::status_bar::StatusBarSegment;
    use crate::theme::Theme;
    use crate::types::{Color, WidgetId};
    use crate::Backend;

    const W: u32 = 320;
    const H: u32 = 24;
    const FONT_SIZE: f64 = 14.0;

    fn font() -> CTFont {
        make_font("Menlo", FONT_SIZE).expect("Menlo installed on every macOS host")
    }

    /// Three-segment bar:
    /// - Left[0]: "(L) " sentinel with bg `(10, 20, 30)` — sets the
    ///   bar fill (first segment's bg) to a colour distinct from the
    ///   clickable segment, so paint-shift mutations can't hide
    ///   behind a matching bar fill.
    /// - Left[1]: " Save " clickable, bg `(40, 80, 120)`.
    /// - Right[0]: " 1:1 " non-clickable, bg `(40, 80, 120)`.
    ///
    /// Leading/trailing spaces give glyph-free padding pixels for
    /// bg-colour probing — same trick the TUI reference tests use.
    fn sample_bar() -> StatusBar {
        StatusBar {
            id: WidgetId::new("status"),
            left_segments: vec![
                StatusBarSegment {
                    text: "(L) ".into(),
                    fg: Color::rgb(255, 255, 255),
                    bg: Color::rgb(10, 20, 30),
                    bold: false,
                    action_id: None,
                },
                StatusBarSegment {
                    text: " Save ".into(),
                    fg: Color::rgb(255, 255, 255),
                    bg: Color::rgb(40, 80, 120),
                    bold: false,
                    action_id: Some(WidgetId::new("status:save")),
                },
            ],
            right_segments: vec![StatusBarSegment {
                text: " 1:1 ".into(),
                fg: Color::rgb(255, 255, 255),
                bg: Color::rgb(40, 80, 120),
                bold: false,
                action_id: None,
            }],
        }
    }

    /// Paint a bar through the full `MacBackend::draw_status_bar`
    /// path and return both the surface (for pixel inspection) and
    /// the layout (for hit_test). Establishes the harness shape every
    /// chrome rasteriser test follows.
    fn paint_via_backend(bar: &StatusBar) -> (BitmapSurface, StatusBarLayout) {
        let surface = BitmapSurface::new(W, H);
        // Clear so we can distinguish "painted by status_bar" from
        // "untouched memory" cleanly.
        surface.fill(0.0, 0.0, 0.0, 0.0);

        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));

        let layout = std::cell::RefCell::new(None);
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            let l = b.draw_status_bar(QRect::new(0.0, 0.0, W as f32, H as f32), bar, None, None);
            *layout.borrow_mut() = Some(l);
        });
        backend.end_frame();
        (surface, layout.into_inner().unwrap())
    }

    /// Probe a column near the leading space of the clickable "Save"
    /// segment — a glyph-free padding region that exposes the
    /// segment's bg fill without interference from rasterised text.
    /// Sample near the top edge where line_height is guaranteed
    /// painted and glyphs (anchored at the ascent baseline) don't
    /// reach.
    fn probe_save_segment_bg(surface: &BitmapSurface, layout: &StatusBarLayout) -> (u8, u8, u8) {
        let save = layout
            .visible_segments
            .iter()
            .filter(|vs| vs.side == StatusSegmentSide::Left)
            .nth(1)
            .expect("save segment visible");
        let probe_x = (save.bounds.x as u32) + 1;
        let probe_y = 2;
        let (r, g, b, _) = surface.pixel(probe_x, probe_y);
        (r, g, b)
    }

    #[test]
    fn paints_segment_backgrounds() {
        let bar = sample_bar();
        let (surface, layout) = paint_via_backend(&bar);
        assert_eq!(
            probe_save_segment_bg(&surface, &layout),
            (40, 80, 120),
            "left-segment bg should match StatusBarSegment.bg",
        );
    }

    #[test]
    fn round_trip_click_hits_clickable_segment() {
        // Paint the bar, then hit-test a coordinate inside the
        // clickable "Save" segment's painted bounds. Assert the layout
        // reports a hit on the segment's action_id.
        let bar = sample_bar();
        let (_surface, layout) = paint_via_backend(&bar);

        let save = layout
            .visible_segments
            .iter()
            .filter(|vs| vs.side == StatusSegmentSide::Left)
            .nth(1)
            .expect("save segment visible");
        let hit_x = save.bounds.x + save.bounds.width * 0.5;
        let hit_y = save.bounds.y + save.bounds.height * 0.5;
        let hit = layout.hit_test(hit_x, hit_y);
        assert_eq!(
            hit,
            StatusBarHit::Segment(WidgetId::new("status:save")),
            "expected clickable Save segment hit at ({}, {})",
            hit_x,
            hit_y,
        );

        // Sanity check: the right segment is non-clickable, so a hit
        // inside its bounds must return Empty.
        let right = layout
            .visible_segments
            .iter()
            .find(|vs| vs.side == StatusSegmentSide::Right)
            .expect("right segment visible");
        let right_hit = layout.hit_test(
            right.bounds.x + right.bounds.width * 0.5,
            right.bounds.y + right.bounds.height * 0.5,
        );
        assert_eq!(
            right_hit,
            StatusBarHit::Empty,
            "non-clickable right segment must hit Empty",
        );
    }

    #[test]
    fn empty_bar_falls_back_to_theme_background() {
        // No segments → fill colour comes from theme.background.
        let bar = StatusBar {
            id: WidgetId::new("empty"),
            left_segments: vec![],
            right_segments: vec![],
        };
        let surface = BitmapSurface::new(W, H);
        surface.fill(0.0, 0.0, 0.0, 0.0);

        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        backend.set_current_theme(Theme {
            background: Color::rgb(1, 2, 3),
            ..Theme::default()
        });
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            b.draw_status_bar(QRect::new(0.0, 0.0, W as f32, H as f32), &bar, None, None);
        });
        backend.end_frame();

        // Every pixel should carry the theme bg.
        let (r, g, b, _) = surface.pixel(W / 2, H / 2);
        assert_eq!(
            (r, g, b),
            (1, 2, 3),
            "empty bar should be filled with theme.background",
        );
    }

    #[test]
    fn hover_tint_lightens_clickable_segment_bg() {
        // Paint with `hovered_id = "status:save"` and assert the bg
        // sample comes back lighter than the un-tinted version.
        let bar = sample_bar();

        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));

        let surface = BitmapSurface::new(W, H);
        surface.fill(0.0, 0.0, 0.0, 0.0);
        let hovered = WidgetId::new("status:save");
        let layout = std::cell::RefCell::new(None);
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            let l = b.draw_status_bar(
                QRect::new(0.0, 0.0, W as f32, H as f32),
                &bar,
                Some(&hovered),
                None,
            );
            *layout.borrow_mut() = Some(l);
        });
        backend.end_frame();

        let layout = layout.into_inner().unwrap();
        let (r, g, b) = probe_save_segment_bg(&surface, &layout);
        // Base colour is (40, 80, 120). `lighten(0.05)` moves each
        // channel 5% of the way to 255. Each channel should be
        // strictly greater than the base.
        assert!(
            r > 40 && g > 80 && b > 120,
            "hover-tinted bg ({}, {}, {}) should be lighter than base (40, 80, 120)",
            r,
            g,
            b,
        );
    }
}
