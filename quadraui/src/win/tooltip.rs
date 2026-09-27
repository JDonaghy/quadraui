//! Direct2D / DirectWrite rasteriser for [`crate::Tooltip`] (issue #28).
//!
//! Content painting moved to the shared
//! [`crate::primitives::tooltip::native_surface_paint::paint`] (#1077,
//! `NativeSurface` Phase 4 slice 4/8) — see that fn's module doc for the
//! one drift it resolved (styled-line span bold/italic/underline; this
//! backend already applied `span.bold` via `DWrite::draw_text_styled`,
//! which is exactly what `NativeSurface::surface_draw_text_run_styled`'s
//! Windows override still does, unchanged).
//!
//! Only compiled on `target_os = "windows"` — see `super::mod`'s
//! `#[cfg(target_os = "windows")] mod tooltip;` and `backend.rs`'s module
//! docs for why the rest of this repo's `--features win` compile gate
//! stays meaningful without a Windows host.
//!
//! # Theme
//!
//! `WinBackend` does not yet carry a live [`Theme`] (see `win::status_bar`'s
//! module doc) — callers without a per-tooltip override fall back to
//! [`Theme::default`].

use windows::Win32::Graphics::Direct2D::ID2D1RenderTarget;

use super::text::DWrite;
use crate::primitives::tooltip::{native_surface_paint, Tooltip, TooltipChrome, TooltipLayout};
use crate::theme::Theme;

/// Draw a [`Tooltip`] at its resolved layout with the default chrome
/// ([`crate::TooltipBorder::Full`], no title) — see
/// [`draw_tooltip_with_chrome`] for the full-chrome entry point
/// [`crate::win::WinBackend`] dispatches
/// [`crate::Backend::draw_tooltip_with_chrome`] to.
pub fn draw_tooltip(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    tooltip: &Tooltip,
    layout: &TooltipLayout,
    line_height: f32,
    padding_x: f32,
) {
    draw_tooltip_with_chrome(
        target,
        dwrite,
        tooltip,
        layout,
        &TooltipChrome::default(),
        line_height,
        padding_x,
    );
}

/// Draw a [`Tooltip`] at its resolved layout, with the border and
/// optional title requested by `chrome` (mirrors
/// `gtk::draw_tooltip_with_chrome`, #541). `padding_x` is the horizontal
/// gap (DIPs) between the left border and the start of text; halved
/// when `chrome.border` is [`crate::TooltipBorder::None`], matching the
/// GTK/TUI rasterisers.
pub fn draw_tooltip_with_chrome(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    tooltip: &Tooltip,
    layout: &TooltipLayout,
    chrome: &TooltipChrome,
    line_height: f32,
    padding_x: f32,
) {
    let theme = Theme::default();
    let mut surface = super::surface::D2dSurface {
        target,
        dwrite: Some(dwrite),
    };
    native_surface_paint::paint(
        tooltip,
        layout,
        chrome,
        line_height,
        padding_x,
        &mut surface,
        &theme,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::tooltip::{ResolvedPlacement, TooltipBorder, TooltipPlacement};
    use crate::types::WidgetId;
    use crate::win::testing::HeadlessSurface;

    const W: u32 = 200;
    const H: u32 = 80;

    fn sample_tooltip() -> Tooltip {
        Tooltip {
            id: WidgetId::new("tip"),
            text: "Hover hint".into(),
            styled_lines: None,
            placement: TooltipPlacement::Bottom,
            fg: None,
            bg: None,
        }
    }

    fn sample_layout() -> TooltipLayout {
        TooltipLayout {
            bounds: crate::event::Rect::new(20.0, 20.0, 120.0, 24.0),
            resolved_placement: ResolvedPlacement::Bottom,
        }
    }

    #[test]
    fn full_border_paints_bg_and_stroke() {
        let surface = HeadlessSurface::new(W, H).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let tooltip = sample_tooltip();
        let layout = sample_layout();
        let theme = Theme::default();

        surface
            .paint(|target| {
                draw_tooltip(target, &dwrite, &tooltip, &layout, 16.0, 8.0);
            })
            .expect("paint tooltip");

        let bg = theme.hover_bg;
        let border = theme.hover_border;
        let b = layout.bounds;
        let inner = surface.pixel_at((b.x + b.width - 4.0) as u32, (b.y + b.height - 4.0) as u32);
        assert_eq!((inner.r, inner.g, inner.b), (bg.r, bg.g, bg.b));

        // `win::text::stroke_rect` insets the stroke by half its width,
        // so a 1-DIP border on integer bounds covers exactly the
        // boundary row — probing `bounds.y` returns the border colour at
        // full strength rather than a half-coverage blend of border over
        // background.
        let top_edge = surface.pixel_at((b.x + b.width / 2.0) as u32, b.y as u32);
        assert_eq!(
            (top_edge.r, top_edge.g, top_edge.b),
            (border.r, border.g, border.b)
        );

        // The stroke stays inside `bounds`: the row above the tooltip is
        // untouched by it (an overlay must not paint over its
        // neighbours' pixels).
        let above = surface.pixel_at((b.x + b.width / 2.0) as u32, (b.y - 1.0) as u32);
        assert_ne!(
            (above.r, above.g, above.b),
            (border.r, border.g, border.b),
            "the border must not bleed above the tooltip's own bounds"
        );
    }

    #[test]
    fn none_border_paints_no_stroke() {
        let surface = HeadlessSurface::new(W, H).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let tooltip = sample_tooltip();
        let layout = sample_layout();
        let theme = Theme::default();
        let chrome = TooltipChrome::new(TooltipBorder::None);

        surface
            .paint(|target| {
                draw_tooltip_with_chrome(target, &dwrite, &tooltip, &layout, &chrome, 16.0, 8.0);
            })
            .expect("paint tooltip");

        let bg = theme.hover_bg;
        let b = layout.bounds;
        let top_edge = surface.pixel_at((b.x + b.width / 2.0) as u32, b.y as u32);
        assert_eq!((top_edge.r, top_edge.g, top_edge.b), (bg.r, bg.g, bg.b));
    }
}
