//! Direct2D rasteriser for [`crate::Scrollbar`] (issue #27).
//!
//! Painting moved to the shared
//! [`crate::primitives::scrollbar::native_surface_paint::paint`] (#811,
//! `NativeSurface` Phase 2d) — see that fn's doc for the one named
//! divergence (quadraui#791) re-verified (already fixed) while unifying
//! `gtk::draw_scrollbar`, `macos::scrollbar::draw_scrollbar` and
//! `win::scrollbar::draw_scrollbar` into one implementation. This module
//! now only carries [`RawScrollbarSurface`] and the deprecated
//! [`draw_scrollbar`] compatibility shim over it, mirroring
//! `win::form::RawFormSurface` (#808).
//!
//! `super::multi_section_view`'s embedded scrollbar still uses its own
//! CPU-premix convention — out of scope here, see that module's doc.
//!
//! Only compiled on `target_os = "windows"` — see `super::mod`'s
//! `#[cfg(target_os = "windows")] mod scrollbar;` and `backend.rs`'s
//! module docs for why the rest of this repo's `--features win` compile
//! gate stays meaningful without a Windows host.

use windows::Win32::Graphics::Direct2D::ID2D1RenderTarget;

use crate::native_surface::NativeSurface;
use crate::primitives::scrollbar::Scrollbar;
use crate::theme::Theme;

/// Minimal [`NativeSurface`] adapter over a bare `&ID2D1RenderTarget`,
/// used only by the deprecated [`draw_scrollbar`] shim below and by this
/// module's own tests — a scrollbar's paint calls exactly one verb
/// (`surface_fill_rect`), so every other method is `unreachable!()`.
/// Mirrors `win::form::RawFormSurface`'s identical pattern (#808).
pub(crate) struct RawScrollbarSurface<'a> {
    pub(crate) target: &'a ID2D1RenderTarget,
}

impl NativeSurface for RawScrollbarSurface<'_> {
    fn surface_begin_frame(&mut self, _viewport: crate::Viewport) {
        unreachable!("RawScrollbarSurface has no backend frame lifecycle to begin")
    }

    fn surface_end_frame(&mut self) {
        unreachable!("RawScrollbarSurface has no backend frame lifecycle to end")
    }

    fn surface_viewport(&self) -> crate::Viewport {
        unreachable!("RawScrollbarSurface has no backend viewport")
    }

    fn surface_line_height(&self) -> f32 {
        unreachable!("RawScrollbarSurface has no backend line height")
    }

    fn surface_char_width(&self) -> f32 {
        unreachable!("RawScrollbarSurface has no backend char width")
    }

    fn surface_measure_text(&self, _text: &str) -> (f32, f32) {
        unreachable!("RawScrollbarSurface has no text measurement")
    }

    fn surface_fill_rect(&mut self, rect: crate::Rect, color: crate::Color) {
        // `win::text::fill_rect` already honours `color.a` with a real
        // translucent `ID2D1SolidColorBrush` (the quadraui#791 fix,
        // predating this issue — see the module doc).
        let _ = super::text::fill_rect(self.target, rect, color);
    }

    fn surface_stroke_rect(
        &mut self,
        _rect: crate::Rect,
        _color: crate::Color,
        _stroke_width: f32,
    ) {
        unreachable!("Scrollbar::paint never strokes a rect")
    }

    fn surface_draw_text_run(&mut self, _rect: crate::Rect, _text: &str, _color: crate::Color) {
        unreachable!("Scrollbar::paint never draws text")
    }

    fn surface_draw_line(
        &mut self,
        _from: crate::Point,
        _to: crate::Point,
        _color: crate::Color,
        _stroke_width: f32,
    ) {
        unreachable!("Scrollbar::paint never strokes a line")
    }

    fn surface_push_clip(&mut self, _rect: crate::Rect) {
        unreachable!("Scrollbar::paint never clips")
    }

    fn surface_pop_clip(&mut self) {
        unreachable!("Scrollbar::paint never clips")
    }

    fn surface_draw_image(
        &mut self,
        _rect: crate::Rect,
        _image: &crate::Image,
    ) -> crate::backend::ImagePaintResult {
        unreachable!("Scrollbar::paint never draws an image")
    }
}

