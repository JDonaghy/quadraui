//! Direct2D / DirectWrite rasteriser for [`crate::StatusBar`] (issue #25).
//!
//! Painting moved to the shared
//! [`crate::primitives::status_bar::native_surface_paint::paint`] (#860,
//! `NativeSurface` Phase 2d slice 3/9) — see that fn's doc for the named
//! divergences (bold-aware measurement: GTK/Win measured a segment's own
//! `bold` weight, macOS ignored it; GTK's missing zero-size guard, now
//! applying the already-fixed quadraui#791 shape uniformly) found while
//! unifying `gtk::draw_status_bar`, `macos::status_bar::draw_status_bar`
//! and `win::status_bar::draw_status_bar` into one implementation. This
//! module now only carries [`win_status_bar_layout`] (pure layout, still
//! needed by `WinBackend::status_bar_layout` for no-paint hit-test
//! queries) and the deprecated [`draw_status_bar`] compatibility shim over
//! [`RawWinStatusBarSurface`], mirroring `win::panel`'s identical #859
//! shape. `MIN_GAP_DIP` stays put — it's still [`win_status_bar_layout`]'s
//! own measurer constant, untouched by this migration.
//!
//! Only compiled on `target_os = "windows"` — see `super::mod`'s
//! `#[cfg(target_os = "windows")] mod status_bar;` and `backend.rs`'s
//! module docs for why the rest of this repo's `--features win` compile
//! gate stays meaningful without a Windows host.
//!
//! # Theme
//!
//! Takes the live theme as a `&Theme` parameter (quadraui#789) — the
//! caller ([`crate::win::WinBackend::draw_status_bar`]) passes
//! `&self.current_theme`, the same field `Backend::set_theme` writes.
//! Callers that don't have segment-level colours to fall back on (the
//! bar's own background, when it has no segments) get that live theme's
//! background, not [`Theme::default`].

use windows::Win32::Graphics::Direct2D::ID2D1RenderTarget;

use super::text::{pop_clip, push_clip, DWrite};
use crate::event::Rect;
use crate::native_surface::NativeSurface;
use crate::primitives::status_bar::StatusSegmentMeasure;
use crate::theme::Theme;
use crate::types::WidgetId;
use crate::{StatusBar, StatusBarLayout};

/// Minimum gap (DIPs) reserved between the left and right segment
/// groups — the DirectWrite twin of [`crate::gtk::MIN_GAP_PX`]. Still
/// used by [`win_status_bar_layout`] — the shared
/// [`crate::primitives::status_bar::native_surface_paint::paint`] carries
/// its own independent copy of the same value (see that module's doc).
pub const MIN_GAP_DIP: f32 = 16.0;

/// Compute a [`StatusBar`]'s layout without painting — the DirectWrite
/// measurer twin of the shared `paint`, and what
/// [`crate::win::WinBackend::status_bar_layout`] calls directly. Both this
/// function and `paint` measure a segment's width via
/// `DWrite::measure_text_styled`, so a no-paint hit-test call always
/// agrees with what the last paint drew.
pub fn win_status_bar_layout(dwrite: &DWrite, rect: Rect, bar: &StatusBar) -> StatusBarLayout {
    bar.layout(rect.width, rect.height, MIN_GAP_DIP, |seg| {
        let (w, _) = dwrite
            .measure_text_styled(&seg.text, seg.bold)
            .unwrap_or((0.0, 0.0));
        StatusSegmentMeasure::new(w)
    })
}

/// Minimal [`NativeSurface`] adapter over a bare `&ID2D1RenderTarget` +
/// [`DWrite`], used only by the deprecated [`draw_status_bar`] shim below
/// and by this module's own tests — mirrors
/// `win::panel::RawPanelSurface`'s identical pattern (#859), extended
/// with bold-aware measurement/drawing (`surface_measure_text_styled`/
/// `surface_draw_text_run_styled`) and clip push/pop, both of which this
/// primitive's paint actually uses.
pub(crate) struct RawWinStatusBarSurface<'a> {
    pub(crate) target: &'a ID2D1RenderTarget,
    pub(crate) dwrite: &'a DWrite,
}

