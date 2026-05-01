//! GTK rasteriser for [`crate::Palette`].
//!
//! Modal-style fuzzy picker with a title bar, query-input row, and a
//! scrollable result list. Cairo + Pango equivalent of
//! `quadraui::tui::draw_palette` with a square stroked border (vs the
//! TUI version's `╭─╮ ╰─╯` glyphs).
//!
//! Per-item `match_positions` (byte offsets) are highlighted via
//! per-character Pango `AttrColor` foreground spans using
//! [`Theme::match_fg`].

use gtk4::cairo::Context;
use gtk4::pango;
use pangocairo::functions as pcfn;

use super::cairo_rgb;
use crate::primitives::palette::{Palette, PaletteItemMeasure};
use crate::theme::Theme;

/// Draw a [`Palette`] modal into `(x, y, w, h)` on `cr`.
///
/// `nerd_fonts_enabled` selects between item icons' Nerd-Font glyph
/// and ASCII fallback. Caller is responsible for sizing / centring
/// the popup; this function paints a square stroked border at the
/// supplied bounds.
#[allow(clippy::too_many_arguments)]
pub fn draw_palette(
    cr: &Context,
    layout: &pango::Layout,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    palette: &Palette,
    theme: &Theme,
    line_height: f64,
    nerd_fonts_enabled: bool,
) {
    if w < 20.0 || h < line_height * 4.0 {
        return;
    }

    // Hard clip to popup bounds — selection bg, scrollbar thumb, match
    // attributes can't escape the frame.
    cr.save().ok();
    cr.rectangle(x, y, w, h);
    cr.clip();

    let bg = cairo_rgb(theme.surface_bg);
    let fg = cairo_rgb(theme.surface_fg);
    let query = cairo_rgb(theme.query_fg);
    let border = cairo_rgb(theme.border_fg);
    let title = cairo_rgb(theme.title_fg);
    let mtch = cairo_rgb(theme.match_fg);
    let sel = cairo_rgb(theme.selected_bg);
    let dim = cairo_rgb(theme.muted_fg);

    cr.set_source_rgb(bg.0, bg.1, bg.2);
    cr.rectangle(x, y, w, h);
    cr.fill().ok();

    cr.set_source_rgb(border.0, border.1, border.2);
    cr.set_line_width(1.0);
    cr.rectangle(x, y, w, h);
    cr.stroke().ok();

    layout.set_attributes(None);

    const BOTTOM_INSET: f64 = 4.0;
    let sep_y = y + 2.0 * line_height;
    let rows_y = sep_y + 1.0;
    let rows_h_raw = ((y + h) - rows_y - BOTTOM_INSET).max(0.0);
    let visible_rows = (rows_h_raw / line_height) as usize;
    let rows_h = visible_rows as f64 * line_height;
    let total = palette.items.len();
    let has_scrollbar = total > visible_rows;
    const SB_W: f64 = 6.0;
    let content_w = if has_scrollbar { w - SB_W } else { w };

    // Clamp so the selected item stays visible AND the visible window
    // stays full when there are enough items above to fill it (mirrors
    // `tui::draw_palette`).
    let max_offset = total.saturating_sub(visible_rows);
    let effective_offset = if visible_rows == 0 {
        0
    } else if palette.selected_idx < palette.scroll_offset {
        palette.selected_idx
    } else if palette.selected_idx >= palette.scroll_offset + visible_rows {
        palette.selected_idx + 1 - visible_rows
    } else {
        palette.scroll_offset
    };
    let effective_offset = effective_offset.min(max_offset);

    // Shallow-clone the palette so we can give `scroll_offset` the
    // visibility-clamped effective value without mutating the caller.
    let mut palette_local = palette.clone();
    palette_local.scroll_offset = effective_offset;
    let palette_layout = palette_local.layout(
        w as f32,
        (rows_y + rows_h - y) as f32,
        line_height as f32,
        line_height as f32,
        |_| PaletteItemMeasure::new(line_height as f32),
    );

    // ── Title row ─────────────────────────────────────────────────────
    if let Some(title_bounds) = palette_layout.title_bounds {
        let ty = y + title_bounds.y as f64;
        let th_px = title_bounds.height as f64;
        let title_text = if palette.total_count > 0 {
            format!(
                " {}  {}/{} ",
                palette.title,
                palette.items.len(),
                palette.total_count
            )
        } else {
            format!(" {} ", palette.title)
        };
        cr.set_source_rgb(title.0, title.1, title.2);
        layout.set_text(&title_text);
        let (_, text_h) = layout.pixel_size();
        cr.move_to(x + 8.0, ty + (th_px - text_h as f64) / 2.0);
        pcfn::show_layout(cr, layout);
    }

    // ── Query row ─────────────────────────────────────────────────────
    if let Some(query_bounds) = palette_layout.query_bounds {
        let query_y = y + query_bounds.y as f64;
        let qh_px = query_bounds.height as f64;
        let prompt = "> ";
        cr.set_source_rgb(query.0, query.1, query.2);
        layout.set_text(prompt);
        let (prompt_w, qh) = layout.pixel_size();
        cr.move_to(x + 8.0, query_y + (qh_px - qh as f64) / 2.0);
        pcfn::show_layout(cr, layout);

        let query_text_x = x + 8.0 + prompt_w as f64;
        layout.set_text(&palette.query);
        cr.move_to(query_text_x, query_y + (qh_px - qh as f64) / 2.0);
        pcfn::show_layout(cr, layout);

        let cursor_prefix: &str = if palette.query_cursor >= palette.query.len() {
            palette.query.as_str()
        } else {
            &palette.query[..palette.query_cursor]
        };
        layout.set_text(cursor_prefix);
        let (cursor_prefix_w, _) = layout.pixel_size();
        let cursor_x = query_text_x + cursor_prefix_w as f64;
        let cursor_char: String = palette
            .query
            .get(palette.query_cursor..)
            .and_then(|s| s.chars().next())
            .map(|c| c.to_string())
            .unwrap_or_else(|| " ".to_string());
        layout.set_text(&cursor_char);
        let (cursor_w, _) = layout.pixel_size();
        let cursor_w = (cursor_w as f64).max(line_height * 0.45);
        cr.set_source_rgb(query.0, query.1, query.2);
        cr.rectangle(cursor_x, query_y, cursor_w, qh_px);
        cr.fill().ok();
        if !cursor_char.trim().is_empty() {
            cr.set_source_rgb(bg.0, bg.1, bg.2);
            cr.move_to(cursor_x, query_y + (qh_px - qh as f64) / 2.0);
            layout.set_text(&cursor_char);
            pcfn::show_layout(cr, layout);
        }
    }

    // ── Separator row ─────────────────────────────────────────────────
    cr.set_source_rgb(border.0, border.1, border.2);
    cr.set_line_width(1.0);
    cr.move_to(x, sep_y);
    cr.line_to(x + w, sep_y);
    cr.stroke().ok();

    // ── Result rows ───────────────────────────────────────────────────
    cr.save().ok();
    cr.rectangle(x, rows_y, content_w, rows_h);
    cr.clip();

    for (render_i, vis_item) in palette_layout.visible_items.iter().enumerate() {
        let item = &palette.items[vis_item.item_idx];
        let row_y = rows_y + render_i as f64 * line_height;
        let row_h = line_height;
        let is_selected = vis_item.item_idx == palette.selected_idx && palette.has_focus;

        if is_selected {
            cr.set_source_rgb(sel.0, sel.1, sel.2);
            cr.rectangle(x, row_y, content_w, row_h);
            cr.fill().ok();
        }

        let full_text: String = item.text.spans.iter().map(|s| s.text.as_str()).collect();

        // Pango AttrList: default fg over full range, then match_fg
        // spans at each `match_positions` byte offset (1 char each).
        let attr_list = pango::AttrList::new();
        let mut attr_fg = pango::AttrColor::new_foreground(
            (fg.0 * 65535.0) as u16,
            (fg.1 * 65535.0) as u16,
            (fg.2 * 65535.0) as u16,
        );
        attr_fg.set_start_index(0);
        attr_fg.set_end_index(full_text.len() as u32);
        attr_list.insert(attr_fg);

        if !item.match_positions.is_empty() {
            for &pos in &item.match_positions {
                if pos >= full_text.len() {
                    continue;
                }
                let char_len = full_text[pos..]
                    .chars()
                    .next()
                    .map(|c| c.len_utf8())
                    .unwrap_or(1);
                let mut attr_match = pango::AttrColor::new_foreground(
                    (mtch.0 * 65535.0) as u16,
                    (mtch.1 * 65535.0) as u16,
                    (mtch.2 * 65535.0) as u16,
                );
                attr_match.set_start_index(pos as u32);
                attr_match.set_end_index((pos + char_len) as u32);
                attr_list.insert(attr_match);
            }
        }

        let mut cursor = x + 8.0;

        // Selection prefix (▶ when focused, two spaces otherwise — keeps
        // non-selected text aligned with selected text).
        {
            let prefix = if is_selected { "▶ " } else { "  " };
            layout.set_attributes(None);
            cr.set_source_rgb(fg.0, fg.1, fg.2);
            layout.set_text(prefix);
            let (pw, ph) = layout.pixel_size();
            cr.move_to(cursor, row_y + (row_h - ph as f64) / 2.0);
            pcfn::show_layout(cr, layout);
            cursor += pw as f64;
        }

        if let Some(ref icon) = item.icon {
            let glyph = if nerd_fonts_enabled {
                icon.glyph.as_str()
            } else {
                icon.fallback.as_str()
            };
            layout.set_attributes(None);
            cr.set_source_rgb(fg.0, fg.1, fg.2);
            layout.set_text(glyph);
            let (iw, ih) = layout.pixel_size();
            cr.move_to(cursor, row_y + (row_h - ih as f64) / 2.0);
            pcfn::show_layout(cr, layout);
            cursor += iw as f64 + 6.0;
        }

        let detail_info = item.detail.as_ref().map(|detail| {
            let detail_text: String = detail.spans.iter().map(|s| s.text.as_str()).collect();
            layout.set_attributes(None);
            layout.set_text(&detail_text);
            let (dw, _) = layout.pixel_size();
            (detail_text, dw as f64)
        });

        layout.set_text(&full_text);
        layout.set_attributes(Some(&attr_list));
        let (_, lh) = layout.pixel_size();
        cr.move_to(cursor, row_y + (row_h - lh as f64) / 2.0);
        pcfn::show_layout(cr, layout);

        if let Some((detail_text, dw)) = detail_info {
            let dx = x + content_w - dw - 8.0;
            cr.set_source_rgb(dim.0, dim.1, dim.2);
            layout.set_attributes(None);
            layout.set_text(&detail_text);
            let (_, dh) = layout.pixel_size();
            cr.move_to(dx, row_y + (row_h - dh as f64) / 2.0);
            pcfn::show_layout(cr, layout);
        }
    }

    cr.restore().ok();
    layout.set_attributes(None);

    // ── Scrollbar ─────────────────────────────────────────────────────
    if has_scrollbar && visible_rows > 0 {
        let sb_x = x + w - SB_W;
        let sb_track_y = rows_y;
        let sb_track_h = rows_h;

        cr.set_source_rgb(bg.0 * 0.7, bg.1 * 0.7, bg.2 * 0.7);
        cr.rectangle(sb_x, sb_track_y, SB_W, sb_track_h);
        cr.fill().ok();

        let thumb_ratio = visible_rows as f64 / total as f64;
        let thumb_h = (sb_track_h * thumb_ratio).max(8.0);
        let max_scroll = total.saturating_sub(visible_rows) as f64;
        let scroll_frac = if max_scroll > 0.0 {
            effective_offset as f64 / max_scroll
        } else {
            0.0
        };
        let thumb_y = sb_track_y + scroll_frac * (sb_track_h - thumb_h);

        cr.set_source_rgb(border.0, border.1, border.2);
        cr.rectangle(sb_x + 1.0, thumb_y, SB_W - 2.0, thumb_h);
        cr.fill().ok();
    }

    cr.restore().ok();
}
