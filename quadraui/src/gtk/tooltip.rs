//! GTK rasteriser for [`crate::Tooltip`].
//!
//! Cairo + Pango equivalent of `quadraui::tui::draw_tooltip`. Paints a
//! background rectangle at the resolved bounds, then border chrome per
//! `layout.border` (#541 — [`crate::TooltipBorder`]; see
//! `primitives::tooltip`'s module doc for why it lives on
//! [`TooltipLayout`] rather than on [`Tooltip`]):
//!
//! - [`TooltipBorder::Full`] (the default) strokes a full 4-sided box —
//!   GTK has always done this, unconditionally, before #541 gave it a
//!   name. An optional `layout.title` is centred over the top edge,
//!   punched through the stroke with a background-coloured backing
//!   rectangle so it reads as embedded in the border rather than a
//!   content row (matching the TUI rasteriser's top-row title).
//! - [`TooltipBorder::Sides`] strokes two vertical lines at the left and
//!   right edges only, no top/bottom — TUI's pre-#542 look, now
//!   available on GTK by explicit request. No title (no top rule).
//! - [`TooltipBorder::None`] strokes nothing.
//!
//! Then draws either the plain `text` or per-row `styled_lines`.

use gtk4::cairo::Context;
use gtk4::pango;

use super::cairo_rgb;
use crate::primitives::tooltip::{Tooltip, TooltipBorder, TooltipLayout};
use crate::theme::Theme;

/// Draw a [`Tooltip`] at its resolved layout position.
///
/// `padding_x` is the horizontal padding (in pixels) from the left
/// border to the start of text — consumers typically pass the same
/// `char_width` they used when computing the tooltip's measured width.
/// Halved when `layout.border` is [`TooltipBorder::None`], since there
/// is no border column to clear first — mirrors the TUI rasteriser's
/// `text_col_offset` dropping from 2 (border + pad) to 1 (pad only).
///
/// Per-tooltip `tooltip.fg` / `tooltip.bg` overrides win over the
/// theme defaults. The frame border always uses [`Theme::hover_border`].
#[allow(clippy::too_many_arguments)]
pub fn draw_tooltip(
    cr: &Context,
    layout: &pango::Layout,
    tooltip: &Tooltip,
    tooltip_layout: &TooltipLayout,
    line_height: f64,
    padding_x: f64,
    theme: &Theme,
) {
    let bounds = tooltip_layout.bounds;
    if bounds.width <= 0.0 || bounds.height <= 0.0 {
        return;
    }

    let bg = tooltip
        .bg
        .map(cairo_rgb)
        .unwrap_or_else(|| cairo_rgb(theme.hover_bg));
    let fg = tooltip
        .fg
        .map(cairo_rgb)
        .unwrap_or_else(|| cairo_rgb(theme.hover_fg));
    let border = cairo_rgb(theme.hover_border);

    let bx = bounds.x as f64;
    let by = bounds.y as f64;
    let bw = bounds.width as f64;
    let bh = bounds.height as f64;

    cr.set_source_rgb(bg.0, bg.1, bg.2);
    cr.rectangle(bx, by, bw, bh);
    cr.fill().ok();

    // Content normally starts 2px below the top edge; a title pushes
    // that down further below, since its real font height (title_h) is
    // typically much taller than the 1px border line it's centred on —
    // without this, a title would visually collide with the first
    // content row instead of sitting in its own space above it, the way
    // the TUI rasteriser's dedicated title row never overlaps content.
    let mut text_top = by + 2.0;

    match tooltip_layout.border {
        TooltipBorder::Full => {
            cr.set_source_rgb(border.0, border.1, border.2);
            cr.set_line_width(1.0);
            cr.rectangle(bx, by, bw, bh);
            cr.stroke().ok();

            if let Some(title) = tooltip_layout
                .title
                .as_deref()
                .map(str::trim)
                .filter(|t| !t.is_empty())
            {
                layout.set_text(title);
                layout.set_attributes(None);
                let (title_w, title_h) = layout.pixel_size();
                let (title_w, title_h) = (title_w as f64, title_h as f64);
                let pad = 4.0;
                let title_x = bx + ((bw - title_w) / 2.0).max(0.0);
                let title_y = by - title_h / 2.0;

                // Punch a background-coloured gap through the border
                // stroke so the title reads as embedded in the top rule,
                // not a content row sitting on top of it.
                cr.set_source_rgb(bg.0, bg.1, bg.2);
                cr.rectangle(title_x - pad, title_y, title_w + pad * 2.0, title_h);
                cr.fill().ok();

                cr.set_source_rgb(fg.0, fg.1, fg.2);
                cr.move_to(title_x, title_y);
                super::painted_text::show_layout(cr, layout);

                text_top = text_top.max(title_y + title_h + 2.0);
            }
        }
        TooltipBorder::Sides => {
            cr.set_source_rgb(border.0, border.1, border.2);
            cr.set_line_width(1.0);
            cr.move_to(bx, by);
            cr.line_to(bx, by + bh);
            cr.stroke().ok();
            cr.move_to(bx + bw, by);
            cr.line_to(bx + bw, by + bh);
            cr.stroke().ok();
        }
        TooltipBorder::None => {}
    }

    let text_padding_x = if matches!(tooltip_layout.border, TooltipBorder::None) {
        padding_x / 2.0
    } else {
        padding_x
    };
    let text_x = bx + text_padding_x;

    if let Some(ref styled_lines) = tooltip.styled_lines {
        for (i, styled) in styled_lines.iter().enumerate() {
            let row_y = text_top + i as f64 * line_height;
            if row_y + line_height > by + bh {
                break;
            }
            let mut x_off = text_x;
            for span in &styled.spans {
                let span_fg = span.fg.map(cairo_rgb).unwrap_or(fg);
                cr.set_source_rgb(span_fg.0, span_fg.1, span_fg.2);
                layout.set_text(&span.text);
                layout.set_attributes(None);
                cr.move_to(x_off, row_y);
                super::painted_text::show_layout(cr, layout);
                let (text_w, _) = layout.pixel_size();
                x_off += text_w as f64;
            }
        }
        return;
    }

    cr.set_source_rgb(fg.0, fg.1, fg.2);
    for (i, text_line) in tooltip.text.lines().enumerate() {
        let row_y = text_top + i as f64 * line_height;
        if row_y + line_height > by + bh {
            break;
        }
        layout.set_text(text_line);
        layout.set_attributes(None);
        cr.move_to(text_x, row_y);
        super::painted_text::show_layout(cr, layout);
    }
}

