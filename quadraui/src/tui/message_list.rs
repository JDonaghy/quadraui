//! TUI rasteriser for [`crate::MessageList`].
//!
//! Walks `rows[scroll_top..]` row by row, painting each row at
//! `panel_bg` with the row's `fg` colour. Indents are in cell units —
//! the caller pre-builds the indent via [`crate::MessageRow::indent`].
//!
//! # Styled rows
//!
//! When a row's `spans` vector is **non-empty**, each span is rendered
//! in its own `fg` colour, with ratatui `Modifier::BOLD` / `ITALIC` /
//! `UNDERLINED` applied as appropriate.  Spans whose `fg` is `None`
//! fall back to `row.fg`.  The `scale` field is ignored by TUI (terminal
//! cells have no variable character size).
//!
//! When `spans` is **empty**, the rasteriser uses the flat `row.text` +
//! `row.fg` path — output is byte-for-byte identical to the pre-styled-row
//! behaviour.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Modifier;

use super::{ratatui_color, set_cell, set_cell_styled};
use crate::primitives::message_list::MessageList;
use crate::types::Color;

/// Draw a [`MessageList`] into `area`, filling each row with `panel_bg`
/// then writing the row's text at `area.x + indent` in the row's `fg`.
/// Stops once `area.height` rows have been painted (any unpainted rows
/// are left untouched — the caller fills the remainder if it wants a
/// uniform panel bg).
///
/// When a row carries a non-empty `spans` vector the rasteriser applies
/// per-span fg/bold/italic via ratatui cell modifiers.  When `spans` is
/// empty the flat `text` + `fg` path is used unchanged.
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
        // Fill the row background first.
        for x in area.x..area.x + area.width {
            set_cell(buf, x, y, ' ', fg, bg);
        }
        let start_col = row.indent.round() as u16;

        if !row.spans.is_empty() {
            // ── Styled path ─────────────────────────────────────────────
            // Iterate spans in order, computing fg + modifier per span.
            let mut col = start_col;
            'span_loop: for span in &row.spans {
                let span_fg = span.fg.map(ratatui_color).unwrap_or(fg);
                let mut modifier = Modifier::empty();
                if span.bold {
                    modifier |= Modifier::BOLD;
                }
                if span.italic {
                    modifier |= Modifier::ITALIC;
                }
                if span.underline {
                    modifier |= Modifier::UNDERLINED;
                }
                for ch in span.text.chars() {
                    let cx = area.x + col;
                    if cx >= area.x + area.width {
                        break 'span_loop;
                    }
                    set_cell_styled(buf, cx, y, ch, span_fg, bg, modifier, None);
                    col += 1;
                }
            }
        } else {
            // ── Flat path (unchanged from before spans were added) ───────
            for (j, ch) in row.text.chars().enumerate() {
                let cx = area.x + start_col + j as u16;
                if cx >= area.x + area.width {
                    break;
                }
                set_cell(buf, cx, y, ch, fg, bg);
            }
        }
    }
}
