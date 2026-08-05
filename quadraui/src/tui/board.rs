//! TUI rasteriser for [`crate::primitives::board::BoardModel`].
//!
//! Paints a horizontal strip of columns; each column has a title header
//! and a vertical stack of rounded card boxes. Each card shows:
//!
//! ```text
//! ╭────────────────────╮
//! │#362 Board           │
//! │✓P ●W ·T ·R ·M       │
//! │hint: use approach B │   ← BoardCard::hint (if present)
//! ╰────────────────────╯
//! ```
//!
//! The selected card gets an accent-coloured border instead of the
//! normal dim border.
//!
//! ## Colour mapping
//!
//! | BadgeStatus | Icon | Colour                         |
//! |-------------|------|---------------------------------|
//! | Passed      | ✓    | green (`theme.badge_passed`)    |
//! | Running     | ●    | yellow (`theme.badge_running`)  |
//! | Warning     | ↩    | orange (`theme.badge_warning`)  |
//! | Blocked     | ✗    | red (`theme.badge_blocked`)     |
//! | Pending     | ·    | muted (`theme.muted_fg`)        |

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color as RatatuiColor;

use super::{ratatui_color, set_cell};
use crate::primitives::board::{
    board_layout, BadgeStatus, BoardLayout, BoardMeasure, BoardModel, CardBadge,
};
use crate::theme::Theme;

/// Minimum column width in TUI cells.
pub(crate) const TUI_BOARD_COL_MIN_CELLS: f32 = 20.0;
/// Gap between adjacent columns in TUI cells.
const TUI_BOARD_COL_GAP: f32 = 1.0;
/// Column header height in TUI cells (one row for title).
const TUI_BOARD_HEADER_H: f32 = 1.0;
/// Card height in TUI cells: top_border + title + badge + hint + bottom_border = 5.
pub(crate) const TUI_BOARD_CARD_H: f32 = 5.0;
/// Vertical gap between cards in TUI cells.
const TUI_BOARD_CARD_GAP: f32 = 0.0;

/// Compute the TUI cell-unit layout for a [`BoardModel`] without painting.
pub fn tui_board_layout(model: &BoardModel, area: Rect) -> BoardLayout {
    board_layout(
        model,
        area.x as f32,
        area.y as f32,
        area.width as f32,
        area.height as f32,
        BoardMeasure::new(
            TUI_BOARD_COL_MIN_CELLS,
            TUI_BOARD_COL_GAP,
            TUI_BOARD_HEADER_H,
            TUI_BOARD_CARD_H,
            TUI_BOARD_CARD_GAP,
        ),
    )
}

