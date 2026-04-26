//! TUI rasteriser for [`crate::Dialog`].
//!
//! Bordered modal popup with a title bar, multi-line body text, an
//! optional input field, and a row (or column) of buttons. Renders
//! with the rounded `╭─╮ ╰─╯` glyphs the TUI has used since the
//! pre-D6 dialog renderer.

use ratatui::buffer::Buffer;

use super::{ratatui_color, set_cell};
use crate::primitives::dialog::{Dialog, DialogLayout};
use crate::theme::Theme;
use crate::types::StyledText;

/// Flatten a [`StyledText`] to plain — dialog title + body don't carry
/// per-span style overrides today.
fn flatten(text: &StyledText) -> String {
    text.spans.iter().map(|s| s.text.as_str()).collect()
}

/// Draw a [`Dialog`] at its resolved layout.
pub fn draw_dialog(buf: &mut Buffer, dialog: &Dialog, layout: &DialogLayout, theme: &Theme) {
    let bg = ratatui_color(theme.surface_bg);
    let fg = ratatui_color(theme.surface_fg);
    let sel_bg = ratatui_color(theme.selected_bg);
    let border_fg = ratatui_color(theme.border_fg);
    let title_fg = ratatui_color(theme.title_fg);
    let input_bg = ratatui_color(theme.input_bg);

    let x = layout.bounds.x.round() as u16;
    let y = layout.bounds.y.round() as u16;
    let w = layout.bounds.width.round() as u16;
    let h = layout.bounds.height.round() as u16;
    if w == 0 || h == 0 {
        return;
    }

    // Clear the box area.
    for row in y..y + h {
        for col in x..x + w {
            set_cell(buf, col, row, ' ', fg, bg);
        }
    }

    // Top border with title overlay.
    for col in 0..w {
        let ch = if col == 0 {
            '╭'
        } else if col == w - 1 {
            '╮'
        } else {
            '─'
        };
        set_cell(buf, x + col, y, ch, border_fg, bg);
    }
    let title_text = format!(" {} ", flatten(&dialog.title));
    for (i, ch) in title_text.chars().enumerate() {
        let col = 2 + i as u16;
        if col + 1 >= w {
            break;
        }
        set_cell(buf, x + col, y, ch, title_fg, bg);
    }

    // Left/right borders.
    for row in (y + 1)..(y + h - 1) {
        set_cell(buf, x, row, '│', border_fg, bg);
        set_cell(buf, x + w - 1, row, '│', border_fg, bg);
    }
    // Bottom border.
    for col in 0..w {
        let ch = if col == 0 {
            '╰'
        } else if col == w - 1 {
            '╯'
        } else {
            '─'
        };
        set_cell(buf, x + col, y + h - 1, ch, border_fg, bg);
    }

    // Body text — split on \n.
    let body_x = layout.body_bounds.x.round() as u16;
    let body_y = layout.body_bounds.y.round() as u16;
    let body_w = layout.body_bounds.width.round() as u16;
    let body_text = flatten(&dialog.body);
    for (i, line) in body_text.split('\n').enumerate() {
        let row = body_y + i as u16;
        if row >= body_y + layout.body_bounds.height.round() as u16 {
            break;
        }
        for (j, ch) in line.chars().enumerate() {
            let col = body_x + j as u16;
            if col >= body_x + body_w {
                break;
            }
            set_cell(buf, col, row, ch, fg, bg);
        }
    }

    // Optional input field.
    if let (Some(input_bounds), Some(input)) = (layout.input_bounds, &dialog.input) {
        let ix = input_bounds.x.round() as u16;
        let iy = input_bounds.y.round() as u16;
        let iw = input_bounds.width.round() as u16;
        for col in ix..ix + iw {
            set_cell(buf, col, iy, ' ', fg, input_bg);
        }
        let display = format!(" {}", input.value);
        for (i, ch) in display.chars().enumerate() {
            let col = ix + i as u16;
            if col >= ix + iw {
                break;
            }
            set_cell(buf, col, iy, ch, fg, input_bg);
        }
    }

    // Buttons — default-button gets a `selected_bg` highlight.
    for vis in &layout.visible_buttons {
        let btn = &dialog.buttons[vis.button_idx];
        let bx = vis.bounds.x.round() as u16;
        let by = vis.bounds.y.round() as u16;
        let bw = vis.bounds.width.round() as u16;
        let btn_bg = if btn.is_default { sel_bg } else { bg };
        for col in bx..bx + bw {
            set_cell(buf, col, by, ' ', fg, btn_bg);
        }
        let label_w = btn.label.chars().count() as u16;
        let start = bx + (bw.saturating_sub(label_w)) / 2;
        for (i, ch) in btn.label.chars().enumerate() {
            let col = start + i as u16;
            if col >= bx + bw {
                break;
            }
            set_cell(buf, col, by, ch, fg, btn_bg);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::dialog::{Dialog, DialogButton, DialogMeasure};
    use crate::types::{StyledSpan, WidgetId};
    use ratatui::layout::Rect;

    fn make_dialog() -> Dialog {
        Dialog {
            id: WidgetId::new("d"),
            title: StyledText {
                spans: vec![StyledSpan::plain("Confirm")],
            },
            body: StyledText {
                spans: vec![StyledSpan::plain("Save before quitting?")],
            },
            buttons: vec![
                DialogButton {
                    id: WidgetId::new("save"),
                    label: "Save".into(),
                    is_default: true,
                    is_cancel: false,
                    tint: None,
                },
                DialogButton {
                    id: WidgetId::new("cancel"),
                    label: "Cancel".into(),
                    is_default: false,
                    is_cancel: true,
                    tint: None,
                },
            ],
            severity: None,
            vertical_buttons: false,
            input: None,
        }
    }

    fn make_layout(dialog: &Dialog) -> DialogLayout {
        let measure = DialogMeasure {
            width: 40.0,
            title_height: 1.0,
            body_height: 2.0,
            input_height: 0.0,
            button_row_height: 1.0,
            button_width: 8.0,
            button_gap: 2.0,
            padding: 1.0,
        };
        let viewport = crate::event::Rect::new(0.0, 0.0, 80.0, 30.0);
        dialog.layout(viewport, measure)
    }

    fn cell_char(buf: &Buffer, x: u16, y: u16) -> char {
        buf[(x, y)].symbol().chars().next().unwrap_or(' ')
    }

    #[test]
    fn paints_corner_glyphs_and_title() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 80, 30));
        let d = make_dialog();
        let layout = make_layout(&d);
        draw_dialog(&mut buf, &d, &layout, &Theme::default());

        let bx = layout.bounds.x.round() as u16;
        let by = layout.bounds.y.round() as u16;
        let bw = layout.bounds.width.round() as u16;
        let bh = layout.bounds.height.round() as u16;

        assert_eq!(cell_char(&buf, bx, by), '╭');
        assert_eq!(cell_char(&buf, bx + bw - 1, by), '╮');
        assert_eq!(cell_char(&buf, bx, by + bh - 1), '╰');
        assert_eq!(cell_char(&buf, bx + bw - 1, by + bh - 1), '╯');
    }

    #[test]
    fn default_button_has_selected_bg() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 80, 30));
        let d = make_dialog();
        let layout = make_layout(&d);
        let theme = Theme {
            selected_bg: crate::types::Color::rgb(99, 0, 0),
            ..Theme::default()
        };
        draw_dialog(&mut buf, &d, &layout, &theme);

        // The first visible button is "Save" (is_default).
        let vis = &layout.visible_buttons[0];
        let bx = vis.bounds.x.round() as u16;
        let by = vis.bounds.y.round() as u16;
        let bg = buf[(bx, by)].bg;
        assert_eq!(bg, ratatui::style::Color::Rgb(99, 0, 0));
    }

    #[test]
    fn renders_input_field_when_present() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 80, 30));
        let mut d = make_dialog();
        d.input = Some(crate::primitives::dialog::DialogInput {
            value: "hello".into(),
            placeholder: String::new(),
            cursor: Some(5),
        });
        let measure = DialogMeasure {
            width: 40.0,
            title_height: 1.0,
            body_height: 2.0,
            input_height: 1.0,
            button_row_height: 1.0,
            button_width: 8.0,
            button_gap: 2.0,
            padding: 1.0,
        };
        let viewport = crate::event::Rect::new(0.0, 0.0, 80.0, 30.0);
        let layout = d.layout(viewport, measure);
        let theme = Theme {
            input_bg: crate::types::Color::rgb(7, 7, 7),
            ..Theme::default()
        };
        draw_dialog(&mut buf, &d, &layout, &theme);

        // Input bounds carry input_bg as the row's bg.
        let ib = layout.input_bounds.expect("input bounds present");
        let bg = buf[(ib.x.round() as u16, ib.y.round() as u16)].bg;
        assert_eq!(bg, ratatui::style::Color::Rgb(7, 7, 7));
    }

    #[test]
    fn zero_size_is_a_no_op() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 80, 30));
        let d = make_dialog();
        // Use a zero-size viewport so the layout collapses.
        let measure = DialogMeasure {
            width: 0.0,
            title_height: 0.0,
            body_height: 0.0,
            input_height: 0.0,
            button_row_height: 0.0,
            button_width: 0.0,
            button_gap: 0.0,
            padding: 0.0,
        };
        let viewport = crate::event::Rect::new(0.0, 0.0, 0.0, 0.0);
        let layout = d.layout(viewport, measure);
        draw_dialog(&mut buf, &d, &layout, &Theme::default());
        assert_eq!(cell_char(&buf, 0, 0), ' ');
    }
}
