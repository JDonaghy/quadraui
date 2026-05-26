//! TUI rasteriser for [`crate::primitives::toolbar::Toolbar`].
//!
//! Paints a single-row strip of `[icon label (key)]` cells with hit
//! zones per `Action`. Separators render as a dim ` │ ` cell; non-
//! clickable `Label` items paint their text in `theme.muted_fg`.
//!
//! ## Per-state colouring
//!
//! | State              | Foreground             | Background         |
//! |--------------------|------------------------|--------------------|
//! | Action, enabled    | `theme.foreground`     | `bar_bg`           |
//! | Action, disabled   | `theme.muted_fg`       | `bar_bg`           |
//! | Action, is_active  | `theme.foreground`     | `theme.selected_bg`|
//! | Action, hovered    | `theme.hover_fg`       | `theme.hover_bg`   |
//! | Action, pressed    | `theme.foreground`     | `theme.selected_bg`|
//! | Separator          | `theme.muted_fg`       | `bar_bg`           |
//! | Label              | `Label.fg` or `muted_fg`| `bar_bg`          |
//!
//! `bar_bg` is `Toolbar.bg.unwrap_or(theme.header_bg)`.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::{qc, set_cell};
use crate::primitives::toolbar::{Toolbar, ToolbarButton, ToolbarItemMeasure, ToolbarLayout};
use crate::theme::Theme;
use crate::types::WidgetId;

/// Compute the TUI cell-unit width of a single toolbar item.
///
/// Action: `[ {icon }{label}{ (key)} ]` — 4 cells of wrapper + content.
/// Separator: 2 cells (` │`).
/// Label: text width.
pub(crate) fn tui_item_width(btn: &ToolbarButton) -> f32 {
    match btn {
        ToolbarButton::Action {
            label,
            icon,
            key_hint,
            ..
        } => {
            let icon_w = icon.as_ref().map(|s| s.chars().count() + 1).unwrap_or(0);
            let hint_w = key_hint
                .as_ref()
                .map(|s| s.chars().count() + 3) // " (xxx)"
                .unwrap_or(0);
            // "[ " + icon? + label + hint? + " ]"
            (4 + icon_w + label.chars().count() + hint_w) as f32
        }
        ToolbarButton::Separator => 2.0,
        ToolbarButton::Label { text, .. } => text.chars().count() as f32,
    }
}

/// Compute the layout the rasteriser would produce. No paint side-effects.
pub fn tui_toolbar_layout(bar: &Toolbar, area: Rect) -> ToolbarLayout {
    bar.layout(
        area.x as f32,
        area.y as f32,
        area.width as f32,
        area.height.max(1) as f32,
        |btn| ToolbarItemMeasure::new(tui_item_width(btn)),
    )
}

