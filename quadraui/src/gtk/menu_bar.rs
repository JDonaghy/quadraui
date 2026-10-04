//! GTK rasteriser for [`crate::MenuBar`].
//!
//! Layout (`gtk_menu_bar_layout`) stays here — it needs Pango's own
//! text measurement to size each item. Content painting (background,
//! active/disabled colouring, label text, Alt-key underline) moved to
//! the shared [`crate::primitives::menu_bar::native_surface_paint::paint`]
//! (#1081, `PaintSurface` Phase 4 slice 5/8) — see that fn's module
//! doc for the underline-mechanism drift it resolved (GTK's Pango
//! per-character `AttrList` underline vs. Windows' manual rectangle vs.
//! macOS having no underline at all).

use gtk4::cairo::Context;
use gtk4::pango;

use crate::event::Rect;
use crate::primitives::menu_bar::{
    native_surface_paint, MenuBar, MenuBarItemMeasure, MenuBarLayout,
};
use crate::theme::Theme;

/// Compute the GTK pixel-unit layout for a [`MenuBar`] without painting.
/// Consumer click routers call this to resolve mouse events against
/// the same layout the rasteriser used to paint.
pub fn gtk_menu_bar_layout(
    pango_layout: &pango::Layout,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    bar: &MenuBar,
) -> MenuBarLayout {
    let bounds = Rect::new(x as f32, y as f32, width as f32, height as f32);
    bar.layout(bounds, |i| {
        let text = display_text(&bar.items[i].label);
        pango_layout.set_text(&text);
        pango_layout.set_attributes(None);
        let w = pango_layout.pixel_size().0.max(0) as f32 + 16.0; // 8px padding each side
        MenuBarItemMeasure::new(w)
    })
}

/// Draw a [`MenuBar`] into `(x, y, width, height)` on `cr`.
/// Returns the layout for host click dispatch.
///
/// The bar occupies the full `height` — background fill, active-item
/// highlight, and clip all span `height`, and labels are vertically
/// centred. Pass `line_height` for a tight single-row bar, or a
/// larger value (e.g. the titlebar DA height) when the bar shares a
/// row with taller widgets like a command centre.
///
/// Menu item labels are chrome, not editor content — per #624, the
/// caller is responsible for setting `pango_layout`'s font description
/// to the desired UI font (`GtkBackend::ui_font`) before calling and
/// restoring whatever it was afterward (`GtkBackend::draw_menu_bar`
/// does this). This rasteriser has no separate "editor font" concept of
/// its own; it measures and paints with whatever font is current on
/// `pango_layout`.
#[allow(clippy::too_many_arguments)]
pub fn draw_menu_bar(
    cr: &Context,
    pango_layout: &pango::Layout,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    bar: &MenuBar,
    theme: &Theme,
) -> MenuBarLayout {
    pango_layout.set_attributes(None);
    pango_layout.set_width(-1);
    pango_layout.set_ellipsize(pango::EllipsizeMode::None);

    cr.save().ok();
    cr.rectangle(x, y, width, height);
    cr.clip();

    let layout = gtk_menu_bar_layout(pango_layout, x, y, width, height, bar);

    let mut surface = super::surface::CairoSurface {
        cr,
        layout: Some(pango_layout),
        translucent_fill: true,
    };
    native_surface_paint::paint(bar, &layout, &mut surface, theme);
    pango_layout.set_attributes(None);

    cr.restore().ok();

    layout
}