impl NativeSurface for RawWinStatusBarSurface<'_> {
    fn surface_begin_frame(&mut self, _viewport: crate::Viewport) {
        unreachable!("RawWinStatusBarSurface has no backend frame lifecycle to begin")
    }

    fn surface_end_frame(&mut self) {
        unreachable!("RawWinStatusBarSurface has no backend frame lifecycle to end")
    }

    fn surface_viewport(&self) -> crate::Viewport {
        unreachable!("RawWinStatusBarSurface has no backend viewport")
    }

    fn surface_line_height(&self) -> f32 {
        unreachable!("RawWinStatusBarSurface has no backend line height")
    }

    fn surface_char_width(&self) -> f32 {
        unreachable!("RawWinStatusBarSurface has no backend char width")
    }

    fn surface_measure_text(&self, text: &str) -> (f32, f32) {
        self.dwrite.measure_text(text).unwrap_or((0.0, 0.0))
    }

    fn surface_measure_text_styled(&self, text: &str, bold: bool) -> (f32, f32) {
        self.dwrite
            .measure_text_styled(text, bold)
            .unwrap_or((0.0, 0.0))
    }

    fn surface_fill_rect(&mut self, rect: crate::Rect, color: crate::Color) {
        let _ = super::text::fill_rect(self.target, rect, color);
    }

    fn surface_stroke_rect(
        &mut self,
        _rect: crate::Rect,
        _color: crate::Color,
        _stroke_width: f32,
    ) {
        unreachable!("StatusBar::paint never strokes a rect")
    }

    fn surface_draw_text_run(&mut self, rect: crate::Rect, text: &str, color: crate::Color) {
        let _ = self.dwrite.draw_text(self.target, text, rect, color);
    }

    #[allow(clippy::too_many_arguments)]
    fn surface_draw_text_run_styled(
        &mut self,
        rect: crate::Rect,
        text: &str,
        color: crate::Color,
        bold: bool,
        _italic: bool,
        _underline: bool,
        _scale_x: f32,
    ) {
        let _ = self
            .dwrite
            .draw_text_styled(self.target, text, rect, color, bold);
    }

    fn surface_draw_line(
        &mut self,
        _from: crate::Point,
        _to: crate::Point,
        _color: crate::Color,
        _stroke_width: f32,
    ) {
        unreachable!("StatusBar::paint never strokes a line")
    }

    fn surface_push_clip(&mut self, rect: crate::Rect) {
        push_clip(self.target, rect);
    }

    fn surface_pop_clip(&mut self) {
        pop_clip(self.target);
    }

    fn surface_draw_image(
        &mut self,
        _rect: crate::Rect,
        _image: &crate::Image,
    ) -> crate::backend::ImagePaintResult {
        unreachable!("StatusBar::paint never draws an image")
    }
}

