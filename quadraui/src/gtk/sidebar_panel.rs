//! GTK rasteriser for [`crate::primitives::sidebar_panel::SidebarPanel`].
//!
//! Painting moved to the shared
//! [`crate::primitives::sidebar_panel::native_surface_paint::paint`]
//! (#862, `NativeSurface` Phase 2d slice 5/9) — see that fn's module doc
//! for the four named divergences found while unifying
//! `gtk::draw_sidebar_panel`, `macos::sidebar_panel::draw_sidebar_panel`
//! and `win::sidebar_panel::draw_sidebar_panel` into one implementation.
//! This module now only carries [`gtk_sidebar_panel_layout`] (pure
//! layout via the Pango-measuring [`PangoMeasure`] adapter, still needed
//! by `GtkBackend::sidebar_panel_layout` for no-paint hit-test queries —
//! the shared `paint` recomputes its own layout via
//! `NativeSurface::surface_measure_text` instead, so it never calls this
//! fn), [`RawSidebarPanelSurface`] and the deprecated
//! [`draw_sidebar_panel`] compatibility shim over it, mirroring
//! `gtk::panel`'s identical #859 shape.

use gtk4::cairo::Context;
use gtk4::pango;

use crate::native_surface::NativeSurface;
use crate::primitives::sidebar_panel::{SidebarPanel, SidebarPanelLayout, SidebarPanelMeasure};
use crate::primitives::toolbar::{measure_button, ToolbarItemMeasure};
use crate::theme::Theme;
use crate::types::WidgetId;

use super::toolbar::PangoMeasure;

/// Compute the GTK pixel-unit layout for a `SidebarPanel`. Uses Pango
/// for accurate text measurement when `pango_layout` is provided; falls
/// back to a `char_width`-based estimate otherwise (matches the
/// `gtk_toolbar_layout` fallback convention).
#[allow(clippy::too_many_arguments)]
pub fn gtk_sidebar_panel_layout(
    panel: &SidebarPanel,
    pango_layout: Option<&pango::Layout>,
    char_width: f64,
    line_height: f64,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
) -> SidebarPanelLayout {
    let bounds = crate::event::Rect::new(x as f32, y as f32, w as f32, h as f32);
    let measure = PangoMeasure {
        pango_layout,
        char_width,
    };
    panel.layout(
        bounds,
        SidebarPanelMeasure::new(line_height as f32, char_width as f32),
        |btn| ToolbarItemMeasure::new(measure_button(&measure, btn)),
    )
}

/// Minimal [`NativeSurface`] adapter over a bare `(&Context, &pango::Layout)`
/// pair, used only by the deprecated [`draw_sidebar_panel`] shim below.
/// The shared paint calls `surface_fill_rect`, `surface_stroke_rect`,
/// `surface_measure_text`, `surface_draw_text_run`, `surface_draw_line`
/// and `surface_push_clip`/`surface_pop_clip` — every other method is
/// `unreachable!()`. Mirrors `gtk::form::RawFormSurface`'s identical
/// pattern (#808); unlike `gtk::scrollbar::RawScrollbarSurface` (#811)
/// this uses plain `set_source` rather than `set_source_rgba`, since
/// (like `RawFormSurface`) nothing this primitive paints is ever
/// translucent — `Toolbar.bg` / theme colours are always opaque.
pub(crate) struct RawSidebarPanelSurface<'a> {
    pub(crate) cr: &'a Context,
    pub(crate) layout: &'a pango::Layout,
}

