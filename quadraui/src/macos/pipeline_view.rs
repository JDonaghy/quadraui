//! macOS rasteriser for [`crate::primitives::pipeline_view::PipelineView`].
//!
//! Paints a horizontal row of bordered stage boxes connected by arrow
//! connectors using Core Graphics. Each box shows a status icon, label,
//! and optional action button.

use core_graphics::geometry::CGRect;
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
/// Border width for stage box outline.
const BORDER_WIDTH: f64 = 1.0;

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
    view.layout(
        x as f32,
        y as f32,
        PipelineViewMeasure::new(w as f32, h as f32, MAC_ARROW_WIDTH_PX, action_h),
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

        // ── Box fill ─────────────────────────────────────────────────────
        fill_rect(ctx, bx, by, bw, bh, theme.surface_bg);

        // ── Box border ───────────────────────────────────────────────────
        let border_color = if is_focused {
            theme.accent_bg
        } else {
            theme.muted_fg
        };
        stroke_rect(ctx, bx, by, bw, bh, border_color, BORDER_WIDTH);

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
    }
}

fn color_to_cg(c: Color) -> (f64, f64, f64, f64) {
    (
        c.r as f64 / 255.0,
        c.g as f64 / 255.0,
        c.b as f64 / 255.0,
        c.a as f64 / 255.0,
    )
}

unsafe fn fill_rect(ctx: CGContextRef, x: f64, y: f64, w: f64, h: f64, c: Color) {
    set_fill_color(ctx, c);
    use core_graphics::geometry::{CGPoint, CGSize};
    CGContextFillRect(ctx, CGRect::new(&CGPoint::new(x, y), &CGSize::new(w, h)));
}

unsafe fn stroke_rect(ctx: CGContextRef, x: f64, y: f64, w: f64, h: f64, c: Color, lw: f64) {
    set_stroke_color(ctx, c);
    CGContextSetLineWidth(ctx, lw);
    use core_graphics::geometry::{CGPoint, CGSize};
    CGContextStrokeRect(ctx, CGRect::new(&CGPoint::new(x, y), &CGSize::new(w, h)));
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
    fn CGContextFillRect(c: CGContextRef, rect: CGRect);
    fn CGContextStrokeRect(c: CGContextRef, rect: CGRect);
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
    fn CGContextStrokePath(c: CGContextRef);
    fn CGContextClosePath(c: CGContextRef);
    fn CGContextFillPath(c: CGContextRef);
}
