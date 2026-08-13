//! TUI rasteriser for [`crate::Tooltip`].
//!
//! Renders a full box — `┌─┐`/`└─┘` top and bottom border rows plus `│`
//! side borders on every row — whenever the measured height leaves room
//! for both border rows and at least one content row (`height >= 3`).
//! This matches `quadraui::gtk::draw_tooltip`, which has always stroked a
//! full rectangle; before this, TUI painted side bars only, so the same
//! primitive produced materially different chrome on the two backends
//! while `screen_has("Keybindings")` stayed `true` on both — the #541
//! divergence #542's structural-parity tier exists to catch. A tooltip
//! too short for that (`height < 3`, e.g. a single content row with no
//! vertical padding measured in) falls back to the original side-bars-only
//! chrome rather than rendering zero usable content rows.
//!
//! When `tooltip.styled_lines` is `Some`, each entry renders as one
//! row of styled spans (multi-line styled path used by signature help
//! and diff peek). Otherwise `tooltip.text` is split on `\n` and each
//! line is rendered plain (LSP hover popup path). Lines that exceed the
//! box width or the available content rows are truncated.

use ratatui::buffer::Buffer;

use super::{ratatui_color, set_cell};
use crate::primitives::tooltip::{Tooltip, TooltipLayout};
use crate::theme::Theme;
use crate::types::Color;

fn qc(c: Color) -> ratatui::style::Color {
    ratatui_color(c)
}

