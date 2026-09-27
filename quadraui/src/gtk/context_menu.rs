//! GTK rasteriser for [`crate::ContextMenu`].
//!
//! Content painting (background, separators, selection highlight,
//! labels + shortcut text, border) moved to the shared
//! [`crate::primitives::context_menu::native_surface_paint::paint`]
//! (#1077, `NativeSurface` Phase 4 slice 4/8) — see that fn's module
//! doc for the two small drifts it resolved (box corner rounding,
//! separator stroke weight).
//!
//! Returns per-clickable-item hit rectangles `(x, y, w, h, WidgetId)`
//! so the caller's click handler can resolve mouse events without
//! re-running layout.

use gtk4::cairo::Context;
use gtk4::pango;

use crate::accelerator::Platform;
use crate::primitives::context_menu::{native_surface_paint, ContextMenu, ContextMenuLayout};
use crate::theme::Theme;
use crate::types::WidgetId;

/// Draw a [`ContextMenu`] popup. Returns the per-clickable-item hit
/// rectangles in target-surface pixels.
#[allow(clippy::too_many_arguments)]
pub fn draw_context_menu(
    cr: &Context,
    layout: &pango::Layout,
    menu: &ContextMenu,
    menu_layout: &ContextMenuLayout,
    line_height: f64,
    theme: &Theme,
) -> Vec<(f64, f64, f64, f64, WidgetId)> {
    let _ = line_height;

    let mut surface = super::surface::CairoSurface {
        cr,
        layout: Some(layout),
        translucent_fill: true,
    };
    let hits = native_surface_paint::paint(menu, menu_layout, Platform::Linux, &mut surface, theme);
    layout.set_attributes(None);

    hits.into_iter()
        .map(|(r, id)| (r.x as f64, r.y as f64, r.width as f64, r.height as f64, id))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::Rect;
    use crate::primitives::context_menu::{
        ContextMenuHit, ContextMenuItem, ContextMenuItemMeasure,
    };
    use crate::types::StyledText;
    use pangocairo::cairo::{Context, Format, ImageSurface};

    fn action(id: &str) -> ContextMenuItem {
        ContextMenuItem {
            id: Some(WidgetId::new(id)),
            label: StyledText::plain(id),
            ..Default::default()
        }
    }

    fn menu() -> ContextMenu {
        ContextMenu {
            id: WidgetId::new("m"),
            items: vec![action("cut"), ContextMenuItem::default(), action("paste")],
            selected_idx: 0,
            bg: None,
            placement: crate::primitives::context_menu::ContextMenuPlacement::AnchorPoint,
        }
    }

    #[test]
    fn draw_context_menu_returns_hits_for_clickable_items_only() {
        let surface = ImageSurface::create(Format::ARgb32, 200, 120).expect("create ImageSurface");
        let cr = Context::new(&surface).expect("Context::new");
        let pango_layout = pangocairo::functions::create_layout(&cr);

        let m = menu();
        let viewport = Rect::new(0.0, 0.0, 200.0, 120.0);
        let layout = m.layout(10.0, 10.0, viewport, 120.0, |i| {
            if m.items[i].is_separator() {
                ContextMenuItemMeasure::new(6.0)
            } else {
                ContextMenuItemMeasure::new(20.0)
            }
        });

        let hits = draw_context_menu(&cr, &pango_layout, &m, &layout, 20.0, &Theme::default());
        assert_eq!(hits.len(), 2, "cut + paste are clickable, separator is not");
        assert_eq!(hits[0].4, WidgetId::new("cut"));
        assert_eq!(hits[1].4, WidgetId::new("paste"));

        // hit_test agrees with what was painted.
        let (x, y, _, _, _) = hits[0];
        let hit = layout.hit_test(x as f32 + 4.0, y as f32 + 2.0);
        assert_eq!(hit, ContextMenuHit::Item(WidgetId::new("cut")));
    }
}
