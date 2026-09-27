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
//! the shared [`super::surface::D2dSurface`] adapter (#1072 — consolidated
//! from this module's own private `RawWinStatusBarSurface`). `MIN_GAP_DIP`
//! stays put — it's still [`win_status_bar_layout`]'s own measurer
//! constant, untouched by this migration.
//!
//! Issue #1078: only [`draw_status_bar`] (the deprecated paint shim) is
//! Windows-only. [`win_status_bar_layout`] is pure geometry generic over
//! [`StatusMeasure`] — no Direct2D/DirectWrite type in its signature —
//! so it compiles and runs everywhere, including a plain `cargo test
//! --features win` on Linux. `super::mod`'s `mod status_bar;` is no
//! longer whole-module gated; see `backend.rs`'s module docs.
//!
//! # Theme
//!
//! Takes the live theme as a `&Theme` parameter (quadraui#789) — the
//! caller ([`crate::win::WinBackend::draw_status_bar`]) passes
//! `&self.current_theme`, the same field `Backend::set_theme` writes.
//! Callers that don't have segment-level colours to fall back on (the
//! bar's own background, when it has no segments) get that live theme's
//! background, not [`Theme::default`].

#[cfg(target_os = "windows")]
use windows::Win32::Graphics::Direct2D::ID2D1RenderTarget;

#[cfg(target_os = "windows")]
use super::text::DWrite;
use crate::event::Rect;
use crate::primitives::status_bar::{
    StatusSegmentMeasure, PIXEL_EDGE_INSET, PIXEL_SEGMENT_PADDING,
};
#[cfg(target_os = "windows")]
use crate::theme::Theme;
#[cfg(target_os = "windows")]
use crate::types::WidgetId;
use crate::{StatusBar, StatusBarLayout};

/// Minimum gap (DIPs) reserved between the left and right segment
/// groups — the DirectWrite twin of [`crate::gtk::MIN_GAP_PX`]. Still
/// used by [`win_status_bar_layout`] — the shared
/// [`crate::primitives::status_bar::native_surface_paint::paint`] carries
/// its own independent copy of the same value (see that module's doc).
pub const MIN_GAP_DIP: f32 = 16.0;

/// Bold-aware text measurement for [`win_status_bar_layout`]. A status
/// segment's `bold` flag changes its measured width, so this can't reuse
/// [`crate::primitives::layout_metrics::TextMeasure`] (no `bold`
/// parameter) the way `win::tab_bar`/`win::data_table` do — a real
/// difference this rasteriser has always had (`DWrite::measure_text_styled`
/// vs. every other module's plain `measure_text`), not new duplication.
pub trait StatusMeasure {
    fn width_of(&self, text: &str, bold: bool) -> f32;
}

#[cfg(target_os = "windows")]
impl StatusMeasure for DWrite {
    fn width_of(&self, text: &str, bold: bool) -> f32 {
        self.measure_text_styled(text, bold)
            .map(|(w, _)| w)
            .unwrap_or(0.0)
    }
}

/// Compute a [`StatusBar`]'s layout without painting — the measurer twin
/// of the shared `paint`, and what
/// [`crate::win::WinBackend::status_bar_layout`] calls directly. Both this
/// function and `paint` measure a segment's width the same bold-aware
/// way, so a no-paint hit-test call always agrees with what the last
/// paint drew.
pub fn win_status_bar_layout(
    measure: &dyn StatusMeasure,
    rect: Rect,
    bar: &StatusBar,
) -> StatusBarLayout {
    // #1155: `layout_padded` (not plain `layout`) so this no-paint twin
    // agrees with the shared `native_surface_paint::paint`'s outer edge
    // inset + per-segment padding — see that fn's doc.
    bar.layout_padded(
        rect.width,
        rect.height,
        MIN_GAP_DIP,
        PIXEL_EDGE_INSET,
        PIXEL_SEGMENT_PADDING,
        |seg| StatusSegmentMeasure::new(measure.width_of(&seg.text, seg.bold)),
    )
}

