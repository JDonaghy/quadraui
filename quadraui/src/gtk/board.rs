//! GTK (Cairo + Pango) rasteriser for [`crate::primitives::board::BoardModel`].
//!
//! Paints columns side by side using Cairo rounded rectangles. Each column
//! has a header strip and a vertical stack of card boxes. Cards show the
//! issue title, an inline badge icon row, and an optional `BoardCard::hint`
//! callout strip.
//!
//! ## Layout constants
//!
//! All pixel values are logical pixels (scaled by the display DPI via Pango /
//! Cairo). The TUI equivalent uses cell units with the same semantic roles.

use gtk4::cairo::Context;
use gtk4::pango;

use super::{rounded_rect_path, set_source};
use crate::primitives::board::{board_layout, BadgeStatus, BoardLayout, BoardMeasure, BoardModel};
use crate::theme::Theme;
use crate::types::Color;

/// Minimum column width in pixels.
pub(crate) const GTK_BOARD_COL_MIN_PX: f32 = 200.0;
/// Gap between adjacent columns in pixels.
const GTK_BOARD_COL_GAP_PX: f32 = 8.0;
/// Column header height in pixels.
const GTK_BOARD_HEADER_H_PX: f32 = 24.0;
/// Card height in pixels (title + badge + optional hint).
const GTK_BOARD_CARD_H_PX: f32 = 64.0;
/// Vertical gap between cards in pixels.
const GTK_BOARD_CARD_GAP_PX: f32 = 6.0;
/// Corner radius for card boxes.
const CARD_CORNER_RADIUS: f64 = 4.0;
/// Horizontal text padding inside a card.
const CARD_H_PAD: f64 = 8.0;
/// Font size for card title text (in Pango units = 1024 * pt).
const TITLE_FONT_SIZE: f64 = 11.0;
/// Font size for badge text.
const BADGE_FONT_SIZE: f64 = 9.0;
/// Font size for hint text.
const HINT_FONT_SIZE: f64 = 9.0;

/// Compute the GTK pixel-unit layout for a [`BoardModel`] without painting.
pub fn gtk_board_layout(model: &BoardModel, x: f64, y: f64, w: f64, h: f64) -> BoardLayout {
    board_layout(
        model,
        x as f32,
        y as f32,
        w as f32,
        h as f32,
        BoardMeasure::new(
            GTK_BOARD_COL_MIN_PX,
            GTK_BOARD_COL_GAP_PX,
            GTK_BOARD_HEADER_H_PX,
            GTK_BOARD_CARD_H_PX,
            GTK_BOARD_CARD_GAP_PX,
        ),
    )
}

