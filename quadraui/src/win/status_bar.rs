//! Direct2D / DirectWrite rasteriser for [`crate::StatusBar`] (issue #25).
//!
//! Mirrors `gtk::status_bar`'s structure: [`StatusBar::layout`] (the D6
//! layout API, see that primitive's module doc) does every positioning
//! and priority-drop decision; this module only measures (via
//! DirectWrite) and paints (via `ID2D1RenderTarget::FillRectangle` +
//! [`DWrite::draw_text_styled`]). Paint and hit-test both derive from one
//! `StatusBar::layout` call, so they can't drift apart.
//!
//! [`draw_status_bar`] clips to its own `rect` and short-circuits on a
//! non-positive width/height (quadraui#791), matching
//! `macos::status_bar::draw_status_bar` — segments can legitimately
//! overflow their own bounds (`StatusBar::layout` keeps at least the
//! highest-priority right segment "even if it alone overflows"), so
//! without the clip that overflow painted straight past the bar's edge
//! on narrow windows.
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

use super::text::{fill_rect, pop_clip, push_clip, DWrite};
use crate::event::Rect;
use crate::primitives::status_bar::{StatusBarSegment, StatusSegmentMeasure};
use crate::theme::Theme;
use crate::types::WidgetId;
use crate::{StatusBar, StatusBarLayout, StatusSegmentSide};

/// Minimum gap (DIPs) reserved between the left and right segment
/// groups — the DirectWrite twin of [`crate::gtk::MIN_GAP_PX`]. Both are
/// 1/96in-scaled pixel units, so the same constant value applies.
pub const MIN_GAP_DIP: f32 = 16.0;

/// Compute a [`StatusBar`]'s layout without painting — the DirectWrite
/// measurer twin of [`draw_status_bar`], and what
/// [`crate::win::WinBackend::status_bar_layout`] calls directly. Both
/// this function and [`draw_status_bar`] call [`StatusBar::layout`] with
/// the identical per-segment measurer, so a no-paint hit-test call always
/// agrees with what the last paint drew.
pub fn win_status_bar_layout(dwrite: &DWrite, rect: Rect, bar: &StatusBar) -> StatusBarLayout {
    bar.layout(rect.width, rect.height, MIN_GAP_DIP, |seg| {
        measure_segment(dwrite, seg)
    })
}

fn measure_segment(dwrite: &DWrite, seg: &StatusBarSegment) -> StatusSegmentMeasure {
    let (w, _) = dwrite
        .measure_text_styled(&seg.text, seg.bold)
        .unwrap_or((0.0, 0.0));
    StatusSegmentMeasure::new(w)
}

/// Draw a [`StatusBar`] into `rect` (DIPs) on `target`. Returns the
/// resolved [`StatusBarLayout`] for host click dispatch — same contract
/// as [`crate::Backend::draw_status_bar`]: hit regions are **bar-local**
/// (relative to `rect.x` / `rect.y`), matching every other backend
/// (issue #552 audit — see that method's doc).
///
/// `hovered_id` / `pressed_id` tint the matching clickable segment's
/// background, mirroring `gtk::draw_status_bar` (the primitive itself
/// carries no mouse state).
///
/// The bar is filled with the first segment's `bg` (falling back to
/// `theme`'s background when the bar has no segments at all), then each
/// visible segment paints its own `fg`/`bg`, honouring `bold` via
/// [`DWrite::draw_text_styled`].
///
/// Zero-size guard and clip mirror `macos::status_bar::draw_status_bar`
/// (quadraui#791): a degenerate `rect` short-circuits to the no-paint
/// layout, and painting is clipped to `rect` so segment text can't
/// overflow the bar on narrow windows.
pub fn draw_status_bar(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    rect: Rect,
    bar: &StatusBar,
    hovered_id: Option<&WidgetId>,
    pressed_id: Option<&WidgetId>,
    theme: &Theme,
) -> StatusBarLayout {
    if rect.width <= 0.0 || rect.height <= 0.0 {
        return win_status_bar_layout(dwrite, rect, bar);
    }

    push_clip(target, rect);

    let layout = win_status_bar_layout(dwrite, rect, bar);

    let fill = bar
        .left_segments
        .first()
        .or(bar.right_segments.first())
        .map(|s| s.bg)
        .unwrap_or(theme.background);
    let _ = fill_rect(target, rect, fill);

    for vs in &layout.visible_segments {
        let seg = match vs.side {
            StatusSegmentSide::Left => &bar.left_segments[vs.segment_idx],
            StatusSegmentSide::Right => &bar.right_segments[vs.segment_idx],
        };
        let seg_rect = Rect::new(
            rect.x + vs.bounds.x,
            rect.y + vs.bounds.y,
            vs.bounds.width,
            vs.bounds.height,
        );

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
        let _ = fill_rect(target, seg_rect, effective_bg);
        let _ = dwrite.draw_text_styled(target, &seg.text, seg_rect, seg.fg, seg.bold);
    }

    pop_clip(target);

    layout
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::status_bar::StatusBarHit;
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

        let layout = surface
            .paint(|target| {
                draw_status_bar(target, &dwrite, rect, &bar, None, None, &Theme::default());
            })
            .map(|_| win_status_bar_layout(&dwrite, rect, &bar))
            .expect("paint status bar");

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
    /// identical `hit_regions` to what `draw_status_bar` used to paint —
    /// same measurer, same bar, same rect.
    #[test]
    fn no_paint_layout_matches_paint_layout() {
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
        let bar = bar();
        let rect = Rect::new(0.0, 0.0, W, H);

        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
        let painted = surface
            .paint(|target| {
                draw_status_bar(target, &dwrite, rect, &bar, None, None, &Theme::default());
            })
            .map(|_| win_status_bar_layout(&dwrite, rect, &bar))
            .expect("paint");
        let no_paint = win_status_bar_layout(&dwrite, rect, &bar);

        assert_eq!(painted, no_paint);
    }

    /// Regression for quadraui#791: `draw_status_bar` had no clip and no
    /// zero-size guard, so a bar narrower than its segments' measured
    /// text let that text overflow the bar's own rect (segments never
    /// priority-drop on the left, and the right side always keeps at
    /// least its highest-priority segment "even if it alone overflows" —
    /// see `StatusBar::layout`'s doc). Paint a status bar inset in a
    /// larger canvas, deliberately narrower than its segment text, and
    /// assert every pixel outside the bar's own rect stays untouched.
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
        surface
            .paint(|target| {
                draw_status_bar(
                    target,
                    &dwrite,
                    bar_rect,
                    &bar,
                    None,
                    None,
                    &Theme::default(),
                );
            })
            .expect("paint status bar");

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
        let painted = surface
            .paint(|target| {
                draw_status_bar(target, &dwrite, rect, &bar, None, None, &Theme::default());
            })
            .map(|_| win_status_bar_layout(&dwrite, rect, &bar))
            .expect("paint must not panic on zero size");

        assert_eq!(painted, win_status_bar_layout(&dwrite, rect, &bar));
    }
}
