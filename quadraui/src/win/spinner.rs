//! Direct2D / DirectWrite rasteriser geometry for [`crate::Spinner`].
//!
//! Painting is delegated to the shared
//! [`crate::primitives::spinner::native_surface_paint::paint`] function
//! — the `Backend`-trait entry point
//! [`super::backend::WinBackend::draw_spinner`] calls it directly
//! (`self` is the [`crate::paint_surface::PaintSurface`]), passing
//! `self.current_theme`. This module carries only
//! [`win_spinner_layout`], the pure-geometry twin
//! `WinBackend::spinner_layout`'s no-paint hit-test path needs.
//!
//! [`win_spinner_layout`] is generic over [`SpinnerMeasureSource`] — no
//! Direct2D/DirectWrite type in its signature — so it compiles and runs
//! everywhere, including a plain `cargo test --features win` on Linux.
//! `super::mod`'s `mod spinner;` is not whole-module gated; see
//! `backend.rs`'s module docs.

#[cfg(target_os = "windows")]
use super::text::DWrite;
use crate::event::Rect;
use crate::primitives::spinner::{spinner_text, Spinner, SpinnerLayout, SpinnerMeasure};

/// Width+height text measurement for [`win_spinner_layout`]. A spinner's
/// hit box height comes from the glyph+label text's own measured height
/// (not a fixed `line_height`), so this can't reuse
/// [`crate::primitives::layout_metrics::TextMeasure`] (width-only) the
/// way `win::menu_bar`/`win::toast` do — a real difference this
/// rasteriser has always had (`DWrite::measure_text` returns both), not
/// new duplication.
pub trait SpinnerMeasureSource {
    fn measure(&self, text: &str) -> (f32, f32);
}

#[cfg(target_os = "windows")]
impl SpinnerMeasureSource for DWrite {
    fn measure(&self, text: &str) -> (f32, f32) {
        self.measure_text(text).unwrap_or((0.0, 0.0))
    }
}

/// Compute a [`Spinner`]'s layout without painting — the measurer twin
/// of [`crate::primitives::spinner::native_surface_paint::paint`]. Both
/// measure the identical glyph+label text (via [`spinner_text`]), so a
/// no-paint hit-test call always agrees with what the last paint drew.
pub fn win_spinner_layout(
    measure: &dyn SpinnerMeasureSource,
    rect: Rect,
    spinner: &Spinner,
) -> SpinnerLayout {
    let text = spinner_text(spinner);
    let (w, h) = measure.measure(&text);
    spinner.layout(rect.x, rect.y, SpinnerMeasure::new(w, h))
}

