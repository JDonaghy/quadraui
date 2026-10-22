//! Direct2D / DirectWrite rasteriser for [`crate::ContextMenu`].
//!
//! Content painting is delegated to the shared
//! [`crate::primitives::context_menu::native_surface_paint::paint`]
//! — see that function's module doc for details on box corner
//! rounding and separator stroke weight. This module builds the
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

    /// An app-supplied [`ContextMenuGlyphs`] override swaps the checked-item
    /// check mark and submenu pull-right arrow the Windows rasteriser
    /// paints; with `glyphs: None`, [`WinBackend::draw_context_menu_with_glyphs`]
    /// paints the same codicon defaults as [`draw_context_menu`].
    #[test]
    fn checked_and_submenu_glyph_override_paints_app_glyph_default_paints_codicon() {
        use crate::backend::Backend;
        use crate::primitives::context_menu::ContextMenuGlyphs;
        use crate::win::backend::WinBackend;

        const W: u32 = 200;
        const H: u32 = 100;

        let mut m = menu();
        m.items[0].checked = Some(true);
        m.items[2].submenu = Some(vec![action("nested")]);
        let viewport = Rect::new(0.0, 0.0, W as f32, H as f32);
        let layout = m.layout(10.0, 10.0, viewport, 120.0, |i| {
            if m.items[i].is_separator() {
                ContextMenuItemMeasure::new(6.0)
            } else {
                ContextMenuItemMeasure::new(20.0)
            }
        });

        let paint = |glyphs: Option<&ContextMenuGlyphs>| {
            let surface = HeadlessSurface::new(W, H).expect("create surface");
            let mut backend = WinBackend::new();
            backend
                .attach_headless(surface.target().clone(), W, H)
                .expect("attach headless surface");
            backend.set_painted_text_recording(true);
            backend.begin_frame(crate::event::Viewport::new(W as f32, H as f32, 1.0));
            let _ = backend.draw_context_menu_with_glyphs(&m, &layout, glyphs);
            backend.end_frame();
            backend.text_runs().to_vec()
        };

        let glyphs = ContextMenuGlyphs::new()
            .with_checked_glyph("V")
            .with_submenu_arrow_glyph(">");
        let overridden = paint(Some(&glyphs));
        assert!(
            overridden.iter().any(|r| r.text == "V"),
            "expected the overridden check glyph among painted runs, got {:?}",
            overridden
                .iter()
                .map(|r| r.text.as_str())
                .collect::<Vec<_>>()
        );
        assert!(
            overridden.iter().any(|r| r.text == ">"),
            "expected the overridden submenu-arrow glyph among painted runs, got {:?}",
            overridden
                .iter()
                .map(|r| r.text.as_str())
                .collect::<Vec<_>>()
        );
        assert!(!overridden
            .iter()
            .any(|r| r.text.contains(crate::codicon::CHECK)));
        assert!(!overridden
            .iter()
            .any(|r| r.text.contains(crate::codicon::CHEVRON_RIGHT)));

        let default = paint(None);
        assert!(
            default
                .iter()
                .any(|r| r.text.contains(crate::codicon::CHECK)),
            "expected the default codicon check glyph among painted runs, got {:?}",
            default.iter().map(|r| r.text.as_str()).collect::<Vec<_>>()
        );
        assert!(
            default
                .iter()
                .any(|r| r.text.contains(crate::codicon::CHEVRON_RIGHT)),
            "expected the default codicon submenu-arrow glyph among painted runs, got {:?}",
            default.iter().map(|r| r.text.as_str()).collect::<Vec<_>>()
        );
    }
}
