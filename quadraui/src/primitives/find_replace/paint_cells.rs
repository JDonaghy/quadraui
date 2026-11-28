//! Cell painter for [`FindReplacePanel`], written against [`CellSurface`].
//!
//! Paints a `┌─┐│└─┘` box filling the layout's area, then walks
//! `panel.hit_regions` — the list [`FindReplaceLayout::hit_test`] reads —
//! so painting and hit-testing derive from the same regions. What the
//! region list doesn't cover directly — the border, the match-count text
//! (a non-clickable status string) and the focused field's cursor and
//! selection — is layered in around the region-driven dispatch.

use crate::cell_surface::{solid_style, stroke_square_frame, CellSurface};
use crate::primitives::find_replace::{
    FindReplaceClickTarget, FindReplaceLayout, FindReplacePanel,
};
use crate::theme::Theme;
use crate::types::Color;

/// The panel's content corner and right border column, in cells.
struct Frame {
    content_x: u16,
    content_y: u16,
    /// The right border's column; nothing inside paints at or past it.
    right_edge: u16,
}

impl Frame {
    /// Put `ch` at content-relative `(col, row)` unless that lands on or
    /// past the right border.
    fn put(&self, cells: &mut dyn CellSurface, col: u16, row: u16, ch: char, fg: Color, bg: Color) {
        let cx = self.content_x + col;
        if cx < self.right_edge {
            cells.put_char(cx, self.content_y + row, ch, solid_style(fg, bg));
        }
    }

    /// Put `text` from content-relative `(col, row)`, at most `width`
    /// cells of it.
    #[allow(clippy::too_many_arguments)]
    fn put_str(
        &self,
        cells: &mut dyn CellSurface,
        col: u16,
        row: u16,
        text: &str,
        fg: Color,
        bg: Color,
        width: u16,
    ) {
        for (i, ch) in text.chars().take(width as usize).enumerate() {
            self.put(cells, col + i as u16, row, ch, fg, bg);
        }
    }
}

/// Paints `panel` over `layout`, which [`crate::primitive::Primitive::layout`]
/// resolved at one cell per row.
pub(crate) fn paint(
    cells: &mut dyn CellSurface,
    panel: &FindReplacePanel,
    layout: &FindReplaceLayout,
    theme: &Theme,
) {
    use FindReplaceClickTarget as T;

    let area = layout.area;
    let x = area.x.max(0.0) as u16;
    let y = area.y.max(0.0) as u16;
    let panel_w = area.width.max(0.0) as u16;
    let panel_h = area.height.max(0.0) as u16;
    if panel_w == 0 || panel_h == 0 {
        return;
    }

    let bg = theme.surface_bg;
    let fg = theme.surface_fg;
    let accent_bg = theme.accent_bg;
    let sel_bg = theme.selection_bg;
    let btn_sel_bg = theme.selected_bg;

    let (cols, rows) = cells.size();
    for row in y..(y + panel_h).min(rows) {
        for col in x..(x + panel_w).min(cols) {
            cells.put_char(col, row, ' ', solid_style(fg, bg));
        }
    }
    stroke_square_frame(cells, area, solid_style(theme.border_fg, bg));

    let frame = Frame {
        content_x: layout.content_origin.0.max(0.0) as u16,
        content_y: layout.content_origin.1.max(0.0) as u16,
        right_edge: x + panel_w - 1,
    };

    // A field's text, then — only while focused — its selection and the
    // block cursor (drawn last so it wins over the selection).
    let paint_input =
        |cells: &mut dyn CellSurface, text: &str, col: u16, row: u16, width: u16, focused: bool| {
            frame.put_str(cells, col, row, text, fg, bg, width);
            if !focused {
                return;
            }
            let cursor = panel.cursor;
            if let Some(anchor) = panel.sel_anchor {
                let (s, e) = (anchor.min(cursor), anchor.max(cursor));
                for i in s..e.min(width as usize) {
                    let ch = text.chars().nth(i).unwrap_or(' ');
                    frame.put(cells, col + i as u16, row, ch, fg, sel_bg);
                }
            }
            if cursor < width as usize {
                let ch = text.chars().nth(cursor).unwrap_or(' ');
                frame.put(cells, col + cursor as u16, row, ch, bg, fg);
            }
        };

    let toggle_colors = |on: bool| if on { (bg, accent_bg) } else { (fg, bg) };

    // The match count is not a hit region (it's status text), so it is
    // painted in the gap between the last find-row toggle (.*) and
    // PrevMatch, the first nav button.
    let mut info_start_col: Option<u16> = None;
    let mut prev_match_col: Option<u16> = None;

    for (region, target) in panel.regions() {
        let (col, row, width) = (region.col, region.row, region.width);
        // The count follows the find input and whichever find-row toggles
        // the panel shows.
        if matches!(
            target,
            T::FindInput(_) | T::ToggleCase | T::ToggleWholeWord | T::ToggleRegex
        ) {
            info_start_col = Some(col + width);
        }
        match target {
            T::Chevron => {
                let chevron = if panel.show_replace { '▼' } else { '▶' };
                frame.put(cells, col, row, chevron, fg, bg);
            }
            T::FindInput(_) => {
                paint_input(cells, &panel.query, col, row, width, panel.focus == 0);
            }
            T::ReplaceInput(_) => {
                paint_input(cells, &panel.replacement, col, row, width, panel.focus == 1);
            }
            T::ToggleCase => {
                let (t_fg, t_bg) = toggle_colors(panel.case_sensitive);
                frame.put_str(cells, col, row, "Aa", t_fg, t_bg, width);
            }
            T::ToggleWholeWord => {
                let (t_fg, t_bg) = toggle_colors(panel.whole_word);
                frame.put_str(cells, col, row, "ab", t_fg, t_bg, width);
            }
            T::ToggleRegex => {
                let (t_fg, t_bg) = toggle_colors(panel.use_regex);
                frame.put_str(cells, col, row, ".*", t_fg, t_bg, width);
            }
            T::PrevMatch => {
                frame.put(cells, col, row, '↑', fg, bg);
                prev_match_col.get_or_insert(col);
            }
            T::NextMatch => frame.put(cells, col, row, '↓', fg, bg),
            T::ToggleInSelection => {
                let (t_fg, t_bg) = toggle_colors(panel.in_selection);
                frame.put(cells, col, row, '≡', t_fg, t_bg);
            }
            T::Close => frame.put(cells, col, row, '×', fg, bg),
            T::TogglePreserveCase => {
                let (t_fg, t_bg) = toggle_colors(panel.preserve_case);
                frame.put_str(cells, col, row, "AB", t_fg, t_bg, width);
            }
            T::ReplaceCurrent => {
                frame.put_str(
                    cells,
                    col,
                    row,
                    &panel.replace_one_glyph,
                    fg,
                    btn_sel_bg,
                    width,
                );
            }
            T::ReplaceAll => {
                frame.put_str(
                    cells,
                    col,
                    row,
                    &panel.replace_all_glyph,
                    fg,
                    btn_sel_bg,
                    width,
                );
            }
        }
    }

    if let (Some(start_col), Some(end_col)) = (info_start_col, prev_match_col) {
        // One-cell gaps after the last toggle and before PrevMatch.
        let info_col = start_col + 1;
        let info_w = end_col.saturating_sub(info_col + 1);
        if info_w > 0 {
            frame.put_str(cells, info_col, 0, &panel.match_info, fg, bg, info_w);
        }
    }
}
