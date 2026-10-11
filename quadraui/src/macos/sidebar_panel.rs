//! macOS rasteriser for [`crate::primitives::sidebar_panel::SidebarPanel`].
//!
//! This module carries [`mac_sidebar_panel_layout`] (pure layout, needed by
//! `MacBackend::sidebar_panel_layout` for hit-test queries). Painting uses the
//! shared [`crate::primitives::sidebar_panel::native_surface_paint::paint`].

use core_text::font::CTFont;

use crate::primitives::sidebar_panel::{SidebarPanel, SidebarPanelLayout, SidebarPanelMeasure};
use crate::primitives::toolbar::{measure_button, ToolbarItemMeasure};

/// Compute the macOS pixel-unit layout for a `SidebarPanel`. `font`
/// is required for accurate text measurement — it implements
/// [`crate::primitives::layout_metrics::TextMeasure`].
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
    panel.layout(
        bounds,
        SidebarPanelMeasure::new(line_height as f32, 8.0),
        |btn| ToolbarItemMeasure::new(measure_button(font, btn)),
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
    /// at both the origin and a non-zero origin. `mac_sidebar_panel_layout` bakes
    /// `x`/`y` straight into `panel.layout`'s returned bounds (absolute frame,
    /// matching the GTK/TUI twins) — call it directly (pure fn, no paint needed)
    /// and prove a click near the header's top-left still resolves to the toolbar
    /// button through `hit_test`.
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

    /// Non-zero-origin regression guard.
    #[test]
    fn click_in_header_resolves_to_toolbar_button_at_nonzero_origin() {
        click_in_header_round_trip_at(7.0, 13.0);
    }

    /// The header toolbar paints its button glyphs with the chrome font
    /// (`ChromeSurface`), so the slot reserved for that row must size
    /// itself off the same font's line height rather than the editor's
    /// independently-sized one — otherwise a chrome font taller than the
    /// editor's current line height clips the tops of the row's own icon
    /// glyphs against the header's clip rect. Holding the editor font
    /// tiny and fixed while growing only the chrome ("ui") font proves
    /// the slot height tracks chrome, not editor, metrics.
    #[test]
    fn toolbar_header_height_tracks_chrome_font_not_editor_font() {
        use crate::Backend;

        fn toolbar_height_for(editor: (&str, f32), chrome_desc: &str) -> f32 {
            let surface = BitmapSurface::new(W, H);
            surface.fill(0.0, 0.0, 0.0, 0.0);
            let mut backend = MacBackend::new();
            Backend::set_editor_font(&mut backend, editor.0, editor.1);
            Backend::set_ui_font(&mut backend, chrome_desc);
            backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
            let panel = panel_with_toolbar();
            let layout = std::cell::RefCell::new(None);
            backend.enter_frame_scope(surface.context_ptr(), |b| {
                let l = b.draw_sidebar_panel_interactive(
                    QRect::new(0.0, 0.0, W as f32, H as f32),
                    &panel,
                    &crate::InteractionState::new(),
                );
                *layout.borrow_mut() = Some(l);
            });
            backend.end_frame();
            layout
                .into_inner()
                .unwrap()
                .toolbar_bounds
                .expect("toolbar bounds present")
                .height
        }

        let h_small_chrome = toolbar_height_for(("Menlo", 9.0), "Menlo 9");
        let h_huge_chrome = toolbar_height_for(("Menlo", 9.0), "Menlo 60");
        assert!(
            h_huge_chrome > h_small_chrome * 2.0,
            "sidebar header slot height must grow with the chrome font, not stay \
             pinned to the editor's: {h_small_chrome} vs {h_huge_chrome}"
        );
    }
}
