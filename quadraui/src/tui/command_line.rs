//! TUI rasteriser for [`crate::primitives::command_line::CommandLine`].

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::{ratatui_color, set_cell};
use crate::primitives::command_line::{CommandLine, CommandLineLayout, CommandLineMeasure};
use crate::text_util::safe_prefix;
use crate::theme::Theme;

/// Compute [`CommandLineLayout`] for `cmd` in `area` — one cell per
/// character column (issue #705).
pub fn tui_command_line_layout(cmd: &CommandLine, area: Rect) -> CommandLineLayout {
    let rect = crate::event::Rect::new(
        area.x as f32,
        area.y as f32,
        area.width as f32,
        area.height as f32,
    );
    cmd.layout(rect, CommandLineMeasure::new(1.0))
}

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
        let cursor_col = safe_prefix(&cmd.text, offset).chars().count() as u16;
        let cx = area.x + cursor_col.min(area.width.saturating_sub(1));
        if cx < area.x + area.width {
            let cell = &mut buf[(cx, area.y)];
            let old_fg = cell.fg;
            let old_bg = cell.bg;
            cell.set_fg(old_bg).set_bg(old_fg);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::WidgetId;

    /// Regression for issue #503: `cursor_offset` is a host-supplied
    /// byte offset with no guarantee it lands on a char boundary — the
    /// pre-fix `cmd.text[..offset.min(cmd.text.len())]` slice panicked
    /// the moment a multibyte character sat left of the cursor.
    #[test]
    fn draw_command_line_with_multibyte_cursor_does_not_panic() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 1));
        let theme = Theme::default();

        // ":éditer" — byte 2 sits inside the 2-byte 'é' (starts at byte 1).
        let text = ":éditer";
        assert!(!text.is_char_boundary(2));
        let cmd = CommandLine {
            id: WidgetId::new("cmdline"),
            text: text.into(),
            cursor_offset: Some(2),
            right_align: false,
        };

        // Must not panic.
        draw_command_line(&mut buf, Rect::new(0, 0, 20, 1), &cmd, &theme);
    }

    /// #505: a LOCAL/ABSOLUTE mixup is invisible at `area.x == 0`, so
    /// `tui_command_line_layout`'s hit test must be exercised at a
    /// nonzero origin too — `hit_test` should ignore clicks left of the
    /// bar and map columns starting at `area.x`, not `0`.
    #[test]
    fn tui_command_line_layout_hit_test_at_nonzero_origin() {
        let cmd = CommandLine {
            id: WidgetId::new("cmdline"),
            text: ":wq".into(),
            cursor_offset: None,
            right_align: false,
        };
        let layout = tui_command_line_layout(&cmd, Rect::new(5, 3, 20, 1));
        // A click left of the bar clamps to the first column's byte offset.
        assert_eq!(layout.hit_test(0.0), 0);
        // Column 0 starts at x == area.x == 5.
        assert_eq!(layout.hit_test(5.0), 0);
        // Column 1 ('w') starts at x == 6.
        assert_eq!(layout.hit_test(6.5), 1);
    }
}
