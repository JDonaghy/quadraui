//! TUI rasteriser for [`crate::ContextMenu`].
//!
//! Box-bordered popup with one row per item. Selected clickable items
//! render inverted (fg/bg swapped); separators draw as a horizontal
//! dash; disabled items render dimmed. Item shortcut (from
//! `item.detail`) is right-aligned within the row.

use ratatui::buffer::Buffer;

use super::{ratatui_color, set_cell};
use crate::primitives::context_menu::{ContextMenu, ContextMenuLayout};
use crate::theme::Theme;

/// Draw a [`ContextMenu`] popup.
pub fn draw_context_menu(
    buf: &mut Buffer,
    menu: &ContextMenu,
    layout: &ContextMenuLayout,
    theme: &Theme,
) {
    let bg = ratatui_color(theme.tab_bar_bg);
    let fg = ratatui_color(theme.foreground);
    let sep_fg = ratatui_color(theme.muted_fg);
    let dim_fg = ratatui_color(theme.muted_fg);

    let inner_x = layout.bounds.x.round() as u16;
    let inner_y = layout.bounds.y.round() as u16;
    let inner_w = layout.bounds.width.round() as u16;
    let inner_h = layout.bounds.height.round() as u16;
    if inner_w == 0 || inner_h == 0 {
        return;
    }
    // `layout.bounds` is the **inner** items region; we draw the chrome
    // border one cell outside on every side.
    let bx = inner_x.saturating_sub(1);
    let by = inner_y.saturating_sub(1);
    let bw = inner_w + 2;
    let bh = inner_h + 2;

    for dy in 0..bh {
        for dx in 0..bw {
            let cx = bx + dx;
            let cy = by + dy;
            let ch = if dy == 0 {
                if dx == 0 {
                    '┌'
                } else if dx == bw - 1 {
                    '┐'
                } else {
                    '─'
                }
            } else if dy == bh - 1 {
                if dx == 0 {
                    '└'
                } else if dx == bw - 1 {
                    '┘'
                } else {
                    '─'
                }
            } else if dx == 0 || dx == bw - 1 {
                '│'
            } else {
                ' '
            };
            set_cell(buf, cx, cy, ch, fg, bg);
        }
    }

    for vis in &layout.visible_items {
        let item = &menu.items[vis.item_idx];
        let row_y = vis.bounds.y.round() as u16;
        if vis.is_separator {
            for dx in 0..inner_w {
                set_cell(buf, inner_x + dx, row_y, '─', sep_fg, bg);
            }
            continue;
        }
        let is_selected = vis.item_idx == menu.selected_idx;
        let (item_fg, item_bg) = if is_selected && vis.clickable {
            (bg, fg) // inverted
        } else if !vis.clickable {
            (dim_fg, bg)
        } else {
            (fg, bg)
        };
        for dx in 0..inner_w {
            set_cell(buf, inner_x + dx, row_y, ' ', item_fg, item_bg);
        }
        let label = item
            .label
            .spans
            .first()
            .map(|s| s.text.as_str())
            .unwrap_or("");
        for (i, ch) in label.chars().enumerate() {
            let col = inner_x + 1 + i as u16;
            if col >= inner_x + inner_w {
                break;
            }
            set_cell(buf, col, row_y, ch, item_fg, item_bg);
        }
        if let Some(ref det) = item.detail {
            let shortcut = det.spans.first().map(|s| s.text.as_str()).unwrap_or("");
            let sc_w = shortcut.chars().count() as u16;
            let sc_start = inner_x + inner_w.saturating_sub(sc_w + 1);
            let sc_fg = if is_selected && vis.clickable {
                item_fg
            } else {
                dim_fg
            };
            for (i, ch) in shortcut.chars().enumerate() {
                let col = sc_start + i as u16;
                if col >= inner_x + inner_w {
                    break;
                }
                set_cell(buf, col, row_y, ch, sc_fg, item_bg);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::context_menu::{ContextMenu, ContextMenuItem, ContextMenuItemMeasure};
    use crate::types::{StyledSpan, StyledText, WidgetId};
    use ratatui::layout::Rect;

    fn item(label: &str, clickable: bool) -> ContextMenuItem {
        ContextMenuItem {
            id: if clickable {
                Some(WidgetId::new(label))
            } else {
                None
            },
            label: StyledText {
                spans: vec![StyledSpan::plain(label)],
            },
            detail: None,
            disabled: !clickable,
        }
    }

    fn make_menu() -> ContextMenu {
        ContextMenu {
            id: WidgetId::new("menu"),
            items: vec![
                item("Open", true),
                item("Open to Side", true),
                // Separator: id = None.
                ContextMenuItem {
                    id: None,
                    label: StyledText {
                        spans: vec![StyledSpan::plain("")],
                    },
                    detail: None,
                    disabled: false,
                },
                item("Delete", true),
            ],
            selected_idx: 0,
            bg: None,
        }
    }

    fn cell_char(buf: &Buffer, x: u16, y: u16) -> char {
        buf[(x, y)].symbol().chars().next().unwrap_or(' ')
    }

    #[test]
    fn paints_corner_glyphs_and_items() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 30, 10));
        let menu = make_menu();
        let layout = menu.layout(
            2.0,
            1.0,
            crate::event::Rect::new(0.0, 0.0, 30.0, 10.0),
            20.0,
            |_| ContextMenuItemMeasure::new(1.0),
        );
        draw_context_menu(&mut buf, &menu, &layout, &Theme::default());

        // Border corners around the inner items region (inset by 1).
        let bx = layout.bounds.x.round() as u16 - 1;
        let by = layout.bounds.y.round() as u16 - 1;
        let bw = layout.bounds.width.round() as u16 + 2;
        let bh = layout.bounds.height.round() as u16 + 2;
        assert_eq!(cell_char(&buf, bx, by), '┌');
        assert_eq!(cell_char(&buf, bx + bw - 1, by), '┐');
        assert_eq!(cell_char(&buf, bx, by + bh - 1), '└');
        assert_eq!(cell_char(&buf, bx + bw - 1, by + bh - 1), '┘');
    }

    #[test]
    fn separator_paints_horizontal_dashes() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 30, 10));
        let menu = make_menu();
        let layout = menu.layout(
            2.0,
            1.0,
            crate::event::Rect::new(0.0, 0.0, 30.0, 10.0),
            20.0,
            |_| ContextMenuItemMeasure::new(1.0),
        );
        draw_context_menu(&mut buf, &menu, &layout, &Theme::default());

        // The third visible item is a separator — find a row that's all '─'.
        let mut found_sep_row = false;
        for vis in &layout.visible_items {
            if vis.is_separator {
                let row_y = vis.bounds.y.round() as u16;
                let inner_x = layout.bounds.x.round() as u16;
                let inner_w = layout.bounds.width.round() as u16;
                let row: String = (inner_x..inner_x + inner_w)
                    .map(|x| cell_char(&buf, x, row_y))
                    .collect();
                assert!(row.chars().all(|c| c == '─'), "separator row: {:?}", row);
                found_sep_row = true;
                break;
            }
        }
        assert!(found_sep_row, "expected at least one separator row");
    }

    #[test]
    fn selected_clickable_inverted() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 30, 10));
        let menu = make_menu(); // selected_idx = 0 → "Open"
        let layout = menu.layout(
            2.0,
            1.0,
            crate::event::Rect::new(0.0, 0.0, 30.0, 10.0),
            20.0,
            |_| ContextMenuItemMeasure::new(1.0),
        );
        let theme = Theme {
            tab_bar_bg: crate::types::Color::rgb(0, 0, 0),
            foreground: crate::types::Color::rgb(255, 255, 255),
            ..Theme::default()
        };
        draw_context_menu(&mut buf, &menu, &layout, &theme);

        // Find the "Open" row's first cell (inner_x). The selected row has
        // inverted bg = foreground colour.
        let inner_x = layout.bounds.x.round() as u16;
        let row_y = layout.visible_items[0].bounds.y.round() as u16;
        let bg = buf[(inner_x, row_y)].bg;
        assert_eq!(bg, ratatui::style::Color::Rgb(255, 255, 255));
    }

    #[test]
    fn zero_size_is_a_no_op() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 10));
        let menu = make_menu();
        let layout = menu.layout(
            0.0,
            0.0,
            crate::event::Rect::new(0.0, 0.0, 0.0, 0.0),
            0.0,
            |_| ContextMenuItemMeasure::new(0.0),
        );
        draw_context_menu(&mut buf, &menu, &layout, &Theme::default());
        assert_eq!(cell_char(&buf, 0, 0), ' ');
    }
}
