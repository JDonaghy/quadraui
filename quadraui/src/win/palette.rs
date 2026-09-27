//! Direct2D / DirectWrite rasteriser for [`crate::Palette`] (issue #28).
//!
//! Content painting (background, border, title/query/item rows,
//! scrollbar, create row, preview pane) moved to the shared
//! [`crate::primitives::palette::native_surface_paint::paint`] (#1076,
//! `NativeSurface` Phase 4 slice 3/8) — see that fn's module doc for
//! what's shared, including the three-way geometry drift it fixes and
//! the per-backend feature gaps it closes. Windows already had
//! match-position highlighting (its own `matched_runs`/
//! `draw_matched_text` run-splitter, ported into the shared `paint` for
//! every backend to use); it gains icon rendering, a query cursor, and
//! `PaletteMode::Input` handling for the first time (it already had
//! this last one; kept).
//!
//! Only compiled on `target_os = "windows"` — see `super::mod`'s
//! `#[cfg(target_os = "windows")] mod palette;` and `backend.rs`'s
//! module docs.

use windows::Win32::Graphics::Direct2D::ID2D1RenderTarget;

use super::text::DWrite;
use crate::event::Rect;
use crate::primitives::palette::{native_surface_paint, Palette, PaletteLayout};

/// Compute a [`Palette`]'s layout at `(rect.x, rect.y)` without
/// painting. Thin wrapper over
/// [`crate::primitives::palette::native_surface_paint::layout`] (#1076)
/// — the canonical geometry formula every backend now shares. See that
/// fn's doc for the three-way drift this closes (`query_height`, row
/// flooring, scrollbar width). `Palette::layout` keeps the selected row
/// visible internally (see #711) — no backend-side scroll clamp needed
/// here.
pub fn win_palette_layout(rect: Rect, palette: &Palette, line_height: f32) -> PaletteLayout {
    native_surface_paint::layout(rect.width, rect.height, palette, line_height).0
}

