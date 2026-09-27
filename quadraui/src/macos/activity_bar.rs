//! macOS rasteriser for [`crate::ActivityBar`].
//!
//! Vertical strip of icon rows. Layout (row positions, top-vs-bottom
//! placement) is the shared [`crate::primitives::activity_bar::ActivityBar::layout`]
//! (matching `gtk::activity_bar` / `win::activity_bar`, since #1081 — see
//! below). Content painting moved to the shared
//! [`crate::primitives::activity_bar::native_surface_paint::paint`]
//! (#1081, `NativeSurface` Phase 4 slice 5/8) — see that fn's module doc
//! for the drift it resolved, including two real gaps this backend used
//! to have (no keyboard-selection highlight at all, and a private
//! `row_plan` helper that ordered `visible_items` differently from GTK/
//! Windows — this module's own doc used to flag that as a "known
//! divergence, deliberately left alone"; unifying onto one `paint`
//! finally resolves it rather than parking it again).
//!
//! Uses the backend's active `CTFont` directly for icon rendering
//! instead of GTK's hardcoded "Symbols Nerd Font" — apps that want
//! Nerd-Font icons install a glyph-bearing font via
//! [`super::MacBackend::set_current_font`] in `setup()`.
//!
//! Returns per-row [`ActivityBarRowHit`]s so callers can route clicks
//! and query tooltips against the same frame's painted positions.
//!
//! # Coordinate space (issue #552 / #934)
//!
//! Spans are **bar-relative**, matching the [`crate::Backend::draw_activity_bar`]
//! contract: this function paints into `(0, 0, width, height)` and is never
//! handed the bar's origin at all, so it *cannot* fold the origin in — the
//! mistake the TUI rasteriser made. That part was audited under #552 and is
//! still compliant; no change needed here.
//!
//! What #552's audit missed: painting into `(0, 0, width, height)` only
//! lands at the bar's *actual* screen position if the caller separately
//! places the CGContext's origin there first. Before #934,
//! `MacBackend::draw_activity_bar[_with_style]` called straight into this
//! module with no such adjustment, so every frame painted the strip at the
//! CGContext's literal `(0, 0)` regardless of where `rect` said the bar
//! should be. Those two methods now wrap this module's calls in
//! `CGContextTranslateCTM(ctx, rect.x, rect.y)` (save/translate/restore),
//! mirroring `GtkBackend::draw_activity_bar`'s `cr.translate(rect.x,
//! rect.y)` — this module itself needed no change, since bar-relative paint
//! is exactly what a translated context wants.

use core_graphics::sys::CGContextRef;
use core_text::font::CTFont;

use crate::primitives::activity_bar::{
    native_surface_paint, ActivityBar, ActivityBarRowHit, ActivityBarStyle,
};
use crate::theme::Theme;

/// Fixed row height in points. Matches `crate::gtk::activity_bar::ACTIVITY_ROW_PX`
/// (= the vimcode native-button height baked into the GTK CSS).
pub const ACTIVITY_ROW_PX: f64 = 48.0;

/// Compute the per-row hit spans [`draw_activity_bar`] would produce for
/// `bar` in a `width` × `height` strip, without painting.
///
/// No-paint twin backing [`crate::Backend::activity_bar_layout`]. Spans
/// are **bar-relative** — `y_start` / `y_end` measured from the top edge
/// of the bar, first row at `0.0` — exactly as the trait requires
/// (quadraui#552). Delegates to the shared
/// [`crate::primitives::activity_bar::ActivityBar::layout`] (#1081) —
/// same call `draw_activity_bar` makes internally, so the two can't
/// drift apart, and `visible_items` order now matches
/// `gtk::activity_bar` / `win::activity_bar` (bottom-pinned items
/// first) instead of this module's old top-first `row_plan`.
pub fn mac_activity_bar_layout(
    width: f64,
    height: f64,
    bar: &ActivityBar,
) -> Vec<ActivityBarRowHit> {
    bar.layout(width as f32, height as f32, ACTIVITY_ROW_PX as f32)
        .visible_items
        .into_iter()
        .map(|vi| {
            let item = match vi.side {
                crate::primitives::activity_bar::ActivitySide::Top => &bar.top_items[vi.item_idx],
                crate::primitives::activity_bar::ActivitySide::Bottom => {
                    &bar.bottom_items[vi.item_idx]
                }
            };
            ActivityBarRowHit {
                y_start: vi.bounds.y,
                y_end: vi.bounds.y + vi.bounds.height,
                id: item.id.clone(),
                tooltip: item.tooltip.clone(),
            }
        })
        .collect()
}

