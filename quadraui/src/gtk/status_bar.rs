//! GTK rasteriser for [`crate::StatusBar`].
//!
//! Painting moved to the shared
//! [`crate::primitives::status_bar::native_surface_paint::paint`] (#860,
//! `NativeSurface` Phase 2d slice 3/9) — see that fn's doc for the named
//! divergences (bold-aware measurement: GTK/Win measured a segment's own
//! `bold` weight, macOS ignored it; GTK's missing zero-size guard, now
//! applying the already-fixed quadraui#791 shape uniformly) found while
//! unifying `gtk::draw_status_bar`, `macos::status_bar::draw_status_bar`
//! and `win::status_bar::draw_status_bar` into one implementation. This
//! module now only carries the deprecated [`draw_status_bar`]
//! compatibility shim over [`RawGtkStatusBarSurface`], mirroring
//! `gtk::panel`'s identical #859 shape. `MIN_GAP_PX` stays put — it's
//! still `GtkBackend::status_bar_layout`'s own no-paint measurer
//! constant, untouched by this migration.

use gtk4::cairo::Context;
use gtk4::pango;

use crate::native_surface::NativeSurface;
use crate::primitives::status_bar::{StatusBar, StatusBarLayout};
use crate::theme::Theme;
use crate::types::WidgetId;

/// 16-pixel minimum gap between left and right segment groups, matching
/// the existing vimcode GTK behaviour. Still used by
/// `GtkBackend::status_bar_layout`'s own no-paint measurer — the shared
/// [`crate::primitives::status_bar::native_surface_paint::paint`] carries
/// its own independent copy of the same value (see that module's doc).
pub const MIN_GAP_PX: f32 = 16.0;

/// Minimal [`NativeSurface`] adapter over a bare Cairo context + Pango
/// layout, used only by the deprecated [`draw_status_bar`] shim below —
/// mirrors `gtk::panel::RawPanelSurface`'s identical pattern (#859).
struct RawGtkStatusBarSurface<'a> {
    cr: &'a Context,
    pango_layout: &'a pango::Layout,
}

impl NativeSurface for RawGtkStatusBarSurface<'_> {
    fn surface_begin_frame(&mut self, _viewport: crate::Viewport) {
        unreachable!("RawGtkStatusBarSurface has no backend frame lifecycle to begin")
    }

    fn surface_end_frame(&mut self) {
        unreachable!("RawGtkStatusBarSurface has no backend frame lifecycle to end")
    }

    fn surface_viewport(&self) -> crate::Viewport {
        unreachable!("RawGtkStatusBarSurface has no backend viewport")
    }

    fn surface_line_height(&self) -> f32 {
        unreachable!("RawGtkStatusBarSurface has no backend line height")
    }

    fn surface_char_width(&self) -> f32 {
        unreachable!("RawGtkStatusBarSurface has no backend char width")
    }

    fn surface_measure_text(&self, text: &str) -> (f32, f32) {
        self.pango_layout.set_text(text);
        self.pango_layout.set_attributes(None);
        let (w, h) = self.pango_layout.pixel_size();
        (w as f32, h as f32)
    }

    fn surface_measure_text_styled(&self, text: &str, bold: bool) -> (f32, f32) {
        self.pango_layout.set_text(text);
        if bold {
            let attrs = pango::AttrList::new();
            attrs.insert(pango::AttrInt::new_weight(pango::Weight::Bold));
            self.pango_layout.set_attributes(Some(&attrs));
        } else {
            self.pango_layout.set_attributes(None);
        }
        let (w, h) = self.pango_layout.pixel_size();
        self.pango_layout.set_attributes(None);
        (w as f32, h as f32)
    }

    fn surface_fill_rect(&mut self, rect: crate::Rect, color: crate::Color) {
        super::set_source_rgba(self.cr, color);
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
        unreachable!("StatusBar::paint never strokes a rect")
    }

    fn surface_draw_text_run(&mut self, rect: crate::Rect, text: &str, color: crate::Color) {
        self.pango_layout.set_text(text);
        self.pango_layout.set_attributes(None);
        super::set_source(self.cr, color);
        self.cr.move_to(rect.x as f64, rect.y as f64);
        super::painted_text::show_layout(self.cr, self.pango_layout);
    }

    #[allow(clippy::too_many_arguments)]
    fn surface_draw_text_run_styled(
        &mut self,
        rect: crate::Rect,
        text: &str,
        color: crate::Color,
        bold: bool,
        italic: bool,
        underline: bool,
        scale_x: f32,
    ) {
        self.pango_layout.set_text(text);
        let attrs = pango::AttrList::new();
        if bold {
            attrs.insert(pango::AttrInt::new_weight(pango::Weight::Bold));
        }
        if italic {
            attrs.insert(pango::AttrInt::new_style(pango::Style::Italic));
        }
        if underline {
            attrs.insert(pango::AttrInt::new_underline(pango::Underline::Single));
        }
        self.pango_layout.set_attributes(Some(&attrs));
        super::set_source(self.cr, color);
        if (scale_x - 1.0).abs() > f32::EPSILON {
            self.cr.save().ok();
            self.cr.translate(rect.x as f64, rect.y as f64);
            self.cr.scale(scale_x as f64, 1.0);
            self.cr.move_to(0.0, 0.0);
            super::painted_text::show_layout(self.cr, self.pango_layout);
            self.cr.restore().ok();
        } else {
            self.cr.move_to(rect.x as f64, rect.y as f64);
            super::painted_text::show_layout(self.cr, self.pango_layout);
        }
        self.pango_layout.set_attributes(None);
    }

    fn surface_draw_line(
        &mut self,
        _from: crate::Point,
        _to: crate::Point,
        _color: crate::Color,
        _stroke_width: f32,
    ) {
        unreachable!("StatusBar::paint never strokes a line")
    }

    fn surface_push_clip(&mut self, rect: crate::Rect) {
        self.cr.save().ok();
        self.cr.rectangle(
            rect.x as f64,
            rect.y as f64,
            rect.width as f64,
            rect.height as f64,
        );
        self.cr.clip();
    }

    fn surface_pop_clip(&mut self) {
        self.cr.restore().ok();
    }

    fn surface_draw_image(
        &mut self,
        _rect: crate::Rect,
        _image: &crate::Image,
    ) -> crate::backend::ImagePaintResult {
        unreachable!("StatusBar::paint never draws an image")
    }
}

