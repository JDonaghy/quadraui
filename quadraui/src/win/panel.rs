//! Direct2D / DirectWrite rasteriser for [`crate::Panel`].
//!
//! `WinBackend::draw_panel` paints the title bar through the mapped
//! `SidebarPanel` + `Toolbar` header — see that method's own doc. This
//! module carries [`win_panel_layout`] for the pure layout queries
//! `WinBackend::panel_layout`'s no-paint hit-test path and
//! `draw_panel`'s own painting path (surfaced and no-surface alike)
//! need.
//!
//! Only compiled on `target_os = "windows"` — see `super::mod`'s
//! `#[cfg(target_os = "windows")] mod panel;` and `backend.rs`'s module
//! docs for why the rest of this repo's `--features win` compile gate
//! stays meaningful without a Windows host.
//!
//! # Theme
//!
//! `WinBackend::draw_panel` passes `&self.current_theme` to the delegate
//! — the same live theme `Backend::set_theme` writes, the same shape
//! `win::status_bar`'s module doc documents.

use crate::event::Rect;
use crate::primitives::layout_metrics::pixel_panel_layout;
// `Panel` is `#[deprecated]` (use `SidebarPanel` + `Toolbar` instead); this
// whole module is `Panel`'s own Windows rasteriser plumbing.
#[allow(deprecated)]
use crate::primitives::panel::{Panel, PanelLayout};

/// Compute a [`Panel`]'s layout without painting — used by
/// `WinBackend::panel_layout` for no-paint hit-test queries.
/// Shares its action-button width with `gtk_panel_layout` / `mac_panel_layout` via
/// [`pixel_panel_layout`].
#[allow(deprecated)] // `Panel` is `#[deprecated]`; this fn is its own plumbing.
pub fn win_panel_layout(rect: Rect, panel: &Panel, line_height: f32) -> PanelLayout {
    pixel_panel_layout(panel, rect.x, rect.y, rect.width, rect.height, line_height)
}

#[cfg(test)]
#[allow(deprecated)] // `Panel`/`PanelAction` are `#[deprecated]`; these tests cover their Windows layout plumbing directly.
mod tests {
    use super::*;
    use crate::primitives::panel::{PanelAction, PanelHit};
    use crate::types::{StyledText, WidgetId};

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

    /// Layout↔click round trip: title bar, action button, and content
    /// area each hit-test to the matching `PanelHit` variant at the
    /// bounds `win_panel_layout` reports. `WinBackend::draw_panel`'s own
    /// paint↔hit-test coverage (through the production
    /// `SidebarPanel` + `Toolbar` delegate) lives in
    /// `win::backend::tests`, Windows-only.
    #[test]
    fn hit_test_round_trip() {
        let panel = panel();
        let rect = Rect::new(0.0, 0.0, W as f32, H as f32);
        let layout = win_panel_layout(rect, &panel, LINE_HEIGHT);

        // Title bar body (left of the action button) resolves to
        // `TitleBar`.
        let tb = layout.title_bar_bounds.expect("title bar present");
        let title_hit = layout.hit_test(2.0, tb.y + tb.height / 2.0);
        assert_eq!(title_hit, PanelHit::TitleBar(WidgetId::new("p")));

        // Action button resolves to `Action`.
        let va = &layout.visible_actions[0];
        let action_hit = layout.hit_test(
            va.bounds.x + va.bounds.width / 2.0,
            va.bounds.y + va.bounds.height / 2.0,
        );
        assert_eq!(action_hit, PanelHit::Action(WidgetId::new("p:close")));

        // Content area, below the title bar, resolves to `Content`.
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

        let rect = Rect::new(0.0, 0.0, W as f32, H as f32);
        let layout = win_panel_layout(rect, &panel, LINE_HEIGHT);

        assert!(layout.title_bar_bounds.is_none());
        assert_eq!(layout.content_bounds.height, H as f32);
    }

    /// `win_panel_layout` must agree with `Panel::layout` given the same
    /// pixel-shaped measure — the twin `gtk_panel_layout`/`mac_panel_layout`
    /// tests make the same assertion for their own backends.
    #[test]
    fn matches_panel_layout_directly() {
        let panel = panel();
        let rect = Rect::new(0.0, 0.0, W as f32, H as f32);

        let via_helper = win_panel_layout(rect, &panel, LINE_HEIGHT);
        let measure = crate::primitives::panel::PanelMeasure {
            title_bar_height: LINE_HEIGHT,
            action_button_width: crate::primitives::layout_metrics::pixel::PANEL_ACTION_BUTTON,
            content_padding: 0.0,
        };
        let direct = panel.layout(rect, measure);

        assert_eq!(via_helper, direct);
    }
}
