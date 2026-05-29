//! TUI rasteriser for [`crate::Dialog`].
//!
//! Bordered modal popup with a title bar, multi-line body text, an
//! optional input field, and a row (or column) of buttons. Renders
//! with the rounded `╭─╮ ╰─╯` glyphs the TUI has used since the
//! pre-D6 dialog renderer.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect as RRect;

use super::{ratatui_color, set_cell};
use crate::primitives::dialog::{Dialog, DialogInput, DialogLayout};
use crate::primitives::toolbar::ToolbarItemMeasure;
use crate::theme::Theme;
use crate::types::StyledText;

/// Flatten a [`StyledText`] to plain — dialog title + body don't carry
/// per-span style overrides today.
fn flatten(text: &StyledText) -> String {
    text.spans.iter().map(|s| s.text.as_str()).collect()
}

/// Compute the TUI dialog layout using cell metrics.
///
/// Uses the same `tui_item_width` measurer as the toolbar rasteriser so
/// toolbar button positions agree between paint and hit-test.
pub fn tui_dialog_layout(dialog: &Dialog, viewport: crate::event::Rect) -> DialogLayout {
    use super::toolbar::tui_item_width;
    use crate::primitives::dialog::DialogMeasure;

    // Measure body height as number of body lines (capped to a maximum
    // to keep the dialog from filling the whole viewport).
    let body_h = (dialog.body.len() as f32).clamp(0.0, 10.0);
    let input_h = if dialog.input.is_some() { 1.0 } else { 0.0 };
    let measure = DialogMeasure {
        width: (viewport.width * 0.5).clamp(30.0, 60.0),
        title_height: if dialog.title.spans.iter().any(|s| !s.text.is_empty()) {
            1.0
        } else {
            0.0
        },
        body_height: body_h,
        input_height: input_h,
        button_row_height: 1.0,
        button_width: 8.0,
        button_gap: 2.0,
        padding: 1.0,
    };
    dialog.layout(viewport, measure, |btn| {
        ToolbarItemMeasure::new(tui_item_width(btn))
    })
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

    // Body text — one StyledText per line.
    let body_x = layout.body_bounds.x.round() as u16;
    let body_y = layout.body_bounds.y.round() as u16;
    let body_w = layout.body_bounds.width.round() as u16;
    for (i, line) in dialog.body.iter().enumerate() {
        let row = body_y + i as u16;
        if row >= body_y + layout.body_bounds.height.round() as u16 {
            break;
        }
        let text = flatten(line);
        for (j, ch) in text.chars().enumerate() {
            let col = body_x + j as u16;
            if col >= body_x + body_w {
                break;
            }
            set_cell(buf, col, row, ch, fg, bg);
        }
    }

    // Optional input slot — text input or embedded toolbar.
    if let (Some(input_bounds), Some(input_kind)) = (layout.input_bounds, &dialog.input) {
        let ix = input_bounds.x.round() as u16;
        let iy = input_bounds.y.round() as u16;
        let iw = input_bounds.width.round() as u16;
        match input_kind {
            DialogInput::TextInput(input) => {
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
            DialogInput::Toolbar(toolbar) => {
                // Delegate to the toolbar rasteriser. The toolbar layout
                // was precomputed in `body_toolbar_layout`; we reconstruct
                // the area rect from the input_bounds so draw_toolbar can
                // repaint with hover/pressed state if the caller provides
                // it. For the dialog paint path we pass `None` for both.
                let toolbar_area = RRect::new(ix, iy, iw, 1);
                super::toolbar::draw_toolbar(buf, toolbar_area, toolbar, theme, None, None);
            }
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
    use crate::primitives::dialog::{Dialog, DialogButton, DialogMeasure, DialogTextInput};
    use crate::types::{StyledSpan, WidgetId};
    use ratatui::layout::Rect;

    fn make_dialog() -> Dialog {
        Dialog {
            id: WidgetId::new("d"),
            title: StyledText {
                spans: vec![StyledSpan::plain("Confirm")],
            },
            body: vec![StyledText {
                spans: vec![StyledSpan::plain("Save before quitting?")],
            }],
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
        dialog.layout(viewport, measure, |_| ToolbarItemMeasure::new(0.0))
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
        d.input = Some(crate::primitives::dialog::DialogInput::TextInput(
            crate::primitives::dialog::DialogTextInput {
                value: "hello".into(),
                placeholder: String::new(),
                cursor: Some(5),
            },
        ));
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
        let layout = d.layout(viewport, measure, |_| ToolbarItemMeasure::new(0.0));
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
        let layout = d.layout(viewport, measure, |_| ToolbarItemMeasure::new(0.0));
        draw_dialog(&mut buf, &d, &layout, &Theme::default());
        assert_eq!(cell_char(&buf, 0, 0), ' ');
    }

    // ── Gap A: DialogInput::Toolbar paint↔click round-trip ───────────────

    use crate::primitives::dialog::{DialogHit, DialogInput};
    use crate::primitives::toolbar::{Toolbar, ToolbarButton};

    fn make_toolbar_dialog() -> Dialog {
        Dialog {
            id: WidgetId::new("migrate"),
            title: StyledText {
                spans: vec![StyledSpan::plain("Confirm migration")],
            },
            body: vec![StyledText {
                spans: vec![StyledSpan::plain("Choose an action:")],
            }],
            buttons: vec![DialogButton {
                id: WidgetId::new("ok"),
                label: "OK".into(),
                is_default: true,
                is_cancel: false,
                tint: None,
            }],
            severity: None,
            vertical_buttons: false,
            input: Some(DialogInput::Toolbar(Toolbar {
                id: WidgetId::new("body-toolbar"),
                buttons: vec![
                    ToolbarButton::Action {
                        id: WidgetId::new("preview"),
                        label: "Preview".into(),
                        icon: None,
                        key_hint: None,
                        enabled: true,
                        is_active: false,
                        tooltip: String::new(),
                    },
                    ToolbarButton::Separator,
                    ToolbarButton::Action {
                        id: WidgetId::new("apply"),
                        label: "Apply".into(),
                        icon: None,
                        key_hint: None,
                        enabled: true,
                        is_active: false,
                        tooltip: String::new(),
                    },
                ],
                bg: None,
            })),
        }
    }

    #[test]
    fn body_toolbar_paints_button_brackets() {
        let d = make_toolbar_dialog();
        let viewport = crate::event::Rect::new(0.0, 0.0, 80.0, 30.0);
        let layout = tui_dialog_layout(&d, viewport);
        let mut buf = Buffer::empty(Rect::new(0, 0, 80, 30));
        draw_dialog(&mut buf, &d, &layout, &Theme::default());

        // The input_bounds slot should be set.
        assert!(
            layout.input_bounds.is_some(),
            "input_bounds should be Some for Toolbar variant"
        );
        assert!(
            layout.body_toolbar_layout.is_some(),
            "body_toolbar_layout should be Some"
        );

        // The toolbar should have painted `[` somewhere inside the dialog.
        let ib = layout.input_bounds.unwrap();
        let start_y = ib.y.round() as u16;
        let start_x = ib.x.round() as u16;
        let end_x = (ib.x + ib.width).round() as u16;

        let mut found_bracket = false;
        for x in start_x..end_x {
            if cell_char(&buf, x, start_y) == '[' {
                found_bracket = true;
                break;
            }
        }
        assert!(
            found_bracket,
            "toolbar `[` bracket should be painted in body slot"
        );
    }

    #[test]
    fn body_toolbar_click_routes_to_body_toolbar_button() {
        let d = make_toolbar_dialog();
        let viewport = crate::event::Rect::new(0.0, 0.0, 80.0, 30.0);
        let layout = tui_dialog_layout(&d, viewport);
        let mut buf = Buffer::empty(Rect::new(0, 0, 80, 30));
        draw_dialog(&mut buf, &d, &layout, &Theme::default());

        let tl = layout
            .body_toolbar_layout
            .as_ref()
            .expect("toolbar layout present");
        // Find the "Preview" button in the toolbar layout.
        let preview_vis = tl
            .visible_items
            .iter()
            .find(|v| v.action_id.as_ref().map(|id| id.as_str()) == Some("preview"))
            .expect("preview button visible");
        assert!(preview_vis.clickable, "preview should be clickable");

        // Click inside the "Preview" button bounds.
        let cx = preview_vis.bounds.x + 1.0;
        let cy = preview_vis.bounds.y;
        match layout.hit_test(cx, cy) {
            DialogHit::BodyToolbarButton(id) => {
                assert_eq!(id.as_str(), "preview");
            }
            other => panic!("expected BodyToolbarButton(preview), got {:?}", other),
        }
    }

    #[test]
    fn body_toolbar_apply_button_also_routes() {
        let d = make_toolbar_dialog();
        let viewport = crate::event::Rect::new(0.0, 0.0, 80.0, 30.0);
        let layout = tui_dialog_layout(&d, viewport);

        let tl = layout
            .body_toolbar_layout
            .as_ref()
            .expect("toolbar layout present");
        let apply_vis = tl
            .visible_items
            .iter()
            .find(|v| v.action_id.as_ref().map(|id| id.as_str()) == Some("apply"))
            .expect("apply button visible");

        let cx = apply_vis.bounds.x + 1.0;
        let cy = apply_vis.bounds.y;
        match layout.hit_test(cx, cy) {
            DialogHit::BodyToolbarButton(id) => {
                assert_eq!(id.as_str(), "apply");
            }
            other => panic!("expected BodyToolbarButton(apply), got {:?}", other),
        }
    }

    // ── Serde round-trip for DialogInput::Toolbar ─────────────────────────

    #[test]
    fn serde_roundtrip_dialog_input_toolbar() {
        use crate::types::WidgetId;
        let input = DialogInput::Toolbar(Toolbar {
            id: WidgetId::new("tb"),
            buttons: vec![
                ToolbarButton::Action {
                    id: WidgetId::new("preview"),
                    label: "Preview".into(),
                    icon: None,
                    key_hint: None,
                    enabled: true,
                    is_active: false,
                    tooltip: String::new(),
                },
                ToolbarButton::Separator,
            ],
            bg: None,
        });
        let json = serde_json::to_string(&input).unwrap();
        let back: DialogInput = serde_json::from_str(&json).unwrap();
        assert_eq!(input, back);
    }

    #[test]
    fn serde_roundtrip_dialog_input_text_input() {
        let input = DialogInput::TextInput(DialogTextInput {
            value: "hello".into(),
            placeholder: "placeholder".into(),
            cursor: Some(5),
        });
        let json = serde_json::to_string(&input).unwrap();
        let back: DialogInput = serde_json::from_str(&json).unwrap();
        assert_eq!(input, back);
    }
}
