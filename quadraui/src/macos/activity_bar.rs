//! macOS rasteriser for [`crate::ActivityBar`].
//!
//! Vertical strip of icon rows. Top items render from the top edge
//! downward at `ACTIVITY_ROW_PX` per row; bottom items pin to the
//! bottom edge upward. Mirrors [`crate::gtk::activity_bar`] in
//! geometry, but uses the backend's active `CTFont` directly for
//! icon rendering instead of GTK's hardcoded "Symbols Nerd Font" —
//! apps that want Nerd-Font icons install a glyph-bearing font via
//! [`super::MacBackend::set_current_font`] in `setup()`.
//!
//! Returns per-row [`ActivityBarRowHit`]s so callers can route clicks
//! and query tooltips against the same frame's painted positions.

use core_graphics::geometry::CGRect;
use core_graphics::sys::CGContextRef;
use core_text::font::CTFont;

use super::text::{draw_text, measure_text};
use crate::primitives::activity_bar::{ActivityBar, ActivityBarRowHit, ActivityItem};
use crate::theme::Theme;
use crate::types::Color;

/// Fixed row height in points. Matches `crate::gtk::activity_bar::ACTIVITY_ROW_PX`
/// (= the vimcode native-button height baked into the GTK CSS).
pub const ACTIVITY_ROW_PX: f64 = 48.0;

/// Paint `bar` into `(0, 0, width, height)` on `ctx`.
///
/// # Safety
///
/// `ctx` must be a valid `CGContextRef` borrowed for the duration of
/// the call.
pub unsafe fn draw_activity_bar(
    ctx: CGContextRef,
    font: &CTFont,
    width: f64,
    height: f64,
    bar: &ActivityBar,
    theme: &Theme,
    hovered_idx: Option<usize>,
) -> Vec<ActivityBarRowHit> {
    CGContextSaveGState(ctx);

    // Background.
    fill_rect(ctx, 0.0, 0.0, width, height, theme.tab_bar_bg);
    // Right-edge separator (1 point).
    fill_rect(ctx, width - 1.0, 0.0, 1.0, height, theme.separator);

    let accent_col = bar.active_accent.unwrap_or(theme.accent_fg);
    let inactive_fg = theme.inactive_fg;
    let active_fg = theme.foreground;
    let hover_bg = theme.tab_bar_bg.lighten(0.10);

    let rows_total = ((height / ACTIVITY_ROW_PX).floor() as usize).max(1);
    let bottom_count = bar.bottom_items.len().min(rows_total);
    let top_capacity = rows_total.saturating_sub(bottom_count);
    let mut regions: Vec<ActivityBarRowHit> = Vec::new();

    let draw_row = |y: f64, item: &ActivityItem, row_idx: usize, regions: &mut Vec<_>| {
        let is_hovered = hovered_idx == Some(row_idx);

        if is_hovered {
            fill_rect(ctx, 0.0, y, width, ACTIVITY_ROW_PX, hover_bg);
        }
        if item.is_active {
            fill_rect(ctx, 0.0, y, 2.0, ACTIVITY_ROW_PX, accent_col);
        }

        let (iw, ih) = measure_text(font, &item.icon);
        let fg = if item.is_active || is_hovered {
            active_fg
        } else {
            inactive_fg
        };
        draw_text(
            ctx,
            font,
            &item.icon,
            (width - iw) / 2.0,
            y + (ACTIVITY_ROW_PX - ih) / 2.0,
            color_to_cg(fg),
        );

        regions.push(ActivityBarRowHit {
            y_start: y,
            y_end: y + ACTIVITY_ROW_PX,
            id: item.id.clone(),
            tooltip: item.tooltip.clone(),
        });
    };

    for (row_idx, item) in bar.top_items.iter().take(top_capacity).enumerate() {
        draw_row(
            row_idx as f64 * ACTIVITY_ROW_PX,
            item,
            row_idx,
            &mut regions,
        );
    }

    for (k, item) in bar.bottom_items.iter().rev().take(bottom_count).enumerate() {
        let y = height - (k + 1) as f64 * ACTIVITY_ROW_PX;
        if y < 0.0 {
            break;
        }
        let row_idx = regions.len();
        draw_row(y, item, row_idx, &mut regions);
    }

    CGContextRestoreGState(ctx);
    regions
}

