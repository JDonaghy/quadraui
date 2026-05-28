//! GTK rasteriser for [`crate::primitives::pipeline_view::PipelineView`].
//!
//! Paints a horizontal row of rounded-rect stage boxes connected by
//! `───▶` arrow connectors using Cairo. Each box shows a status icon,
//! the stage label, and an optional action button in `[text]` style.
//!
//! ## Colour mapping (same as TUI)
//!
//! | Status   | Icon | Fill                |
//! |----------|------|---------------------|
//! | Done     | ✓    | `theme.success`     |
//! | Active   | ●    | `theme.accent_bg`   |
//! | Failed   | ✗    | `theme.error`       |
//! | Pending  | ·    | `theme.muted_fg`    |
//! | Skipped  | ─    | `theme.muted_fg`    |

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
/// Height reserved above stage boxes for the focus indicator (pixels).
const GTK_FOCUS_INDICATOR_H: f64 = 8.0;

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
    // Note: the returned layout (incl. `bounds`) is offset down by
    // `GTK_FOCUS_INDICATOR_H`, so `bounds.y` starts below the reserved caret
    // strip. The focus caret is painted in the gap between the passed-in `y`
    // and `bounds.y`; a host that clips drawing to `layout.bounds` would clip
    // the caret — clip to the original `(y, h)` instead.
    view.layout(
        x as f32,
        (y + GTK_FOCUS_INDICATOR_H) as f32,
        PipelineViewMeasure::new(
            w as f32,
            (h - GTK_FOCUS_INDICATOR_H).max(0.0) as f32,
            GTK_ARROW_WIDTH_PX,
            action_h,
        ),
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
        set_source(cr, theme.surface_bg);
        rounded_rect_path(cr, bx, by, bw, bh, CORNER_RADIUS);
        cr.fill().ok();

        // ── Box border (per-status colour; focus uses an above-box indicator) ──
        let border_color = match stage.status {
            StageStatus::Active => theme.accent_bg,
            StageStatus::Done => theme.git_added,
            StageStatus::Failed => theme.error_fg,
            StageStatus::Stale | StageStatus::Pending | StageStatus::Skipped => theme.muted_fg,
        };
        set_source(cr, border_color);
        cr.set_line_width(BORDER_WIDTH);
        rounded_rect_path(cr, bx, by, bw, bh, CORNER_RADIUS);
        cr.stroke().ok();

        // ── Focus indicator (small ▼ triangle above the box) ─────────────
        if is_focused {
            let ind_x = bx + bw / 2.0;
            let tri_tip_y = by - 1.0;
            let tri_base_y = by - GTK_FOCUS_INDICATOR_H + 1.0;
            let tri_half_w = 5.0;
            set_source(cr, theme.muted_fg);
            cr.move_to(ind_x, tri_tip_y);
            cr.line_to(ind_x - tri_half_w, tri_base_y);
            cr.line_to(ind_x + tri_half_w, tri_base_y);
            cr.close_path();
            cr.fill().ok();
        }

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

// ── Tests ──────────────────────────────────────────────────────────────────
//
// Headless painted-indicator tests (mirror the TUI tests in
// `tui/pipeline_view.rs`). They verify two state-derived geometry facts that
// this primitive's focus decoupling depends on:
//
//   1. A focused stage paints the `▼` caret in the reserved strip above the
//      box (and an unfocused stage leaves that strip blank).
//   2. The box border renders in the per-status colour (`git_added` for Done)
//      rather than the focus accent (`accent_bg`).
//
// Uses a Cairo `ImageSurface` (no display required) and reads back pixels
// directly, following the established pattern in `gtk/tab_bar.rs`. Gated on the
// `gtk` feature so it only runs under `cargo test --features gtk`.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::pipeline_view::{PipelineStage, PipelineViewLayout};
    use crate::types::WidgetId;
    use pangocairo::cairo::{Context, Format, ImageSurface};

    // Surface large enough to contain the box plus the reserved caret strip.
    const W: i32 = 240;
    const H: i32 = 80;
    const X: f64 = 20.0;
    const Y: f64 = 20.0;
    const BOX_W: f64 = 200.0;
    const BOX_H: f64 = 50.0;

    /// Read an RGB triple from an ARgb32 surface at pixel (x, y).
    ///
    /// Cairo's `ARgb32` stores each pixel as four bytes in native
    /// (little-endian) byte order: [B, G, R, A]. `stride` is in bytes and may
    /// include padding. All pixels painted here are opaque, so premultiplied
    /// and straight alpha coincide.
    fn pixel(data: &[u8], stride: usize, x: i32, y: i32) -> (u8, u8, u8) {
        let off = y as usize * stride + x as usize * 4;
        (data[off + 2], data[off + 1], data[off])
    }

    /// Squared Euclidean distance between two RGB triples.
    fn dist2(a: (u8, u8, u8), b: (u8, u8, u8)) -> i64 {
        let dr = a.0 as i64 - b.0 as i64;
        let dg = a.1 as i64 - b.1 as i64;
        let db = a.2 as i64 - b.2 as i64;
        dr * dr + dg * dg + db * db
    }

    /// Find the most-chromatic pixel (max channel spread) in an inclusive
    /// rectangular region. Used to locate the coloured border line, which is a
    /// 1px antialiased stroke blended with its surroundings.
    fn most_chromatic(
        data: &[u8],
        stride: usize,
        x0: i32,
        y0: i32,
        x1: i32,
        y1: i32,
    ) -> (u8, u8, u8) {
        let mut best = (0u8, 0u8, 0u8);
        let mut best_chroma = -1i32;
        for y in y0..=y1 {
            for x in x0..=x1 {
                let (r, g, b) = pixel(data, stride, x, y);
                let chroma = r.max(g).max(b) as i32 - r.min(g).min(b) as i32;
                if chroma > best_chroma {
                    best_chroma = chroma;
                    best = (r, g, b);
                }
            }
        }
        best
    }

    fn make_view() -> PipelineView {
        PipelineView {
            id: WidgetId::new("pipe"),
            stages: vec![PipelineStage {
                label: "Build".into(),
                status: StageStatus::Done,
                action: None,
            }],
            focused_stage: None,
        }
    }

    /// Paint a single Done stage into a fresh white surface with the given
    /// focus state. Returns the surface and the resolved layout so tests can
    /// derive box geometry rather than hardcoding it.
    fn paint(focused: Option<usize>) -> (ImageSurface, PipelineViewLayout) {
        let surface = ImageSurface::create(Format::ARgb32, W, H).expect("create ImageSurface");
        let layout;
        {
            let cr = Context::new(&surface).expect("Context::new");
            // White background so any untouched pixel is clearly white.
            cr.set_source_rgb(1.0, 1.0, 1.0);
            cr.paint().ok();

            let pango_layout = pangocairo::functions::create_layout(&cr);
            let mut view = make_view();
            view.focused_stage = focused;
            layout = draw_pipeline_view(
                &cr,
                &pango_layout,
                X,
                Y,
                BOX_W,
                BOX_H,
                &view,
                &Theme::default(),
            );
        }
        (surface, layout)
    }

    /// Centroid of the `▼` caret painted above the box: tip at `by - 1`, base
    /// at `by - 7`, so the centroid sits at `by - 5` on the box centre column.
    fn caret_centroid(layout: &PipelineViewLayout) -> (i32, i32) {
        let bb = layout.stages[0].box_bounds;
        let cx = (bb.x + bb.width / 2.0).round() as i32;
        let cy = (bb.y as f64 - 5.0).round() as i32;
        (cx, cy)
    }

    /// Focused Done stage: the caret is painted above the box AND the box
    /// border keeps its per-status (`git_added`) colour rather than the focus
    /// accent. This is the exact issue scenario — focus + Done together.
    #[test]
    fn focused_done_stage_shows_indicator_and_retains_border() {
        let (mut surface, layout) = paint(Some(0));
        surface.flush();
        let stride = surface.stride() as usize;
        let data = surface.data().expect("surface data");

        let theme = Theme::default();
        let git_added = (theme.git_added.r, theme.git_added.g, theme.git_added.b);
        let accent = (theme.accent_bg.r, theme.accent_bg.g, theme.accent_bg.b);
        let muted = (theme.muted_fg.r, theme.muted_fg.g, theme.muted_fg.b);

        // (a) The caret region above the box is painted (not background white)
        //     and is the muted indicator colour.
        let (cx, cy) = caret_centroid(&layout);
        let caret_px = pixel(&data, stride, cx, cy);
        assert_ne!(
            caret_px,
            (255, 255, 255),
            "focus caret should paint the reserved strip above the box, got white"
        );
        assert!(
            dist2(caret_px, muted) < 1500,
            "caret pixel {caret_px:?} should be ~muted_fg {muted:?}"
        );

        // (b) The box border renders in the Done colour, not the focus accent.
        //     Scan the straight left-border segment and find the coloured
        //     stroke pixel (a blended 1px line).
        let bb = layout.stages[0].box_bounds;
        let bx = bb.x.round() as i32;
        let by = bb.y.round() as i32;
        let bh = bb.height.round() as i32;
        let border = most_chromatic(&data, stride, bx - 2, by + bh / 4, bx + 2, by + bh * 3 / 4);
        assert!(
            dist2(border, git_added) < dist2(border, accent),
            "border {border:?} should be closer to git_added {git_added:?} \
             than to accent_bg {accent:?}"
        );
    }

    /// When no stage is focused the reserved strip above the box stays blank.
    #[test]
    fn no_indicator_when_not_focused() {
        let (mut surface, layout) = paint(None);
        surface.flush();
        let stride = surface.stride() as usize;
        let data = surface.data().expect("surface data");

        let (cx, cy) = caret_centroid(&layout);
        let px = pixel(&data, stride, cx, cy);
        assert_eq!(
            px,
            (255, 255, 255),
            "no focus → reserved strip above the box must stay background white, got {px:?}"
        );
    }
}
