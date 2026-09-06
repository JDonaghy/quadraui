//! macOS rasteriser for [`crate::Panel`].
//!
//! Painting moved to the shared
//! [`crate::primitives::panel::native_surface_paint::paint`] (#859,
//! `NativeSurface` Phase 2d slice 2/9) — see that fn's doc for the named
//! divergences (unclamped glyph centring vs. win's `.max(0.0)`; GTK's
//! `set_source` alpha-drop, fixed at the source by #811) found while
//! unifying `gtk::draw_panel`, `macos::panel::draw_panel` and
//! `win::panel::draw_panel` into one implementation. This module now
//! only carries [`mac_panel_layout`] (pure layout, still needed by
//! `MacBackend::panel_layout` for no-paint hit-test queries) and the
//! deprecated [`draw_panel`] compatibility shim over
//! [`RawPanelSurface`], mirroring `macos::scrollbar`'s identical #811
//! shape.

use core_graphics::sys::CGContextRef;
use core_text::font::CTFont;

use crate::event::Rect as QRect;
use crate::native_surface::NativeSurface;
use crate::primitives::panel::{Panel, PanelLayout, PanelMeasure};
use crate::theme::Theme;

/// 24-pt action-button width, matching GTK.
const ACTION_BUTTON_PX: f32 = 24.0;

/// Compute the macOS pixel-unit layout for a [`Panel`] without painting.
pub fn mac_panel_layout(
    panel: &Panel,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    line_height: f64,
) -> PanelLayout {
    let bounds = QRect::new(x as f32, y as f32, w as f32, h as f32);
    let measure = PanelMeasure {
        title_bar_height: if panel.title.is_some() {
            line_height as f32
        } else {
            0.0
        },
        action_button_width: ACTION_BUTTON_PX,
        content_padding: 0.0,
    };
    panel.layout(bounds, measure)
}

/// Minimal [`NativeSurface`] adapter over a bare `CGContextRef` + font,
/// used only by the deprecated [`draw_panel`] shim below — a panel's
/// paint calls `surface_fill_rect`, `surface_measure_text` and
/// `surface_draw_text_run`; every other method is `unreachable!()`.
/// Mirrors `macos::scrollbar::RawScrollbarSurface`'s identical pattern
/// (#811).
struct RawPanelSurface<'a> {
    ctx: CGContextRef,
    font: &'a CTFont,
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
        let (w, h) = super::text::measure_text(self.font, text);
        (w as f32, h as f32)
    }

    fn surface_fill_rect(&mut self, rect: crate::Rect, color: crate::Color) {
        // SAFETY: `ctx` is a valid `CGContextRef` for the caller's paint
        // pass — see this struct's construction site. `ns_fill_rect`
        // already honours `color.a` with a real alpha blend (unlike the
        // GTK `NativeSurface::surface_fill_rect` bug quadraui#811 fixed
        // — see this module's doc).
        unsafe { super::backend::ns_fill_rect(self.ctx, rect, color) };
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
        // SAFETY: `self.ctx` is the caller-supplied context passed to
        // `draw_panel`, valid for the duration of the shim call.
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
/// direct `quadraui::macos::draw_panel` reference rather than going
/// through [`crate::Backend::draw_panel`] — the sanctioned entry point,
/// and the one every in-tree call site already uses, which is why this
/// shim has no in-repo caller left to trip the `-D warnings`-denied
/// `deprecated` lint.
///
/// # Safety
///
/// `ctx` must be a valid `CGContextRef` borrowed for the duration of
/// the call.
#[deprecated(
    since = "0.0.1",
    note = "call `Backend::draw_panel` instead — this free function is a compatibility shim over the shared #859 implementation"
)]
#[allow(clippy::too_many_arguments)]
pub unsafe fn draw_panel(
    ctx: CGContextRef,
    font: &CTFont,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    panel: &Panel,
    theme: &Theme,
    line_height: f64,
) -> PanelLayout {
    let layout = mac_panel_layout(panel, x, y, w, h, line_height);
    let mut surface = RawPanelSurface { ctx, font };
    crate::primitives::panel::native_surface_paint::paint(panel, &layout, &mut surface, theme);
    layout
}

#[cfg(test)]
mod tests {
    use super::super::headless::BitmapSurface;
    use super::super::text::make_font;
    use super::super::MacBackend;
    use super::*;
    use crate::event::Viewport;
    use crate::primitives::panel::{PanelAction, PanelHit};
    use crate::types::{StyledText, WidgetId};
    use crate::Backend;

    const W: u32 = 240;
    const H: u32 = 160;

    fn font() -> CTFont {
        make_font("Menlo", 14.0).expect("Menlo installed")
    }

    fn sample_panel() -> Panel {
        Panel {
            id: WidgetId::new("panel"),
            title: Some(StyledText::plain("Terminal")),
            actions: vec![
                PanelAction {
                    id: WidgetId::new("panel:close"),
                    icon: "×".into(),
                    tooltip: "Close".into(),
                    is_active: false,
                },
                PanelAction {
                    id: WidgetId::new("panel:max"),
                    icon: "□".into(),
                    tooltip: "Maximize".into(),
                    is_active: false,
                },
            ],
            accent: None,
            collapsed: false,
        }
    }