impl NativeSurface for RawSidebarPanelSurface<'_> {
    fn surface_begin_frame(&mut self, _viewport: crate::Viewport) {
        unreachable!("RawSidebarPanelSurface has no backend frame lifecycle to begin")
    }

    fn surface_end_frame(&mut self) {
        unreachable!("RawSidebarPanelSurface has no backend frame lifecycle to end")
    }

    fn surface_viewport(&self) -> crate::Viewport {
        unreachable!("RawSidebarPanelSurface has no backend viewport")
    }

    fn surface_line_height(&self) -> f32 {
        unreachable!("RawSidebarPanelSurface has no backend line height")
    }

    fn surface_char_width(&self) -> f32 {
        unreachable!("RawSidebarPanelSurface has no backend char width")
    }

    fn surface_measure_text(&self, text: &str) -> (f32, f32) {
        self.layout.set_text(text);
        self.layout.set_attributes(None);
        let (w, h) = self.layout.pixel_size();
        (w as f32, h as f32)
    }

    fn surface_fill_rect(&mut self, rect: crate::Rect, color: crate::Color) {
        super::set_source(self.cr, color);
        self.cr.rectangle(
            rect.x as f64,
            rect.y as f64,
            rect.width as f64,
            rect.height as f64,
        );
        self.cr.fill().ok();
    }

    fn surface_stroke_rect(&mut self, rect: crate::Rect, color: crate::Color, stroke_width: f32) {
        super::set_source(self.cr, color);
        self.cr.set_line_width(stroke_width as f64);
        self.cr.rectangle(
            rect.x as f64,
            rect.y as f64,
            rect.width as f64,
            rect.height as f64,
        );
        self.cr.stroke().ok();
    }

    fn surface_draw_text_run(&mut self, rect: crate::Rect, text: &str, color: crate::Color) {
        self.layout.set_text(text);
        self.layout.set_attributes(None);
        super::set_source(self.cr, color);
        self.cr.move_to(rect.x as f64, rect.y as f64);
        super::painted_text::show_layout(self.cr, self.layout);
    }

    fn surface_draw_line(
        &mut self,
        from: crate::Point,
        to: crate::Point,
        color: crate::Color,
        stroke_width: f32,
    ) {
        super::set_source(self.cr, color);
        self.cr.set_line_width(stroke_width as f64);
        self.cr.move_to(from.x as f64, from.y as f64);
        self.cr.line_to(to.x as f64, to.y as f64);
        self.cr.stroke().ok();
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
        unreachable!("SidebarPanel::paint never draws an image")
    }
}

/// Deprecated free-function shim (#862, CLAUDE.md rule 8): reproduces
/// the pre-#862 signature exactly for any external caller that held a
/// direct `quadraui::gtk::draw_sidebar_panel` reference rather than going
/// through [`crate::Backend::draw_sidebar_panel`] — the sanctioned entry
/// point, and the one every in-tree call site already uses, which is why
/// this shim has no in-repo caller left to trip the `-D
/// warnings`-denied `deprecated` lint.
#[deprecated(
    since = "0.0.1",
    note = "call `Backend::draw_sidebar_panel` instead — this free function is a compatibility shim over the shared #862 implementation"
)]
#[allow(clippy::too_many_arguments)]
pub fn draw_sidebar_panel(
    cr: &Context,
    pango_layout: &pango::Layout,
    line_height: f64,
    char_width: f64,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    panel: &SidebarPanel,
    theme: &Theme,
    hovered_toolbar_id: Option<&WidgetId>,
    pressed_toolbar_id: Option<&WidgetId>,
) -> SidebarPanelLayout {
    let _ = char_width;
    let bounds = crate::event::Rect::new(x as f32, y as f32, w as f32, h as f32);
    let mut surface = RawSidebarPanelSurface {
        cr,
        layout: pango_layout,
    };
    crate::primitives::sidebar_panel::native_surface_paint::paint(
        panel,
        &mut surface,
        theme,
        bounds,
        line_height as f32,
        hovered_toolbar_id,
        pressed_toolbar_id,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::sidebar_panel::SidebarPanelHit;

    /// `sidebar_panel_layout` is documented **ABSOLUTE** (issue #505):
    /// `content_bounds` must start at the panel's own origin, not
    /// (0, 0) — the case that hides a LOCAL/ABSOLUTE mixup.
    fn round_trip_at(x: f64, y: f64) {
        let panel = SidebarPanel {
            id: WidgetId::new("sp"),
            toolbar: None,
            toolbar_height: None,
        };
        let layout = gtk_sidebar_panel_layout(&panel, None, 6.0, 14.0, x, y, 100.0, 60.0);

        assert_eq!(layout.content_bounds.x as f64, x);
        assert_eq!(layout.content_bounds.y as f64, y);

        let cx = layout.content_bounds.x + 1.0;
        let cy = layout.content_bounds.y + 1.0;
        match layout.hit_test(cx, cy) {
            SidebarPanelHit::Content { x: rel_x, y: rel_y } => {
                assert!((rel_x - 1.0).abs() < 0.01 && (rel_y - 1.0).abs() < 0.01);
            }
            other => panic!("expected Content hit at ({cx}, {cy}), got {other:?}"),
        }
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
