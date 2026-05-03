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

    // Optional title row at the top. Body shrinks by one row when present.
    let body = if let Some(ref title) = display.title {
        let row_y = area.y;
        let mut col = 0u16;
        for span in &title.spans {
            let span_fg = span.fg.map(ratatui_color).unwrap_or(fg);
            let span_bg = span.bg.map(ratatui_color).unwrap_or(bg);
            for ch in span.text.chars() {
                if col >= area.width {
                    break;
                }
                set_cell(buf, area.x + col, row_y, ch, span_fg, span_bg);
                col += 1;
            }
        }
        Rect {
            x: area.x,
            y: area.y + 1,
            width: area.width,
            height: area.height.saturating_sub(1),
        }
    } else {
        area
    };
    if body.height == 0 {
        return;
    }

    let layout = if display.show_scrollbar {
        display.layout_with_scrollbar(body.width as f32, body.height as f32, 1.0, 1.0, |_| {
            TextDisplayLineMeasure::new(1.0)
        })
    } else {
        display.layout(body.width as f32, body.height as f32, |_| {
            TextDisplayLineMeasure::new(1.0)
        })
    };

    for vis in &layout.visible_lines {
        let line = &display.lines[vis.line_idx];
        let row_y = body.y + vis.bounds.y.round() as u16;
        if row_y >= body.y + body.height {
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
                if col >= body.width {
                    break;
                }
                set_cell(buf, body.x + col, row_y, ch, muted, bg);
                col += 1;
            }
            if col < body.width {
                set_cell(buf, body.x + col, row_y, ' ', muted, bg);
                col += 1;
            }
        }

        // Spans.
        let line_end = if display.show_scrollbar {
            body.width.saturating_sub(1)
        } else {
            body.width
        };
        for span in &line.spans {
            let span_fg = span.fg.map(ratatui_color).unwrap_or(line_fg);
            let span_bg = span.bg.map(ratatui_color).unwrap_or(bg);
            for ch in span.text.chars() {
                if col >= line_end {
                    break;
                }
                set_cell(buf, body.x + col, row_y, ch, span_fg, span_bg);
                col += 1;
            }
        }
    }

    // Scrollbar gutter.
    if display.show_scrollbar {
        let gutter_x = body.x + body.width - 1;
        let track_fg = ratatui_color(theme.muted_fg);
        for dy in 0..body.height {
            set_cell(buf, gutter_x, body.y + dy, '│', track_fg, bg);
        }
        if let Some(thumb) = layout.thumb_bounds {
            let thumb_y = body.y + thumb.y.round() as u16;
            let thumb_h = thumb.height.round() as u16;
            let thumb_fg = ratatui_color(theme.foreground);
            for dy in 0..thumb_h {
                let y = thumb_y + dy;
                if y < body.y + body.height {
                    set_cell(buf, gutter_x, y, '█', thumb_fg, bg);
                }
            }
        }
    }
}

