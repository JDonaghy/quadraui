//! TUI rasteriser for [`crate::TextDisplay`].
//!
//! Per D6: this function asks the primitive for a
//! [`crate::TextDisplayLayout`] using a uniform 1-cell-per-line
//! measurer (TUI rows are always 1 cell tall) and paints the resolved
//! `visible_lines` verbatim.
//!
//! Each line's spans render with their own `fg` / `bg` (falling back
//! to the theme defaults). Optional `timestamp` prefix is rendered in
//! [`Theme::muted_fg`]. Per-line `decoration` (`Error`/`Warning`/
//! `Muted`) overrides the default fg for the entire line.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::{ratatui_color, set_cell};
use crate::primitives::text_display::{TextDisplay, TextDisplayLineMeasure};
use crate::theme::Theme;
use crate::types::Decoration;

/// Draw a [`TextDisplay`] into `area` on `buf`.
///
/// # Visual contract
///
/// - Background: filled with [`Theme::background`].
/// - Per-line decoration → default fg: `Error → error_fg`,
///   `Warning → warning_fg`, `Muted → muted_fg`, others →
///   [`Theme::foreground`].
/// - Per-span overrides: `span.fg` / `span.bg` win over the per-line
///   default.
/// - Timestamp prefix (when present): rendered in
///   [`Theme::muted_fg`] before the spans, separated by a single
///   space.
pub fn draw_text_display(buf: &mut Buffer, area: Rect, display: &TextDisplay, theme: &Theme) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let bg = ratatui_color(theme.background);
    let fg = ratatui_color(theme.foreground);
    let muted = ratatui_color(theme.muted_fg);
    let error = ratatui_color(theme.error_fg);
    let warning = ratatui_color(theme.warning_fg);

    // Fill the area background.
    for y in area.y..area.y + area.height {
        for x in area.x..area.x + area.width {
            set_cell(buf, x, y, ' ', fg, bg);
        }
    }

    let layout = display.layout(area.width as f32, area.height as f32, |_| {
        TextDisplayLineMeasure::new(1.0)
    });

    for vis in &layout.visible_lines {
        let line = &display.lines[vis.line_idx];
        let row_y = area.y + vis.bounds.y.round() as u16;
        if row_y >= area.y + area.height {
            break;
        }

        let line_fg = match line.decoration {
            Decoration::Error => error,
            Decoration::Warning => warning,
            Decoration::Muted => muted,
            _ => fg,
        };

        let mut col: u16 = 0;

        // Timestamp prefix (if present).
        if let Some(ref ts) = line.timestamp {
            for ch in ts.chars() {
                if col >= area.width {
                    break;
                }
                set_cell(buf, area.x + col, row_y, ch, muted, bg);
                col += 1;
            }
            if col < area.width {
                set_cell(buf, area.x + col, row_y, ' ', muted, bg);
                col += 1;
            }
        }

        // Spans.
        for span in &line.spans {
            let span_fg = span.fg.map(ratatui_color).unwrap_or(line_fg);
            let span_bg = span.bg.map(ratatui_color).unwrap_or(bg);
            for ch in span.text.chars() {
                if col >= area.width {
                    break;
                }
                set_cell(buf, area.x + col, row_y, ch, span_fg, span_bg);
                col += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::text_display::{TextDisplay, TextDisplayLine};
    use crate::types::{Color, StyledSpan, WidgetId};

    fn line(text: &str) -> TextDisplayLine {
        TextDisplayLine {
            spans: vec![StyledSpan::plain(text)],
            decoration: Decoration::Normal,
            timestamp: None,
        }
    }

    fn cell_char(buf: &Buffer, x: u16, y: u16) -> char {
        buf[(x, y)].symbol().chars().next().unwrap_or(' ')
    }

    #[test]
    fn paints_lines_top_to_bottom() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 5));
        let display = TextDisplay {
            id: WidgetId::new("td"),
            lines: vec![line("alpha"), line("beta"), line("gamma")],
            scroll_offset: 0,
            auto_scroll: false,
            max_lines: 0,
            has_focus: false,
        };
        draw_text_display(
            &mut buf,
            Rect::new(0, 0, 20, 5),
            &display,
            &Theme::default(),
        );
        let row0: String = (0..5).map(|x| cell_char(&buf, x, 0)).collect();
        let row1: String = (0..4).map(|x| cell_char(&buf, x, 1)).collect();
        let row2: String = (0..5).map(|x| cell_char(&buf, x, 2)).collect();
        assert_eq!(row0, "alpha");
        assert_eq!(row1, "beta");
        assert_eq!(row2, "gamma");
    }

    #[test]
    fn span_fg_override_wins() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 5));
        let display = TextDisplay {
            id: WidgetId::new("td"),
            lines: vec![TextDisplayLine {
                spans: vec![
                    StyledSpan {
                        text: "key:".into(),
                        fg: Some(Color::rgb(99, 0, 0)),
                        bg: None,
                        bold: false,
                        italic: false,
                        underline: false,
                    },
                    StyledSpan::plain(" value"),
                ],
                decoration: Decoration::Normal,
                timestamp: None,
            }],
            scroll_offset: 0,
            auto_scroll: false,
            max_lines: 0,
            has_focus: false,
        };
        draw_text_display(
            &mut buf,
            Rect::new(0, 0, 20, 5),
            &display,
            &Theme::default(),
        );
        // 'k' at col 0 should be in (99, 0, 0).
        let fg = buf[(0u16, 0u16)].fg;
        assert_eq!(fg, ratatui::style::Color::Rgb(99, 0, 0));
    }

    #[test]
    fn auto_scroll_pins_to_bottom() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 3));
        let display = TextDisplay {
            id: WidgetId::new("td"),
            lines: (0..10).map(|i| line(&format!("line{i}"))).collect(),
            scroll_offset: 0,
            auto_scroll: true,
            max_lines: 0,
            has_focus: false,
        };
        draw_text_display(
            &mut buf,
            Rect::new(0, 0, 20, 3),
            &display,
            &Theme::default(),
        );
        // Last 3 lines visible (line7, line8, line9).
        let row0: String = (0..5).map(|x| cell_char(&buf, x, 0)).collect();
        let row1: String = (0..5).map(|x| cell_char(&buf, x, 1)).collect();
        let row2: String = (0..5).map(|x| cell_char(&buf, x, 2)).collect();
        assert_eq!(row0, "line7");
        assert_eq!(row1, "line8");
        assert_eq!(row2, "line9");
    }

    #[test]
    fn zero_size_is_a_no_op() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 5));
        let display = TextDisplay {
            id: WidgetId::new("td"),
            lines: vec![line("x")],
            scroll_offset: 0,
            auto_scroll: false,
            max_lines: 0,
            has_focus: false,
        };
        draw_text_display(&mut buf, Rect::new(0, 0, 0, 5), &display, &Theme::default());
        assert_eq!(cell_char(&buf, 0, 0), ' ');
    }
}
