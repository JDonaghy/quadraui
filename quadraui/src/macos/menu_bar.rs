//! macOS rasteriser for [`crate::MenuBar`].
//!
//! Layout (`mac_menu_bar_layout`) stays here — it needs Core Text's own
//! measurement to size each item. Content painting (background,
//! active/disabled colouring, label text, Alt-key underline) moved to
//! the shared [`crate::primitives::menu_bar::native_surface_paint::paint`]
//! (#1081, `NativeSurface` Phase 4 slice 5/8), which also **closes this
//! backend's own documented gap**: pre-migration macOS painted no
//! Alt-key underline at all (Core Text's `kCTUnderlineStyleAttributeName`
//! needs attributed-string plumbing `super::text::draw_text` never had —
//! see this module's git history for the old "Scope omissions" doc).
//! The shared `paint` draws the underline as a manually-positioned
//! filled rectangle instead of a font-level attribute, so it needs
//! nothing beyond the measure/fill verbs every backend already has —
//! see that fn's module doc for the full three-way drift it resolved.

use core_graphics::sys::CGContextRef;
use core_text::font::CTFont;

use super::text::measure_text;
use crate::event::Rect as QRect;
use crate::primitives::menu_bar::{
    native_surface_paint, MenuBar, MenuBarItemMeasure, MenuBarLayout,
};
use crate::theme::Theme;

/// 8-pt padding each side of the menu label inside its hit slot.
const ITEM_PAD: f32 = 8.0;

/// Compute the macOS pixel-unit layout for `bar` without painting.
/// Mirrors `crate::gtk::gtk_menu_bar_layout`. Apps call this to route
/// clicks via the same layout the rasteriser used.
pub fn mac_menu_bar_layout(
    font: &CTFont,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    bar: &MenuBar,
) -> MenuBarLayout {
    let bounds = QRect::new(x as f32, y as f32, width as f32, height as f32);
    bar.layout(bounds, |i| {
        let text = display_text(&bar.items[i].label);
        let (w, _) = measure_text(font, &text);
        MenuBarItemMeasure::new(w as f32 + ITEM_PAD * 2.0)
    })
}

/// Paint `bar` into `(x, y, width, height)` on `ctx`. Returns the
/// layout for caller click dispatch.
///
/// # Safety
///
/// `ctx` must be a valid `CGContextRef` borrowed for the duration of
/// the call.
#[allow(clippy::too_many_arguments)]
pub unsafe fn draw_menu_bar(
    ctx: CGContextRef,
    font: &CTFont,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    bar: &MenuBar,
    theme: &Theme,
) -> MenuBarLayout {
    CGContextSaveGState(ctx);

    let layout = mac_menu_bar_layout(font, x, y, width, height, bar);

    let mut surface = super::surface::CgSurface {
        ctx,
        font: Some(font),
    };
    native_surface_paint::paint(bar, &layout, &mut surface, theme);

    CGContextRestoreGState(ctx);
    layout
}