/// Draw a [`BoardModel`] into `area` on `buf`. Returns the layout for
/// host click dispatch and selection-follow clamping.
pub fn draw_board(buf: &mut Buffer, area: Rect, model: &BoardModel, theme: &Theme) -> BoardLayout {
    let layout = tui_board_layout(model, area);

    if area.width == 0 || area.height == 0 {
        return layout;
    }

    let fg = ratatui_color(theme.surface_fg);
    let muted = ratatui_color(theme.muted_fg);
    let header_bg = ratatui_color(theme.board_col_header_bg);
    let header_fg = ratatui_color(theme.header_fg);
    let selected_border = ratatui_color(theme.accent_bg);
    let card_border = ratatui_color(theme.border_fg);
    let card_bg = ratatui_color(theme.surface_bg);
    let hint_bg = ratatui_color(theme.card_hint_bg);
    let hint_fg = ratatui_color(theme.card_hint_fg);

    for (li, col_layout) in layout.columns.iter().enumerate() {
        let col = &model.columns[col_layout.col_index];

        // ── Column header ────────────────────────────────────────────────
        let hx = col_layout.header_bounds.x.round() as u16;
        let hy = col_layout.header_bounds.y.round() as u16;
        let hw = col_layout.header_bounds.width.round() as u16;

        // Background fill for header.
        for dx in 0..hw {
            set_cell(buf, hx + dx, hy, ' ', header_fg, header_bg);
        }

        // Column title, left-aligned, padded by 1.
        let title_chars: Vec<char> = col.title.chars().collect();
        let avail = hw.saturating_sub(2) as usize;
        for (i, ch) in title_chars.iter().take(avail).enumerate() {
            set_cell(buf, hx + 1 + i as u16, hy, *ch, header_fg, header_bg);
        }

        // Column index indicator (helps when columns overflow viewport).
        if li == 0 && model.col_scroll_offset > 0 {
            // Show a "‹" to indicate there are columns to the left.
            if hw >= 1 {
                set_cell(buf, hx + hw - 1, hy, '‹', muted, header_bg);
            }
        }

        // ── Cards ────────────────────────────────────────────────────────
        for card_layout in &col_layout.cards {
            let card = &col.cards[card_layout.card_index];
            let is_selected = model
                .selected_card_id
                .as_ref()
                .map(|id| id == &card.id)
                .unwrap_or(false);

            let bx = card_layout.bounds.x.round() as u16;
            let by = card_layout.bounds.y.round() as u16;
            let bw = card_layout.bounds.width.round() as u16;
            let bh = card_layout.bounds.height.round() as u16;

            if bw == 0 || bh == 0 {
                continue;
            }

            let border_fg = if is_selected {
                selected_border
            } else {
                card_border
            };
            let text_bg = if is_selected {
                ratatui_color(theme.board_selected_card_bg)
            } else {
                card_bg
            };

            // Draw rounded card border.
            draw_card_border(buf, bx, by, bw, bh, border_fg, text_bg);

            // ── Title row (row 1 inside the card) ───────────────────────
            if bh >= 2 {
                let title_row = by + 1;
                let inner_w = bw.saturating_sub(2) as usize;
                // Build label prefix from `labels` (e.g. "#362 ").
                let prefix: String = if card.labels.is_empty() {
                    String::new()
                } else {
                    format!("{} ", card.labels.join(" "))
                };
                let full_title = format!("{}{}", prefix, card.title);
                let title_chars: Vec<char> = full_title.chars().collect();
                for (i, ch) in title_chars.iter().take(inner_w).enumerate() {
                    let col_x = bx + 1 + i as u16;
                    if col_x < bx + bw.saturating_sub(1) {
                        let text_fg = if is_selected {
                            fg
                        } else {
                            ratatui_color(theme.surface_fg)
                        };
                        set_cell(buf, col_x, title_row, *ch, text_fg, text_bg);
                    }
                }
                // Pad remaining cells with spaces.
                let written = title_chars.len().min(inner_w);
                for i in written..inner_w {
                    let col_x = bx + 1 + i as u16;
                    if col_x < bx + bw.saturating_sub(1) {
                        set_cell(buf, col_x, title_row, ' ', fg, text_bg);
                    }
                }
            }

            // ── Badge row (row 2 inside the card) ───────────────────────
            if bh >= 3 {
                let badge_row = by + 2;
                let end_x = bx + bw.saturating_sub(1);
                let written =
                    paint_badges(buf, bx + 1, badge_row, end_x, &card.badges, theme, text_bg);
                // Pad remaining with spaces.
                let mut col_x = written;
                while col_x < end_x {
                    set_cell(buf, col_x, badge_row, ' ', fg, text_bg);
                    col_x += 1;
                }
            }

            // ── Hint (row 3 inside the card, if present) ─────────────────
            // Requires bh >= 5: top_border(0) + title(1) + badge(2) + hint(3) + bottom(4).
            if bh >= 5 {
                if let Some(hint) = &card.hint {
                    let hint_row = by + 3;
                    // Check that the hint row is inside the card border.
                    if hint_row < by + bh.saturating_sub(1) {
                        let inner_w = bw.saturating_sub(2) as usize;
                        let hint_chars: Vec<char> = hint.chars().collect();
                        let written = hint_chars.len().min(inner_w);
                        for (i, ch) in hint_chars.iter().take(inner_w).enumerate() {
                            let col_x = bx + 1 + i as u16;
                            if col_x < bx + bw.saturating_sub(1) {
                                set_cell(buf, col_x, hint_row, *ch, hint_fg, hint_bg);
                            }
                        }
                        // Pad remaining.
                        for i in written..inner_w {
                            let col_x = bx + 1 + i as u16;
                            if col_x < bx + bw.saturating_sub(1) {
                                set_cell(buf, col_x, hint_row, ' ', hint_fg, hint_bg);
                            }
                        }
                    }
                }
            }
        }
    }

    layout
}

