//! macOS (Core Graphics + Core Text) rasteriser for
//! [`crate::primitives::board::BoardModel`].
//!
//! Port of [`crate::gtk::board::draw_board`]. Paints columns side by side,
//! each with a header strip and a vertical stack of rounded card boxes.
//! Cards show the issue title, an inline badge row, and an optional
//! [`crate::primitives::board::BoardCard::hint`] callout strip. The
//! selected card is highlighted with an accent border.
//!
//! ## Layout constants
//!
//! Point values, deliberately identical to the GTK twin's pixel values so
//! the two backends produce the same column/card grid for the same rect —
//! `board_layout` is shared, and only the [`BoardMeasure`] fed into it is
//! backend-supplied.
//!
//! ## Divergence from the GTK twin (deliberate)
//!
//! GTK sets a distinct Pango font size per element (11pt title, 9pt badge,
//! 9pt hint). macOS renders all three with the backend's single installed
//! `CTFont` — the same simplification every other macOS rasteriser makes
//! (see [`super::activity_bar`]), because per-element sizing needs a
//! `CTFontCreateCopyWithAttributes` variant cache that no macOS rasteriser
//! has yet. Text that would overflow a card is clipped to the card rect
//! rather than ellipsised (Core Text has no `EllipsizeMode` equivalent
//! without a full `CTFramesetter` path).

use core_graphics::geometry::CGRect;
use core_graphics::sys::CGContextRef;
use core_text::font::CTFont;

use super::text::{draw_text, measure_text};
use crate::primitives::board::{
    badge_fg_color, badge_icon, board_layout, BoardLayout, BoardMeasure, BoardModel,
    BOARD_CARD_CORNER_RADIUS_PX, BOARD_CARD_GAP_PX, BOARD_CARD_H_PAD_PX, BOARD_CARD_H_PX,
    BOARD_COL_GAP_PX, BOARD_COL_MIN_PX, BOARD_HEADER_H_PX,
};
use crate::theme::Theme;
use crate::types::Color;

/// Border stroke width for card boxes.
const CARD_BORDER_W: f64 = 1.0;
/// Title baseline offset from the card top.
const TITLE_Y_OFF: f64 = 6.0;
/// Badge-row offset from the card top.
const BADGE_Y_OFF: f64 = 26.0;
/// Hint strip offset from the card bottom.
const HINT_Y_OFF: f64 = 18.0;
/// Hint strip height.
const HINT_H: f64 = 14.0;
/// Column header text offset from the header top.
const HEADER_Y_OFF: f64 = 4.0;

/// Compute the macOS point-unit layout for a [`BoardModel`] without
/// painting. `x` / `y` are baked into every returned rect (absolute
/// frame), matching the GTK twin.
pub fn mac_board_layout(model: &BoardModel, x: f64, y: f64, w: f64, h: f64) -> BoardLayout {
    board_layout(
        model,
        x as f32,
        y as f32,
        w as f32,
        h as f32,
        BoardMeasure::new(
            BOARD_COL_MIN_PX,
            BOARD_COL_GAP_PX,
            BOARD_HEADER_H_PX,
            BOARD_CARD_H_PX,
            BOARD_CARD_GAP_PX,
        ),
    )
}

