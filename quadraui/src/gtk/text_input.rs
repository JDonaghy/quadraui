//! GTK rasteriser for [`crate::TextInput`].
//!
//! Paints a 1px border, the visible text lines, and a thin vertical
//! cursor bar (matches `FieldKind::TextInput` rendering inside `Form`).
//! Placeholder renders in `theme.muted_fg` when active.
//!
//! # Focus-visible ring
//!
//! When `ti.has_focus`, [`draw_text_input`] strokes an outset ring in
//! [`Theme::focus_ring_color`] around `rect`, geometry resolved through
//! the shared
//! [`crate::primitives::button::native_surface_paint::focus_ring_geometry`]
//! helper (`style.focus_ring_width`) — the same seam `gtk::button` and
//! `gtk::switch` already paint through for their own rings, and the same
//! convention `win::text_input` and the macOS `TextInput` painter use.
//! `TextInput`'s own border always strokes square (no radius), so the
//! ring strokes square too (a plain `cr.rectangle`), ignoring the
//! geometry's `corner_radius`.

use gtk4::cairo::Context;
use gtk4::pango;

use super::cairo_rgb;
use crate::primitives::button::native_surface_paint::focus_ring_geometry;
use crate::primitives::text_input::{TextInput, TextInputLayout, TextInputMeasure};
use crate::style::Style;
use crate::theme::Theme;
use crate::types::Color;

pub fn gtk_text_input_layout(
    ti: &TextInput,
    rect: crate::event::Rect,
    line_height: f32,
    char_width: f32,
) -> TextInputLayout {
    ti.layout(rect, TextInputMeasure::new(line_height, char_width))
}

