//! Direct2D / DirectWrite rasteriser for [`crate::ContextMenu`] (issue #28).
//!
//! Content painting moved to the shared
//! [`crate::primitives::context_menu::native_surface_paint::paint`]
//! (#1077, `PaintSurface` Phase 4 slice 4/8) — see that fn's module
//! doc for the two small drifts it resolved (box corner rounding,
//! separator stroke weight). This module only builds the
//! [`crate::win::surface::D2dSurface`] adapter and delegates.
//!
//! Only compiled on `target_os = "windows"` — see `super::mod`'s
//! `#[cfg(target_os = "windows")] mod context_menu;` and `backend.rs`'s
//! module docs.

use windows::Win32::Graphics::Direct2D::ID2D1RenderTarget;

use super::text::DWrite;
use crate::accelerator::Platform;
use crate::event::Rect;
use crate::primitives::context_menu::{native_surface_paint, ContextMenu, ContextMenuLayout};
use crate::theme::Theme;
use crate::types::WidgetId;

/// Draw a [`ContextMenu`] popup at its resolved `menu_layout`. Returns
/// the per-clickable-item hit rectangles (bar-local — same bounds the
/// layout itself carries) so the caller's click handler can resolve a
/// click without re-running layout.
pub fn draw_context_menu(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    menu: &ContextMenu,
    menu_layout: &ContextMenuLayout,
    theme: &Theme,
) -> Vec<(Rect, WidgetId)> {
    let mut surface = super::surface::D2dSurface {
        target,
        dwrite: Some(dwrite),
    };
    native_surface_paint::paint(
        menu,
        menu_layout,
        Platform::Windows,
        &mut surface,
        theme,
        &crate::style::Style::default(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::context_menu::{
        ContextMenuHit, ContextMenuItem, ContextMenuItemMeasure, ContextMenuPlacement,
    };
    use crate::types::StyledText;
    use crate::win::testing::HeadlessSurface;

    fn action(id: &str) -> ContextMenuItem {
        ContextMenuItem {
            id: Some(WidgetId::new(id)),
            label: StyledText::plain(id),
            ..Default::default()
        }
    }

    fn menu() -> ContextMenu {
        ContextMenu {
            id: WidgetId::new("ctx"),
            items: vec![action("copy"), ContextMenuItem::default(), action("paste")],
            selected_idx: 0,
            bg: None,
            placement: ContextMenuPlacement::AnchorPoint,
        }
    }

    #[test]
    fn paint_and_hit_test_round_trip() {
        let surface = HeadlessSurface::new(200, 100).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let m = menu();
        let viewport = Rect::new(0.0, 0.0, 200.0, 100.0);
        let layout = m.layout(10.0, 10.0, viewport, 120.0, |i| {
            if m.items[i].is_separator() {
                ContextMenuItemMeasure::new(6.0)
            } else {
                ContextMenuItemMeasure::new(20.0)
            }
        });

        // `HeadlessSurface::paint`'s closure is `FnOnce() -> ()`, so the
        // hit-rect `Vec` is captured into an outer binding rather than
        // returned through `paint` itself.
        let mut hits = Vec::new();
        surface
            .paint(|target| {
                hits = draw_context_menu(target, &dwrite, &m, &layout, &Theme::default());
            })
            .expect("paint context menu");

        assert_eq!(
            hits.len(),
            2,
            "copy + paste are clickable, separator is not"
        );
        assert_eq!(hits[0].1, WidgetId::new("copy"));
        assert_eq!(hits[1].1, WidgetId::new("paste"));

        // Selected (first) item's bg should be painted at its own bounds.
        let sel_bg = Theme::default().selected_bg;
        let (rect0, _) = &hits[0];
        let px = surface.pixel_at(
            (rect0.x + 4.0) as u32,
            (rect0.y + rect0.height / 2.0) as u32,
        );
        assert_eq!((px.r, px.g, px.b), (sel_bg.r, sel_bg.g, sel_bg.b));

        // hit_test agrees with what was painted.
        let hit = layout.hit_test(rect0.x + 4.0, rect0.y + 2.0);
        assert_eq!(hit, ContextMenuHit::Item(WidgetId::new("copy")));
    }
}
