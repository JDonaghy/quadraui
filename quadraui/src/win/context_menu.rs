//! Direct2D / DirectWrite rasteriser for [`crate::ContextMenu`] (issue #28).
//!
//! Mirrors `gtk::context_menu`'s structure: the layout (row bounds,
//! separators, clickability) is fully resolved upstream by the host —
//! see `crate::compose::menu_system` — and passed in as a
//! [`ContextMenuLayout`]; this module only paints it and collects the
//! per-clickable-item hit rectangles the [`crate::Backend::draw_context_menu`]
//! contract asks for.
//!
//! Only compiled on `target_os = "windows"` — see `super::mod`'s
//! `#[cfg(target_os = "windows")] mod context_menu;` and `backend.rs`'s
//! module docs.

use windows::Win32::Graphics::Direct2D::ID2D1RenderTarget;

use super::text::{draw_line, fill_rect, stroke_rect, DWrite};
use crate::accelerator::{render_accelerator, Platform};
use crate::event::Rect;
use crate::primitives::context_menu::{ContextMenu, ContextMenuItem, ContextMenuLayout};
use crate::theme::Theme;
use crate::types::WidgetId;

/// Right-aligned shortcut text — `item.detail` (preferred, back-compat)
/// or rendered from `item.key_equivalent`. `None` if neither is set.
fn shortcut_text(item: &ContextMenuItem) -> Option<String> {
    if let Some(ref det) = item.detail {
        return Some(det.spans.iter().map(|sp| sp.text.as_str()).collect());
    }
    item.key_equivalent
        .as_ref()
        .map(|acc| render_accelerator(acc, Platform::Windows))
}

/// Draw a [`ContextMenu`] popup at its resolved `menu_layout`. Returns
/// the per-clickable-item hit rectangles (bar-local — same bounds the
/// layout itself carries) so the caller's click handler can resolve a
/// click without re-running layout.
pub fn draw_context_menu(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    menu: &ContextMenu,
    menu_layout: &ContextMenuLayout,
) -> Vec<(Rect, WidgetId)> {
    let bounds = menu_layout.bounds;
    if bounds.width <= 0.0 || bounds.height <= 0.0 {
        return Vec::new();
    }

    let theme = Theme::default();
    let bg = menu.bg.unwrap_or(theme.hover_bg);
    let border = theme.hover_border;
    let fg = theme.foreground;
    let sel = theme.selected_bg;
    let dim = theme.muted_fg;

    let _ = fill_rect(target, bounds, bg);

    let mut rects: Vec<(Rect, WidgetId)> = Vec::new();

    // Pass 1: backgrounds (separators + selection highlight).
    for vis in &menu_layout.visible_items {
        if vis.is_separator {
            let sep_y = vis.bounds.y + vis.bounds.height * 0.5;
            let _ = draw_line(
                target,
                vis.bounds.x + 4.0,
                sep_y,
                vis.bounds.x + vis.bounds.width - 4.0,
                sep_y,
                dim,
                1.0,
            );
            continue;
        }

        let is_selected = vis.item_idx == menu.selected_idx && vis.clickable;
        if is_selected {
            let sel_rect = Rect::new(
                vis.bounds.x + 1.0,
                vis.bounds.y,
                (vis.bounds.width - 2.0).max(0.0),
                vis.bounds.height,
            );
            let _ = fill_rect(target, sel_rect, sel);
        }

        if vis.clickable {
            if let Some(ref id) = menu.items[vis.item_idx].id {
                rects.push((vis.bounds, id.clone()));
            }
        }
    }

    // Pass 2: text (labels + detail/shortcut) — on top of every
    // background so descenders are never clipped.
    for vis in &menu_layout.visible_items {
        if vis.is_separator {
            continue;
        }
        let item = &menu.items[vis.item_idx];

        let prefix = match item.checked {
            Some(true) => "\u{2713} ",
            Some(false) => "  ",
            None => "",
        };
        let label_text: String = std::iter::once(prefix.to_string())
            .chain(item.label.spans.iter().map(|s| s.text.clone()))
            .collect();
        let label_fg = if vis.clickable { fg } else { dim };
        let label_rect = Rect::new(
            vis.bounds.x + 8.0,
            vis.bounds.y,
            (vis.bounds.width - 8.0).max(0.0),
            vis.bounds.height,
        );
        let _ = dwrite.draw_text(target, &label_text, label_rect, label_fg);

        if let Some(shortcut) = shortcut_text(item) {
            if !shortcut.is_empty() {
                let (sw, _) = dwrite.measure_text(&shortcut).unwrap_or((0.0, 0.0));
                let sc_rect = Rect::new(
                    vis.bounds.x + vis.bounds.width - sw - 8.0,
                    vis.bounds.y,
                    sw.max(1.0),
                    vis.bounds.height,
                );
                let _ = dwrite.draw_text(target, &shortcut, sc_rect, dim);
            }
        }
    }

    // Border on top so the selection bg never obscures the edges.
    let _ = stroke_rect(target, bounds, border, 1.0);

    rects
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::context_menu::{
        ContextMenuHit, ContextMenuItemMeasure, ContextMenuPlacement,
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
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
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
                hits = draw_context_menu(target, &dwrite, &m, &layout);
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