/// Draw a [`Palette`] modal into `rect`. Backend-internal layout (see
/// [`win_palette_layout`]) — `Palette` has no layout-passthrough trait
/// method (unlike `ContextMenu`/`Dialog`), matching
/// [`crate::Backend::draw_palette`]'s signature.
///
/// `nerd_fonts_enabled` selects between item icons' Nerd-Font glyph and
/// ASCII fallback (issue #804), matching every other backend's
/// `draw_palette`. Colours come from `Theme::default()` rather than a
/// live `WinBackend` theme field — same convention `win::status_bar`
/// and every other pre-`NativeSurface` Win-GUI rasteriser use.
pub fn draw_palette(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    rect: Rect,
    palette: &Palette,
    line_height: f32,
    nerd_fonts_enabled: bool,
) {
    if rect.width < 20.0 || rect.height < line_height * 4.0 {
        return;
    }

    let theme = crate::theme::Theme::default();
    let (palette_layout, rows_h) =
        native_surface_paint::layout(rect.width, rect.height, palette, line_height);

    let mut surface = super::surface::D2dSurface {
        target,
        dwrite: Some(dwrite),
    };
    native_surface_paint::paint(
        palette,
        rect,
        &palette_layout,
        rows_h,
        line_height,
        nerd_fonts_enabled,
        &mut surface,
        &theme,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::palette::PaletteMode;
    use crate::theme::Theme;
    use crate::types::{Color, StyledText, WidgetId};
    use crate::win::testing::HeadlessSurface;

    fn item(text: &str, match_positions: Vec<usize>) -> crate::primitives::palette::PaletteItem {
        crate::primitives::palette::PaletteItem {
            text: StyledText::plain(text),
            detail: None,
            icon: None,
            match_positions,
            depth: 0,
            expandable: false,
            expanded: false,
        }
    }

    fn palette() -> Palette {
        Palette {
            id: WidgetId::new("pal"),
            title: "Commands".into(),
            query: "op".into(),
            query_cursor: 2,
            items: vec![item("open file", vec![0, 1]), item("close", vec![])],
            selected_idx: 0,
            scroll_offset: 0,
            total_count: 0,
            has_focus: true,
            show_query: true,
            create_label: None,
            preview: None,
            mode: PaletteMode::List,
        }
    }

    #[test]
    fn renders_and_highlights_match_positions() {
        let surface = HeadlessSurface::new(300, 200).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let p = palette();
        let rect = Rect::new(0.0, 0.0, 300.0, 200.0);
        let line_height = 18.0;

        surface
            .paint(|target| {
                draw_palette(target, &dwrite, rect, &p, line_height, false);
            })
            .expect("paint palette");

        let layout = win_palette_layout(rect, &p, line_height);
        let first_item = layout.visible_items[0].bounds;

        let theme = Theme::default();
        // Scan the whole first-item row box for match-highlighted ink —
        // matched runs ('o', 'p' at byte offsets 0, 1 of "open file")
        // paint in `match_fg`, warm and far from every other colour on
        // that row. Two deliberate weakenings versus an exact-equality
        // probe at a guessed coordinate: the scan covers the row's full
        // height (real DirectWrite glyph placement inside the row box
        // isn't predictable ahead of a live Windows font pass), and a
        // pixel counts when it is *nearer* `match_fg` than the row's
        // other two colours rather than exactly equal to it, since
        // glyph edges are antialiased blends over the selection fill.
        let dist2 = |px: Color, c: Color| {
            let dr = px.r as i32 - c.r as i32;
            let dg = px.g as i32 - c.g as i32;
            let db = px.b as i32 - c.b as i32;
            dr * dr + dg * dg + db * db
        };
        let found = (first_item.y as u32..(first_item.y + first_item.height) as u32).any(|y| {
            (first_item.x as u32..(first_item.x + first_item.width) as u32).any(|x| {
                let px = surface.pixel_at(x, y);
                let d_match = dist2(px, theme.match_fg);
                d_match < dist2(px, theme.surface_fg) && d_match < dist2(px, theme.selected_bg)
            })
        });
        assert!(
            found,
            "expected to find match_fg-coloured pixels on the matched item's row"
        );
    }

    /// Regression for #1076: pre-migration, `win::palette::draw_palette`
    /// never painted a query cursor at all (this module's own doc used
    /// to name it directly, contrasting with GTK/macOS's block cursor).
    /// The shared `paint` now fills a cursor block in `query_fg` on
    /// every backend, including Windows.
    #[test]
    fn query_row_paints_a_cursor_block() {
        let surface = HeadlessSurface::new(300, 200).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let p = palette();
        let rect = Rect::new(0.0, 0.0, 300.0, 200.0);
        let line_height = 18.0;

        surface
            .paint(|target| {
                draw_palette(target, &dwrite, rect, &p, line_height, false);
            })
            .expect("paint palette");

        let layout = win_palette_layout(rect, &p, line_height);
        let qb = layout.query_bounds.expect("query bounds present");
        let theme = Theme::default();
        // A cursor is a *solid fill* spanning the row's full height —
        // unlike glyph ink (which never covers a whole text-row column
        // solidly, since real fonts leave leading above ascenders and
        // below descenders), so scan for the column with the highest
        // fraction of query_fg-near pixels and require it to be nearly
        // fully covered. Plain "> op" text alone (the pre-#1076
        // behaviour) never produces a column this solid.
        let dist2 = |px: Color, c: Color| {
            let dr = px.r as i32 - c.r as i32;
            let dg = px.g as i32 - c.g as i32;
            let db = px.b as i32 - c.b as i32;
            dr * dr + dg * dg + db * db
        };
        let rows: Vec<u32> = (qb.y as u32..(qb.y + qb.height) as u32).collect();
        let best_coverage = (qb.x as u32..(qb.x + qb.width) as u32)
            .map(|x| {
                let covered = rows
                    .iter()
                    .filter(|&&y| dist2(surface.pixel_at(x, y), theme.query_fg) < 400)
                    .count();
                covered as f32 / rows.len().max(1) as f32
            })
            .fold(0.0_f32, f32::max);
        assert!(
            best_coverage > 0.9,
            "expected a full-row-height query_fg cursor block; best column coverage was {best_coverage}"
        );
    }

    /// Regression for #1076's **query_height** geometry drift: pre-#1076
    /// `win_palette_layout` reserved a bare `line_height` for the query
    /// row (and `win::draw_palette` painted no query/list separator at
    /// all); GTK alone reserved `line_height + 1.0`, the `+ 1.0` being
    /// the separator stroke that sits in the query band's **last pixel**
    /// (D-007 §2). `native_surface_paint::layout` now uses GTK's formula
    /// for every backend, so Windows gains the separator *and* its first
    /// item row moves one pixel down, from `title_h + line_height` to
    /// `title_h + line_height + 1.0`.
    ///
    /// Paints through the real Direct2D `HeadlessSurface` and pins all
    /// three rows: the reserved separator pixel carries border ink, the
    /// query text row above it does not, and the first item row below it
    /// carries the selected row's fill — i.e. the separator neither
    /// climbs into the text row nor steals (and get overpainted by) the
    /// first item row, which is exactly what it used to do.
    #[test]
    fn separator_paints_at_corrected_row_not_drifted_one() {
        let surface = HeadlessSurface::new(300, 200).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let p = palette();
        let rect = Rect::new(0.0, 0.0, 300.0, 200.0);
        let line_height = 18.0_f32;

        surface
            .paint(|target| {
                draw_palette(target, &dwrite, rect, &p, line_height, false);
            })
            .expect("paint palette");

        let theme = Theme::default();
        let layout = win_palette_layout(rect, &p, line_height);
        let qb = layout.query_bounds.expect("query bounds present");
        assert_eq!(
            qb.height,
            line_height + 1.0,
            "query band must reserve line_height + 1.0 (the +1 is the separator)",
        );

        let title_h = line_height; // native_surface_paint::layout: title_h = line_height
        let sep_y = (qb.y + qb.height - 1.0) as u32; // corrected: the reserved pixel
        let first_row_y = (qb.y + qb.height) as u32; // pre-#1076: this was sep_y
        assert_eq!(sep_y, (title_h + line_height) as u32);
        assert_eq!(first_row_y, (title_h + line_height + 1.0) as u32);
        assert_eq!(
            layout.visible_items[0].bounds.y,
            qb.y + qb.height,
            "first item row starts immediately below the query band",
        );

        // `p`'s row 0 is the selected one, so the first item row paints
        // `selected_bg` — the fill that used to swallow a separator
        // painted at `first_row_y`.
        let probe_x = 150_u32;
        let is_border = |y: u32| {
            let px = surface.pixel_at(probe_x, y);
            (px.r, px.g, px.b) == (theme.border_fg.r, theme.border_fg.g, theme.border_fg.b)
        };
        assert!(
            is_border(sep_y),
            "expected separator ink in the query band's reserved pixel, row {sep_y}",
        );
        assert!(
            !is_border(sep_y - 1),
            "separator climbed into the query *text* row {} — it belongs in the \
             single pixel query_bounds reserves for it",
            sep_y - 1,
        );
        let first_row_px = surface.pixel_at(probe_x, first_row_y);
        assert_eq!(
            (first_row_px.r, first_row_px.g, first_row_px.b),
            (
                theme.selected_bg.r,
                theme.selected_bg.g,
                theme.selected_bg.b
            ),
            "row {first_row_y} is the first item row (selected), not the separator's",
        );
    }

    /// Regression for #1076's **row-flooring** geometry drift: pre-#1076
    /// `win_palette_layout` fed `Palette::layout` the full popup height,
    /// so its per-row clamp (`height.min(remaining)`) could hand back a
    /// clipped, partial-height last row whenever the available item-list
    /// height wasn't an exact multiple of `line_height`. GTK's pre-#1076
    /// formula floored the row count and fed a reduced `viewport_height`
    /// instead; `native_surface_paint::layout` now does that for every
    /// backend. The rect height (200) and `line_height` (18) are chosen
    /// so the available item area (159px after title/query/bottom-inset)
    /// is *not* an exact multiple of 18 — the exact condition that
    /// produced a partial last row pre-#1076.
    #[test]
    fn item_list_never_shows_a_partial_last_row() {
        let surface = HeadlessSurface::new(300, 200).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let mut p = palette();
        p.items = (0..40)
            .map(|i| item(&format!("item {i}"), vec![]))
            .collect();
        let rect = Rect::new(0.0, 0.0, 300.0, 200.0);
        let line_height = 18.0_f32;

        surface
            .paint(|target| {
                draw_palette(target, &dwrite, rect, &p, line_height, false);
            })
            .expect("paint palette");

        let layout = win_palette_layout(rect, &p, line_height);
        let last = layout
            .visible_items
            .last()
            .expect("some items should be visible");
        assert_eq!(
            last.bounds.height, line_height,
            "last visible row must be full height — a partial row means the \
             pre-#1076 unfloored-viewport drift regressed",
        );

        // Driver-tier: paint through the real Direct2D backend and
        // confirm no ink bleeds into the row immediately below the
        // floored boundary — a partial 9th row would have painted glyph
        // ink there.
        let theme = Theme::default();
        let boundary_y = (last.bounds.y + last.bounds.height + 1.0) as u32;
        let probe_x = (last.bounds.x + 4.0) as u32;
        let px = surface.pixel_at(probe_x, boundary_y);
        assert_eq!(
            (px.r, px.g, px.b),
            (theme.surface_bg.r, theme.surface_bg.g, theme.surface_bg.b),
            "expected plain surface_bg just below the floored last row, not a \
             partially-clipped row's content",
        );
    }

    #[test]
    fn selected_row_paints_selection_bg() {
        let surface = HeadlessSurface::new(300, 200).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let p = palette();
        let rect = Rect::new(0.0, 0.0, 300.0, 200.0);
        let line_height = 18.0;

        surface
            .paint(|target| {
                draw_palette(target, &dwrite, rect, &p, line_height, false);
            })
            .expect("paint palette");

        let layout = win_palette_layout(rect, &p, line_height);
        let first_item = layout.visible_items[0].bounds;
        let sel = Theme::default().selected_bg;
        let px = surface.pixel_at(
            (first_item.x + first_item.width - 2.0) as u32,
            (first_item.y + 1.0) as u32,
        );
        assert_eq!((px.r, px.g, px.b), (sel.r, sel.g, sel.b));
    }
}
