//! GTK rasteriser for [`crate::Palette`].
//!
//! Content painting (background, border, title/query/item rows,
//! scrollbar, create row, preview pane) uses the shared
//! [`crate::primitives::palette::native_surface_paint::paint`]
//! function — see that fn's module doc for what's shared, including
//! the three-way geometry drift it fixes (`query_height`, row flooring,
//! scrollbar width) and per-backend feature gaps (match-position
//! highlighting, icon rendering, query cursor, `PaletteMode::Input`
//! suppression, preview per-span colour).
//!
//! Modal-style fuzzy picker with a title bar, query-input row, and a
//! scrollable result list. Cairo + Pango equivalent of
//! `quadraui::tui::draw_palette` with a square stroked border (vs the
//! TUI version's `╭─╮ ╰─╯` glyphs).

use gtk4::cairo::Context;
use gtk4::pango;

use crate::primitives::palette::{native_surface_paint, Palette, PaletteHit, PaletteLayout};
use crate::theme::Theme;

/// Click/drag hit-box width for the palette's scrollbar thumb/track —
/// [`crate::style::Style::scrollbar_width`]'s own default. Wider than
/// the narrower visual bar `native_surface_paint::paint`'s own `SB_W`
/// (`6.0`) constant actually draws — see that constant's own doc for
/// why the *painted* width stays put — so a mouse can reliably land on
/// it, VS Code's own overlay scrollbars extend their click target past
/// the painted thumb the same way.
const PALETTE_SCROLLBAR_HIT_WIDTH: f32 = 10.0;

/// Compute the GTK [`Palette`] layout — the shared geometry
/// [`draw_palette`] paints from and `Backend::palette_layout`
/// exposes for hit-testing. Thin `f64`-converting wrapper over
/// [`crate::primitives::palette::native_surface_paint::layout`]
/// — the canonical geometry formula every backend now shares instead of
/// once-per-backend copies — plus [`widen_scrollbar_hit_area`], which
/// widens only the scrollbar's *hit* regions, not its painted geometry.
///
/// Coordinate frame: **LOCAL** — `(0, 0)` is the popup's own top-left
/// corner, matching [`Palette::layout`]'s native contract (same
/// convention `mac_palette_layout` / `win_palette_layout` already use).
/// Returns the layout alongside `rows_h` (the item area's full row
/// capacity in pixels, already floored) — `draw_palette` needs it for
/// the scrollbar track / preview-pane / create-row positions that sit
/// below the last item, which aren't otherwise exposed as a single
/// [`PaletteLayout`] field.
pub fn gtk_palette_layout(
    w: f64,
    h: f64,
    palette: &Palette,
    line_height: f64,
) -> (PaletteLayout, f64) {
    let (mut layout, rows_h) =
        native_surface_paint::layout(w as f32, h as f32, palette, line_height as f32);
    widen_scrollbar_hit_area(&mut layout);
    (layout, rows_h as f64)
}

/// Widen `layout`'s scrollbar thumb/track **hit regions** to
/// [`PALETTE_SCROLLBAR_HIT_WIDTH`], anchored to the same trailing edge
/// as the painted bar, without touching `layout.scrollbar` (the
/// geometry [`native_surface_paint::paint`] actually draws from) — a
/// 6px-wide click target (`native_surface_paint`'s own `SB_W`) is hard
/// to land with a mouse.
///
/// Replaces the original (narrow) `ScrollbarThumb`/`ScrollbarTrack`
/// entries `Palette::layout` pushed and reinserts the widened pair at
/// the front of `hit_regions`, ahead of every item row, so a click in
/// the overlap between the widened band and an item underneath
/// resolves to the scrollbar — the same precedence VS Code's own
/// overlay scrollbar hit-testing gives a click near, but not exactly
/// on, the thumb.
fn widen_scrollbar_hit_area(layout: &mut PaletteLayout) {
    let Some(sb) = layout.scrollbar else {
        return;
    };
    let extra = (PALETTE_SCROLLBAR_HIT_WIDTH - sb.track.width).max(0.0);
    if extra <= 0.0 {
        return;
    }
    layout
        .hit_regions
        .retain(|(_, hit)| !matches!(hit, PaletteHit::ScrollbarThumb | PaletteHit::ScrollbarTrack));
    let widen = |r: crate::event::Rect| {
        crate::event::Rect::new(r.x - extra, r.y, r.width + extra, r.height)
    };
    layout
        .hit_regions
        .insert(0, (widen(sb.track), PaletteHit::ScrollbarTrack));
    layout
        .hit_regions
        .insert(0, (widen(sb.thumb), PaletteHit::ScrollbarThumb));
}

