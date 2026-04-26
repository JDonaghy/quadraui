//! GTK rasteriser for [`crate::StatusBar`].
//!
//! Paints a status bar onto a [`Context`] using a
//! [`pango::Layout`] for text measurement. Computes the primitive's
//! [`crate::StatusBarLayout`] internally — Pango measurement and
//! rendering both go through the same `pango::Layout` handle, so
//! splitting the work across the call boundary would force callers to
//! plumb the handle through twice. The hit regions the layout would
//! have produced are returned so callers can dispatch clicks.
//!
//! Per D6: layout policy (priority drop, gap rules, …) lives in
//! [`crate::StatusBar::layout`]; this rasteriser just paints what
//! that returns.

use gtk4::cairo::Context;
use gtk4::pango;
use pangocairo::functions as pcfn;

use super::{cairo_rgb, set_source};
use crate::primitives::status_bar::{
    StatusBar, StatusBarHitRegion, StatusBarSegment, StatusSegmentMeasure, StatusSegmentSide,
};
use crate::theme::Theme;

/// 16-pixel minimum gap between left and right segment groups, matching
/// the existing vimcode GTK behaviour. Right segments are dropped from
/// the front (least important first) until they fit while preserving
/// this gap.
const MIN_GAP_PX: f32 = 16.0;

/// Draw a [`StatusBar`] into `(x, y, width, line_height)` on `cr`.
///
/// `layout` is the shared `pango::Layout` the caller uses for text
/// rendering on this surface. The rasteriser temporarily mutates its
/// `text` and `attributes` while measuring + painting and resets
/// `attributes` to `None` before returning — but **does not** restore
/// the previous text. (Caller doesn't typically depend on the layout's
/// text after a draw call returns.)
///
/// Returns hit regions in **bar-local coordinates** (relative to `x`).
/// Caller pushes them into its per-window segment map for click
/// resolution. Widths are clamped to `u16::MAX` to match the existing
/// `StatusBarHitRegion` shape.
///
/// The bar is filled with the first segment's `bg` (or
/// [`Theme::background`] when the bar has no segments), then each
/// resolved visible segment is painted in its own `fg` / `bg` with
/// `bold` honoured via Pango's bold weight attribute.
#[allow(clippy::too_many_arguments)]
pub fn draw_status_bar(
    cr: &Context,
    layout: &pango::Layout,
    x: f64,
    y: f64,
    width: f64,
    line_height: f64,
    bar: &StatusBar,
    theme: &Theme,
) -> Vec<StatusBarHitRegion> {
    // Reset layout state.
    layout.set_attributes(None);
    layout.set_width(-1);
    layout.set_ellipsize(pango::EllipsizeMode::None);

    // Clip to the bar's rect so right-aligned segments that overflow
    // are truncated at the right edge instead of painting past it.
    cr.save().ok();
    cr.rectangle(x, y, width, line_height);
    cr.clip();

    // Background fill: first segment's bg, else theme bg.
    let fill = bar
        .left_segments
        .first()
        .or(bar.right_segments.first())
        .map(|s| cairo_rgb(s.bg))
        .unwrap_or_else(|| cairo_rgb(theme.background));
    cr.set_source_rgb(fill.0, fill.1, fill.2);
    cr.rectangle(x, y, width, line_height);
    cr.fill().ok();

    let apply_bold = |layout: &pango::Layout, bold: bool| {
        if bold {
            let attrs = pango::AttrList::new();
            attrs.insert(pango::AttrInt::new_weight(pango::Weight::Bold));
            layout.set_attributes(Some(&attrs));
        } else {
            layout.set_attributes(None);
        }
    };

    // Pango pixel-width measurer.
    let measure = |seg: &StatusBarSegment| -> StatusSegmentMeasure {
        layout.set_text(&seg.text);
        apply_bold(layout, seg.bold);
        let w_px = layout.pixel_size().0.max(0) as f32;
        StatusSegmentMeasure::new(w_px)
    };
    let bar_layout = bar.layout(width as f32, line_height as f32, MIN_GAP_PX, measure);

    // Paint visible segments + build hit regions in a single pass.
    let mut regions: Vec<StatusBarHitRegion> = Vec::new();
    for vs in &bar_layout.visible_segments {
        let seg = match vs.side {
            StatusSegmentSide::Left => &bar.left_segments[vs.segment_idx],
            StatusSegmentSide::Right => &bar.right_segments[vs.segment_idx],
        };
        layout.set_text(&seg.text);
        apply_bold(layout, seg.bold);

        let seg_x = x + vs.bounds.x as f64;
        let seg_w = vs.bounds.width as f64;

        // Segment background fill.
        set_source(cr, seg.bg);
        cr.rectangle(seg_x, y, seg_w, line_height);
        cr.fill().ok();

        // Segment foreground text.
        set_source(cr, seg.fg);
        cr.move_to(seg_x, y);
        pcfn::show_layout(cr, layout);

        // Hit region for clickable segments.
        if let Some(ref id) = seg.action_id {
            regions.push(StatusBarHitRegion {
                col: (vs.bounds.x.round() as i64).clamp(0, u16::MAX as i64) as u16,
                width: (vs.bounds.width.round() as i64).clamp(0, u16::MAX as i64) as u16,
                id: id.clone(),
            });
        }
    }

    layout.set_attributes(None);
    cr.restore().ok();

    regions
}
