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
//! `GtkBackend::panel_layout` for no-paint hit-test queries); the
//! deprecated `draw_panel` compatibility shim over the shared
//! [`super::surface::CairoSurface`] adapter was removed in issue #1109
//! (zero uses in coord-tui's `main` and vimcode's `develop`).

use crate::primitives::layout_metrics::pixel_panel_layout;
use crate::primitives::panel::{Panel, PanelLayout};

/// Compute the GTK pixel-unit layout for a [`Panel`] without painting.
/// Shares its action-button width with `mac_panel_layout` /
/// `win_panel_layout` via [`pixel_panel_layout`] (issue #1079).
pub fn gtk_panel_layout(
    panel: &Panel,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    line_height: f64,
) -> PanelLayout {
    pixel_panel_layout(
        panel,
        x as f32,
        y as f32,
        w as f32,
        h as f32,
        line_height as f32,
    )
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
