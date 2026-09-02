//! Direct2D / DirectWrite rasteriser for [`crate::Completions`] (issue #28).
//!
//! Mirrors `gtk::completions`'s structure: the layout is fully resolved
//! upstream (host calls [`crate::primitives::completions::Completions::layout`]
//! with the cursor anchor, viewport, popup size, and a per-item measure
//! closure); this module paints the resolved [`CompletionsLayout`]
//! verbatim — background fill, full 4-side border stroke, per-item
//! selected-row highlight, `" {label}"` text.
//!
//! Only compiled on `target_os = "windows"` — see `super::mod`'s
//! `#[cfg(target_os = "windows")] mod completions;` and `backend.rs`'s
//! module docs.

use windows::Win32::Graphics::Direct2D::ID2D1RenderTarget;

use super::text::{fill_rect, stroke_rect, DWrite};
use crate::event::Rect;
use crate::primitives::completions::{Completions, CompletionsLayout};
use crate::theme::Theme;

/// Paint a [`Completions`] popup at its resolved `layout`.
pub fn draw_completions(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    completions: &Completions,
    layout: &CompletionsLayout,
) {
    let bounds = layout.bounds;
    if bounds.width <= 0.0 || bounds.height <= 0.0 {
        return;
    }

    let theme = Theme::default();
    let _ = fill_rect(target, bounds, theme.completion_bg);
    let _ = stroke_rect(target, bounds, theme.completion_border, 1.0);

    for vis in &layout.visible_items {
        let Some(item) = completions.items.get(vis.item_idx) else {
            continue;
        };

        if vis.item_idx == completions.selected_idx {
            let _ = fill_rect(target, vis.bounds, theme.completion_selected_bg);
        }

        let label = item
            .label
            .spans
            .first()
            .map(|s| s.text.as_str())
            .unwrap_or("");
        let display = format!(" {label}");
        let _ = dwrite.draw_text(target, &display, vis.bounds, theme.completion_fg);

        if let Some(ref detail) = item.detail {
            let detail_text: String = detail.spans.iter().map(|s| s.text.as_str()).collect();
            if !detail_text.is_empty() {
                let (dw, _) = dwrite.measure_text(&detail_text).unwrap_or((0.0, 0.0));
                let dx = vis.bounds.x + vis.bounds.width - dw - 4.0;
                let detail_rect = Rect::new(dx, vis.bounds.y, dw.max(1.0), vis.bounds.height);
                let _ = dwrite.draw_text(target, &detail_text, detail_rect, theme.muted_fg);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::completions::{CompletionItem, CompletionItemMeasure};
    use crate::types::{StyledText, WidgetId};
    use crate::win::testing::HeadlessSurface;

    fn completions() -> Completions {
        Completions {
            id: WidgetId::new("comp"),
            items: vec![
                CompletionItem {
                    label: StyledText::plain("println!"),
                    detail: None,
                    documentation: None,
                    kind: Default::default(),
                    icon: None,
                },
                CompletionItem {
                    label: StyledText::plain("print!"),
                    detail: None,
                    documentation: None,
                    kind: Default::default(),
                    icon: None,
                },
            ],
            selected_idx: 0,
            scroll_offset: 0,
            has_focus: true,
        }
    }

    #[test]
    fn paints_background_border_and_selection() {
        let surface = HeadlessSurface::new(200, 100).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
        let c = completions();
        let viewport = Rect::new(0.0, 0.0, 200.0, 100.0);
        let layout = c.layout(10.0, 10.0, 16.0, viewport, 120.0, 60.0, |_| {
            CompletionItemMeasure::new(16.0)
        });

        surface
            .paint(|target| {
                draw_completions(target, &dwrite, &c, &layout);
            })
            .expect("paint completions");

        let theme = Theme::default();
        let sel_bg = theme.completion_selected_bg;
        let first = layout.visible_items[0].bounds;
        let px = surface.pixel_at((first.x + 2.0) as u32, (first.y + 1.0) as u32);
        assert_eq!((px.r, px.g, px.b), (sel_bg.r, sel_bg.g, sel_bg.b));

        let border = theme.completion_border;
        let b = layout.bounds;
        let edge = surface.pixel_at((b.x + b.width / 2.0) as u32, b.y as u32);
        assert_eq!((edge.r, edge.g, edge.b), (border.r, border.g, border.b));
    }
}
