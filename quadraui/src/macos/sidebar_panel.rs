//! macOS rasteriser for [`crate::primitives::sidebar_panel::SidebarPanel`].
//!
//! Painting moved to the shared
//! [`crate::primitives::sidebar_panel::native_surface_paint::paint`]
//! (#862, `NativeSurface` Phase 2d slice 5/9) — see that fn's module doc
//! for the four named divergences found while unifying
//! `gtk::draw_sidebar_panel`, `macos::sidebar_panel::draw_sidebar_panel`
//! and `win::sidebar_panel::draw_sidebar_panel` into one implementation.
//! This module now only carries [`mac_sidebar_panel_layout`] (pure
//! layout, still needed by `MacBackend::sidebar_panel_layout` for
//! no-paint hit-test queries), [`RawSidebarPanelSurface`] and the
//! deprecated [`draw_sidebar_panel`] compatibility shim over it,
//! mirroring `macos::panel`'s identical #859 shape.

use core_graphics::sys::CGContextRef;
use core_text::font::CTFont;

use crate::native_surface::NativeSurface;
use crate::primitives::sidebar_panel::{SidebarPanel, SidebarPanelLayout, SidebarPanelMeasure};
use crate::primitives::toolbar::{measure_button, ToolbarItemMeasure};
use crate::theme::Theme;
use crate::types::WidgetId;

use super::toolbar::CtFontMeasure;

/// Compute the macOS pixel-unit layout for a `SidebarPanel`. `font`
/// is required for accurate text measurement.
pub fn mac_sidebar_panel_layout(
    panel: &SidebarPanel,
    font: &CTFont,
    line_height: f64,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
) -> SidebarPanelLayout {
    let bounds = crate::event::Rect::new(x as f32, y as f32, w as f32, h as f32);
    let measure = CtFontMeasure(font);
    panel.layout(
        bounds,
        SidebarPanelMeasure::new(line_height as f32, 8.0),
        |btn| ToolbarItemMeasure::new(measure_button(&measure, btn)),
    )
}

/// Minimal [`NativeSurface`] adapter over a bare `CGContextRef` + font,
/// used only by the deprecated [`draw_sidebar_panel`] shim below. The
/// shared paint calls `surface_fill_rect`, `surface_stroke_rect`,
/// `surface_measure_text`, `surface_draw_text_run`,
/// `surface_draw_line` and `surface_push_clip`/`surface_pop_clip`;
/// every other method is `unreachable!()`. Mirrors
/// `macos::panel::RawPanelSurface`'s identical #859 pattern.
struct RawSidebarPanelSurface<'a> {
    ctx: CGContextRef,
    font: &'a CTFont,
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
        let (w, h) = super::text::measure_text(self.font, text);
        (w as f32, h as f32)
    }

    fn surface_fill_rect(&mut self, rect: crate::Rect, color: crate::Color) {
        // SAFETY: `ctx` is a valid `CGContextRef` for the caller's paint
        // pass — see this struct's construction site. `ns_fill_rect`
        // already honours `color.a` with a real alpha blend (unlike the
        // GTK `NativeSurface::surface_fill_rect` bug quadraui#811 fixed
        // — see this module's doc, divergence 3).
        unsafe { super::backend::ns_fill_rect(self.ctx, rect, color) };
    }

    fn surface_stroke_rect(&mut self, rect: crate::Rect, color: crate::Color, stroke_width: f32) {
        // SAFETY: same contract as `surface_fill_rect` above.
        unsafe { super::backend::ns_stroke_rect(self.ctx, rect, color, stroke_width as f64) };
    }

    fn surface_draw_text_run(&mut self, rect: crate::Rect, text: &str, color: crate::Color) {
        // SAFETY: `self.ctx` is the caller-supplied context passed to
        // `draw_sidebar_panel`, valid for the duration of the shim call.
        unsafe {
            super::text::draw_text(
                self.ctx,
                self.font,
                text,
                rect.x as f64,
                rect.y as f64,
                super::backend::ns_color_to_cg(color),
            );
        }
    }

    fn surface_draw_line(
        &mut self,
        from: crate::Point,
        to: crate::Point,
        color: crate::Color,
        stroke_width: f32,
    ) {
        // SAFETY: same contract as `surface_fill_rect` above.
        unsafe {
            super::backend::ns_draw_line(
                self.ctx,
                from.x as f64,
                from.y as f64,
                to.x as f64,
                to.y as f64,
                color,
                stroke_width as f64,
            );
        }
    }

    fn surface_push_clip(&mut self, rect: crate::Rect) {
        // SAFETY: same contract as `surface_fill_rect` above.
        unsafe { super::backend::ns_push_clip(self.ctx, rect) };
    }

    fn surface_pop_clip(&mut self) {
        // SAFETY: same contract as `surface_fill_rect` above.
        unsafe { super::backend::ns_pop_clip(self.ctx) };
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
/// direct `quadraui::macos::draw_sidebar_panel` reference rather than
/// going through [`crate::Backend::draw_sidebar_panel`] — the sanctioned
/// entry point, and the one every in-tree call site already uses, which
/// is why this shim has no in-repo caller left to trip the
/// `-D warnings`-denied `deprecated` lint.
///
/// # Safety
///
/// `ctx` must be a valid `CGContextRef` borrowed for the duration of
/// the call (typical: the frame-scope pointer stashed on
/// [`super::MacBackend`]).
#[deprecated(
    since = "0.0.1",
    note = "call `Backend::draw_sidebar_panel` instead — this free function is a compatibility shim over the shared #862 implementation"
)]
#[allow(clippy::too_many_arguments)]
pub unsafe fn draw_sidebar_panel(
    ctx: CGContextRef,
    font: &CTFont,
    line_height: f64,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    panel: &SidebarPanel,
    theme: &Theme,
    hovered_toolbar_id: Option<&WidgetId>,
    pressed_toolbar_id: Option<&WidgetId>,
) -> SidebarPanelLayout {
    let bounds = crate::event::Rect::new(x as f32, y as f32, w as f32, h as f32);
    let mut surface = RawSidebarPanelSurface { ctx, font };
    // `false`: this deprecated shim reproduces the pre-#862 signature
    // exactly (see its doc above), which predates `nerd_fonts_enabled`
    // entirely — there's no flag for a caller of this shim to have
    // passed. `false` matches the fallback-only behaviour every such
    // caller already observed.
    crate::primitives::sidebar_panel::native_surface_paint::paint(
        panel,
        &mut surface,
        theme,
        bounds,
        line_height as f32,
        hovered_toolbar_id,
        pressed_toolbar_id,
        false,
    )
}