// Tests paint through the shared `native_surface_paint::paint` over a
// real `DWrite`/`HeadlessSurface`, the same shape `win::progress`'s test
// module uses.
#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::*;
    use crate::primitives::spinner::SpinnerHit;
    use crate::theme::Theme;
    use crate::types::{Color, WidgetId};
    use crate::win::testing::HeadlessSurface;

    fn spinner(frame_idx: usize) -> Spinner {
        Spinner {
            id: WidgetId::new("sp"),
            label: "Indexing…".into(),
            frame_idx,
            accent: None,
        }
    }

    /// Paint `spinner` via the shared
    /// [`crate::primitives::spinner::native_surface_paint::paint`]
    /// through a [`super::super::surface::D2dSurface`] over `surface`'s
    /// headless target — the same adapter
    /// `WinBackend::draw_spinner` uses in production (there, `self` is
    /// the surface directly; here it's constructed explicitly to paint
    /// without a live backend).
    fn paint(
        surface: &HeadlessSurface,
        dwrite: &DWrite,
        rect: Rect,
        spinner: &Spinner,
        theme: &Theme,
    ) -> SpinnerLayout {
        surface
            .paint(|target| {
                let mut raw = super::super::surface::D2dSurface {
                    target,
                    dwrite: Some(dwrite),
                };
                crate::primitives::spinner::native_surface_paint::paint(
                    spinner, &mut raw, theme, rect.x, rect.y,
                );
            })
            .map(|_| win_spinner_layout(dwrite, rect, spinner))
            .expect("paint spinner")
    }

    /// The layout bounds are wide enough to hold glyph + label, and
    /// `hit_test` resolves a click inside them to `Body` (outside, to
    /// `Empty`) — the round trip a spinner supports (it's read-only, so
    /// there's no dismiss/action sub-region to cover).
    #[test]
    fn layout_hit_test_round_trip() {
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let spinner = spinner(0);
        let rect = Rect::new(10.0, 10.0, 0.0, 0.0);

        let layout = win_spinner_layout(&dwrite, rect, &spinner);
        assert!(layout.bounds.width > 0.0);
        assert!(layout.bounds.height > 0.0);

        let inside = layout.hit_test(
            layout.bounds.x + 1.0,
            layout.bounds.y + 1.0,
            &WidgetId::new("sp"),
        );
        assert_eq!(inside, SpinnerHit::Body(WidgetId::new("sp")));

        let outside = layout.hit_test(0.0, 0.0, &WidgetId::new("sp"));
        assert_eq!(outside, SpinnerHit::Empty);
    }

    /// Painting doesn't panic and leaves the glyph's own ink somewhere
    /// inside the measured bounds — probing the fg colour would be
    /// glyph-hinting-fragile (see `tooltip`'s module doc on the same
    /// hazard), so this just paints and re-derives the layout to prove
    /// the call succeeds against a real (headless) render target.
    #[test]
    fn paints_without_panicking() {
        let surface = HeadlessSurface::new(200, 20).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let spinner = spinner(3);
        let rect = Rect::new(0.0, 0.0, 0.0, 0.0);

        let layout = paint(&surface, &dwrite, rect, &spinner, &Theme::default());

        assert!(layout.bounds.width > 0.0);
    }

    /// `frame_idx` cycles through the frame table, not the label —
    /// different frames still measure to non-zero, and painting each
    /// glyph in the same accent colour and label is exercised without
    /// panicking (glyph identity itself isn't a Direct2D-observable
    /// property this test can assert on without pixel-perfect glyph
    /// probing — hence the `_does_not_panic` name rather than a claim
    /// this asserts the accent colour was actually used).
    #[test]
    fn accent_colour_paints_without_panicking() {
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let mut spinner = spinner(9);
        spinner.accent = Some(Color::rgb(255, 0, 0));
        let rect = Rect::new(0.0, 0.0, 0.0, 0.0);

        let surface = HeadlessSurface::new(200, 20).expect("create surface");
        paint(&surface, &dwrite, rect, &spinner, &Theme::default());
    }

    /// With no `accent` set, the glyph's ink colour tracks the live
    /// theme's `foreground`, not [`Theme::default`] — a non-default theme
    /// passed in must show up in what gets painted. Offsets the rect
    /// away from the surface's own origin so `(0, 0)` is a reliable
    /// unpainted background sample (same shape as
    /// `win::activity_bar`'s `paints_at_rect_origin_not_at_surface_origin`),
    /// then finds the most-inked pixel (max colour distance from that
    /// background) per paint and checks it lands nearer the theme that
    /// was actually passed in than the other one — robust to
    /// anti-aliasing, unlike an exact-pixel-colour match.
    #[test]
    fn theme_foreground_colours_the_glyph() {
        let custom_theme = Theme {
            foreground: Color::rgb(30, 160, 230),
            ..Theme::default()
        };
        let default_theme = Theme::default();
        let spinner = spinner(0);
        let rect = Rect::new(10.0, 4.0, 0.0, 0.0);

        let dist = |a: (u8, u8, u8), c: Color| {
            let d = |x: u8, y: u8| (x as i32 - y as i32).pow(2);
            d(a.0, c.r) + d(a.1, c.g) + d(a.2, c.b)
        };

        let most_inked = |theme: &Theme| -> (u8, u8, u8) {
            let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
            let surface = HeadlessSurface::new(220, 24).expect("create surface");
            paint(&surface, &dwrite, rect, &spinner, theme);

            let bg = surface.pixel_at(0, 0);
            let bg = (bg.r, bg.g, bg.b);
            let mut best = bg;
            let mut best_d = -1i32;
            for y in 0..24u32 {
                for x in 0..220u32 {
                    let px = surface.pixel_at(x, y);
                    let c = (px.r, px.g, px.b);
                    let d = (c.0 as i32 - bg.0 as i32).pow(2)
                        + (c.1 as i32 - bg.1 as i32).pow(2)
                        + (c.2 as i32 - bg.2 as i32).pow(2);
                    if d > best_d {
                        best_d = d;
                        best = c;
                    }
                }
            }
            best
        };

        let custom_ink = most_inked(&custom_theme);
        assert!(
            dist(custom_ink, custom_theme.foreground) < dist(custom_ink, default_theme.foreground),
            "most-inked pixel {custom_ink:?} painted with a non-default \
             theme should be closer to that theme's foreground \
             {:?} than to Theme::default()'s {:?}",
            custom_theme.foreground,
            default_theme.foreground,
        );
    }
}
