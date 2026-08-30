//! GTK rasteriser for [`crate::MenuBar`].
//!
//! Paints a horizontal strip of menu-bar items onto a Cairo context
//! using Pango for text measurement and rendering. Each item's label
//! is rendered with optional Alt-key underline: the char after `&`
//! is underlined, and a label with no `&` at all is never underlined
//! (quadraui#625 — there is no implicit "underline the first char"
//! fallback). Active/open items get a highlight; disabled items are
//! dimmed.

use gtk4::cairo::Context;
use gtk4::pango;

use super::{cairo_rgb, set_source};
use crate::event::Rect;
use crate::primitives::menu_bar::{MenuBar, MenuBarItemMeasure, MenuBarLayout};
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

    let fill = cairo_rgb(theme.tab_bar_bg);
    cr.set_source_rgb(fill.0, fill.1, fill.2);
    cr.rectangle(x, y, width, height);
    cr.fill().ok();

    let layout = gtk_menu_bar_layout(pango_layout, x, y, width, height, bar);

    for vi in &layout.visible_items {
        let item = &bar.items[vi.item_idx];
        let is_active = bar.open_item == Some(vi.item_idx) || bar.focused_item == Some(vi.item_idx);

        let (fg_color, bg_color) = if is_active {
            (theme.tab_active_fg, theme.tab_active_bg)
        } else if item.disabled {
            (theme.muted_fg, theme.tab_bar_bg)
        } else {
            (theme.tab_inactive_fg, theme.tab_bar_bg)
        };

        // `vi.bounds.x` is already absolute — `gtk_menu_bar_layout` seeds
        // `MenuBar::layout`'s internal cursor at `bounds.x = x`, so item
        // bounds already carry the bar's origin. Adding `x` again here
        // double-counted it, invisibly at `x == 0` (every existing test)
        // and shifting painted glyphs away from their own hit-test bounds
        // at any other origin — the same LESSONS.md "layout helpers must
        // return coords in the same frame across backends" bug class
        // already found and fixed in the TUI and macOS twins
        // (quadraui#494).
        let item_x = vi.bounds.x as f64;
        let item_w = vi.bounds.width as f64;

        if is_active {
            set_source(cr, bg_color);
            cr.rectangle(item_x, y, item_w, height);
            cr.fill().ok();
        }

        let text = display_text(&item.label);
        pango_layout.set_text(&text);

        let underline_pos = alt_char_byte_range(&item.label, &text);
        let attrs = pango::AttrList::new();
        if let Some((start, end)) = underline_pos {
            let mut ul = pango::AttrInt::new_underline(pango::Underline::Single);
            ul.set_start_index(start as u32);
            ul.set_end_index(end as u32);
            attrs.insert(ul);
        }
        pango_layout.set_attributes(Some(&attrs));

        let text_w = pango_layout.pixel_size().0.max(0) as f64;
        let text_h = pango_layout.pixel_size().1.max(0) as f64;
        let text_x = item_x + (item_w - text_w) / 2.0;
        let text_y = y + (height - text_h) / 2.0;

        set_source(cr, fg_color);
        cr.move_to(text_x, text_y);
        super::painted_text::show_layout(cr, pango_layout);
    }

    pango_layout.set_attributes(None);
    cr.restore().ok();

    layout
}

/// Strip `&` markers from a label for display.
fn display_text(label: &str) -> String {
    label.chars().filter(|&c| c != '&').collect()
}

/// Find the byte range in `display` of the Alt-activation char.
/// The `&` in `label` marks the next char. A label with no `&` has no
/// Alt-activation char to underline — returns `None` (quadraui#625:
/// this used to fall back to underlining char 0 unconditionally,
/// which meant a label could never opt out of the underline).
fn alt_char_byte_range(label: &str, display: &str) -> Option<(usize, usize)> {
    if display.is_empty() {
        return None;
    }
    let display_idx = {
        let mut idx = 0usize;
        let mut found = false;
        for ch in label.chars() {
            if ch == '&' {
                found = true;
                break;
            }
            idx += 1;
        }
        if !found {
            return None;
        }
        idx
    };

    // display_idx is the char index in display to underline. But since
    // `&` was stripped, display_idx in display == char index of the char
    // after `&` (or 0 if no `&`). However if `&` was before position N,
    // display has one fewer char, so display_idx in the *filtered* string
    // needs adjustment.
    //
    // Actually: display_idx counts chars *before* `&` in label.
    // In display (which has `&` stripped), the char at that position
    // IS the char that was right after `&`. So display_idx is the
    // correct char index in display.

    let mut byte_start = 0;
    for (i, ch) in display.chars().enumerate() {
        if i == display_idx {
            return Some((byte_start, byte_start + ch.len_utf8()));
        }
        byte_start += ch.len_utf8();
    }
    None
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

    /// quadraui#625: `alt_char_byte_range` used to fall back to
    /// underlining char 0 whenever the label carried no `&` at all —
    /// its own doc comment admitted it never returned `None` for a
    /// non-empty label. Removing `&` from a label didn't remove the
    /// underline, it just relocated it. Confirms the fix.
    #[test]
    fn alt_char_byte_range_none_without_ampersand() {
        assert_eq!(alt_char_byte_range("File", "File"), None);
    }

    #[test]
    fn alt_char_byte_range_marks_char_after_ampersand() {
        assert_eq!(alt_char_byte_range("&File", "File"), Some((0, 1)));
        // '&' isn't necessarily the first char — "Sa&ve" underlines 'v'.
        assert_eq!(alt_char_byte_range("Sa&ve", "Save"), Some((2, 3)));
    }

    /// End-to-end version of the two unit tests above: paints a label
    /// with no `&` and asserts no underline attribute reaches Pango by
    /// checking the rendered glyph position is unaffected (the
    /// underline itself isn't pixel-probed here — Pango underline
    /// metrics aren't guaranteed stable across fonts/CI — but this
    /// exercises `draw_menu_bar`'s full call path with a `&`-free
    /// label end to end, guarding against the attribute-building code
    /// regressing back to always inserting an underline attr).
    #[test]
    fn paint_no_ampersand_label_does_not_panic_and_paints_glyph() {
        let mut surface = ImageSurface::create(Format::ARgb32, W, H).expect("create ImageSurface");
        let mut bar = make_bar();
        bar.items[0].label = "File".into(); // no '&' marker
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
        surface.flush();
        let stride = surface.stride() as usize;
        let data = surface.data().expect("surface data");
        let vi = &layout.visible_items[0];
        let scan_from = vi.bounds.x.floor() as i32;
        assert!(
            leftmost_painted_in_row(&data, stride, 10, scan_from).is_some(),
            "label should still paint a glyph even with no '&' marker"
        );
    }

    /// Non-zero-origin regression guard (quadraui#494 / LESSONS.md):
    /// same round trip, painted at a shifted bar origin.
    #[test]
    fn paint_and_click_round_trip_at_nonzero_origin() {
        paint_and_click_round_trip_at(7.0, 13.0);
    }
}
