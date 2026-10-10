//! macOS rasteriser for [`crate::MenuBar`].
//!
//! Layout (`mac_menu_bar_layout`) stays here — it needs Core Text's own
//! measurement to size each item. Content painting (background,
//! active/disabled colouring, label text) moved to the shared
//! [`crate::primitives::menu_bar::native_surface_paint`].
//!
//! **No Alt-mnemonic underline here.** This backend paints through
//! [`crate::primitives::menu_bar::native_surface_paint::paint_without_mnemonics`]
//! rather than that module's plain `paint`. macOS has no Alt-mnemonic
//! keyboard convention at all — ⌘-based shortcuts
//! ([`crate::accelerator::Accelerator`] / `NSMenuItem.keyEquivalent`) are
//! the platform idiom instead — so underlining a character nothing
//! responds to would be a visual lie.

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
    native_surface_paint::paint_without_mnemonics(bar, &layout, &mut surface, theme);

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
        // macOS resolves `MenuStyle::Auto` to the native menu bar, which
        // paints nothing in-window; these tests cover the painted strip,
        // so they opt out explicitly.
        backend.set_menu_style(crate::backend::MenuStyle::Custom);
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

    /// macOS has no Alt-mnemonic keyboard convention, so `draw_menu_bar`
    /// must never paint the underline bar beneath an item's activation
    /// character, even for the open item. Mirrors `gtk::menu_bar::tests::
    /// draw_menu_bar_paints_alt_underline_beneath_activation_char`'s
    /// scan shape (a long contiguous run of `tab_active_fg` pixels in
    /// the lower half of the item is what an underline bar looks like),
    /// but asserts the run's *absence* — proving the backend routes
    /// through `native_surface_paint::paint_without_mnemonics`, not
    /// `paint`.
    ///
    /// Deliberately does **not** recompute `text_x`/`text_y` via its own
    /// `measure_text(&font(), ..)` call — that assumes the test's font
    /// matches whatever the backend actually painted with, which drifted
    /// after per-backend platform font defaults (#1156) started
    /// overriding `set_current_font`. Instead it scans directly inside
    /// the item's *layout* bounds (which the real paint call already
    /// resolved) for a horizontal run of contiguous `tab_active_fg`
    /// pixels in the lower half of the item — individual glyph strokes
    /// (same colour) don't produce a long run; "F"'s vertical stem is
    /// only 2-3px wide, and its horizontal bars sit in the upper half of
    /// the glyph, above the baseline an underline would sit under.
    #[test]
    fn open_item_paints_no_alt_underline_beneath_activation_char() {
        let bar = sample_bar();
        let (surface, layout) = paint_via_backend(&bar);
        let theme = Theme::default();

        // File ("&File") is the open item.
        let file = &layout.visible_items[0];
        let x0 = file.bounds.x.floor() as u32;
        let x1 = ((file.bounds.x + file.bounds.width).ceil() as u32).min(W);
        let y_mid = (file.bounds.y + file.bounds.height / 2.0).floor() as u32;
        let y1 = ((file.bounds.y + file.bounds.height).ceil() as u32).min(H);

        // What an underline bar would look like, if one were painted —
        // a run at least this long, of exactly this colour, is not
        // explainable by glyph strokes alone.
        const UNDERLINE_RUN: u32 = 3;
        let target = (
            theme.tab_active_fg.r,
            theme.tab_active_fg.g,
            theme.tab_active_fg.b,
        );

        let mut best_run = 0u32;
        for row in y_mid..y1 {
            let mut run = 0u32;
            for x in x0..x1 {
                let (r, g, b, _) = surface.pixel(x, row);
                if (r, g, b) == target {
                    run += 1;
                    best_run = best_run.max(run);
                } else {
                    run = 0;
                }
            }
        }

        assert!(
            best_run < UNDERLINE_RUN,
            "macOS has no Alt-mnemonic convention and must paint no underline bar — found a \
             contiguous run of {best_run} {target:?} pixels in the lower half of the File \
             item (columns {x0}..{x1}, rows {y_mid}..{y1}), as long as an underline bar's own \
             {UNDERLINE_RUN}px minimum",
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
