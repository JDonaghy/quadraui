//! GTK rasteriser for [`crate::TextInput`].
//!
//! Paints a 1px border, the visible text lines, and a thin vertical
//! cursor bar (matches `FieldKind::TextInput` rendering inside `Form`).
//! Placeholder renders in `theme.muted_fg` when active.

use gtk4::cairo::Context;
use gtk4::pango;
use pangocairo::functions as pcfn;

use super::cairo_rgb;
use crate::primitives::text_input::{TextInput, TextInputLayout, TextInputMeasure};
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
                pcfn::show_layout(cr, layout);
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
            pcfn::show_layout(cr, layout);
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
