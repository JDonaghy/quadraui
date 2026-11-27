//! Cell painter for [`MessageList`], written against [`CellSurface`].
//!
//! Walks `rows[scroll_top..]` one row per cell line, filling each row with
//! the panel background and writing its text from `indent` cells in. A row
//! with `spans` paints each span in its own colour and weight (a span with
//! no `fg` takes the row's); a row without paints `text` in the row's `fg`.
//! `scale` has no meaning on a cell grid and is ignored.

use crate::cell_surface::{solid_style, CellStyle, CellSurface};
use crate::event::Rect;
use crate::primitives::message_list::MessageList;
use crate::types::Color;

/// Paints `list` into `area` over `panel_bg`. Rows past `area`'s height
/// are not painted, and cells below the last row are left untouched.
pub(crate) fn paint(cells: &mut dyn CellSurface, area: Rect, list: &MessageList, panel_bg: Color) {
    let (ax, ay) = (area.x as u16, area.y as u16);
    let (aw, ah) = (area.width as u16, area.height as u16);
    if aw == 0 || ah == 0 {
        return;
    }
    for (i, row) in list.rows.iter().skip(list.scroll_top).enumerate() {
        if i >= ah as usize {
            break;
        }
        let y = ay + i as u16;
        let row_style = solid_style(row.fg, panel_bg);
        for x in ax..ax + aw {
            cells.put_char(x, y, ' ', row_style);
        }
        let start_col = row.indent.round() as u16;
        let mut col = start_col;
        if row.spans.is_empty() {
            for ch in row.text.chars() {
                let cx = ax + col;
                if cx >= ax + aw {
                    break;
                }
                cells.put_char(cx, y, ch, row_style);
                col += 1;
            }
            continue;
        }
        'spans: for span in &row.spans {
            let span_fg = span.fg.unwrap_or(row.fg);
            let style = CellStyle {
                fg: Some(span_fg),
                bg: Some(panel_bg),
                bold: span.bold,
                italic: span.italic,
                underline: span.underline.then_some(span_fg),
                ..CellStyle::default()
            };
            for ch in span.text.chars() {
                let cx = ax + col;
                if cx >= ax + aw {
                    break 'spans;
                }
                cells.put_char(cx, y, ch, style);
                col += 1;
            }
        }
    }
}