#[cfg(test)]
mod tests {
    use super::super::headless::BitmapSurface;
    use super::super::text::make_font;
    use super::super::MacBackend;
    use super::*;
    use crate::event::{Rect as QRect, Viewport};
    use crate::primitives::sidebar_panel::SidebarPanelHit;
    use crate::primitives::toolbar::{Toolbar, ToolbarButton};
    use crate::types::WidgetId;
    use crate::Backend;

    const W: u32 = 240;
    const H: u32 = 200;

    fn font() -> CTFont {
        make_font("Menlo", 14.0).expect("Menlo installed")
    }

    fn panel_with_toolbar() -> SidebarPanel {
        SidebarPanel {
            id: WidgetId::new("sb"),
            toolbar: Some(Toolbar {
                id: WidgetId::new("sb:toolbar"),
                buttons: vec![ToolbarButton::Action {
                    id: WidgetId::new("refine"),
                    label: "Refine".into(),
                    icon: None,
                    key_hint: None,
                    enabled: true,
                    is_active: false,
                    tooltip: String::new(),
                }],
                bg: None,
                focused_index: None,
                icon_overrides: Vec::new(),
            }),
            toolbar_height: None,
        }
    }

    fn paint_via_backend(panel: &SidebarPanel) -> (BitmapSurface, SidebarPanelLayout) {
        let surface = BitmapSurface::new(W, H);
        surface.fill(0.0, 0.0, 0.0, 0.0);
        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        let layout = std::cell::RefCell::new(None);
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            let l = b.draw_sidebar_panel_interactive(
                QRect::new(0.0, 0.0, W as f32, H as f32),
                panel,
                &crate::InteractionState::new(),
            );
            *layout.borrow_mut() = Some(l);
        });
        backend.end_frame();
        (surface, layout.into_inner().unwrap())
    }

    #[test]
    fn toolbar_reserves_header_slot_content_starts_below() {
        let panel = panel_with_toolbar();
        let (_surface, layout) = paint_via_backend(&panel);
        let tb = layout.toolbar_bounds.expect("toolbar bounds present");
        assert_eq!(tb.y, 0.0);
        assert!(
            layout.content_bounds.y >= tb.y + tb.height,
            "content should start below the toolbar slot: tb.bottom={}, content.y={}",
            tb.y + tb.height,
            layout.content_bounds.y,
        );
    }

    #[test]
    fn no_toolbar_gives_full_rect_to_content() {
        let panel = SidebarPanel {
            id: WidgetId::new("sb"),
            toolbar: None,
            toolbar_height: None,
        };
        let (_surface, layout) = paint_via_backend(&panel);
        assert!(layout.toolbar_bounds.is_none());
        assert_eq!(layout.content_bounds.y, 0.0);
        assert_eq!(layout.content_bounds.height, H as f32);
    }

    /// Shared body for the header-click↔toolbar-button round trip, run
    /// at both the origin and a non-zero origin (quadraui#494 /
    /// LESSONS.md "Layout helpers must return coords in the same frame
    /// across backends"). `mac_sidebar_panel_layout` bakes `x`/`y`
    /// straight into `panel.layout`'s returned bounds (absolute frame,
    /// matching the GTK/TUI twins) — call it directly (pure fn, no
    /// paint needed) and prove a click near the header's top-left
    /// still resolves to the toolbar button through `hit_test`.
    fn click_in_header_round_trip_at(origin_x: f64, origin_y: f64) {
        let panel = panel_with_toolbar();
        let f = font();
        let layout =
            mac_sidebar_panel_layout(&panel, &f, 16.0, origin_x, origin_y, W as f64, H as f64);
        match layout.hit_test(origin_x as f32 + 2.0, origin_y as f32) {
            SidebarPanelHit::ToolbarButton(id) => assert_eq!(id.as_str(), "refine"),
            other => panic!("expected ToolbarButton, got {other:?}"),
        }
    }

    #[test]
    fn click_in_header_resolves_to_toolbar_button() {
        click_in_header_round_trip_at(0.0, 0.0);
    }

    /// Non-zero-origin regression guard (quadraui#494).
    #[test]
    fn click_in_header_resolves_to_toolbar_button_at_nonzero_origin() {
        click_in_header_round_trip_at(7.0, 13.0);
    }
}
