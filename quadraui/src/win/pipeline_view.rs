//! Direct2D / DirectWrite rasteriser for
//! [`crate::primitives::pipeline_view::PipelineView`] (#735).
//!
//! Mirrors the gtk/macos/tui twins' structure: [`PipelineView::layout`]
//! (the shared layout API) resolves every stage's box/icon/label/action/
//! arrow bounds — this module only measures (fixed DIP constants, same
//! posture as the GTK/macOS/TUI twins' own `*_ARROW_WIDTH_*` /
//! `*_ACTION_HEIGHT_*` constants — pipeline geometry needs no per-glyph
//! text measurement to lay out, only to paint) and paints via
//! [`DWrite::draw_text`]/[`fill_rect`]/[`stroke_rect`]/[`draw_line`].
//!
//! The `status → glyph` / `status → colour` tables are **not** duplicated
//! here. They lived three times over (gtk, macos, tui) before this issue;
//! #713's primitive-first rule forbids a fourth copy, so this rasteriser
//! (and the three pre-existing ones, migrated in the same PR) all call
//! [`crate::primitives::pipeline_view::status_glyph`] /
//! [`crate::primitives::pipeline_view::status_color`] instead.
//!
//! No rounded-rect helper exists in `win::text` (see `win::toolbar`'s and
//! `win::command_center`'s module docs for the same note — Direct2D needs
//! an `ID2D1RoundedRectangleGeometry` for a rounded one), so the stage box
//! paints as a straight-edged rectangle via [`stroke_rect`] rather than
//! GTK/macOS's rounded-rect pill. Hit-test bounds and click routing are
//! unaffected — [`PipelineViewLayout`] carries only rectangles, and the
//! visual corner radius is not part of its contract.
//!
//! The `▼` focus caret and arrow connector's `▶` head are, similarly,
//! painted as two short strokes forming a chevron via [`draw_line`]
//! rather than a filled triangle path — `win::text` exposes no
//! filled-path primitive (only rectangles, lines, and circles; see that
//! module's doc), and a two-line chevron reads the same as a small
//! filled triangle at this size.
//!
//! Only compiled on `target_os = "windows"` — see `super::mod`'s
//! `#[cfg(target_os = "windows")] mod pipeline_view;` and `backend.rs`'s
//! module docs for why the rest of this repo's `--features win` compile
//! gate stays meaningful without a Windows host.

use windows::Win32::Graphics::Direct2D::ID2D1RenderTarget;

use super::text::{blend, draw_line, fill_rect, stroke_rect, DWrite};
use crate::event::Rect;
use crate::primitives::pipeline_view::{
    status_color, status_glyph, PipelineView, PipelineViewLayout, PipelineViewMeasure,
};
use crate::theme::Theme;

/// Arrow connector width in DIPs. Same value as the GTK/macOS
/// `*_ARROW_WIDTH_PX` constants — geometry constants are shared-*value*
/// (PRIMITIVE_RULES.md's #713 list), not shared-code, since each
/// backend's native unit differs (DIPs here, px there, cells in TUI).
const WIN_ARROW_WIDTH_DIP: f32 = 32.0;
/// Height reserved for the action button in DIPs.
const WIN_ACTION_HEIGHT_DIP: f32 = 22.0;
/// Height reserved above stage boxes for the focus indicator (DIPs).
const WIN_FOCUS_INDICATOR_H: f32 = 8.0;
/// Border stroke width in DIPs.
const BORDER_WIDTH_DIP: f32 = 1.0;
/// Horizontal padding inside each stage box, used to clip an overlong
/// label rather than let it bleed past the box edge.
const H_PAD_DIP: f32 = 8.0;

/// Compute the Win-GUI DIP-unit layout for a [`PipelineView`] without
/// painting — the DirectWrite twin of [`draw_pipeline_view`]'s internal
/// layout call.
///
/// Note: the returned layout (incl. `bounds`) is offset down by
/// `WIN_FOCUS_INDICATOR_H`, so `bounds.y` starts below the reserved caret
/// strip. The focus caret is painted in the gap between `rect.y` and
/// `bounds.y`; a host that clips drawing to `layout.bounds` would clip
/// the caret — clip to the original `rect` instead. Same contract as the
/// GTK/macOS/TUI twins' `*_pipeline_view_layout`.
pub fn win_pipeline_view_layout(view: &PipelineView, rect: Rect) -> PipelineViewLayout {
    let action_h = if view.stages.iter().any(|s| s.action.is_some()) {
        WIN_ACTION_HEIGHT_DIP
    } else {
        0.0
    };
    view.layout(
        rect.x,
        rect.y + WIN_FOCUS_INDICATOR_H,
        PipelineViewMeasure::new(
            rect.width,
            (rect.height - WIN_FOCUS_INDICATOR_H).max(0.0),
            WIN_ARROW_WIDTH_DIP,
            action_h,
        ),
    )
}