/// Draw a [`Tooltip`] into `layout.bounds` on `buf`.
///
/// Per-tooltip `tooltip.fg` / `tooltip.bg` overrides win over the
/// theme defaults. The frame border always uses [`Theme::hover_border`].
pub fn draw_tooltip(buf: &mut Buffer, tooltip: &Tooltip, layout: &TooltipLayout, theme: &Theme) {
    let x = layout.bounds.x.round() as u16;
    let y = layout.bounds.y.round() as u16;
    let w = layout.bounds.width.round() as u16;
    let h = layout.bounds.height.round() as u16;
    if w == 0 || h == 0 {
        return;
    }

    let fg = tooltip
        .fg
        .map(qc)
        .unwrap_or_else(|| ratatui_color(theme.hover_fg));
    let bg = tooltip
        .bg
        .map(qc)
        .unwrap_or_else(|| ratatui_color(theme.hover_bg));
    let border = ratatui_color(theme.hover_border);

    // A full box costs one row for the top border and one for the
    // bottom, leaving `h - 2` for content; below that there's no room
    // for both border rows and any content, so keep the legacy
    // side-bars-only chrome instead of rendering an empty box.
    let has_horizontal_border = h >= 3 && w >= 2;
    let content_row0: u16 = if has_horizontal_border { 1 } else { 0 };
    let content_rows: u16 = if has_horizontal_border { h - 2 } else { h };

    if has_horizontal_border {
        let paint_border_row = |buf: &mut Buffer, row: u16, top: bool| {
            for col in 0..w {
                let ch = if col == 0 {
                    if top {
                        '┌'
                    } else {
                        '└'
                    }
                } else if col == w - 1 {
                    if top {
                        '┐'
                    } else {
                        '┘'
                    }
                } else {
                    '─'
                };
                set_cell(buf, x + col, row, ch, border, bg);
            }
        };
        paint_border_row(buf, y, true);
        paint_border_row(buf, y + h - 1, false);
    }

    let paint_row_background = |buf: &mut Buffer, row: u16| {
        for col in 0..w {
            let ch = if col == 0 || col == w - 1 { '│' } else { ' ' };
            let cell_fg = if col == 0 || col == w - 1 { border } else { fg };
            set_cell(buf, x + col, row, ch, cell_fg, bg);
        }
    };

    if let Some(ref styled_lines) = tooltip.styled_lines {
        for (i, styled) in styled_lines.iter().enumerate().take(content_rows as usize) {
            let row = y + content_row0 + i as u16;
            paint_row_background(buf, row);
            let mut col_off: u16 = 2; // skip border + 1 pad
            for span in &styled.spans {
                let span_fg = span.fg.map(qc).unwrap_or(fg);
                let span_bg = span.bg.map(qc).unwrap_or(bg);
                for ch in span.text.chars() {
                    let col = x + col_off;
                    if col + 1 >= x + w {
                        break;
                    }
                    set_cell(buf, col, row, ch, span_fg, span_bg);
                    col_off += 1;
                }
            }
        }
        return;
    }

    let lines: Vec<&str> = tooltip.text.lines().collect();
    for (i, text_line) in lines.iter().enumerate().take(content_rows as usize) {
        let row = y + content_row0 + i as u16;
        paint_row_background(buf, row);
        for (j, ch) in text_line.chars().enumerate() {
            let col = x + 2 + j as u16;
            if col + 1 >= x + w {
                break;
            }
            set_cell(buf, col, row, ch, fg, bg);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::Rect as QRect;
    use crate::primitives::tooltip::{ResolvedPlacement, Tooltip, TooltipLayout};
    use crate::types::{StyledSpan, StyledText, WidgetId};
    use ratatui::layout::Rect;

    fn make_layout(x: f32, y: f32, w: f32, h: f32) -> TooltipLayout {
        TooltipLayout {
            bounds: QRect::new(x, y, w, h),
            resolved_placement: ResolvedPlacement::Bottom,
        }
    }

    fn cell_char(buf: &Buffer, x: u16, y: u16) -> char {
        buf[(x, y)].symbol().chars().next().unwrap_or(' ')
    }

    #[test]
    fn paints_side_borders_and_plain_text() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 30, 5));
        let tt = Tooltip {
            id: WidgetId::new("hover"),
            text: "hello".into(),
            styled_lines: None,
            placement: crate::primitives::tooltip::TooltipPlacement::Bottom,
            fg: None,
            bg: None,
        };
        let layout = make_layout(0.0, 0.0, 10.0, 1.0);
        draw_tooltip(&mut buf, &tt, &layout, &Theme::default());

        // Borders at col 0 and col 9.
        assert_eq!(cell_char(&buf, 0, 0), '│');
        assert_eq!(cell_char(&buf, 9, 0), '│');
        // Text starts at col 2.
        let row: String = (2..7).map(|x| cell_char(&buf, x, 0)).collect();
        assert_eq!(row, "hello");
    }

    /// #541/#542: once there's room for both border rows (`height >= 3`),
    /// the box must be closed on every side — not just left/right — so a
    /// structural-parity observer sees the same chrome shape GTK has
    /// always drawn.
    #[test]
    fn paints_a_full_box_when_height_allows_it() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 30, 5));
        let tt = Tooltip {
            id: WidgetId::new("hover"),
            text: "hello".into(),
            styled_lines: None,
            placement: crate::primitives::tooltip::TooltipPlacement::Bottom,
            fg: None,
            bg: None,
        };
        let layout = make_layout(0.0, 0.0, 10.0, 3.0);
        draw_tooltip(&mut buf, &tt, &layout, &Theme::default());

        // Top row: corners + a solid horizontal rule.
        assert_eq!(cell_char(&buf, 0, 0), '┌');
        assert_eq!(cell_char(&buf, 9, 0), '┐');
        assert_eq!(cell_char(&buf, 4, 0), '─');
        // Bottom row: corners + a solid horizontal rule.
        assert_eq!(cell_char(&buf, 0, 2), '└');
        assert_eq!(cell_char(&buf, 9, 2), '┘');
        assert_eq!(cell_char(&buf, 4, 2), '─');
        // Content moved down one row, side borders unchanged.
        assert_eq!(cell_char(&buf, 0, 1), '│');
        assert_eq!(cell_char(&buf, 9, 1), '│');
        let row: String = (2..7).map(|x| cell_char(&buf, x, 1)).collect();
        assert_eq!(row, "hello");
    }

    #[test]
    fn styled_lines_paint_each_row() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 30, 5));
        let tt = Tooltip {
            id: WidgetId::new("sig"),
            text: String::new(),
            styled_lines: Some(vec![
                StyledText {
                    spans: vec![StyledSpan::plain("line1")],
                },
                StyledText {
                    spans: vec![StyledSpan::plain("line2")],
                },
            ]),
            placement: crate::primitives::tooltip::TooltipPlacement::Bottom,
            fg: None,
            bg: None,
        };
        let layout = make_layout(0.0, 0.0, 12.0, 2.0);
        draw_tooltip(&mut buf, &tt, &layout, &Theme::default());

        let r0: String = (2..7).map(|x| cell_char(&buf, x, 0)).collect();
        let r1: String = (2..7).map(|x| cell_char(&buf, x, 1)).collect();
        assert_eq!(r0, "line1");
        assert_eq!(r1, "line2");
    }

    #[test]
    fn per_tooltip_bg_overrides_theme() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 30, 5));
        let tt = Tooltip {
            id: WidgetId::new("hover"),
            text: "x".into(),
            styled_lines: None,
            placement: crate::primitives::tooltip::TooltipPlacement::Bottom,
            fg: None,
            bg: Some(Color::rgb(100, 0, 0)),
        };
        let layout = make_layout(0.0, 0.0, 10.0, 1.0);
        draw_tooltip(&mut buf, &tt, &layout, &Theme::default());
        // Cell 2 should have bg = (100, 0, 0).
        let bg = buf[(2u16, 0u16)].bg;
        assert_eq!(bg, ratatui::style::Color::Rgb(100, 0, 0));
    }

    #[test]
    fn zero_size_is_a_no_op() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 5));
        let tt = Tooltip {
            id: WidgetId::new("hover"),
            text: "x".into(),
            styled_lines: None,
            placement: crate::primitives::tooltip::TooltipPlacement::Bottom,
            fg: None,
            bg: None,
        };
        let layout = make_layout(0.0, 0.0, 0.0, 1.0);
        draw_tooltip(&mut buf, &tt, &layout, &Theme::default());
        assert_eq!(cell_char(&buf, 0, 0), ' ');
    }
}
