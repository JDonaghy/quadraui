//! macOS rasteriser for [`crate::Split`].
//!
//! Mirrors [`crate::gtk::split::draw_split`]: paints only the divider
//! as a filled rectangle. Pane content is the app's responsibility —
//! the rasteriser returns the resolved [`SplitLayout`] so the host can
//! paint into `first_bounds` / `second_bounds` and route clicks via
//! `hit_test`.

use core_graphics::geometry::CGRect;
use core_graphics::sys::CGContextRef;

use crate::event::Rect as QRect;
use crate::primitives::split::{Split, SplitLayout, SplitMeasure};
use crate::theme::Theme;
use crate::types::Color;

/// 4-point divider thickness, matching GTK.
const DIVIDER_PX: f32 = 4.0;

/// Compute the macOS pixel-unit layout for a [`Split`] without painting.
pub fn mac_split_layout(split: &Split, x: f64, y: f64, w: f64, h: f64) -> SplitLayout {
    let bounds = QRect::new(x as f32, y as f32, w as f32, h as f32);
    split.layout(bounds, SplitMeasure::new(DIVIDER_PX))
}

/// Draw a [`Split`] divider onto `ctx`. Returns the layout for host
/// click/drag dispatch. Pane content is NOT painted.
///
/// # Safety
///
/// `ctx` must be a valid `CGContextRef` borrowed for the duration of
/// the call.
pub unsafe fn draw_split(
    ctx: CGContextRef,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    split: &Split,
    theme: &Theme,
) -> SplitLayout {
    let layout = mac_split_layout(split, x, y, w, h);
    let d = layout.divider_bounds;
    fill_rect(
        ctx,
        d.x as f64,
        d.y as f64,
        d.width as f64,
        d.height as f64,
        theme.separator,
    );
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
    use crate::primitives::split::{SplitDirection, SplitHit};
    use crate::types::WidgetId;
    use crate::Backend;

    const W: u32 = 200;
    const H: u32 = 120;

    fn sample_split(direction: SplitDirection) -> Split {
        Split {
            id: WidgetId::new("split"),
            direction,
            ratio: 0.5,
            first_min: 0.0,
            second_min: 0.0,
        }
    }

    fn paint_via_backend(split: &Split) -> (BitmapSurface, SplitLayout) {
        let surface = BitmapSurface::new(W, H);
        // Fill with a known background so the divider's separator
        // colour reads against a known starting state.
        surface.fill(1.0, 1.0, 1.0, 1.0);
        let mut backend = MacBackend::new();
        backend.set_current_font(make_font("Menlo", 14.0).expect("Menlo installed"));
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        let layout = std::cell::RefCell::new(None);
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            let l = b.draw_split(QRect::new(0.0, 0.0, W as f32, H as f32), split);
            *layout.borrow_mut() = Some(l);
        });
        backend.end_frame();
        (surface, layout.into_inner().unwrap())
    }

    #[test]
    fn horizontal_divider_paints_separator() {
        let split = sample_split(SplitDirection::Horizontal);
        let (surface, layout) = paint_via_backend(&split);
        let theme = Theme::default();
        // Probe inside the divider rect.
        let d = layout.divider_bounds;
        let px = (d.x + d.width / 2.0) as u32;
        let py = (d.y + d.height / 2.0) as u32;
        let (r, g, b, _) = surface.pixel(px, py);
        assert_eq!(
            (r, g, b),
            (theme.separator.r, theme.separator.g, theme.separator.b),
        );
    }

    #[test]
    fn vertical_divider_paints_separator() {
        let split = sample_split(SplitDirection::Vertical);
        let (surface, layout) = paint_via_backend(&split);
        let theme = Theme::default();
        let d = layout.divider_bounds;
        let px = (d.x + d.width / 2.0) as u32;
        let py = (d.y + d.height / 2.0) as u32;
        let (r, g, b, _) = surface.pixel(px, py);
        assert_eq!(
            (r, g, b),
            (theme.separator.r, theme.separator.g, theme.separator.b),
        );
    }

    #[test]
    fn pane_areas_left_unpainted_outside_divider() {
        // Background fill stays white outside the divider band — split
        // paints chrome only.
        let split = sample_split(SplitDirection::Horizontal);
        let (surface, _layout) = paint_via_backend(&split);
        // Probe near the right edge of the first pane (well left of
        // the divider at W/2).
        let (r, g, b, _) = surface.pixel(10, H / 2);
        assert_eq!((r, g, b), (255, 255, 255), "left pane should be unpainted");
    }

    #[test]
    fn hit_test_resolves_divider_and_panes() {
        let split = sample_split(SplitDirection::Horizontal);
        let (_surface, layout) = paint_via_backend(&split);

        let d = layout.divider_bounds;
        let hit = layout.hit_test(d.x + d.width / 2.0, d.y + d.height / 2.0);
        assert!(
            matches!(hit, SplitHit::Divider(_)),
            "divider hit was {:?}",
            hit
        );

        // First pane: well left of the divider.
        let hit = layout.hit_test(10.0, 10.0);
        assert!(matches!(hit, SplitHit::FirstPane(_)));

        // Second pane: well right of the divider.
        let hit = layout.hit_test((W - 10) as f32, 10.0);
        assert!(matches!(hit, SplitHit::SecondPane(_)));
    }

    #[test]
    fn min_size_clamps_ratio() {
        // Force ratio = 0.1 but require first_min = W*0.4; resolved
        // ratio should bump up so first pane >= first_min.
        let split = Split {
            id: WidgetId::new("split"),
            direction: SplitDirection::Horizontal,
            ratio: 0.1,
            first_min: (W as f32) * 0.4,
            second_min: 0.0,
        };
        let (_surface, layout) = paint_via_backend(&split);
        assert!(
            layout.resolved_ratio > 0.3,
            "resolved_ratio {} should be clamped above 0.3 to honour first_min",
            layout.resolved_ratio,
        );
    }
}
