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
// `TabBarHits` is `#[deprecated]` (issue #823) — `mac_tab_bar_layout`
// still constructs it directly rather than narrowing it down from a
// `TabBarLayout`, per #504's original audit — so the import needs the
// same allow every use site below does. Issue #919 added
// `mac_tab_bar_native_layout`, a *second*, independently-computed
// function that builds the real `TabBarLayout` `Backend::resolve_tab_bar_layout`
// exposes; see that function's doc for why it duplicates rather than
// shares `mac_tab_bar_layout`'s measurement code.
use crate::event::Rect;
#[allow(deprecated)]
use crate::primitives::tab_bar::{TabBar, TabBarHits};
use crate::primitives::tab_bar::{TabBarHit, TabBarLayout, VisibleSegment, VisibleTab};
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
/// Slack added either side of the close glyph when building its hit box.
const CLOSE_PAD: f64 = 2.0;

/// Compute the [`TabBarHits`] [`draw_tab_bar`] would produce for `bar` at
/// `width`, without painting.
///
/// This is the single measurement path: [`draw_tab_bar`] calls it too and
/// paints from its output, so the no-paint twin backing
/// [`crate::Backend::tab_bar_layout`] cannot drift from what was painted.
///
/// # Coordinate space — bar-relative, not absolute (known divergence)
///
/// The [`crate::Backend::tab_bar_layout`] doc pins the *contract* at
/// target-surface (absolute) coordinates, and the TUI / GTK backends shift
/// by `rect.x` to honour it. The macOS rasteriser is not handed `rect.x`
/// at all — [`crate::macos::MacBackend::draw_tab_bar`] passes only
/// `rect.width` / `rect.y`, and paints tabs from `x = 0` — so both its
/// paint and its hits are bar-relative. Making only the *hits* absolute
/// here would put them out of step with the pixels, which is strictly
/// worse than a documented offset. Closing the gap properly means teaching
/// the rasteriser to paint at `rect.x`; that is a behaviour change to the
/// live tab bar and is deliberately left to the #552 follow-up rather than
/// smuggled into quadraui#484's compile fix. What this function guarantees
/// is the invariant that is actually load-bearing: `tab_bar_layout` returns
/// exactly what `draw_tab_bar` painted.
#[allow(deprecated)] // returns the deprecated `TabBarHits` — issue #823
pub fn mac_tab_bar_layout(font: &CTFont, width: f64, bar: &TabBar) -> TabBarHits {
    let tab_pad = if bar.compact { 2.0 } else { TAB_PAD };
    let tab_inner_gap = if bar.compact { 4.0 } else { TAB_INNER_GAP };
    let tab_outer_gap = if bar.compact { 0.0 } else { TAB_OUTER_GAP };

    // ── Right-segment widths (reserved before tabs get their budget) ──
    let right_widths: Vec<f64> = bar
        .right_segments
        .iter()
        .map(|seg| measure_text(font, &seg.text).0)
        .collect();
    let reserved_px: f64 = right_widths.iter().sum();
    let effective_tab_area = (width - reserved_px).max(0.0);

    // Close-glyph width measured once — individual tabs use it
    // conditionally based on `bar.show_tab_close && tab.is_closable`. The
    // `●` dirty variant is the same width in Menlo and most monospace
    // fonts.
    let close_w = if bar.show_tab_close {
        measure_text(font, "×").0
    } else {
        0.0
    };
    let close_extra_for = |tab_idx: usize| -> f64 {
        if bar.show_tab_close && bar.tabs[tab_idx].is_closable {
            tab_inner_gap + close_w
        } else {
            0.0
        }
    };

    // Pre-measure every tab's full slot width — used for scroll-offset
    // resolution.
    let tab_slot_widths: Vec<f64> = bar
        .tabs
        .iter()
        .enumerate()
        .map(|(i, tab)| {
            let (name_w, _) = measure_text(font, &tab.label);
            tab_pad + name_w + close_extra_for(i) + tab_pad + tab_outer_gap
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

    // ── Slot geometry ────────────────────────────────────────────────
    let mut slot_positions: Vec<(f64, f64)> = Vec::with_capacity(bar.tabs.len());
    let mut close_bounds: Vec<Option<(f64, f64)>> = Vec::with_capacity(bar.tabs.len());
    for _ in 0..bar.scroll_offset.min(bar.tabs.len()) {
        slot_positions.push((0.0, 0.0));
        close_bounds.push(None);
    }

    let mut x = 0.0_f64;
    for (tab_idx, tab) in bar.tabs.iter().enumerate().skip(bar.scroll_offset) {
        let (tab_name_w, _) = measure_text(font, &tab.label);
        let tab_content_w = tab_pad + tab_name_w + close_extra_for(tab_idx) + tab_pad;
        let slot_w = tab_content_w + tab_outer_gap;
        if x + slot_w > effective_tab_area {
            break;
        }
        slot_positions.push((x, x + slot_w));

        if bar.show_tab_close && tab.is_closable {
            let close_x = x + tab_pad + tab_name_w + tab_inner_gap;
            close_bounds.push(Some((close_x - CLOSE_PAD, close_x + close_w + CLOSE_PAD)));
        } else {
            close_bounds.push(None);
        }

        x += slot_w;
    }

    // ── Right segments ───────────────────────────────────────────────
    let mut right_segment_bounds: Vec<(f64, f64)> = Vec::with_capacity(right_widths.len());
    let mut sx = width - reserved_px;
    for seg_w in &right_widths {
        right_segment_bounds.push((sx, sx + seg_w));
        sx += seg_w;
    }

    // Cell-width estimation: 15-char sample width / 15 → average glyph
    // advance. Matches the GTK convention so app-level char-col math
    // doesn't diverge across backends.
    let (sample_px, _) = measure_text(font, CELL_WIDTH_SAMPLE);
    let char_w = (sample_px / CELL_WIDTH_SAMPLE.chars().count() as f64).max(1.0);
    let available_cols = (effective_tab_area / char_w).floor().max(0.0) as usize;

    TabBarHits {
        slot_positions,
        close_bounds,
        right_segment_bounds,
        available_cols,
        correct_scroll_offset,
    }
}

/// Compute the [`TabBarLayout`] [`draw_tab_bar`] paints, without
/// painting — issue #919's `TabBarLayout`-returning counterpart to
/// [`mac_tab_bar_layout`], backing [`crate::Backend::resolve_tab_bar_layout`].
///
/// # Why this duplicates `mac_tab_bar_layout` instead of sharing it
///
/// Every other backend derives its `TabBarHits` by calling
/// [`crate::TabBar::layout`] (the shared D6 layout API) and narrowing the
/// resulting `TabBarLayout` down via
/// [`crate::backend::tab_bar_hits_from_layout`]. macOS never adopted that
/// path (#504's audit) — `mac_tab_bar_layout` above measures and
/// positions tabs by hand, one `f64` tuple at a time, with no
/// intermediate `TabBarLayout` to source native `Rect` coordinates from.
/// Retrofitting `mac_tab_bar_layout` onto the shared layout API is real,
/// independent rasteriser work (out of scope for this additive-only
/// issue — see its "Scope" section), so this function instead mirrors
/// `mac_tab_bar_layout`'s measurement expressions line-for-line and
/// builds a `TabBarLayout` directly. `native_layout_agrees_with_hits_layout`
/// (below) pins the two against each other so they cannot silently
/// drift apart.
///
/// # Coordinate space — bar-relative, matching `mac_tab_bar_layout`
///
/// Same x-axis convention as `mac_tab_bar_layout` (see its doc): macOS
/// paints tabs from `x = 0`, never shifted by `rect.x`, so both this
/// and that function are already bar-relative in x — no `TabBarHits`
/// absolute-coordinate divergence to account for. `height` (the caller's
/// `rect.height`) becomes every tab's `bounds.height`, and every
/// `bounds.y` is `0.0` — [`TabBarLayout`]'s own documented convention
/// (origin at the bar's top-left).
pub fn mac_tab_bar_native_layout(
    font: &CTFont,
    width: f64,
    height: f64,
    bar: &TabBar,
) -> TabBarLayout {
    let tab_pad = if bar.compact { 2.0 } else { TAB_PAD };
    let tab_inner_gap = if bar.compact { 4.0 } else { TAB_INNER_GAP };
    let tab_outer_gap = if bar.compact { 0.0 } else { TAB_OUTER_GAP };

    // ── Right-segment widths (reserved before tabs get their budget) ──
    let right_widths: Vec<f64> = bar
        .right_segments
        .iter()
        .map(|seg| measure_text(font, &seg.text).0)
        .collect();
    let reserved_px: f64 = right_widths.iter().sum();
    let effective_tab_area = (width - reserved_px).max(0.0);

    let close_w = if bar.show_tab_close {
        measure_text(font, "×").0
    } else {
        0.0
    };
    let close_extra_for = |tab_idx: usize| -> f64 {
        if bar.show_tab_close && bar.tabs[tab_idx].is_closable {
            tab_inner_gap + close_w
        } else {
            0.0
        }
    };

    // Pre-measure every tab's full slot width — used for scroll-offset
    // resolution, exactly as `mac_tab_bar_layout` does.
    let tab_slot_widths: Vec<f64> = bar
        .tabs
        .iter()
        .enumerate()
        .map(|(i, tab)| {
            let (name_w, _) = measure_text(font, &tab.label);
            tab_pad + name_w + close_extra_for(i) + tab_pad + tab_outer_gap
        })
        .collect();

    let active_idx = bar.tabs.iter().position(|t| t.is_active);
    let resolved_scroll_offset = if let Some(active) = active_idx {
        TabBar::fit_active_scroll_offset(active, bar.tabs.len(), effective_tab_area as usize, |i| {
            tab_slot_widths[i] as usize
        })
    } else {
        bar.scroll_offset
    };

    // ── Visible tabs ─────────────────────────────────────────────────
    // Painted from `bar.scroll_offset` (the caller's value), not the
    // freshly-resolved `resolved_scroll_offset` above — exactly what
    // `mac_tab_bar_layout` does: the "engine feedback" correction is
    // reported back for the *next* frame's caller to apply, not applied
    // to this one. Unlike `TabBarHits`, there is no `(0.0, 0.0)`
    // sentinel padding to keep vector indices aligned — `visible_tabs`
    // is sparse and carries its own `tab_idx`, matching every other
    // backend's `TabBarLayout`.
    let mut visible_tabs: Vec<VisibleTab> = Vec::new();
    let mut close_regions: Vec<(Rect, TabBarHit)> = Vec::new();
    let mut body_regions: Vec<(Rect, TabBarHit)> = Vec::new();

    let mut x = 0.0_f64;
    for (tab_idx, tab) in bar.tabs.iter().enumerate().skip(bar.scroll_offset) {
        let (tab_name_w, _) = measure_text(font, &tab.label);
        let tab_content_w = tab_pad + tab_name_w + close_extra_for(tab_idx) + tab_pad;
        let slot_w = tab_content_w + tab_outer_gap;
        if x + slot_w > effective_tab_area {
            break;
        }
        let bounds = Rect::new(x as f32, 0.0, slot_w as f32, height as f32);

        let close_bounds = if bar.show_tab_close && tab.is_closable {
            let close_x = x + tab_pad + tab_name_w + tab_inner_gap;
            let cb = Rect::new(
                (close_x - CLOSE_PAD) as f32,
                0.0,
                (close_w + 2.0 * CLOSE_PAD) as f32,
                height as f32,
            );
            close_regions.push((cb, TabBarHit::TabClose(tab_idx)));
            Some(cb)
        } else {
            None
        };

        body_regions.push((bounds, TabBarHit::Tab(tab_idx)));
        visible_tabs.push(VisibleTab {
            tab_idx,
            bounds,
            close_bounds,
        });

        x += slot_w;
    }

    // Close regions before body regions — matches `TabBar::layout`'s own
    // hit-region ordering (close-before-body) so `hit_test` returns the
    // more-specific close hit when the pointer is on the × glyph.
    let mut hit_regions: Vec<(Rect, TabBarHit)> =
        Vec::with_capacity(close_regions.len() + body_regions.len());
    hit_regions.extend(close_regions);
    hit_regions.extend(body_regions);

    // ── Right segments ───────────────────────────────────────────────
    let mut visible_segments: Vec<VisibleSegment> = Vec::with_capacity(right_widths.len());
    let mut sx = width - reserved_px;
    for (segment_idx, seg_w) in right_widths.iter().enumerate() {
        let bounds = Rect::new(sx as f32, 0.0, *seg_w as f32, height as f32);
        let seg = &bar.right_segments[segment_idx];
        let clickable = seg.id.is_some();
        if let Some(id) = &seg.id {
            hit_regions.push((bounds, TabBarHit::RightSegment(id.clone())));
        }
        visible_segments.push(VisibleSegment {
            segment_idx,
            bounds,
            clickable,
        });
        sx += seg_w;
    }

    TabBarLayout {
        bar_width: width as f32,
        bar_height: height as f32,
        visible_tabs,
        visible_segments,
        scroll_left: None,
        scroll_right: None,
        hit_regions,
        resolved_scroll_offset,
    }
}

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
#[allow(deprecated)] // returns the deprecated `TabBarHits` — issue #823
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
    let tab_outer_gap = if bar.compact { 0.0 } else { TAB_OUTER_GAP };

    // Single source of truth for geometry — `Backend::tab_bar_layout`
    // calls the same function, so the no-paint twin can never drift from
    // what this loop actually paints.
    let hits = mac_tab_bar_layout(font, width, bar);

    CGContextSaveGState(ctx);

    // Tab-bar background.
    fill_rect(ctx, 0.0, y_offset, width, row_height, theme.tab_bar_bg);

    // ── Tabs paint loop ──────────────────────────────────────────────
    // `slot_positions` is padded with `(0.0, 0.0)` for the scrolled-past
    // tabs, so start after them and stop where the layout stopped.
    for (tab_idx, tab) in bar
        .tabs
        .iter()
        .enumerate()
        .skip(bar.scroll_offset)
        .take(hits.slot_positions.len().saturating_sub(bar.scroll_offset))
    {
        let (slot_x, slot_end) = hits.slot_positions[tab_idx];
        let tab_content_w = slot_end - slot_x - tab_outer_gap;

        // Tab background.
        let bg_col = if tab.is_active {
            theme.tab_active_bg
        } else {
            theme.tab_bar_bg
        };
        fill_rect(ctx, slot_x, y_offset, tab_content_w, row_height, bg_col);

        // Top accent line for the active tab.
        if tab.is_active {
            if let Some(accent) = bar.active_accent {
                fill_rect(ctx, slot_x, y_offset, tab_content_w, ACCENT_HEIGHT, accent);
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
            slot_x + tab_pad,
            text_y_offset,
            color_to_cg(fg_col),
        );

        // Close glyph (× or ● for dirty), tinted on hover. Painted at the
        // exact x the hit box was built around, so a click that lands in
        // `close_bounds` lands on the glyph the user saw.
        if let Some((close_lo, _)) = hits.close_bounds[tab_idx] {
            let close_x = close_lo + CLOSE_PAD;
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
    }

    // ── Right segments paint loop ────────────────────────────────────
    for (i, seg) in bar.right_segments.iter().enumerate() {
        let (sx, _) = hits.right_segment_bounds[i];
        let fg_col = if seg.is_active {
            theme.tab_active_fg
        } else {
            theme.tab_inactive_fg
        };
        draw_text(ctx, font, &seg.text, sx, text_y_offset, color_to_cg(fg_col));
    }

    CGContextRestoreGState(ctx);

    hits
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
                    is_closable: true,
                },
                TabItem {
                    label: "lib.rs".into(),
                    is_active: false,
                    is_dirty: true,
                    is_preview: false,
                    is_closable: true,
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
    #[allow(deprecated)] // returns the deprecated `TabBarHits` — issue #823
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
    #[allow(deprecated)] // exercises the deprecated `TabBarHits` — issue #823
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
    #[allow(deprecated)] // exercises the deprecated `TabBarHits` — issue #823
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
    #[allow(deprecated)] // exercises the deprecated `TabBarHits` — issue #823
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
    #[allow(deprecated)] // exercises the deprecated `TabBarHits` — issue #823
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
    #[allow(deprecated)] // exercises the deprecated `TabBarHits` — issue #823
    fn non_closable_tab_has_no_close_bounds_even_when_bar_show_close_is_true() {
        // Regression: `bar.show_tab_close = true` is a bar-level flag, but
        // individual tabs may opt out via `is_closable = false`. The macOS
        // rasteriser must consult both — otherwise non-closable tabs render
        // a phantom × that triggers no action when clicked.
        // Use identical labels so the *only* width difference between the
        // two slots is the close-glyph reservation, isolating the regression.
        let bar = TabBar {
            id: WidgetId::new("tabs"),
            tabs: vec![
                TabItem {
                    label: "tab.rs".into(),
                    is_active: true,
                    is_dirty: false,
                    is_preview: false,
                    is_closable: true,
                },
                TabItem {
                    label: "tab.rs".into(),
                    is_active: false,
                    is_dirty: false,
                    is_preview: false,
                    is_closable: false,
                },
            ],
            scroll_offset: 0,
            right_segments: vec![],
            active_accent: None,
            show_tab_close: true,
            compact: false,
        };
        let (_surface, hits) = paint_via_backend(&bar, None);
        assert!(
            hits.close_bounds[0].is_some(),
            "closable tab must have close bounds",
        );
        assert!(
            hits.close_bounds[1].is_none(),
            "non-closable tab must have no close bounds even when bar.show_tab_close is true",
        );
        // The non-closable tab's slot must also be narrower (no close-glyph
        // reservation), so the two slot widths differ in proportion to the
        // close glyph + inner gap.
        let (s0, e0) = hits.slot_positions[0];
        let (s1, e1) = hits.slot_positions[1];
        let closable_slot = e0 - s0;
        let pinned_slot = e1 - s1;
        assert!(
            closable_slot > pinned_slot,
            "closable tab slot ({closable_slot}) should reserve more width than pinned slot ({pinned_slot})",
        );
    }

    #[test]
    #[allow(deprecated)] // exercises the deprecated `TabBarHits` — issue #823
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

    /// `Backend::tab_bar_layout` must return exactly what
    /// `draw_tab_bar` painted — both route through `mac_tab_bar_layout`,
    /// and this pins that (quadraui#484). See that function's docs for
    /// the known bar-relative-vs-absolute divergence (#552); what is
    /// guaranteed here is that layout == paint.
    #[test]
    #[allow(deprecated)] // exercises the deprecated `TabBarHits` — issue #823
    fn layout_twin_matches_the_painted_hits() {
        let bar = sample_bar();
        let (_surface, painted) = paint_via_backend(&bar, None);

        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        let computed = backend.tab_bar_layout(QRect::new(0.0, 0.0, W as f32, H as f32), &bar);

        assert_eq!(painted.slot_positions, computed.slot_positions);
        assert_eq!(painted.close_bounds, computed.close_bounds);
        assert_eq!(painted.right_segment_bounds, computed.right_segment_bounds);
        assert_eq!(painted.available_cols, computed.available_cols);
        assert_eq!(
            painted.correct_scroll_offset,
            computed.correct_scroll_offset
        );
    }

    /// Issue #919: `mac_tab_bar_native_layout` — the new `TabBarLayout`-
    /// returning function backing `Backend::resolve_tab_bar_layout` —
    /// must report exactly the same geometry `mac_tab_bar_layout`
    /// (the deprecated `TabBarHits`-returning one) does, since the two
    /// duplicate rather than share their measurement code (see
    /// `mac_tab_bar_native_layout`'s doc for why). Converting the new
    /// function's `TabBarLayout` through the shared
    /// `tab_bar_hits_from_layout` helper and comparing field-by-field
    /// against the old function's direct output pins the two together.
    #[test]
    #[allow(deprecated)] // exercises the deprecated `TabBarHits` — issue #823
    fn native_layout_agrees_with_hits_layout() {
        let bar = sample_bar();
        let f = font();

        let hits = mac_tab_bar_layout(&f, W as f64, &bar);
        let native = mac_tab_bar_native_layout(&f, W as f64, H as f64, &bar);
        let native_as_hits = crate::backend::tab_bar_hits_from_layout(&native, &bar);

        assert_eq!(
            hits.slot_positions, native_as_hits.slot_positions,
            "tab slots must agree between the two independently-computed layouts"
        );
        assert_eq!(
            hits.close_bounds, native_as_hits.close_bounds,
            "close-button spans must agree too"
        );
        assert_eq!(
            hits.right_segment_bounds, native_as_hits.right_segment_bounds,
            "right-segment spans must agree too"
        );
        assert_eq!(
            hits.correct_scroll_offset, native.resolved_scroll_offset,
            "the \"fit active tab\" scroll correction must agree"
        );
    }

    /// A click at the centre of a reported close box lands on the close
    /// glyph the rasteriser actually drew — the paint↔click round trip
    /// the shared `mac_tab_bar_layout` exists to guarantee.
    #[test]
    #[allow(deprecated)] // exercises the deprecated `TabBarHits` — issue #823
    fn close_box_centre_is_where_the_glyph_was_painted() {
        let bar = sample_bar();
        let (surface, hits) = paint_via_backend(&bar, None);
        let theme = Theme::default();
        let (lo, hi) = hits.close_bounds[0].expect("tab 0 is closable");
        let bg = theme.tab_active_bg;

        // Somewhere in the close box's x-span, some row differs from the
        // tab background: that is the glyph.
        let inked = (lo.ceil() as u32..hi.floor() as u32).any(|x| {
            (0..H).any(|y| {
                let (r, g, b, _) = surface.pixel(x.min(W - 1), y);
                (r, g, b) != (bg.r, bg.g, bg.b)
            })
        });
        assert!(
            inked,
            "close box [{lo}, {hi}] contains no painted glyph — hits and paint have drifted",
        );
    }
}
