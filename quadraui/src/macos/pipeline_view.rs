//! macOS rasteriser for [`crate::primitives::pipeline_view::PipelineView`].
//!
//! Paints a horizontal row of bordered stage boxes connected by arrow
//! connectors using Core Graphics. Each box shows a status icon, label,
//! and optional action button.

use core_graphics::sys::CGContextRef;
use core_text::font::CTFont;

use super::text::{draw_text, measure_text};
use crate::primitives::pipeline_view::{
    PipelineView, PipelineViewLayout, PipelineViewMeasure, StageStatus,
};
use crate::theme::Theme;
use crate::types::Color;

/// Arrow connector width in pixels.
const MAC_ARROW_WIDTH_PX: f32 = 32.0;
/// Height reserved for the action button in pixels.
const MAC_ACTION_HEIGHT_PX: f32 = 22.0;
/// Corner radius for stage boxes (matches the GTK `CORNER_RADIUS`).
const CORNER_RADIUS: f64 = 4.0;
/// Border width for stage box outline.
const BORDER_WIDTH: f64 = 1.0;
/// Height reserved above stage boxes for the focus indicator (pixels).
const MAC_FOCUS_INDICATOR_H: f64 = 8.0;

/// Compute the macOS pixel-unit layout for a [`PipelineView`].
pub fn mac_pipeline_view_layout(
    view: &PipelineView,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
) -> PipelineViewLayout {
    let action_h = if view.stages.iter().any(|s| s.action.is_some()) {
        MAC_ACTION_HEIGHT_PX
    } else {
        0.0
    };
    // Note: the returned layout (incl. `bounds`) is offset down by
    // `MAC_FOCUS_INDICATOR_H`, so `bounds.y` starts below the reserved caret
    // strip. The focus caret is painted in the gap between the passed-in `y`
    // and `bounds.y`; a host that clips drawing to `layout.bounds` would clip
    // the caret — clip to the original `(y, h)` instead.
    view.layout(
        x as f32,
        (y + MAC_FOCUS_INDICATOR_H) as f32,
        PipelineViewMeasure::new(
            w as f32,
            (h - MAC_FOCUS_INDICATOR_H).max(0.0) as f32,
            MAC_ARROW_WIDTH_PX,
            action_h,
        ),
    )
}

