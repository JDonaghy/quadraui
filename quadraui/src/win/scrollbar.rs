//! Direct2D rasteriser for [`crate::Scrollbar`] (issue #27).
//!
//! Overlay-style scrollbar matching [`crate::gtk::scrollbar::draw_scrollbar`]
//! / [`crate::macos::scrollbar::draw_scrollbar`]: a thin track with a
//! brighter thumb on top, both bumping opacity on hover/drag so the bar
//! pops while the user is interacting with it.
//!
//! Both axes share this implementation — [`Scrollbar::axis`] determines
//! whether `thumb_start`/`thumb_len` are applied vertically or
//! horizontally.
//!
//! Unlike the GTK/macOS twins (which paint a real alpha-blended overlay
//! straight onto whatever is already on screen), this rasteriser
//! premixes track/thumb colours against `theme.background` via
//! [`super::text::blend`] before filling — see `super::text::blend`'s
//! doc and `super::multi_section_view`'s module doc for why every
//! translucency in this backend goes through a CPU premix rather than a
//! native Direct2D alpha-blended fill.
//!
//! Only compiled on `target_os = "windows"` — see `super::mod`'s
//! `#[cfg(target_os = "windows")] mod scrollbar;` and `backend.rs`'s
//! module docs for why the rest of this repo's `--features win` compile
//! gate stays meaningful without a Windows host.

use windows::Win32::Graphics::Direct2D::ID2D1RenderTarget;

use super::text::{blend, fill_rect};
use crate::event::Rect;
use crate::primitives::scrollbar::{ScrollAxis, Scrollbar};
use crate::theme::Theme;

/// Paint `scrollbar` onto `target`.
pub fn draw_scrollbar(target: &ID2D1RenderTarget, scrollbar: &Scrollbar, theme: &Theme) {
    let track = scrollbar.track;
    if track.width <= 0.0 || track.height <= 0.0 {
        return;
    }

    let track_alpha = if scrollbar.hovered || scrollbar.dragging {
        0.35
    } else {
        0.20
    };
    let thumb_alpha = if scrollbar.dragging {
        0.85
    } else if scrollbar.hovered {
        0.70
    } else {
        0.50
    };

    let track_color = blend(theme.background, theme.scrollbar_track, track_alpha);
    let _ = fill_rect(target, track, track_color);

    let thumb_rect = match scrollbar.axis {
        ScrollAxis::Vertical => Rect::new(
            track.x,
            track.y + scrollbar.thumb_start,
            track.width,
            scrollbar.thumb_len,
        ),
        ScrollAxis::Horizontal => Rect::new(
            track.x + scrollbar.thumb_start,
            track.y,
            scrollbar.thumb_len,
            track.height,
        ),
    };
    let thumb_color = blend(track_color, theme.scrollbar_thumb, thumb_alpha);
    let _ = fill_rect(target, thumb_rect, thumb_color);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::WidgetId;
    use crate::win::testing::HeadlessSurface;

    const W: u32 = 80;
    const H: u32 = 200;

    fn paint(scrollbar: &Scrollbar) -> HeadlessSurface {
        let surface = HeadlessSurface::new(W, H).expect("create surface");
        // Fill with a known background so blended track/thumb colours
        // are deterministic to probe.
        surface
            .fill_rect(
                Rect::new(0.0, 0.0, W as f32, H as f32),
                Theme::default().background,
            )
            .expect("fill bg");
        surface
            .paint(|target| {
                draw_scrollbar(target, scrollbar, &Theme::default());
            })
            .expect("paint scrollbar");
        surface
    }

    fn vertical_bar(scroll: f32, total: f32, visible: f32) -> Scrollbar {
        Scrollbar::vertical(
            WidgetId::new("sb"),
            Rect::new(0.0, 0.0, 8.0, H as f32),
            scroll,
            total,
            visible,
            20.0,
        )
    }

    #[test]
    fn track_paints_over_background() {
        let sb = vertical_bar(0.0, 200.0, 50.0);
        let surface = paint(&sb);
        let theme = Theme::default();
        // Probe mid-track at a y the thumb does NOT cover (thumb sits
        // at the top with thumb_len ≈ 50px when scroll=0).
        let c = surface.pixel_at(4, 120);
        assert_ne!(
            (c.r, c.g, c.b),
            (theme.background.r, theme.background.g, theme.background.b),
            "track should paint something other than plain background",
        );
    }

    #[test]
    fn thumb_paints_differently_than_track_only_zone() {
        let sb = vertical_bar(0.0, 200.0, 50.0);
        let surface = paint(&sb);
        // Thumb at top: probe inside thumb_len.
        let thumb_px = surface.pixel_at(4, 5);
        // Track only: probe well below the thumb.
        let track_px = surface.pixel_at(4, 150);
        assert_ne!(
            (thumb_px.r, thumb_px.g, thumb_px.b),
            (track_px.r, track_px.g, track_px.b),
            "thumb zone should differ from a track-only zone",
        );
    }

    #[test]
    fn dragging_makes_thumb_more_opaque() {
        let mut sb = vertical_bar(0.0, 200.0, 50.0);
        let normal = paint(&sb).pixel_at(4, 5);
        sb.dragging = true;
        let dragging = paint(&sb).pixel_at(4, 5);
        assert_ne!(
            (normal.r, normal.g, normal.b),
            (dragging.r, dragging.g, dragging.b),
            "dragging should change the thumb's blended colour",
        );
    }

    #[test]
    fn full_scroll_lands_thumb_at_track_bottom() {
        // scroll = total - visible should align the thumb's bottom edge
        // to the track end.
        let sb = vertical_bar(150.0, 200.0, 50.0);
        let surface = paint(&sb);
        let bottom = surface.pixel_at(4, H - 4);
        let mid = surface.pixel_at(4, 80);
        assert_ne!(
            (bottom.r, bottom.g, bottom.b),
            (mid.r, mid.g, mid.b),
            "full-scroll: the bottom probe (now inside the thumb) should differ from mid-track",
        );
    }

    #[test]
    fn horizontal_orientation_uses_width() {
        let track = Rect::new(0.0, 50.0, W as f32, 8.0);
        let sb = Scrollbar::horizontal(WidgetId::new("h"), track, 0.0, 200.0, 40.0, 10.0);
        let surface = paint(&sb);
        // Thumb at left: x ∈ [0, thumb_len) should show thumb;
        // x past thumb should show track only.
        let left = surface.pixel_at(2, 54);
        let right = surface.pixel_at(W - 4, 54);
        assert_ne!(
            (left.r, left.g, left.b),
            (right.r, right.g, right.b),
            "horizontal: thumb zone (left) should differ from track-only zone (right)",
        );
    }

    #[test]
    fn zero_size_track_is_a_no_op() {
        let sb = Scrollbar::vertical(
            WidgetId::new("sb"),
            Rect::new(0.0, 0.0, 0.0, 0.0),
            0.0,
            200.0,
            50.0,
            20.0,
        );
        // Must not panic (a zero-size fill_rect is a no-op D2D call).
        let _ = paint(&sb);
    }
}
