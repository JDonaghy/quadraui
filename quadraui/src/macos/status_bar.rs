//! macOS rasteriser for [`crate::StatusBar`].
//!
//! Painting moved to the shared
//! [`crate::primitives::status_bar::native_surface_paint::paint`] (#860,
//! `NativeSurface` Phase 2d slice 3/9) — see that fn's doc for the named
//! divergences (bold-aware measurement: GTK/Win measured a segment's own
//! `bold` weight, macOS ignored it; GTK's missing zero-size guard, now
//! applying the already-fixed quadraui#791 shape uniformly) found while
//! unifying `gtk::draw_status_bar`, `macos::status_bar::draw_status_bar`
//! and `win::status_bar::draw_status_bar` into one implementation. This
//! module now only carries [`mac_status_bar_layout`] (pure layout, still
//! needed by `MacBackend::status_bar_layout` for no-paint hit-test
//! queries) and the deprecated [`draw_status_bar`] compatibility shim over
//! [`RawMacStatusBarSurface`], mirroring `macos::panel`'s identical #859
//! shape. `MIN_GAP_PX` stays put — it's still [`mac_status_bar_layout`]'s
//! own measurer constant, untouched by this migration.
//!
//! ## Bold segments
//!
//! Tracked separately — `bold` on a segment was, and remains, ignored on
//! this backend: [`NativeSurface::surface_measure_text_styled`]'s default
//! (drop `bold`, forward to [`NativeSurface::surface_measure_text`])
//! reproduces this file's pre-#860 measurement exactly, so `MacBackend`
//! needed no override. Bold support requires materialising a bold
//! variant of the active font via `CTFontCreateCopyWithSymbolicTraits`
//! and is out of scope for #38; follow-up after the chrome batch lands.
//!
//! [`Backend`]: crate::Backend
//! [`NativeSurface`]: crate::native_surface::NativeSurface
//! [`NativeSurface::surface_measure_text_styled`]: crate::native_surface::NativeSurface::surface_measure_text_styled
//! [`NativeSurface::surface_measure_text`]: crate::native_surface::NativeSurface::surface_measure_text

use core_graphics::sys::CGContextRef;
use core_text::font::CTFont;

use crate::native_surface::NativeSurface;
use crate::primitives::status_bar::StatusSegmentMeasure;
use crate::theme::Theme;
use crate::types::WidgetId;
use crate::{StatusBar, StatusBarLayout};

/// 16-point minimum gap between left and right segment groups. Still
/// used by [`mac_status_bar_layout`] — the shared
/// [`crate::primitives::status_bar::native_surface_paint::paint`] carries
/// its own independent copy of the same value (see that module's doc).
pub(crate) const MIN_GAP_PX: f32 = 16.0;

/// Compute the layout the shared `paint` would produce for `bar` at
/// `width` × `line_height`, without painting.
///
/// This is the no-paint twin backing [`crate::Backend::status_bar_layout`].
/// Hit regions are in **bar-local coordinates** (relative to the bar's
/// origin), which is why neither `x` nor `y` is a parameter — the
/// primitive measures from `0.0` and nothing here folds the origin in.
/// Audited under quadraui#552; see the trait doc for why the status bar
/// (unlike the tab bar) needed no shift.
pub fn mac_status_bar_layout(
    font: &CTFont,
    width: f64,
    line_height: f64,
    bar: &StatusBar,
) -> StatusBarLayout {
    // Degenerate rect: reproduce exactly what the shared `paint` returns
    // so the paint and no-paint paths never disagree.
    if width <= 0.0 || line_height <= 0.0 {
        return bar.layout(
            width.max(0.0) as f32,
            line_height.max(0.0) as f32,
            MIN_GAP_PX,
            |_| StatusSegmentMeasure::new(0.0),
        );
    }
    // Measure each visible segment via Core Text. `bold` is ignored — see
    // this module's doc, "Bold segments".
    let measure = |seg: &crate::primitives::status_bar::StatusBarSegment| -> StatusSegmentMeasure {
        let (w, _) = super::text::measure_text(font, &seg.text);
        StatusSegmentMeasure::new(w as f32)
    };
    bar.layout(width as f32, line_height as f32, MIN_GAP_PX, measure)
}