/// Draw a [`PipelineView`] onto `ctx`. Returns the layout for host click
/// dispatch.
///
/// # Safety
///
/// `ctx` must be a valid `CGContextRef` borrowed for the duration of the call.
#[allow(clippy::too_many_arguments)]
pub unsafe fn draw_pipeline_view(
    ctx: CGContextRef,
    font: &CTFont,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    view: &PipelineView,
    theme: &Theme,
) -> PipelineViewLayout {
    let layout = mac_pipeline_view_layout(view, x, y, w, h);

    if w <= 0.0 || h <= 0.0 {
        return layout;
    }

    for sb in &layout.stages {
        let stage = &view.stages[sb.index];
        let is_focused = view.focused_stage == Some(sb.index);

        let bx = sb.box_bounds.x as f64;
        let by = sb.box_bounds.y as f64;
        let bw = sb.box_bounds.width as f64;
        let bh = sb.box_bounds.height as f64;

        if bw <= 0.0 || bh <= 0.0 {
            continue;
        }

        // ── Box fill (rounded corners) ────────────────────────────────────
        set_fill_color(ctx, theme.surface_bg);
        add_rounded_rect_path(ctx, bx, by, bw, bh, CORNER_RADIUS);
        CGContextFillPath(ctx);

        // ── Box border (per-status colour; focus uses an above-box indicator) ──
        let border_color = match stage.status {
            StageStatus::Active => theme.accent_bg,
            StageStatus::Done => theme.git_added,
            StageStatus::Failed => theme.error_fg,
            StageStatus::Stale | StageStatus::Pending | StageStatus::Skipped => theme.muted_fg,
        };
        set_stroke_color(ctx, border_color);
        CGContextSetLineWidth(ctx, BORDER_WIDTH);
        add_rounded_rect_path(ctx, bx, by, bw, bh, CORNER_RADIUS);
        CGContextStrokePath(ctx);

        // ── Focus indicator (small ▼ triangle above the box) ─────────────
        if is_focused {
            let ind_x = bx + bw / 2.0;
            let tri_tip_y = by - 1.0;
            let tri_base_y = by - MAC_FOCUS_INDICATOR_H + 1.0;
            let tri_half_w = 5.0_f64;
            set_fill_color(ctx, theme.muted_fg);
            CGContextMoveToPoint(ctx, ind_x, tri_tip_y);
            CGContextAddLineToPoint(ctx, ind_x - tri_half_w, tri_base_y);
            CGContextAddLineToPoint(ctx, ind_x + tri_half_w, tri_base_y);
            CGContextClosePath(ctx);
            CGContextFillPath(ctx);
        }

        // ── Status icon ───────────────────────────────────────────────────
        let icon_text = status_icon_text(stage);
        let icon_color = status_icon_color(stage, theme);
        let (iw, _ih) = measure_text(font, icon_text);
        let icon_cx = bx + bw / 2.0 - iw / 2.0;
        let icon_cy = by + bh / 5.0;
        draw_text(
            ctx,
            font,
            icon_text,
            icon_cx,
            icon_cy,
            color_to_cg(icon_color),
        );

        // ── Label ─────────────────────────────────────────────────────────
        if !stage.label.is_empty() {
            let (lw, _lh) = measure_text(font, &stage.label);
            let label_cx = bx + bw / 2.0 - lw / 2.0;
            let label_cy = by + bh / 2.0 - 8.0;
            draw_text(
                ctx,
                font,
                &stage.label,
                label_cx,
                label_cy,
                color_to_cg(theme.foreground),
            );
        }

        // ── Action button ─────────────────────────────────────────────────
        if let (Some(ab), Some(action_text)) = (sb.action_bounds, &stage.action) {
            let btn_label = format!("[{}]", action_text);
            let (bw2, _) = measure_text(font, &btn_label);
            let btn_cx = bx + bw / 2.0 - bw2 / 2.0;
            let btn_cy = ab.y as f64;
            draw_text(
                ctx,
                font,
                &btn_label,
                btn_cx,
                btn_cy,
                color_to_cg(theme.accent_bg),
            );
        }

        // ── Arrow connector ───────────────────────────────────────────────
        if let Some(arrow) = sb.arrow_bounds {
            let ax = arrow.x as f64;
            let mid_y = arrow.y as f64 + arrow.height as f64 / 2.0;
            let aw = arrow.width as f64;

            // Horizontal line.
            set_stroke_color(ctx, theme.muted_fg);
            CGContextSetLineWidth(ctx, 1.0);
            CGContextMoveToPoint(ctx, ax, mid_y);
            CGContextAddLineToPoint(ctx, ax + aw - 6.0, mid_y);
            CGContextStrokePath(ctx);

            // Simple arrowhead triangle.
            let tip_x = ax + aw - 1.0;
            let tail_x = ax + aw - 7.0;
            let hh = 4.0;
            CGContextMoveToPoint(ctx, tip_x, mid_y);
            CGContextAddLineToPoint(ctx, tail_x, mid_y - hh);
            CGContextAddLineToPoint(ctx, tail_x, mid_y + hh);
            CGContextClosePath(ctx);
            set_fill_color(ctx, theme.muted_fg);
            CGContextFillPath(ctx);
        }
    }

    layout
}

fn status_icon_text(stage: &crate::primitives::pipeline_view::PipelineStage) -> &'static str {
    match stage.status {
        StageStatus::Done => "✓",
        StageStatus::Active => "●",
        StageStatus::Failed => "✗",
        StageStatus::Pending => "·",
        StageStatus::Skipped => "─",
        StageStatus::Stale => "↻",
    }
}

fn status_icon_color(
    stage: &crate::primitives::pipeline_view::PipelineStage,
    theme: &Theme,
) -> Color {
    match stage.status {
        StageStatus::Done => theme.git_added,
        StageStatus::Active => theme.accent_bg,
        StageStatus::Failed => theme.error_fg,
        StageStatus::Pending => theme.muted_fg,
        StageStatus::Skipped => theme.muted_fg,
        StageStatus::Stale => theme.muted_fg,
    }
}

/// Build a rounded-rectangle CG path and make it the current path on `ctx`.
///
/// Uses `CGContextAddArcToPoint` to produce four rounded corners with radius
/// `r`. Equivalent to the GTK `rounded_rect_path` helper but expressed in
/// Core Graphics primitives.
unsafe fn add_rounded_rect_path(ctx: CGContextRef, x: f64, y: f64, w: f64, h: f64, r: f64) {
    // Clamp radius so it never exceeds half the shortest side.
    let r = r.min(w / 2.0).min(h / 2.0);
    CGContextBeginPath(ctx);
    // Start at top-left corner (after the radius offset).
    CGContextMoveToPoint(ctx, x + r, y);
    // Top edge → top-right corner.
    CGContextAddLineToPoint(ctx, x + w - r, y);
    CGContextAddArcToPoint(ctx, x + w, y, x + w, y + r, r);
    // Right edge → bottom-right corner.
    CGContextAddLineToPoint(ctx, x + w, y + h - r);
    CGContextAddArcToPoint(ctx, x + w, y + h, x + w - r, y + h, r);
    // Bottom edge → bottom-left corner.
    CGContextAddLineToPoint(ctx, x + r, y + h);
    CGContextAddArcToPoint(ctx, x, y + h, x, y + h - r, r);
    // Left edge → top-left corner.
    CGContextAddLineToPoint(ctx, x, y + r);
    CGContextAddArcToPoint(ctx, x, y, x + r, y, r);
    CGContextClosePath(ctx);
}

