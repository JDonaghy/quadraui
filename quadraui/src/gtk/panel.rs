//! GTK rasteriser for [`crate::Panel`].
//!
//! Painting moved to the shared
//! [`crate::primitives::panel::native_surface_paint::paint`] (#859,
//! `NativeSurface` Phase 2d slice 2/9) — see that fn's doc for the named
//! divergences (unclamped glyph centring vs. win's `.max(0.0)`; GTK's
//! `set_source` alpha-drop, fixed at the source by #811) found while
//! unifying `gtk::draw_panel`, `macos::panel::draw_panel` and
//! `win::panel::draw_panel` into one implementation. This module now
//! only carries [`gtk_panel_layout`] (pure layout, still needed by
//! `GtkBackend::panel_layout` for no-paint hit-test queries) and the
//! deprecated [`draw_panel`] compatibility shim, mirroring
//! `gtk::scrollbar`'s identical #811 shape.

use gtk4::cairo::Context;
use gtk4::pango;

use crate::event::Rect;
use crate::native_surface::NativeSurface;
use crate::primitives::panel::{Panel, PanelLayout, PanelMeasure};
use crate::theme::Theme;

const GTK_ACTION_BUTTON_PX: f32 = 24.0;

/// Compute the GTK pixel-unit layout for a [`Panel`] without painting.
pub fn gtk_panel_layout(
    panel: &Panel,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    line_height: f64,
) -> PanelLayout {
    let bounds = Rect::new(x as f32, y as f32, w as f32, h as f32);
    let measure = PanelMeasure {
        title_bar_height: if panel.title.is_some() {
            line_height as f32
        } else {
            0.0
        },
        action_button_width: GTK_ACTION_BUTTON_PX,
        content_padding: 0.0,
    };
    panel.layout(bounds, measure)
}

/// Minimal [`NativeSurface`] adapter over a bare Cairo context + Pango
/// layout, used only by the deprecated [`draw_panel`] shim below — a
/// panel's paint calls `surface_fill_rect`, `surface_measure_text` and
/// `surface_draw_text_run`; every other method is `unreachable!()`.
/// Mirrors `gtk::scrollbar::RawScrollbarSurface`'s identical pattern
/// (#811).
struct RawPanelSurface<'a> {
    cr: &'a Context,
    pango_layout: &'a pango::Layout,
}

impl NativeSurface for RawPanelSurface<'_> {
    fn surface_begin_frame(&mut self, _viewport: crate::Viewport) {
        unreachable!("RawPanelSurface has no backend frame lifecycle to begin")
    }

    fn surface_end_frame(&mut self) {
        unreachable!("RawPanelSurface has no backend frame lifecycle to end")
    }

    fn surface_viewport(&self) -> crate::Viewport {
        unreachable!("RawPanelSurface has no backend viewport")
    }

    fn surface_line_height(&self) -> f32 {
        unreachable!("RawPanelSurface has no backend line height")
    }

    fn surface_char_width(&self) -> f32 {
        unreachable!("RawPanelSurface has no backend char width")
    }

    fn surface_measure_text(&self, text: &str) -> (f32, f32) {
        self.pango_layout.set_text(text);
        self.pango_layout.set_attributes(None);
        let (w, h) = self.pango_layout.pixel_size();
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
        unreachable!("Panel::paint never strokes a rect")
    }

    fn surface_draw_text_run(&mut self, rect: crate::Rect, text: &str, color: crate::Color) {
        self.pango_layout.set_text(text);
        self.pango_layout.set_attributes(None);
        super::set_source(self.cr, color);
        self.cr.move_to(rect.x as f64, rect.y as f64);
        super::painted_text::show_layout(self.cr, self.pango_layout);
    }

    fn surface_draw_line(
        &mut self,
        _from: crate::Point,
        _to: crate::Point,
        _color: crate::Color,
        _stroke_width: f32,
    ) {
        unreachable!("Panel::paint never strokes a line")
    }

    fn surface_push_clip(&mut self, _rect: crate::Rect) {
        unreachable!("Panel::paint never clips")
    }

    fn surface_pop_clip(&mut self) {
        unreachable!("Panel::paint never clips")
    }

    fn surface_draw_image(
        &mut self,
        _rect: crate::Rect,
        _image: &crate::Image,
    ) -> crate::backend::ImagePaintResult {
        unreachable!("Panel::paint never draws an image")
    }
}

/// Deprecated free-function shim (#859, CLAUDE.md rule 8): reproduces
/// the pre-#859 signature exactly for any external caller that held a
/// direct `quadraui::gtk::draw_panel` reference rather than going
/// through [`crate::Backend::draw_panel`] — the sanctioned entry point,
/// and the one every in-tree call site already uses, which is why this
/// shim has no in-repo caller left to trip the `-D warnings`-denied
/// `deprecated` lint.
#[deprecated(
    since = "0.0.1",
    note = "call `Backend::draw_panel` instead — this free function is a compatibility shim over the shared #859 implementation"
)]
#[allow(clippy::too_many_arguments)]
pub fn draw_panel(
    cr: &Context,
    pango_layout: &pango::Layout,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    panel: &Panel,
    theme: &Theme,
    line_height: f64,
) -> PanelLayout {
    let layout = gtk_panel_layout(panel, x, y, w, h, line_height);
    let mut surface = RawPanelSurface { cr, pango_layout };
    crate::primitives::panel::native_surface_paint::paint(panel, &layout, &mut surface, theme);
    layout
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::panel::{Panel, PanelAction, PanelHit};
    use crate::types::{StyledText, WidgetId};

    /// `panel_layout` is documented **ABSOLUTE** (issue #505):
    /// `title_bar_bounds` / `content_bounds` must start at the panel's
    /// own origin, not at (0, 0) — the case that would hide a
    /// LOCAL/ABSOLUTE mixup.
    fn round_trip_at(x: f64, y: f64) {
        let panel = Panel {
            id: WidgetId::new("p"),
            title: Some(StyledText::plain("Title")),
            actions: vec![PanelAction {
                id: WidgetId::new("close"),
                icon: "×".into(),
                tooltip: String::new(),
                is_active: false,
            }],
            accent: None,
            collapsed: false,
        };
        let layout = gtk_panel_layout(&panel, x, y, 200.0, 100.0, 20.0);

        let tb = layout.title_bar_bounds.expect("title bar present");
        assert_eq!(tb.x as f64, x);
        assert_eq!(tb.y as f64, y);
        assert_eq!(layout.content_bounds.x as f64, x);

        let va = &layout.visible_actions[0];
        let acx = va.bounds.x + va.bounds.width / 2.0;
        let acy = va.bounds.y + va.bounds.height / 2.0;
        assert_eq!(layout.hit_test(acx, acy), PanelHit::Action(va.id.clone()));

        let ccx = layout.content_bounds.x + 1.0;
        let ccy = layout.content_bounds.y + 1.0;
        assert_eq!(layout.hit_test(ccx, ccy), PanelHit::Content(panel.id));
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