/// Draw a [`PipelineView`] into `rect` (DIPs, target-relative) on
/// `target`. Returns the resolved [`PipelineViewLayout`] — same contract
/// as the GTK/macOS/TUI twins' `draw_pipeline_view`: callers (and tests)
/// read the layout back instead of re-deriving it, so paint and hit-test
/// can't drift apart.
pub fn draw_pipeline_view(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    rect: Rect,
    view: &PipelineView,
    theme: &Theme,
) -> PipelineViewLayout {
    let layout = win_pipeline_view_layout(view, rect);

    if rect.width <= 0.0 || rect.height <= 0.0 {
        return layout;
    }

    for sb in &layout.stages {
        let stage = &view.stages[sb.index];
        let is_focused = view.focused_stage == Some(sb.index);
        let bb = sb.box_bounds;

        if bb.width <= 0.0 || bb.height <= 0.0 {
            continue;
        }

        // ── Box fill ─────────────────────────────────────────────────────
        let _ = fill_rect(target, bb, theme.surface_bg);

        // ── Box border (per-status colour; focus uses an above-box
        // indicator, not a border override) ─────────────────────────────
        let border_color = status_color(&stage.status, theme);
        let _ = stroke_rect(target, bb, border_color, BORDER_WIDTH_DIP);

        // ── Focus indicator (▼ chevron above the box) ──────────────────
        if is_focused {
            let ind_x = bb.x + bb.width / 2.0;
            let tri_tip_y = bb.y - 1.0;
            let tri_base_y = bb.y - WIN_FOCUS_INDICATOR_H + 1.0;
            let half_w = 5.0;
            let _ = draw_line(
                target,
                ind_x - half_w,
                tri_base_y,
                ind_x,
                tri_tip_y,
                theme.muted_fg,
                1.5,
            );
            let _ = draw_line(
                target,
                ind_x,
                tri_tip_y,
                ind_x + half_w,
                tri_base_y,
                theme.muted_fg,
                1.5,
            );
        }

        // ── Status icon (top third of box) ───────────────────────────────
        let icon_text = status_glyph(&stage.status);
        let icon_color = status_color(&stage.status, theme);
        if let Ok((iw, ih)) = dwrite.measure_text(icon_text) {
            let icon_h = bb.height / 3.0;
            let icon_cx = bb.x + bb.width / 2.0 - iw / 2.0;
            let icon_cy = bb.y + icon_h / 2.0 - ih / 2.0;
            let icon_rect = Rect::new(icon_cx, icon_cy, iw.max(1.0), ih.max(1.0));
            let _ = dwrite.draw_text(target, icon_text, icon_rect, icon_color);
        }

        // ── Label (middle of box, clipped to the padded box width) ───────
        if !stage.label.is_empty() {
            if let Ok((lw, lh)) = dwrite.measure_text(&stage.label) {
                let avail_w = (bb.width - 2.0 * H_PAD_DIP).max(0.0);
                let draw_w = lw.min(avail_w).max(1.0);
                let label_cx = bb.x + bb.width / 2.0 - draw_w / 2.0;
                let label_cy = bb.y + bb.height / 2.0 - lh / 2.0;
                let label_rect = Rect::new(label_cx, label_cy, draw_w, lh.max(1.0));
                let _ = dwrite.draw_text(target, &stage.label, label_rect, theme.foreground);
            }
        }

        // ── Action button (bottom strip) ─────────────────────────────────
        if let (Some(ab), Some(action_text)) = (sb.action_bounds, &stage.action) {
            let btn_label = format!("[{}]", action_text);

            // Subtle tint background for the button area.
            let tint = blend(theme.surface_bg, theme.accent_bg, 0.15);
            let _ = fill_rect(target, ab, tint);

            if let Ok((bw2, bh2)) = dwrite.measure_text(&btn_label) {
                let btn_cx = ab.x + ab.width / 2.0 - bw2 / 2.0;
                let btn_cy = ab.y + ab.height / 2.0 - bh2 / 2.0;
                let btn_rect = Rect::new(btn_cx, btn_cy, bw2.max(1.0), bh2.max(1.0));
                let _ = dwrite.draw_text(target, &btn_label, btn_rect, theme.accent_bg);
            }
        }

        // ── Arrow connector (─── ▶ chevron head) ──────────────────────────
        if let Some(arrow) = sb.arrow_bounds {
            let ax = arrow.x;
            let mid_y = arrow.y + arrow.height / 2.0;
            let aw = arrow.width;

            let _ = draw_line(target, ax, mid_y, ax + aw - 6.0, mid_y, theme.muted_fg, 1.0);

            let tip_x = ax + aw - 1.0;
            let tail_x = ax + aw - 7.0;
            let half_h = 4.0;
            let _ = draw_line(
                target,
                tail_x,
                mid_y - half_h,
                tip_x,
                mid_y,
                theme.muted_fg,
                1.0,
            );
            let _ = draw_line(
                target,
                tip_x,
                mid_y,
                tail_x,
                mid_y + half_h,
                theme.muted_fg,
                1.0,
            );
        }
    }

    layout
}

