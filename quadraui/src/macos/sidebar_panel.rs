//! macOS rasteriser for [`crate::primitives::sidebar_panel::SidebarPanel`].
//!
//! Paints the optional toolbar header by delegating to
//! [`super::toolbar::draw_toolbar`]. The content rect is **not**
//! painted — returned in `SidebarPanelLayout.content_bounds` for the
//! host to draw into.

use core_graphics::sys::CGContextRef;
use core_text::font::CTFont;

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

/// Paint a `SidebarPanel` onto `ctx`. Returns the resolved layout
/// for the host to paint content into and route clicks.
///
/// # Safety
///
/// `ctx` must be a valid `CGContextRef` borrowed for the duration of
/// the call (typical: the frame-scope pointer stashed on
/// [`super::MacBackend`]).
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
    let layout = mac_sidebar_panel_layout(panel, font, line_height, x, y, w, h);

    if w <= 0.0 || h <= 0.0 {
        return layout;
    }

    if let (Some(bar), Some(tb)) = (&panel.toolbar, layout.toolbar_bounds) {
        let _ = super::toolbar::draw_toolbar(
            ctx,
            font,
            tb.x as f64,
            tb.y as f64,
            tb.width as f64,
            tb.height as f64,
            bar,
            theme,
            hovered_toolbar_id,
            pressed_toolbar_id,
        );
    }

    layout
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
            let l =
                b.draw_sidebar_panel(QRect::new(0.0, 0.0, W as f32, H as f32), panel, None, None);
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
