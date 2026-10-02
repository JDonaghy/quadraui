//! macOS (Core Graphics + Core Text) rasteriser for
//! [`crate::primitives::board::BoardModel`].
//!
//! Painting moved to the shared
//! [`crate::primitives::board::native_surface_paint::paint`] (#1085,
//! `NativeSurface` Phase 4 slice 8/8) — see that fn's doc for the four
//! named divergences (column-header overflow; card-title wrapping;
//! rounded vs. straight card borders; per-element font size) found
//! while unifying `gtk::board::draw_board`, `macos::board::draw_board`
//! and `win::board::draw_board` into one implementation. This module now
//! only carries [`mac_board_layout`] (still real, backend-specific pure
//! geometry — no painting involved); the deprecated `draw_board`
//! compatibility shim over the shared [`super::surface::CgSurface`]
//! adapter was removed in issue #1109 (zero uses in coord-tui's `main`
//! and vimcode's `develop`).

use crate::primitives::board::{BoardLayout, BoardModel};
use crate::primitives::layout_metrics::pixel_board_layout;
#[cfg(test)]
use crate::theme::Theme;

/// Compute the macOS point-unit layout for a [`BoardModel`] without
/// painting. `x` / `y` are baked into every returned rect (absolute
/// frame), matching the GTK twin. Shares its column/card measure with
/// `gtk_board_layout` / `win_board_layout` via [`pixel_board_layout`]
/// (issue #1079).
pub fn mac_board_layout(model: &BoardModel, x: f64, y: f64, w: f64, h: f64) -> BoardLayout {
    pixel_board_layout(model, x as f32, y as f32, w as f32, h as f32)
}

#[cfg(test)]
mod tests {
    use super::super::headless::BitmapSurface;
    use super::super::text::make_font;
    use super::super::MacBackend;
    use super::*;
    use crate::event::{Rect as QRect, Viewport};
    use crate::primitives::board::{BadgeStatus, BoardCard, BoardColumn, BoardHit, CardBadge};
    use crate::types::WidgetId;
    use crate::Backend;

    const W: u32 = 460;
    const H: u32 = 300;

    fn card(id: &str, title: &str) -> BoardCard {
        BoardCard {
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

    /// Paint through the real `Backend::draw_board` path — which now
    /// routes through the shared `native_surface_paint::paint` — the
    /// same call chain the live `drawRect:` runner uses, and avoids
    /// tripping the `-D warnings`-denied `deprecated` lint (CLAUDE.md
    /// rule 3) by not calling the deprecated free-function shim above.
    fn paint_via_backend(model: &BoardModel, rect: QRect) -> (BitmapSurface, BoardLayout) {
        let surface = BitmapSurface::new(W, H);
        surface.fill(1.0, 1.0, 1.0, 1.0);
        let mut backend = MacBackend::new();
        backend.set_current_font(make_font("Menlo", 12.0).expect("Menlo installed"));
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        let captured = std::cell::RefCell::new(None);
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            *captured.borrow_mut() = Some(b.draw_board(rect, model));
        });
        backend.end_frame();
        (surface, captured.into_inner().expect("layout captured"))
    }

    #[test]
    fn draw_board_is_not_the_trait_no_op_default() {
        // The default returns `columns: vec![]` and paints nothing.
        let model = sample_model();
        let (surface, layout) = paint_via_backend(&model, QRect::new(0.0, 0.0, W as f32, H as f32));
        assert!(
            !layout.columns.is_empty(),
            "MacBackend must override `draw_board` — an empty `columns` is the trait default",
        );
        let painted = surface
            .bytes()
            .chunks_exact(4)
            .any(|p| (p[0], p[1], p[2]) != (255, 255, 255));
        assert!(painted, "draw_board painted nothing at all");
    }

    #[test]
    fn column_header_paints_its_background() {
        let model = sample_model();
        let (surface, layout) = paint_via_backend(&model, QRect::new(0.0, 0.0, W as f32, H as f32));
        let theme = Theme::default();
        let hb = layout.columns[0].header_bounds;
        // Probe the right end of the header strip, clear of the title text.
        let px = (hb.x + hb.width - 4.0) as u32;
        let py = (hb.y + hb.height / 2.0) as u32;
        let (r, g, b, _) = surface.pixel(px, py);
        assert_eq!(
            (r, g, b),
            (
                theme.board_col_header_bg.r,
                theme.board_col_header_bg.g,
                theme.board_col_header_bg.b
            ),
        );
    }

    /// Regression for #1085 divergence 1: an overlong header title now
    /// hard-clips to its own header strip instead of bleeding into the
    /// next column — previously unclipped on macOS (`CGContextClipToRect`
    /// bracketed the header draw, but a title wider than the *entire*
    /// board would still have escaped since the clip was already scoped
    /// to `header_bounds`; the real observable regression this guards is
    /// the shared `paint`'s hard-clip staying in force after the #1085
    /// port, not a behaviour change on this backend specifically).
    #[test]
    fn overlong_header_title_does_not_paint_past_its_own_column() {
        let mut model = sample_model();
        model.columns[0].title = "A".repeat(200);
        let (surface, layout) = paint_via_backend(&model, QRect::new(0.0, 0.0, W as f32, H as f32));
        let hb0 = layout.columns[0].header_bounds;
        let hb1 = layout.columns[1].header_bounds;
        // A few pixels into the second column's header must still show
        // that column's own header background, not glyph ink bled over
        // from column 0's oversized title.
        let theme = Theme::default();
        let px = (hb1.x + 4.0) as u32;
        let py = (hb1.y + hb1.height / 2.0) as u32;
        let (r, g, b, _) = surface.pixel(px, py);
        assert_eq!(
            (r, g, b),
            (
                theme.board_col_header_bg.r,
                theme.board_col_header_bg.g,
                theme.board_col_header_bg.b
            ),
            "column 1's header must not show column 0's overflowed title ink \
             (hb0 ends at {}, hb1 starts at {})",
            hb0.x + hb0.width,
            hb1.x,
        );
    }

