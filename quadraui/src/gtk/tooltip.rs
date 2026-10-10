//! GTK rasteriser for [`crate::Tooltip`].
//!
//! Content painting (background, border chrome, title punch, plain/
//! styled text) moved to the shared
//! [`crate::primitives::tooltip::native_surface_paint::paint`] (#1077,
//! `PaintSurface` Phase 4 slice 4/8) — see that fn's module doc for the
//! one drift it resolved (styled-line span bold/italic/underline, which
//! GTK gains here for the first time).
//!
//! [`draw_tooltip`] keeps its pre-#541 signature and renders
//! `TooltipChrome::default()`; [`draw_tooltip_with_chrome`] takes the
//! chrome request explicitly. See [`crate::TooltipBorder`] for what each
//! variant paints.

use gtk4::cairo::Context;
use gtk4::pango;

use crate::primitives::tooltip::{native_surface_paint, Tooltip, TooltipChrome, TooltipLayout};
use crate::theme::Theme;

/// Draw a [`Tooltip`] at its resolved layout position with the default
/// chrome — a [`crate::TooltipBorder::Full`] box, no title, i.e. exactly
/// what this rasteriser drew before #541 added a choice.
///
/// `padding_x` is the horizontal padding (in pixels) from the left
/// border to the start of text — consumers typically pass the same
/// `char_width` they used when computing the tooltip's measured width.
///
/// Per-tooltip `tooltip.fg` / `tooltip.bg` overrides win over the
/// theme defaults. The frame border always uses [`Theme::hover_border`].
///
/// To ask for different chrome, call [`draw_tooltip_with_chrome`].
pub fn draw_tooltip(
    cr: &Context,
    layout: &pango::Layout,
    tooltip: &Tooltip,
    tooltip_layout: &TooltipLayout,
    line_height: f64,
    padding_x: f64,
    theme: &Theme,
) {
    draw_tooltip_with_chrome(
        cr,
        layout,
        tooltip,
        tooltip_layout,
        &TooltipChrome::default(),
        line_height,
        padding_x,
        theme,
    );
}

/// Draw a [`Tooltip`] at its resolved layout position, with the border
/// and optional title requested by `chrome` (#541).
///
/// `padding_x` is the horizontal padding (in pixels) from the left
/// border to the start of text — consumers typically pass the same
/// `char_width` they used when computing the tooltip's measured width.
/// Halved when `chrome.border` is [`crate::TooltipBorder::None`], since
/// there is no border column to clear first.
///
/// Per-tooltip `tooltip.fg` / `tooltip.bg` overrides win over the
/// theme defaults. The frame border always uses [`Theme::hover_border`].
#[allow(clippy::too_many_arguments)]
pub fn draw_tooltip_with_chrome(
    cr: &Context,
    layout: &pango::Layout,
    tooltip: &Tooltip,
    tooltip_layout: &TooltipLayout,
    chrome: &TooltipChrome,
    line_height: f64,
    padding_x: f64,
    theme: &Theme,
) {
    let mut surface = super::surface::CairoSurface {
        cr,
        layout: Some(layout),
        translucent_fill: true,
    };
    native_surface_paint::paint(
        tooltip,
        tooltip_layout,
        chrome,
        line_height as f32,
        padding_x as f32,
        &mut surface,
        theme,
        &crate::style::Style::default(),
    );
    layout.set_attributes(None);
}

#[cfg(test)]
mod tests {
    use gtk4::cairo::{Context, Format, ImageSurface};

    use super::*;
    use crate::event::Rect as QRect;
    use crate::primitives::tooltip::{ResolvedPlacement, TooltipBorder, TooltipPlacement};
    use crate::types::WidgetId;

    const W: i32 = 200;
    const H: i32 = 80;
    const LINE_H: f64 = 16.0;
    const PAD_X: f64 = 8.0;

