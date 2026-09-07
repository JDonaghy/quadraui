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

use super::cairo_rgb;
use crate::primitives::palette::{Palette, PaletteItemMeasure, PaletteLayout, PaletteMode};
use crate::text_util::safe_prefix;
use crate::theme::Theme;

/// Scrollbar width in pixels — shared by [`gtk_palette_layout`] and
/// [`draw_palette`] so the two can't disagree on it.
const SB_W: f64 = 6.0;

/// Bottom margin reserved below the item list before the popup's own
/// bottom edge (there is no bottom border row on GTK the way TUI has
/// one — this is purely breathing room).
const BOTTOM_INSET: f64 = 4.0;

/// Compute the GTK [`Palette`] layout — the shared geometry
/// [`draw_palette`] paints from and `Backend::palette_layout` (#818)
/// exposes for hit-testing, so the two can't drift the way they used to
/// (see `docs/decisions/DECISIONS.md` D-007, "Palette: deferred, not
/// missed" — this closes it for GTK: `draw_palette` used to paint item
/// rows at an independently-derived `rows_y + i * line_height` instead
/// of this struct's own `visible_items[i].bounds.y`, under-reporting the
/// real paint position by the query/list separator's 1px whenever
/// `show_query` was true).
///
/// `query_h` bakes in that 1px separator stroke [`draw_palette`] paints
/// just below the query row, so [`PaletteLayout`]'s own `items_top`
/// (`title_height + query_height`) lands exactly on the first painted
/// item row.
///
/// Row count is floored to whole rows *before* calling [`Palette::layout`]
/// (matching `draw_palette`'s pre-#818 arithmetic) so the item list never
/// paints — or hit-tests — a partial last row.
///
/// Coordinate frame: **LOCAL** — `(0, 0)` is the popup's own top-left
/// corner, matching [`Palette::layout`]'s native contract (same
/// convention `mac_palette_layout` / `win_palette_layout` already use).
/// Returns the layout alongside `rows_h` (the item area's full row
/// capacity in pixels, already floored) — `draw_palette` needs it for
/// the scrollbar track / preview-pane / create-row positions that sit
/// below the last item, which aren't otherwise exposed as a single
/// [`PaletteLayout`] field.
pub fn gtk_palette_layout(
    w: f64,
    h: f64,
    palette: &Palette,
    line_height: f64,
) -> (PaletteLayout, f64) {
    let title_h = line_height as f32;
    let query_h = if palette.show_query {
        line_height as f32 + 1.0
    } else {
        0.0
    };
    let has_create = palette.create_label.is_some();
    let create_reserved = if has_create { line_height } else { 0.0 };
    let items_top = (title_h + query_h) as f64;
    let raw_items_h = (h - items_top - BOTTOM_INSET - create_reserved).max(0.0);
    let visible_rows = (raw_items_h / line_height) as usize;
    let rows_h = visible_rows as f64 * line_height;
    let viewport_h = items_top + rows_h + create_reserved;

    let layout = palette.layout(
        w as f32,
        viewport_h as f32,
        title_h,
        query_h,
        SB_W as f32,
        8.0,
        |_| PaletteItemMeasure::new(line_height as f32),
    );
    (layout, rows_h)
}

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

    let has_preview = palette.preview.is_some();
    let list_w = if has_preview { (w * 0.4).round() } else { w };

    // Separator paint position — not part of any `PaletteHit` region, so
    // it's fine for this to stay an independent formula (see
    // `gtk_palette_layout`'s doc for the fields that *are* shared to
    // avoid drift).
    let sep_y = if palette.show_query {
        y + 2.0 * line_height
    } else {
        y + line_height
    };

    // `Palette::layout` keeps the selected item visible internally (see
    // #711) — no backend-side scroll clamp needed here.
    let (palette_layout, rows_h) = gtk_palette_layout(w, h, palette, line_height);

    // `items_top` (title_height + query_height, LOCAL) read back from the
    // layout's own bounds rather than recomputed — the same fix applied
    // to the item rows below, applied here too so `rows_y` (used for the
    // scrollbar/preview/create-row positions outside `PaletteLayout`)
    // can't drift from it either.
    let items_top = palette_layout
        .query_bounds
        .map(|b| (b.y + b.height) as f64)
        .or_else(|| palette_layout.title_bounds.map(|b| (b.y + b.height) as f64))
        .unwrap_or(0.0);
    let rows_y = y + items_top;

    let content_w = palette_layout.item_list_width as f64
        - if palette_layout.scrollbar.is_some() {
            SB_W
        } else {
            0.0
        };

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
        super::painted_text::show_layout(cr, layout);
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
        super::painted_text::show_layout(cr, layout);

        let query_text_x = x + 8.0 + prompt_w as f64;
        layout.set_text(&palette.query);
        cr.move_to(query_text_x, query_y + (qh_px - qh as f64) / 2.0);
        super::painted_text::show_layout(cr, layout);

        let cursor_prefix: &str = safe_prefix(&palette.query, palette.query_cursor);
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
            super::painted_text::show_layout(cr, layout);
        }
    }

    // ── Separator row ─────────────────────────────────────────────────
    // In Input mode there is no item list, so no separator is drawn.
    if palette.show_query && palette.mode != PaletteMode::Input {
        cr.set_source_rgb(border.0, border.1, border.2);
        cr.set_line_width(1.0);
        cr.move_to(x, sep_y);
        cr.line_to(x + w, sep_y);
        cr.stroke().ok();
    }

    // ── Result rows ───────────────────────────────────────────────────
    // Input mode suppresses the item list entirely — the query field is the
    // only interaction target. A future iteration can add a mode badge to
    // the GTK title row; for now the list is simply hidden.
    if palette.mode == PaletteMode::Input {
        cr.restore().ok();
        return;
    }

    cr.save().ok();
    cr.rectangle(x, rows_y, content_w, rows_h);
    cr.clip();

    for vis_item in &palette_layout.visible_items {
        let item = &palette.items[vis_item.item_idx];
        // Consume the layout's own bounds directly — issue #818 / D-007's
        // fix for the 1px item-row drift this used to recompute
        // independently as `rows_y + render_i * line_height`.
        let row_y = y + vis_item.bounds.y as f64;
        let row_h = vis_item.bounds.height as f64;
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
            super::painted_text::show_layout(cr, layout);
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
            // #416: swap in a Nerd-Font-fallback variant of whatever base
            // font the row is painting with (the caller's `ui_font`, or —
            // for a palette with no explicit override — whatever font was
            // live before `draw_palette` was called) just for the glyph,
            // then restore immediately so the label/detail text painted
            // around it is unaffected. Without the swap, `icon.glyph`'s
            // Private-Use-Area codepoint depends on Pango finding some
            // installed font that covers it — unreliable, and prone to a
            // blank/tofu glyph.
            let base_font = layout.font_description().unwrap_or_default();
            let icon_font = super::with_nerd_font_fallback(&base_font);
            layout.set_font_description(Some(&icon_font));
            layout.set_text(glyph);
            let (iw, ih) = layout.pixel_size();
            cr.move_to(cursor, row_y + (row_h - ih as f64) / 2.0);
            super::painted_text::show_layout(cr, layout);
            layout.set_font_description(Some(&base_font));
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
        super::painted_text::show_layout(cr, layout);

        if let Some((detail_text, dw)) = detail_info {
            let dx = x + content_w - dw - 8.0;
            cr.set_source_rgb(dim.0, dim.1, dim.2);
            layout.set_attributes(None);
            layout.set_text(&detail_text);
            let (_, dh) = layout.pixel_size();
            cr.move_to(dx, row_y + (row_h - dh as f64) / 2.0);
            super::painted_text::show_layout(cr, layout);
        }
    }

    cr.restore().ok();
    layout.set_attributes(None);

    // ── Scrollbar ─────────────────────────────────────────────────────
    // Painted straight from the layout's own track/thumb rects — issue
    // #818's fix applies here too: this used to recompute a *different*
    // thumb-position formula (`thumb_ratio` / `scroll_frac`) than
    // `Palette::layout`'s internal `fit_thumb`, so a host hit-testing
    // `PaletteHit::ScrollbarThumb` against the struct would have
    // disagreed with where this function actually painted it.
    if let Some(sb) = &palette_layout.scrollbar {
        let track_x = x + sb.track.x as f64;
        let track_y = y + sb.track.y as f64;
        cr.set_source_rgb(bg.0 * 0.7, bg.1 * 0.7, bg.2 * 0.7);
        cr.rectangle(
            track_x,
            track_y,
            sb.track.width as f64,
            sb.track.height as f64,
        );
        cr.fill().ok();

        let thumb_x = x + sb.thumb.x as f64;
        let thumb_y = y + sb.thumb.y as f64;
        cr.set_source_rgb(border.0, border.1, border.2);
        cr.rectangle(
            thumb_x + 1.0,
            thumb_y,
            (sb.thumb.width as f64 - 2.0).max(0.0),
            sb.thumb.height as f64,
        );
        cr.fill().ok();
    }

    // ── Create action row (pinned below items) ────────────────────────
    if let Some(ref label) = palette.create_label {
        let create_y = rows_y + rows_h;
        let accent = cairo_rgb(theme.accent_fg);
        let prefix = "+ ";
        cr.set_source_rgb(accent.0, accent.1, accent.2);
        layout.set_attributes(None);
        layout.set_text(prefix);
        let (pw, ph) = layout.pixel_size();
        cr.move_to(x + 8.0, create_y + (line_height - ph as f64) / 2.0);
        super::painted_text::show_layout(cr, layout);
        layout.set_text(label);
        let (_, lh) = layout.pixel_size();
        cr.move_to(
            x + 8.0 + pw as f64,
            create_y + (line_height - lh as f64) / 2.0,
        );
        super::painted_text::show_layout(cr, layout);
    }

    // ── Preview pane ───────────────────────────────────────────────────
    if let Some(ref preview) = palette.preview {
        let preview_x = x + list_w;
        let preview_w = w - list_w;

        // Vertical separator between items and preview.
        cr.set_source_rgb(border.0, border.1, border.2);
        cr.set_line_width(1.0);
        cr.move_to(preview_x, rows_y);
        cr.line_to(preview_x, rows_y + rows_h);
        cr.stroke().ok();

        // Clip to preview area.
        cr.save().ok();
        cr.rectangle(preview_x, rows_y, preview_w, rows_h);
        cr.clip();

        let content_x = preview_x + 8.0;
        let content_right = x + w - 8.0;
        let mut cursor_y = rows_y;

        // Preview title.
        if let Some(ref title_text) = preview.title {
            cr.set_source_rgb(dim.0, dim.1, dim.2);
            layout.set_attributes(None);
            layout.set_text(title_text);
            let (_, th) = layout.pixel_size();
            cr.move_to(content_x, cursor_y + (line_height - th as f64) / 2.0);
            super::painted_text::show_layout(cr, layout);
            cursor_y += line_height;
        }

        // Preview content lines.
        let sel_rgb = cairo_rgb(theme.selected_bg);
        let preview_visible = ((rows_y + rows_h - cursor_y) / line_height) as usize;
        for (vi, line_idx) in (preview.scroll_offset..).take(preview_visible).enumerate() {
            let row_y = cursor_y + vi as f64 * line_height;
            if row_y + line_height > rows_y + rows_h + 0.5 {
                break;
            }

            let is_highlight = preview.highlight_line == Some(line_idx);
            if is_highlight {
                cr.set_source_rgb(sel_rgb.0, sel_rgb.1, sel_rgb.2);
                cr.rectangle(preview_x, row_y, preview_w, line_height);
                cr.fill().ok();
            }

            if line_idx < preview.lines.len() {
                let line = &preview.lines[line_idx];
                let mut cx = content_x;
                for span in &line.spans {
                    let span_rgb = span.fg.map(cairo_rgb).unwrap_or(fg);
                    cr.set_source_rgb(span_rgb.0, span_rgb.1, span_rgb.2);
                    layout.set_attributes(None);
                    layout.set_text(&span.text);
                    let (sw, sh) = layout.pixel_size();
                    cr.move_to(cx, row_y + (line_height - sh as f64) / 2.0);
                    super::painted_text::show_layout(cr, layout);
                    cx += sw as f64;
                    if cx > content_right {
                        break;
                    }
                }
            }
        }

        cr.restore().ok();
    }

    cr.restore().ok();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::palette::PaletteMode;
    use crate::types::WidgetId;
    use pangocairo::cairo::{Context, Format, ImageSurface};

    fn sample_palette(query: &str, query_cursor: usize) -> Palette {
        Palette {
            id: WidgetId::new("palette"),
            title: "Commands".into(),
            query: query.into(),
            query_cursor,
            items: Vec::new(),
            selected_idx: 0,
            scroll_offset: 0,
            total_count: 0,
            has_focus: true,
            show_query: true,
            create_label: None,
            preview: None,
            mode: PaletteMode::List,
        }
    }

    /// Regression for issue #503: `query_cursor` is a host-supplied byte
    /// offset (per the field doc above) with no guarantee it lands on a
    /// char boundary — `&palette.query[..query_cursor]` used to panic
    /// the moment a multibyte character sat left of the cursor.
    #[test]
    fn draw_palette_with_multibyte_cursor_does_not_panic() {
        let surface = ImageSurface::create(Format::ARgb32, 300, 200).expect("create ImageSurface");
        let cr = Context::new(&surface).expect("Context::new");
        let pango_layout = pangocairo::functions::create_layout(&cr);

        // "café🎉" — byte 4 sits inside the 2-byte 'é' (starts at byte 3).
        let query = "café🎉";
        assert!(!query.is_char_boundary(4));
        let palette = sample_palette(query, 4);
        let theme = Theme::default();

        // Must not panic.
        draw_palette(
            &cr,
            &pango_layout,
            0.0,
            0.0,
            300.0,
            200.0,
            &palette,
            &theme,
            18.0,
            false,
        );
    }
}
