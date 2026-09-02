//! Direct2D / DirectWrite rasteriser for [`crate::MenuBar`] (issue #25).
//!
//! Paints a horizontal strip of menu-bar items using DirectWrite for text
//! measurement and painting. Each item's label supports an Alt-key
//! underline: the character right after `&` is underlined by painting a
//! thin filled rectangle beneath it (DirectWrite's native
//! `IDWriteTextLayout::SetUnderline` API is deliberately not used here —
//! a manually-positioned rectangle reuses the same measure/fill
//! primitives every other rasteriser in this module already relies on,
//! rather than introducing a new, unverified DirectWrite call path). A
//! label with no `&` at all is never underlined — no implicit
//! "underline the first char" fallback, mirroring `gtk::menu_bar`
//! (quadraui#625).
//!
//! Only compiled on `target_os = "windows"` — see `super::mod`'s
//! `#[cfg(target_os = "windows")] mod menu_bar;` and `backend.rs`'s
//! module docs. See `win::status_bar`'s module doc for why colours come
//! from `Theme::default()` rather than a live `WinBackend` theme field.

use windows::Win32::Graphics::Direct2D::ID2D1RenderTarget;

use super::text::{fill_rect, DWrite};
use crate::event::Rect;
use crate::theme::Theme;
use crate::{MenuBar, MenuBarItemMeasure, MenuBarLayout};

/// Horizontal padding (DIPs) reserved on each side of an item's label —
/// mirrors `gtk::menu_bar::gtk_menu_bar_layout`'s `+ 16.0` (8px each
/// side).
const ITEM_H_PADDING_DIP: f32 = 16.0;
/// Thickness (DIPs) of the Alt-key underline rectangle.
const UNDERLINE_HEIGHT_DIP: f32 = 2.0;

/// Strip `&` markers from a label for display — mirrors
/// `gtk::menu_bar::display_text`.
fn display_text(label: &str) -> String {
    label.chars().filter(|&c| c != '&').collect()
}

/// The **char index** (not byte index — DirectWrite measurement works
/// over substrings) into the display string of the Alt-activation
/// character (the character immediately after `&`), or `None` when
/// `label` carries no `&` at all. Mirrors
/// `gtk::menu_bar::alt_char_byte_range`'s "no implicit fallback"
/// contract (quadraui#625).
fn alt_char_index(label: &str) -> Option<usize> {
    let marker_byte = label.find('&')?;
    Some(label[..marker_byte].chars().count())
}

/// Compute the [`MenuBar`]'s layout without painting — the DirectWrite
/// measurer twin of [`draw_menu_bar`], and what
/// [`crate::win::WinBackend::menu_bar_layout`] calls directly.
pub fn win_menu_bar_layout(dwrite: &DWrite, rect: Rect, bar: &MenuBar) -> MenuBarLayout {
    bar.layout(rect, |i| {
        let text = display_text(&bar.items[i].label);
        let (w, _) = dwrite.measure_text(&text).unwrap_or((0.0, 0.0));
        MenuBarItemMeasure::new(w + ITEM_H_PADDING_DIP)
    })
}