/// Strip `&` markers from a label for display.
fn display_text(label: &str) -> String {
    label.chars().filter(|&c| c != '&').collect()
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
    use crate::event::Viewport;
    use crate::primitives::menu_bar::{MenuBar, MenuBarHit, MenuBarItem};
    use crate::theme::Theme;
    use crate::types::WidgetId;
    use crate::Backend;

    const W: u32 = 320;
    const H: u32 = 24;

    fn font() -> CTFont {
        make_font("Menlo", 14.0).expect("Menlo installed")
    }

    fn sample_bar() -> MenuBar {
        MenuBar {
            id: WidgetId::new("menus"),
            items: vec![
                MenuBarItem {
                    id: WidgetId::new("menu:file"),
                    label: "&File".into(),
                    disabled: false,
                    submenu: None,
                },
                MenuBarItem {
                    id: WidgetId::new("menu:edit"),
                    label: "&Edit".into(),
                    disabled: false,
                    submenu: None,
                },
                MenuBarItem {
                    id: WidgetId::new("menu:view"),
                    label: "&View".into(),
                    disabled: true,
                    submenu: None,
                },
            ],
            open_item: Some(0),
            focused_item: None,
        }
    }

    fn paint_via_backend(bar: &MenuBar) -> (BitmapSurface, MenuBarLayout) {
        let surface = BitmapSurface::new(W, H);
        surface.fill(0.0, 0.0, 0.0, 0.0);
        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        let layout = std::cell::RefCell::new(None);
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            let l = b.draw_menu_bar(QRect::new(0.0, 0.0, W as f32, H as f32), bar);
            *layout.borrow_mut() = Some(l);
        });
        backend.end_frame();
        (surface, layout.into_inner().unwrap())
    }

    #[test]
    fn open_item_paints_active_bg() {
        // First item (File) is the open menu — its bg should be
        // tab_active_bg, distinct from tab_bar_bg.
        let bar = sample_bar();
        let (surface, layout) = paint_via_backend(&bar);
        let theme = Theme::default();
        let file = &layout.visible_items[0];
        // Probe column 1 (inside the leading padding so no glyph
        // ink) at the bottom edge of the bar.
        let probe_x = (file.bounds.x as u32) + 1;
        let probe_y = H - 2;
        let (r, g, b, _) = surface.pixel(probe_x, probe_y);
        assert_eq!(
            (r, g, b),
            (
                theme.tab_active_bg.r,
                theme.tab_active_bg.g,
                theme.tab_active_bg.b
            ),
        );
    }

    #[test]
    fn inactive_item_paints_bar_bg() {
        let bar = sample_bar();
        let (surface, layout) = paint_via_backend(&bar);
        let theme = Theme::default();
        // Second item (Edit) is inactive — its bg should still be
        // tab_bar_bg (not painted over).
        let edit = &layout.visible_items[1];
        let probe_x = (edit.bounds.x as u32) + 1;
        let probe_y = H - 2;
        let (r, g, b, _) = surface.pixel(probe_x, probe_y);
        assert_eq!(
            (r, g, b),
            (theme.tab_bar_bg.r, theme.tab_bar_bg.g, theme.tab_bar_bg.b),
        );
    }

    /// Regression guard for #1081: pre-migration macOS painted **no**
    /// Alt-key underline at all (see this module's doc comment above —
    /// Core Text's attributed-string underline plumbing never existed
    /// here). Mirrors `gtk::menu_bar::tests::
    /// draw_menu_bar_paints_alt_underline_beneath_activation_char`, but
    /// runs through the real `draw_menu_bar` → `CgSurface` → CGContext
    /// path instead of a synthetic `RecordingSurface`, so it actually
    /// proves the macOS backend now paints the pixel, not just that the
    /// shared `native_surface_paint::paint` fn computes the right rect.
    #[test]
    fn open_item_paints_alt_underline_beneath_activation_char() {
        let bar = sample_bar();
        let (surface, layout) = paint_via_backend(&bar);
        let theme = Theme::default();
        let f = font();

        // File ("&File") is the open item — underline sits under 'F'
        // (char index 0, no prefix offset) in `theme.tab_active_fg`.
        // Reproduce `native_surface_paint::paint`'s own measurements
        // (same font, same text) rather than assuming pixel rows.
        let file = &layout.visible_items[0];
        let (text_w, text_h) = measure_text(&f, "File");
        let (char_w, _) = measure_text(&f, "F");
        let text_x = file.bounds.x + (file.bounds.width - text_w as f32) / 2.0;
        let text_y = file.bounds.y + (file.bounds.height - text_h as f32) / 2.0;
        let underline_top = (text_y + text_h as f32 - 2.0).floor() as u32;
        let scan_x_from = text_x.floor() as u32;
        let scan_x_to = scan_x_from + (char_w.max(1.0) as u32) + 1;

        let found = (underline_top..underline_top + 2).any(|row| {
            (scan_x_from..scan_x_to).any(|x| {
                let (r, g, b, _) = surface.pixel(x, row);
                (r, g, b)
                    == (
                        theme.tab_active_fg.r,
                        theme.tab_active_fg.g,
                        theme.tab_active_fg.b,
                    )
            })
        });
        assert!(
            found,
            "expected a painted underline pixel near rows {underline_top}..{}, columns {scan_x_from}..{scan_x_to}",
            underline_top + 2,
        );
    }

    /// `cargo test -p quadraui --features macos -- --ignored --nocapture macos::menu_bar::tests::dump_smoke_ppm`
    ///
    /// Paints the sample bar (File / Edit / View, File open) into a
    /// 320 × 24 surface and writes `/tmp/quadraui_menu_bar.ppm`. Open
    /// in Preview to confirm:
    /// - "File" reads in `tab_active_fg` over `tab_active_bg` (the
    ///   open-menu highlight).
    /// - "Edit" reads in `tab_inactive_fg` over the bar's normal bg.
    /// - "View" (disabled) reads dimmer (`muted_fg`).
    /// - `&` markers are stripped from rendered text.
    #[test]
    #[ignore = "writes /tmp/quadraui_menu_bar.ppm — opt in with --ignored"]
    fn dump_smoke_ppm() {
        let bar = sample_bar();
        let (surface, _) = paint_via_backend(&bar);
        surface.write_ppm_and_open("/tmp/quadraui_menu_bar.ppm");
    }

    #[test]
    fn hit_test_resolves_clickable_items_via_layout() {
        let bar = sample_bar();
        let (_surface, layout) = paint_via_backend(&bar);
        // Clickable items (File, Edit) resolve as Item(i). Disabled
        // View falls through to Bar — matches the primitive contract.
        for (i, vi) in layout.visible_items.iter().enumerate() {
            let cx = vi.bounds.x + vi.bounds.width * 0.5;
            let cy = vi.bounds.y + vi.bounds.height * 0.5;
            let expected = if vi.clickable {
                MenuBarHit::Item(i)
            } else {
                MenuBarHit::Bar
            };
            assert_eq!(
                layout.hit_test(cx, cy),
                expected,
                "item {} (clickable={})",
                i,
                vi.clickable
            );
        }
    }

    #[test]
    fn hit_test_resolves_clickable_items_at_nonzero_origin() {
        // Regression for quadraui#494 / LESSONS.md "Layout helpers must
        // return coords in the same frame across backends": every other
        // test in this file lays out at origin (0, 0). `mac_menu_bar_
        // layout` bakes `x`/`y` straight into `visible_items[].bounds`
        // (absolute frame, matching the GTK/TUI twins) via
        // `MenuBar::layout`, whose cursor starts at `bounds.x` — call it
        // directly (pure fn, no paint needed) at a non-zero origin and
        // prove hit_test still resolves the right item.
        let bar = sample_bar();
        let f = font();
        let origin_x = 7.0;
        let origin_y = 13.0;
        let layout = mac_menu_bar_layout(&f, origin_x, origin_y, W as f64, H as f64, &bar);
        for (i, vi) in layout.visible_items.iter().enumerate() {
            let cx = vi.bounds.x + vi.bounds.width * 0.5;
            let cy = vi.bounds.y + vi.bounds.height * 0.5;
            let expected = if vi.clickable {
                MenuBarHit::Item(i)
            } else {
                MenuBarHit::Bar
            };
            assert_eq!(
                layout.hit_test(cx, cy),
                expected,
                "item {} (clickable={})",
                i,
                vi.clickable
            );
        }
    }
}