/// Deprecated free-function shim (#860, CLAUDE.md rule 8): reproduces
/// the pre-#860 signature exactly for any external caller that held a
/// direct `quadraui::win::draw_status_bar` reference rather than going
/// through [`crate::Backend::draw_status_bar`] — the sanctioned entry
/// point, and the one every in-tree call site already uses, which is why
/// this shim has no in-repo caller left to trip the `-D
/// warnings`-denied `deprecated` lint.
#[cfg(target_os = "windows")]
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
    let mut surface = super::surface::D2dSurface {
        target,
        dwrite: Some(dwrite),
    };
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

// #1155: pins `win_status_bar_layout`'s outer-edge-inset + per-segment
// padding numerically, the same way `gtk::testing`'s
// `find_locates_status_bar_segment_by_text` and
// `primitives::status_bar`'s own unit tests do — see this issue's
// non-blocking review note that Windows had no test asserting the actual
// inset numbers, relying entirely on the shared `layout_padded` unit
// tests. Unlike the `#[cfg(target_os = "windows")]`-gated `mod tests`
// below (which needs a real `DWrite`), this measures through a trivial
// fake `StatusMeasure` — `win_status_bar_layout` is pure geometry (see
// this module's doc, "Issue #1078") — so it runs on every host, including
// this Linux sandbox's `cargo test --features win`.
#[cfg(test)]
mod layout_padding_tests {
    use super::{win_status_bar_layout, StatusMeasure, MIN_GAP_DIP};
    use crate::event::Rect;
    use crate::primitives::status_bar::{
        StatusBarSegment, PIXEL_EDGE_INSET, PIXEL_SEGMENT_PADDING,
    };
    use crate::types::{Color, WidgetId};
    use crate::StatusBar;

    /// Fixed-width-per-character fake — no real font metrics needed, just
    /// something deterministic to measure `layout_padded`'s inset/padding
    /// arithmetic against.
    struct FixedWidthMeasure {
        px_per_char: f32,
    }

    impl StatusMeasure for FixedWidthMeasure {
        fn width_of(&self, text: &str, _bold: bool) -> f32 {
            text.chars().count() as f32 * self.px_per_char
        }
    }

    fn segment(text: &str) -> StatusBarSegment {
        StatusBarSegment {
            text: text.to_string(),
            fg: Color::rgb(0, 0, 0),
            bg: Color::rgb(10, 20, 30),
            bold: false,
            action_id: None,
        }
    }

    /// A lone left segment must start `PIXEL_EDGE_INSET` in from the
    /// bar's own left edge (issue #1155), not flush at x=0.
    #[test]
    fn lone_left_segment_starts_after_edge_inset() {
        let measure = FixedWidthMeasure { px_per_char: 8.0 };
        let bar = StatusBar {
            id: WidgetId::new("status"),
            left_segments: vec![segment("hi")],
            right_segments: vec![],
        };
        let layout = win_status_bar_layout(&measure, Rect::new(0.0, 0.0, 400.0, 20.0), &bar);

        let left = layout
            .visible_segments
            .first()
            .expect("left segment visible");
        assert_eq!(
            left.bounds.x, PIXEL_EDGE_INSET,
            "left-most segment should start `PIXEL_EDGE_INSET` in from x=0"
        );
    }

    /// A lone right segment must end `PIXEL_EDGE_INSET` short of the
    /// bar's own right edge (issue #1155), not flush against it.
    #[test]
    fn lone_right_segment_ends_before_edge_inset() {
        let measure = FixedWidthMeasure { px_per_char: 8.0 };
        let bar_width = 400.0;
        let bar = StatusBar {
            id: WidgetId::new("status"),
            left_segments: vec![],
            right_segments: vec![segment("bye")],
        };
        let layout = win_status_bar_layout(&measure, Rect::new(0.0, 0.0, bar_width, 20.0), &bar);

        let right = layout
            .visible_segments
            .first()
            .expect("right segment visible");
        assert_eq!(
            right.bounds.x + right.bounds.width,
            bar_width - PIXEL_EDGE_INSET,
            "right-most segment should end `PIXEL_EDGE_INSET` short of the bar's right edge"
        );
    }

