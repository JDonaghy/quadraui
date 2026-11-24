//! macOS entry point for [`crate::Completions`] — a thin delegate onto
//! the widget's own shared pixel painter,
//! [`crate::primitives::completions::native_surface_paint::paint`], which
//! documents the popup chrome and row content it paints. This module only
//! adapts a Core Graphics context plus its font into the
//! [`crate::PaintSurface`] that painter takes.

use core_graphics::sys::CGContextRef;
use core_text::font::CTFont;

use crate::primitives::completions::{native_surface_paint, Completions, CompletionsLayout};
use crate::theme::Theme;

/// Paint a [`Completions`] popup at its resolved layout.
///
/// Kind icons paint their [`crate::Icon::fallback`] text: this entry
/// point carries no Nerd-Font toggle of its own, so it passes
/// `nerd_fonts_enabled: false`. A caller that wants the Nerd-Font glyph
/// instead paints the popup through
/// [`crate::DrawsPrimitives::draw_primitive`],
/// which resolves the toggle from the live backend.
///
/// # Safety
///
/// `ctx` must be a valid `CGContextRef` borrowed for the duration of
/// the call.
pub unsafe fn draw_completions(
    ctx: CGContextRef,
    font: &CTFont,
    completions: &Completions,
    layout: &CompletionsLayout,
    theme: &Theme,
) {
    let mut surface = super::surface::CgSurface {
        ctx,
        font: Some(font),
    };
    native_surface_paint::paint(
        completions,
        layout,
        &mut surface,
        theme,
        &crate::style::Style::default(),
        /* nerd_fonts_enabled */ false,
    );
}

#[cfg(test)]
mod tests {
    use super::super::headless::BitmapSurface;
    use super::super::text::make_font;
    use super::super::MacBackend;
    use super::*;
    use crate::event::{Rect as QRect, Viewport};
    use crate::primitives::completions::{
        CompletionItem, CompletionItemMeasure, CompletionKind, CompletionsHit,
    };
    use crate::types::{StyledText, WidgetId};
    use crate::Backend;

    const W: u32 = 200;
    const H: u32 = 120;

    fn font() -> CTFont {
        make_font("Menlo", 14.0).expect("Menlo installed")
    }

    fn sample() -> Completions {
        Completions {
            id: WidgetId::new("comp"),
            items: vec![
                CompletionItem {
                    label: StyledText::plain("len"),
                    detail: None,
                    documentation: None,
                    kind: CompletionKind::Method,
                    icon: None,
                },
                CompletionItem {
                    label: StyledText::plain("clone"),
                    detail: None,
                    documentation: None,
                    kind: CompletionKind::Method,
                    icon: None,
                },
                CompletionItem {
                    label: StyledText::plain("map"),
                    detail: None,
                    documentation: None,
                    kind: CompletionKind::Method,
                    icon: None,
                },
            ],
            selected_idx: 1,
            scroll_offset: 0,
            has_focus: true,
        }
    }

    fn layout_for(c: &Completions) -> CompletionsLayout {
        layout_for_at(c, 0.0, 0.0)
    }

    /// Like [`layout_for`] but anchors the cursor + viewport at an
    /// arbitrary `(origin_x, origin_y)` instead of always `(0, 0)`.
    /// Layout helpers must return coordinates in the same frame across
    /// backends; this method tests non-zero origins to catch placement
    /// regressions that a `(0, 0)`-origin viewport would not detect.
    fn layout_for_at(c: &Completions, origin_x: f32, origin_y: f32) -> CompletionsLayout {
        c.layout(
            origin_x + 20.0,
            origin_y + 20.0,
            16.0,
            QRect::new(origin_x, origin_y, W as f32 - origin_x, H as f32 - origin_y),
            120.0,
            80.0,
            |_| CompletionItemMeasure::new(16.0),
        )
    }

    fn paint_via_backend(c: &Completions, l: &CompletionsLayout) -> BitmapSurface {
        let surface = BitmapSurface::new(W, H);
        surface.fill(0.0, 0.0, 0.0, 0.0);
        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            b.draw_completions(c, l);
        });
        backend.end_frame();
        surface
    }

    #[test]
    fn popup_paints_completion_bg() {
        let c = sample();
        let l = layout_for(&c);
        let surface = paint_via_backend(&c, &l);
        let theme = Theme::default();
        // Probe near bottom-right of the popup interior.
        let px = (l.bounds.x + l.bounds.width - 4.0) as u32;
        let py = (l.bounds.y + l.bounds.height - 4.0) as u32;
        let (r, g, b, _) = surface.pixel(px, py);
        assert_eq!(
            (r, g, b),
            (
                theme.completion_bg.r,
                theme.completion_bg.g,
                theme.completion_bg.b
            ),
        );
    }

    #[test]
    fn selected_item_paints_completion_selected_bg() {
        let c = sample();
        let l = layout_for(&c);
        let surface = paint_via_backend(&c, &l);
        let theme = Theme::default();
        // selected_idx = 1 — find the visible item with idx 1.
        let item = l
            .visible_items
            .iter()
            .find(|v| v.item_idx == 1)
            .expect("selected item visible");
        let px = (item.bounds.x + item.bounds.width - 4.0) as u32;
        let py = (item.bounds.y + item.bounds.height / 2.0) as u32;
        let (r, g, b, _) = surface.pixel(px, py);
        assert_eq!(
            (r, g, b),
            (
                theme.completion_selected_bg.r,
                theme.completion_selected_bg.g,
                theme.completion_selected_bg.b
            ),
        );
    }

    /// Shared body for `hit_test_resolves_visible_items`, testing
    /// non-zero origins to catch placement regressions.
    fn hit_test_resolves_visible_items_at(origin_x: f32, origin_y: f32) {
        let c = sample();
        let l = layout_for_at(&c, origin_x, origin_y);
        assert!(
            !l.visible_items.is_empty(),
            "sample popup should show items"
        );
        for vis in &l.visible_items {
            let cx = vis.bounds.x + vis.bounds.width * 0.5;
            let cy = vis.bounds.y + vis.bounds.height * 0.5;
            assert_eq!(l.hit_test(cx, cy), CompletionsHit::Item(vis.item_idx));
        }
    }

    #[test]
    fn hit_test_resolves_visible_items() {
        hit_test_resolves_visible_items_at(0.0, 0.0);
    }

    /// Non-zero-origin regression guard: same round trip,
    /// with the popup's cursor + viewport anchored at a shifted origin.
    #[test]
    fn hit_test_resolves_visible_items_at_nonzero_origin() {
        hit_test_resolves_visible_items_at(7.0, 13.0);
    }
}