/// Deprecated free-function shim (#860, CLAUDE.md rule 8): reproduces
/// the pre-#860 signature exactly for any external caller that held a
/// direct `quadraui::gtk::draw_status_bar` reference rather than going
/// through [`crate::Backend::draw_status_bar`] — the sanctioned entry
/// point, and the one every in-tree call site already uses, which is why
/// this shim has no in-repo caller left to trip the `-D warnings`-denied
/// `deprecated` lint.
#[deprecated(
    since = "0.0.1",
    note = "call `Backend::draw_status_bar` instead — this free function is a compatibility shim over the shared #860 implementation"
)]
#[allow(clippy::too_many_arguments)]
pub fn draw_status_bar(
    cr: &Context,
    pango_layout: &pango::Layout,
    x: f64,
    y: f64,
    width: f64,
    line_height: f64,
    bar: &StatusBar,
    theme: &Theme,
    hovered_id: Option<&WidgetId>,
    pressed_id: Option<&WidgetId>,
) -> StatusBarLayout {
    let mut surface = RawGtkStatusBarSurface { cr, pango_layout };
    crate::primitives::status_bar::native_surface_paint::paint(
        bar,
        &mut surface,
        theme,
        x as f32,
        y as f32,
        width as f32,
        line_height as f32,
        hovered_id,
        pressed_id,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::status_bar::{StatusBarHit, StatusBarSegment};
    use crate::types::Color;
    use pangocairo::cairo::{Context as CairoContext, Format, ImageSurface};

    fn headless_cairo_and_pango() -> (ImageSurface, pango::Layout) {
        let surface = ImageSurface::create(Format::ARgb32, 200, 200).expect("create ImageSurface");
        let cr = CairoContext::new(&surface).expect("Context::new");
        let layout = pangocairo::functions::create_layout(&cr);
        (surface, layout)
    }

    fn test_bar() -> StatusBar {
        StatusBar {
            id: WidgetId::new("sb"),
            left_segments: vec![StatusBarSegment {
                text: "Ready".into(),
                fg: Color::rgb(255, 255, 255),
                bg: Color::rgb(0, 0, 0),
                bold: false,
                action_id: Some(WidgetId::new("sb:action")),
            }],
            right_segments: vec![],
        }
    }

    /// `status_bar_layout` is documented **LOCAL** (issue #505):
    /// `StatusBar::layout` (called by `paint` internally) only ever
    /// receives `width`/`line_height` — the `x`/`y` this function paints
    /// at are used purely to offset the drawing calls, never folded into
    /// the returned `StatusBarLayout`. Painting at a non-zero origin must
    /// therefore produce byte-identical segment bounds / hit regions to
    /// painting at the origin — the same "ignores origin" shape
    /// `data_table_layout`/`form_layout` already guard on GTK
    /// (`gtk::backend` tests), which `status_bar_layout` had no
    /// equivalent for.
    ///
    /// Exercises the shared paint through [`RawGtkStatusBarSurface`]
    /// directly rather than the deprecated [`draw_status_bar`] shim, so
    /// this test doesn't trip the `-D warnings`-denied `deprecated` lint
    /// (CLAUDE.md rule 3; mirrors `gtk::panel`'s identical test-migration
    /// note).
    fn round_trip_at(x: f64, y: f64) {
        let (surface, pango_layout) = headless_cairo_and_pango();
        let cr = CairoContext::new(&surface).expect("Context::new");
        let theme = Theme::default();
        let bar = test_bar();

        let mut raw = RawGtkStatusBarSurface {
            cr: &cr,
            pango_layout: &pango_layout,
        };
        let at_origin = crate::primitives::status_bar::native_surface_paint::paint(
            &bar, &mut raw, &theme, 0.0, 0.0, 100.0, 20.0, None, None,
        );
        let mut raw = RawGtkStatusBarSurface {
            cr: &cr,
            pango_layout: &pango_layout,
        };
        let shifted = crate::primitives::status_bar::native_surface_paint::paint(
            &bar, &mut raw, &theme, x as f32, y as f32, 100.0, 20.0, None, None,
        );

        assert_eq!(
            at_origin, shifted,
            "LOCAL status_bar_layout must not shift segment bounds by x/y"
        );

        let seg = shifted.visible_segments[0];
        let cx = seg.bounds.x + 1.0;
        let cy = seg.bounds.y + 1.0;
        assert_eq!(
            shifted.hit_test(cx, cy),
            StatusBarHit::Segment(bar.left_segments[0].action_id.clone().unwrap())
        );
    }

    #[test]
    fn paint_and_click_round_trip() {
        round_trip_at(0.0, 0.0);
    }

    /// Non-zero-origin regression guard (issue #505 / LESSONS.md).
    #[test]
    fn paint_and_click_round_trip_at_nonzero_origin() {
        round_trip_at(7.0, 13.0);
    }
}
