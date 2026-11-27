//! TUI entry point for [`crate::primitives::board::BoardModel`]; the cell
//! painter is the widget's own (`primitives/board/paint_cells.rs`).
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

#[cfg(test)]
use super::ratatui_color;
use super::BufferCellSurface;
#[cfg(test)]
use crate::primitives::board::CardBadge;
use crate::primitives::board::{BoardLayout, BoardModel};
use crate::theme::Theme;

/// Compute the TUI cell-unit layout for a [`BoardModel`] without painting
/// — [`BoardModel::cell_layout`].
pub fn tui_board_layout(model: &BoardModel, area: Rect) -> BoardLayout {
    model.cell_layout(crate::event::Rect::new(
        area.x as f32,
        area.y as f32,
        area.width as f32,
        area.height as f32,
    ))
}

/// Paints `model` into `area` and returns the layout for host click
/// dispatch and selection-follow clamping.
pub fn draw_board(buf: &mut Buffer, area: Rect, model: &BoardModel, theme: &Theme) -> BoardLayout {
    let layout = tui_board_layout(model, area);
    crate::primitives::board::paint_cells::paint(
        &mut BufferCellSurface::new(buf),
        model,
        &layout,
        theme,
    );
    layout
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::board::{BadgeStatus, BoardCard, BoardColumn, BoardHit, MoveDir};
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
    fn overlong_card_title_ellipsizes_instead_of_clipping_mid_glyph() {
        let area = Rect::new(0, 0, 60, 10);
        let mut buf = Buffer::empty(area);
        let mut model = make_model();
        model.columns[0].cards[0].title = "y".repeat(50);
        model.columns[0].cards[0].labels = vec![];
        let layout = draw_board(&mut buf, area, &model, &Theme::default());

        let cb = layout.columns[0].cards[0].bounds;
        let bx = cb.x.round() as u16;
        let by = cb.y.round() as u16;
        let bw = cb.width.round() as u16;
        let title_row = by + 1;
        let row: String = (bx + 1..bx + bw.saturating_sub(1))
            .map(|x| cell_char(&buf, x, title_row))
            .collect();
        assert!(
            row.trim_end().ends_with('…'),
            "overlong card title must end in an ellipsis, got {row:?}"
        );
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

    /// Shared body for `hit_test_card_round_trip[_at_nonzero_origin]`:
    /// `tui_board_layout` bakes `area.x`/`area.y` into the returned bounds
    /// (absolute frame, see `board_layout`'s `origin_x`/`origin_y`
    /// threading), so a click must resolve correctly at a non-zero origin
    /// too — not just at `(0, 0)`, where the LESSONS.md "layout helpers
    /// must return coords in the same frame across backends" bug class is
    /// invisible.
    fn hit_test_card_round_trip_at(origin_x: u16, origin_y: u16) {
        let area = Rect::new(origin_x, origin_y, 60, 10);
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
    fn hit_test_card_round_trip() {
        hit_test_card_round_trip_at(0, 0);
    }

    #[test]
    fn hit_test_card_round_trip_at_nonzero_origin() {
        hit_test_card_round_trip_at(7, 13);
    }

    /// Shared body for `hit_test_column_header_round_trip[_at_nonzero_origin]`
    /// — see `hit_test_card_round_trip_at` for the non-zero-origin rationale.
    fn hit_test_column_header_round_trip_at(origin_x: u16, origin_y: u16) {
        let area = Rect::new(origin_x, origin_y, 60, 10);
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
    fn hit_test_column_header_round_trip() {
        hit_test_column_header_round_trip_at(0, 0);
    }

    #[test]
    fn hit_test_column_header_round_trip_at_nonzero_origin() {
        hit_test_column_header_round_trip_at(7, 13);
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
        // paint_badges must track each badge's actual rendered width
        // instead of assuming a fixed "icon + 1 char" stride, or a
        // later badge's icon paints over an earlier multi-char label's
        // trailing cells.
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

    /// The widget's contract cell paint reaches the buffer through the
    /// `CellSurface` verbs alone and paints what the TUI entry point does.
    #[test]
    fn contract_cell_paint_needs_only_the_verbs() {
        use crate::primitive::{LayoutCtx, Primitive};
        let w = make_model();
        let theme = Theme::default();
        let style = crate::Style::default();
        let area = Rect::new(0, 0, 72, 14);
        let mut legacy = Buffer::empty(area);
        draw_board(&mut legacy, area, &w, &theme);
        let mut buf = Buffer::empty(area);
        let rect = crate::event::Rect::new(
            area.x as f32,
            area.y as f32,
            area.width as f32,
            area.height as f32,
        );
        let layout = Primitive::layout(&w, &LayoutCtx::new(1.0, 1.0, false), rect);
        w.paint_cells(
            &layout,
            &mut crate::tui::cell_surface::VerbsOnly(super::BufferCellSurface::new(&mut buf)),
            &crate::PaintEnv::new(&theme, &style),
        );
        assert_eq!(buf, legacy);
    }
}
