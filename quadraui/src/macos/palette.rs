//! macOS rasteriser for [`crate::Palette`].
//!
//! Content painting (background, border, title/query/item rows,
//! scrollbar, create row, preview pane) moved to the shared
//! [`crate::primitives::palette::native_surface_paint::paint`] (#1076,
//! `NativeSurface` Phase 4 slice 3/8) — see that fn's module doc for
//! what's shared. Notably, this migration upgrades macOS's item rows
//! from plain-fg-only text (this module's pre-migration "Scope
//! omissions" below, kept for the historical record) to the same
//! match-position highlighting, icon rendering, query cursor, and
//! `PaletteMode::Input` suppression every other backend now shares.
//!
//! Mirrors [`crate::gtk::palette::draw_palette`]: bordered box, title
//! row, query input with cursor, separator, scrollable item list with
//! selection highlight, optional pinned create-action row, optional
//! preview pane, optional scrollbar.
//!
//! ## Scope omissions (historical, pre-#1076)
//!
//! - **Match-position highlighting** — `PaletteItem.match_positions`
//!   per-character fg highlights were deferred with the unified
//!   text-attribute pass. Items rendered in plain fg. Fixed by #1076.
//! - **Preview pane content** — preview lines rendered as plain text;
//!   syntax-highlighted spans landed with the same pass. Fixed by #1076
//!   (preview lines now honour each span's own `fg`).

use core_graphics::sys::CGContextRef;
use core_text::font::CTFont;

use crate::primitives::palette::{native_surface_paint, Palette, PaletteLayout};
use crate::theme::Theme;

/// Compute the macOS palette layout (used by both paint and host
/// hit-testing). Thin `f64`-converting wrapper over
/// [`crate::primitives::palette::native_surface_paint::layout`] (#1076)
/// — the canonical geometry formula every backend now shares instead of
/// once-per-backend copies (see that module's doc for the three-way
/// drift this closes: `query_height`, row flooring, scrollbar width).
///
/// Coordinate frame: all returned bounds (`title_bounds`,
/// `query_bounds`, `visible_items.bounds`, `create_bounds`,
/// `preview_bounds`, `scrollbar.{track,thumb}`, `hit_regions`) are in
/// **palette-local** coords (origin at 0, 0). Hosts must subtract
/// the palette's `area.x` / `area.y` from absolute click coords before
/// calling `PaletteLayout::hit_test`. The `x` / `y` params are kept in
/// the signature for symmetry with `draw_palette` but do not affect
/// output.
///
/// See `quadraui/docs/decisions/DECISIONS.md` D-007, "Palette: deferred,
/// not missed" for why `Backend::palette_layout` didn't ship until #818
/// (resolved before #1076).
pub fn mac_palette_layout(
    palette: &Palette,
    _x: f64,
    _y: f64,
    w: f64,
    h: f64,
    line_height: f64,
) -> PaletteLayout {
    native_surface_paint::layout(w as f32, h as f32, palette, line_height as f32).0
}

/// Draw a [`Palette`] modal into `(x, y, w, h)` on `ctx`.
///
/// `nerd_fonts_enabled` selects between item icons' Nerd-Font glyph and
/// ASCII fallback (issue #804), matching every other backend's
/// `draw_palette`.
///
/// # Safety
///
/// `ctx` must be a valid `CGContextRef` borrowed for the duration of
/// the call.
#[allow(clippy::too_many_arguments)]
pub unsafe fn draw_palette(
    ctx: CGContextRef,
    font: &CTFont,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    palette: &Palette,
    theme: &Theme,
    line_height: f64,
    nerd_fonts_enabled: bool,
) {
    if w < 20.0 || h < line_height * 4.0 {
        return;
    }

    let (palette_layout, rows_h) =
        native_surface_paint::layout(w as f32, h as f32, palette, line_height as f32);

    let area = crate::event::Rect::new(x as f32, y as f32, w as f32, h as f32);
    let mut surface = super::surface::CgSurface {
        ctx,
        font: Some(font),
    };
    native_surface_paint::paint(
        palette,
        area,
        &palette_layout,
        rows_h,
        line_height as f32,
        nerd_fonts_enabled,
        &mut surface,
        theme,
    );
}

#[cfg(test)]
mod tests {
    use super::super::headless::BitmapSurface;
    use super::super::text::make_font;
    use super::super::MacBackend;
    use super::*;
    use crate::event::{Rect as QRect, Viewport};
    use crate::primitives::palette::{PaletteHit, PaletteItem};
    use crate::types::{StyledText, WidgetId};
    use crate::Backend;

