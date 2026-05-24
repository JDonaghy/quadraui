//! GTK rasteriser for [`crate::primitives::pipeline_view::PipelineView`].
//!
//! Paints a horizontal row of rounded-rect stage boxes connected by
//! `───▶` arrow connectors using Cairo. Each box shows a status icon,
//! the stage label, and an optional action button in `[text]` style.
//!
//! ## Colour mapping (same as TUI)
//!
//! | Status   | Icon | Fill tint              | Border                        |
//! |----------|------|------------------------|-------------------------------|
//! | Active   | ●    | accent blue (20% α)    | accent (`theme.accent_bg`)    |
//! | Done     | ✓    | none                   | green (`theme.git_added`)     |
//! | Failed   | ✗    | dim red (12% α)        | red (`theme.error_fg`)        |
//! | Pending  | ·    | none                   | muted (`theme.muted_fg`)      |
//! | Skipped  | ─    | none                   | muted (`theme.muted_fg`)      |
//!
//! `Active`, `Done`, and `Failed` use a 2 px border stroke so the status
//! colour is clearly visible. `Pending`/`Skipped` keep the default 1 px.

use gtk4::cairo::Context;
use gtk4::pango;
use pangocairo::functions as pcfn;

use super::{rounded_rect_path, set_source};
use crate::primitives::pipeline_view::{
    PipelineView, PipelineViewLayout, PipelineViewMeasure, StageStatus,
};
use crate::theme::Theme;
use crate::types::Color;

/// Arrow connector width in pixels.
const GTK_ARROW_WIDTH_PX: f32 = 32.0;
/// Height reserved for the action button in pixels.
const GTK_ACTION_HEIGHT_PX: f32 = 22.0;
/// Corner radius for stage boxes.
const CORNER_RADIUS: f64 = 4.0;
/// Padding inside each stage box (left/right, in px).
const H_PAD: f64 = 8.0;
/// Border width for stage box outline.
const BORDER_WIDTH: f64 = 1.0;

/// Compute the GTK pixel-unit layout for a [`PipelineView`] without painting.
pub fn gtk_pipeline_view_layout(
    view: &PipelineView,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
) -> PipelineViewLayout {
    let action_h = if view.stages.iter().any(|s| s.action.is_some()) {
        GTK_ACTION_HEIGHT_PX
    } else {
        0.0
    };
    view.layout(
        x as f32,
        y as f32,
        PipelineViewMeasure::new(w as f32, h as f32, GTK_ARROW_WIDTH_PX, action_h),
    )
}