/// Compute the text-display layout using TUI-native metrics (1 cell per
/// line, 1-cell scrollbar gutter). Consumers call this to drive hit-testing
/// for scrollbar drag interaction without re-deriving metrics.
pub fn tui_text_display_layout(
    display: &TextDisplay,
    area: Rect,
) -> crate::primitives::text_display::TextDisplayLayout {
    let body = if display.title.is_some() {
        Rect {
            x: area.x,
            y: area.y + 1,
            width: area.width,
            height: area.height.saturating_sub(1),
        }
    } else {
        area
    };
    if body.height == 0 {
        return display.layout(0.0, 0.0, |_| TextDisplayLineMeasure::new(1.0));
    }
    if display.show_scrollbar {
        display.layout_with_scrollbar(body.width as f32, body.height as f32, 1.0, 1.0, |_| {
            TextDisplayLineMeasure::new(1.0)
        })
    } else {
        display.layout(body.width as f32, body.height as f32, |_| {
            TextDisplayLineMeasure::new(1.0)
        })
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
            title: None,
            show_scrollbar: false,
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
            title: None,
            show_scrollbar: false,
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
            title: None,
            show_scrollbar: false,
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
            title: None,
            show_scrollbar: false,
        };
        draw_text_display(&mut buf, Rect::new(0, 0, 0, 5), &display, &Theme::default());
        assert_eq!(cell_char(&buf, 0, 0), ' ');
    }

    #[test]
    fn scrollbar_thumb_paint_and_click_round_trip() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 5));
        let display = TextDisplay {
            id: WidgetId::new("td"),
            lines: (0..20).map(|i| line(&format!("line{i}"))).collect(),
            scroll_offset: 0,
            auto_scroll: false,
            max_lines: 0,
            has_focus: false,
            title: None,
            show_scrollbar: true,
        };
        draw_text_display(
            &mut buf,
            Rect::new(0, 0, 20, 5),
            &display,
            &Theme::default(),
        );

        // Scrollbar gutter at column 19. Thumb should be at top (scroll_offset=0).
        assert_eq!(cell_char(&buf, 19, 0), '█');

        // Use layout for hit-test.
        let layout = display
            .layout_with_scrollbar(20.0, 5.0, 1.0, 1.0, |_| TextDisplayLineMeasure::new(1.0));
        let thumb = layout.thumb_bounds.expect("thumb bounds present");
        let hit = layout.hit_test(thumb.x + 0.5, thumb.y + 0.5);
        assert_eq!(
            hit,
            crate::primitives::text_display::TextDisplayHit::ScrollbarThumb
        );
    }

    #[test]
    fn scrollbar_track_after_hit() {
        let display = TextDisplay {
            id: WidgetId::new("td"),
            lines: (0..20).map(|i| line(&format!("line{i}"))).collect(),
            scroll_offset: 0,
            auto_scroll: false,
            max_lines: 0,
            has_focus: false,
            title: None,
            show_scrollbar: true,
        };
        let layout = display
            .layout_with_scrollbar(20.0, 5.0, 1.0, 1.0, |_| TextDisplayLineMeasure::new(1.0));
        // Click below the thumb → TrackAfter.
        let hit = layout.hit_test(19.5, 4.5);
        assert_eq!(
            hit,
            crate::primitives::text_display::TextDisplayHit::ScrollbarTrackAfter
        );
    }

    #[test]
    fn no_scrollbar_when_disabled() {
        let display = TextDisplay {
            id: WidgetId::new("td"),
            lines: (0..20).map(|i| line(&format!("line{i}"))).collect(),
            scroll_offset: 0,
            auto_scroll: false,
            max_lines: 0,
            has_focus: false,
            title: None,
            show_scrollbar: false,
        };
        let layout = display.layout(20.0, 5.0, |_| TextDisplayLineMeasure::new(1.0));
        assert!(layout.scrollbar_bounds.is_none());
        assert!(layout.thumb_bounds.is_none());
    }

    #[test]
    fn scrollbar_body_width_reduced() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 5));
        let display = TextDisplay {
            id: WidgetId::new("td"),
            lines: vec![line("abcdefghijklmnopqrstuvwxyz")],
            scroll_offset: 0,
            auto_scroll: false,
            max_lines: 0,
            has_focus: false,
            title: None,
            show_scrollbar: true,
        };
        draw_text_display(
            &mut buf,
            Rect::new(0, 0, 20, 5),
            &display,
            &Theme::default(),
        );
        // Text should stop before the scrollbar column (col 19).
        // Col 18 should have 's' (19th char), col 19 should be scrollbar.
        assert_eq!(cell_char(&buf, 18, 0), 's');
        assert_ne!(cell_char(&buf, 19, 0), 't');
    }

    #[test]
    fn track_before_page_up_reaches_line_zero() {
        // Scenario: 40 lines, 30-row viewport, scrolled to bottom
        // (offset=10, showing lines 10..39). Consumer clicks TrackBefore
        // and pages up by visible_lines.len(). After paging, offset
        // should be 0 and lines 0..29 should be visible.
        let viewport_h = 30.0_f32;
        let total_lines = 40;

        // Step 1: layout at the bottom (scroll_offset=10).
        let display_bottom = TextDisplay {
            id: WidgetId::new("td"),
            lines: (0..total_lines)
                .map(|i| line(&format!("L{i:02}")))
                .collect(),
            scroll_offset: 10,
            auto_scroll: false,
            max_lines: 0,
            has_focus: false,
            title: None,
            show_scrollbar: true,
        };
        let layout_bottom =
            display_bottom.layout_with_scrollbar(40.0, viewport_h, 1.0, 1.0, |_| {
                TextDisplayLineMeasure::new(1.0)
            });
        assert_eq!(layout_bottom.resolved_scroll_offset, 10);
        assert_eq!(layout_bottom.visible_lines.len(), 30);
        assert_eq!(layout_bottom.visible_lines[0].line_idx, 10);
        assert_eq!(layout_bottom.visible_lines[29].line_idx, 39);

        // Thumb should be at the bottom of the track.
        let thumb = layout_bottom.thumb_bounds.expect("thumb");
        assert!(
            thumb.y > 0.0,
            "thumb should not be at top when scrolled to bottom"
        );

        // TrackBefore should exist above the thumb.
        let hit_top = layout_bottom.hit_test(39.5, 0.5);
        assert_eq!(
            hit_top,
            crate::primitives::text_display::TextDisplayHit::ScrollbarTrackBefore
        );

        // Step 2: consumer pages up by visible_lines.len().
        let page_size = layout_bottom.visible_lines.len();
        let new_offset = 10_usize.saturating_sub(page_size);
        assert_eq!(new_offset, 0, "page-up from 10 by 30 should reach 0");

        // Step 3: re-layout at offset 0.
        let display_top = TextDisplay {
            id: WidgetId::new("td"),
            lines: (0..total_lines)
                .map(|i| line(&format!("L{i:02}")))
                .collect(),
            scroll_offset: new_offset,
            auto_scroll: false,
            max_lines: 0,
            has_focus: false,
            title: None,
            show_scrollbar: true,
        };
        let layout_top = display_top.layout_with_scrollbar(40.0, viewport_h, 1.0, 1.0, |_| {
            TextDisplayLineMeasure::new(1.0)
        });
        assert_eq!(layout_top.resolved_scroll_offset, 0);
        assert_eq!(
            layout_top.visible_lines[0].line_idx, 0,
            "first visible line should be 0 after paging to top"
        );
        assert_eq!(layout_top.visible_lines.len(), 30);

        // Thumb should now be at y=0.
        let thumb_top = layout_top.thumb_bounds.expect("thumb at top");
        assert_eq!(
            thumb_top.y, 0.0,
            "thumb should be at top when scroll_offset=0"
        );
    }
}