    /// Same byte layout as `gtk/data_table.rs`'s test helper and
    /// `GtkDriver::pixel`.
    fn pixel(data: &[u8], stride: usize, x: i32, y: i32) -> (u8, u8, u8) {
        let off = y as usize * stride + x as usize * 4;
        (data[off + 2], data[off + 1], data[off])
    }

    /// A pure-background reference pixel: bottom-right corner, a few
    /// pixels in from both edges — clear of any border stroke (which
    /// hugs the very edge) and of `sample_tooltip`'s short, top-aligned
    /// "Hover hint" body text (which a dead-centre probe can land on,
    /// since the box is only one line tall).
    fn bg_reference(data: &[u8], stride: usize, bounds: QRect) -> (u8, u8, u8) {
        pixel(
            data,
            stride,
            (bounds.x + bounds.width) as i32 - 4,
            (bounds.y + bounds.height) as i32 - 4,
        )
    }

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

    /// Bounds pinned to whole pixels — `cr`'s 1px-wide strokes centre on
    /// the path, so a whole-pixel origin keeps the edge probe and the
    /// "clear of any stroke" probe from straddling the same anti-aliased
    /// pixel row/column (same reasoning as
    /// `macos::tooltip::tests::tooltip_border_paints_at_edge`).
    fn sample_layout(border: TooltipBorder, title: Option<&str>) -> (TooltipLayout, TooltipChrome) {
        let layout = TooltipLayout {
            bounds: QRect::new(20.0, 20.0, 120.0, 24.0),
            resolved_placement: ResolvedPlacement::Bottom,
        };
        let mut chrome = TooltipChrome::new(border);
        chrome.title = title.map(str::to_string);
        (layout, chrome)
    }

    /// Paint `tooltip` at `layout.bounds` on a fresh surface and return
    /// the raw pixel buffer alongside the stride.
    fn paint(
        tooltip: &Tooltip,
        layout: &TooltipLayout,
        chrome: &TooltipChrome,
    ) -> (Vec<u8>, usize) {
        let mut surface = ImageSurface::create(Format::ARgb32, W, H).expect("create ImageSurface");
        {
            let cr = Context::new(&surface).expect("Context::new");
            let pango_layout = pangocairo::functions::create_layout(&cr);
            draw_tooltip_with_chrome(
                &cr,
                &pango_layout,
                tooltip,
                layout,
                chrome,
                LINE_H,
                PAD_X,
                &Theme::default(),
            );
        }
        surface.flush();
        let stride = surface.stride() as usize;
        let data = surface.data().expect("surface data").to_vec();
        (data, stride)
    }

    #[test]
    fn full_border_strokes_all_four_edges() {
        let tooltip = sample_tooltip();
        let (layout, chrome) = sample_layout(TooltipBorder::Full, None);
        let (data, stride) = paint(&tooltip, &layout, &chrome);
        let b = layout.bounds;
        let (bx, by, bw, bh) = (b.x as i32, b.y as i32, b.width as i32, b.height as i32);

        let inner = bg_reference(&data, stride, b);
        for (name, edge) in [
            ("top", pixel(&data, stride, bx + bw / 2, by)),
            ("bottom", pixel(&data, stride, bx + bw / 2, by + bh - 1)),
            ("left", pixel(&data, stride, bx, by + bh / 2)),
            ("right", pixel(&data, stride, bx + bw - 1, by + bh / 2)),
        ] {
            assert_ne!(
                edge, inner,
                "TooltipBorder::Full: {name} edge should show border ink, distinct from the \
                 interior background (edge={edge:?}, inner={inner:?})"
            );
        }
    }

