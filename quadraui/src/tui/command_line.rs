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

/// Paint `cmd` into `buf` at `area` and return the [`CommandLineLayout`]
/// used to place its glyphs — the same value `tui_command_line_layout`
/// would compute, handed back so callers (and tests) never have to
/// re-derive it and risk it drifting from what was actually painted
/// (issue #705 review: the paint/click round-trip test below reads this
/// back instead of asserting a formula in isolation).
pub fn draw_command_line(
    buf: &mut Buffer,
    area: Rect,
    cmd: &CommandLine,
    theme: &Theme,
) -> CommandLineLayout {
    let layout = tui_command_line_layout(cmd, area);
    let fg = ratatui_color(theme.command_line_fg);
    let bg = ratatui_color(theme.command_line_bg);

    for x in area.x..area.x + area.width {
        set_cell(buf, x, area.y, ' ', fg, bg);
    }

    if cmd.text.is_empty() {
        return layout;
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

    layout
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

    fn cell_char(buf: &Buffer, x: u16, y: u16) -> char {
        buf[(x, y)].symbol().chars().next().unwrap_or(' ')
    }

    /// Paint/click round-trip (`docs/TESTING.md` coverage-taxonomy row 1):
    /// paint via the real `draw_command_line` rasteriser, read back the
    /// actual painted cell character from the `Buffer`, then hit_test
    /// that exact painted coordinate and assert it resolves to that
    /// character's byte offset. Unlike a test that calls
    /// `tui_command_line_layout` in isolation and checks formula-predicted
    /// x-positions, this catches `draw_command_line`'s glyph placement
    /// drifting away from `CommandLine::layout`'s column formula (e.g. a
    /// future prompt gutter added to one but not the other) — see #705
    /// review.
    ///
    /// #505: a LOCAL/ABSOLUTE mixup is invisible at `area.x == 0`, so this
    /// is exercised at a nonzero origin too — `hit_test` should ignore
    /// clicks left of the bar and map columns starting at `area.x`, not
    /// `0`.
    #[test]
    fn tui_command_line_paint_and_click_round_trip_at_nonzero_origin() {
        let area = Rect::new(5, 3, 20, 1);
        let mut buf = Buffer::empty(area);
        let theme = Theme::default();
        let cmd = CommandLine {
            id: WidgetId::new("cmdline"),
            text: ":wq".into(),
            cursor_offset: None,
            right_align: false,
        };

        let layout = draw_command_line(&mut buf, area, &cmd, &theme);

        // Column 0 (':') is actually painted at the absolute origin.
        assert_eq!(cell_char(&buf, 5, 3), ':');
        // Column 1 ('w') is actually painted one cell to the right.
        assert_eq!(cell_char(&buf, 6, 3), 'w');

        // hit_test at the real painted position of 'w' (a click lands
        // somewhere inside the cell, not necessarily at its left edge)
        // must resolve to 'w's byte offset (1) in ":wq".
        assert_eq!(layout.hit_test(6.5), 1);

        // A click left of the bar clamps to the first column's byte offset.
        assert_eq!(layout.hit_test(0.0), 0);
        // Column 0 starts at x == area.x == 5.
        assert_eq!(layout.hit_test(5.0), 0);
    }
}
