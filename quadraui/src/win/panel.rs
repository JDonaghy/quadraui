//! Direct2D / DirectWrite rasteriser for [`crate::Panel`] (issue #29).
//!
//! Painting moved to the shared
//! [`crate::primitives::panel::native_surface_paint::paint`] (#859,
//! `NativeSurface` Phase 2d slice 2/9) — see that fn's doc for the named
//! divergences (unclamped glyph centring vs. this backend's old
//! `.max(0.0)` clamp; GTK's `set_source` alpha-drop, fixed at the source
//! by #811) found while unifying `gtk::draw_panel`,
//! `macos::panel::draw_panel` and `win::panel::draw_panel` into one
//! implementation. This module now only carries [`win_panel_layout`]
//! (pure layout, still needed by `WinBackend::panel_layout` for no-paint
//! hit-test queries) and the deprecated [`draw_panel`] compatibility
//! shim over [`RawPanelSurface`], mirroring `win::scrollbar`'s identical
//! #811 shape.
//!
//! Only compiled on `target_os = "windows"` — see `super::mod`'s
//! `#[cfg(target_os = "windows")] mod panel;` and `backend.rs`'s module
//! docs for why the rest of this repo's `--features win` compile gate
//! stays meaningful without a Windows host.
//!
//! # Theme
//!
//! `WinBackend` does not yet carry a live [`Theme`] — see `win::status_bar`'s
//! module doc for the "placeholder until a later issue wires the app's
//! real theme through" posture this module shares.

use windows::Win32::Graphics::Direct2D::ID2D1RenderTarget;

use super::text::DWrite;
use crate::event::Rect;
use crate::native_surface::NativeSurface;
use crate::primitives::panel::{Panel, PanelLayout, PanelMeasure};
use crate::theme::Theme;

/// Width (DIPs) reserved per title-bar action button — the DirectWrite
/// twin of `gtk::panel::GTK_ACTION_BUTTON_PX`.
pub const ACTION_BUTTON_DIP: f32 = 24.0;

/// Compute a [`Panel`]'s layout without painting — the DirectWrite
/// measurer twin of [`draw_panel`]. Both call [`Panel::layout`] with the
/// identical measure, so a no-paint hit-test call always agrees with
/// what the last paint drew.
pub fn win_panel_layout(rect: Rect, panel: &Panel, line_height: f32) -> PanelLayout {
    let measure = PanelMeasure {
        title_bar_height: if panel.title.is_some() {
            line_height
        } else {
            0.0
        },
        action_button_width: ACTION_BUTTON_DIP,
        content_padding: 0.0,
    };
    panel.layout(rect, measure)
}

