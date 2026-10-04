//! macOS rasteriser for [`crate::primitives::pipeline_view::PipelineView`].
//!
//! Painting moved to the shared
//! [`crate::primitives::pipeline_view::native_surface_paint::paint`]
//! (#1085, `PaintSurface` Phase 4 slice 8/8) — see that fn's doc for the
//! seven named divergences found while unifying
//! `gtk::pipeline_view::draw_pipeline_view`,
//! `macos::pipeline_view::draw_pipeline_view` and
//! `win::pipeline_view::draw_pipeline_view` into one implementation. This
//! module now only carries [`mac_pipeline_view_layout`] (still real,
//! backend-specific pure geometry — no painting involved); the
//! deprecated `draw_pipeline_view` compatibility shim over the shared
//! [`super::surface::CgSurface`] adapter was removed in issue #1109
//! (zero uses in coord-tui's `main` and vimcode's `develop`).

#[cfg(test)]
use core_text::font::CTFont;

use crate::primitives::layout_metrics::pixel_pipeline_view_layout;
use crate::primitives::pipeline_view::{PipelineView, PipelineViewLayout};
#[cfg(test)]
use crate::theme::Theme;

/// Compute the macOS pixel-unit layout for a [`PipelineView`]. Shares its
/// geometry with `gtk_pipeline_view_layout` / `win_pipeline_view_layout`
/// via [`pixel_pipeline_view_layout`] (issue #1079).
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

    /// Regression for #1085 divergence 4: macOS previously painted no
    /// tint at all behind the action button — this fixture's stage 1
    /// ("Test") has an action, so its `action_bounds` must now show a
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

    /// Regression for #1085 divergence 5: the action-button label used to
    /// paint flush to `ab.y` on macOS (ignoring the button's own height);
    /// it now centres like GTK/Windows, so the glyph ink should not touch
    /// the very top row of `action_bounds`.
    #[test]
    fn action_button_label_is_vertically_centred() {
        let view = make_view();
        let (surface, layout) = paint_via_backend(&view);
        let ab = layout.stages[1]
            .action_bounds
            .expect("stage 1 has an action button");
        // The top-most row of the action strip should be clear of glyph
        // ink (just the translucent tint) once the label is centred
        // rather than flush to the top.
        let mut top_row_has_dark_glyph_ink = false;
        for x in (ab.x as u32)..(ab.x + ab.width) as u32 {
            let (r, g, b, _) = surface.pixel(x, ab.y as u32);
            // Glyph ink here is painted in `accent_bg`; the tint alone is
            // a much darker blend toward the black background. Look for
            // a near-pure accent_bg pixel specifically at the top row.
            let theme = Theme::default();
            if (r, g, b) == (theme.accent_bg.r, theme.accent_bg.g, theme.accent_bg.b) {
                top_row_has_dark_glyph_ink = true;
            }
        }
        assert!(
            !top_row_has_dark_glyph_ink,
            "action label should be vertically centred in its button, not flush to the top row",
        );
    }

    /// Shared body for the action↔click round trip, run at both the
    /// origin and a non-zero origin (quadraui#494 / LESSONS.md "Layout
    /// helpers must return coords in the same frame across backends").
    /// `mac_pipeline_view_layout` bakes `x`/`y` straight into the
    /// returned bounds (absolute frame, matching the GTK/TUI twins) —
    /// and *also* adds
    /// [`crate::primitives::layout_metrics::pixel::PIPELINE_FOCUS_INDICATOR_H`]
    /// to `y` itself before laying out, an extra reason a non-zero-origin
    /// regression is plausible here. Deriving `ab`/`bb` from the layout
    /// (not hardcoding them) means this exercises whatever origin math
    /// the function actually does, at any origin. Calls
    /// `mac_pipeline_view_layout` directly (pure fn, no font/paint
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

    /// Non-zero-origin regression guard (quadraui#494).
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

    /// Non-zero-origin regression guard (quadraui#494).
    #[test]
    fn layout_hit_test_body_round_trip_at_nonzero_origin() {
        layout_hit_test_body_round_trip_at(7.0, 13.0);
    }
}
