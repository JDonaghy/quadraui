//! macOS (Core Graphics) rasteriser for [`crate::DropOverlay`].
//!
//! Port of [`crate::gtk::drop_overlay::draw_drop_overlay`]:
//!
//! - Highlight: semi-transparent (15%) filled rect in `theme.accent_fg`.
//! - Insertion bar: solid rect in `theme.accent_fg`, at least 2 points wide.
//!
//! `DropOverlay::ghost_position` is not rendered — neither GTK nor TUI
//! paints a ghost label either, so this is parity, not a macOS gap.

use core_graphics::geometry::CGRect;
use core_graphics::sys::CGContextRef;

use crate::primitives::drop_zone::DropOverlay;
use crate::theme::Theme;

/// Alpha applied to the highlight tint. Matches the GTK twin's `0.15`.
const HIGHLIGHT_ALPHA: f64 = 0.15;
/// Minimum insertion-bar thickness in points.
const MIN_BAR_W: f64 = 2.0;

/// Paint `overlay` on top of the current frame.
///
/// # Safety
///
/// `ctx` must be a valid `CGContextRef` borrowed for the duration of the
/// call (typical: the frame-scope pointer stashed on [`super::MacBackend`]).
/// Calling with a freed or null pointer is UB.
pub unsafe fn draw_drop_overlay(ctx: CGContextRef, overlay: &DropOverlay, theme: &Theme) {
    let a = theme.accent_fg;
    let (ar, ag, ab) = (a.r as f64 / 255.0, a.g as f64 / 255.0, a.b as f64 / 255.0);

    if let Some(h) = overlay.highlight {
        if h.width > 0.0 && h.height > 0.0 {
            CGContextSetRGBFillColor(ctx, ar, ag, ab, HIGHLIGHT_ALPHA);
            CGContextFillRect(
                ctx,
                CGRect::new_xywh(h.x as f64, h.y as f64, h.width as f64, h.height as f64),
            );
        }
    }

    if let Some(bar) = overlay.insertion_bar {
        if bar.height > 0.0 {
            CGContextSetRGBFillColor(ctx, ar, ag, ab, 1.0);
            CGContextFillRect(
                ctx,
                CGRect::new_xywh(
                    bar.x as f64,
                    bar.y as f64,
                    (bar.width as f64).max(MIN_BAR_W),
                    bar.height as f64,
                ),
            );
        }
    }
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
    use crate::Backend;

    const W: u32 = 200;
    const H: u32 = 120;

    fn paint_via_backend(overlay: &DropOverlay) -> BitmapSurface {
        let surface = BitmapSurface::new(W, H);
        // Opaque black base so the 15% tint composites to a predictable,
        // non-white value.
        surface.fill(0.0, 0.0, 0.0, 1.0);
        let mut backend = MacBackend::new();
        backend.set_current_font(make_font("Menlo", 14.0).expect("Menlo installed"));
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            b.draw_drop_overlay(overlay);
        });
        backend.end_frame();
        surface
    }

    /// The impl this replaced had an empty body — the whole frame stayed
    /// exactly as the caller left it (quadraui#484 §4).
    #[test]
    fn highlight_tints_the_target_rect() {
        let overlay = DropOverlay {
            highlight: Some(QRect::new(20.0, 20.0, 60.0, 40.0)),
            insertion_bar: None,
            ghost_position: None,
        };
        let surface = paint_via_backend(&overlay);
        let theme = Theme::default();

        let (r, g, b, _) = surface.pixel(50, 40);
        assert_ne!(
            (r, g, b),
            (0, 0, 0),
            "the highlight must actually tint the surface, not no-op",
        );
        // 15% of accent over black — each channel lands near 0.15 * accent.
        let expect = |c: u8| (c as f64 * HIGHLIGHT_ALPHA).round() as i32;
        for (got, want, name) in [
            (r as i32, expect(theme.accent_fg.r), "r"),
            (g as i32, expect(theme.accent_fg.g), "g"),
            (b as i32, expect(theme.accent_fg.b), "b"),
        ] {
            assert!(
                (got - want).abs() <= 2,
                "{name} channel {got} should be within 2 of the composited {want}",
            );
        }

        // Outside the highlight rect: untouched.
        assert_eq!(
            {
                let (r, g, b, _) = surface.pixel(5, 5);
                (r, g, b)
            },
            (0, 0, 0),
            "nothing should paint outside the highlight rect",
        );
    }

    #[test]
    fn insertion_bar_paints_solid_accent() {
        let overlay = DropOverlay {
            highlight: None,
            insertion_bar: Some(QRect::new(100.0, 10.0, 2.0, 80.0)),
            ghost_position: None,
        };
        let surface = paint_via_backend(&overlay);
        let theme = Theme::default();
        let (r, g, b, _) = surface.pixel(100, 50);
        assert_eq!(
            (r, g, b),
            (theme.accent_fg.r, theme.accent_fg.g, theme.accent_fg.b),
            "the insertion bar is opaque accent",
        );
    }

    /// Non-zero-origin guard: the overlay carries absolute rects, so a
    /// bar at x=140 must paint at x=140 and nowhere else.
    #[test]
    fn insertion_bar_honours_a_nonzero_origin() {
        let overlay = DropOverlay {
            highlight: None,
            insertion_bar: Some(QRect::new(140.0, 33.0, 0.0, 40.0)),
            ghost_position: None,
        };
        let surface = paint_via_backend(&overlay);
        let theme = Theme::default();
        let accent = (theme.accent_fg.r, theme.accent_fg.g, theme.accent_fg.b);

        // Zero-width bars are widened to MIN_BAR_W, matching GTK.
        let (r, g, b, _) = surface.pixel(140, 50);
        assert_eq!((r, g, b), accent, "bar should paint at its own x");
        let (r, g, b, _) = surface.pixel(141, 50);
        assert_eq!((r, g, b), accent, "zero-width bar widens to 2 points");

        // Above the bar's y and left of its x: untouched.
        assert_eq!(
            {
                let (r, g, b, _) = surface.pixel(140, 20);
                (r, g, b)
            },
            (0, 0, 0),
            "nothing should paint above the bar",
        );
        assert_eq!(
            {
                let (r, g, b, _) = surface.pixel(130, 50);
                (r, g, b)
            },
            (0, 0, 0),
            "nothing should paint left of the bar",
        );
    }

    #[test]
    fn empty_overlay_paints_nothing() {
        let overlay = DropOverlay {
            highlight: None,
            insertion_bar: None,
            ghost_position: None,
        };
        let surface = paint_via_backend(&overlay);
        assert!(
            surface
                .bytes()
                .chunks_exact(4)
                .all(|p| (p[0], p[1], p[2]) == (0, 0, 0)),
            "an overlay with no geometry must leave the frame untouched",
        );
    }
}