#[allow(clippy::too_many_arguments)]
pub fn draw_text_input(
    cr: &Context,
    layout: &pango::Layout,
    rect: crate::event::Rect,
    ti: &TextInput,
    theme: &Theme,
    style: &Style,
    line_height: f64,
    char_width: f64,
) -> TextInputLayout {
    let li = gtk_text_input_layout(ti, rect, line_height as f32, char_width as f32);
    if rect.width <= 0.0 || rect.height <= 0.0 {
        return li;
    }

    let (bg_r, bg_g, bg_b) = cairo_rgb(theme.background);
    let (fg_r, fg_g, fg_b) = cairo_rgb(theme.foreground);
    let (muted_r, muted_g, muted_b) = cairo_rgb(theme.muted_fg);
    let border = if ti.has_focus {
        theme.accent_fg
    } else {
        theme.border_fg
    };
    let (br, bg2, bb) = cairo_rgb(border);

    // Background.
    cr.set_source_rgb(bg_r, bg_g, bg_b);
    cr.rectangle(
        rect.x as f64,
        rect.y as f64,
        rect.width as f64,
        rect.height as f64,
    );
    cr.fill().ok();
    // 1px border.
    cr.set_source_rgb(br, bg2, bb);
    cr.set_line_width(1.0);
    cr.rectangle(
        rect.x as f64 + 0.5,
        rect.y as f64 + 0.5,
        rect.width as f64 - 1.0,
        rect.height as f64 - 1.0,
    );
    cr.stroke().ok();

    // Focus-visible ring: a second, outset stroke in the theme's ring
    // colour, independent of the border tint above — see this module's
    // doc for why it strokes square rather than at the geometry's
    // `corner_radius`. `ti.has_focus` already carries the caller's
    // focus-visible suppression (composed via
    // `crate::compose::FocusRing::is_focus_visible` before it's set on
    // `TextInput`), so no separate check is needed here.
    if ti.has_focus {
        let ring = focus_ring_geometry(rect, style);
        let (ring_r, ring_g, ring_b) = cairo_rgb(theme.focus_ring_color());
        cr.set_source_rgb(ring_r, ring_g, ring_b);
        cr.set_line_width(ring.width as f64);
        cr.rectangle(
            ring.rect.x as f64,
            ring.rect.y as f64,
            ring.rect.width as f64,
            ring.rect.height as f64,
        );
        cr.stroke().ok();
    }

    // Clip content rendering to the box interior so glyphs that
    // overhang the rect (proportional fonts, last visible char at the
    // edge) can't bleed past the border.
    cr.save().ok();
    cr.rectangle(
        rect.x as f64 + 1.0,
        rect.y as f64 + 1.0,
        rect.width as f64 - 2.0,
        rect.height as f64 - 2.0,
    );
    cr.clip();

    let h_scroll = li.resolved_scroll_col;

    // Helper: slice a line at char offset `h_scroll` and return the
    // tail string (the part the user can see given horizontal scroll).
    let slice_from = |line: &str, off: usize| -> String {
        if off == 0 {
            line.to_string()
        } else {
            line.chars().skip(off).collect()
        }
    };

    // Content.
    if li.placeholder_active {
        if let Some(text) = ti.placeholder.as_ref() {
            if let Some(first) = li.visible_lines.first() {
                layout.set_text(text);
                layout.set_attributes(None);
                let (_, th) = layout.pixel_size();
                cr.set_source_rgb(muted_r, muted_g, muted_b);
                let row_y = first.bounds.y as f64 + (first.bounds.height as f64 - th as f64) / 2.0;
                cr.move_to(first.bounds.x as f64, row_y);
                super::painted_text::show_layout(cr, layout);
            }
        }
    } else {
        for vis in &li.visible_lines {
            let full = ti.lines.get(vis.line_idx).map(String::as_str).unwrap_or("");
            let visible = slice_from(full, h_scroll);
            if visible.is_empty() {
                continue;
            }
            layout.set_text(&visible);
            layout.set_attributes(None);
            let (_, th) = layout.pixel_size();
            cr.set_source_rgb(fg_r, fg_g, fg_b);
            let row_y = vis.bounds.y as f64 + (vis.bounds.height as f64 - th as f64) / 2.0;
            cr.move_to(vis.bounds.x as f64, row_y);
            super::painted_text::show_layout(cr, layout);
        }
    }

    // Cursor — thin vertical bar at the proportional-font x position.
    // We re-shape the visible (post-scroll) tail through Pango and use
    // index_to_pos so the cursor lands on the actual painted glyph.
    if ti.has_focus {
        if let Some(cb) = li.cursor_bounds {
            let cursor_color = if theme.cursor == Color::rgb(0, 0, 0) {
                theme.accent_fg
            } else {
                theme.cursor
            };
            let (cr_r, cr_g, cr_b) = cairo_rgb(cursor_color);

            // Find the visible line that owns the cursor.
            let cursor_vis = li
                .visible_lines
                .iter()
                .find(|v| (v.bounds.y - cb.y).abs() < 0.5);
            let cursor_x_px = if let Some(vis) = cursor_vis {
                let full = ti.lines.get(vis.line_idx).map(String::as_str).unwrap_or("");
                let visible = slice_from(full, h_scroll);
                // Cursor column relative to the visible (post-scroll) string.
                let rel_col = ti.cursor_col.saturating_sub(h_scroll);
                let byte_off = visible
                    .char_indices()
                    .nth(rel_col)
                    .map(|(b, _)| b)
                    .unwrap_or(visible.len());
                layout.set_text(&visible);
                let pos = layout.index_to_pos(byte_off as i32);
                vis.bounds.x as f64 + pos.x() as f64 / pango::SCALE as f64
            } else {
                cb.x as f64
            };

            cr.set_source_rgb(cr_r, cr_g, cr_b);
            cr.rectangle(cursor_x_px, cb.y as f64 + 1.0, 1.5, cb.height as f64 - 2.0);
            cr.fill().ok();
        }
    }

    cr.restore().ok(); // pop the content clip

    li
}

#[cfg(test)]
mod layout_tests {
    use super::*;
    use crate::event::Point;
    use crate::primitives::text_input::TextInputHit;
    use crate::types::WidgetId;

    /// `text_input_layout` is documented **ABSOLUTE**:
    /// `content_bounds` / hit regions must be shifted by `rect.x`/
    /// `rect.y`, not left at (0, 0) — the case that hides a
    /// LOCAL/ABSOLUTE mixup.
    fn round_trip_at(x: f32, y: f32) {
        let mut ti = TextInput::new(WidgetId::new("ti"));
        ti.lines = vec!["hello".into()];
        ti.has_focus = true;
        let rect = crate::event::Rect::new(x, y, 40.0, 20.0);
        let layout = gtk_text_input_layout(&ti, rect, 10.0, 8.0);

        // Content sits inset by a fixed border + padding from `rect`'s
        // own origin (1px border + 1 char-width horizontal padding, 1px
        // vertical) — ABSOLUTE means that inset tracks `rect.x`/`rect.y`
        // exactly, not a fixed (0, 0)-relative offset.
        assert_eq!(layout.content_bounds.x, x + 1.0 + 8.0);
        assert_eq!(layout.content_bounds.y, y + 1.0);

        let p = Point::new(layout.content_bounds.x + 0.5, layout.content_bounds.y + 0.5);
        let hit = layout
            .hit_regions
            .iter()
            .find(|(r, _)| r.contains(p))
            .map(|(_, h)| h.clone());
        assert_eq!(hit, Some(TextInputHit::Line { line_idx: 0 }));
    }