/// Minimal [`NativeSurface`] adapter over a bare `&ID2D1RenderTarget` +
/// [`DWrite`], used only by the deprecated [`draw_panel`] shim below and
/// by this module's own tests — a panel's paint calls
/// `surface_fill_rect`, `surface_measure_text` and
/// `surface_draw_text_run`; every other method is `unreachable!()`.
/// Mirrors `win::scrollbar::RawScrollbarSurface`'s identical pattern
/// (#811).
pub(crate) struct RawPanelSurface<'a> {
    pub(crate) target: &'a ID2D1RenderTarget,
    pub(crate) dwrite: &'a DWrite,
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
        self.dwrite.measure_text(text).unwrap_or((0.0, 0.0))
    }

    fn surface_fill_rect(&mut self, rect: crate::Rect, color: crate::Color) {
        let _ = super::text::fill_rect(self.target, rect, color);
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
        let _ = self.dwrite.draw_text(self.target, text, rect, color);
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
/// direct `quadraui::win::draw_panel` reference rather than going
/// through [`crate::Backend::draw_panel`] — the sanctioned entry point,
/// and the one every in-tree call site already uses, which is why this
/// shim has no in-repo caller left to trip the `-D warnings`-denied
/// `deprecated` lint.
#[deprecated(
    since = "0.0.1",
    note = "call `Backend::draw_panel` instead — this free function is a compatibility shim over the shared #859 implementation"
)]
pub fn draw_panel(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    rect: Rect,
    panel: &Panel,
    line_height: f32,
) -> PanelLayout {
    let layout = win_panel_layout(rect, panel, line_height);
    let theme = Theme::default();
    let mut surface = RawPanelSurface { target, dwrite };
    crate::primitives::panel::native_surface_paint::paint(panel, &layout, &mut surface, &theme);
    layout
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::panel::{PanelAction, PanelHit};
    use crate::types::{StyledText, WidgetId};
    use crate::win::testing::HeadlessSurface;

    const W: u32 = 200;
    const H: u32 = 100;
    const LINE_HEIGHT: f32 = 20.0;

    fn panel() -> Panel {
        Panel {
            id: WidgetId::new("p"),
            title: Some(StyledText::plain("Terminal")),
            actions: vec![PanelAction {
                id: WidgetId::new("p:close"),
                icon: "\u{d7}".into(),
                tooltip: "Close".into(),
                is_active: false,
            }],
            accent: None,
            collapsed: false,
        }
    }

    /// Paint `panel` via the shared
    /// [`crate::primitives::panel::native_surface_paint::paint`] through
    /// a [`RawPanelSurface`] over `surface`'s headless target — the same
    /// adapter the deprecated [`draw_panel`] shim uses, exercised here
    /// directly so these tests don't trip the `-D warnings`-denied
    /// `deprecated` lint (CLAUDE.md rule 3; mirrors `win::scrollbar`'s
    /// identical test-migration note).
    fn paint(surface: &HeadlessSurface, dwrite: &DWrite, rect: Rect, panel: &Panel) -> PanelLayout {
        let layout = win_panel_layout(rect, panel, LINE_HEIGHT);
        surface
            .paint(|target| {
                let mut raw = RawPanelSurface { target, dwrite };
                crate::primitives::panel::native_surface_paint::paint(
                    panel,
                    &layout,
                    &mut raw,
                    &Theme::default(),
                );
            })
            .expect("paint panel");
        layout
    }

    /// Paint↔click round trip: title bar, action button, and content
    /// area each paint their own bg (or, for content, are simply left
    /// unpainted chrome) at the bounds `hit_test` resolves to the
    /// matching `PanelHit` variant.
    #[test]
    fn paint_and_hit_test_round_trip() {
        let surface = HeadlessSurface::new(W, H).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
        let panel = panel();
        let rect = Rect::new(0.0, 0.0, W as f32, H as f32);

        let layout = paint(&surface, &dwrite, rect, &panel);
        let theme = Theme::default();

        // Title bar: painted bg matches theme.separator (no accent
        // override), and a click on the body (left of the action
        // button) resolves to `TitleBar`.
        let tb = layout.title_bar_bounds.expect("title bar present");
        let tb_px = surface.pixel_at(2, (tb.y + tb.height / 2.0) as u32);
        assert_eq!(
            (tb_px.r, tb_px.g, tb_px.b),
            (theme.separator.r, theme.separator.g, theme.separator.b)
        );
        let title_hit = layout.hit_test(2.0, tb.y + tb.height / 2.0);
        assert_eq!(title_hit, PanelHit::TitleBar(WidgetId::new("p")));

        // Action button: painted at its own bounds, hit-tests to
        // `Action`.
        let va = &layout.visible_actions[0];
        let action_hit = layout.hit_test(
            va.bounds.x + va.bounds.width / 2.0,
            va.bounds.y + va.bounds.height / 2.0,
        );
        assert_eq!(action_hit, PanelHit::Action(WidgetId::new("p:close")));

        // Content area: below the title bar, hit-tests to `Content`.
        let cb = layout.content_bounds;
        assert!(cb.height > 0.0, "content area should be non-empty");
        let content_hit = layout.hit_test(cb.x + 2.0, cb.y + 2.0);
        assert_eq!(content_hit, PanelHit::Content(WidgetId::new("p")));
    }

    /// A no-title panel has no title bar and the content area fills the
    /// whole bounds.
    #[test]
    fn no_title_panel_has_no_title_bar() {
        let mut panel = panel();
        panel.title = None;
        panel.actions = Vec::new();

        let surface = HeadlessSurface::new(W, H).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
        let rect = Rect::new(0.0, 0.0, W as f32, H as f32);

        let layout = paint(&surface, &dwrite, rect, &panel);

        assert!(layout.title_bar_bounds.is_none());
        assert_eq!(layout.content_bounds.height, H as f32);
    }

    /// `win_panel_layout` (no-paint) must produce byte-identical layout
    /// to what `paint` used to paint — same panel, same rect, same line
    /// height.
    #[test]
    fn no_paint_layout_matches_paint_layout() {
        let panel = panel();
        let rect = Rect::new(0.0, 0.0, W as f32, H as f32);

        let surface = HeadlessSurface::new(W, H).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
        let painted = paint(&surface, &dwrite, rect, &panel);
        let no_paint = win_panel_layout(rect, &panel, LINE_HEIGHT);

        assert_eq!(painted, no_paint);
    }
}
