//! GTK rasteriser for [`crate::MenuBar`].
//!
//! Paints a horizontal strip of menu-bar items onto a Cairo context
//! using Pango for text measurement and rendering. Each item's label
//! is rendered with optional Alt-key underline (the char after `&`,
//! or the first char if no `&`). Active/open items get a highlight;
//! disabled items are dimmed.

use gtk4::cairo::Context;
use gtk4::pango;
use pangocairo::functions as pcfn;

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

        let item_x = x + vi.bounds.x as f64;
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
        pcfn::show_layout(cr, pango_layout);
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
/// The `&` in `label` marks the next char; if no `&`, use the first char.
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
        if found {
            idx
        } else {
            0
        }
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