/// Draw a [`BoardModel`] onto `ctx`. Returns the layout for host click
/// dispatch (`BoardLayout::hit_test`) and selection-follow clamping.
///
/// # Safety
///
/// `ctx` must be a valid `CGContextRef` borrowed for the duration of the
/// call (typical: the frame-scope pointer stashed on [`super::MacBackend`]).
/// Calling with a freed or null pointer is UB.
#[allow(clippy::too_many_arguments)]
pub unsafe fn draw_board(
    ctx: CGContextRef,
    font: &CTFont,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    model: &BoardModel,
    theme: &Theme,
) -> BoardLayout {
    let layout = mac_board_layout(model, x, y, w, h);

    if w <= 0.0 || h <= 0.0 {
        return layout;
    }

    for col_layout in &layout.columns {
        let col = &model.columns[col_layout.col_index];

        // ── Column header ────────────────────────────────────────────────
        let hb = col_layout.header_bounds;
        fill_rect(
            ctx,
            hb.x as f64,
            hb.y as f64,
            hb.width as f64,
            hb.height as f64,
            theme.board_col_header_bg,
        );

        CGContextSaveGState(ctx);
        CGContextClipToRect(
            ctx,
            CGRect::new_xywh(hb.x as f64, hb.y as f64, hb.width as f64, hb.height as f64),
        );
        draw_text(
            ctx,
            font,
            &col.title,
            hb.x as f64 + BOARD_CARD_H_PAD_PX,
            hb.y as f64 + HEADER_Y_OFF,
            color_to_cg(theme.header_fg),
        );
        CGContextRestoreGState(ctx);

        // ── Cards ────────────────────────────────────────────────────────
        for card_layout in &col_layout.cards {
            let card = &col.cards[card_layout.card_index];
            let is_selected = model
                .selected_card_id
                .as_ref()
                .map(|id| id == &card.id)
                .unwrap_or(false);

            let cb = card_layout.bounds;
            let (bx, by, bw, bh) = (cb.x as f64, cb.y as f64, cb.width as f64, cb.height as f64);
            if bw <= 0.0 || bh <= 0.0 {
                continue;
            }

            // Card background.
            let card_bg = if is_selected {
                theme.board_selected_card_bg
            } else {
                theme.surface_bg
            };
            add_rounded_rect_path(ctx, bx, by, bw, bh, BOARD_CARD_CORNER_RADIUS_PX);
            set_fill(ctx, card_bg);
            CGContextFillPath(ctx);

            // Card border.
            let border_col = if is_selected {
                theme.accent_bg
            } else {
                theme.border_fg
            };
            add_rounded_rect_path(ctx, bx, by, bw, bh, BOARD_CARD_CORNER_RADIUS_PX);
            set_stroke(ctx, border_col);
            CGContextSetLineWidth(ctx, CARD_BORDER_W);
            CGContextStrokePath(ctx);

            // Everything below is clipped to the card box — Core Text has
            // no ellipsize, so an over-long title truncates at the border
            // rather than bleeding into the next column.
            CGContextSaveGState(ctx);
            CGContextClipToRect(ctx, CGRect::new_xywh(bx, by, bw, bh));

            // ── Title line ───────────────────────────────────────────────
            let prefix = if card.labels.is_empty() {
                String::new()
            } else {
                format!("{} ", card.labels.join(" "))
            };
            let full_title = format!("{}{}", prefix, card.title);
            draw_text(
                ctx,
                font,
                &full_title,
                bx + BOARD_CARD_H_PAD_PX,
                by + TITLE_Y_OFF,
                color_to_cg(theme.surface_fg),
            );

            // ── Badge row ────────────────────────────────────────────────
            let badge_y = by + BADGE_Y_OFF;
            let mut badge_x = bx + BOARD_CARD_H_PAD_PX;
            for badge in &card.badges {
                let badge_str = format!("{}{} ", badge_icon(badge.status), badge.label);
                draw_text(
                    ctx,
                    font,
                    &badge_str,
                    badge_x,
                    badge_y,
                    color_to_cg(badge_fg_color(badge.status, theme)),
                );
                let (bw_text, _) = measure_text(font, &badge_str);
                badge_x += bw_text;
                if badge_x > bx + bw - BOARD_CARD_H_PAD_PX {
                    break;
                }
            }

            // ── Hint ─────────────────────────────────────────────────────
            if let Some(hint) = &card.hint {
                let hint_y = by + bh - HINT_Y_OFF;
                if hint_y > badge_y + 10.0 {
                    fill_rect(
                        ctx,
                        bx + 2.0,
                        hint_y - 2.0,
                        bw - 4.0,
                        HINT_H,
                        theme.card_hint_bg,
                    );
                    draw_text(
                        ctx,
                        font,
                        hint,
                        bx + BOARD_CARD_H_PAD_PX,
                        hint_y,
                        color_to_cg(theme.card_hint_fg),
                    );
                }
            }

            CGContextRestoreGState(ctx);
        }
    }

    layout
}

fn color_to_cg(c: Color) -> (f64, f64, f64, f64) {
    (
        c.r as f64 / 255.0,
        c.g as f64 / 255.0,
        c.b as f64 / 255.0,
        c.a as f64 / 255.0,
    )
}