/// Draw a rounded card border (╭ ╮ ╰ ╯) with the given border colour
/// and interior background.
fn draw_card_border(
    buf: &mut Buffer,
    bx: u16,
    by: u16,
    bw: u16,
    bh: u16,
    border_fg: RatatuiColor,
    bg: RatatuiColor,
) {
    // Top edge.
    set_cell(buf, bx, by, '╭', border_fg, bg);
    for dx in 1..bw.saturating_sub(1) {
        set_cell(buf, bx + dx, by, '─', border_fg, bg);
    }
    if bw >= 2 {
        set_cell(buf, bx + bw - 1, by, '╮', border_fg, bg);
    }

    // Bottom edge.
    if bh >= 2 {
        let yb = by + bh - 1;
        set_cell(buf, bx, yb, '╰', border_fg, bg);
        for dx in 1..bw.saturating_sub(1) {
            set_cell(buf, bx + dx, yb, '─', border_fg, bg);
        }
        if bw >= 2 {
            set_cell(buf, bx + bw - 1, yb, '╯', border_fg, bg);
        }
    }

    // Side edges + interior fill.
    for dy in 1..bh.saturating_sub(1) {
        set_cell(buf, bx, by + dy, '│', border_fg, bg);
        if bw >= 2 {
            set_cell(buf, bx + bw - 1, by + dy, '│', border_fg, bg);
        }
        for dx in 1..bw.saturating_sub(1) {
            set_cell(buf, bx + dx, by + dy, ' ', border_fg, bg);
        }
    }
}

/// Return the icon character for a [`BadgeStatus`].
fn badge_icon(status: BadgeStatus) -> char {
    match status {
        BadgeStatus::Passed => '✓',
        BadgeStatus::Running => '●',
        BadgeStatus::Warning => '↩',
        BadgeStatus::Blocked => '✗',
        BadgeStatus::Pending => '·',
    }
}

/// Paint the badge row in a single pass: `✓Passed ●Running ·Pending`
/// (icon + host-supplied label, space-separated), each badge coloured by
/// its status.
///
/// `CardBadge::label` is an arbitrary host-supplied string (see
/// `primitives::board` docs) — it is *not* guaranteed to be one
/// character — so this walks each badge's actual rendered width rather
/// than assuming a fixed per-badge stride. Mirrors how `gtk/board.rs`
/// measures `pango_layout.pixel_size()` per badge and advances by the
/// real rendered width. Returns the x position just past the last cell
/// written, so the caller can pad the remainder of the row.
fn paint_badges(
    buf: &mut Buffer,
    start_x: u16,
    row: u16,
    end_x: u16,
    badges: &[CardBadge],
    theme: &Theme,
    bg: RatatuiColor,
) -> u16 {
    let mut x = start_x;
    for (i, badge) in badges.iter().enumerate() {
        if x >= end_x {
            break;
        }
        if i > 0 {
            set_cell(buf, x, row, ' ', ratatui_color(theme.muted_fg), bg);
            x += 1;
            if x >= end_x {
                break;
            }
        }
        let color = badge_color(badge.status, theme);
        set_cell(buf, x, row, badge_icon(badge.status), color, bg);
        x += 1;
        for ch in badge.label.chars() {
            if x >= end_x {
                break;
            }
            set_cell(buf, x, row, ch, color, bg);
            x += 1;
        }
    }
    x
}

