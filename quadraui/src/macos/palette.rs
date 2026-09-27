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