    const W: u32 = 400;
    const H: u32 = 240;

    fn font() -> CTFont {
        make_font("Menlo", 14.0).expect("Menlo installed")
    }

    fn sample_palette() -> Palette {
        Palette {
            id: WidgetId::new("pal"),
            title: "Commands".into(),
            query: "fo".into(),
            query_cursor: 2,
            items: vec![
                PaletteItem {
                    text: StyledText::plain("foo: open file"),
                    detail: Some(StyledText::plain("Ctrl+O")),
                    icon: None,
                    match_positions: vec![0, 1],
                    depth: 0,
                    expandable: false,
                    expanded: false,
                },
                PaletteItem {
                    text: StyledText::plain("foo: close tab"),
                    detail: None,
                    icon: None,
                    match_positions: vec![0, 1],
                    depth: 0,
                    expandable: false,
                    expanded: false,
                },
            ],
            selected_idx: 1,
            scroll_offset: 0,
            total_count: 25,
            has_focus: true,
            show_query: true,
            create_label: None,
            preview: None,
            mode: crate::primitives::palette::PaletteMode::List,
        }
    }

    fn paint_via_backend(palette: &Palette) -> BitmapSurface {
        let surface = BitmapSurface::new(W, H);
        surface.fill(0.0, 0.0, 0.0, 0.0);
        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        // `set_current_font` derives `current_line_height` from the real
        // CTFont metrics (Menlo 14pt's ascent+descent+leading), which is
        // *not* exactly `MacBackend::new()`'s 16.0 default. The
        // pixel-exact drift-regression tests below
        // (`separator_paints_at_corrected_row_not_drifted_one`,
        // `scrollbar_track_width_is_six_px_not_eight`) compute their
        // expected geometry from a literal `16.0` — pin
        // `current_line_height` back to that value so what those tests
        // predict is what `draw_palette` actually paints with.
        backend.set_current_line_height(16.0);
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            b.draw_palette(QRect::new(0.0, 0.0, W as f32, H as f32), palette);
        });
        backend.end_frame();
        surface
    }

    #[test]
    fn palette_paints_surface_bg() {
        let p = sample_palette();
        let surface = paint_via_backend(&p);
        let theme = Theme::default();
        // Probe right edge of the popup, well below all rows.
        let (r, g, b, _) = surface.pixel(W - 4, H - 4);
        assert_eq!(
            (r, g, b),
            (theme.surface_bg.r, theme.surface_bg.g, theme.surface_bg.b),
        );
    }

    #[test]
    fn selected_item_paints_selected_bg() {
        let p = sample_palette();
        let surface = paint_via_backend(&p);
        let theme = Theme::default();
        let layout = mac_palette_layout(&p, 0.0, 0.0, W as f64, H as f64, 16.0);
        let sel = layout
            .visible_items
            .iter()
            .find(|v| v.item_idx == 1)
            .expect("selected item visible");
        let px = (sel.bounds.x + sel.bounds.width - 4.0) as u32;
        let py = (sel.bounds.y + sel.bounds.height / 2.0) as u32;
        let (r, g, b, _) = surface.pixel(px, py);
        assert_eq!(
            (r, g, b),
            (
                theme.selected_bg.r,
                theme.selected_bg.g,
                theme.selected_bg.b
            ),
        );
    }

    /// Regression for issue #503: `query_cursor` is a host-supplied byte
    /// offset with no guarantee it lands on a char boundary —
    /// `&palette.query[..query_cursor]` used to panic the moment a
    /// multibyte character sat left of the cursor.
    #[test]
    fn draw_palette_with_multibyte_cursor_does_not_panic() {
        let mut p = sample_palette();
        // "café🎉" — byte 4 sits inside the 2-byte 'é' (starts at byte 3).
        p.query = "café🎉".into();
        assert!(!p.query.is_char_boundary(4));
        p.query_cursor = 4;

        // Must not panic.
        let _surface = paint_via_backend(&p);
    }

    /// Regression for #1076: pre-migration, macOS's `draw_palette`
    /// painted every item row in plain `surface_fg` regardless of
    /// `match_positions` — this module's own "Scope omissions" doc used
    /// to name it directly. `paint`'s shared `matched_runs` splitter
    /// (ported from `win::palette`) now highlights matched characters in
    /// `match_fg` on every backend, including macOS. Scanning for
    /// `match_fg`-nearest ink (rather than an exact-equality probe at a
    /// guessed glyph coordinate) mirrors `win::palette`'s own
    /// `renders_and_highlights_match_positions` test, since real
    /// CoreText glyph placement inside the row box isn't predictable
    /// ahead of a live paint pass.
    #[test]
    fn selected_item_text_highlights_match_positions() {
        let p = sample_palette();
        let surface = paint_via_backend(&p);
        let theme = Theme::default();
        let layout = mac_palette_layout(&p, 0.0, 0.0, W as f64, H as f64, 16.0);
        // Item 0 ("foo: open file", match_positions [0, 1], not selected)
        // — plain fg row, so any match_fg ink on it is exclusively from
        // the highlight, not a selection-bg blend.
        let row = layout
            .visible_items
            .iter()
            .find(|v| v.item_idx == 0)
            .expect("item 0 visible")
            .bounds;
        let dist2 = |px: (u8, u8, u8), c: crate::types::Color| {
            let dr = px.0 as i32 - c.r as i32;
            let dg = px.1 as i32 - c.g as i32;
            let db = px.2 as i32 - c.b as i32;
            dr * dr + dg * dg + db * db
        };
        let found = (row.y as u32..(row.y + row.height) as u32).any(|y| {
            (row.x as u32..(row.x + row.width) as u32).any(|x| {
                let (r, g, b, _) = surface.pixel(x, y);
                let d_match = dist2((r, g, b), theme.match_fg);
                d_match < dist2((r, g, b), theme.surface_fg)
                    && d_match < dist2((r, g, b), theme.surface_bg)
            })
        });
        assert!(
            found,
            "expected match_fg-coloured pixels on item 0's row (match_positions highlight)"
        );
    }

    /// Regression for #1076: pre-migration, macOS's `draw_palette` never
    /// rendered `PaletteItem::icon` at all — only GTK did. `paint` now
    /// paints it via `NativeSurface::surface_draw_icon_glyph` on every
    /// backend.
    #[test]
    fn item_icon_paints_left_of_the_label() {
        let mut p = sample_palette();
        p.items[0].icon = Some(crate::types::Icon {
            glyph: "\u{f15b}".into(),
            fallback: "F".into(),
            color: None,
        });
        let surface = paint_via_backend(&p);
        let theme = Theme::default();
        let layout = mac_palette_layout(&p, 0.0, 0.0, W as f64, H as f64, 16.0);
        let row = layout
            .visible_items
            .iter()
            .find(|v| v.item_idx == 0)
            .expect("item 0 visible")
            .bounds;
        // Scan a narrow band right after the "  "/"▶ " prefix — the
        // fallback icon glyph "F" paints in surface_fg there when
        // nerd_fonts_enabled is false (`draw_palette`'s default). Any
        // non-background ink in that band is the icon (item 0's label
        // starts further right, past the icon + its 6px gutter).
        let dist2 = |px: (u8, u8, u8), c: crate::types::Color| {
            let dr = px.0 as i32 - c.r as i32;
            let dg = px.1 as i32 - c.g as i32;
            let db = px.2 as i32 - c.b as i32;
            dr * dr + dg * dg + db * db
        };
        let band_x0 = row.x as u32 + 8;
        let band_x1 = (row.x + 30.0) as u32;
        let found = (row.y as u32..(row.y + row.height) as u32).any(|y| {
            (band_x0..band_x1).any(|x| {
                let (r, g, b, _) = surface.pixel(x, y);
                dist2((r, g, b), theme.surface_bg) > 400
            })
        });
        assert!(
            found,
            "expected non-background ink in the icon gutter when PaletteItem::icon is set"
        );
    }

    /// Regression for #1076's **query_height** geometry drift: pre-#1076
    /// `mac_palette_layout` reserved a bare `line_height` for the query
    /// row; GTK alone added `+ 1.0` for the separator stroke drawn
    /// immediately below it (D-007 §2). `native_surface_paint::layout`
    /// now uses GTK's `+ 1.0` formula for every backend, so the
    /// separator — and hence the first item row — sits one pixel lower
    /// than the pre-#1076 macOS geometry did. Paints through the real
    /// `MacBackend`/Core Graphics path and checks the separator's ink
    /// lands at the corrected row, not the drifted one.
    #[test]
    fn separator_paints_at_corrected_row_not_drifted_one() {
        let p = sample_palette();
        let surface = paint_via_backend(&p);
        let theme = Theme::default();
        // `MacBackend::current_line_height` defaults to 16.0 and
        // `paint_via_backend` never overrides it.
        let line_height = 16.0_f32;
        let title_h = line_height; // native_surface_paint::layout: title_h = line_height
        let drifted_sep_y = (title_h + line_height) as u32; // pre-#1076 macOS formula
        let fixed_sep_y = (title_h + line_height + 1.0) as u32; // corrected formula

        let is_border = |x: u32, y: u32| {
            let (r, g, b, _) = surface.pixel(x, y);
            (r, g, b) == (theme.border_fg.r, theme.border_fg.g, theme.border_fg.b)
        };
        let probe_x = W / 2;
        assert!(
            is_border(probe_x, fixed_sep_y),
            "expected separator ink at the corrected row {fixed_sep_y}",
        );
        assert!(
            !is_border(probe_x, drifted_sep_y),
            "separator painted at the pre-#1076 drifted row {drifted_sep_y} — \
             query_bounds.height must be line_height + 1.0, not line_height",
        );
    }

    fn many_items(n: usize) -> Vec<PaletteItem> {
        (0..n)
            .map(|i| PaletteItem {
                text: StyledText::plain(format!("item {i}")),
                detail: None,
                icon: None,
                match_positions: vec![],
                depth: 0,
                expandable: false,
                expanded: false,
            })
            .collect()
    }

    /// Regression for #1076's **row-flooring** geometry drift: pre-#1076
    /// `mac_palette_layout` fed `Palette::layout` the full popup height,
    /// so its per-row clamp (`height.min(remaining)`) could hand back a
    /// clipped, partial-height last row whenever the available item-list
    /// height wasn't an exact multiple of `line_height`. GTK's pre-#1076
    /// formula floored the row count and fed a reduced `viewport_height`
    /// instead; `native_surface_paint::layout` now does that for every
    /// backend. `H` (240) and `line_height` (16) are chosen so the
    /// available item area (203px after title/query/bottom-inset) is
    /// *not* an exact multiple of 16 — the exact condition that produced
    /// a partial last row pre-#1076.
    #[test]
    fn item_list_never_shows_a_partial_last_row() {
        let mut p = sample_palette();
        p.items = many_items(40);
        p.selected_idx = 0;
        let line_height = 16.0_f32;
        let layout = mac_palette_layout(&p, 0.0, 0.0, W as f64, H as f64, line_height as f64);
        let last = layout
            .visible_items
            .last()
            .expect("some items should be visible");
        assert_eq!(
            last.bounds.height, line_height,
            "last visible row must be full height — a partial row means the \
             pre-#1076 unfloored-viewport drift regressed",
        );

        // Driver-tier: paint through the real backend and confirm no ink
        // bleeds into the row immediately below the floored boundary —
        // a partial 13th row would have painted glyph ink there.
        let surface = paint_via_backend(&p);
        let theme = Theme::default();
        let boundary_y = (last.bounds.y + last.bounds.height + 1.0) as u32;
        let probe_x = (last.bounds.x + 4.0) as u32;
        let (r, g, b, _) = surface.pixel(probe_x, boundary_y);
        assert_eq!(
            (r, g, b),
            (theme.surface_bg.r, theme.surface_bg.g, theme.surface_bg.b),
            "expected plain surface_bg just below the floored last row, not a \
             partially-clipped row's content",
        );
    }

    /// Regression for #1076's **scrollbar width** geometry drift:
    /// pre-#1076 macOS's own `mac_palette_layout` used `(8.0, 8.0)` for
    /// `(scrollbar_width, min_thumb_len)`; GTK and Windows had already
    /// agreed on `(6.0, 8.0)`. `native_surface_paint` adopts the
    /// GTK/Windows majority value for every backend, narrowing macOS's
    /// scrollbar track from 8px to 6px.
    #[test]
    fn scrollbar_track_width_is_six_px_not_eight() {
        let mut p = sample_palette();
        p.items = many_items(40);
        let line_height = 16.0_f64;
        let layout = mac_palette_layout(&p, 0.0, 0.0, W as f64, H as f64, line_height);
        let sb = layout
            .scrollbar
            .as_ref()
            .expect("scrollbar present when items overflow the viewport");
        assert_eq!(
            sb.track.width, 6.0,
            "scrollbar track width must be 6.0px (GTK/Windows pre-#1076 value), \
             not macOS's pre-#1076 8.0px",
        );

        // Driver-tier: paint through the real backend and find where the
        // track's ink visually begins by scanning leftward from the
        // popup's right edge, comparing each column against a baseline
        // sampled well inside the item content area (away from either a
        // 6px or 8px track). The boundary must land at `sb.track.x`
        // (item_list_width - 6), not 2px further left as an 8px track
        // would place it.
        let surface = paint_via_backend(&p);
        let track_y = (sb.track.y + sb.track.height / 2.0) as u32;
        let dist2 = |a: (u8, u8, u8), b: (u8, u8, u8)| {
            let dr = a.0 as i32 - b.0 as i32;
            let dg = a.1 as i32 - b.1 as i32;
            let db = a.2 as i32 - b.2 as i32;
            dr * dr + dg * dg + db * db
        };
        let baseline_x = sb.track.x as u32 - 15;
        let (br, bg, bb, _) = surface.pixel(baseline_x, track_y);
        let baseline = (br, bg, bb);
        let boundary_x = ((sb.track.x as u32 - 12)..=(sb.track.x as u32 + 1))
            .find(|&x| {
                let (r, g, b, _) = surface.pixel(x, track_y);
                dist2((r, g, b), baseline) > 100
            })
            .expect("expected the track's ink to start somewhere in the scanned range");
        assert!(
            (boundary_x as i32 - sb.track.x as i32).abs() <= 1,
            "expected track ink to start at x={} (item_list_width - 6), found it \
             starting at x={boundary_x} instead — scrollbar track width has drifted \
             from the shared 6.0px value",
            sb.track.x,
        );
    }

    #[test]
    fn hit_test_resolves_query_row() {
        let p = sample_palette();
        let layout = mac_palette_layout(&p, 0.0, 0.0, W as f64, H as f64, 16.0);
        let qb = layout.query_bounds.expect("query bounds present");
        let hit = layout.hit_test(qb.x + 20.0, qb.y + qb.height * 0.5);
        assert_eq!(hit, PaletteHit::Query);
    }

    #[test]
    fn layout_returns_local_coords_when_area_offset() {
        // Cross-backend contract: all bounds returned by
        // mac_palette_layout are in palette-local coords (origin
        // 0, 0), regardless of where the rasteriser paints, matching
        // `tui_palette_layout` and `gtk_palette_layout`. Hosts
        // subtract area.x/area.y from absolute click coords before
        // hit_test.
        //
        // Regression for #190: prior to the fix, mac_palette_layout
        // shifted hit_regions to absolute coords. Latent today (no
        // `Backend::palette_layout` trait method exposes the layout
        // to consumers), but ready to bite the moment one is added —
        // same shape as #44's tree/form click drift.
        let p = sample_palette();
        // Area offset by (40, 80) — palette modals are typically
        // centred / inset within a window.
        let area_x: f64 = 40.0;
        let area_y: f64 = 80.0;
        let layout = mac_palette_layout(&p, area_x, area_y, W as f64, H as f64, 16.0);
        // Locality: title_bounds.y must be 0, not area_y.
        let tb = layout.title_bounds.expect("title present");
        assert_eq!(
            tb.y, 0.0,
            "title_bounds.y must be local (0.0), got {}",
            tb.y,
        );
        let qb = layout.query_bounds.expect("query bounds present");
        assert!(
            qb.y < area_y as f32,
            "query_bounds.y must be local (< area_y), got {} vs area_y={}",
            qb.y,
            area_y,
        );
        // Round-trip: simulate a click at the absolute centre of the
        // query row, localise the way AppLogic does, and assert it
        // still hits Query. Pre-fix this returned the wrong region.
        let abs_y = area_y as f32 + qb.y + qb.height * 0.5;
        let abs_x = area_x as f32 + qb.x + 20.0;
        let local_x = abs_x - area_x as f32;
        let local_y = abs_y - area_y as f32;
        assert_eq!(
            layout.hit_test(local_x, local_y),
            PaletteHit::Query,
            "query click → wrong hit (coord-frame drift)",
        );
        // Round-trip each visible item the same way.
        for vi in &layout.visible_items {
            let abs_x = area_x as f32 + vi.bounds.x + vi.bounds.width * 0.5;
            let abs_y = area_y as f32 + vi.bounds.y + vi.bounds.height * 0.5;
            let local_x = abs_x - area_x as f32;
            let local_y = abs_y - area_y as f32;
            assert_eq!(
                layout.hit_test(local_x, local_y),
                PaletteHit::Item(vi.item_idx),
                "item {} click → wrong hit (coord-frame drift)",
                vi.item_idx,
            );
        }
    }
}
