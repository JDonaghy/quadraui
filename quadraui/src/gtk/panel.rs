//! GTK rasteriser for [`crate::Panel`].
//!
//! Painting goes through `GtkBackend::draw_panel`, which maps the title
//! bar's background + label onto a [`crate::SidebarPanel`] +
//! [`crate::Toolbar`] header (via
//! [`Panel::header_toolbar`][crate::primitives::panel::Panel::header_toolbar],
//! with its `Action` buttons dropped) and paints that with
//! [`crate::primitives::sidebar_panel::native_surface_paint::paint`].
//! Action buttons are painted directly against this module's own
//! `PanelLayout::visible_actions` rects instead — `header_toolbar`'s
//! `Action` buttons are measured by `Toolbar`'s text-width-based
//! `measure_button`, not `PanelMeasure::action_button_width`, so the
//! delegate's own action layout would otherwise disagree with the
//! fixed-width rects this module's layout (and `draw_panel`'s hit-test
//! contract) use. This module provides [`gtk_panel_layout`] for
//! computing pixel-unit layout without painting, used by
//! `GtkBackend::panel_layout` (and as `draw_panel`'s own hit-test return
//! value) for hit-test queries.

use crate::primitives::layout_metrics::pixel_panel_layout;
// `Panel` is `#[deprecated]` (use `SidebarPanel` + `Toolbar` instead); this
// whole module is `Panel`'s own GTK rasteriser plumbing.
#[allow(deprecated)]
use crate::primitives::panel::{Panel, PanelLayout};

/// Compute the GTK pixel-unit layout for a [`Panel`] without painting.
/// Shares its action-button width with `mac_panel_layout` /
/// `win_panel_layout` via [`pixel_panel_layout`].
#[allow(deprecated)] // `Panel` is `#[deprecated]`; this fn is its own plumbing.
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
#[allow(deprecated)] // `Panel`/`PanelAction` are `#[deprecated]`; these tests cover their GTK rasteriser directly.
mod tests {
    use super::*;
    use crate::primitives::panel::{Panel, PanelAction, PanelHit};
    use crate::types::{StyledText, WidgetId};

    /// `panel_layout` is documented **ABSOLUTE**:
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

    /// Non-zero-origin regression guard.
    #[test]
    fn paint_and_click_round_trip_at_nonzero_origin() {
        round_trip_at(7.0, 13.0);
    }
}
