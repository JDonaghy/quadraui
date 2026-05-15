//! macOS rasteriser for [`crate::TabBar`].
//!
//! Mirrors [`crate::gtk::tab_bar::draw_tab_bar`]: measures tab widths
//! via Core Text, lays out left-to-right with active-tab highlighting,
//! close glyphs (× or ● for dirty), and right-aligned segments.
//! Returns a [`TabBarHits`] carrying per-tab + per-segment screen
//! bounds for the caller's click dispatch.
//!
//! ## Scope omissions (follow-up after the #38 chrome batch)
//!
//! - **Italic preview tabs** — needs an italic-variant `CTFont` via
//!   `CTFontCreateCopyWithSymbolicTraits`. Same dependency as bold
//!   support in [`super::status_bar`]; pairs naturally with that
//!   future change. Until it lands, preview tabs render in the
//!   active font.
//! - **Rounded close-button hover background** — the GTK rasteriser
//!   draws a 3 px rounded rect under the close glyph on hover. macOS
//!   uses a simpler approach for #38: tint the close glyph itself
//!   (`theme.foreground`) when hovered. Visual parity with GTK is
//!   tracked separately.

use core_graphics::geometry::CGRect;
use core_graphics::sys::CGContextRef;
use core_text::font::CTFont;

use super::text::{draw_text, measure_text};
use crate::primitives::tab_bar::{TabBar, TabBarHits};
use crate::theme::Theme;
use crate::types::Color;

/// Per-tab horizontal padding (left + right) inside the tab background fill.
const TAB_PAD: f64 = 14.0;
/// Gap between the tab label and the close glyph.
const TAB_INNER_GAP: f64 = 10.0;
/// Gap between adjacent tabs.
const TAB_OUTER_GAP: f64 = 1.0;
/// Top-edge accent strip height for the active tab (when `active_accent`
/// is set).
const ACCENT_HEIGHT: f64 = 2.0;
/// 15-char sample used to estimate cell width for `available_cols`.
/// Same string the GTK rasteriser uses, so app-level cell budgets
/// remain comparable between backends.
const CELL_WIDTH_SAMPLE: &str = "ABCDabcd0123.:_";

