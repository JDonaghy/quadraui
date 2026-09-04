//! Direct2D / DirectWrite rasteriser for
//! [`crate::primitives::board::BoardModel`] (#736).
//!
//! Mirrors `gtk::board`/`macos::board`'s structure: [`board_layout`] (the
//! shared layout API) resolves every column's header/body bounds and each
//! card's box bounds — this module only measures (using the primitive's
//! shared `BOARD_*_PX` constants directly; a DIP is a pixel at 100% display
//! scale, the same numeric unit `gtk`/`macos` call these constants in, so
//! reusing them here avoids an eighth copy of values #736 just finished
//! deduplicating down to one) and paints via
//! [`DWrite::draw_text`]/[`fill_rect`]/[`stroke_rect`].
//!
//! The badge icon/colour tables are **not** duplicated here either. They
//! lived three times over (gtk, macos, tui) before #736; #713's
//! primitive-first rule forbids a fourth copy, so this rasteriser calls
//! [`crate::primitives::board::badge_icon`] /
//! [`crate::primitives::board::badge_fg_color`] instead.
//!
//! No rounded-rect helper exists in `win::text` (see `win::pipeline_view`'s
//! and `win::toolbar`'s module docs for the same note — Direct2D needs an
//! `ID2D1RoundedRectangleGeometry` for a rounded one), so card boxes paint
//! as straight-edged rectangles via [`stroke_rect`] rather than GTK/macOS's
//! rounded-rect pill. Hit-test bounds and click routing are unaffected —
//! [`BoardLayout`] carries only rectangles, and the visual corner radius is
//! not part of its contract (mirrors `win::pipeline_view`'s same note,
//! and why `BOARD_CARD_CORNER_RADIUS_PX` is never imported here).
//!
//! Like `macos::board` (see that module's "Divergence from the GTK twin"
//! doc), every element paints with the backend's single configured
//! [`DWrite`] text format rather than GTK's three distinct Pango sizes —
//! DirectWrite text formats aren't cheap to vary per call without a format
//! cache no Win-GUI rasteriser has yet. Overlong text is clipped by
//! [`DWrite::draw_text`]'s own `D2D1_DRAW_TEXT_OPTIONS_CLIP` rather than
//! ellipsised — no explicit `push_clip`/`pop_clip` bracket is needed the
//! way `macos::board` needs `CGContextClipToRect`.
//!
//! Only compiled on `target_os = "windows"` — see `super::mod`'s
//! `#[cfg(target_os = "windows")] mod board;` and `backend.rs`'s module
//! docs for why the rest of this repo's `--features win` compile gate
//! stays meaningful without a Windows host.

use windows::Win32::Graphics::Direct2D::ID2D1RenderTarget;

use super::text::{fill_rect, stroke_rect, DWrite};
use crate::event::Rect;
use crate::primitives::board::{
    badge_fg_color, badge_icon, board_layout, BoardLayout, BoardMeasure, BoardModel,
    BOARD_CARD_GAP_PX, BOARD_CARD_H_PAD_PX, BOARD_CARD_H_PX, BOARD_COL_GAP_PX, BOARD_COL_MIN_PX,
    BOARD_HEADER_H_PX,
};
use crate::theme::Theme;

/// Border stroke width for card boxes, in DIPs. Same value as
/// `macos::board::CARD_BORDER_W`; kept backend-local since it, like the
/// offset constants below, was never one of the seven duplicated-in-two-
/// places constants #736 lifted (it only ever existed in `macos::board`).
const CARD_BORDER_WIDTH_DIP: f32 = 1.0;
/// Horizontal text padding inside a card, in DIPs. Derived from the shared
/// `f64` primitive constant rather than a hand-copied literal, so this is
/// still one source of truth even though every Direct2D DIP in this module
/// is `f32`.
const CARD_H_PAD_DIP: f32 = BOARD_CARD_H_PAD_PX as f32;
/// Title baseline offset from the card top, in DIPs.
const TITLE_Y_OFF_DIP: f32 = 6.0;
/// Badge-row offset from the card top, in DIPs.
const BADGE_Y_OFF_DIP: f32 = 26.0;
/// Hint strip offset from the card bottom, in DIPs.
const HINT_Y_OFF_DIP: f32 = 18.0;
/// Hint strip height, in DIPs.
const HINT_H_DIP: f32 = 14.0;
/// Column header text offset from the header top, in DIPs.
const HEADER_Y_OFF_DIP: f32 = 4.0;