    #[test]
    fn sides_border_only_strokes_left_and_right() {
        let tooltip = sample_tooltip();
        let (layout, chrome) = sample_layout(TooltipBorder::Sides, None);
        let (data, stride) = paint(&tooltip, &layout, &chrome);
        let b = layout.bounds;
        let (bx, by, bw, bh) = (b.x as i32, b.y as i32, b.width as i32, b.height as i32);

        let inner = bg_reference(&data, stride, b);
        let top = pixel(&data, stride, bx + bw / 2, by);
        let bottom = pixel(&data, stride, bx + bw / 2, by + bh - 1);
        let left = pixel(&data, stride, bx, by + bh / 2);
        let right = pixel(&data, stride, bx + bw - 1, by + bh / 2);

        assert_eq!(
            top, inner,
            "TooltipBorder::Sides must not stroke the top edge — no top/bottom rule, \
             regardless of box height (top={top:?}, inner={inner:?})"
        );
        assert_eq!(
            bottom, inner,
            "TooltipBorder::Sides must not stroke the bottom edge (bottom={bottom:?}, \
             inner={inner:?})"
        );
        assert_ne!(
            left, inner,
            "TooltipBorder::Sides must still stroke the left edge (left={left:?}, \
             inner={inner:?})"
        );
        assert_ne!(
            right, inner,
            "TooltipBorder::Sides must still stroke the right edge (right={right:?}, \
             inner={inner:?})"
        );
    }

    #[test]
    fn none_border_strokes_nothing() {
        let tooltip = sample_tooltip();
        let (layout, chrome) = sample_layout(TooltipBorder::None, None);
        let (data, stride) = paint(&tooltip, &layout, &chrome);
        let b = layout.bounds;
        let (bx, by, bw, bh) = (b.x as i32, b.y as i32, b.width as i32, b.height as i32);

        let inner = bg_reference(&data, stride, b);
        for (name, edge) in [
            ("top", pixel(&data, stride, bx + bw / 2, by)),
            ("bottom", pixel(&data, stride, bx + bw / 2, by + bh - 1)),
            ("left", pixel(&data, stride, bx, by + bh / 2)),
            ("right", pixel(&data, stride, bx + bw - 1, by + bh / 2)),
        ] {
            assert_eq!(
                edge, inner,
                "TooltipBorder::None must not stroke {name} — no chrome at all \
                 (edge={edge:?}, inner={inner:?})"
            );
        }
    }

    /// #541 ask 2: a title punches a background-coloured gap through the
    /// top stroke so it reads as embedded in the border, not a content
    /// row. Scans the top edge rather than probing one fixed x — the
    /// title is horizontally centred, so a single dead-centre sample can
    /// land on a glyph's own ink instead of the cleared pad around it.
    /// What the punch guarantees is: somewhere in the interior the top
    /// edge reads as background (the gap), and near the corners it's
    /// still the plain stroke (the punch is local to the title, not the
    /// whole edge).
    #[test]
    fn full_border_title_punches_a_gap_but_leaves_the_rest_of_the_top_edge_stroked() {
        let tooltip = sample_tooltip();
        let (layout, chrome) = sample_layout(TooltipBorder::Full, Some("Hi"));
        let (data, stride) = paint(&tooltip, &layout, &chrome);
        let b = layout.bounds;
        let (bx, by, bw) = (b.x as i32, b.y as i32, b.width as i32);

        let inner = bg_reference(&data, stride, b);
        let margin = 3; // stay clear of the corner glyphs (┌/┐ equivalent stroke joins)
        let interior: Vec<(u8, u8, u8)> = (bx + margin..bx + bw - margin)
            .map(|x| pixel(&data, stride, x, by))
            .collect();

        assert!(
            interior.contains(&inner),
            "the title's short label + padding should punch at least one background-\
             coloured pixel into the top edge somewhere in its interior (inner={inner:?}, \
             top-edge interior samples={interior:?})"
        );

        let top_near_corner = pixel(&data, stride, bx + 1, by);
        assert_ne!(
            top_near_corner, inner,
            "the top edge right at the corner should still show the plain border stroke, \
             i.e. the punch must not swallow the whole edge \
             (top_near_corner={top_near_corner:?}, inner={inner:?})"
        );
    }