fn color_to_cg(c: Color) -> (f64, f64, f64, f64) {
    (
        c.r as f64 / 255.0,
        c.g as f64 / 255.0,
        c.b as f64 / 255.0,
        c.a as f64 / 255.0,
    )
}

unsafe fn fill_rect(ctx: CGContextRef, x: f64, y: f64, w: f64, h: f64, c: Color) {
    let (r, g, b, a) = color_to_cg(c);
    CGContextSetRGBFillColor(ctx, r, g, b, a);
    use core_graphics::geometry::{CGPoint, CGSize};
    CGContextFillRect(ctx, CGRect::new(&CGPoint::new(x, y), &CGSize::new(w, h)));
}

extern "C" {
    fn CGContextSaveGState(c: CGContextRef);
    fn CGContextRestoreGState(c: CGContextRef);
    fn CGContextSetRGBFillColor(
        c: CGContextRef,
        red: core_graphics::base::CGFloat,
        green: core_graphics::base::CGFloat,
        blue: core_graphics::base::CGFloat,
        alpha: core_graphics::base::CGFloat,
    );
    fn CGContextFillRect(c: CGContextRef, rect: CGRect);
}

#[cfg(test)]
mod tests {
    use super::super::headless::BitmapSurface;
    use super::super::text::make_font;
    use super::super::MacBackend;
    use super::*;
    use crate::event::{Rect as QRect, Viewport};
    use crate::primitives::activity_bar::ActivityItem;
    use crate::theme::Theme;
    use crate::types::{Color, WidgetId};
    use crate::Backend;

    const W: u32 = 48;
    const H: u32 = 240; // 5 rows × 48px

    fn font() -> CTFont {
        make_font("Menlo", 14.0).expect("Menlo installed")
    }

    fn sample_bar() -> ActivityBar {
        ActivityBar {
            id: WidgetId::new("activity"),
            top_items: vec![
                ActivityItem {
                    id: WidgetId::new("activity:explorer"),
                    icon: "E".into(),
                    tooltip: "Explorer".into(),
                    is_active: true,
                    is_keyboard_selected: false,
                },
                ActivityItem {
                    id: WidgetId::new("activity:search"),
                    icon: "S".into(),
                    tooltip: "Search".into(),
                    is_active: false,
                    is_keyboard_selected: false,
                },
            ],
            bottom_items: vec![ActivityItem {
                id: WidgetId::new("activity:settings"),
                icon: "G".into(),
                tooltip: "Settings".into(),
                is_active: false,
                is_keyboard_selected: false,
            }],
            active_accent: Some(Color::rgb(80, 140, 255)),
            selection_bg: None,
        }
    }