    #[test]
    fn paint_and_click_round_trip() {
        round_trip_at(0.0, 0.0);
    }

    /// Tests that painting and clicking work correctly with non-zero origins.
    #[test]
    fn paint_and_click_round_trip_at_nonzero_origin() {
        round_trip_at(7.0, 13.0);
    }
}

#[cfg(test)]
mod paint_tests {
    use super::*;
    use crate::event::Rect;
    use crate::types::WidgetId;
    use pangocairo::cairo::{Format, ImageSurface};

    const W: i32 = 200;
    const H: i32 = 60;

    fn pixel(data: &[u8], stride: usize, x: i32, y: i32) -> (u8, u8, u8) {
        let off = y as usize * stride + x as usize * 4;
        // Cairo ARGB32 byte order on little-endian is BGRA.
        (data[off + 2], data[off + 1], data[off])
    }

    fn sample(ti: &TextInput, rect: Rect, style: &Style) -> ImageSurface {
        let surface = ImageSurface::create(Format::ARgb32, W, H).expect("create ImageSurface");
        {
            let cr = Context::new(&surface).expect("Context::new");
            // Baseline fill distinct from every colour `draw_text_input`
            // paints, so any pixel still showing it proves nothing was
            // painted there.
            cr.set_source_rgb(10.0 / 255.0, 20.0 / 255.0, 30.0 / 255.0);
            cr.paint().ok();

            let pango_layout = pangocairo::functions::create_layout(&cr);
            draw_text_input(
                &cr,
                &pango_layout,
                rect,
                ti,
                &Theme::default(),
                style,
                16.0,
                8.0,
            );
        }
        surface
    }

    /// Focus-visible ring: present (outset of the border, in
    /// `theme.focus_ring_color()`) when `ti.has_focus`, absent when not
    /// — the gtk-rasteriser half of the seam's "ring present after Tab
    /// focus, absent after a click" acceptance. Samples a point on the
    /// ring's *outset* band (outside `rect` entirely, in the
    /// `focus_ring_geometry` gap), so unlike a sample inside `rect` this
    /// cannot pass merely because the border happens to share
    /// `theme.accent_fg` with `Theme::focus_ring_color()`; only the ring
    /// stroke itself reaches this pixel.
    #[test]
    fn focus_visible_ring_paints_in_the_outset_gap_only_when_focused() {
        let rect = Rect::new(20.0, 15.0, 100.0, 30.0);
        let style = Style::default();
        let theme = Theme::default();

        // Centre of the ring's stroke band on the left edge, well clear
        // of the top/bottom corners: `focus_ring_geometry`'s outset gap
        // is 2px, so `rect.x - 2.0` sits on the ring's own outset rect
        // edge, inside the 2px-wide stroke centred on it.
        let ring_x = (rect.x - 2.0) as i32;
        let ring_y = (rect.y + rect.height / 2.0) as i32;

        for has_focus in [true, false] {
            let mut ti = TextInput::new(WidgetId::new("ti"));
            ti.lines = vec!["hello".into()];
            ti.has_focus = has_focus;

            let mut surface = sample(&ti, rect, &style);
            let stride = surface.stride() as usize;
            let data = surface.data().expect("surface data");
            let (r, g, b) = pixel(&data, stride, ring_x, ring_y);

            if has_focus {
                assert_eq!(
                    (r, g, b),
                    (
                        theme.focus_ring_color().r,
                        theme.focus_ring_color().g,
                        theme.focus_ring_color().b
                    ),
                    "focused TextInput should paint the focus-visible ring in the outset gap",
                );
            } else {
                assert_eq!(
                    (r, g, b),
                    (10, 20, 30),
                    "unfocused TextInput should not paint a focus-visible ring",
                );
            }
        }
    }
}