/// Draw a [`BoardModel`] onto `cr`. Returns the layout for host click
/// dispatch and selection-follow clamping.
///
/// # Arguments
/// * `cr` — Cairo context (active draw pass only).
/// * `pango_layout` — shared Pango layout for text measurement.
/// * `x`, `y`, `w`, `h` — widget bounds in logical pixels.
/// * `model` — the board data (host-owned state).
/// * `theme` — active colour palette.
#[allow(clippy::too_many_arguments)]
pub fn draw_board(
    cr: &Context,
    pango_layout: &pango::Layout,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    model: &BoardModel,
    theme: &Theme,
) -> BoardLayout {
    let layout = gtk_board_layout(model, x, y, w, h);

    if w <= 0.0 || h <= 0.0 {
        return layout;
    }

    for col_layout in &layout.columns {
        let col = &model.columns[col_layout.col_index];

        // ── Column header ────────────────────────────────────────────────
        let hb = col_layout.header_bounds;
        set_source(cr, theme.board_col_header_bg);
        cr.rectangle(hb.x as f64, hb.y as f64, hb.width as f64, hb.height as f64);
        let _ = cr.fill();

        // Header title text.
        pango_layout.set_text(&col.title);
        set_pango_size(pango_layout, TITLE_FONT_SIZE);
        set_source(cr, theme.header_fg);
        cr.move_to(hb.x as f64 + CARD_H_PAD, hb.y as f64 + 4.0);
        super::painted_text::show_layout(cr, pango_layout);

        // ── Cards ────────────────────────────────────────────────────────
        for card_layout in &col_layout.cards {
            let card = &col.cards[card_layout.card_index];
            let is_selected = model
                .selected_card_id
                .as_ref()
                .map(|id| id == &card.id)
                .unwrap_or(false);

            let cb = card_layout.bounds;
            let bx = cb.x as f64;
            let by = cb.y as f64;
            let bw = cb.width as f64;
            let bh = cb.height as f64;

            if bw <= 0.0 || bh <= 0.0 {
                continue;
            }

            // Card background.
            let card_bg = if is_selected {
                theme.board_selected_card_bg
            } else {
                theme.surface_bg
            };
            set_source(cr, card_bg);
            rounded_rect_path(cr, bx, by, bw, bh, CARD_CORNER_RADIUS);
            let _ = cr.fill();

            // Card border.
            let border_col = if is_selected {
                theme.accent_bg
            } else {
                theme.border_fg
            };
            set_source(cr, border_col);
            cr.set_line_width(1.0);
            rounded_rect_path(cr, bx, by, bw, bh, CARD_CORNER_RADIUS);
            let _ = cr.stroke();

            let text_fg = theme.surface_fg;

            // ── Title line ───────────────────────────────────────────────
            let prefix = if card.labels.is_empty() {
                String::new()
            } else {
                format!("{} ", card.labels.join(" "))
            };
            let full_title = format!("{}{}", prefix, card.title);
            pango_layout.set_text(&full_title);
            set_pango_size(pango_layout, TITLE_FONT_SIZE);
            set_pango_width(pango_layout, (bw - CARD_H_PAD * 2.0) as f32);
            set_source(cr, text_fg);
            cr.move_to(bx + CARD_H_PAD, by + 6.0);
            super::painted_text::show_layout(cr, pango_layout);

            // ── Badge row ────────────────────────────────────────────────
            let badge_y = by + 26.0;
            let mut badge_x = bx + CARD_H_PAD;
            for badge in &card.badges {
                let icon = badge_icon(badge.status);
                let badge_str = format!("{}{} ", icon, badge.label);
                pango_layout.set_text(&badge_str);
                set_pango_size(pango_layout, BADGE_FONT_SIZE);
                set_pango_width(pango_layout, -1.0);
                let col = badge_fg_color(badge.status, theme);
                set_source(cr, col);
                cr.move_to(badge_x, badge_y);
                super::painted_text::show_layout(cr, pango_layout);
                let (pw, _) = pango_layout.pixel_size();
                badge_x += pw as f64;
                if badge_x > bx + bw - CARD_H_PAD {
                    break;
                }
            }

            // ── Hint ─────────────────────────────────────────────────────
            if let Some(hint) = &card.hint {
                let hint_y = by + bh - 18.0;
                if hint_y > badge_y + 10.0 {
                    // Background strip.
                    set_source(cr, theme.card_hint_bg);
                    cr.rectangle(bx + 2.0, hint_y - 2.0, bw - 4.0, 14.0);
                    let _ = cr.fill();
                    // Text.
                    pango_layout.set_text(hint);
                    set_pango_size(pango_layout, HINT_FONT_SIZE);
                    set_pango_width(pango_layout, (bw - CARD_H_PAD * 2.0) as f32);
                    set_source(cr, theme.card_hint_fg);
                    cr.move_to(bx + CARD_H_PAD, hint_y);
                    super::painted_text::show_layout(cr, pango_layout);
                }
            }
        }
    }

    layout
}

/// Return the icon char for a badge status.
fn badge_icon(status: BadgeStatus) -> char {
    match status {
        BadgeStatus::Passed => '✓',
        BadgeStatus::Running => '●',
        BadgeStatus::Warning => '↩',
        BadgeStatus::Blocked => '✗',
        BadgeStatus::Pending => '·',
    }
}

/// Return the foreground colour for a badge icon.
fn badge_fg_color(status: BadgeStatus, theme: &Theme) -> Color {
    match status {
        BadgeStatus::Passed => theme.badge_passed,
        BadgeStatus::Running => theme.badge_running,
        BadgeStatus::Warning => theme.badge_warning,
        BadgeStatus::Blocked => theme.badge_blocked,
        BadgeStatus::Pending => theme.muted_fg,
    }
}

/// Set the font size on a Pango layout (in points).
fn set_pango_size(layout: &pango::Layout, size_pt: f64) {
    if let Some(mut desc) = layout.font_description() {
        desc.set_size((size_pt * pango::SCALE as f64) as i32);
        layout.set_font_description(Some(&desc));
    }
}

/// Set the maximum width for a Pango layout (in pixels; -1 = unlimited).
fn set_pango_width(layout: &pango::Layout, width_px: f32) {
    if width_px < 0.0 {
        layout.set_width(-1);
    } else {
        layout.set_width((width_px * pango::SCALE as f32) as i32);
    }
}
