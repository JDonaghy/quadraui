//! GTK rasteriser for [`crate::Palette`].
//!
//! Content painting (background, border, title/query/item rows,
//! scrollbar, create row, preview pane) moved to the shared
//! [`crate::primitives::palette::native_surface_paint::paint`] (#1076,
//! `PaintSurface` Phase 4 slice 3/8) — see that fn's module doc for
//! what's shared, including the three-way geometry drift it fixes
//! (`query_height`, row flooring, scrollbar width) and the per-backend
//! feature gaps it closes (match-position highlighting, icon rendering,
//! query cursor, `PaletteMode::Input` suppression, preview per-span
//! colour — GTK already had all of these; macOS/Windows gained
//! whichever ones they were missing).
//!
//! Modal-style fuzzy picker with a title bar, query-input row, and a
//! scrollable result list. Cairo + Pango equivalent of
//! `quadraui::tui::draw_palette` with a square stroked border (vs the
//! TUI version's `╭─╮ ╰─╯` glyphs).

use gtk4::cairo::Context;
use gtk4::pango;

use crate::primitives::palette::{native_surface_paint, Palette, PaletteLayout};
use crate::theme::Theme;

/// Compute the GTK [`Palette`] layout — the shared geometry
/// [`draw_palette`] paints from and `Backend::palette_layout` (#818)
/// exposes for hit-testing. Thin `f64`-converting wrapper over
/// [`crate::primitives::palette::native_surface_paint::layout`] (#1076)
/// — the canonical geometry formula every backend now shares instead of
/// once-per-backend copies.
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
    let (layout, rows_h) =
        native_surface_paint::layout(w as f32, h as f32, palette, line_height as f32);
    (layout, rows_h as f64)
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

    // `Palette::layout` keeps the selected item visible internally (see
    // #711) — no backend-side scroll clamp needed here.
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
    );

    layout.set_attributes(None);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::palette::PaletteMode;
    use crate::types::WidgetId;
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

    /// Regression for issue #503: `query_cursor` is a host-supplied byte
    /// offset (per the field doc above) with no guarantee it lands on a
    /// char boundary — `&palette.query[..query_cursor]` used to panic
    /// the moment a multibyte character sat left of the cursor.
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
