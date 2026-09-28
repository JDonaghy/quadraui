//! Direct2D / DirectWrite rasteriser for
//! [`crate::primitives::pipeline_view::PipelineView`] (#735).
//!
//! Painting moved to the shared
//! [`crate::primitives::pipeline_view::native_surface_paint::paint`]
//! (#1085, `NativeSurface` Phase 4 slice 8/8) — see that fn's doc for the
//! seven named divergences found while unifying
//! `gtk::pipeline_view::draw_pipeline_view`,
//! `macos::pipeline_view::draw_pipeline_view` and
//! `win::pipeline_view::draw_pipeline_view` into one implementation
//! (several of which — the straight-rect stage-box border, the two-line
//! chevron arrow head/focus indicator, the clamped+clipped label — were
//! already this backend's own behaviour and are now what every backend
//! shares). This module now only carries [`win_pipeline_view_layout`]
//! (still real, backend-specific pure geometry — no painting involved)
//! and the deprecated [`draw_pipeline_view`] compatibility shim over the
//! shared [`super::surface::D2dSurface`] adapter (mirrors
//! `win::diff_view`'s #866 shim).
//!
//! Only compiled on `target_os = "windows"` — see `super::mod`'s
//! `#[cfg(target_os = "windows")] mod pipeline_view;` and `backend.rs`'s
//! module docs for why the rest of this repo's `--features win` compile
//! gate stays meaningful without a Windows host.

use windows::Win32::Graphics::Direct2D::ID2D1RenderTarget;

use super::text::DWrite;
use crate::event::Rect;
use crate::primitives::layout_metrics::pixel_pipeline_view_layout;
use crate::primitives::pipeline_view::{PipelineView, PipelineViewLayout};
use crate::theme::Theme;

/// Compute the Win-GUI DIP-unit layout for a [`PipelineView`] without
/// painting — the DirectWrite twin of [`draw_pipeline_view`]'s internal
/// layout call. Shares its geometry with `gtk_pipeline_view_layout` /
/// `mac_pipeline_view_layout` via [`pixel_pipeline_view_layout`] (issue
/// #1079).
///
/// Note: the returned layout (incl. `bounds`) is offset down by
/// [`pixel::PIPELINE_FOCUS_INDICATOR_H`], so `bounds.y` starts below the
/// reserved caret strip. The focus caret is painted in the gap between
/// `rect.y` and `bounds.y`; a host that clips drawing to `layout.bounds`
/// would clip the caret — clip to the original `rect` instead. Same
/// contract as the GTK/macOS/TUI twins' `*_pipeline_view_layout`.
pub fn win_pipeline_view_layout(view: &PipelineView, rect: Rect) -> PipelineViewLayout {
    pixel_pipeline_view_layout(view, rect.x, rect.y, rect.width, rect.height)
}

