//! macOS rasteriser for [`crate::primitives::pipeline_view::PipelineView`].
//!
//! Painting is implemented in the shared
//! [`crate::primitives::pipeline_view::native_surface_paint::paint`].
//! This module carries [`mac_pipeline_view_layout`], the backend-specific
//! pure geometry for pipeline view layout.

#[cfg(test)]
use core_text::font::CTFont;

use crate::primitives::layout_metrics::pixel_pipeline_view_layout;
use crate::primitives::pipeline_view::{PipelineView, PipelineViewLayout};
#[cfg(test)]
use crate::theme::Theme;

/// Compute the macOS pixel-unit layout for a [`PipelineView`]. Shares its
/// geometry with `gtk_pipeline_view_layout` / `win_pipeline_view_layout`
/// via [`pixel_pipeline_view_layout`].
pub fn mac_pipeline_view_layout(
    view: &PipelineView,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
) -> PipelineViewLayout {
    pixel_pipeline_view_layout(view, x as f32, y as f32, w as f32, h as f32)
}

#[cfg(test)]
mod tests {
    use super::super::headless::BitmapSurface;
    use super::super::text::make_font;
    use super::super::MacBackend;
    use super::*;
    use crate::event::{Rect as QRect, Viewport};
    use crate::primitives::pipeline_view::{PipelineHit, PipelineStage, StageStatus};
    use crate::types::WidgetId;
    use crate::Backend;

    const W: u32 = 300;
    const H: u32 = 80;

    fn font() -> CTFont {
        make_font("Menlo", 14.0).expect("Menlo installed")
    }

    fn make_view() -> PipelineView {
        PipelineView {
            id: WidgetId::new("pipe"),
            stages: vec![
                PipelineStage {
                    label: "Build".into(),
                    status: StageStatus::Done,
                    action: None,
                },
                PipelineStage {
                    label: "Test".into(),
                    status: StageStatus::Active,
                    action: Some("Retry".into()),
                },
            ],
            focused_stage: None,
        }
    }