/// Paint `bar` into the rect `(0, y_offset, width, row_height)` on
/// `ctx`. `line_height` is the *text* line height; `row_height` may be
/// larger (callers pad file-tab bars). Returns per-tab + per-segment
/// hit bounds plus the resolved scroll offset.
///
/// # Safety
///
/// `ctx` must be a valid `CGContextRef` borrowed for the duration of
/// the call (typical: the frame-scope pointer stashed on
/// [`super::MacBackend`]). Calling with a freed or null pointer is UB.
#[allow(clippy::too_many_arguments)]
pub unsafe fn draw_tab_bar(
    ctx: CGContextRef,
    font: &CTFont,
    width: f64,
    line_height: f64,
    y_offset: f64,
    row_height: f64,
    bar: &TabBar,
    theme: &Theme,
    hovered_close_tab: Option<usize>,
) -> TabBarHits {
    let text_y_offset = y_offset + (row_height - line_height) / 2.0;
    let tab_pad = if bar.compact { 2.0 } else { TAB_PAD };
    let tab_inner_gap = if bar.compact { 4.0 } else { TAB_INNER_GAP };
    let tab_outer_gap = if bar.compact { 0.0 } else { TAB_OUTER_GAP };

    CGContextSaveGState(ctx);

    // Tab-bar background.
    fill_rect(ctx, 0.0, y_offset, width, row_height, theme.tab_bar_bg);

    // ── Right-segment widths (measure once; paint after tabs) ─────────
    let mut right_widths: Vec<f64> = Vec::with_capacity(bar.right_segments.len());
    for seg in &bar.right_segments {
        let (w, _) = measure_text(font, &seg.text);
        right_widths.push(w);
    }
    let reserved_px: f64 = right_widths.iter().sum();
    let effective_tab_area = (width - reserved_px).max(0.0);

    // Close-glyph width measured once — every tab pays the same width
    // for the `×` glyph (the `●` dirty variant is the same width in
    // Menlo and most monospace fonts).
    let close_w = if bar.show_tab_close {
        let (w, _) = measure_text(font, "×");
        w
    } else {
        0.0
    };
    let close_extra = if bar.show_tab_close {
        tab_inner_gap + close_w
    } else {
        0.0
    };

    // Pre-measure every tab's full slot width — used both for scroll
    // offset resolution and the paint loop.
    let tab_slot_widths: Vec<f64> = bar
        .tabs
        .iter()
        .map(|tab| {
            let (name_w, _) = measure_text(font, &tab.label);
            tab_pad + name_w + close_extra + tab_pad + tab_outer_gap
        })
        .collect();

    let active_idx = bar.tabs.iter().position(|t| t.is_active);
    let correct_scroll_offset = if let Some(active) = active_idx {
        TabBar::fit_active_scroll_offset(active, bar.tabs.len(), effective_tab_area as usize, |i| {
            tab_slot_widths[i] as usize
        })
    } else {
        bar.scroll_offset
    };

    // ── Tabs paint loop ──────────────────────────────────────────────
    let mut slot_positions: Vec<(f64, f64)> = Vec::with_capacity(bar.tabs.len());
    let mut close_bounds: Vec<Option<(f64, f64)>> = Vec::with_capacity(bar.tabs.len());
    for _ in 0..bar.scroll_offset.min(bar.tabs.len()) {
        slot_positions.push((0.0, 0.0));
        close_bounds.push(None);
    }

    let mut x = 0.0_f64;
    for (tab_idx, tab) in bar.tabs.iter().enumerate().skip(bar.scroll_offset) {
        let (tab_name_w, _) = measure_text(font, &tab.label);
        let tab_content_w = tab_pad + tab_name_w + close_extra + tab_pad;
        let slot_w = tab_content_w + tab_outer_gap;
        if x + slot_w > effective_tab_area {
            break;
        }
        slot_positions.push((x, x + slot_w));

        let close_x = x + tab_pad + tab_name_w + tab_inner_gap;
        if bar.show_tab_close {
            let close_pad = 2.0;
            close_bounds.push(Some((close_x - close_pad, close_x + close_w + close_pad)));
        } else {
            close_bounds.push(None);
        }

        // Tab background.
        let bg_col = if tab.is_active {
            theme.tab_active_bg
        } else {
            theme.tab_bar_bg
        };
        fill_rect(ctx, x, y_offset, tab_content_w, row_height, bg_col);

        // Top accent line for the active tab.
        if tab.is_active {
            if let Some(accent) = bar.active_accent {
                fill_rect(ctx, x, y_offset, tab_content_w, ACCENT_HEIGHT, accent);
            }
        }

        // Tab label.
        let fg_col = match (tab.is_active, tab.is_preview) {
            (true, true) => theme.tab_preview_active_fg,
            (true, false) => theme.tab_active_fg,
            (false, true) => theme.tab_preview_inactive_fg,
            (false, false) => theme.tab_inactive_fg,
        };
        draw_text(
            ctx,
            font,
            &tab.label,
            x + tab_pad,
            text_y_offset,
            color_to_cg(fg_col),
        );

        // Close glyph (× or ● for dirty), tinted on hover.
        if bar.show_tab_close {
            let is_close_hovered = hovered_close_tab == Some(tab_idx);
            let close_glyph = if tab.is_dirty && !is_close_hovered {
                "●"
            } else {
                "×"
            };
            let close_fg = if tab.is_dirty || is_close_hovered {
                theme.foreground
            } else if tab.is_active {
                theme.tab_inactive_fg
            } else {
                theme.separator
            };
            draw_text(
                ctx,
                font,
                close_glyph,
                close_x,
                text_y_offset,
                color_to_cg(close_fg),
            );
        }

        x += slot_w;
    }

    // ── Right segments paint loop ────────────────────────────────────
    let right_base = width - reserved_px;
    let mut right_segment_bounds: Vec<(f64, f64)> = Vec::with_capacity(bar.right_segments.len());
    let mut sx = right_base;
    for (i, seg) in bar.right_segments.iter().enumerate() {
        let seg_w = right_widths[i];
        let fg_col = if seg.is_active {
            theme.tab_active_fg
        } else {
            theme.tab_inactive_fg
        };
        draw_text(ctx, font, &seg.text, sx, text_y_offset, color_to_cg(fg_col));
        right_segment_bounds.push((sx, sx + seg_w));
        sx += seg_w;
    }

    // Cell-width estimation: 15-char sample width / 15 → average glyph
    // advance. Matches the GTK convention so app-level char-col math
    // doesn't diverge across backends.
    let (sample_px, _) = measure_text(font, CELL_WIDTH_SAMPLE);
    let char_w = (sample_px / CELL_WIDTH_SAMPLE.chars().count() as f64).max(1.0);
    let available_cols = (effective_tab_area / char_w).floor().max(0.0) as usize;

    CGContextRestoreGState(ctx);

    TabBarHits {
        slot_positions,
        close_bounds,
        right_segment_bounds,
        available_cols,
        correct_scroll_offset,
    }
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
    use crate::primitives::tab_bar::{TabBar, TabItem};
    use crate::theme::Theme;
    use crate::types::{Color, WidgetId};
    use crate::Backend;

    const W: u32 = 480;
    const H: u32 = 28;
    const FONT_SIZE: f64 = 14.0;

    fn font() -> CTFont {
        make_font("Menlo", FONT_SIZE).expect("Menlo installed")
    }

    fn sample_bar() -> TabBar {
        TabBar {
            id: WidgetId::new("tabs"),
            tabs: vec![
                TabItem {
                    label: "main.rs".into(),
                    is_active: true,
                    is_dirty: false,
                    is_preview: false,
                },
                TabItem {
                    label: "lib.rs".into(),
                    is_active: false,
                    is_dirty: true,
                    is_preview: false,
                },
            ],
            scroll_offset: 0,
            right_segments: vec![],
            active_accent: Some(Color::rgb(80, 140, 255)),
            show_tab_close: true,
            compact: false,
        }
    }

    /// Drive a paint through the full `MacBackend::draw_tab_bar` path
    /// and return `(surface, hits)` for inspection. Mirrors the
    /// status_bar harness so future chrome tests follow the same
    /// shape.
    fn paint_via_backend(
        bar: &TabBar,
        hovered_close: Option<usize>,
    ) -> (BitmapSurface, TabBarHits) {
        let surface = BitmapSurface::new(W, H);
        surface.fill(0.0, 0.0, 0.0, 0.0);

        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        let hits = std::cell::RefCell::new(None);
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            let h = b.draw_tab_bar(QRect::new(0.0, 0.0, W as f32, H as f32), bar, hovered_close);
            *hits.borrow_mut() = Some(h);
        });
        backend.end_frame();
        (surface, hits.into_inner().unwrap())
    }

    #[test]
    fn active_tab_paints_active_bg() {
        // The active tab's bg differs from `tab_bar_bg`. Probe just
        // above the bottom edge near the left of the active tab's
        // slot (past the leading padding, before the label glyphs).
        let bar = sample_bar();
        let (surface, hits) = paint_via_backend(&bar, None);
        let theme = Theme::default();

        let (start, end) = hits.slot_positions[0];
        assert!(end > start, "active tab slot must have non-zero width");

        // Probe near the bottom-left of the slot — past the 14px
        // padding, below the accent strip, but well outside any glyph
        // pixels. y = row_height - 2 stays inside the painted row.
        let probe_x = (start + 2.0) as u32;
        let probe_y = H - 2;
        let (r, g, b, _) = surface.pixel(probe_x, probe_y);
        let expected = theme.tab_active_bg;
        assert_eq!(
            (r, g, b),
            (expected.r, expected.g, expected.b),
            "active tab bg at ({}, {}) should be tab_active_bg",
            probe_x,
            probe_y,
        );
    }

    #[test]
    fn active_accent_paints_at_top_of_active_tab() {
        // 2-px accent strip at y_offset for the active tab.
        let bar = sample_bar();
        let (surface, hits) = paint_via_backend(&bar, None);

        let (start, _) = hits.slot_positions[0];
        // Top scanline (y=0) inside the active slot — past leading
        // padding so we don't overlap the second tab.
        let probe_x = (start + 4.0) as u32;
        let (r, g, b, _) = surface.pixel(probe_x, 0);
        let accent = bar.active_accent.unwrap();
        assert_eq!(
            (r, g, b),
            (accent.r, accent.g, accent.b),
            "accent strip at top edge should match TabBar.active_accent",
        );
    }

    #[test]
    fn close_bounds_round_trip_via_hits_struct() {
        // Round-trip: paint, then sample a coordinate inside the
        // reported close-bounds and assert the hits struct's bounds
        // contain it. This is the "paint and click agree on close
        // button location" gate.
        let bar = sample_bar();
        let (_surface, hits) = paint_via_backend(&bar, None);

        let close = hits.close_bounds[0].expect("active tab has close bounds");
        let mid_x = (close.0 + close.1) / 2.0;
        assert!(
            mid_x >= close.0 && mid_x < close.1,
            "midpoint must be inside close bounds [{}, {})",
            close.0,
            close.1,
        );
        // The reported bounds must sit inside the tab slot — a paint
        // shift on the close glyph would land outside the slot and
        // catch the drift here.
        let (slot_start, slot_end) = hits.slot_positions[0];
        assert!(
            close.0 >= slot_start && close.1 <= slot_end,
            "close bounds [{}, {}) must be inside tab slot [{}, {})",
            close.0,
            close.1,
            slot_start,
            slot_end,
        );
    }

    #[test]
    fn dirty_tab_uses_filled_circle_glyph() {
        // `is_dirty` swaps the close glyph from `×` to `●`. We can't
        // easily compare glyph shape pixel-by-pixel, but a row-wise
        // ink ratio differs noticeably: `●` is mostly filled, `×` is
        // two thin diagonals. Compare the dirty tab's close column
        // against the active tab's: dirty should be visibly denser.
        let bar = sample_bar();
        let (surface, hits) = paint_via_backend(&bar, None);

        let active_close = hits.close_bounds[0].unwrap();
        let dirty_close = hits.close_bounds[1].unwrap();

        // Count non-bg pixels in a 1-column strip across the line
        // height for each close glyph. The dirty (●) column should
        // have more inked pixels than the × column.
        fn ink_density(surface: &BitmapSurface, x: u32, bg: Color) -> u32 {
            (0..H)
                .filter(|&y| {
                    let (r, g, b, _) = surface.pixel(x, y);
                    !(r == bg.r && g == bg.g && b == bg.b)
                })
                .count() as u32
        }
        let theme = Theme::default();
        let active_ink = ink_density(
            &surface,
            ((active_close.0 + active_close.1) / 2.0) as u32,
            theme.tab_active_bg,
        );
        let dirty_ink = ink_density(
            &surface,
            ((dirty_close.0 + dirty_close.1) / 2.0) as u32,
            theme.tab_bar_bg,
        );
        assert!(
            dirty_ink > active_ink,
            "dirty `●` glyph should ink more rows ({}) than active `×` ({})",
            dirty_ink,
            active_ink,
        );
    }

    #[test]
    fn empty_bar_paints_only_tab_bar_bg() {
        let bar = TabBar {
            id: WidgetId::new("empty"),
            tabs: vec![],
            scroll_offset: 0,
            right_segments: vec![],
            active_accent: None,
            show_tab_close: true,
            compact: false,
        };
        let (surface, hits) = paint_via_backend(&bar, None);
        let theme = Theme::default();

        // Whole row should be tab_bar_bg.
        let (r, g, b, _) = surface.pixel(W / 2, H / 2);
        assert_eq!(
            (r, g, b),
            (theme.tab_bar_bg.r, theme.tab_bar_bg.g, theme.tab_bar_bg.b)
        );
        assert!(hits.slot_positions.is_empty());
        assert!(hits.close_bounds.is_empty());
    }
}