/// Draw a [`Toolbar`] into `area` on `buf`. Returns the layout for host
/// click dispatch.
pub fn draw_toolbar(
    buf: &mut Buffer,
    area: Rect,
    bar: &Toolbar,
    theme: &Theme,
    hovered_id: Option<&WidgetId>,
    pressed_id: Option<&WidgetId>,
) -> ToolbarLayout {
    let layout = tui_toolbar_layout(bar, area);

    if area.width == 0 || area.height == 0 {
        return layout;
    }

    let bar_bg = qc(bar.bg.unwrap_or(theme.header_bg));
    let fg = qc(theme.foreground);
    let muted = qc(theme.muted_fg);
    let hover_bg = qc(theme.hover_bg);
    let hover_fg = qc(theme.hover_fg);
    let active_bg = qc(theme.selected_bg);

    // Fill the bar background.
    let row_y = area.y;
    for x in 0..area.width {
        set_cell(buf, area.x + x, row_y, ' ', fg, bar_bg);
    }

    for vis in &layout.visible_items {
        let item_x = vis.bounds.x.round() as u16;
        let item_w = vis.bounds.width.round() as u16;
        if item_w == 0 {
            continue;
        }
        let btn = &bar.buttons[vis.item_idx];

        match btn {
            ToolbarButton::Action {
                id,
                label,
                icon,
                key_hint,
                enabled,
                is_active,
                ..
            } => {
                let is_hovered = *enabled && hovered_id == Some(id);
                let is_pressed = *enabled && pressed_id == Some(id);
                let (cell_fg, cell_bg) = if !*enabled {
                    (muted, bar_bg)
                } else if is_pressed || *is_active {
                    (fg, active_bg)
                } else if is_hovered {
                    (hover_fg, hover_bg)
                } else {
                    (fg, bar_bg)
                };

                // Build the rendered string: `[ {icon }{label}{ (hint)} ]`
                let mut s = String::with_capacity(item_w as usize);
                s.push('[');
                s.push(' ');
                if let Some(icon) = icon {
                    s.push_str(icon);
                    s.push(' ');
                }
                s.push_str(label);
                if let Some(hint) = key_hint {
                    s.push_str(" (");
                    s.push_str(hint);
                    s.push(')');
                }
                s.push(' ');
                s.push(']');

                // Paint each cell. Background covers full bounds even if
                // text is shorter than item_w (helps hover highlight read
                // as one visual unit).
                for dx in 0..item_w {
                    let ch = s.chars().nth(dx as usize).unwrap_or(' ');
                    set_cell(buf, item_x + dx, row_y, ch, cell_fg, cell_bg);
                }
            }
            ToolbarButton::Separator => {
                // 2 cells: ` │`
                if item_w >= 1 {
                    set_cell(buf, item_x, row_y, ' ', muted, bar_bg);
                }
                if item_w >= 2 {
                    set_cell(buf, item_x + 1, row_y, '│', muted, bar_bg);
                }
            }
            ToolbarButton::Label { text, fg: label_fg } => {
                let color = label_fg.map(qc).unwrap_or(muted);
                for (i, ch) in text.chars().enumerate() {
                    if i as u16 >= item_w {
                        break;
                    }
                    set_cell(buf, item_x + i as u16, row_y, ch, color, bar_bg);
                }
            }
        }
    }

    layout
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::toolbar::{ToolbarButton, ToolbarHit};
    use crate::types::WidgetId;

    fn cell_char(buf: &Buffer, x: u16, y: u16) -> char {
        buf[(x, y)].symbol().chars().next().unwrap_or(' ')
    }

    fn mk_action(id: &str, label: &str, enabled: bool) -> ToolbarButton {
        ToolbarButton::Action {
            id: WidgetId::new(id),
            label: label.to_string(),
            icon: None,
            key_hint: None,
            enabled,
            is_active: false,
            tooltip: String::new(),
        }
    }

    #[test]
    fn draws_action_buttons_with_brackets() {
        let area = Rect::new(0, 0, 40, 1);
        let mut buf = Buffer::empty(area);
        let bar = Toolbar {
            id: WidgetId::new("tb"),
            buttons: vec![mk_action("a", "Refine", true)],
            bg: None,
        };
        let _layout = draw_toolbar(&mut buf, area, &bar, &Theme::default(), None, None);
        // First two cells should be `[` then ` `.
        assert_eq!(cell_char(&buf, 0, 0), '[');
        assert_eq!(cell_char(&buf, 1, 0), ' ');
    }

    #[test]
    fn click_round_trip_through_layout() {
        let area = Rect::new(0, 0, 40, 1);
        let mut buf = Buffer::empty(area);
        let bar = Toolbar {
            id: WidgetId::new("tb"),
            buttons: vec![mk_action("a", "Refine", true), mk_action("b", "Drop", true)],
            bg: None,
        };
        let layout = draw_toolbar(&mut buf, area, &bar, &Theme::default(), None, None);
        // Click inside the first button.
        let b = layout.visible_items[0].bounds;
        let hit = layout.hit_test(b.x + 1.0, b.y);
        match hit {
            ToolbarHit::Button(id) => assert_eq!(id.as_str(), "a"),
            _ => panic!("expected button-a hit"),
        }
        // Click inside the second.
        let b = layout.visible_items[1].bounds;
        let hit = layout.hit_test(b.x + 1.0, b.y);
        match hit {
            ToolbarHit::Button(id) => assert_eq!(id.as_str(), "b"),
            _ => panic!("expected button-b hit"),
        }
    }

    #[test]
    fn disabled_action_not_clickable_after_paint() {
        let area = Rect::new(0, 0, 40, 1);
        let mut buf = Buffer::empty(area);
        let bar = Toolbar {
            id: WidgetId::new("tb"),
            buttons: vec![mk_action("a", "Refine", false)],
            bg: None,
        };
        let layout = draw_toolbar(&mut buf, area, &bar, &Theme::default(), None, None);
        let b = layout.visible_items[0].bounds;
        assert_eq!(layout.hit_test(b.x + 1.0, b.y), ToolbarHit::Empty);
    }

    #[test]
    fn separator_draws_pipe() {
        let area = Rect::new(0, 0, 40, 1);
        let mut buf = Buffer::empty(area);
        let bar = Toolbar {
            id: WidgetId::new("tb"),
            buttons: vec![ToolbarButton::Separator],
            bg: None,
        };
        let _layout = draw_toolbar(&mut buf, area, &bar, &Theme::default(), None, None);
        // First cell is space, second is the pipe char.
        assert_eq!(cell_char(&buf, 0, 0), ' ');
        assert_eq!(cell_char(&buf, 1, 0), '│');
    }

    #[test]
    fn label_draws_text() {
        let area = Rect::new(0, 0, 40, 1);
        let mut buf = Buffer::empty(area);
        let bar = Toolbar {
            id: WidgetId::new("tb"),
            buttons: vec![ToolbarButton::Label {
                text: "2/5".into(),
                fg: None,
            }],
            bg: None,
        };
        let _layout = draw_toolbar(&mut buf, area, &bar, &Theme::default(), None, None);
        assert_eq!(cell_char(&buf, 0, 0), '2');
        assert_eq!(cell_char(&buf, 1, 0), '/');
        assert_eq!(cell_char(&buf, 2, 0), '5');
    }

    #[test]
    fn zero_size_area_is_noop() {
        let buf_area = Rect::new(0, 0, 10, 1);
        let mut buf = Buffer::empty(buf_area);
        let area = Rect::new(0, 0, 0, 0);
        let bar = Toolbar {
            id: WidgetId::new("tb"),
            buttons: vec![mk_action("a", "X", true)],
            bg: None,
        };
        let _ = draw_toolbar(&mut buf, area, &bar, &Theme::default(), None, None);
        assert_eq!(cell_char(&buf, 0, 0), ' ');
    }
}
