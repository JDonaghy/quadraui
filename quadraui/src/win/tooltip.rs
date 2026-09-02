//! Direct2D / DirectWrite rasteriser for [`crate::Tooltip`] (issue #28).
//!
//! Mirrors `gtk::tooltip`'s structure: [`Tooltip::layout`] (called by the
//! host, per the D6 contract — see `crate::primitives::tooltip`'s module
//! doc) already resolved `TooltipLayout::bounds`; this module only paints
//! the background, the border chrome requested by a [`TooltipChrome`],
//! and the text.
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

use super::text::{draw_line, fill_rect, stroke_rect, DWrite};
use crate::event::Rect;
use crate::primitives::tooltip::{Tooltip, TooltipBorder, TooltipChrome, TooltipLayout};
use crate::theme::Theme;

/// Draw a [`Tooltip`] at its resolved layout with the default chrome
/// ([`TooltipBorder::Full`], no title) — see [`draw_tooltip_with_chrome`]
/// for the full-chrome entry point [`crate::win::WinBackend`] dispatches
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
/// optional title requested by `chrome` (mirrors `gtk::draw_tooltip_with_chrome`,
/// #541). `padding_x` is the horizontal gap (DIPs) between the left
/// border and the start of text; halved when `chrome.border` is
/// [`TooltipBorder::None`], matching the GTK/TUI rasterisers.
pub fn draw_tooltip_with_chrome(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    tooltip: &Tooltip,
    layout: &TooltipLayout,
    chrome: &TooltipChrome,
    line_height: f32,
    padding_x: f32,
) {
    let bounds = layout.bounds;
    if bounds.width <= 0.0 || bounds.height <= 0.0 {
        return;
    }

    let theme = Theme::default();
    let bg = tooltip.bg.unwrap_or(theme.hover_bg);
    let fg = tooltip.fg.unwrap_or(theme.hover_fg);
    let border = theme.hover_border;

    let _ = fill_rect(target, bounds, bg);

    let mut text_top = bounds.y + 2.0;

    match chrome.border {
        TooltipBorder::Full => {
            let _ = stroke_rect(target, bounds, border, 1.0);

            if let Some(title) = chrome
                .title
                .as_deref()
                .map(str::trim)
                .filter(|t| !t.is_empty())
            {
                let (title_w, title_h) = dwrite.measure_text(title).unwrap_or((0.0, 0.0));
                let pad = 4.0;
                let title_x = bounds.x + ((bounds.width - title_w) / 2.0).max(0.0);
                let title_y = bounds.y - title_h / 2.0;

                // Punch a background-coloured gap through the border stroke
                // so the title reads as embedded in the top rule.
                let gap_rect = Rect::new(title_x - pad, title_y, title_w + pad * 2.0, title_h);
                let _ = fill_rect(target, gap_rect, bg);

                let text_rect = Rect::new(title_x, title_y, title_w.max(1.0), title_h.max(1.0));
                let _ = dwrite.draw_text(target, title, text_rect, fg);

                text_top = text_top.max(title_y + title_h + 2.0);
            }
        }
        TooltipBorder::Sides => {
            let _ = draw_line(
                target,
                bounds.x,
                bounds.y,
                bounds.x,
                bounds.y + bounds.height,
                border,
                1.0,
            );
            let _ = draw_line(
                target,
                bounds.x + bounds.width,
                bounds.y,
                bounds.x + bounds.width,
                bounds.y + bounds.height,
                border,
                1.0,
            );
        }
        TooltipBorder::None => {}
    }

    let text_padding_x = if matches!(chrome.border, TooltipBorder::None) {
        padding_x / 2.0
    } else {
        padding_x
    };
    let text_x = bounds.x + text_padding_x;
    let text_w = (bounds.x + bounds.width - text_x).max(0.0);

    if let Some(ref styled_lines) = tooltip.styled_lines {
        for (i, styled) in styled_lines.iter().enumerate() {
            let row_y = text_top + i as f32 * line_height;
            if row_y + line_height > bounds.y + bounds.height {
                break;
            }
            let mut x_off = text_x;
            for span in &styled.spans {
                let span_fg = span.fg.unwrap_or(fg);
                let (span_w, _) = dwrite
                    .measure_text_styled(&span.text, span.bold)
                    .unwrap_or((0.0, 0.0));
                let rect = Rect::new(x_off, row_y, span_w.max(1.0), line_height);
                let _ = dwrite.draw_text_styled(target, &span.text, rect, span_fg, span.bold);
                x_off += span_w;
            }
        }
        return;
    }

    for (i, text_line) in tooltip.text.lines().enumerate() {
        let row_y = text_top + i as f32 * line_height;
        if row_y + line_height > bounds.y + bounds.height {
            break;
        }
        let rect = Rect::new(text_x, row_y, text_w, line_height);
        let _ = dwrite.draw_text(target, text_line, rect, fg);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::tooltip::{ResolvedPlacement, TooltipPlacement};
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
            bounds: Rect::new(20.0, 20.0, 120.0, 24.0),
            resolved_placement: ResolvedPlacement::Bottom,
        }
    }

    #[test]
    fn full_border_paints_bg_and_stroke() {
        let surface = HeadlessSurface::new(W, H).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
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

        let top_edge = surface.pixel_at((b.x + b.width / 2.0) as u32, b.y as u32);
        assert_eq!(
            (top_edge.r, top_edge.g, top_edge.b),
            (border.r, border.g, border.b)
        );
    }

    #[test]
    fn none_border_paints_no_stroke() {
        let surface = HeadlessSurface::new(W, H).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
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