    /// Each segment's measured text reserves `PIXEL_SEGMENT_PADDING` on
    /// both sides — its bounds are wider than the raw measured text
    /// width by `2 * PIXEL_SEGMENT_PADDING`.
    #[test]
    fn segment_bounds_add_padding_on_both_sides_of_measured_text() {
        let px_per_char = 8.0;
        let measure = FixedWidthMeasure { px_per_char };
        let text = "hi";
        let bar = StatusBar {
            id: WidgetId::new("status"),
            left_segments: vec![segment(text)],
            right_segments: vec![],
        };
        let layout = win_status_bar_layout(&measure, Rect::new(0.0, 0.0, 400.0, 20.0), &bar);

        let left = layout
            .visible_segments
            .first()
            .expect("left segment visible");
        let measured_width = text.chars().count() as f32 * px_per_char;
        assert_eq!(
            left.bounds.width,
            measured_width + 2.0 * PIXEL_SEGMENT_PADDING,
            "segment bounds should be the measured text width plus \
             `PIXEL_SEGMENT_PADDING` on both sides"
        );
    }

    /// Sanity: `MIN_GAP_DIP` is still reserved between left and right
    /// groups on top of the new edge inset/padding, i.e. this change
    /// composes with the pre-existing gap rather than replacing it.
    #[test]
    fn min_gap_still_reserved_between_groups() {
        let measure = FixedWidthMeasure { px_per_char: 8.0 };
        let bar = StatusBar {
            id: WidgetId::new("status"),
            left_segments: vec![segment("L")],
            right_segments: vec![segment("R")],
        };
        // Bar just wide enough for both groups plus insets/padding, with
        // exactly `MIN_GAP_DIP` of slack between them.
        let left_w = 1.0 * 8.0 + 2.0 * PIXEL_SEGMENT_PADDING;
        let right_w = 1.0 * 8.0 + 2.0 * PIXEL_SEGMENT_PADDING;
        let width = 2.0 * PIXEL_EDGE_INSET + left_w + right_w + MIN_GAP_DIP;
        let layout = win_status_bar_layout(&measure, Rect::new(0.0, 0.0, width, 20.0), &bar);

        assert_eq!(
            layout.visible_segments.len(),
            2,
            "both segments should still fit with exactly MIN_GAP_DIP of slack"
        );
        let left = &layout.visible_segments[0];
        let right = &layout.visible_segments[1];
        assert!(
            (right.bounds.x - (left.bounds.x + left.bounds.width) - MIN_GAP_DIP).abs() < 0.01,
            "gap between groups should be exactly MIN_GAP_DIP"
        );
    }
}

// #1078: every test below paints through a real `DWrite`/`HeadlessSurface`
// — gated the same way the whole module used to be, rather than
// pretending they run on Linux.
#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::*;
    use crate::primitives::status_bar::{StatusBarHit, StatusBarSegment, StatusSegmentSide};
    use crate::types::{Color, WidgetId};
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
    /// through a [`super::super::surface::D2dSurface`] over `surface`'s headless
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
                let mut raw = super::super::surface::D2dSurface {
                    target,
                    dwrite: Some(dwrite),
                };
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
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
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
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
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
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");

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
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let bar = bar();
        let rect = Rect::new(0.0, 0.0, 0.0, 0.0);

        let surface = HeadlessSurface::new(10, 10).expect("create surface");
        let painted = paint(&surface, &dwrite, rect, &bar, None, None);

        assert_eq!(painted, win_status_bar_layout(&dwrite, rect, &bar));
    }
}