/// Draw a [`Palette`] modal into `(x, y, w, h)` on `cr`.
///
/// `nerd_fonts_enabled` selects between item icons' Nerd-Font glyph
/// and ASCII fallback. Caller is responsible for sizing / centring
/// the popup; this function paints a square stroked border at the
/// supplied bounds.
#[allow(clippy::too_many_arguments)]
pub fn draw_palette(
    cr: &Context,
    layout: &pango::Layout,
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

    layout.set_attributes(None);

    // `Palette::layout` keeps the selected item visible internally —
    // no backend-side scroll clamp needed here.
    let (palette_layout, rows_h) = gtk_palette_layout(w, h, palette, line_height);

    let area = crate::event::Rect::new(x as f32, y as f32, w as f32, h as f32);
    let mut surface = super::surface::CairoSurface {
        cr,
        layout: Some(layout),
        translucent_fill: true,
    };
    native_surface_paint::paint(
        palette,
        area,
        &palette_layout,
        rows_h as f32,
        line_height as f32,
        nerd_fonts_enabled,
        &mut surface,
        theme,
        &crate::style::Style::default(),
    );

    layout.set_attributes(None);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::palette::{PaletteItem, PaletteMode};
    use crate::types::{StyledText, WidgetId};
    use pangocairo::cairo::{Context, Format, ImageSurface};

    fn sample_palette(query: &str, query_cursor: usize) -> Palette {
        Palette {
            id: WidgetId::new("palette"),
            title: "Commands".into(),
            query: query.into(),
            query_cursor,
            items: Vec::new(),
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

    fn item(label: &str) -> PaletteItem {
        PaletteItem {
            text: StyledText::plain(label.to_string()),
            detail: None,
            icon: None,
            match_positions: vec![],
            depth: 0,
            expandable: false,
            expanded: false,
        }
    }

    /// The scrollbar's **hit** area is wider than its painted track.
    /// `native_surface_paint`'s own `SB_W` (`6.0`) stays the painted
    /// width (no visual change), but a click up to
    /// `PALETTE_SCROLLBAR_HIT_WIDTH` in from the popup's trailing edge —
    /// including over the item rows the narrow visual bar leaves
    /// otherwise-clickable — must resolve to the scrollbar.
    #[test]
    fn scrollbar_hit_area_is_wider_than_the_painted_track() {
        let mut palette = sample_palette("", 0);
        palette.items = (0..40).map(|i| item(&format!("item {i}"))).collect();
        palette.total_count = 40;

        let (layout, _rows_h) = gtk_palette_layout(100.0, 200.0, &palette, 14.0);
        let sb = layout
            .scrollbar
            .expect("40 items in a 200px-tall popup must need a scrollbar");

        // Painted geometry is untouched — still the narrow SB_W convention.
        assert_eq!(sb.track.width, 6.0, "visual track width must not change");

        // Just inside the widened hit band, but left of the painted
        // 6px track — over what would otherwise be an item row.
        let probe_x = sb.track.x + sb.track.width - PALETTE_SCROLLBAR_HIT_WIDTH + 1.0;
        let probe_y = sb.track.y + 2.0;
        assert!(
            matches!(
                layout.hit_test(probe_x, probe_y),
                PaletteHit::ScrollbarThumb | PaletteHit::ScrollbarTrack
            ),
            "expected a click at ({probe_x}, {probe_y}) — inside the widened hit \
             band but left of the painted 6px track — to resolve to the scrollbar"
        );

        // Well clear of the widened band (deep into the item column)
        // must still resolve to an item, not the scrollbar.
        let item_probe_x = sb.track.x - PALETTE_SCROLLBAR_HIT_WIDTH - 10.0;
        assert!(
            matches!(layout.hit_test(item_probe_x, probe_y), PaletteHit::Item(_)),
            "a click well clear of the widened scrollbar band must still hit an item row"
        );
    }

    /// A palette whose items all fit has no scrollbar at all — widening
    /// a non-existent hit area must be a no-op, not a panic.
    #[test]
    fn no_scrollbar_hit_widening_when_items_fit() {
        let mut palette = sample_palette("", 0);
        palette.items = vec![item("only one")];

        let (layout, _rows_h) = gtk_palette_layout(100.0, 200.0, &palette, 14.0);
        assert!(layout.scrollbar.is_none());
        assert!(!layout.hit_regions.iter().any(|(_, hit)| matches!(
            hit,
            PaletteHit::ScrollbarThumb | PaletteHit::ScrollbarTrack
        )));
    }

    /// `query_cursor` is a host-supplied byte offset (per the field doc
    /// above) with no guarantee it lands on a char boundary — the slice
    /// `&palette.query[..query_cursor]` requires a valid UTF-8 boundary.
    #[test]
    fn draw_palette_with_multibyte_cursor_does_not_panic() {
        let surface = ImageSurface::create(Format::ARgb32, 300, 200).expect("create ImageSurface");
        let cr = Context::new(&surface).expect("Context::new");
        let pango_layout = pangocairo::functions::create_layout(&cr);

        // "café🎉" — byte 4 sits inside the 2-byte 'é' (starts at byte 3).
        let query = "café🎉";
        assert!(!query.is_char_boundary(4));
        let palette = sample_palette(query, 4);
        let theme = Theme::default();

        // Must not panic.
        draw_palette(
            &cr,
            &pango_layout,
            0.0,
            0.0,
            300.0,
            200.0,
            &palette,
            &theme,
            18.0,
            false,
        );
    }
}
