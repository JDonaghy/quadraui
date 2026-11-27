//! Cell painter for [`BoardModel`], written against [`CellSurface`].
//!
//! Paints each visible column's header row and its cards as rounded boxes:
//! a title row, a badge row and, when the card is tall enough, a hint row.
//! The selected card takes the accent border and the selected-card
//! background.

use crate::cell_surface::{solid_style, CellSurface};
use crate::primitives::board::{badge_fg_color, badge_icon, BoardLayout, BoardModel, CardBadge};
use crate::theme::Theme;
use crate::types::Color;

/// One cell in solid `fg` on `bg`.
fn put(cells: &mut dyn CellSurface, x: u16, y: u16, ch: char, fg: Color, bg: Color) {
    cells.put_char(x, y, ch, solid_style(fg, bg));
}

/// Paints `model` over `layout`, which [`BoardModel::cell_layout`]
/// computed.
pub(crate) fn paint(
    cells: &mut dyn CellSurface,
    model: &BoardModel,
    layout: &BoardLayout,
    theme: &Theme,
) {
    if layout.bounds.width < 1.0 || layout.bounds.height < 1.0 {
        return;
    }

    let fg = theme.surface_fg;
    let muted = theme.muted_fg;
    let header_bg = theme.board_col_header_bg;
    let header_fg = theme.header_fg;
    let selected_border = theme.accent_bg;
    let card_border = theme.border_fg;
    let card_bg = theme.surface_bg;
    let hint_bg = theme.card_hint_bg;
    let hint_fg = theme.card_hint_fg;

    for (li, col_layout) in layout.columns.iter().enumerate() {
        let col = &model.columns[col_layout.col_index];

        // ── Column header ────────────────────────────────────────────────
        let hx = col_layout.header_bounds.x.round() as u16;
        let hy = col_layout.header_bounds.y.round() as u16;
        let hw = col_layout.header_bounds.width.round() as u16;

        // Background fill for header.
        for dx in 0..hw {
            put(cells, hx + dx, hy, ' ', header_fg, header_bg);
        }

        // Column title, left-aligned, padded by 1.
        let title_chars: Vec<char> = col.title.chars().collect();
        let avail = hw.saturating_sub(2) as usize;
        for (i, ch) in title_chars.iter().take(avail).enumerate() {
            put(cells, hx + 1 + i as u16, hy, *ch, header_fg, header_bg);
        }

        // Column index indicator (helps when columns overflow viewport).
        if li == 0 && model.col_scroll_offset > 0 {
            // Show a "‹" to indicate there are columns to the left.
            if hw >= 1 {
                put(cells, hx + hw - 1, hy, '‹', muted, header_bg);
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
                theme.board_selected_card_bg
            } else {
                card_bg
            };

            // Draw rounded card border.
            draw_card_border(cells, bx, by, bw, bh, border_fg, text_bg);

            // ── Title row (row 1 inside the card) ───────────────────────
            if bh >= 2 {
                let title_row = by + 1;
                let inner_w = bw.saturating_sub(2) as usize;
                // Build label prefix from card labels (e.g. "#362 ").
                let prefix: String = if card.labels.is_empty() {
                    String::new()
                } else {
                    format!("{} ", card.labels.join(" "))
                };
                let full_title = format!("{}{}", prefix, card.title);
                // An over-long title is ellipsis-truncated to the card's
                // own inner width via the shared `truncate_with_ellipsis`
                // rather than clipped mid-glyph.
                let full_title = if full_title.chars().count() <= inner_w {
                    full_title
                } else {
                    crate::text_util::truncate_with_ellipsis(&full_title, inner_w as f32, |s| {
                        s.chars().count() as f32
                    })
                    .0
                };
                let title_chars: Vec<char> = full_title.chars().collect();
                for (i, ch) in title_chars.iter().take(inner_w).enumerate() {
                    let col_x = bx + 1 + i as u16;
                    if col_x < bx + bw.saturating_sub(1) {
                        let text_fg = if is_selected { fg } else { theme.surface_fg };
                        put(cells, col_x, title_row, *ch, text_fg, text_bg);
                    }
                }
                // Pad remaining cells with spaces.
                let written = title_chars.len().min(inner_w);
                for i in written..inner_w {
                    let col_x = bx + 1 + i as u16;
                    if col_x < bx + bw.saturating_sub(1) {
                        put(cells, col_x, title_row, ' ', fg, text_bg);
                    }
                }
            }

            // ── Badge row (row 2 inside the card) ───────────────────────
            if bh >= 3 {
                let badge_row = by + 2;
                let end_x = bx + bw.saturating_sub(1);
                let written = paint_badges(
                    cells,
                    bx + 1,
                    badge_row,
                    end_x,
                    &card.badges,
                    theme,
                    text_bg,
                );
                // Pad remaining with spaces.
                let mut col_x = written;
                while col_x < end_x {
                    put(cells, col_x, badge_row, ' ', fg, text_bg);
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
                                put(cells, col_x, hint_row, *ch, hint_fg, hint_bg);
                            }
                        }
                        // Pad remaining.
                        for i in written..inner_w {
                            let col_x = bx + 1 + i as u16;
                            if col_x < bx + bw.saturating_sub(1) {
                                put(cells, col_x, hint_row, ' ', hint_fg, hint_bg);
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Draw a rounded card border (╭ ╮ ╰ ╯) with the given border colour
/// and interior background.
fn draw_card_border(
    cells: &mut dyn CellSurface,
    bx: u16,
    by: u16,
    bw: u16,
    bh: u16,
    border_fg: Color,
    bg: Color,
) {
    // Top edge.
    put(cells, bx, by, '╭', border_fg, bg);
    for dx in 1..bw.saturating_sub(1) {
        put(cells, bx + dx, by, '─', border_fg, bg);
    }
    if bw >= 2 {
        put(cells, bx + bw - 1, by, '╮', border_fg, bg);
    }

    // Bottom edge.
    if bh >= 2 {
        let yb = by + bh - 1;
        put(cells, bx, yb, '╰', border_fg, bg);
        for dx in 1..bw.saturating_sub(1) {
            put(cells, bx + dx, yb, '─', border_fg, bg);
        }
        if bw >= 2 {
            put(cells, bx + bw - 1, yb, '╯', border_fg, bg);
        }
    }

    // Side edges + interior fill.
    for dy in 1..bh.saturating_sub(1) {
        put(cells, bx, by + dy, '│', border_fg, bg);
        if bw >= 2 {
            put(cells, bx + bw - 1, by + dy, '│', border_fg, bg);
        }
        for dx in 1..bw.saturating_sub(1) {
            put(cells, bx + dx, by + dy, ' ', border_fg, bg);
        }
    }
}

/// Paint the badge row in a single pass: `✓Passed ●Running ·Pending`
/// (icon + host-supplied label, space-separated), each badge coloured by
/// its status.
///
/// `CardBadge::label` is an arbitrary host-supplied string (see
/// `primitives::board` docs) — it is *not* guaranteed to be one
/// character — so this walks each badge's actual rendered width rather
/// than assuming a fixed per-badge stride. Returns the x position just past the last cell
/// written, so the caller can pad the remainder of the row.
fn paint_badges(
    cells: &mut dyn CellSurface,
    start_x: u16,
    row: u16,
    end_x: u16,
    badges: &[CardBadge],
    theme: &Theme,
    bg: Color,
) -> u16 {
    let mut x = start_x;
    for (i, badge) in badges.iter().enumerate() {
        if x >= end_x {
            break;
        }
        if i > 0 {
            put(cells, x, row, ' ', theme.muted_fg, bg);
            x += 1;
            if x >= end_x {
                break;
            }
        }
        let color = badge_fg_color(badge.status, theme);
        put(cells, x, row, badge_icon(badge.status), color, bg);
        x += 1;
        for ch in badge.label.chars() {
            if x >= end_x {
                break;
            }
            put(cells, x, row, ch, color, bg);
            x += 1;
        }
    }
    x
}
