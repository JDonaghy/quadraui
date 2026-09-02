//! Direct2D / DirectWrite rasteriser for [`crate::Panel`] (issue #29).
//!
//! Mirrors `gtk::panel`'s structure: [`Panel::layout`] (the D6 layout
//! API — see that primitive's module doc) computes title-bar/action-
//! button/content geometry; this module only measures (title-bar height
//! from `line_height`) and paints (title-bar fill, title text,
//! action-button glyphs) via Direct2D / DirectWrite. Paint and hit-test
//! both derive from one `Panel::layout` call (through
//! [`win_panel_layout`]), so they can't drift apart. Content is NOT
//! painted — apps draw into `layout.content_bounds` themselves, same
//! contract as every other backend.
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

use super::text::{fill_rect, DWrite};
use crate::event::Rect;
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

/// Draw a [`Panel`]'s chrome onto `target`. Returns the layout for host
/// click dispatch. Content is NOT painted.
pub fn draw_panel(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    rect: Rect,
    panel: &Panel,
    line_height: f32,
) -> PanelLayout {
    let layout = win_panel_layout(rect, panel, line_height);
    let theme = Theme::default();

    if let Some(tb) = layout.title_bar_bounds {
        let title_bg = panel.accent.unwrap_or(theme.separator);
        let _ = fill_rect(target, tb, title_bg);

        if let Some(ref title) = panel.title {
            let text: String = title.spans.iter().map(|s| s.text.as_str()).collect();
            let text_rect = Rect::new(tb.x + 4.0, tb.y, (tb.width - 4.0).max(0.0), tb.height);
            let _ = dwrite.draw_text(target, &text, text_rect, theme.foreground);
        }

        for va in &layout.visible_actions {
            let action = &panel.actions[va.action_idx];
            let action_bg = if action.is_active {
                theme.accent_bg
            } else {
                title_bg
            };
            let _ = fill_rect(target, va.bounds, action_bg);

            let (glyph_w, _) = dwrite.measure_text(&action.icon).unwrap_or((0.0, 0.0));
            let glyph_x = va.bounds.x + ((va.bounds.width - glyph_w) / 2.0).max(0.0);
            let glyph_rect = Rect::new(
                glyph_x,
                va.bounds.y,
                (va.bounds.x + va.bounds.width - glyph_x).max(1.0),
                va.bounds.height,
            );
            let _ = dwrite.draw_text(target, &action.icon, glyph_rect, theme.foreground);
        }
    }

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

        let layout = surface
            .paint(|target| {
                draw_panel(target, &dwrite, rect, &panel, LINE_HEIGHT);
            })
            .map(|_| win_panel_layout(rect, &panel, LINE_HEIGHT))
            .expect("paint panel");

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

        let layout = surface
            .paint(|target| {
                draw_panel(target, &dwrite, rect, &panel, LINE_HEIGHT);
            })
            .map(|_| win_panel_layout(rect, &panel, LINE_HEIGHT))
            .expect("paint panel");

        assert!(layout.title_bar_bounds.is_none());
        assert_eq!(layout.content_bounds.height, H as f32);
    }

    /// `win_panel_layout` (no-paint) must produce byte-identical layout
    /// to what `draw_panel` used to paint — same panel, same rect, same
    /// line height.
    #[test]
    fn no_paint_layout_matches_paint_layout() {
        let panel = panel();
        let rect = Rect::new(0.0, 0.0, W as f32, H as f32);

        let surface = HeadlessSurface::new(W, H).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
        let painted = surface
            .paint(|target| {
                draw_panel(target, &dwrite, rect, &panel, LINE_HEIGHT);
            })
            .map(|_| win_panel_layout(rect, &panel, LINE_HEIGHT))
            .expect("paint");
        let no_paint = win_panel_layout(rect, &panel, LINE_HEIGHT);

        assert_eq!(painted, no_paint);
    }
}
