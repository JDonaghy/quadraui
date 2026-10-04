//! GTK rasteriser for [`crate::primitives::pipeline_view::PipelineView`].
//!
//! Painting moved to the shared
//! [`crate::primitives::pipeline_view::native_surface_paint::paint`]
//! (#1085, `PaintSurface` Phase 4 slice 8/8) — see that fn's doc for the
//! seven named divergences found while unifying
//! `gtk::pipeline_view::draw_pipeline_view`,
//! `macos::pipeline_view::draw_pipeline_view` and
//! `win::pipeline_view::draw_pipeline_view` into one implementation. This
//! module now only carries [`gtk_pipeline_view_layout`] (still real,
//! backend-specific pure geometry — no painting involved); the
//! deprecated `draw_pipeline_view` compatibility shim over the shared
//! [`super::surface::CairoSurface`] adapter was removed in issue #1109
//! (zero uses in coord-tui's `main` and vimcode's `develop`).

use crate::primitives::layout_metrics::pixel_pipeline_view_layout;
use crate::primitives::pipeline_view::{PipelineView, PipelineViewLayout};
#[cfg(test)]
use crate::theme::Theme;

/// Compute the GTK pixel-unit layout for a [`PipelineView`] without
/// painting. Shares its geometry with `mac_pipeline_view_layout` /
/// `win_pipeline_view_layout` via [`pixel_pipeline_view_layout`] (issue
/// #1079).
///
/// Note: the returned layout (incl. `bounds`) is offset down by
/// [`crate::primitives::layout_metrics::pixel::PIPELINE_FOCUS_INDICATOR_H`],
/// so `bounds.y` starts below the reserved caret strip. The focus caret is
/// painted in the gap between the passed-in `y` and `bounds.y`; a host
/// that clips drawing to `layout.bounds` would clip the caret — clip to
/// the original `(y, h)` instead.
pub fn gtk_pipeline_view_layout(
    view: &PipelineView,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
) -> PipelineViewLayout {
    pixel_pipeline_view_layout(view, x as f32, y as f32, w as f32, h as f32)
}

// ── Tests ──────────────────────────────────────────────────────────────────
//
// Headless painted-indicator tests (mirror the TUI tests in
// `tui/pipeline_view.rs`). Uses a Cairo `ImageSurface` (no display
// required) and reads back pixels directly, following the established
// pattern in `gtk/tab_bar.rs`. Routed through `GtkBackend::draw_pipeline_view`
// (the real `Backend` trait method — which now paints via the shared
// `native_surface_paint::paint`) — the now-removed `draw_pipeline_view`
// free-function shim (issue #1109) is not involved.
//
// Regression for #1085 divergence 6: the focus indicator is now a
// two-line chevron (mirrors Windows's pre-existing shape) rather than a
// filled triangle, so the exact interior centroid a filled triangle
// would have painted is now the chevron's hollow middle — these tests
// probe a point on one of the two strokes instead (see `caret_probe`).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::Rect as QRect;
    use crate::gtk::backend::GtkBackend;
    use crate::primitives::pipeline_view::{PipelineStage, PipelineViewLayout, StageStatus};
    use crate::types::WidgetId;
    use crate::Backend;
    use pangocairo::cairo::{Context, Format, ImageSurface};

    // Surface large enough to contain the box plus the reserved caret strip.
    const W: i32 = 240;
    const H: i32 = 80;
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
    /// focus state, via the real `Backend::draw_pipeline_view` path.
    /// Returns the surface and the resolved layout so tests can derive box
    /// geometry rather than hardcoding it.
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
            let mut backend = GtkBackend::new();
            let painted = std::cell::RefCell::new(None);
            backend.enter_frame_scope(&cr, &pango_layout, |b| {
                *painted.borrow_mut() = Some(
                    b.draw_pipeline_view(QRect::new(20.0, 20.0, BOX_W as f32, BOX_H as f32), &view),
                );
            });
            layout = painted.into_inner().expect("layout captured");
        }
        (surface, layout)
    }

    /// Midpoint of the chevron's left stroke — from `(ind_x - 5, by - 7)`
    /// to `(ind_x, by - 1)` — comfortably clear of the box's top border
    /// (whose 1px stroke straddles `by` itself and antialiases a pixel or
    /// two above it, which would otherwise false-positive this probe).
    fn caret_probe(layout: &PipelineViewLayout) -> (i32, i32) {
        let bb = layout.stages[0].box_bounds;
        let ind_x = bb.x + bb.width / 2.0;
        let cx = (ind_x - 2.5).round() as i32;
        let cy = (bb.y as f64 - 4.0).round() as i32;
        (cx, cy)
    }

    /// Focused Done stage: the chevron tip is painted above the box AND
    /// the box border keeps its per-status (`git_added`) colour rather
    /// than the focus accent. This is the exact issue scenario — focus +
    /// Done together.
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

        // (a) The chevron tip above the box is painted (not background
        //     white) and is the muted indicator colour.
        let (cx, cy) = caret_probe(&layout);
        let caret_px = pixel(&data, stride, cx, cy);
        assert_ne!(
            caret_px,
            (255, 255, 255),
            "focus chevron should paint above the box, got white"
        );
        assert!(
            dist2(caret_px, muted) < 1500,
            "chevron tip pixel {caret_px:?} should be ~muted_fg {muted:?}"
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

        let (cx, cy) = caret_probe(&layout);
        let px = pixel(&data, stride, cx, cy);
        assert_eq!(
            px,
            (255, 255, 255),
            "no focus → reserved strip above the box must stay background white, got {px:?}"
        );
    }

    /// `pipeline_view_layout` is documented **ABSOLUTE** (issue #505):
    /// `box_bounds` / `action_bounds` must be shifted by the widget's
    /// own origin, not left at (0, 0) — the case that hides a
    /// LOCAL/ABSOLUTE mixup.
    fn hit_test_round_trip_at(x: f64, y: f64) {
        use crate::primitives::pipeline_view::PipelineHit;

        let mut view = make_view();
        view.stages[0].action = Some("Retry".into());
        let layout = gtk_pipeline_view_layout(&view, x, y, 200.0, 50.0);

        let stage = &layout.stages[0];
        assert_eq!(stage.box_bounds.x as f64, x);

        let action = stage.action_bounds.expect("action bounds present");
        let acx = action.x + action.width / 2.0;
        let acy = action.y + action.height / 2.0;
        assert_eq!(layout.hit_test(acx, acy), PipelineHit::Action(0));

        let bcx = stage.box_bounds.x + 2.0;
        let bcy = stage.box_bounds.y + 2.0;
        assert_eq!(layout.hit_test(bcx, bcy), PipelineHit::Body(0));
    }

    #[test]
    fn hit_test_round_trip() {
        hit_test_round_trip_at(0.0, 0.0);
    }

    /// Non-zero-origin regression guard (issue #505 / LESSONS.md).
    #[test]
    fn hit_test_round_trip_at_nonzero_origin() {
        hit_test_round_trip_at(7.0, 13.0);
    }
}
