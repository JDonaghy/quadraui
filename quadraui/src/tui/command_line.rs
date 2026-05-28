//! TUI rasteriser for [`crate::primitives::command_line::CommandLine`].

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::{ratatui_color, set_cell};
use crate::primitives::command_line::CommandLine;
use crate::theme::Theme;

pub fn draw_command_line(buf: &mut Buffer, area: Rect, cmd: &CommandLine, theme: &Theme) {
    let fg = ratatui_color(theme.command_line_fg);
    let bg = ratatui_color(theme.command_line_bg);

    for x in area.x..area.x + area.width {
        set_cell(buf, x, area.y, ' ', fg, bg);
    }

    if cmd.text.is_empty() {
        return;
    }

    if cmd.right_align {
        let chars: Vec<char> = cmd.text.chars().collect();
        let len = chars.len() as u16;
        if len <= area.width {
            for (x, &ch) in (area.x + area.width - len..).zip(chars.iter()) {
                if x >= area.x + area.width {
                    break;
                }
                set_cell(buf, x, area.y, ch, fg, bg);
            }
        }
    } else {
        for (x, ch) in (area.x..).zip(cmd.text.chars()) {
            if x >= area.x + area.width {
                break;
            }
            set_cell(buf, x, area.y, ch, fg, bg);
        }
    }

    if let Some(offset) = cmd.cursor_offset {
        let cursor_col = cmd.text[..offset.min(cmd.text.len())].chars().count() as u16;
        let cx = area.x + cursor_col.min(area.width.saturating_sub(1));
        if cx < area.x + area.width {
            let cell = &mut buf[(cx, area.y)];
            let old_fg = cell.fg;
            let old_bg = cell.bg;
            cell.set_fg(old_bg).set_bg(old_fg);
        }
    }
}
