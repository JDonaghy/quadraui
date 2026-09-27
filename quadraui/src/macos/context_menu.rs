//! macOS rasteriser for [`crate::ContextMenu`].
//!
//! Content painting moved to the shared
//! [`crate::primitives::context_menu::native_surface_paint::paint`]
//! (#1077, `NativeSurface` Phase 4 slice 4/8) — see that fn's module
//! doc for the two small drifts it resolved (box corner rounding,
//! separator stroke weight). Returns per-clickable hit rectangles as
//! `Vec<(Rect, WidgetId)>` so the caller's click handler can resolve
//! menu clicks without re-running layout.

use core_graphics::sys::CGContextRef;
use core_text::font::CTFont;

use crate::accelerator::Platform;
use crate::event::Rect as QRect;
use crate::primitives::context_menu::{native_surface_paint, ContextMenu, ContextMenuLayout};
use crate::theme::Theme;
use crate::types::WidgetId;

/// Draw a [`ContextMenu`] popup. Returns per-clickable hit
/// rectangles paired with their item IDs.
///
/// # Safety
///
/// `ctx` must be a valid `CGContextRef` borrowed for the duration of
/// the call.
pub unsafe fn draw_context_menu(
    ctx: CGContextRef,
    font: &CTFont,
    menu: &ContextMenu,
    menu_layout: &ContextMenuLayout,
    theme: &Theme,
) -> Vec<(QRect, WidgetId)> {
    let mut surface = super::surface::CgSurface {
        ctx,
        font: Some(font),
    };
    native_surface_paint::paint(menu, menu_layout, Platform::Macos, &mut surface, theme)
}

#[cfg(test)]
mod tests {
    use super::super::headless::BitmapSurface;
    use super::super::text::make_font;
    use super::super::MacBackend;
    use super::*;
    use crate::event::Viewport;
    use crate::primitives::context_menu::{
        ContextMenuItem, ContextMenuItemMeasure, ContextMenuPlacement,
    };
    use crate::types::StyledText;
    use crate::Backend;

    const W: u32 = 200;
    const H: u32 = 120;

    fn font() -> CTFont {
        make_font("Menlo", 14.0).expect("Menlo installed")
    }

    fn item(id: &str, label: &str) -> ContextMenuItem {
        ContextMenuItem {
            id: Some(WidgetId::new(id)),
            label: StyledText::plain(label),
            ..Default::default()
        }
    }

    fn separator() -> ContextMenuItem {
        ContextMenuItem::default()
    }

    fn sample_menu() -> ContextMenu {
        ContextMenu {
            id: WidgetId::new("menu"),
            items: vec![
                item("cut", "Cut"),
                item("copy", "Copy"),
                separator(),
                item("paste", "Paste"),
            ],
            selected_idx: 1,
            placement: ContextMenuPlacement::Below,
            bg: None,
        }
    }

    fn layout_for(menu: &ContextMenu, viewport: QRect, item_h: f32) -> ContextMenuLayout {
        menu.layout(20.0, 20.0, viewport, 120.0, |_| {
            ContextMenuItemMeasure::new(item_h)
        })
    }

    fn paint_via_backend(
        menu: &ContextMenu,
        layout: &ContextMenuLayout,
    ) -> (BitmapSurface, Vec<(QRect, WidgetId)>) {
        let surface = BitmapSurface::new(W, H);
        surface.fill(0.0, 0.0, 0.0, 0.0);
        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        let hits = std::cell::RefCell::new(Vec::new());
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            *hits.borrow_mut() = b.draw_context_menu(menu, layout);
        });
        backend.end_frame();
        (surface, hits.into_inner())
    }

    #[test]
    fn menu_paints_bg_inside_bounds() {
        let menu = sample_menu();
        let viewport = QRect::new(0.0, 0.0, W as f32, H as f32);
        let layout = layout_for(&menu, viewport, 20.0);
        let (surface, _hits) = paint_via_backend(&menu, &layout);
        let theme = Theme::default();
        let b = layout.bounds;
        // Probe just inside the bordered rect, away from item glyphs.
        let (r, g, bp, _) =
            surface.pixel((b.x + b.width - 8.0) as u32, (b.y + b.height - 8.0) as u32);
        assert_eq!(
            (r, g, bp),
            (theme.hover_bg.r, theme.hover_bg.g, theme.hover_bg.b),
        );
    }

    #[test]
    fn selected_row_paints_selected_bg() {
        let menu = sample_menu();
        let viewport = QRect::new(0.0, 0.0, W as f32, H as f32);
        let layout = layout_for(&menu, viewport, 20.0);
        let (surface, _hits) = paint_via_backend(&menu, &layout);
        let theme = Theme::default();
        // selected_idx = 1 → second row (Copy).
        let row = layout
            .visible_items
            .iter()
            .find(|v| v.item_idx == 1 && v.clickable)
            .expect("copy row visible");
        let px = (row.bounds.x + row.bounds.width - 4.0) as u32;
        let py = (row.bounds.y + row.bounds.height / 2.0) as u32;
        let (r, g, b, _) = surface.pixel(px, py);
        assert_eq!(
            (r, g, b),
            (
                theme.selected_bg.r,
                theme.selected_bg.g,
                theme.selected_bg.b
            ),
        );
    }

    #[test]
    fn key_equivalent_renders_via_platform_macos() {
        // macOS renders KeyBinding::Save as ⌘S (vs Ctrl+S elsewhere).
        // Sanity-check the shared helper directly through its module
        // path — asserting on bitmap pixels for a multi-codepoint glyph
        // like ⌘ is brittle.
        use crate::accelerator::{
            render_accelerator, Accelerator, AcceleratorId, AcceleratorScope, KeyBinding, Platform,
        };
        let acc = Accelerator {
            id: AcceleratorId::new("editor.save"),
            binding: KeyBinding::Save,
            scope: AcceleratorScope::Global,
            label: None,
        };
        let shortcut = render_accelerator(&acc, Platform::Macos);
        assert!(
            shortcut.contains('⌘'),
            "macOS shortcut should contain ⌘, got {shortcut:?}",
        );
        assert!(
            shortcut.contains('S'),
            "macOS shortcut should contain S, got {shortcut:?}",
        );
    }

    #[test]
    fn hits_returned_for_clickable_items_only() {
        let menu = sample_menu();
        let viewport = QRect::new(0.0, 0.0, W as f32, H as f32);
        let layout = layout_for(&menu, viewport, 20.0);
        let (_surface, hits) = paint_via_backend(&menu, &layout);
        // Three clickable items (cut, copy, paste); separator not in hits.
        assert_eq!(hits.len(), 3);
        let ids: Vec<&str> = hits.iter().map(|(_, id)| id.as_str()).collect();
        assert!(ids.contains(&"cut"));
        assert!(ids.contains(&"copy"));
        assert!(ids.contains(&"paste"));
    }
}
