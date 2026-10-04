//! Direct2D / DirectWrite rasteriser for
//! [`crate::primitives::board::BoardModel`] (#736).
//!
//! Painting moved to the shared
//! [`crate::primitives::board::native_surface_paint::paint`] (#1085,
//! `PaintSurface` Phase 4 slice 8/8) — see that fn's doc for the four
//! named divergences (column-header overflow; card-title wrapping;
//! rounded vs. straight card borders — this backend's own straight-rect
//! border becomes the behaviour every backend now shares; per-element
//! font size — this backend's own single-text-format behaviour becomes
//! the behaviour every backend now shares) found while unifying
//! `gtk::board::draw_board`, `macos::board::draw_board` and
//! `win::board::draw_board` into one implementation. This module now
//! only carries [`win_board_layout`] (still real, backend-specific pure
//! geometry — no painting involved); the deprecated `draw_board`
//! compatibility shim over the shared [`super::surface::D2dSurface`]
//! adapter was removed in issue #1109 (zero uses in coord-tui's `main`
//! and vimcode's `develop`).
//!
//! Only compiled on `target_os = "windows"` — see `super::mod`'s
//! `#[cfg(target_os = "windows")] mod board;` and `backend.rs`'s module
//! docs for why the rest of this repo's `--features win` compile gate
//! stays meaningful without a Windows host.

#[cfg(test)]
use super::text::DWrite;
use crate::event::Rect;
use crate::primitives::board::{BoardLayout, BoardModel};
use crate::primitives::layout_metrics::pixel_board_layout;
#[cfg(test)]
use crate::theme::Theme;

/// Compute the Win-GUI DIP-unit layout for a [`BoardModel`] without
/// painting — the DirectWrite twin of `crate::Backend::draw_board`'s
/// internal layout call (the free-function `draw_board` shim this
/// backed was removed in issue #1109). Same contract as the
/// GTK/macOS/TUI twins' `*_board_layout`:
/// `rect.x`/`rect.y` are baked into every returned bound (absolute
/// frame). Shares its column/card measure with `gtk_board_layout` /
/// `mac_board_layout` via [`pixel_board_layout`] (issue #1079).
pub fn win_board_layout(model: &BoardModel, rect: Rect) -> BoardLayout {
    pixel_board_layout(model, rect.x, rect.y, rect.width, rect.height)
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

    /// Paint `model` via the shared
    /// [`crate::primitives::board::native_surface_paint::paint`] through
    /// a [`super::super::surface::D2dSurface`] over `surface`'s headless
    /// target — the same adapter the now-removed `draw_board` shim
    /// used (issue #1109), exercised here directly.
    fn paint(
        surface: &HeadlessSurface,
        dwrite: &DWrite,
        rect: Rect,
        model: &BoardModel,
        theme: &Theme,
    ) -> BoardLayout {
        surface
            .paint(|target| {
                let mut raw = super::super::surface::D2dSurface {
                    target,
                    dwrite: Some(dwrite),
                };
                crate::primitives::board::native_surface_paint::paint(model, &mut raw, theme, rect);
            })
            .map(|_| win_board_layout(model, rect))
            .expect("paint board")
    }

    /// C0 smoke: `draw_board` must actually paint text + a click-routable
    /// layout rather than panicking or hitting a `todo!()` (#736's
    /// acceptance bar — "draw_board survives C0 with text_ok on win").
    #[test]
    fn draw_board_paints_text_and_returns_layout() {
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let theme = Theme {
            background: Color::rgb(255, 255, 255),
            surface_bg: Color::rgb(255, 255, 255),
            foreground: Color::rgb(0, 0, 0),
            ..Theme::default()
        };
        let model = sample_model();
        let rect = Rect::new(0.0, 0.0, W, H);

        let layout = paint(&surface, &dwrite, rect, &model, &theme);

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
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let theme = Theme::default();
        let model = sample_model();
        let rect = Rect::new(origin_x, origin_y, W - origin_x, H - origin_y);

        let layout = paint(&surface, &dwrite, rect, &model, &theme);

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
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");

        let painted = paint(&surface, &dwrite, rect, &model, &Theme::default());
        let no_paint = win_board_layout(&model, rect);
        assert_eq!(painted, no_paint);
    }

    /// Zero-size rect is a no-op — mirrors every other `win::` rasteriser's
    /// same guard (see `win::pipeline_view::zero_size_rect_is_a_no_op`).
    #[test]
    fn zero_size_rect_is_a_no_op() {
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let theme = Theme::default();
        let model = sample_model();
        let rect = Rect::new(0.0, 0.0, 0.0, H);

        surface
            .fill_rect(Rect::new(0.0, 0.0, W, H), Color::rgb(255, 255, 255))
            .expect("fill background");

        paint(&surface, &dwrite, rect, &model, &theme);

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
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let theme = Theme::default();
        let model = sample_model();
        let rect = Rect::new(0.0, 0.0, W, H);

        let layout = paint(&surface, &dwrite, rect, &model, &theme);

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