/// Draw a [`MenuBar`] into `rect` (DIPs) on `target`. Returns the layout
/// for host click dispatch.
///
/// # Visual contract
///
/// - **Background:** filled with `theme.tab_bar_bg`.
/// - **Open/focused item:** `theme.tab_active_bg` fill,
///   `theme.tab_active_fg` label.
/// - **Disabled item:** `theme.muted_fg` label, no fill.
/// - **Alt-underline:** a [`UNDERLINE_HEIGHT_DIP`]-tall bar under the
///   character following `&` in the raw label, in the label's own
///   foreground colour. No `&` in the label ⇒ no underline at all.
pub fn draw_menu_bar(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    rect: Rect,
    bar: &MenuBar,
) -> MenuBarLayout {
    let theme = Theme::default();
    let _ = fill_rect(target, rect, theme.tab_bar_bg);

    let layout = win_menu_bar_layout(dwrite, rect, bar);

    for vi in &layout.visible_items {
        let item = &bar.items[vi.item_idx];
        let is_active = bar.open_item == Some(vi.item_idx) || bar.focused_item == Some(vi.item_idx);

        let (fg, bg) = if is_active {
            (theme.tab_active_fg, theme.tab_active_bg)
        } else if item.disabled {
            (theme.muted_fg, theme.tab_bar_bg)
        } else {
            (theme.tab_inactive_fg, theme.tab_bar_bg)
        };

        if is_active {
            let _ = fill_rect(target, vi.bounds, bg);
        }

        let text = display_text(&item.label);
        let (text_w, text_h) = dwrite.measure_text(&text).unwrap_or((0.0, 0.0));
        let text_x = vi.bounds.x + (vi.bounds.width - text_w) / 2.0;
        let text_y = vi.bounds.y + (vi.bounds.height - text_h) / 2.0;
        let text_rect = Rect::new(text_x, text_y, text_w, text_h);
        let _ = dwrite.draw_text(target, &text, text_rect, fg);

        if let Some(idx) = alt_char_index(&item.label) {
            if let Some(ch) = text.chars().nth(idx) {
                let prefix: String = text.chars().take(idx).collect();
                let (prefix_w, _) = dwrite.measure_text(&prefix).unwrap_or((0.0, 0.0));
                let (char_w, _) = dwrite.measure_text(&ch.to_string()).unwrap_or((0.0, 0.0));
                let underline_rect = Rect::new(
                    text_x + prefix_w,
                    text_y + text_h - UNDERLINE_HEIGHT_DIP,
                    char_w.max(1.0),
                    UNDERLINE_HEIGHT_DIP,
                );
                let _ = fill_rect(target, underline_rect, fg);
            }
        }
    }

    layout
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::menu_bar::{MenuBarHit, MenuBarItem};
    use crate::types::{Color, WidgetId};
    use crate::win::testing::HeadlessSurface;

    const W: f32 = 300.0;
    const H: f32 = 24.0;

    fn bar() -> MenuBar {
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
            open_item: Some(0),
            focused_item: None,
        }
    }

    fn is_painted(surface: &HeadlessSurface, x: u32, y: u32, bg: Color) -> bool {
        let px = surface.pixel_at(x, y);
        (px.r, px.g, px.b) != (bg.r, bg.g, bg.b)
    }

    /// Paint↔click round trip: each visible item paints a distinguishable
    /// glyph inside its own bounds, and a click at each item's own
    /// (absolute) bounds centre resolves back to that item via
    /// `hit_test`.
    #[test]
    fn paint_and_hit_test_round_trip() {
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
        let bar = bar();
        let rect = Rect::new(0.0, 0.0, W, H);

        surface
            .paint(|target| {
                draw_menu_bar(target, &dwrite, rect, &bar);
            })
            .expect("paint menu bar");

        let layout = win_menu_bar_layout(&dwrite, rect, &bar);
        assert_eq!(layout.visible_items.len(), 2, "both items should fit");

        let theme = Theme::default();
        for vi in &layout.visible_items {
            let cx = vi.bounds.x + vi.bounds.width / 2.0;
            let cy = vi.bounds.y + vi.bounds.height / 2.0;
            assert_eq!(
                layout.hit_test(cx, cy),
                MenuBarHit::Item(vi.item_idx),
                "item {} centre should hit-test back to itself",
                vi.item_idx,
            );

            // Some pixel inside the item's row must differ from the bar's
            // own background — either the open item's active-bg fill, or
            // an inactive item's painted label glyph.
            let row_y = cy as u32;
            let found = (vi.bounds.x as u32..(vi.bounds.x + vi.bounds.width) as u32)
                .any(|x| is_painted(&surface, x, row_y, theme.tab_bar_bg));
            assert!(
                found,
                "item {} should paint something distinguishable from the bar background",
                vi.item_idx,
            );
        }
    }

    /// The no-paint layout must agree byte-for-byte with what
    /// `draw_menu_bar` painted.
    #[test]
    fn no_paint_layout_matches_paint_layout() {
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
        let bar = bar();
        let rect = Rect::new(3.0, 0.0, W, H);

        let surface = HeadlessSurface::new((W + 3.0) as u32, H as u32).expect("create surface");
        let mut painted = None;
        surface
            .paint(|target| {
                painted = Some(draw_menu_bar(target, &dwrite, rect, &bar));
            })
            .expect("paint");
        let painted = painted.expect("draw_menu_bar ran");
        let no_paint = win_menu_bar_layout(&dwrite, rect, &bar);

        assert_eq!(painted, no_paint);
    }

    #[test]
    fn alt_char_index_none_without_ampersand() {
        assert_eq!(alt_char_index("File"), None);
    }

    #[test]
    fn alt_char_index_marks_char_after_ampersand() {
        assert_eq!(alt_char_index("&File"), Some(0));
        // '&' isn't necessarily the first char — "Sa&ve" underlines 'v'
        // (char index 2 in "Save").
        assert_eq!(alt_char_index("Sa&ve"), Some(2));
    }

    #[test]
    fn alt_char_index_handles_empty_and_trailing_marker() {
        assert_eq!(alt_char_index(""), None);
        // A trailing `&` marks a char index past the display string's end
        // — `draw_menu_bar`'s `text.chars().nth(idx)` guard turns that
        // into "no underline" rather than panicking.
        assert_eq!(alt_char_index("File&"), Some(4));
        assert_eq!(display_text("File&").chars().nth(4), None);
    }
}
