//! Direct2D / DirectWrite entry point for [`crate::Completions`] — a
//! thin delegate onto the widget's own shared pixel painter,
//! [`crate::primitives::completions::native_surface_paint::paint`], which
//! documents the popup chrome and row content it paints. This module only
//! adapts a render target plus its DirectWrite handle into the
//! [`crate::PaintSurface`] that painter takes.
//!
//! Only compiled on `target_os = "windows"` — see `super::mod`'s
//! `#[cfg(target_os = "windows")] mod completions;` and `backend.rs`'s
//! module docs.

use windows::Win32::Graphics::Direct2D::ID2D1RenderTarget;

use super::text::DWrite;
use crate::primitives::completions::{native_surface_paint, Completions, CompletionsLayout};
use crate::theme::Theme;

/// Paint a [`Completions`] popup at its resolved `layout`.
///
/// Kind icons paint their [`crate::Icon::fallback`] text: this entry
/// point carries no Nerd-Font toggle of its own, so it passes
/// `nerd_fonts_enabled: false`. A caller that wants the Nerd-Font glyph
/// instead paints the popup through
/// [`crate::DrawsPrimitives::draw_primitive`](crate::backend::DrawsPrimitives::draw_primitive),
/// which resolves the toggle from the live backend.
pub fn draw_completions(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    completions: &Completions,
    layout: &CompletionsLayout,
    theme: &Theme,
) {
    let mut surface = super::surface::D2dSurface {
        target,
        dwrite: Some(dwrite),
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
    use super::*;
    use crate::event::Rect;
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
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let c = completions();
        let viewport = Rect::new(0.0, 0.0, 200.0, 100.0);
        let layout = c.layout(10.0, 10.0, 16.0, viewport, 120.0, 60.0, |_| {
            CompletionItemMeasure::new(16.0)
        });

        surface
            .paint(|target| {
                draw_completions(target, &dwrite, &c, &layout, &Theme::default());
            })
            .expect("paint completions");

        let theme = Theme::default();
        let sel_bg = theme.completion_selected_bg;
        let first = layout.visible_items[0].bounds;
        let px = surface.pixel_at((first.x + 2.0) as u32, (first.y + 1.0) as u32);
        assert_eq!((px.r, px.g, px.b), (sel_bg.r, sel_bg.g, sel_bg.b));

        // Probe the *bottom* edge, not the top one: the selected row's
        // fill spans the popup's full width and is painted after the
        // border stroke, so it legitimately covers the top border line
        // (identical paint order to `gtk::draw_completions`, which this
        // rasteriser mirrors). The bottom edge sits under the second,
        // unselected item, where the stroke survives — and
        // `win::text::stroke_rect` insets the stroke so that last row
        // inside `bounds` is fully covered rather than antialiased
        // across the boundary.
        let border = theme.completion_border;
        let b = layout.bounds;
        let edge = surface.pixel_at((b.x + b.width / 2.0) as u32, (b.y + b.height - 1.0) as u32);
        assert_eq!((edge.r, edge.g, edge.b), (border.r, border.g, border.b));
    }
}
