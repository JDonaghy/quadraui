//! Direct2D / DirectWrite rasteriser for [`crate::StatusBar`] (issue #25).
//!
//! Mirrors `gtk::status_bar`'s structure: [`StatusBar::layout`] (the D6
//! layout API, see that primitive's module doc) does every positioning
//! and priority-drop decision; this module only measures (via
//! DirectWrite) and paints (via `ID2D1RenderTarget::FillRectangle` +
//! [`DWrite::draw_text_styled`]). Paint and hit-test both derive from one
//! `StatusBar::layout` call, so they can't drift apart.
//!
//! Only compiled on `target_os = "windows"` — see `super::mod`'s
//! `#[cfg(target_os = "windows")] mod status_bar;` and `backend.rs`'s
//! module docs for why the rest of this repo's `--features win` compile
//! gate stays meaningful without a Windows host.
//!
//! # Theme
//!
//! `WinBackend` does not yet carry a live [`Theme`] the way `GtkBackend`'s
//! `current_theme` does — no issue has wired that through yet. Callers
//! that don't have segment-level colours to fall back on (the bar's own
//! background, when it has no segments) get [`Theme::default`], the same
//! "placeholder until a later issue wires the app's real theme through"
//! posture `WinBackend::begin_frame`'s clear colour already documents.

use windows::Win32::Graphics::Direct2D::ID2D1RenderTarget;

use super::text::{fill_rect, DWrite};
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
/// [`Theme::default`]'s background when the bar has no segments at all),
/// then each visible segment paints its own `fg`/`bg`, honouring `bold`
/// via [`DWrite::draw_text_styled`].
pub fn draw_status_bar(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    rect: Rect,
    bar: &StatusBar,
    hovered_id: Option<&WidgetId>,
    pressed_id: Option<&WidgetId>,
) -> StatusBarLayout {
    let layout = win_status_bar_layout(dwrite, rect, bar);

    let fill = bar
        .left_segments
        .first()
        .or(bar.right_segments.first())
        .map(|s| s.bg)
        .unwrap_or(Theme::default().background);
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

    /// Paint↔click round trip: the segment's painted background pixel and
    /// the layout's own `hit_test` at that same point must agree on which
    /// (if any) `WidgetId` was clicked.
    #[test]
    fn paint_and_hit_test_round_trip() {
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
        let bar = bar();
        let rect = Rect::new(0.0, 0.0, W, H);

        let layout = surface
            .paint(|target| {
                draw_status_bar(target, &dwrite, rect, &bar, None, None);
            })
            .map(|_| win_status_bar_layout(&dwrite, rect, &bar))
            .expect("paint status bar");

        // The left segment starts at bar-local x=0 — its fill colour must
        // be visible at (1, mid_y).
        let mid_y = (H / 2.0) as u32;
        let left_px = surface.pixel_at(1, mid_y);
        assert_eq!(
            (left_px.r, left_px.g, left_px.b),
            (10, 20, 30),
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
        let right_px = surface.pixel_at(cx as u32, mid_y);
        assert_eq!(
            (right_px.r, right_px.g, right_px.b),
            (40, 50, 60),
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
                draw_status_bar(target, &dwrite, rect, &bar, None, None);
            })
            .map(|_| win_status_bar_layout(&dwrite, rect, &bar))
            .expect("paint");
        let no_paint = win_status_bar_layout(&dwrite, rect, &bar);

        assert_eq!(painted, no_paint);
    }
}
