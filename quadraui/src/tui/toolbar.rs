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

use super::{cell_width, qc, set_cell};
use crate::primitives::toolbar::{Toolbar, ToolbarButton, ToolbarItemMeasure, ToolbarLayout};
use crate::theme::Theme;
use crate::types::WidgetId;

/// Cell width of a single character, accounting for double-width
/// glyphs (CJK, Nerd Font PUA). Tiny wrapper over `super::cell_width`
/// kept here so the toolbar's measurement is colocated with its paint.
fn char_cells(c: char) -> usize {
    cell_width(c) as usize
}

/// Cell width of a `&str` using UAX#11 double-width detection.
fn str_cells(s: &str) -> usize {
    s.chars().map(char_cells).sum()
}

/// Compute the TUI cell-unit width of a single toolbar item.
///
/// Action layout:
/// - icon-only (label empty): `[ {icon} ]` = 4 + icon_w cells
/// - normal: `[ {icon }{label}{ (key)} ]` = 4 + icon_block + label_w + hint_block
///
/// Separator: 2 cells (` │`). Label: text width (double-width aware).
pub(crate) fn tui_item_width(btn: &ToolbarButton) -> f32 {
    match btn {
        ToolbarButton::Action {
            label,
            icon,
            key_hint,
            ..
        } => {
            let label_w = str_cells(label);
            let icon_w = icon.as_ref().map(|s| str_cells(s)).unwrap_or(0);
            // Icon-only: drop the trailing space after the icon — pack
            // glyph snug between the brackets.
            let icon_block = if icon.is_some() && label_w > 0 {
                icon_w + 1
            } else {
                icon_w
            };
            let hint_w = key_hint
                .as_ref()
                .map(|s| str_cells(s) + 3) // " (xxx)"
                .unwrap_or(0);
            // "[ " + icon_block + label + hint_w + " ]"
            (4 + icon_block + label_w + hint_w) as f32
        }
        ToolbarButton::Separator => 2.0,
        ToolbarButton::Label { text, .. } => str_cells(text) as f32,
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

    // Vertically centre text on rects taller than 1 row. With even
    // heights the row above centre wins (matches GTK's
    // line_height-based centring).
    let text_row = area.y + area.height.saturating_sub(1) / 2;

    // Fill every row of the bar background so multi-row hosts (Gap 1)
    // get a uniform-coloured strip rather than 1 row painted + N-1
    // rows of stale buffer.
    for dy in 0..area.height {
        for dx in 0..area.width {
            set_cell(buf, area.x + dx, area.y + dy, ' ', fg, bar_bg);
        }
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

                // Build the rendered cells. Icon-only buttons compact
                // to `[ icon ]` (no trailing space after the icon).
                let label_w = str_cells(label);
                let icon_only = icon.is_some() && label_w == 0;
                let mut cells: Vec<char> = Vec::with_capacity(item_w as usize);
                cells.push('[');
                cells.push(' ');
                if let Some(icon) = icon {
                    for ch in icon.chars() {
                        cells.push(ch);
                    }
                    if !icon_only {
                        cells.push(' ');
                    }
                }
                for ch in label.chars() {
                    cells.push(ch);
                }
                if let Some(hint) = key_hint {
                    cells.push(' ');
                    cells.push('(');
                    for ch in hint.chars() {
                        cells.push(ch);
                    }
                    cells.push(')');
                }
                cells.push(' ');
                cells.push(']');

                // Paint background across every row of the item so the
                // hover/active highlight reads as one solid button on
                // multi-row toolbars (Gap 1).
                for dy in 0..area.height {
                    for dx in 0..item_w {
                        set_cell(buf, item_x + dx, area.y + dy, ' ', cell_fg, cell_bg);
                    }
                }

                // Paint text on the vertically-centred row, advancing
                // by the wide-char-aware cell width of each glyph.
                let mut col: u16 = 0;
                for ch in cells {
                    let cw = char_cells(ch) as u16;
                    if col + cw > item_w {
                        break;
                    }
                    set_cell(buf, item_x + col, text_row, ch, cell_fg, cell_bg);
                    col += cw;
                }
            }
            ToolbarButton::Separator => {
                // 2 cells: ` │`, drawn through every row of the bar so
                // the divider reads as a single full-height rule on
                // multi-row toolbars.
                for dy in 0..area.height {
                    if item_w >= 1 {
                        set_cell(buf, item_x, area.y + dy, ' ', muted, bar_bg);
                    }
                    if item_w >= 2 {
                        set_cell(buf, item_x + 1, area.y + dy, '│', muted, bar_bg);
                    }
                }
            }
            ToolbarButton::Label { text, fg: label_fg } => {
                let color = label_fg.map(qc).unwrap_or(muted);
                let mut col: u16 = 0;
                for ch in text.chars() {
                    let cw = char_cells(ch) as u16;
                    if col + cw > item_w {
                        break;
                    }
                    set_cell(buf, item_x + col, text_row, ch, color, bar_bg);
                    col += cw;
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

    // ── Gap 1: multi-row rasteriser ─────────────────────────────────────

    #[test]
    fn multi_row_fills_background_across_all_rows() {
        // 3-row bar with a single action button. Every cell of every
        // row must be painted (not just row 0).
        let area = Rect::new(0, 0, 12, 3);
        let mut buf = Buffer::empty(area);
        let bar = Toolbar {
            id: WidgetId::new("tb"),
            buttons: vec![mk_action("a", "Go", true)],
            bg: None,
        };
        let _ = draw_toolbar(&mut buf, area, &bar, &Theme::default(), None, None);

        // Row 0 (top): button text painted in centre — but the bg
        // outside the button text still fills.
        // Row 1 (middle): button text is here (centred vertically).
        // Row 2 (bottom): background fill only.
        //
        // Pre-Gap-1 behaviour would leave row 1 and row 2 untouched
        // (default Buffer cell symbol = " " but with default colours
        // not the bar's). Use any non-text cell (last column past the
        // button) — it should carry the bar bg every row.
        for y in 0..area.height {
            let cell = &buf[(area.width - 1, y)];
            assert_eq!(
                cell.symbol(),
                " ",
                "row {} last column not background-filled",
                y
            );
        }
    }

    #[test]
    fn multi_row_centres_button_text_vertically() {
        // Height 3 → text on row 1 (middle).
        let area = Rect::new(0, 0, 12, 3);
        let mut buf = Buffer::empty(area);
        let bar = Toolbar {
            id: WidgetId::new("tb"),
            buttons: vec![mk_action("a", "Go", true)],
            bg: None,
        };
        let _ = draw_toolbar(&mut buf, area, &bar, &Theme::default(), None, None);
        // Row 1 col 0 should be `[`.
        assert_eq!(cell_char(&buf, 0, 1), '[');
        // Rows 0 and 2 should not have the bracket.
        assert_ne!(cell_char(&buf, 0, 0), '[');
        assert_ne!(cell_char(&buf, 0, 2), '[');
    }

    #[test]
    fn multi_row_click_in_any_row_hits_button() {
        let area = Rect::new(0, 0, 20, 3);
        let mut buf = Buffer::empty(area);
        let bar = Toolbar {
            id: WidgetId::new("tb"),
            buttons: vec![mk_action("a", "Go", true)],
            bg: None,
        };
        let layout = draw_toolbar(&mut buf, area, &bar, &Theme::default(), None, None);
        let r = layout.visible_items[0].bounds;
        // Click in each row of the button bounds — every row should
        // resolve to the button.
        for dy in 0..3 {
            let hit = layout.hit_test(r.x + 1.0, r.y + dy as f32);
            match hit {
                ToolbarHit::Button(ref id) => assert_eq!(id.as_str(), "a"),
                _ => panic!("row {dy} did not hit button"),
            }
        }
    }

    #[test]
    fn multi_row_separator_extends_full_height() {
        let area = Rect::new(0, 0, 10, 3);
        let mut buf = Buffer::empty(area);
        let bar = Toolbar {
            id: WidgetId::new("tb"),
            buttons: vec![ToolbarButton::Separator],
            bg: None,
        };
        let _ = draw_toolbar(&mut buf, area, &bar, &Theme::default(), None, None);
        // The │ glyph should appear on every row at column 1.
        for y in 0..area.height {
            assert_eq!(cell_char(&buf, 1, y), '│', "row {y} missing pipe");
        }
    }

    // ── Gap 4: icon-only buttons + wide-glyph icons ─────────────────────

    #[test]
    fn icon_only_button_compacts_to_three_plus_icon() {
        // Icon-only button: `[ X ]` = 5 cells (4 wrapper + 1-cell icon).
        // The pre-Gap-4 layout would emit `[ X  ]` = 6 cells (trailing
        // space after the icon).
        let area = Rect::new(0, 0, 20, 1);
        let mut buf = Buffer::empty(area);
        let bar = Toolbar {
            id: WidgetId::new("tb"),
            buttons: vec![ToolbarButton::Action {
                id: WidgetId::new("a"),
                label: "".into(),
                icon: Some("X".into()),
                key_hint: None,
                enabled: true,
                is_active: false,
                tooltip: String::new(),
            }],
            bg: None,
        };
        let layout = draw_toolbar(&mut buf, area, &bar, &Theme::default(), None, None);
        assert_eq!(layout.visible_items[0].bounds.width, 5.0);
        // Painted: `[ X ]`
        assert_eq!(cell_char(&buf, 0, 0), '[');
        assert_eq!(cell_char(&buf, 1, 0), ' ');
        assert_eq!(cell_char(&buf, 2, 0), 'X');
        assert_eq!(cell_char(&buf, 3, 0), ' ');
        assert_eq!(cell_char(&buf, 4, 0), ']');
    }

    #[test]
    fn double_width_icon_reserves_two_cells() {
        // CJK or emoji glyphs occupy 2 cells. The measurer must
        // reserve that width so the right bracket lands at the
        // expected column. Use 一 (U+4E00) — explicit 2-cell width.
        let area = Rect::new(0, 0, 20, 1);
        let mut buf = Buffer::empty(area);
        let bar = Toolbar {
            id: WidgetId::new("tb"),
            buttons: vec![ToolbarButton::Action {
                id: WidgetId::new("a"),
                label: "Go".into(),
                icon: Some("一".into()),
                key_hint: None,
                enabled: true,
                is_active: false,
                tooltip: String::new(),
            }],
            bg: None,
        };
        let layout = draw_toolbar(&mut buf, area, &bar, &Theme::default(), None, None);
        // "[ " (2) + icon (2 cells) + " " (1) + "Go" (2) + " ]" (2) = 9
        assert_eq!(layout.visible_items[0].bounds.width, 9.0);
    }
}