    #[test]
    fn selected_card_uses_the_selection_background() {
        let model = sample_model();
        let (surface, layout) = paint_via_backend(&model, QRect::new(0.0, 0.0, W as f32, H as f32));
        let theme = Theme::default();
        let cb = layout.columns[0].cards[0].bounds;
        // Probe near the bottom-right of the card, clear of title/badge
        // glyphs and the 1pt border.
        let px = (cb.x + cb.width - 6.0) as u32;
        let py = (cb.y + cb.height - 6.0) as u32;
        let (r, g, b, _) = surface.pixel(px, py);
        assert_eq!(
            (r, g, b),
            (
                theme.board_selected_card_bg.r,
                theme.board_selected_card_bg.g,
                theme.board_selected_card_bg.b
            ),
            "the selected card should paint `board_selected_card_bg`",
        );

        // ...and its unselected sibling should not.
        let cb2 = layout.columns[0].cards[1].bounds;
        let (r2, g2, b2, _) = surface.pixel(
            (cb2.x + cb2.width - 6.0) as u32,
            (cb2.y + cb2.height - 6.0) as u32,
        );
        assert_ne!(
            (r2, g2, b2),
            (
                theme.board_selected_card_bg.r,
                theme.board_selected_card_bg.g,
                theme.board_selected_card_bg.b
            ),
            "an unselected card must not use the selection background",
        );
    }

    /// Regression for #1085 divergence 3: the card border is a straight
    /// rectangle on every backend now, so the extreme corner pixel (which
    /// a rounded-rect border would have left unstroked) is stroked in
    /// `border_fg` here.
    #[test]
    fn card_border_is_a_straight_rect_not_a_rounded_one() {
        let model = sample_model();
        let (surface, layout) = paint_via_backend(&model, QRect::new(0.0, 0.0, W as f32, H as f32));
        let theme = Theme::default();
        let cb = layout.columns[0].cards[1].bounds; // unselected -> border_fg
        let px = cb.x as u32;
        let py = cb.y as u32;
        let (r, g, b, _) = surface.pixel(px, py);
        assert_eq!(
            (r, g, b),
            (theme.border_fg.r, theme.border_fg.g, theme.border_fg.b),
            "the exact top-left corner pixel must be stroked -- a rounded \
             border would have left it as the card background instead",
        );
    }

    /// Shared paint↔click round trip: click the centre of each painted
    /// card / header and prove `hit_test` resolves to the same entity.
    fn paint_click_round_trip_at(origin_x: f32, origin_y: f32) {
        let model = sample_model();
        let rect = QRect::new(origin_x, origin_y, W as f32 - origin_x, H as f32 - origin_y);
        let (surface, layout) = paint_via_backend(&model, rect);
        let theme = Theme::default();

        assert!(!layout.columns.is_empty());

        for col in &layout.columns {
            // Header pixel is painted where the layout says it is.
            let hx = col.header_bounds.x + col.header_bounds.width - 4.0;
            let hy = col.header_bounds.y + col.header_bounds.height / 2.0;
            let (r, g, b, _) = surface.pixel(hx as u32, hy as u32);
            assert_eq!(
                (r, g, b),
                (
                    theme.board_col_header_bg.r,
                    theme.board_col_header_bg.g,
                    theme.board_col_header_bg.b
                ),
                "header of {:?} not painted at origin ({origin_x}, {origin_y})",
                col.col_id,
            );

            // ...and clicking it resolves back to the same column.
            assert_eq!(
                layout.hit_test(hx, hy),
                BoardHit::ColumnHeader(col.col_id.clone()),
                "header click at origin ({origin_x}, {origin_y}) must resolve to its column",
            );

            for cardl in &col.cards {
                let cx = cardl.bounds.x + cardl.bounds.width / 2.0;
                let cy = cardl.bounds.y + cardl.bounds.height / 2.0;
                assert_eq!(
                    layout.hit_test(cx, cy),
                    BoardHit::Card(cardl.id.clone()),
                    "card click at origin ({origin_x}, {origin_y}) must resolve to its card",
                );
                assert!(
                    cardl.bounds.x >= origin_x - 0.001 && cardl.bounds.y >= origin_y - 0.001,
                    "card {:?} escaped the requested origin ({origin_x}, {origin_y})",
                    cardl.id,
                );
            }
        }
    }

    #[test]
    fn paint_click_round_trip() {
        paint_click_round_trip_at(0.0, 0.0);
    }

    /// Non-zero-origin regression guard (quadraui#494, LESSONS.md:159-181).
    #[test]
    fn paint_click_round_trip_at_nonzero_origin() {
        paint_click_round_trip_at(31.0, 17.0);
    }

    #[test]
    fn layout_twin_matches_the_painted_layout() {
        let model = sample_model();
        let rect = QRect::new(13.0, 9.0, 400.0, 220.0);
        let (_surface, painted) = paint_via_backend(&model, rect);
        let computed = mac_board_layout(
            &model,
            rect.x as f64,
            rect.y as f64,
            rect.width as f64,
            rect.height as f64,
        );
        assert_eq!(painted, computed);
    }

    #[test]
    fn empty_rect_returns_an_empty_layout_without_panicking() {
        let model = sample_model();
        let (_surface, layout) = paint_via_backend(&model, QRect::new(0.0, 0.0, 0.0, 0.0));
        assert!(layout.columns.is_empty());
    }
}