/// Draw a [`PipelineView`] onto `cr`. Returns the layout for host click
/// dispatch.
#[allow(clippy::too_many_arguments)]
pub fn draw_pipeline_view(
    cr: &Context,
    pango_layout: &pango::Layout,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    view: &PipelineView,
    theme: &Theme,
) -> PipelineViewLayout {
    let layout = gtk_pipeline_view_layout(view, x, y, w, h);

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
        // Base fill: always surface_bg.
        set_source(cr, theme.surface_bg);
        rounded_rect_path(cr, bx, by, bw, bh, CORNER_RADIUS);
        cr.fill().ok();

        // Status-specific colour overlay on top of the base fill.
        match &stage.status {
            StageStatus::Active => {
                // Filled blue accent tint — makes the running stage
                // immediately obvious even at a glance.
                cr.set_source_rgba(
                    theme.accent_bg.r as f64 / 255.0,
                    theme.accent_bg.g as f64 / 255.0,
                    theme.accent_bg.b as f64 / 255.0,
                    0.20,
                );
                rounded_rect_path(cr, bx, by, bw, bh, CORNER_RADIUS);
                cr.fill().ok();
            }
            StageStatus::Failed => {
                // Dim red tint — signals failure with a subtle background wash.
                cr.set_source_rgba(
                    theme.error_fg.r as f64 / 255.0,
                    theme.error_fg.g as f64 / 255.0,
                    theme.error_fg.b as f64 / 255.0,
                    0.12,
                );
                rounded_rect_path(cr, bx, by, bw, bh, CORNER_RADIUS);
                cr.fill().ok();
            }
            _ => {}
        }

        // ── Box border ───────────────────────────────────────────────────
        let (border_color, border_width) = stage_border_style(&stage.status, is_focused, theme);
        set_source(cr, border_color);
        cr.set_line_width(border_width);
        rounded_rect_path(cr, bx, by, bw, bh, CORNER_RADIUS);
        cr.stroke().ok();

        // ── Status icon (top third of box) ───────────────────────────────
        let icon_text = status_icon_text(stage);
        let icon_color = status_icon_color(stage, theme);
        set_source(cr, icon_color);
        pango_layout.set_text(icon_text);
        pango_layout.set_attributes(None);
        let (iw, ih) = pango_layout.pixel_size();
        let icon_cx = bx + bw / 2.0 - iw as f64 / 2.0;
        let icon_h = bh / 3.0;
        let icon_cy = by + icon_h / 2.0 - ih as f64 / 2.0;
        cr.move_to(icon_cx, icon_cy);
        pcfn::show_layout(cr, pango_layout);

        // ── Label (middle third) ─────────────────────────────────────────
        if !stage.label.is_empty() {
            set_source(cr, theme.foreground);
            pango_layout.set_text(&stage.label);
            pango_layout.set_width((bw - 2.0 * H_PAD) as i32 * pango::SCALE);
            pango_layout.set_ellipsize(pango::EllipsizeMode::End);
            let (lw, lh) = pango_layout.pixel_size();
            let label_cx = bx + bw / 2.0 - lw as f64 / 2.0;
            let label_cy = by + bh / 2.0 - lh as f64 / 2.0;
            cr.move_to(label_cx, label_cy);
            pcfn::show_layout(cr, pango_layout);
            pango_layout.set_width(-1); // reset
        }

        // ── Action button (bottom strip) ─────────────────────────────────
        if let (Some(ab), Some(action_text)) = (sb.action_bounds, &stage.action) {
            let btn_label = format!("[{}]", action_text);
            let aby = ab.y as f64;
            let abh = ab.height as f64;

            // Subtle tint background for the button area.
            set_source(cr, theme.accent_bg);
            cr.set_source_rgba(
                theme.accent_bg.r as f64 / 255.0,
                theme.accent_bg.g as f64 / 255.0,
                theme.accent_bg.b as f64 / 255.0,
                0.15,
            );
            cr.rectangle(bx + 1.0, aby, bw - 2.0, abh - 1.0);
            cr.fill().ok();

            set_source(cr, theme.accent_bg);
            pango_layout.set_text(&btn_label);
            pango_layout.set_width(-1);
            let (bw2, bh2) = pango_layout.pixel_size();
            let btn_cx = bx + bw / 2.0 - bw2 as f64 / 2.0;
            let btn_cy = aby + abh / 2.0 - bh2 as f64 / 2.0;
            cr.move_to(btn_cx, btn_cy);
            pcfn::show_layout(cr, pango_layout);
        }

        // ── Arrow connector ───────────────────────────────────────────────
        if let Some(arrow) = sb.arrow_bounds {
            let ax = arrow.x as f64;
            let ay = (arrow.y + arrow.height / 2.0) as f64;
            let aw = arrow.width as f64;

            set_source(cr, theme.muted_fg);
            cr.set_line_width(1.0);
            // Dashed line up to the arrowhead.
            cr.move_to(ax, ay);
            cr.line_to(ax + aw - 6.0, ay);
            cr.stroke().ok();

            // Simple filled triangle arrowhead.
            let tip_x = ax + aw - 1.0;
            let tail_x = ax + aw - 7.0;
            let half_h = 4.0;
            cr.move_to(tip_x, ay);
            cr.line_to(tail_x, ay - half_h);
            cr.line_to(tail_x, ay + half_h);
            cr.close_path();
            cr.fill().ok();
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

/// Border colour and stroke width for a stage based on its status.
///
/// `Active`, `Done`, and `Failed` use a 2 px stroke so the status signal
/// is clearly visible against the stage background. `Pending`/`Skipped`
/// keep the quieter 1 px stroke.
///
/// `is_focused` overrides the border colour to `theme.accent_bg` for
/// `Done` and `Pending`/`Skipped` to show keyboard focus.
fn stage_border_style(status: &StageStatus, is_focused: bool, theme: &Theme) -> (Color, f64) {
    match status {
        StageStatus::Active => (theme.accent_bg, 2.0),
        StageStatus::Done => {
            let color = if is_focused { theme.accent_bg } else { theme.git_added };
            (color, 2.0)
        }
        StageStatus::Failed => (theme.error_fg, 2.0),
        StageStatus::Pending | StageStatus::Skipped => {
            let color = if is_focused { theme.accent_bg } else { theme.muted_fg };
            (color, BORDER_WIDTH)
        }
    }
}
