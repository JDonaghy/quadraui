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
//! fn) and the deprecated [`draw_sidebar_panel`] compatibility shim over
//! the shared [`super::surface::CairoSurface`] adapter (#1072 —
//! consolidated from this module's own private
//! `RawSidebarPanelSurface`; opaque fill preserved via
//! `CairoSurface::translucent_fill: false`, since — like the old
//! adapter's doc noted — nothing this primitive paints is ever
//! translucent).

use gtk4::cairo::Context;
use gtk4::pango;

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
    let mut surface = super::surface::CairoSurface {
        cr,
        layout: Some(pango_layout),
        translucent_fill: false,
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