/// Strip `&` markers from a label for display.
fn display_text(label: &str) -> String {
    label.chars().filter(|&c| c != '&').collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::menu_bar::{MenuBarHit, MenuBarItem};
    use crate::types::{Color, WidgetId};
    use pangocairo::cairo::{Context, Format, ImageSurface};

    const W: i32 = 300;
    const H: i32 = 40;

    fn make_bar() -> MenuBar {
        MenuBar {
            id: WidgetId::new("bar"),
            items: vec![
                MenuBarItem {
                    id: WidgetId::new("bar:file"),
                    label: "&File".into(),
                    disabled: false,
                    submenu: None,
                },
                MenuBarItem {
                    id: WidgetId::new("bar:edit"),
                    label: "&Edit".into(),
                    disabled: false,
                    submenu: None,
                },
            ],
            open_item: None,
            focused_item: None,
        }
    }

    /// White bar background so only glyphs (not the bar's own background
    /// fill, which otherwise paints every cell from `origin_x` onward and
    /// would swamp the pixel scan below) show up as non-white pixels.
    fn test_theme() -> Theme {
        Theme {
            tab_bar_bg: Color::rgb(255, 255, 255),
            tab_inactive_fg: Color::rgb(0, 0, 0),
            tab_active_fg: Color::rgb(0, 0, 0),
            tab_active_bg: Color::rgb(255, 255, 255),
            ..Theme::default()
        }
    }

    fn pixel(data: &[u8], stride: usize, x: i32, y: i32) -> (u8, u8, u8) {
        let off = y as usize * stride + x as usize * 4;
        (data[off + 2], data[off + 1], data[off])
    }

    fn is_painted(data: &[u8], stride: usize, x: i32, y: i32) -> bool {
        if x < 0 || y < 0 || x >= W || y >= H {
            return false;
        }
        let (r, g, b) = pixel(data, stride, x, y);
        !(r == 255 && g == 255 && b == 255)
    }

    /// Leftmost painted column in row `y`, scanning `[x_from, W)`.
    fn leftmost_painted_in_row(data: &[u8], stride: usize, y: i32, x_from: i32) -> Option<i32> {
        (x_from..W).find(|&x| is_painted(data, stride, x, y))
    }

    /// Paint→click round trip at `(origin_x, origin_y)`: paints the bar,
    /// then for each item, confirms the painted label's leftmost pixel
    /// lands close to `vi.bounds.x` (plus the fixed 8px padding
    /// `gtk_menu_bar_layout`'s measure closure reserves) — not shifted an
    /// extra `origin_x` to the right — and that `hit_test` at the
    /// item's own painted position still resolves to that item.
    ///
    /// This is the LESSONS.md "layout helpers must return coords in the
    /// same frame across backends" regression shape (quadraui#494):
    /// `vi.bounds.x` is already absolute (the bar's `layout()` seeds its
    /// cursor at `bounds.x = origin_x`), so `draw_menu_bar` must paint at
    /// `vi.bounds.x` directly, not `origin_x + vi.bounds.x` — the bug this
    /// test guards against painted glyphs `origin_x` cells to the right of
    /// where `hit_test` expects them, invisible at `origin_x == 0`.
    fn paint_and_click_round_trip_at(origin_x: f64, origin_y: f64) {
        let mut surface = ImageSurface::create(Format::ARgb32, W, H).expect("create ImageSurface");
        let bar = make_bar();
        let layout = {
            let cr = Context::new(&surface).expect("Context::new");
            cr.set_source_rgb(1.0, 1.0, 1.0);
            cr.paint().ok();
            let pango_layout = pangocairo::functions::create_layout(&cr);
            draw_menu_bar(
                &cr,
                &pango_layout,
                origin_x,
                origin_y,
                (W as f64) - origin_x,
                20.0,
                &bar,
                &test_theme(),
            )
        };
        surface.flush();
        let stride = surface.stride() as usize;
        let data = surface.data().expect("surface data");

        assert_eq!(layout.visible_items.len(), 2, "both items should fit");
        for vi in &layout.visible_items {
            let row_y = (origin_y + 10.0) as i32; // inside the 20px-tall bar
            let scan_from = vi.bounds.x.floor() as i32;
            let painted_x = leftmost_painted_in_row(&data, stride, row_y, scan_from)
                .unwrap_or_else(|| {
                    panic!(
                        "item {} ({:?}) should paint a visible glyph on row {row_y} at or after x={scan_from}",
                        vi.item_idx, vi.id,
                    )
                });
            // 8px left padding (see `gtk_menu_bar_layout`'s measure
            // closure: `pixel_size().0 + 16.0`, split evenly). Generous
            // tolerance for antialiasing/font metrics — the point is
            // catching a whole extra `origin_x` of drift (7px in the
            // non-zero-origin test below), not pixel-perfect kerning.
            let expected = vi.bounds.x + 8.0;
            assert!(
                (painted_x as f32 - expected).abs() < 4.0,
                "item {} painted glyph at x={painted_x}, expected near {expected} \
                 (vi.bounds.x={}, origin_x={origin_x}) — painting must not add \
                 origin_x on top of vi.bounds.x, which is already absolute",
                vi.item_idx,
                vi.bounds.x,
            );

            // Round-trip: a click at the item's own (absolute) bounds
            // centre must resolve back to that item via `hit_test`.
            let cx = vi.bounds.x + vi.bounds.width / 2.0;
            let cy = vi.bounds.y + vi.bounds.height / 2.0;
            assert_eq!(
                layout.hit_test(cx, cy),
                MenuBarHit::Item(vi.item_idx),
                "item {} centre should hit-test back to itself",
                vi.item_idx,
            );
        }
    }

    #[test]
    fn paint_and_click_round_trip() {
        paint_and_click_round_trip_at(0.0, 0.0);
    }

    /// quadraui#625/#1081: the Alt-key underline logic itself
    /// (`alt_char_index`/`display_text`) now lives in and is tested by
    /// `crate::primitives::menu_bar::native_surface_paint::tests` — see
    /// that module for the "no implicit fallback" and multi-byte-label
    /// coverage this file used to carry directly against
    /// `alt_char_byte_range`. This test instead pins that `draw_menu_bar`
    /// actually *paints* the underline it computes: a real regression
    /// guard for #1081, since pre-migration `gtk::menu_bar` was the only
    /// one of the three backends where this worked at all (macOS had no
    /// underline; this is what the shared path now gives every backend).
    #[test]
    fn draw_menu_bar_paints_alt_underline_beneath_activation_char() {
        let mut surface = ImageSurface::create(Format::ARgb32, W, H).expect("create ImageSurface");
        let bar = make_bar();
        let (layout, pango_layout) = {
            let cr = Context::new(&surface).expect("Context::new");
            cr.set_source_rgb(1.0, 1.0, 1.0);
            cr.paint().ok();
            let pango_layout = pangocairo::functions::create_layout(&cr);
            let layout = draw_menu_bar(
                &cr,
                &pango_layout,
                0.0,
                0.0,
                W as f64,
                20.0,
                &bar,
                &test_theme(),
            );
            (layout, pango_layout)
        };
        surface.flush();
        let stride = surface.stride() as usize;
        let data = surface.data().expect("surface data");

        // Reproduce `native_surface_paint::paint`'s own measurements
        // (same `pango_layout`, same font — nothing else touched it
        // since `draw_menu_bar` returned) rather than guessing pixel
        // rows from an assumed line height, which is font/host
        // dependent and not this test's concern.
        pango_layout.set_attributes(None);
        pango_layout.set_text("File");
        let (_, text_h) = pango_layout.pixel_size();
        pango_layout.set_text("F");
        let (char_w, _) = pango_layout.pixel_size();

        let file_item = &layout.visible_items[0];
        let text_y = (20.0 - text_h as f64) / 2.0;
        // `UNDERLINE_HEIGHT` in `native_surface_paint` — a 2px-tall band
        // at the very bottom of the measured text box. `draw_menu_bar`
        // clips to the bar's own `(x, y, width, height)`, so when the
        // (test-environment-dependent) default font is tall enough that
        // the underline's bottom row falls outside the 20px-tall bar,
        // only its top row remains visible — scan both candidate rows
        // rather than assume either survives the clip.
        let underline_top = (text_y + text_h as f64 - 2.0).floor() as i32;
        let scan_from = file_item.bounds.x.floor() as i32 + 8;
        let scan_to = scan_from + char_w.max(1) + 2;
        let found = (underline_top..underline_top + 2)
            .any(|row| (scan_from..scan_to).any(|x| is_painted(&data, stride, x, row)));
        assert!(
            found,
            "expected a painted underline pixel near rows {underline_top}..{}, columns {scan_from}..{scan_to}",
            underline_top + 2,
        );
    }

    /// Non-zero-origin regression guard (quadraui#494 / LESSONS.md):
    /// same round trip, painted at a shifted bar origin.
    #[test]
    fn paint_and_click_round_trip_at_nonzero_origin() {
        paint_and_click_round_trip_at(7.0, 13.0);
    }
}