#[cfg(test)]
mod tests {
    use gtk4::cairo::{Context, Format, ImageSurface};

    use super::*;
    use crate::event::Rect as QRect;
    use crate::primitives::tooltip::{ResolvedPlacement, Tooltip, TooltipPlacement};
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
    fn sample_layout(border: TooltipBorder, title: Option<&str>) -> TooltipLayout {
        let mut layout = TooltipLayout {
            bounds: QRect::new(20.0, 20.0, 120.0, 24.0),
            resolved_placement: ResolvedPlacement::Bottom,
            border,
            title: None,
        };
        if let Some(t) = title {
            layout = layout.with_title(t);
        }
        layout
    }

    /// Paint `tooltip` at `layout.bounds` on a fresh surface and return
    /// the raw pixel buffer alongside the stride.
    fn paint(tooltip: &Tooltip, layout: &TooltipLayout) -> (Vec<u8>, usize) {
        let mut surface = ImageSurface::create(Format::ARgb32, W, H).expect("create ImageSurface");
        {
            let cr = Context::new(&surface).expect("Context::new");
            let pango_layout = pangocairo::functions::create_layout(&cr);
            draw_tooltip(
                &cr,
                &pango_layout,
                tooltip,
                layout,
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
        let layout = sample_layout(TooltipBorder::Full, None);
        let (data, stride) = paint(&tooltip, &layout);
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
        let layout = sample_layout(TooltipBorder::Sides, None);
        let (data, stride) = paint(&tooltip, &layout);
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
        let layout = sample_layout(TooltipBorder::None, None);
        let (data, stride) = paint(&tooltip, &layout);
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
        let layout = sample_layout(TooltipBorder::Full, Some("Hi"));
        let (data, stride) = paint(&tooltip, &layout);
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
            let with_layout = sample_layout(border, Some("Ignored"));
            let without_layout = sample_layout(border, None);
            let (with_data, stride) = paint(&with_title, &with_layout);
            let (without_data, _) = paint(&without_title, &without_layout);
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
}
