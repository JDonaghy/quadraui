//! GTK rasteriser for [`crate::Split`].
//!
//! Painting moved to the shared
//! [`crate::primitives::split::native_surface_paint::paint`] (#864,
//! `NativeSurface` Phase 2d slice 7/9, child of #811) — see that fn's
//! module doc for a **reported divergence**: the deleted `draw_split`
//! below painted the divider opaque-only (`set_source`), while the live
//! `Backend::draw_split` path now honours `theme.separator`'s alpha via
//! `GtkBackend::surface_fill_rect` (`gtk::set_source_rgba`, inherited
//! from the #811 slice 1 scrollbar fix). This module now carries
//! [`gtk_split_layout`], [`RawSplitSurface`], and the deprecated
//! [`draw_split`] compatibility shim over the shared paint, mirroring
//! `gtk::split_tree::RawSplitTreeSurface` (#863, slice 6/9).

use gtk4::cairo::Context;

use crate::event::Rect;
use crate::primitives::split::{Split, SplitLayout, SplitMeasure};
use crate::theme::Theme;

const GTK_DIVIDER_PX: f32 = 4.0;

/// Compute the GTK pixel-unit layout for a [`Split`] without painting.
pub fn gtk_split_layout(split: &Split, x: f64, y: f64, w: f64, h: f64) -> SplitLayout {
    let bounds = Rect::new(x as f32, y as f32, w as f32, h as f32);
    split.layout(bounds, SplitMeasure::new(GTK_DIVIDER_PX))
}

/// Minimal [`crate::native_surface::NativeSurface`] adapter over a bare
/// Cairo context, used by the deprecated [`draw_split`] shim below — a
/// split's paint calls exactly one verb (`surface_fill_rect`, once for
/// the divider), so every other method is `unreachable!()`. Mirrors
/// `gtk::split_tree::RawSplitTreeSurface`'s identical pattern (#863).
pub(crate) struct RawSplitSurface<'a> {
    pub(crate) cr: &'a Context,
}

impl crate::native_surface::NativeSurface for RawSplitSurface<'_> {
    fn surface_begin_frame(&mut self, _viewport: crate::Viewport) {
        unreachable!("RawSplitSurface has no backend frame lifecycle to begin")
    }

    fn surface_end_frame(&mut self) {
        unreachable!("RawSplitSurface has no backend frame lifecycle to end")
    }

    fn surface_viewport(&self) -> crate::Viewport {
        unreachable!("RawSplitSurface has no backend viewport")
    }

    fn surface_line_height(&self) -> f32 {
        unreachable!("RawSplitSurface has no backend line height")
    }

    fn surface_char_width(&self) -> f32 {
        unreachable!("RawSplitSurface has no backend char width")
    }

    fn surface_measure_text(&self, _text: &str) -> (f32, f32) {
        unreachable!("RawSplitSurface has no text measurement")
    }

    fn surface_fill_rect(&mut self, rect: crate::Rect, color: crate::Color) {
        // Deliberately opaque-only `set_source` (not `_rgba`): this
        // reproduces the pre-#864 free function's behaviour exactly,
        // byte for byte, for any external caller still holding a direct
        // reference to it. `theme.separator` is NOT guaranteed opaque
        // (see `primitives::split::native_surface_paint`'s module doc
        // for why) — the sanctioned `Backend::draw_split` entry point
        // now honours its alpha via `GtkBackend::surface_fill_rect`
        // (`set_source_rgba`); this deprecated shim intentionally does
        // not, to keep this preservation guarantee.
        super::set_source(self.cr, color);
        self.cr.rectangle(
            rect.x as f64,
            rect.y as f64,
            rect.width as f64,
            rect.height as f64,
        );
        self.cr.fill().ok();
    }

    fn surface_stroke_rect(
        &mut self,
        _rect: crate::Rect,
        _color: crate::Color,
        _stroke_width: f32,
    ) {
        unreachable!("Split::paint never strokes a rect")
    }

    fn surface_draw_text_run(&mut self, _rect: crate::Rect, _text: &str, _color: crate::Color) {
        unreachable!("Split::paint never draws text")
    }

    fn surface_draw_line(
        &mut self,
        _from: crate::Point,
        _to: crate::Point,
        _color: crate::Color,
        _stroke_width: f32,
    ) {
        unreachable!("Split::paint never strokes a line")
    }

    fn surface_push_clip(&mut self, _rect: crate::Rect) {
        unreachable!("Split::paint never clips")
    }

    fn surface_pop_clip(&mut self) {
        unreachable!("Split::paint never clips")
    }

    fn surface_draw_image(
        &mut self,
        _rect: crate::Rect,
        _image: &crate::Image,
    ) -> crate::backend::ImagePaintResult {
        unreachable!("Split::paint never draws an image")
    }
}

/// Deprecated free-function shim (#864, CLAUDE.md rule 8): reproduces
/// the pre-#864 signature exactly for any external caller that held a
/// direct `quadraui::gtk::draw_split` reference rather than going
/// through [`crate::Backend::draw_split`] — the sanctioned entry point,
/// and the one every in-tree call site already uses, which is why this
/// shim has no in-repo caller left to trip the `-D warnings`-denied
/// `deprecated` lint.
#[allow(clippy::too_many_arguments)]
#[deprecated(
    since = "0.0.1",
    note = "call `Backend::draw_split` instead — this free function is a compatibility shim over the shared #864 implementation"
)]
pub fn draw_split(
    cr: &Context,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    split: &Split,
    theme: &Theme,
) -> SplitLayout {
    let layout = gtk_split_layout(split, x, y, w, h);
    let mut surface = RawSplitSurface { cr };
    crate::primitives::split::native_surface_paint::paint(&layout, &mut surface, theme);
    layout
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::split::{Split, SplitDirection, SplitHit};
    use crate::types::WidgetId;

    fn round_trip_at(x: f64, y: f64) {
        let split = Split {
            id: WidgetId::new("s"),
            direction: SplitDirection::Horizontal,
            ratio: 0.5,
            first_min: 0.0,
            second_min: 0.0,
        };
        let layout = gtk_split_layout(&split, x, y, 100.0, 40.0);

        // `split_layout` is documented **ABSOLUTE** (issue #505):
        // `first_bounds` must start exactly at the origin the split was
        // laid out at, not at (0, 0).
        assert_eq!(layout.first_bounds.x as f64, x);
        assert_eq!(layout.first_bounds.y as f64, y);

        // A click inside the first pane's own (absolute) bounds must
        // resolve back to it without any further coordinate shift.
        let cx = layout.first_bounds.x + layout.first_bounds.width / 2.0;
        let cy = layout.first_bounds.y + layout.first_bounds.height / 2.0;
        assert_eq!(
            layout.hit_test(cx, cy),
            SplitHit::FirstPane(split.id.clone())
        );

        let dx = layout.divider_bounds.x + layout.divider_bounds.width / 2.0;
        let dy = layout.divider_bounds.y + layout.divider_bounds.height / 2.0;
        assert_eq!(layout.hit_test(dx, dy), SplitHit::Divider(split.id));
    }

    #[test]
    fn paint_and_click_round_trip() {
        round_trip_at(0.0, 0.0);
    }

    /// Non-zero-origin regression guard (issue #505 / LESSONS.md
    /// "Layout helpers must return coords in the same frame across
    /// backends"): `(x, y) = (0, 0)` is exactly the case where a
    /// LOCAL/ABSOLUTE mixup in `gtk_split_layout` would be invisible.
    #[test]
    fn paint_and_click_round_trip_at_nonzero_origin() {
        round_trip_at(7.0, 13.0);
    }
}