/// Deprecated free-function shim (#811, CLAUDE.md rule 8): reproduces
/// the pre-#811 signature exactly for any external caller that held a
/// direct `quadraui::win::draw_scrollbar` reference rather than going
/// through [`crate::Backend::draw_scrollbar`] — the sanctioned entry
/// point, and the one every in-tree call site already uses, which is
/// why this shim has no in-repo caller left to trip the
/// `-D warnings`-denied `deprecated` lint.
#[deprecated(
    since = "0.0.1",
    note = "call `Backend::draw_scrollbar` instead — this free function is a compatibility shim over the shared #811 implementation"
)]
pub fn draw_scrollbar(target: &ID2D1RenderTarget, scrollbar: &Scrollbar, theme: &Theme) {
    let mut surface = RawScrollbarSurface { target };
    crate::primitives::scrollbar::native_surface_paint::paint(scrollbar, &mut surface, theme);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::Rect;
    use crate::types::{Color, WidgetId};
    use crate::win::testing::HeadlessSurface;

    const W: u32 = 80;
    const H: u32 = 200;

    /// Paint `scrollbar` via the shared
    /// [`crate::primitives::scrollbar::native_surface_paint::paint`]
    /// through a [`RawScrollbarSurface`] over `surface`'s headless
    /// target — the same adapter the deprecated [`draw_scrollbar`] shim
    /// uses, exercised here directly so these tests don't trip the
    /// `-D warnings`-denied `deprecated` lint (CLAUDE.md rule 3;
    /// mirrors `win::form`'s identical test-migration note).
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
                let mut raw = RawScrollbarSurface { target };
                crate::primitives::scrollbar::native_surface_paint::paint(
                    scrollbar,
                    &mut raw,
                    &Theme::default(),
                );
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

    /// Reference alpha blend, independent of production code, for
    /// computing the expected pixel colour in
    /// [`track_blends_with_real_background_not_theme_background`].
    fn expected_alpha_blend(base: Color, over: Color, alpha: f32) -> Color {
        let mix =
            |b: u8, o: u8| -> u8 { (b as f32 * (1.0 - alpha) + o as f32 * alpha).round() as u8 };
        Color::rgb(
            mix(base.r, over.r),
            mix(base.g, over.g),
            mix(base.b, over.b),
        )
    }

    /// Regression for quadraui#791: `draw_scrollbar` used to premix the
    /// track colour against `theme.background` via `super::text::blend`
    /// regardless of what was already painted underneath, so a
    /// scrollbar drawn over an editor/terminal/panel header (anything
    /// other than the bare theme background) got a
    /// `theme.background`-coloured halo instead of blending with the
    /// real content beneath it. Paint over a background that's
    /// deliberately far from `theme.background` and assert the result
    /// is the real alpha blend of *that* background with the track
    /// colour — not the old premixed halo.
    #[test]
    fn track_blends_with_real_background_not_theme_background() {
        let sb = vertical_bar(0.0, 200.0, 50.0);
        let theme = Theme::default();
        let real_bg = Color::rgb(200, 30, 30);
        assert_ne!(
            (real_bg.r, real_bg.g, real_bg.b),
            (theme.background.r, theme.background.g, theme.background.b),
            "test fixture must differ from theme.background to be a meaningful probe",
        );

        let surface = HeadlessSurface::new(W, H).expect("create surface");
        surface
            .fill_rect(Rect::new(0.0, 0.0, W as f32, H as f32), real_bg)
            .expect("fill bg");
        surface
            .paint(|target| {
                let mut raw = RawScrollbarSurface { target };
                crate::primitives::scrollbar::native_surface_paint::paint(&sb, &mut raw, &theme);
            })
            .expect("paint scrollbar");

        // Not hovered/dragging: track_alpha = 0.20 (see the shared `paint`).
        let track_alpha = 0.20;
        let expected = expected_alpha_blend(real_bg, theme.scrollbar_track, track_alpha);
        let old_halo = expected_alpha_blend(theme.background, theme.scrollbar_track, track_alpha);

        // Probe mid-track, below the thumb (thumb_len ≈ 50px at scroll=0).
        let c = surface.pixel_at(4, 120);
        assert_eq!(
            (c.r, c.g, c.b),
            (expected.r, expected.g, expected.b),
            "track should alpha-blend with the real background beneath it",
        );
        assert_ne!(
            (c.r, c.g, c.b),
            (old_halo.r, old_halo.g, old_halo.b),
            "track must not premix against theme.background regardless of what's painted underneath",
        );
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