    fn paint_via_backend(
        bar: &ActivityBar,
        hovered: Option<usize>,
    ) -> (BitmapSurface, Vec<ActivityBarRowHit>) {
        let surface = BitmapSurface::new(W, H);
        surface.fill(0.0, 0.0, 0.0, 0.0);

        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        let regions = std::cell::RefCell::new(Vec::new());
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            let r = b.draw_activity_bar(QRect::new(0.0, 0.0, W as f32, H as f32), bar, hovered);
            *regions.borrow_mut() = r;
        });
        backend.end_frame();
        (surface, regions.into_inner())
    }

    #[test]
    fn background_is_tab_bar_bg() {
        let bar = sample_bar();
        let (surface, _) = paint_via_backend(&bar, None);
        let theme = Theme::default();
        // Probe deep inside the second (non-active) row, well away
        // from any glyph centre.
        let (r, g, b, _) = surface.pixel(W - 6, (ACTIVITY_ROW_PX as u32) + 4);
        assert_eq!(
            (r, g, b),
            (theme.tab_bar_bg.r, theme.tab_bar_bg.g, theme.tab_bar_bg.b),
        );
    }

    #[test]
    fn active_item_paints_accent_strip_on_left_edge() {
        // 2-px accent strip at x ∈ [0, 2). Probe column 0 inside the
        // first row (active) and assert accent colour.
        let bar = sample_bar();
        let (surface, _) = paint_via_backend(&bar, None);
        let accent = bar.active_accent.unwrap();
        // y = ACTIVITY_ROW_PX/2 sits mid-row inside the first
        // (active) item.
        let probe_y = (ACTIVITY_ROW_PX as u32) / 2;
        let (r, g, b, _) = surface.pixel(0, probe_y);
        assert_eq!((r, g, b), (accent.r, accent.g, accent.b));
        let (r2, g2, b2, _) = surface.pixel(1, probe_y);
        assert_eq!((r2, g2, b2), (accent.r, accent.g, accent.b));

        // x = 2 should already be off the accent strip — proves the
        // strip is exactly 2 points wide. Probe at row top to dodge
        // any wide-glyph render.
        let (r3, g3, b3, _) = surface.pixel(2, 2);
        let theme = Theme::default();
        assert_eq!(
            (r3, g3, b3),
            (theme.tab_bar_bg.r, theme.tab_bar_bg.g, theme.tab_bar_bg.b),
            "x=2 should be tab_bar_bg, not accent",
        );
    }

    #[test]
    fn row_hits_cover_painted_rows() {
        // Round-trip: returned regions must point at where the icon
        // was painted. Verify each region's y-span matches the
        // ACTIVITY_ROW_PX grid AND its id matches the item.
        let bar = sample_bar();
        let (_surface, regions) = paint_via_backend(&bar, None);
        // 2 top + 1 bottom = 3 visible rows.
        assert_eq!(regions.len(), 3);
        assert_eq!(regions[0].y_start, 0.0);
        assert_eq!(regions[0].y_end, ACTIVITY_ROW_PX);
        assert_eq!(regions[0].id, WidgetId::new("activity:explorer"));

        assert_eq!(regions[1].y_start, ACTIVITY_ROW_PX);
        assert_eq!(regions[1].y_end, 2.0 * ACTIVITY_ROW_PX);
        assert_eq!(regions[1].id, WidgetId::new("activity:search"));

        // Bottom-pinned item.
        assert_eq!(regions[2].y_start, H as f64 - ACTIVITY_ROW_PX);
        assert_eq!(regions[2].y_end, H as f64);
        assert_eq!(regions[2].id, WidgetId::new("activity:settings"));
    }

    #[test]
    fn right_edge_has_separator_pixel() {
        let bar = sample_bar();
        let (surface, _) = paint_via_backend(&bar, None);
        let theme = Theme::default();
        let (r, g, b, _) = surface.pixel(W - 1, H / 2);
        assert_eq!(
            (r, g, b),
            (theme.separator.r, theme.separator.g, theme.separator.b),
        );
    }

    #[test]
    fn hover_lightens_row_background() {
        let bar = sample_bar();
        // Hover second row (index 1) — currently non-active so hover
        // tint is the only thing painting bg there.
        let (surface, _) = paint_via_backend(&bar, Some(1));
        let theme = Theme::default();
        let expected = theme.tab_bar_bg.lighten(0.10);
        // Probe deep into row 1, away from glyph centre.
        let (r, g, b, _) = surface.pixel(W - 6, (ACTIVITY_ROW_PX as u32) + 4);
        assert_eq!(
            (r, g, b),
            (expected.r, expected.g, expected.b),
            "hovered row should paint the lightened bg",
        );
    }
}