    /// Paint via the real `Backend::draw_pipeline_view` path — which now
    /// routes through the shared `native_surface_paint::paint` — avoids
    /// tripping the `-D warnings`-denied `deprecated` lint (CLAUDE.md
    /// rule 3) by not calling the deprecated free-function shim above.
    fn paint_via_backend(view: &PipelineView) -> (BitmapSurface, PipelineViewLayout) {
        let surface = BitmapSurface::new(W, H);
        surface.fill(0.0, 0.0, 0.0, 0.0);
        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        let layout = std::cell::RefCell::new(None);
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            let l = b.draw_pipeline_view(QRect::new(0.0, 0.0, W as f32, H as f32), view);
            *layout.borrow_mut() = Some(l);
        });
        backend.end_frame();
        (surface, layout.into_inner().unwrap())
    }

    #[test]
    fn draws_without_panic_and_has_two_stages() {
        let view = make_view();
        let (_surface, layout) = paint_via_backend(&view);
        assert_eq!(layout.stages.len(), 2);
    }

    /// When a stage has an action button, its `action_bounds` displays a
    /// visible `accent_bg` tint rather than the plain box background.
    #[test]
    fn action_button_paints_a_translucent_accent_tint() {
        let view = make_view();
        let (surface, layout) = paint_via_backend(&view);
        let ab = layout.stages[1]
            .action_bounds
            .expect("stage 1 has an action button");
        // Probe near the tint's edge, clear of the button's own glyphs.
        let px = (ab.x + 2.0) as u32;
        let py = (ab.y + ab.height - 2.0) as u32;
        let (r, g, b, _a) = surface.pixel(px, py);
        assert_ne!(
            (r, g, b),
            (0, 0, 0),
            "action button area must show the accent tint, not the plain (black) box fill",
        );
    }

    /// The action-button label centres in its button like GTK/Windows
    /// rather than painting flush to `ab.y`. The button carries a 1px
    /// `accent_bg` border on its outer rows/columns, so the "no glyph
    /// ink near the top" probe starts two rows inside the border and
    /// skips the side-border columns; the border itself is asserted
    /// separately on the top row.
    #[test]
    fn action_button_label_is_vertically_centred() {
        let view = make_view();
        let (surface, layout) = paint_via_backend(&view);
        let ab = layout.stages[1]
            .action_bounds
            .expect("stage 1 has an action button");
        let theme = Theme::default();
        let accent = (theme.accent_bg.r, theme.accent_bg.g, theme.accent_bg.b);
        let is_accent = |x: u32, y: u32| {
            let (r, g, b, _) = surface.pixel(x, y);
            (r, g, b) == accent
        };

        let top = ab.y as u32;
        let x0 = ab.x as u32;
        let x1 = (ab.x + ab.width) as u32;

        assert!(
            (x0..x1).any(|x| is_accent(x, top)),
            "the action button's top row should carry its accent border",
        );

        // Glyph ink is painted in `accent_bg`; the tint alone is a much
        // darker blend toward the black background. A near-pure
        // accent_bg pixel just inside the border means the label is
        // hugging the top of the button.
        let probe_y = top + 2;
        let top_rows_have_glyph_ink =
            ((x0 + 2)..x1.saturating_sub(2)).any(|x| is_accent(x, probe_y));
        assert!(
            !top_rows_have_glyph_ink,
            "action label should be vertically centred in its button, not flush to the top row",
        );
    }

    /// Shared body for the action↔click round trip, run at both the
    /// origin and a non-zero origin. `mac_pipeline_view_layout` bakes
    /// `x`/`y` straight into the returned bounds (absolute frame, matching
    /// the GTK/TUI twins) and also adds
    /// [`crate::primitives::layout_metrics::pixel::PIPELINE_FOCUS_INDICATOR_H`]
    /// to `y` itself before laying out. Deriving `ab`/`bb` from the layout
    /// (not hardcoding them) exercises the origin math across backends.
    /// Calls `mac_pipeline_view_layout` directly (pure fn, no font/paint
    /// dependency — it forwards to `PipelineView::layout`, which is
    /// plain geometry).
    fn layout_hit_test_action_round_trip_at(origin_x: f64, origin_y: f64) {
        let view = make_view();
        let layout = mac_pipeline_view_layout(&view, origin_x, origin_y, 300.0, 80.0);

        // Box top must sit exactly at origin_y + PIPELINE_FOCUS_INDICATOR_H,
        // not a hardcoded absolute value — pins the offset math
        // independently of the hit_test round trip below.
        let bb0 = layout.stages[0].box_bounds;
        assert!(
            (bb0.y as f64
                - (origin_y
                    + (crate::primitives::layout_metrics::pixel::PIPELINE_FOCUS_INDICATOR_H
                        as f64)))
                .abs()
                < 0.001,
            "stage box top should be origin_y + PIPELINE_FOCUS_INDICATOR_H, got {}",
            bb0.y,
        );

        // Stage 1 has action bounds.
        let ab = layout.stages[1]
            .action_bounds
            .expect("action bounds for stage 1");
        let hit = layout.hit_test(ab.x + ab.width / 2.0, ab.y + ab.height / 2.0);
        assert_eq!(hit, PipelineHit::Action(1));
    }

    #[test]
    fn layout_hit_test_action_round_trip() {
        layout_hit_test_action_round_trip_at(0.0, 0.0);
    }

    /// Non-zero-origin regression guard.
    #[test]
    fn layout_hit_test_action_round_trip_at_nonzero_origin() {
        layout_hit_test_action_round_trip_at(7.0, 13.0);
    }

    fn layout_hit_test_body_round_trip_at(origin_x: f64, origin_y: f64) {
        let view = make_view();
        let layout = mac_pipeline_view_layout(&view, origin_x, origin_y, 300.0, 80.0);

        let bb = layout.stages[0].box_bounds;
        assert!(
            (bb.y as f64
                - (origin_y
                    + (crate::primitives::layout_metrics::pixel::PIPELINE_FOCUS_INDICATOR_H
                        as f64)))
                .abs()
                < 0.001,
            "stage box top should be origin_y + PIPELINE_FOCUS_INDICATOR_H, got {}",
            bb.y,
        );
        // Stage 0 has no action, so a click inside its box resolves to Body.
        let hit = layout.hit_test(bb.x + 1.0, bb.y + 1.0);
        assert_eq!(hit, PipelineHit::Body(0));
    }

    #[test]
    fn layout_hit_test_body_round_trip() {
        layout_hit_test_body_round_trip_at(0.0, 0.0);
    }

    /// Non-zero-origin regression guard.
    #[test]
    fn layout_hit_test_body_round_trip_at_nonzero_origin() {
        layout_hit_test_body_round_trip_at(7.0, 13.0);
    }
}