    #[test]
    fn title_is_ignored_when_border_is_sides_or_none() {
        // A title set on a non-`Full` tooltip has no top rule to embed
        // into — both variants must render identically to their
        // no-title counterparts (no stray top-edge ink from an attempted
        // title punch/paint).
        for border in [TooltipBorder::Sides, TooltipBorder::None] {
            let with_title = sample_tooltip();
            let without_title = sample_tooltip();
            let (with_layout, with_chrome) = sample_layout(border, Some("Ignored"));
            let (without_layout, without_chrome) = sample_layout(border, None);
            let (with_data, stride) = paint(&with_title, &with_layout, &with_chrome);
            let (without_data, _) = paint(&without_title, &without_layout, &without_chrome);
            let b = with_layout.bounds;
            let (bx, by, bw) = (b.x as i32, b.y as i32, b.width as i32);
            let top_with = pixel(&with_data, stride, bx + bw / 2, by);
            let top_without = pixel(&without_data, stride, bx + bw / 2, by);
            assert_eq!(
                top_with, top_without,
                "{border:?}: setting `title` must not change what's painted at the top \
                 edge (top_with={top_with:?}, top_without={top_without:?})"
            );
        }
    }

    /// #1077: styled-line spans now render `bold` through
    /// `PaintSurface::surface_draw_text_run_styled` — GTK's
    /// pre-migration rasteriser cleared Pango attributes before drawing
    /// each span, silently dropping it. Observed RED before the port
    /// (GTK ignored `span.bold` entirely, so this failed): paint the
    /// same text once bold and once regular, and assert the bold run's
    /// ink extends further right — Pango's bold face is strictly wider
    /// per-glyph for any real font.
    #[test]
    fn styled_line_bold_span_paints_wider_ink_than_regular() {
        use crate::types::{StyledSpan, StyledText};

        // Furthest-right column (within `x_start..x_end`) that differs
        // from the pure-background reference colour, scanning every row
        // in `y_start..y_end` — i.e. the right edge of the painted ink.
        fn last_ink_x(
            data: &[u8],
            stride: usize,
            bg: (u8, u8, u8),
            x_start: i32,
            x_end: i32,
            y_start: i32,
            y_end: i32,
        ) -> i32 {
            let mut last = x_start;
            for y in y_start..y_end {
                for x in x_start..x_end {
                    if pixel(data, stride, x, y) != bg {
                        last = last.max(x);
                    }
                }
            }
            last
        }

        let (layout, chrome) = sample_layout(TooltipBorder::None, None);
        let b = layout.bounds;

        // Short enough that neither weight's ink reaches the tooltip's
        // own right edge (120px wide, minus 8px left padding) — the
        // very first version of this test used a string long enough
        // that *both* weights saturated the scan range at the same
        // rightmost column, hiding the width difference entirely.
        let mut bold_tip = sample_tooltip();
        bold_tip.styled_lines = Some(vec![StyledText {
            spans: vec![StyledSpan {
                bold: true,
                ..StyledSpan::plain("MMM")
            }],
        }]);
        let mut regular_tip = sample_tooltip();
        regular_tip.styled_lines = Some(vec![StyledText {
            spans: vec![StyledSpan::plain("MMM")],
        }]);

        let (bold_data, stride) = paint(&bold_tip, &layout, &chrome);
        let (regular_data, _) = paint(&regular_tip, &layout, &chrome);

        let x_start = b.x as i32;
        let x_end = (b.x + b.width) as i32;
        let y_start = b.y as i32 + 2; // text_top offset used by `paint`
        let y_end = y_start + LINE_H as i32;
        let bg = bg_reference(&bold_data, stride, b);

        let bold_ink_x = last_ink_x(&bold_data, stride, bg, x_start, x_end, y_start, y_end);
        let regular_ink_x = last_ink_x(&regular_data, stride, bg, x_start, x_end, y_start, y_end);

        assert!(
            bold_ink_x > regular_ink_x,
            "bold span should paint wider than the same text at regular weight \
             (bold_ink_x={bold_ink_x}, regular_ink_x={regular_ink_x})"
        );
    }
}