/// Paint `bar` into `(0, 0, width, height)` on `ctx`.
///
/// Equivalent to [`draw_activity_bar_with_style`] with
/// `ActivityBarStyle::default()`, i.e. no active-row fill — see that
/// function for the full behaviour and #658's reasoning for why the fill
/// lives in a separate style value rather than a field on [`ActivityBar`].
///
/// `nerd_fonts_enabled` picks which half of each item's [`crate::Icon`]
/// paints — `glyph` when `true`, `fallback` when `false` (issue #683).
/// Unlike GTK's hardcoded "Symbols Nerd Font", macOS paints with whatever
/// font `set_current_font` installed (see this module's header), so a
/// caller that hasn't installed a glyph-bearing font should pass `false`
/// — a wrong `false` shows a plain-but-correct glyph, a wrong `true` shows
/// tofu.
///
/// # Safety
///
/// `ctx` must be a valid `CGContextRef` borrowed for the duration of
/// the call.
#[allow(clippy::too_many_arguments)]
pub unsafe fn draw_activity_bar(
    ctx: CGContextRef,
    font: &CTFont,
    width: f64,
    height: f64,
    bar: &ActivityBar,
    theme: &Theme,
    hovered_idx: Option<usize>,
    nerd_fonts_enabled: bool,
) -> Vec<ActivityBarRowHit> {
    draw_activity_bar_with_style(
        ctx,
        font,
        width,
        height,
        bar,
        &ActivityBarStyle::default(),
        theme,
        hovered_idx,
        nerd_fonts_enabled,
    )
}

/// [`draw_activity_bar`] with an explicit [`ActivityBarStyle`] request
/// (#658). `ActivityBarStyle::default()` reproduces [`draw_activity_bar`]
/// pixel for pixel.
///
/// # Safety
///
/// `ctx` must be a valid `CGContextRef` borrowed for the duration of
/// the call.
#[allow(clippy::too_many_arguments)]
pub unsafe fn draw_activity_bar_with_style(
    ctx: CGContextRef,
    font: &CTFont,
    width: f64,
    height: f64,
    bar: &ActivityBar,
    style: &ActivityBarStyle,
    theme: &Theme,
    hovered_idx: Option<usize>,
    nerd_fonts_enabled: bool,
) -> Vec<ActivityBarRowHit> {
    CGContextSaveGState(ctx);

    let layout = bar.layout(width as f32, height as f32, ACTIVITY_ROW_PX as f32);
    let mut surface = super::surface::CgSurface {
        ctx,
        font: Some(font),
    };
    let regions = native_surface_paint::paint(
        bar,
        &layout,
        style,
        &mut surface,
        theme,
        hovered_idx,
        nerd_fonts_enabled,
    );

    CGContextRestoreGState(ctx);
    regions
}