/// Deprecated free-function shim (#860, CLAUDE.md rule 8): reproduces
/// the pre-#860 signature exactly for any external caller that held a
/// direct `quadraui::win::draw_status_bar` reference rather than going
/// through [`crate::Backend::draw_status_bar`] — the sanctioned entry
/// point, and the one every in-tree call site already uses, which is why
/// this shim has no in-repo caller left to trip the `-D
/// warnings`-denied `deprecated` lint.
#[deprecated(
    since = "0.0.1",
    note = "call `Backend::draw_status_bar` instead — this free function is a compatibility shim over the shared #860 implementation"
)]
pub fn draw_status_bar(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    rect: Rect,
    bar: &StatusBar,
    hovered_id: Option<&WidgetId>,
    pressed_id: Option<&WidgetId>,
    theme: &Theme,
) -> StatusBarLayout {
    let mut surface = RawWinStatusBarSurface { target, dwrite };
    crate::primitives::status_bar::native_surface_paint::paint(
        bar,
        &mut surface,
        theme,
        rect.x,
        rect.y,
        rect.width,
        rect.height,
        hovered_id,
        pressed_id,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::status_bar::{StatusBarHit, StatusBarSegment, StatusSegmentSide};
    use crate::types::Color;
    use crate::win::testing::HeadlessSurface;

    const W: f32 = 200.0;
    const H: f32 = 20.0;

    fn bar() -> StatusBar {
        StatusBar {
            id: WidgetId::new("status"),
            left_segments: vec![StatusBarSegment {
                text: "NORMAL".into(),
                fg: Color::rgb(0, 0, 0),
                bg: Color::rgb(10, 20, 30),
                bold: false,
                action_id: Some(WidgetId::new("status:mode")),
            }],
            right_segments: vec![StatusBarSegment {
                text: "Ln 3, Col 8".into(),
                fg: Color::rgb(0, 0, 0),
                bg: Color::rgb(40, 50, 60),
                bold: false,
                action_id: Some(WidgetId::new("status:cursor")),
            }],
        }
    }

    /// Paint `bar` via the shared
    /// [`crate::primitives::status_bar::native_surface_paint::paint`]
    /// through a [`RawWinStatusBarSurface`] over `surface`'s headless
    /// target — the same adapter the deprecated [`draw_status_bar`] shim
    /// uses, exercised here directly so these tests don't trip the
    /// `-D warnings`-denied `deprecated` lint (CLAUDE.md rule 3; mirrors
    /// `win::panel`'s identical test-migration note).
    fn paint(
        surface: &HeadlessSurface,
        dwrite: &DWrite,
        rect: Rect,
        bar: &StatusBar,
        hovered_id: Option<&WidgetId>,
        pressed_id: Option<&WidgetId>,
    ) -> StatusBarLayout {
        surface
            .paint(|target| {
                let mut raw = RawWinStatusBarSurface { target, dwrite };
                crate::primitives::status_bar::native_surface_paint::paint(
                    bar,
                    &mut raw,
                    &Theme::default(),
                    rect.x,
                    rect.y,
                    rect.width,
                    rect.height,
                    hovered_id,
                    pressed_id,
                );
            })
            // `HeadlessSurface::paint`'s closure returns `()` (it just
            // drives Direct2D's `BeginDraw`/`EndDraw` bracket) — the
            // layout `paint` computed during painting is discarded, so
            // recompute it via the same measurer
            // (`win_status_bar_layout`), matching this file's pre-#860
            // test shape. `hovered_id`/`pressed_id` never affect layout
            // (only which segment gets tinted), so this is exact.
            .map(|_| win_status_bar_layout(dwrite, rect, bar))
            .expect("paint status bar")
    }

    /// Does `color` appear anywhere on row `y` between `x0` and `x1`
    /// (bar-local DIPs, half-open)?
    ///
    /// A *scan* rather than a single `pixel_at` probe: `draw_status_bar`
    /// paints each segment's label with `DrawText` into the segment's own
    /// rect, left- and top-aligned with no padding (the primitive's layout
    /// adds none), so the exact pixel at a segment's left edge or centre
    /// may well land on a glyph stem. Which pixels the glyphs cover is a
    /// DirectWrite font-rasterisation detail (hinting, ClearType fringes,
    /// whichever `Segoe UI` version the host ships) and is *not* what these
    /// assertions are about — the claim under test is "this segment's `bg`
    /// was filled across this segment's own bounds", and inter-glyph gaps
    /// make that observable no matter where the ink lands. See
    /// `tab_bar`'s sibling test, which dodges the same hazard by sampling
    /// below the glyph band.
    fn row_contains(surface: &HeadlessSurface, x0: f32, x1: f32, y: u32, color: Color) -> bool {
        (x0.max(0.0) as u32..x1.max(0.0) as u32).any(|x| {
            let px = surface.pixel_at(x, y);
            (px.r, px.g, px.b) == (color.r, color.g, color.b)
        })
    }

    /// Paint↔click round trip: the segment's painted background and the
    /// layout's own `hit_test` over those same bounds must agree on which
    /// (if any) `WidgetId` was clicked.
    #[test]
    fn paint_and_hit_test_round_trip() {
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
        let bar = bar();
        let rect = Rect::new(0.0, 0.0, W, H);

        let layout = paint(&surface, &dwrite, rect, &bar, None, None);

        // The left segment starts at bar-local x=0 — its fill colour must
        // be visible somewhere across its own bounds.
        let mid_y = (H / 2.0) as u32;
        let left_vs = layout
            .visible_segments
            .iter()
            .find(|vs| vs.side == StatusSegmentSide::Left)
            .expect("left segment is visible");
        assert!(
            row_contains(
                &surface,
                left_vs.bounds.x,
                left_vs.bounds.x + left_vs.bounds.width,
                mid_y,
                Color::rgb(10, 20, 30),
            ),
            "left segment's bg should be painted at its own bounds"
        );

        let left_hit = layout.hit_test(1.0, H / 2.0);
        assert_eq!(
            left_hit,
            StatusBarHit::Segment(WidgetId::new("status:mode"))
        );

        // Right segment is right-aligned; its hit-test centre must resolve
        // to the cursor segment, and that x must fall inside painted
        // (non-default-background) pixels.
        let right_vs = layout
            .visible_segments
            .iter()
            .find(|vs| vs.side == StatusSegmentSide::Right)
            .expect("right segment is visible");
        let cx = right_vs.bounds.x + right_vs.bounds.width / 2.0;
        let right_hit = layout.hit_test(cx, H / 2.0);
        assert_eq!(
            right_hit,
            StatusBarHit::Segment(WidgetId::new("status:cursor"))
        );
        assert!(
            row_contains(
                &surface,
                right_vs.bounds.x,
                right_vs.bounds.x + right_vs.bounds.width,
                mid_y,
                Color::rgb(40, 50, 60),
            ),
            "right segment's bg should be painted at its own hit-tested bounds"
        );
    }

    /// A click outside every segment's bounds (the gap) resolves to
    /// `Empty`, and `win_status_bar_layout` (no-paint) must produce byte-
    /// identical `hit_regions` to what `paint` used to paint — same
    /// measurer, same bar, same rect.
    #[test]
    fn no_paint_layout_matches_paint_layout() {
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
        let bar = bar();
        let rect = Rect::new(0.0, 0.0, W, H);

        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
        let painted = paint(&surface, &dwrite, rect, &bar, None, None);
        let no_paint = win_status_bar_layout(&dwrite, rect, &bar);

        assert_eq!(painted, no_paint);
    }

    /// Regression for quadraui#791, ported to the shared paint path: a bar
    /// narrower than its segments' measured text used to let that text
    /// overflow the bar's own rect (segments never priority-drop on the
    /// left, and the right side always keeps at least its highest-
    /// priority segment "even if it alone overflows" — see
    /// `StatusBar::layout`'s doc). Paint a status bar inset in a larger
    /// canvas, deliberately narrower than its segment text, and assert
    /// every pixel outside the bar's own rect stays untouched.
    #[test]
    fn paint_does_not_escape_rect_bounds() {
        let canvas_w = 240u32;
        let canvas_h = 40u32;
        let sentinel = Color::rgb(1, 2, 3);
        // Narrow enough that "NORMAL" alone overflows it.
        let bar_rect = Rect::new(20.0, 10.0, 40.0, H);
        let bar = bar();
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");

        let surface = HeadlessSurface::new(canvas_w, canvas_h).expect("create surface");
        surface
            .fill_rect(
                Rect::new(0.0, 0.0, canvas_w as f32, canvas_h as f32),
                sentinel,
            )
            .expect("fill sentinel");
        let _ = paint(&surface, &dwrite, bar_rect, &bar, None, None);

        for y in 0..canvas_h {
            for x in 0..canvas_w {
                let inside = (x as f32) >= bar_rect.x
                    && (x as f32) < bar_rect.x + bar_rect.width
                    && (y as f32) >= bar_rect.y
                    && (y as f32) < bar_rect.y + bar_rect.height;
                if inside {
                    continue;
                }
                let px = surface.pixel_at(x, y);
                assert_eq!(
                    (px.r, px.g, px.b),
                    (sentinel.r, sentinel.g, sentinel.b),
                    "pixel ({x}, {y}) outside the bar's own rect should stay untouched",
                );
            }
        }
    }

    /// A zero-size rect must not panic (no degenerate clip pushed) and
    /// paint must agree with the no-paint layout — mirrors
    /// `macos::status_bar`'s zero-size guard.
    #[test]
    fn zero_size_rect_is_a_no_op() {
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
        let bar = bar();
        let rect = Rect::new(0.0, 0.0, 0.0, 0.0);

        let surface = HeadlessSurface::new(10, 10).expect("create surface");
        let painted = paint(&surface, &dwrite, rect, &bar, None, None);

        assert_eq!(painted, win_status_bar_layout(&dwrite, rect, &bar));
    }
}