/// Deprecated free-function shim (#1085, CLAUDE.md rule 8): reproduces
/// the pre-#1085 signature exactly for any external caller that held a
/// direct `quadraui::win::draw_pipeline_view` reference rather than
/// going through [`crate::Backend::draw_pipeline_view`] — the sanctioned
/// entry point, and the one every in-tree call site already uses, which
/// is why this shim has no in-repo caller left to trip the `-D
/// warnings`-denied `deprecated` lint.
#[deprecated(
    since = "0.0.1",
    note = "call `Backend::draw_pipeline_view` instead — this free function is a compatibility shim over the shared #1085 implementation"
)]
pub fn draw_pipeline_view(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    rect: Rect,
    view: &PipelineView,
    theme: &Theme,
) -> PipelineViewLayout {
    let mut surface = super::surface::D2dSurface {
        target,
        dwrite: Some(dwrite),
    };
    crate::primitives::pipeline_view::native_surface_paint::paint(view, &mut surface, theme, rect)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::layout_metrics::pixel;
    use crate::primitives::pipeline_view::{PipelineHit, PipelineStage, StageStatus};
    use crate::types::{Color, WidgetId};
    use crate::win::testing::HeadlessSurface;

    const W: f32 = 300.0;
    const H: f32 = 80.0;

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

    /// Paint `view` via the shared
    /// [`crate::primitives::pipeline_view::native_surface_paint::paint`]
    /// through a [`super::super::surface::D2dSurface`] over `surface`'s
    /// headless target — the same adapter the deprecated
    /// [`draw_pipeline_view`] shim uses, exercised here directly so these
    /// tests don't trip the `-D warnings`-denied `deprecated` lint
    /// (CLAUDE.md rule 3; mirrors `win::diff_view`'s identical
    /// test-migration note).
    fn paint(
        surface: &HeadlessSurface,
        dwrite: &DWrite,
        rect: Rect,
        view: &PipelineView,
        theme: &Theme,
    ) -> PipelineViewLayout {
        surface
            .paint(|target| {
                let mut raw = super::super::surface::D2dSurface {
                    target,
                    dwrite: Some(dwrite),
                };
                crate::primitives::pipeline_view::native_surface_paint::paint(
                    view, &mut raw, theme, rect,
                );
            })
            .map(|_| win_pipeline_view_layout(view, rect))
            .expect("paint pipeline view")
    }

    /// C0 smoke: `draw_pipeline_view` must actually paint text + a
    /// click-routable layout rather than panicking or hitting a
    /// `todo!()` (#735's acceptance bar — "draw_pipeline_view survives C0
    /// with text_ok on win"), and the Tier-1 conformance scenario
    /// `pipeline.click_advances_stage` needs a real box to click.
    #[test]
    fn draw_pipeline_view_paints_text_and_returns_layout() {
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let theme = Theme {
            background: Color::rgb(255, 255, 255),
            surface_bg: Color::rgb(255, 255, 255),
            foreground: Color::rgb(0, 0, 0),
            ..Theme::default()
        };
        let view = make_view();
        let rect = Rect::new(0.0, 0.0, W, H);

        let layout = paint(&surface, &dwrite, rect, &view, &theme);

        assert_eq!(layout.stages.len(), 2);

        // "text_ok" — some non-background pixel actually painted inside
        // the first stage's label area (proves DrawText ran, not just the
        // border/fill).
        let bb = layout.stages[0].box_bounds;
        let mut painted_any = false;
        for x in (bb.x as u32)..(bb.x + bb.width) as u32 {
            for y in (bb.y as u32)..(bb.y + bb.height) as u32 {
                let px = surface.pixel_at(x, y);
                if (px.r, px.g, px.b) != (255, 255, 255) {
                    painted_any = true;
                }
            }
        }
        assert!(
            painted_any,
            "expected pipeline_view to paint visible glyphs"
        );
    }

    /// Paint↔click round trip (`docs/TESTING.md` coverage-taxonomy row 1)
    /// at a non-zero origin — #505's LOCAL/ABSOLUTE mixup regression
    /// guard, mirrored from `win::sidebar_panel`/`win::text_input`'s own
    /// nonzero-origin tests. Also exercises the action-button hit, which
    /// is what the Tier-1 `pipeline.click_advances_stage` scenario clicks.
    #[test]
    fn paint_and_click_round_trip_action_button_at_nonzero_origin() {
        let origin_x = 12.0_f32;
        let origin_y = 5.0_f32;
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let theme = Theme::default();
        let view = make_view();
        let rect = Rect::new(origin_x, origin_y, W - origin_x, H - origin_y);

        let layout = paint(&surface, &dwrite, rect, &view, &theme);

        // Box top must sit exactly at origin_y + PIPELINE_FOCUS_INDICATOR_H, not
        // a hardcoded absolute value — pins the offset math independently
        // of the hit_test round trip below.
        let bb0 = layout.stages[0].box_bounds;
        assert!(
            (bb0.y - (origin_y + pixel::PIPELINE_FOCUS_INDICATOR_H)).abs() < 0.001,
            "stage box top should be origin_y + PIPELINE_FOCUS_INDICATOR_H, got {}",
            bb0.y,
        );

        // Stage 1 ("Test") has an action button.
        let ab = layout.stages[1]
            .action_bounds
            .expect("action bounds for stage 1");
        let hit = layout.hit_test(ab.x + ab.width / 2.0, ab.y + ab.height / 2.0);
        assert_eq!(hit, PipelineHit::Action(1));

        // Stage 0 ("Build") has no action — a click in its body resolves
        // to Body, not Action.
        let hit0 = layout.hit_test(bb0.x + 2.0, bb0.y + 2.0);
        assert_eq!(hit0, PipelineHit::Body(0));
    }

    /// No-paint layout must agree byte-for-byte with what
    /// `draw_pipeline_view` painted — same contract every other `win::`
    /// rasteriser's `no_paint_layout_matches_paint_layout` test proves
    /// (see `win::sidebar_panel`, `win::text_input`).
    #[test]
    fn no_paint_layout_matches_paint_layout() {
        let view = make_view();
        let rect = Rect::new(0.0, 0.0, W, H);
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");

        let painted = paint(&surface, &dwrite, rect, &view, &Theme::default());
        let no_paint = win_pipeline_view_layout(&view, rect);
        assert_eq!(painted, no_paint);
    }

    /// Zero-size rect is a no-op — mirrors every other `win::` rasteriser's
    /// same guard (see `win::text_input::zero_width_rect_is_a_no_op`).
    #[test]
    fn zero_size_rect_is_a_no_op() {
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let theme = Theme::default();
        let view = make_view();
        let rect = Rect::new(0.0, 0.0, 0.0, H);

        surface
            .fill_rect(Rect::new(0.0, 0.0, W, H), Color::rgb(255, 255, 255))
            .expect("fill background");

        paint(&surface, &dwrite, rect, &view, &theme);

        let px = surface.pixel_at(1, 1);
        assert_eq!(
            (px.r, px.g, px.b),
            (255, 255, 255),
            "a zero-width pipeline view should paint nothing at all",
        );
    }

    /// Regression for #1085 divergence 4: the action-button tint is now a
    /// real [`crate::native_surface::NativeSurface::surface_fill_rect_alpha`]
    /// composite instead of a CPU-side `Color::blend` — probing just
    /// above the button's centre (clear of the label glyphs) must show a
    /// colour strictly between the plain box background and a fully
    /// opaque `accent_bg`.
    #[test]
    fn action_tint_is_a_real_alpha_composite_not_a_cpu_blend() {
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let theme = Theme::default();
        let view = make_view();
        let rect = Rect::new(0.0, 0.0, W, H);

        let layout = paint(&surface, &dwrite, rect, &view, &theme);
        let ab = layout.stages[1]
            .action_bounds
            .expect("stage 1 has an action button");
        let px = surface.pixel_at((ab.x + 2.0) as u32, ab.y as u32);
        assert_ne!(
            (px.r, px.g, px.b),
            (theme.surface_bg.r, theme.surface_bg.g, theme.surface_bg.b),
            "action button area must show the accent tint, not the plain box fill",
        );
        assert_ne!(
            (px.r, px.g, px.b),
            (theme.accent_bg.r, theme.accent_bg.g, theme.accent_bg.b),
            "a 0.15-alpha tint must not paint fully opaque accent_bg",
        );
    }
}
