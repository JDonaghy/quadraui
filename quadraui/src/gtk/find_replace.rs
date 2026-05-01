//! GTK rasteriser for [`crate::FindReplacePanel`].
//!
//! Cairo + Pango equivalent of [`crate::tui::draw_find_replace`].
//! Paints the find/replace overlay anchored at the top-right of
//! `panel.group_bounds`, walking `panel.hit_regions` for paint and
//! click hit-test single-sourcing.
//!
//! `char_width` and `line_height` are the editor's monospace cell
//! dimensions in pixels; cell-unit hit-region coordinates are scaled
//! by these to absolute Cairo coordinates.

use gtk4::cairo::Context;
use gtk4::pango;
use pangocairo::functions as pcfn;

use super::cairo_rgb;
use crate::primitives::find_replace::{FindReplaceClickTarget, FindReplacePanel};
use crate::theme::Theme;

/// Draw a [`FindReplacePanel`] at its anchored position in the active
/// editor group. Walks `panel.hit_regions` so paint and click
/// hit-test agree by construction.
#[allow(clippy::too_many_arguments)]
pub fn draw_find_replace(
    cr: &Context,
    layout: &pango::Layout,
    panel: &FindReplacePanel,
    theme: &Theme,
    line_height: f64,
    char_width: f64,
) {
    use FindReplaceClickTarget as T;

    let cw = char_width.max(1.0);
    let lh = line_height.max(1.0);

    // Engine returns regions in char cells; the renderer scales to
    // pixels via `cw` / `lh`. panel_width includes the 1-cell border
    // on each side.
    let panel_w_cells: f64 = panel.panel_width as f64;
    let popup_w = panel_w_cells * cw;
    let row_count = if panel.show_replace { 2.0 } else { 1.0 };
    // Panel height in pixels: 2 border rows + content rows.
    let popup_h = (row_count + 2.0) * lh;

    let gb = &panel.group_bounds;
    let popup_x = ((gb.x + gb.width) as f64 - popup_w - 10.0).max(gb.x as f64);
    let popup_y = gb.y as f64 + 2.0;

    // Background + border.
    let (bg_r, bg_g, bg_b) = cairo_rgb(theme.surface_bg);
    cr.set_source_rgb(bg_r, bg_g, bg_b);
    cr.rectangle(popup_x, popup_y, popup_w, popup_h);
    let _ = cr.fill();
    let (sep_r, sep_g, sep_b) = cairo_rgb(theme.separator);
    cr.set_source_rgb(sep_r, sep_g, sep_b);
    cr.rectangle(popup_x, popup_y, popup_w, popup_h);
    cr.set_line_width(1.0);
    let _ = cr.stroke();

    let (fg_r, fg_g, fg_b) = cairo_rgb(theme.foreground);
    let (accent_r, accent_g, accent_b) = cairo_rgb(theme.accent_bg);
    let (btn_bg_r, btn_bg_g, btn_bg_b) = cairo_rgb(theme.background);

    // Content origin — 1 cell inside the borders.
    let content_x = popup_x + cw;
    let content_y = popup_y + lh;

    // Helpers.
    let paint_label = |layout: &pango::Layout, text: &str, px: f64, py: f64| {
        layout.set_text(text);
        cr.move_to(px, py);
        pcfn::show_layout(cr, layout);
    };

    let paint_toggle = |col: u16, row: u16, width: u16, label: &str, active: bool| {
        let bx = content_x + col as f64 * cw;
        let by = content_y + row as f64 * lh;
        let bw = width as f64 * cw;
        if active {
            cr.set_source_rgb(accent_r, accent_g, accent_b);
            cr.rectangle(bx, by, bw, lh);
            let _ = cr.fill();
            cr.set_source_rgb(btn_bg_r, btn_bg_g, btn_bg_b);
        } else {
            cr.set_source_rgb(sep_r, sep_g, sep_b);
            cr.rectangle(bx, by, bw, lh);
            cr.set_line_width(0.5);
            let _ = cr.stroke();
            cr.set_source_rgb(fg_r, fg_g, fg_b);
        }
        layout.set_text(label);
        let (tw, _) = layout.pixel_size();
        cr.move_to(bx + (bw - tw as f64) / 2.0, by);
        pcfn::show_layout(cr, layout);
    };

    let paint_glyph = |col: u16, row: u16, width: u16, label: &str, active: bool| {
        let bx = content_x + col as f64 * cw;
        let by = content_y + row as f64 * lh;
        let bw = width as f64 * cw;
        if active {
            cr.set_source_rgb(accent_r, accent_g, accent_b);
            cr.rectangle(bx, by, bw, lh);
            let _ = cr.fill();
            cr.set_source_rgb(btn_bg_r, btn_bg_g, btn_bg_b);
        } else {
            cr.set_source_rgb(fg_r, fg_g, fg_b);
        }
        layout.set_text(label);
        let (tw, _) = layout.pixel_size();
        cr.move_to(bx + (bw - tw as f64) / 2.0, by);
        pcfn::show_layout(cr, layout);
    };

    let paint_input = |col: u16,
                       row: u16,
                       width: u16,
                       text: &str,
                       is_focused: bool,
                       cursor: usize,
                       sel_anchor: Option<usize>| {
        let bx = content_x + col as f64 * cw;
        let by = content_y + row as f64 * lh;
        let bw = width as f64 * cw;
        // Input background + thin border.
        cr.set_source_rgb(btn_bg_r, btn_bg_g, btn_bg_b);
        cr.rectangle(bx, by, bw, lh);
        let _ = cr.fill();
        cr.set_source_rgb(sep_r, sep_g, sep_b);
        cr.rectangle(bx, by, bw, lh);
        cr.set_line_width(0.5);
        let _ = cr.stroke();
        // Text.
        cr.set_source_rgb(fg_r, fg_g, fg_b);
        paint_label(layout, text, bx + 4.0, by);
        if !is_focused {
            return;
        }
        // Selection highlight.
        if let Some(anchor) = sel_anchor {
            let s = anchor.min(cursor);
            let e = anchor.max(cursor);
            if s != e {
                let s_prefix = &text[..text
                    .char_indices()
                    .nth(s)
                    .map(|(i, _)| i)
                    .unwrap_or(text.len())];
                let e_prefix = &text[..text
                    .char_indices()
                    .nth(e)
                    .map(|(i, _)| i)
                    .unwrap_or(text.len())];
                layout.set_text(s_prefix);
                let (sx, _) = layout.pixel_size();
                layout.set_text(e_prefix);
                let (ex, _) = layout.pixel_size();
                let (sr, sg, sb) = cairo_rgb(theme.selection_bg);
                cr.set_source_rgba(sr, sg, sb, 0.5);
                cr.rectangle(bx + 4.0 + sx as f64, by, (ex - sx) as f64, lh);
                let _ = cr.fill();
            }
        }
        // Cursor — 2-px-wide vertical bar.
        let prefix = &text[..text
            .char_indices()
            .nth(cursor)
            .map(|(i, _)| i)
            .unwrap_or(text.len())];
        layout.set_text(prefix);
        let (cpx, _) = layout.pixel_size();
        cr.set_source_rgb(fg_r, fg_g, fg_b);
        cr.rectangle(bx + 4.0 + cpx as f64, by + 2.0, 2.0, lh - 4.0);
        let _ = cr.fill();
    };

    // Walk hit_regions and paint each one.
    let mut regex_end_col: Option<u16> = None;
    let mut prev_match_col: Option<u16> = None;

    for (region, target) in &panel.hit_regions {
        match target {
            T::Chevron => {
                let chevron = if panel.show_replace { "▼" } else { "▶" };
                cr.set_source_rgb(fg_r, fg_g, fg_b);
                let px = content_x + region.col as f64 * cw;
                let py = content_y + region.row as f64 * lh;
                paint_label(layout, chevron, px, py);
            }
            T::FindInput(_) => {
                paint_input(
                    region.col,
                    region.row,
                    region.width,
                    &panel.query,
                    panel.focus == 0,
                    panel.cursor,
                    panel.sel_anchor,
                );
            }
            T::ReplaceInput(_) => {
                paint_input(
                    region.col,
                    region.row,
                    region.width,
                    &panel.replacement,
                    panel.focus == 1,
                    panel.cursor,
                    panel.sel_anchor,
                );
            }
            T::ToggleCase => {
                paint_toggle(
                    region.col,
                    region.row,
                    region.width,
                    "Aa",
                    panel.case_sensitive,
                );
            }
            T::ToggleWholeWord => {
                paint_toggle(region.col, region.row, region.width, "ab", panel.whole_word);
            }
            T::ToggleRegex => {
                paint_toggle(region.col, region.row, region.width, ".*", panel.use_regex);
                regex_end_col = Some(region.col + region.width);
            }
            T::PrevMatch => {
                paint_glyph(region.col, region.row, region.width, "\u{2191}", false);
                prev_match_col.get_or_insert(region.col);
            }
            T::NextMatch => {
                paint_glyph(region.col, region.row, region.width, "\u{2193}", false);
            }
            T::ToggleInSelection => {
                // ASCII-compat `≡` (U+2261) directly — Nerd Font
                // subset bundled with GTK doesn't include the
                // nf-cod-selection glyph at U+EB54.
                paint_glyph(
                    region.col,
                    region.row,
                    region.width,
                    "\u{2261}",
                    panel.in_selection,
                );
            }
            T::Close => {
                paint_glyph(region.col, region.row, region.width, "\u{00d7}", false);
            }
            T::TogglePreserveCase => {
                paint_toggle(
                    region.col,
                    region.row,
                    region.width,
                    "AB",
                    panel.preserve_case,
                );
            }
            T::ReplaceCurrent => {
                paint_glyph(
                    region.col,
                    region.row,
                    region.width,
                    &panel.replace_one_glyph,
                    false,
                );
            }
            T::ReplaceAll => {
                paint_glyph(
                    region.col,
                    region.row,
                    region.width,
                    &panel.replace_all_glyph,
                    false,
                );
            }
        }
    }

    // Match count text between the regex toggle and PrevMatch (not a
    // hit region; positions derived from neighbours — same trick
    // TUI uses).
    if let (Some(start_col), Some(end_col)) = (regex_end_col, prev_match_col) {
        let info_col = start_col + 1; // 1-cell gap after regex toggle
        if end_col > info_col + 1 {
            let px = content_x + info_col as f64 * cw;
            let py = content_y;
            cr.set_source_rgb(fg_r, fg_g, fg_b);
            paint_label(layout, &panel.match_info, px, py);
        }
    }
}