fn color_to_cg(c: Color) -> (f64, f64, f64, f64) {
    (
        c.r as f64 / 255.0,
        c.g as f64 / 255.0,
        c.b as f64 / 255.0,
        c.a as f64 / 255.0,
    )
}

unsafe fn set_fill_color(ctx: CGContextRef, c: Color) {
    CGContextSetRGBFillColor(
        ctx,
        c.r as f64 / 255.0,
        c.g as f64 / 255.0,
        c.b as f64 / 255.0,
        c.a as f64 / 255.0,
    );
}

unsafe fn set_stroke_color(ctx: CGContextRef, c: Color) {
    CGContextSetRGBStrokeColor(
        ctx,
        c.r as f64 / 255.0,
        c.g as f64 / 255.0,
        c.b as f64 / 255.0,
        c.a as f64 / 255.0,
    );
}

extern "C" {
    fn CGContextSetRGBFillColor(
        c: CGContextRef,
        red: core_graphics::base::CGFloat,
        green: core_graphics::base::CGFloat,
        blue: core_graphics::base::CGFloat,
        alpha: core_graphics::base::CGFloat,
    );
    fn CGContextSetRGBStrokeColor(
        c: CGContextRef,
        red: core_graphics::base::CGFloat,
        green: core_graphics::base::CGFloat,
        blue: core_graphics::base::CGFloat,
        alpha: core_graphics::base::CGFloat,
    );
    fn CGContextSetLineWidth(c: CGContextRef, width: core_graphics::base::CGFloat);
    fn CGContextBeginPath(c: CGContextRef);
    fn CGContextMoveToPoint(
        c: CGContextRef,
        x: core_graphics::base::CGFloat,
        y: core_graphics::base::CGFloat,
    );
    fn CGContextAddLineToPoint(
        c: CGContextRef,
        x: core_graphics::base::CGFloat,
        y: core_graphics::base::CGFloat,
    );
    fn CGContextAddArcToPoint(
        c: CGContextRef,
        x1: core_graphics::base::CGFloat,
        y1: core_graphics::base::CGFloat,
        x2: core_graphics::base::CGFloat,
        y2: core_graphics::base::CGFloat,
        radius: core_graphics::base::CGFloat,
    );
    fn CGContextStrokePath(c: CGContextRef);
    fn CGContextClosePath(c: CGContextRef);
    fn CGContextFillPath(c: CGContextRef);
}

#[cfg(test)]
mod tests {
    use super::super::headless::BitmapSurface;
    use super::super::text::make_font;
    use super::super::MacBackend;
    use super::*;
    use crate::event::{Rect as QRect, Viewport};
    use crate::primitives::pipeline_view::{PipelineHit, PipelineStage};
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

    /// Shared body for the action↔click round trip, run at both the
    /// origin and a non-zero origin (quadraui#494 / LESSONS.md "Layout
    /// helpers must return coords in the same frame across backends").
    /// `mac_pipeline_view_layout` bakes `x`/`y` straight into the
    /// returned bounds (absolute frame, matching the GTK/TUI twins) —
    /// and *also* adds `MAC_FOCUS_INDICATOR_H` to `y` itself before
    /// laying out, an extra reason a non-zero-origin regression is
    /// plausible here. Deriving `ab`/`bb` from the layout (not
    /// hardcoding them) means this exercises whatever origin math the
    /// function actually does, at any origin. Calls
    /// `mac_pipeline_view_layout` directly (pure fn, no font/paint
    /// dependency — it forwards to `PipelineView::layout`, which is
    /// plain geometry).
    fn layout_hit_test_action_round_trip_at(origin_x: f64, origin_y: f64) {
        let view = make_view();
        let layout = mac_pipeline_view_layout(&view, origin_x, origin_y, 300.0, 80.0);

        // Box top must sit exactly at origin_y + MAC_FOCUS_INDICATOR_H,
        // not a hardcoded absolute value — pins the offset math
        // independently of the hit_test round trip below.
        let bb0 = layout.stages[0].box_bounds;
        assert!(
            (bb0.y as f64 - (origin_y + MAC_FOCUS_INDICATOR_H)).abs() < 0.001,
            "stage box top should be origin_y + MAC_FOCUS_INDICATOR_H, got {}",
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
            (bb.y as f64 - (origin_y + MAC_FOCUS_INDICATOR_H)).abs() < 0.001,
            "stage box top should be origin_y + MAC_FOCUS_INDICATOR_H, got {}",
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
