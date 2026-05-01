//! TUI rasteriser for [`crate::MessageList`].
//!
//! Walks `rows[scroll_top..]` row by row, painting each row at
//! `panel_bg` with the row's `fg` colour. Indents are in cell units —
//! the caller pre-builds the indent via [`crate::MessageRow::indent`].

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::{ratatui_color, set_cell};
use crate::primitives::message_list::MessageList;
use crate::types::Color;

/// Draw a [`MessageList`] into `area`, filling each row with `panel_bg`
/// then writing the row's text at `area.x + indent` in the row's `fg`.
/// Stops once `area.height` rows have been painted (any unpainted rows
/// are left untouched — the caller fills the remainder if it wants a
/// uniform panel bg).
pub fn draw_message_list(buf: &mut Buffer, area: Rect, list: &MessageList, panel_bg: Color) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let bg = ratatui_color(panel_bg);
    let max_row = area.height as usize;
    for (i, row) in list.rows.iter().skip(list.scroll_top).enumerate() {
        if i >= max_row {
            break;
        }
        let y = area.y + i as u16;
        let fg = ratatui_color(row.fg);
        for x in area.x..area.x + area.width {
            set_cell(buf, x, y, ' ', fg, bg);
        }
        let start_col = row.indent.round() as u16;
        for (j, ch) in row.text.chars().enumerate() {
            let cx = area.x + start_col + j as u16;
            if cx >= area.x + area.width {
                break;
            }
            set_cell(buf, cx, y, ch, fg, bg);
        }
    }
}
