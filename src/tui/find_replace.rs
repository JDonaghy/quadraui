//! TUI rasteriser for [`crate::FindReplacePanel`].
//!
//! Walks `panel.hit_regions` (the shared cross-backend layout
//! source-of-truth from
//! [`crate::compute_find_replace_hit_regions`]). Painting and
//! hit-test then derive from the same `FrHitRegion` list, so column
//! drift bugs (the same class fixed for debug toolbar + breadcrumb)
//! can't recur on this overlay.
//!
//! `editor_left` is the absolute screen column of the editor area's
//! left edge (after activity bar + sidebar). `panel.group_bounds.x/y`
//! are content-relative; the overlay anchors at the top-right of the
//! active editor group.
//!
//! Painting that the hit-region list doesn't directly cover —
//! borders, the match-count text (a non-clickable status string), and
//! the focused field's cursor + selection — is layered in around the
//! region-driven dispatch.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::{ratatui_color, set_cell};
use crate::primitives::find_replace::{FindReplaceClickTarget, FindReplacePanel};
use crate::theme::Theme;

pub fn draw_find_replace(
    buf: &mut Buffer,
    area: Rect,
    panel: &FindReplacePanel,
    theme: &Theme,
    editor_left: u16,
) {
    use FindReplaceClickTarget as T;

    let bg = ratatui_color(theme.surface_bg);
    let fg = ratatui_color(theme.surface_fg);
    let border_fg = ratatui_color(theme.border_fg);
    let accent_bg = ratatui_color(theme.accent_bg);
    let sel_bg = ratatui_color(theme.selection_bg);
    let btn_sel_bg = ratatui_color(theme.selected_bg);

    let panel_w: u16 = panel.panel_width.min(area.width.saturating_sub(2));
    let row_count: u16 = if panel.show_replace { 2 } else { 1 };
    let panel_h: u16 = row_count + 2; // +2 for top/bottom borders

    // Position: top-right of active editor group.
    let gb = &panel.group_bounds;
    let gb_right = editor_left + gb.x as u16 + gb.width as u16;
    let x = gb_right.saturating_sub(panel_w + 1).max(editor_left);
    let y = (gb.y as u16).max(1);
    if panel_w == 0 || panel_h == 0 {
        return;
    }

    // Clear background.
    for row in y..y + panel_h {
        for col in x..x + panel_w {
            if col < area.width && row < area.height {
                set_cell(buf, col, row, ' ', fg, bg);
            }
        }
    }

    // Borders.
    for col in x..x + panel_w {
        set_cell(buf, col, y, '─', border_fg, bg);
    }
    set_cell(buf, x, y, '┌', border_fg, bg);
    if x + panel_w > 0 {
        set_cell(buf, x + panel_w - 1, y, '┐', border_fg, bg);
    }
    let bot = y + panel_h - 1;
    for col in x..x + panel_w {
        set_cell(buf, col, bot, '─', border_fg, bg);
    }
    set_cell(buf, x, bot, '└', border_fg, bg);
    if x + panel_w > 0 {
        set_cell(buf, x + panel_w - 1, bot, '┘', border_fg, bg);
    }
    for row in y + 1..bot {
        set_cell(buf, x, row, '│', border_fg, bg);
        if x + panel_w > 0 {
            set_cell(buf, x + panel_w - 1, row, '│', border_fg, bg);
        }
    }

    // Content origin: 1 cell inside the borders.
    let content_x = x + 1;
    let content_y = y + 1;
    let right_edge = x + panel_w - 1;

    // Helper: paint a string at a region (col is content-relative).
    let paint_at = |buf: &mut Buffer,
                    region_col: u16,
                    region_row: u16,
                    text: &str,
                    cell_fg: ratatui::style::Color,
                    cell_bg: ratatui::style::Color,
                    width: u16| {
        let row_y = content_y + region_row;
        for (i, ch) in text.chars().enumerate() {
            let cx = content_x + region_col + i as u16;
            if cx >= content_x + width.saturating_add(region_col) {
                break;
            }
            if cx < right_edge {
                set_cell(buf, cx, row_y, ch, cell_fg, cell_bg);
            }
        }
    };

    // Helper: paint a single character at a region.
    let paint_char = |buf: &mut Buffer,
                      region_col: u16,
                      region_row: u16,
                      ch: char,
                      cell_fg: ratatui::style::Color,
                      cell_bg: ratatui::style::Color| {
        let row_y = content_y + region_row;
        let cx = content_x + region_col;
        if cx < right_edge {
            set_cell(buf, cx, row_y, ch, cell_fg, cell_bg);
        }
    };

    // Helper: paint a focused input field's text + cursor + selection.
    let paint_input = |buf: &mut Buffer,
                       text: &str,
                       region_col: u16,
                       region_row: u16,
                       region_w: u16,
                       focused: bool,
                       cursor: usize,
                       sel_anchor: Option<usize>| {
        let row_y = content_y + region_row;
        // Body text.
        for (i, ch) in text.chars().enumerate() {
            let cx = content_x + region_col + i as u16;
            if cx >= content_x + region_col + region_w {
                break;
            }
            if cx < right_edge {
                set_cell(buf, cx, row_y, ch, fg, bg);
            }
        }
        if !focused {
            return;
        }
        // Selection (drawn before cursor so cursor wins on overlap).
        if let Some(anchor) = sel_anchor {
            let s = anchor.min(cursor) as u16;
            let e = anchor.max(cursor) as u16;
            for i in s..e {
                let cx = content_x + region_col + i;
                if cx >= content_x + region_col + region_w {
                    break;
                }
                if cx < right_edge {
                    let ch = text.chars().nth(i as usize).unwrap_or(' ');
                    set_cell(buf, cx, row_y, ch, fg, sel_bg);
                }
            }
        }
        // Cursor block.
        let cursor_col = content_x + region_col + cursor as u16;
        if cursor_col < content_x + region_col + region_w && cursor_col < right_edge {
            let ch = text.chars().nth(cursor).unwrap_or(' ');
            set_cell(buf, cursor_col, row_y, ch, bg, fg);
        }
    };

    // Walk hit regions, painting per target. The match count is NOT
    // a hit region (it's status text, non-clickable), so we paint it
    // separately by reading the gap between the last toggle (.*) and
    // PrevMatch (the first nav button).
    let mut regex_end_col: Option<u16> = None;
    let mut prev_match_col: Option<u16> = None;

    for (region, target) in &panel.hit_regions {
        match target {
            T::Chevron => {
                let chevron = if panel.show_replace { '▼' } else { '▶' };
                paint_char(buf, region.col, region.row, chevron, fg, bg);
            }
            T::FindInput(_) => {
                paint_input(
                    buf,
                    &panel.query,
                    region.col,
                    region.row,
                    region.width,
                    panel.focus == 0,
                    panel.cursor,
                    panel.sel_anchor,
                );
            }
            T::ReplaceInput(_) => {
                paint_input(
                    buf,
                    &panel.replacement,
                    region.col,
                    region.row,
                    region.width,
                    panel.focus == 1,
                    panel.cursor,
                    panel.sel_anchor,
                );
            }
            T::ToggleCase => {
                let (t_fg, t_bg) = if panel.case_sensitive {
                    (bg, accent_bg)
                } else {
                    (fg, bg)
                };
                paint_at(buf, region.col, region.row, "Aa", t_fg, t_bg, region.width);
            }
            T::ToggleWholeWord => {
                let (t_fg, t_bg) = if panel.whole_word {
                    (bg, accent_bg)
                } else {
                    (fg, bg)
                };
                paint_at(buf, region.col, region.row, "ab", t_fg, t_bg, region.width);
            }
            T::ToggleRegex => {
                let (t_fg, t_bg) = if panel.use_regex {
                    (bg, accent_bg)
                } else {
                    (fg, bg)
                };
                paint_at(buf, region.col, region.row, ".*", t_fg, t_bg, region.width);
                regex_end_col = Some(region.col + region.width);
            }
            T::PrevMatch => {
                paint_char(buf, region.col, region.row, '↑', fg, bg);
                prev_match_col.get_or_insert(region.col);
            }
            T::NextMatch => {
                paint_char(buf, region.col, region.row, '↓', fg, bg);
            }
            T::ToggleInSelection => {
                let (n_fg, n_bg) = if panel.in_selection {
                    (bg, accent_bg)
                } else {
                    (fg, bg)
                };
                paint_char(buf, region.col, region.row, '≡', n_fg, n_bg);
            }
            T::Close => {
                paint_char(buf, region.col, region.row, '×', fg, bg);
            }
            T::TogglePreserveCase => {
                let (ab_fg, ab_bg) = if panel.preserve_case {
                    (bg, accent_bg)
                } else {
                    (fg, bg)
                };
                paint_at(
                    buf,
                    region.col,
                    region.row,
                    "AB",
                    ab_fg,
                    ab_bg,
                    region.width,
                );
            }
            T::ReplaceCurrent => {
                paint_at(
                    buf,
                    region.col,
                    region.row,
                    &panel.replace_one_glyph,
                    fg,
                    btn_sel_bg,
                    region.width,
                );
            }
            T::ReplaceAll => {
                paint_at(
                    buf,
                    region.col,
                    region.row,
                    &panel.replace_all_glyph,
                    fg,
                    btn_sel_bg,
                    region.width,
                );
            }
        }
    }

    // Match count text — sits between the regex toggle and the
    // PrevMatch arrow, in find row (row 0). Non-clickable, so it has
    // no FrHitRegion entry; positions are derived from neighbours.
    if let (Some(start_col), Some(end_col)) = (regex_end_col, prev_match_col) {
        // 1-cell gap on the left after the regex toggle.
        let info_col = start_col + 1;
        let info_w = end_col.saturating_sub(info_col + 1); // 1-cell gap before PrevMatch
        if info_w > 0 {
            paint_at(buf, info_col, 0, &panel.match_info, fg, bg, info_w);
        }
    }
}
