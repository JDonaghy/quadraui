//! GTK rasteriser for [`crate::primitives::sidebar_panel::SidebarPanel`].
//!
//! Painting is implemented in the shared
//! [`crate::primitives::sidebar_panel::native_surface_paint::paint`].
//! This module provides [`gtk_sidebar_panel_layout`] for pure layout
//! via the Pango-measuring [`PangoMeasure`] adapter, used by
//! `GtkBackend::sidebar_panel_layout` for no-paint hit-test queries.

use gtk4::pango;

use crate::primitives::sidebar_panel::{SidebarPanel, SidebarPanelLayout, SidebarPanelMeasure};
use crate::primitives::toolbar::{measure_button, ToolbarItemMeasure};
#[cfg(test)]
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::sidebar_panel::SidebarPanelHit;

    /// `sidebar_panel_layout` is documented **ABSOLUTE**:
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

    /// Regression guard: paint and click must round-trip at nonzero origin.
    #[test]
    fn paint_and_click_round_trip_at_nonzero_origin() {
        round_trip_at(7.0, 13.0);
    }
}