#[cfg(test)]
mod tests {
    use super::*;
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

    /// C0 smoke: `draw_pipeline_view` must actually paint text + a
    /// click-routable layout rather than panicking or hitting a
    /// `todo!()` (#735's acceptance bar — "draw_pipeline_view survives C0
    /// with text_ok on win"), and the Tier-1 conformance scenario
    /// `pipeline.click_advances_stage` needs a real box to click.
    #[test]
    fn draw_pipeline_view_paints_text_and_returns_layout() {
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
        let theme = Theme {
            background: Color::rgb(255, 255, 255),
            surface_bg: Color::rgb(255, 255, 255),
            foreground: Color::rgb(0, 0, 0),
            ..Theme::default()
        };
        let view = make_view();
        let rect = Rect::new(0.0, 0.0, W, H);

        let layout = surface
            .paint(|target| {
                draw_pipeline_view(target, &dwrite, rect, &view, &theme);
            })
            .map(|_| win_pipeline_view_layout(&view, rect))
            .expect("paint pipeline view");

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
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
        let theme = Theme::default();
        let view = make_view();
        let rect = Rect::new(origin_x, origin_y, W - origin_x, H - origin_y);

        let layout = surface
            .paint(|target| {
                draw_pipeline_view(target, &dwrite, rect, &view, &theme);
            })
            .map(|_| win_pipeline_view_layout(&view, rect))
            .expect("paint pipeline view");

        // Box top must sit exactly at origin_y + WIN_FOCUS_INDICATOR_H, not
        // a hardcoded absolute value — pins the offset math independently
        // of the hit_test round trip below.
        let bb0 = layout.stages[0].box_bounds;
        assert!(
            (bb0.y - (origin_y + WIN_FOCUS_INDICATOR_H)).abs() < 0.001,
            "stage box top should be origin_y + WIN_FOCUS_INDICATOR_H, got {}",
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
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");

        let painted = surface
            .paint(|target| {
                draw_pipeline_view(target, &dwrite, rect, &view, &Theme::default());
            })
            .map(|_| win_pipeline_view_layout(&view, rect))
            .expect("paint");
        let no_paint = win_pipeline_view_layout(&view, rect);
        assert_eq!(painted, no_paint);
    }

    /// Zero-size rect is a no-op — mirrors every other `win::` rasteriser's
    /// same guard (see `win::text_input::zero_width_rect_is_a_no_op`).
    #[test]
    fn zero_size_rect_is_a_no_op() {
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
        let theme = Theme::default();
        let view = make_view();
        let rect = Rect::new(0.0, 0.0, 0.0, H);

        surface
            .fill_rect(Rect::new(0.0, 0.0, W, H), Color::rgb(255, 255, 255))
            .expect("fill background");

        surface
            .paint(|target| {
                draw_pipeline_view(target, &dwrite, rect, &view, &theme);
            })
            .expect("paint pipeline view");

        let px = surface.pixel_at(1, 1);
        assert_eq!(
            (px.r, px.g, px.b),
            (255, 255, 255),
            "a zero-width pipeline view should paint nothing at all",
        );
    }
}