/// Return the foreground colour for a badge icon.
fn badge_color(status: BadgeStatus, theme: &Theme) -> RatatuiColor {
    match status {
        BadgeStatus::Passed => ratatui_color(theme.badge_passed),
        BadgeStatus::Running => ratatui_color(theme.badge_running),
        BadgeStatus::Warning => ratatui_color(theme.badge_warning),
        BadgeStatus::Blocked => ratatui_color(theme.badge_blocked),
        BadgeStatus::Pending => ratatui_color(theme.muted_fg),
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::board::{BoardCard, BoardColumn, BoardHit, MoveDir};
    use crate::types::WidgetId;

    fn cell_char(buf: &Buffer, x: u16, y: u16) -> char {
        buf[(x, y)].symbol().chars().next().unwrap_or(' ')
    }

    fn make_card(id: &str, title: &str) -> BoardCard {
        BoardCard {
            id: WidgetId::new(id),
            title: title.to_string(),
            labels: vec!["#1".to_string()],
            badges: vec![
                CardBadge {
                    label: "P".into(),
                    status: BadgeStatus::Passed,
                },
                CardBadge {
                    label: "W".into(),
                    status: BadgeStatus::Running,
                },
                CardBadge {
                    label: "T".into(),
                    status: BadgeStatus::Pending,
                },
                CardBadge {
                    label: "R".into(),
                    status: BadgeStatus::Pending,
                },
                CardBadge {
                    label: "M".into(),
                    status: BadgeStatus::Pending,
                },
            ],
            hint: None,
        }
    }

    fn make_model() -> BoardModel {
        BoardModel {
            id: WidgetId::new("board"),
            columns: vec![
                BoardColumn {
                    id: WidgetId::new("col:backlog"),
                    title: "Backlog".to_string(),
                    cards: vec![make_card("card:1", "Card One")],
                    scroll_offset: 0,
                },
                BoardColumn {
                    id: WidgetId::new("col:done"),
                    title: "Done".to_string(),
                    cards: vec![],
                    scroll_offset: 0,
                },
            ],
            selected_card_id: None,
            col_scroll_offset: 0,
        }
    }

    // ── Paint round-trip tests ────────────────────────────────────────

    #[test]
    fn draws_without_panic_zero_size_is_noop() {
        let buf_area = Rect::new(0, 0, 60, 10);
        let mut buf = Buffer::empty(buf_area);
        let model = make_model();
        let area = Rect::new(0, 0, 0, 0);
        let _layout = draw_board(&mut buf, area, &model, &Theme::default());
        // Buffer should remain entirely empty (all spaces).
        assert_eq!(cell_char(&buf, 0, 0), ' ');
    }

    #[test]
    fn draws_column_header() {
        let area = Rect::new(0, 0, 60, 10);
        let mut buf = Buffer::empty(area);
        let model = make_model();
        draw_board(&mut buf, area, &model, &Theme::default());
        // Header is row 0; "Backlog" should start at col 1.
        assert_eq!(cell_char(&buf, 1, 0), 'B', "header must start with 'B'");
    }

    #[test]
    fn draws_card_border() {
        let area = Rect::new(0, 0, 60, 10);
        let mut buf = Buffer::empty(area);
        let model = make_model();
        draw_board(&mut buf, area, &model, &Theme::default());
        // Card starts at row 1 (after header). Top-left corner of card = ╭.
        assert_eq!(
            cell_char(&buf, 0, 1),
            '╭',
            "card top-left corner must be '╭'"
        );
    }

    #[test]
    fn draws_card_title() {
        let area = Rect::new(0, 0, 60, 10);
        let mut buf = Buffer::empty(area);
        let model = make_model();
        draw_board(&mut buf, area, &model, &Theme::default());
        // Title row is row 2 (border row 1 + title row 2). Check '#' from label.
        // The cell at (1, 2) should contain '#'.
        assert_eq!(cell_char(&buf, 1, 2), '#', "title row must start with '#'");
    }

    #[test]
    fn selected_card_gets_accent_border() {
        let area = Rect::new(0, 0, 60, 10);
        let mut buf_plain = Buffer::empty(area);
        let buf_selected = {
            let mut b = Buffer::empty(area);
            let mut model = make_model();
            model.selected_card_id = Some(WidgetId::new("card:1"));
            draw_board(&mut b, area, &model, &Theme::default());
            b
        };
        let model = make_model();
        draw_board(&mut buf_plain, area, &model, &Theme::default());

        // The border character in both is '╭' (same glyph), but the FG colour
        // should differ. Check that the FG of the top-left corner changed.
        let plain_fg = buf_plain[(0, 1)].fg;
        let sel_fg = buf_selected[(0, 1)].fg;
        assert_ne!(
            plain_fg, sel_fg,
            "selected card border colour must differ from unselected"
        );
    }

    // ── Layout / hit-test round-trip ──────────────────────────────────

    #[test]
    fn hit_test_card_round_trip() {
        let area = Rect::new(0, 0, 60, 10);
        let mut buf = Buffer::empty(area);
        let model = make_model();
        let layout = draw_board(&mut buf, area, &model, &Theme::default());

        // There's a card in column 0.
        let card_layout = &layout.columns[0].cards[0];
        let cx = card_layout.bounds.x + card_layout.bounds.width / 2.0;
        let cy = card_layout.bounds.y + card_layout.bounds.height / 2.0;
        match layout.hit_test(cx, cy) {
            BoardHit::Card(id) => assert_eq!(id.as_str(), "card:1"),
            other => panic!("expected Card hit, got {other:?}"),
        }
    }

    #[test]
    fn hit_test_column_header_round_trip() {
        let area = Rect::new(0, 0, 60, 10);
        let mut buf = Buffer::empty(area);
        let model = make_model();
        let layout = draw_board(&mut buf, area, &model, &Theme::default());

        let hdr = layout.columns[0].header_bounds;
        match layout.hit_test(hdr.x + 1.0, hdr.y + 0.5) {
            BoardHit::ColumnHeader(id) => assert_eq!(id.as_str(), "col:backlog"),
            other => panic!("expected ColumnHeader hit, got {other:?}"),
        }
    }

    #[test]
    fn hit_test_miss_returns_empty() {
        let area = Rect::new(0, 0, 60, 10);
        let mut buf = Buffer::empty(area);
        let model = make_model();
        let layout = draw_board(&mut buf, area, &model, &Theme::default());
        assert_eq!(layout.hit_test(900.0, 900.0), BoardHit::Empty);
    }

    #[test]
    fn visible_cards_per_column_matches_layout() {
        let area = Rect::new(0, 0, 60, 10);
        let mut buf = Buffer::empty(area);
        let model = make_model();
        let layout = draw_board(&mut buf, area, &model, &Theme::default());
        // With card_height=5, header=1, area height=10 → body=9 → floor(9/5)=1
        // But there is only 1 card in col 0, so cards.len() == 1.
        assert!(layout.columns[0].visible_cards >= 1);
        assert_eq!(layout.columns[0].cards.len(), 1);
    }

    #[test]
    fn hint_drawn_when_present() {
        // Card height = 5 rows:
        //   row 0 (card_y + 0 = 1): ╭ top border
        //   row 1 (card_y + 1 = 2): title
        //   row 2 (card_y + 2 = 3): badge row
        //   row 3 (card_y + 3 = 4): hint  ← assert here
        //   row 4 (card_y + 4 = 5): ╰ bottom border
        // Header occupies row 0, so card_y = 1 and hint_row = 4.
        let area = Rect::new(0, 0, 60, 10);
        let mut buf = Buffer::empty(area);
        let mut model = make_model();
        model.columns[0].cards[0].hint = Some("use plan B".to_string());
        draw_board(&mut buf, area, &model, &Theme::default());
        // Cell (1, 4) should hold 'u', the first character of "use plan B".
        assert_eq!(
            cell_char(&buf, 1, 4),
            'u',
            "hint row (row 4) must contain first char of hint"
        );
    }

    #[test]
    fn multi_char_badge_labels_do_not_corrupt_neighbouring_cells() {
        // Regression test for #476: paint_badges must track each badge's
        // actual rendered width instead of assuming a fixed "icon + 1
        // char" stride (a leftover from when `label` was always exactly
        // one character), or a later badge's icon paints over an earlier
        // multi-char label's trailing cells.
        let area = Rect::new(0, 0, 60, 10);
        let mut buf = Buffer::empty(area);
        let mut model = make_model();
        model.columns[0].cards[0].badges = vec![
            CardBadge {
                label: "Mix".into(),
                status: BadgeStatus::Passed,
            },
            CardBadge {
                label: "Proof".into(),
                status: BadgeStatus::Running,
            },
        ];
        draw_board(&mut buf, area, &model, &Theme::default());

        // Badge row is row 3 (header=0, card top border=1, title=2, badge=3).
        let badge_row = 3;
        let expected = "✓Mix ●Proof";
        for (i, ch) in expected.chars().enumerate() {
            let x = 1 + i as u16;
            assert_eq!(
                cell_char(&buf, x, badge_row),
                ch,
                "cell ({x}, {badge_row}) mismatch: expected badge text {expected:?} intact"
            );
        }

        // Each badge must be coloured by its own status, at its own real
        // offset, not the fixed 3-cells-per-badge offset a single-char
        // assumption would compute.
        let theme = Theme::default();
        let mix_last_char_x = 4; // 'x' in "Mix" — must survive untouched.
        let proof_icon_x = 6; // '●' — the old stride math would have put
                              // this at x=4, clobbering "Mix".
        assert_eq!(
            buf[(mix_last_char_x, badge_row)].fg,
            ratatui_color(theme.badge_passed),
            "'Mix' label cells must keep the first badge's colour"
        );
        assert_eq!(
            cell_char(&buf, proof_icon_x, badge_row),
            '●',
            "second badge's icon must land past the full 'Mix' label"
        );
        assert_eq!(
            buf[(proof_icon_x, badge_row)].fg,
            ratatui_color(theme.badge_running),
            "second badge's icon must be coloured by its own status"
        );
    }

    // ── Keyboard handling ─────────────────────────────────────────────

    #[test]
    fn handle_key_j_via_model() {
        use crate::primitives::board::BoardAction;
        use crate::types::Modifiers;
        let model = make_model();
        assert_eq!(
            model.handle_key("j", Modifiers::default()),
            Some(BoardAction::MoveSelection(MoveDir::Down))
        );
    }
}