/// Minimal [`NativeSurface`] adapter over a bare `CGContextRef` + font,
/// used only by the deprecated [`draw_status_bar`] shim below — mirrors
/// `macos::panel::RawPanelSurface`'s identical pattern (#859), extended
/// with clip push/pop, which this primitive's paint actually uses.
/// `surface_measure_text_styled`/`surface_draw_text_run_styled` take the
/// trait's default (drop `bold`) — see this module's doc, "Bold segments".
struct RawMacStatusBarSurface<'a> {
    ctx: CGContextRef,
    font: &'a CTFont,
}

impl NativeSurface for RawMacStatusBarSurface<'_> {
    fn surface_begin_frame(&mut self, _viewport: crate::Viewport) {
        unreachable!("RawMacStatusBarSurface has no backend frame lifecycle to begin")
    }

    fn surface_end_frame(&mut self) {
        unreachable!("RawMacStatusBarSurface has no backend frame lifecycle to end")
    }

    fn surface_viewport(&self) -> crate::Viewport {
        unreachable!("RawMacStatusBarSurface has no backend viewport")
    }

    fn surface_line_height(&self) -> f32 {
        unreachable!("RawMacStatusBarSurface has no backend line height")
    }

    fn surface_char_width(&self) -> f32 {
        unreachable!("RawMacStatusBarSurface has no backend char width")
    }

    fn surface_measure_text(&self, text: &str) -> (f32, f32) {
        let (w, h) = super::text::measure_text(self.font, text);
        (w as f32, h as f32)
    }

    fn surface_fill_rect(&mut self, rect: crate::Rect, color: crate::Color) {
        // SAFETY: `ctx` is a valid `CGContextRef` for the caller's paint
        // pass — see this struct's construction site.
        unsafe { super::backend::ns_fill_rect(self.ctx, rect, color) };
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
        // SAFETY: `self.ctx` is the caller-supplied context passed to
        // `draw_status_bar`, valid for the duration of the shim call.
        unsafe {
            super::text::draw_text(
                self.ctx,
                self.font,
                text,
                rect.x as f64,
                rect.y as f64,
                super::backend::ns_color_to_cg(color),
            );
        }
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
        // SAFETY: see `surface_fill_rect`.
        unsafe { super::backend::ns_push_clip(self.ctx, rect) };
    }

    fn surface_pop_clip(&mut self) {
        // SAFETY: see `surface_fill_rect`.
        unsafe { super::backend::ns_pop_clip(self.ctx) };
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
/// direct `quadraui::macos::draw_status_bar` reference rather than going
/// through [`crate::Backend::draw_status_bar`] — the sanctioned entry
/// point, and the one every in-tree call site already uses, which is why
/// this shim has no in-repo caller left to trip the `-D
/// warnings`-denied `deprecated` lint.
///
/// # Safety
///
/// `ctx` must be a valid `CGContextRef` borrowed for the duration of
/// the call.
#[deprecated(
    since = "0.0.1",
    note = "call `Backend::draw_status_bar` instead — this free function is a compatibility shim over the shared #860 implementation"
)]
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
    let mut surface = RawMacStatusBarSurface { ctx, font };
    crate::primitives::status_bar::native_surface_paint::paint(
        bar,
        &mut surface,
        theme,
        x as f32,
        y as f32,
        width as f32,
        line_height as f32,
        hovered_id,
        pressed_id,
    )
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
    use crate::primitives::status_bar::StatusSegmentSide;
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
        paint_via_backend_at(bar, 0.0, 0.0)
    }

    /// Like [`paint_via_backend`] but paints at an arbitrary `(x, y)`
    /// origin instead of always `(0, 0)`. Lets tests exercise the
    /// paint-time `x`/`y` shift independently of the bar-local
    /// `StatusBarLayout` it returns.
    fn paint_via_backend_at(bar: &StatusBar, x: f32, y: f32) -> (BitmapSurface, StatusBarLayout) {
        let surface = BitmapSurface::new(W, H);
        // Clear so we can distinguish "painted by status_bar" from
        // "untouched memory" cleanly.
        surface.fill(0.0, 0.0, 0.0, 0.0);

        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));

        let layout = std::cell::RefCell::new(None);
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            let l = b.draw_status_bar_interactive(
                QRect::new(x, y, W as f32 - x, H as f32 - y),
                bar,
                &crate::InteractionState::new(),
            );
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

    /// Shared body for `round_trip_click_hits_clickable_segment` and its
    /// `_at_nonzero_origin` sibling. Paints `sample_bar()` at
    /// `(origin_x, origin_y)`, then round-trips an absolute click through
    /// localisation the way a real host (`ScreenLayout` / `AppLogic`)
    /// does before calling `hit_test`, and — since this file's rasteriser
    /// DOES use `x`/`y` for paint even though the returned layout is
    /// bar-local (see module doc comment) — also checks the *painted*
    /// segment lands at the origin-shifted absolute position. Guards
    /// against a #44-class paint/layout frame mismatch
    /// (quadraui#494 / LESSONS.md "Layout helpers must return coords in
    /// the same frame across backends").
    fn round_trip_click_hits_clickable_segment_at(origin_x: f32, origin_y: f32) {
        let bar = sample_bar();
        let (surface, layout) = paint_via_backend_at(&bar, origin_x, origin_y);

        let save = layout
            .visible_segments
            .iter()
            .filter(|vs| vs.side == StatusSegmentSide::Left)
            .nth(1)
            .expect("save segment visible");

        // Paint-vs-layout frame check: the segment's bg must be painted
        // at the origin-shifted absolute x, not at the bar-local x.
        let probe_x = (origin_x + save.bounds.x) as u32 + 1;
        let probe_y = origin_y as u32 + 2;
        let (r, g, b, _) = surface.pixel(probe_x, probe_y);
        assert_eq!(
            (r, g, b),
            (40, 80, 120),
            "painted Save segment bg should be at absolute x = origin_x + local bounds.x",
        );

        // Round trip: absolute click position -> localise like a real
        // host -> hit_test against the bar-local layout.
        let abs_x = origin_x + save.bounds.x + save.bounds.width * 0.5;
        let abs_y = origin_y + save.bounds.y + save.bounds.height * 0.5;
        let local_x = abs_x - origin_x;
        let local_y = abs_y - origin_y;
        let hit = layout.hit_test(local_x, local_y);
        assert_eq!(
            hit,
            StatusBarHit::Segment(WidgetId::new("status:save")),
            "expected clickable Save segment hit at local ({}, {})",
            local_x,
            local_y,
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
    fn round_trip_click_hits_clickable_segment() {
        round_trip_click_hits_clickable_segment_at(0.0, 0.0);
    }

    #[test]
    fn round_trip_click_hits_clickable_segment_at_nonzero_origin() {
        // The origin-(0,0) variant above can't exercise localisation —
        // `abs_x - x` is a no-op when `x == 0`. Paint at a non-zero
        // origin so the round trip only passes if paint and layout agree
        // on which frame `x`/`y` apply in.
        round_trip_click_hits_clickable_segment_at(7.0, 4.0);
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
            b.draw_status_bar_interactive(
                QRect::new(0.0, 0.0, W as f32, H as f32),
                &bar,
                &crate::InteractionState::new(),
            );
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

    /// `Backend::status_bar_layout` must return exactly what
    /// `draw_status_bar` painted — both route through
    /// `mac_status_bar_layout`, and this pins that (quadraui#484).
    #[test]
    fn layout_twin_matches_the_painted_layout() {
        let bar = sample_bar();
        let (_surface, painted) = paint_via_backend(&bar);

        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        let computed = backend.status_bar_layout(QRect::new(0.0, 0.0, W as f32, H as f32), &bar);

        assert_eq!(
            painted.visible_segments.len(),
            computed.visible_segments.len(),
        );
        for (p, c) in painted
            .visible_segments
            .iter()
            .zip(computed.visible_segments.iter())
        {
            assert_eq!(p.segment_idx, c.segment_idx);
            assert_eq!(p.side, c.side);
            assert!((p.bounds.x - c.bounds.x).abs() < 0.001);
            assert!((p.bounds.width - c.bounds.width).abs() < 0.001);
        }
    }

    /// The no-paint twin is also origin-independent: hit regions are
    /// bar-local, so painting at a non-zero origin does not move them
    /// (quadraui#552 — audited and ruled out for the status bar).
    #[test]
    fn layout_twin_is_bar_local_at_a_nonzero_origin() {
        let bar = sample_bar();
        let (_surface, painted) = paint_via_backend_at(&bar, 0.0, 0.0);
        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        let computed = backend.status_bar_layout(QRect::new(17.0, 3.0, W as f32, H as f32), &bar);
        for (p, c) in painted
            .visible_segments
            .iter()
            .zip(computed.visible_segments.iter())
        {
            assert!(
                (p.bounds.x - c.bounds.x).abs() < 0.001,
                "segment x moved with the origin: {} vs {}",
                p.bounds.x,
                c.bounds.x,
            );
        }
    }
}