    fn paint_via_backend(panel: &Panel) -> (BitmapSurface, PanelLayout) {
        let surface = BitmapSurface::new(W, H);
        surface.fill(0.0, 0.0, 0.0, 0.0);
        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        let layout = std::cell::RefCell::new(None);
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            let l = b.draw_panel(QRect::new(0.0, 0.0, W as f32, H as f32), panel);
            *layout.borrow_mut() = Some(l);
        });
        backend.end_frame();
        (surface, layout.into_inner().unwrap())
    }

    #[test]
    fn title_bar_paints_separator_bg_by_default() {
        let panel = sample_panel();
        let (surface, layout) = paint_via_backend(&panel);
        let theme = Theme::default();
        let tb = layout.title_bar_bounds.expect("title bar present");
        // Probe right side of the title bar, left of the action buttons.
        // First action starts at W - 24; sample around W/2.
        let px = (tb.x + tb.width / 2.0) as u32;
        let py = (tb.y + tb.height - 1.0) as u32;
        let (r, g, b, _) = surface.pixel(px, py);
        assert_eq!(
            (r, g, b),
            (theme.separator.r, theme.separator.g, theme.separator.b),
        );
    }

    #[test]
    fn content_bounds_excludes_title_bar() {
        let panel = sample_panel();
        let (_surface, layout) = paint_via_backend(&panel);
        let tb = layout.title_bar_bounds.expect("title bar present");
        let cb = layout.content_bounds;
        assert!(
            cb.y >= tb.y + tb.height,
            "content should start below title bar: tb.bottom={}, content.y={}",
            tb.y + tb.height,
            cb.y,
        );
    }

    #[test]
    fn action_buttons_right_aligned() {
        let panel = sample_panel();
        let (_surface, layout) = paint_via_backend(&panel);
        // Two actions, each 24px wide. Right-most at (W-24..W).
        assert_eq!(layout.visible_actions.len(), 2);
        let first = &layout.visible_actions[0];
        // First action_idx=0 painted right-most.
        assert!((first.bounds.x - (W as f32 - ACTION_BUTTON_PX)).abs() < 0.5);
    }

    #[test]
    fn hit_test_resolves_action_vs_title_vs_content() {
        let panel = sample_panel();
        let (_surface, layout) = paint_via_backend(&panel);
        let first_action = &layout.visible_actions[0];
        let hit = layout.hit_test(
            first_action.bounds.x + first_action.bounds.width * 0.5,
            first_action.bounds.y + first_action.bounds.height * 0.5,
        );
        assert!(matches!(hit, PanelHit::Action(_)));

        // Title body (left of actions).
        let tb = layout.title_bar_bounds.unwrap();
        let hit = layout.hit_test(tb.x + 10.0, tb.y + tb.height * 0.5);
        assert!(matches!(hit, PanelHit::TitleBar(_)), "hit was {:?}", hit);

        // Content area.
        let cb = layout.content_bounds;
        let hit = layout.hit_test(cb.x + cb.width * 0.5, cb.y + cb.height * 0.5);
        assert!(matches!(hit, PanelHit::Content(_)));
    }

    #[test]
    fn hit_test_resolves_action_vs_title_vs_content_at_nonzero_origin() {
        // Regression for quadraui#494 / LESSONS.md "Layout helpers must
        // return coords in the same frame across backends": every other
        // test in this file lays out at origin (0, 0). `mac_panel_
        // layout` bakes `x`/`y` straight into `bounds` (absolute frame,
        // matching the GTK/TUI twins) — call it directly (pure fn, no
        // paint needed) at a non-zero origin and prove action/title/
        // content clicks still resolve through `hit_test`.
        let panel = sample_panel();
        let layout = mac_panel_layout(&panel, 7.0, 13.0, W as f64, H as f64, 16.0);

        let first_action = &layout.visible_actions[0];
        let hit = layout.hit_test(
            first_action.bounds.x + first_action.bounds.width * 0.5,
            first_action.bounds.y + first_action.bounds.height * 0.5,
        );
        assert!(matches!(hit, PanelHit::Action(_)));

        // Title body (left of actions).
        let tb = layout.title_bar_bounds.unwrap();
        let hit = layout.hit_test(tb.x + 10.0, tb.y + tb.height * 0.5);
        assert!(matches!(hit, PanelHit::TitleBar(_)), "hit was {:?}", hit);

        // Content area.
        let cb = layout.content_bounds;
        let hit = layout.hit_test(cb.x + cb.width * 0.5, cb.y + cb.height * 0.5);
        assert!(matches!(hit, PanelHit::Content(_)));
    }

    #[test]
    fn collapsed_panel_zero_content_height() {
        let mut panel = sample_panel();
        panel.collapsed = true;
        let (_surface, layout) = paint_via_backend(&panel);
        assert_eq!(layout.content_bounds.height, 0.0);
    }
}