extern "C" {
    fn CGContextSaveGState(c: CGContextRef);
    fn CGContextRestoreGState(c: CGContextRef);
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
            is_keyboard_focused: false,
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

    /// Like [`paint_via_backend`] but paints into a canvas larger than the
    /// bar itself, at an arbitrary `(x, y)` origin — models the bar sitting
    /// below a title bar and/or right of other chrome (issue #934).
    fn paint_via_backend_at(
        bar: &ActivityBar,
        hovered: Option<usize>,
        x: f32,
        y: f32,
        canvas_w: u32,
        canvas_h: u32,
    ) -> (BitmapSurface, Vec<ActivityBarRowHit>) {
        let surface = BitmapSurface::new(canvas_w, canvas_h);
        surface.fill(0.0, 0.0, 0.0, 0.0);

        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        backend.begin_frame(Viewport::new(canvas_w as f32, canvas_h as f32, 1.0));
        let regions = std::cell::RefCell::new(Vec::new());
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            let r = b.draw_activity_bar(QRect::new(x, y, W as f32, H as f32), bar, hovered);
            *regions.borrow_mut() = r;
        });
        backend.end_frame();
        (surface, regions.into_inner())
    }

    fn paint_via_backend_with_style(
        bar: &ActivityBar,
        style: &crate::ActivityBarStyle,
        hovered: Option<usize>,
    ) -> (BitmapSurface, Vec<ActivityBarRowHit>) {
        let surface = BitmapSurface::new(W, H);
        surface.fill(0.0, 0.0, 0.0, 0.0);

        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        let regions = std::cell::RefCell::new(Vec::new());
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            let r = b.draw_activity_bar_with_style(
                QRect::new(0.0, 0.0, W as f32, H as f32),
                bar,
                hovered,
                style,
            );
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

    /// #658 acceptance: `style.active_bg: Some(..)` + `active_accent: None`
    /// paints a filled active row with **zero** accent-line pixels — the
    /// legacy 2-pt left-edge strip (x ∈ [0, 2)) must show the fill colour,
    /// not any accent tint. macOS parity with the GTK/TUI acceptance tests.
    #[test]
    fn active_bg_fills_row_with_zero_accent_pixels_when_accent_is_none() {
        let mut bar = sample_bar();
        bar.active_accent = None;
        let fill = Color::rgb(49, 50, 51);
        let style = crate::ActivityBarStyle::new().with_active_bg(fill);
        let (surface, _) = paint_via_backend_with_style(&bar, &style, None);

        let probe_y = (ACTIVITY_ROW_PX as u32) / 2;
        for x in [0, 1] {
            let (r, g, b, _) = surface.pixel(x, probe_y);
            assert_eq!(
                (r, g, b),
                (fill.r, fill.g, fill.b),
                "x={x} is inside the legacy accent strip; with active_accent \
                 None it must show the active_bg fill, not any accent tint"
            );
        }
        // Deep in the row, away from the glyph, should also be filled.
        let (r, g, b, _) = surface.pixel(W - 6, probe_y);
        assert_eq!((r, g, b), (fill.r, fill.g, fill.b));
    }

    #[test]
    fn row_hits_cover_painted_rows() {
        // Round-trip: returned regions must point at where the icon
        // was painted. Verify each region's y-span matches the
        // ACTIVITY_ROW_PX grid AND its id matches the item.
        //
        // #1081: `visible_items` order is now bottom-pinned-first (the
        // shared `ActivityBar::layout`'s order — matching
        // `gtk::activity_bar` / `win::activity_bar`), not this module's
        // old top-first `row_plan` order. Only the *order* of these
        // assertions changed; the y-spans themselves are unchanged.
        let bar = sample_bar();
        let (_surface, regions) = paint_via_backend(&bar, None);
        // 2 top + 1 bottom = 3 visible rows.
        assert_eq!(regions.len(), 3);

        // Bottom-pinned item comes first.
        assert_eq!(regions[0].y_start, H as f32 - ACTIVITY_ROW_PX as f32);
        assert_eq!(regions[0].y_end, H as f32);
        assert_eq!(regions[0].id, WidgetId::new("activity:settings"));

        assert_eq!(regions[1].y_start, 0.0);
        assert_eq!(regions[1].y_end, ACTIVITY_ROW_PX as f32);
        assert_eq!(regions[1].id, WidgetId::new("activity:explorer"));

        assert_eq!(regions[2].y_start, ACTIVITY_ROW_PX as f32);
        assert_eq!(regions[2].y_end, 2.0 * ACTIVITY_ROW_PX as f32);
        assert_eq!(regions[2].id, WidgetId::new("activity:search"));
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

    /// #1081 regression: pre-migration macOS's `draw_row` closure had no
    /// `is_keyboard_selected` branch at all — arrow-key navigation was
    /// completely invisible on this backend (`ActivityItem::is_keyboard_selected`'s
    /// own doc already admitted "Both the TUI and GTK rasterizers honour
    /// this flag", i.e. not macOS). The shared
    /// `native_surface_paint::paint` now fills `bar.selection_bg` (or the
    /// `lighten(0.20)` default) for a selected row, same as GTK/Windows.
    #[test]
    fn keyboard_selected_row_paints_selection_bg() {
        let mut bar = sample_bar();
        // Search (top_items[1], not active) is the keyboard-selected row.
        bar.top_items[1].is_keyboard_selected = true;
        let (surface, _) = paint_via_backend(&bar, None);
        let theme = Theme::default();
        let expected = theme.tab_bar_bg.lighten(0.20);

        // Search paints at y ∈ [48, 96) — probe deep in the row, away
        // from the glyph and the (unrelated) accent strip.
        let probe_y = ACTIVITY_ROW_PX as u32 + ACTIVITY_ROW_PX as u32 / 2;
        let (r, g, b, _) = surface.pixel(W - 6, probe_y);
        assert_eq!(
            (r, g, b),
            (expected.r, expected.g, expected.b),
            "keyboard-selected row should paint the selection background \
             (quadraui#1081 — this used to be a no-op on macOS)",
        );
    }

    /// `cargo test -p quadraui --features macos -- --ignored --nocapture macos::activity_bar::tests::dump_smoke_ppm`
    ///
    /// Paints the sample bar (2 top items + 1 bottom-pinned) into a
    /// 48 × 240 surface and writes `/tmp/quadraui_activity_bar.ppm`.
    /// Hover row 1 so the lightened bg is visible. Open in Preview to
    /// confirm:
    /// - Top item (active) has a 2-pt accent strip on the left edge.
    /// - Middle item shows the hover-lightened bg.
    /// - Bottom item is pinned to the bottom edge.
    /// - Right-edge has a 1-pt separator column.
    #[test]
    #[ignore = "writes /tmp/quadraui_activity_bar.ppm — opt in with --ignored"]
    fn dump_smoke_ppm() {
        let bar = sample_bar();
        let (surface, _) = paint_via_backend(&bar, Some(1));
        surface.write_ppm_and_open("/tmp/quadraui_activity_bar.ppm");
    }

    #[test]
    fn hover_lightens_row_background() {
        let bar = sample_bar();
        // Hover `visible_items[1]` — the explorer row (`ActivityBar::layout`
        // orders bottom-pinned items first as of #1081, so index 1 is the
        // first top item, not `search` as it was under this module's old
        // top-first `row_plan`). Hover paints unconditionally regardless of
        // `is_active`, and the default style has no `active_bg`, so the
        // probed pixel is still the plain hover tint either way.
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

    /// `Backend::activity_bar_layout` must return exactly what
    /// `draw_activity_bar` painted — both walk the same `row_plan`, and
    /// this pins that (quadraui#484).
    #[test]
    fn layout_twin_matches_the_painted_rows() {
        let bar = sample_bar();
        let (_surface, painted) = paint_via_backend(&bar, None);

        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        let computed = backend.activity_bar_layout(QRect::new(0.0, 0.0, W as f32, H as f32), &bar);

        assert_eq!(painted.len(), computed.len());
        for (p, c) in painted.iter().zip(computed.iter()) {
            assert_eq!(p.id, c.id);
            assert_eq!(p.tooltip, c.tooltip);
            assert!((p.y_start - c.y_start).abs() < 0.001);
            assert!((p.y_end - c.y_end).abs() < 0.001);
        }
    }

    /// Spans are bar-relative: the first row starts at `0.0` regardless
    /// of where the bar sits, and the no-paint twin agrees
    /// (quadraui#552).
    #[test]
    fn layout_twin_spans_are_bar_relative_at_a_nonzero_origin() {
        let bar = sample_bar();
        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        let at_origin = backend.activity_bar_layout(QRect::new(0.0, 0.0, W as f32, H as f32), &bar);
        let moved = backend.activity_bar_layout(QRect::new(23.0, 41.0, W as f32, H as f32), &bar);

        assert_eq!(at_origin.len(), moved.len());
        assert!((at_origin[0].y_start - 0.0).abs() < 0.001);
        for (a, m) in at_origin.iter().zip(moved.iter()) {
            assert!(
                (a.y_start - m.y_start).abs() < 0.001,
                "row {:?} moved with the bar origin: {} vs {}",
                a.id,
                a.y_start,
                m.y_start,
            );
        }
    }

    /// Issue #934 RED-verify: before the fix, `MacBackend::draw_activity_bar`
    /// forwarded only `rect.width` / `rect.height` into this bar-relative
    /// rasteriser, with no CTM translate to place it at `rect`'s actual
    /// origin — so the strip always painted at the CGContext's literal
    /// `(0, 0)`, no matter where the caller said it should sit (e.g. below
    /// a title bar, as on the reported bug). Painting at a non-zero origin
    /// must leave `(0, 0)` untouched and land the strip's background +
    /// right-edge separator exactly at `(rect.x, rect.y)`.
    #[test]
    fn background_paints_at_rect_origin_not_at_window_origin() {
        const ORIGIN_X: f32 = 30.0;
        const ORIGIN_Y: f32 = 28.0; // e.g. a title bar's height
        let canvas_w = ORIGIN_X as u32 + W;
        let canvas_h = ORIGIN_Y as u32 + H;

        let bar = sample_bar();
        let (surface, regions) =
            paint_via_backend_at(&bar, None, ORIGIN_X, ORIGIN_Y, canvas_w, canvas_h);
        let theme = Theme::default();

        // The window's literal top-left corner must stay exactly as the
        // surface was initialised — fully transparent — never the
        // activity bar's background fill.
        let (_, _, _, a) = surface.pixel(0, 0);
        assert_eq!(
            a, 0,
            "window origin (0,0) must stay untouched when the bar sits at ({ORIGIN_X}, {ORIGIN_Y})",
        );

        // Deep inside the (non-active) second row, offset by the bar's
        // real origin: activity-bar background.
        let probe_x = ORIGIN_X as u32 + W - 6;
        let probe_y = ORIGIN_Y as u32 + ACTIVITY_ROW_PX as u32 + 4;
        let (r, g, b, _) = surface.pixel(probe_x, probe_y);
        assert_eq!(
            (r, g, b),
            (theme.tab_bar_bg.r, theme.tab_bar_bg.g, theme.tab_bar_bg.b),
            "activity bar background should paint at rect's origin, not the window origin",
        );

        // Right-edge separator (1 pt) shifts with the origin too.
        let (sr, sg, sb, _) = surface.pixel(ORIGIN_X as u32 + W - 1, ORIGIN_Y as u32 + H / 2);
        assert_eq!(
            (sr, sg, sb),
            (theme.separator.r, theme.separator.g, theme.separator.b),
            "right-edge separator should shift with the bar's origin",
        );

        // Hit regions stay bar-relative (issue #552's audited contract) —
        // the CTM translate must not leak into the returned spans.
        assert_eq!(regions[0].y_start, 0.0);
    }
}