unsafe fn set_fill(ctx: CGContextRef, c: Color) {
    let (r, g, b, a) = color_to_cg(c);
    CGContextSetRGBFillColor(ctx, r, g, b, a);
}

unsafe fn set_stroke(ctx: CGContextRef, c: Color) {
    let (r, g, b, a) = color_to_cg(c);
    CGContextSetRGBStrokeColor(ctx, r, g, b, a);
}

unsafe fn fill_rect(ctx: CGContextRef, x: f64, y: f64, w: f64, h: f64, c: Color) {
    set_fill(ctx, c);
    CGContextFillRect(ctx, CGRect::new_xywh(x, y, w, h));
}

/// Append a rounded-rect path to `ctx`. Same construction as
/// `super::pipeline_view`'s helper.
unsafe fn add_rounded_rect_path(ctx: CGContextRef, x: f64, y: f64, w: f64, h: f64, r: f64) {
    let r = r.min(w / 2.0).min(h / 2.0);
    CGContextBeginPath(ctx);
    CGContextMoveToPoint(ctx, x + r, y);
    CGContextAddLineToPoint(ctx, x + w - r, y);
    CGContextAddArcToPoint(ctx, x + w, y, x + w, y + r, r);
    CGContextAddLineToPoint(ctx, x + w, y + h - r);
    CGContextAddArcToPoint(ctx, x + w, y + h, x + w - r, y + h, r);
    CGContextAddLineToPoint(ctx, x + r, y + h);
    CGContextAddArcToPoint(ctx, x, y + h, x, y + h - r, r);
    CGContextAddLineToPoint(ctx, x, y + r);
    CGContextAddArcToPoint(ctx, x, y, x + r, y, r);
    CGContextClosePath(ctx);
}

trait CGRectExt {
    fn new_xywh(x: f64, y: f64, w: f64, h: f64) -> Self;
}
impl CGRectExt for CGRect {
    fn new_xywh(x: f64, y: f64, w: f64, h: f64) -> Self {
        use core_graphics::geometry::{CGPoint, CGSize};
        CGRect::new(&CGPoint::new(x, y), &CGSize::new(w, h))
    }
}

extern "C" {
    fn CGContextSaveGState(c: CGContextRef);
    fn CGContextRestoreGState(c: CGContextRef);
    fn CGContextClipToRect(c: CGContextRef, rect: CGRect);
    fn CGContextSetRGBFillColor(
        c: CGContextRef,
        red: core_graphics::base::CGFloat,
        green: core_graphics::base::CGFloat,
        blue: core_graphics::base::CGFloat,
        alpha: core_graphics::base::CGFloat,
    );
    fn CGContextSetRGBStrokeColor(
        c: CGContextRef,
        red: core_graphics::base::CGFloat,
        green: core_graphics::base::CGFloat,
        blue: core_graphics::base::CGFloat,
        alpha: core_graphics::base::CGFloat,
    );
    fn CGContextSetLineWidth(c: CGContextRef, width: core_graphics::base::CGFloat);
    fn CGContextFillRect(c: CGContextRef, rect: CGRect);
    fn CGContextBeginPath(c: CGContextRef);
    fn CGContextMoveToPoint(
        c: CGContextRef,
        x: core_graphics::base::CGFloat,
        y: core_graphics::base::CGFloat,
    );
    fn CGContextAddLineToPoint(
        c: CGContextRef,
        x: core_graphics::base::CGFloat,
        y: core_graphics::base::CGFloat,
    );
    fn CGContextAddArcToPoint(
        c: CGContextRef,
        x1: core_graphics::base::CGFloat,
        y1: core_graphics::base::CGFloat,
        x2: core_graphics::base::CGFloat,
        y2: core_graphics::base::CGFloat,
        radius: core_graphics::base::CGFloat,
    );
    fn CGContextClosePath(c: CGContextRef);
    fn CGContextFillPath(c: CGContextRef);
    fn CGContextStrokePath(c: CGContextRef);
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

    /// Paint through the real `Backend::draw_board` path — the same call
    /// chain the live `drawRect:` runner uses. Proves the trait method is
    /// overridden rather than silently taking the trait's no-op default
    /// (quadraui#600).
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