/// Compute the Win-GUI DIP-unit layout for a [`BoardModel`] without
/// painting — the DirectWrite twin of [`draw_board`]'s internal layout
/// call. Same contract as the GTK/macOS/TUI twins' `*_board_layout`:
/// `rect.x`/`rect.y` are baked into every returned bound (absolute frame).
pub fn win_board_layout(model: &BoardModel, rect: Rect) -> BoardLayout {
    board_layout(
        model,
        rect.x,
        rect.y,
        rect.width,
        rect.height,
        BoardMeasure::new(
            BOARD_COL_MIN_PX,
            BOARD_COL_GAP_PX,
            BOARD_HEADER_H_PX,
            BOARD_CARD_H_PX,
            BOARD_CARD_GAP_PX,
        ),
    )
}

/// Draw a [`BoardModel`] into `rect` (DIPs, target-relative) on `target`.
/// Returns the resolved [`BoardLayout`] — same contract as the GTK/macOS/
/// TUI twins' `draw_board`: callers (and tests) read the layout back
/// instead of re-deriving it, so paint and hit-test can't drift apart.
pub fn draw_board(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    rect: Rect,
    model: &BoardModel,
    theme: &Theme,
) -> BoardLayout {
    let layout = win_board_layout(model, rect);

    if rect.width <= 0.0 || rect.height <= 0.0 {
        return layout;
    }

    for col_layout in &layout.columns {
        let col = &model.columns[col_layout.col_index];

        // ── Column header ────────────────────────────────────────────────
        let hb = col_layout.header_bounds;
        let _ = fill_rect(target, hb, theme.board_col_header_bg);
        let title_rect = Rect::new(
            hb.x + CARD_H_PAD_DIP,
            hb.y + HEADER_Y_OFF_DIP,
            (hb.width - CARD_H_PAD_DIP).max(0.0),
            (hb.height - HEADER_Y_OFF_DIP).max(0.0),
        );
        let _ = dwrite.draw_text(target, &col.title, title_rect, theme.header_fg);

        // ── Cards ────────────────────────────────────────────────────────
        for card_layout in &col_layout.cards {
            let card = &col.cards[card_layout.card_index];
            let is_selected = model
                .selected_card_id
                .as_ref()
                .map(|id| id == &card.id)
                .unwrap_or(false);

            let cb = card_layout.bounds;
            if cb.width <= 0.0 || cb.height <= 0.0 {
                continue;
            }

            // Card background.
            let card_bg = if is_selected {
                theme.board_selected_card_bg
            } else {
                theme.surface_bg
            };
            let _ = fill_rect(target, cb, card_bg);

            // Card border.
            let border_col = if is_selected {
                theme.accent_bg
            } else {
                theme.border_fg
            };
            let _ = stroke_rect(target, cb, border_col, CARD_BORDER_WIDTH_DIP);

            // ── Title line ───────────────────────────────────────────────
            let prefix = if card.labels.is_empty() {
                String::new()
            } else {
                format!("{} ", card.labels.join(" "))
            };
            let full_title = format!("{}{}", prefix, card.title);
            let title_rect = Rect::new(
                cb.x + CARD_H_PAD_DIP,
                cb.y + TITLE_Y_OFF_DIP,
                (cb.width - CARD_H_PAD_DIP * 2.0).max(0.0),
                (cb.height - TITLE_Y_OFF_DIP).max(0.0),
            );
            let _ = dwrite.draw_text(target, &full_title, title_rect, theme.surface_fg);

            // ── Badge row ────────────────────────────────────────────────
            let badge_y = cb.y + BADGE_Y_OFF_DIP;
            let mut badge_x = cb.x + CARD_H_PAD_DIP;
            for badge in &card.badges {
                let badge_str = format!("{}{} ", badge_icon(badge.status), badge.label);
                if let Ok((bw, bh)) = dwrite.measure_text(&badge_str) {
                    let badge_rect = Rect::new(badge_x, badge_y, bw.max(1.0), bh.max(1.0));
                    let color = badge_fg_color(badge.status, theme);
                    let _ = dwrite.draw_text(target, &badge_str, badge_rect, color);
                    badge_x += bw;
                }
                if badge_x > cb.x + cb.width - CARD_H_PAD_DIP {
                    break;
                }
            }

            // ── Hint ─────────────────────────────────────────────────────
            if let Some(hint) = &card.hint {
                let hint_y = cb.y + cb.height - HINT_Y_OFF_DIP;
                if hint_y > badge_y + 10.0 {
                    // Background strip.
                    let strip = Rect::new(
                        cb.x + 2.0,
                        hint_y - 2.0,
                        (cb.width - 4.0).max(0.0),
                        HINT_H_DIP,
                    );
                    let _ = fill_rect(target, strip, theme.card_hint_bg);
                    // Text.
                    let hint_rect = Rect::new(
                        cb.x + CARD_H_PAD_DIP,
                        hint_y,
                        (cb.width - CARD_H_PAD_DIP * 2.0).max(0.0),
                        HINT_H_DIP,
                    );
                    let _ = dwrite.draw_text(target, hint, hint_rect, theme.card_hint_fg);
                }
            }
        }
    }

    layout
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::board::{BadgeStatus, BoardColumn, BoardHit, CardBadge};
    use crate::types::{Color, WidgetId};
    use crate::win::testing::HeadlessSurface;

    const W: f32 = 460.0;
    const H: f32 = 300.0;

    fn card(id: &str, title: &str) -> crate::primitives::board::BoardCard {
        crate::primitives::board::BoardCard {
            id: WidgetId::new(id),
            title: title.into(),
            labels: vec!["#1".into()],
            badges: vec![CardBadge {
                label: "P".into(),
                status: BadgeStatus::Passed,
            }],
            hint: None,
        }
    }

    fn sample_model() -> BoardModel {
        BoardModel {
            id: WidgetId::new("board"),
            columns: vec![
                BoardColumn {
                    id: WidgetId::new("col:backlog"),
                    title: "Backlog".into(),
                    cards: vec![card("card:a", "First"), card("card:b", "Second")],
                    scroll_offset: 0,
                },
                BoardColumn {
                    id: WidgetId::new("col:done"),
                    title: "Done".into(),
                    cards: vec![card("card:c", "Third")],
                    scroll_offset: 0,
                },
            ],
            selected_card_id: Some(WidgetId::new("card:a")),
            col_scroll_offset: 0,
        }
    }

    /// C0 smoke: `draw_board` must actually paint text + a click-routable
    /// layout rather than panicking or hitting a `todo!()` (#736's
    /// acceptance bar — "draw_board survives C0 with text_ok on win").
    #[test]
    fn draw_board_paints_text_and_returns_layout() {
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
        let theme = Theme {
            background: Color::rgb(255, 255, 255),
            surface_bg: Color::rgb(255, 255, 255),
            foreground: Color::rgb(0, 0, 0),
            ..Theme::default()
        };
        let model = sample_model();
        let rect = Rect::new(0.0, 0.0, W, H);

        let layout = surface
            .paint(|target| {
                draw_board(target, &dwrite, rect, &model, &theme);
            })
            .map(|_| win_board_layout(&model, rect))
            .expect("paint board");

        assert!(!layout.columns.is_empty());

        // "text_ok" — some non-background pixel actually painted inside
        // the first card's title area (proves DrawText ran, not just the
        // border/fill).
        let cb = layout.columns[0].cards[0].bounds;
        let mut painted_any = false;
        for x in (cb.x as u32)..(cb.x + cb.width) as u32 {
            for y in (cb.y as u32)..(cb.y + cb.height) as u32 {
                let px = surface.pixel_at(x, y);
                if (px.r, px.g, px.b) != (255, 255, 255) {
                    painted_any = true;
                }
            }
        }
        assert!(painted_any, "expected draw_board to paint visible glyphs");
    }

    /// Paint↔click round trip (`docs/TESTING.md` coverage-taxonomy row 1)
    /// at a non-zero origin — #505's LOCAL/ABSOLUTE mixup regression guard,
    /// mirrored from `win::sidebar_panel`/`win::pipeline_view`'s own
    /// nonzero-origin tests.
    #[test]
    fn paint_and_click_round_trip_at_nonzero_origin() {
        let origin_x = 12.0_f32;
        let origin_y = 5.0_f32;
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
        let theme = Theme::default();
        let model = sample_model();
        let rect = Rect::new(origin_x, origin_y, W - origin_x, H - origin_y);

        let layout = surface
            .paint(|target| {
                draw_board(target, &dwrite, rect, &model, &theme);
            })
            .map(|_| win_board_layout(&model, rect))
            .expect("paint board");

        assert!(!layout.columns.is_empty());

        for col in &layout.columns {
            let hb = col.header_bounds;
            assert!(
                hb.x >= origin_x - 0.001 && hb.y >= origin_y - 0.001,
                "column header escaped the requested origin ({origin_x}, {origin_y})",
            );
            let hit = layout.hit_test(hb.x + 1.0, hb.y + hb.height / 2.0);
            assert_eq!(hit, BoardHit::ColumnHeader(col.col_id.clone()));

            for cardl in &col.cards {
                let cx = cardl.bounds.x + cardl.bounds.width / 2.0;
                let cy = cardl.bounds.y + cardl.bounds.height / 2.0;
                assert_eq!(
                    layout.hit_test(cx, cy),
                    BoardHit::Card(cardl.id.clone()),
                    "card click must resolve to its card",
                );
            }
        }
    }

    /// No-paint layout must agree byte-for-byte with what `draw_board`
    /// painted — same contract every other `win::` rasteriser's
    /// `no_paint_layout_matches_paint_layout` test proves.
    #[test]
    fn no_paint_layout_matches_paint_layout() {
        let model = sample_model();
        let rect = Rect::new(0.0, 0.0, W, H);
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");

        let painted = surface
            .paint(|target| {
                draw_board(target, &dwrite, rect, &model, &Theme::default());
            })
            .map(|_| win_board_layout(&model, rect))
            .expect("paint");
        let no_paint = win_board_layout(&model, rect);
        assert_eq!(painted, no_paint);
    }

    /// Zero-size rect is a no-op — mirrors every other `win::` rasteriser's
    /// same guard (see `win::pipeline_view::zero_size_rect_is_a_no_op`).
    #[test]
    fn zero_size_rect_is_a_no_op() {
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
        let theme = Theme::default();
        let model = sample_model();
        let rect = Rect::new(0.0, 0.0, 0.0, H);

        surface
            .fill_rect(Rect::new(0.0, 0.0, W, H), Color::rgb(255, 255, 255))
            .expect("fill background");

        surface
            .paint(|target| {
                draw_board(target, &dwrite, rect, &model, &theme);
            })
            .expect("paint board");

        let px = surface.pixel_at(1, 1);
        assert_eq!(
            (px.r, px.g, px.b),
            (255, 255, 255),
            "a zero-width board should paint nothing at all",
        );
    }

    /// Selected card uses `board_selected_card_bg`, unselected does not —
    /// mirrors `macos::board::selected_card_uses_the_selection_background`.
    #[test]
    fn selected_card_uses_the_selection_background() {
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
        let theme = Theme::default();
        let model = sample_model();
        let rect = Rect::new(0.0, 0.0, W, H);

        let layout = surface
            .paint(|target| {
                draw_board(target, &dwrite, rect, &model, &theme);
            })
            .map(|_| win_board_layout(&model, rect))
            .expect("paint board");

        // Probe near the bottom-right of the selected card, clear of
        // title/badge glyphs and the 1-DIP border.
        let cb = layout.columns[0].cards[0].bounds;
        let px = surface.pixel_at(
            (cb.x + cb.width - 6.0) as u32,
            (cb.y + cb.height - 6.0) as u32,
        );
        assert_eq!(
            (px.r, px.g, px.b),
            (
                theme.board_selected_card_bg.r,
                theme.board_selected_card_bg.g,
                theme.board_selected_card_bg.b
            ),
        );

        let cb2 = layout.columns[0].cards[1].bounds;
        let px2 = surface.pixel_at(
            (cb2.x + cb2.width - 6.0) as u32,
            (cb2.y + cb2.height - 6.0) as u32,
        );
        assert_ne!(
            (px2.r, px2.g, px2.b),
            (
                theme.board_selected_card_bg.r,
                theme.board_selected_card_bg.g,
                theme.board_selected_card_bg.b
            ),
            "an unselected card must not use the selection background",
        );
    }
}
