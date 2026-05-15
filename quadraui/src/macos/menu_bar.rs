//! macOS rasteriser for [`crate::MenuBar`].
//!
//! Horizontal strip of top-level menu labels. Mirrors
//! [`crate::gtk::menu_bar`]: per-item Core Text measurement,
//! active-item highlight, label rendering with `&` stripped.
//!
//! ## Scope omissions (follow-up)
//!
//! - **Alt-key underline** — GTK applies a Pango underline attribute
//!   to the `&`-marked character. Core Text supports underline via
//!   `kCTUnderlineStyleAttributeName` but the existing
//!   [`super::text::draw_text`] path doesn't thread attributes
//!   through. Deferred with bold/italic to a unified text-attribute
//!   pass.

use core_graphics::geometry::CGRect;
use core_graphics::sys::CGContextRef;
use core_text::font::CTFont;

use super::text::{draw_text, measure_text};
use crate::event::Rect as QRect;
use crate::primitives::menu_bar::{MenuBar, MenuBarItemMeasure, MenuBarLayout};
use crate::theme::Theme;
use crate::types::Color;

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
    fill_rect(ctx, x, y, width, height, theme.tab_bar_bg);

    let layout = mac_menu_bar_layout(font, x, y, width, height, bar);

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
            fill_rect(ctx, item_x, y, item_w, height, bg_color);
        }

        let text = display_text(&item.label);
        let (text_w, text_h) = measure_text(font, &text);
        let text_x = item_x + (item_w - text_w) / 2.0;
        let text_y = y + (height - text_h) / 2.0;
        draw_text(ctx, font, &text, text_x, text_y, color_to_cg(fg_color));
    }

    CGContextRestoreGState(ctx);
    layout
}

/// Strip `&` markers from a label for display.
fn display_text(label: &str) -> String {
    label.chars().filter(|&c| c != '&').collect()
}

fn color_to_cg(c: Color) -> (f64, f64, f64, f64) {
    (
        c.r as f64 / 255.0,
        c.g as f64 / 255.0,
        c.b as f64 / 255.0,
        c.a as f64 / 255.0,
    )
}

unsafe fn fill_rect(ctx: CGContextRef, x: f64, y: f64, w: f64, h: f64, c: Color) {
    let (r, g, b, a) = color_to_cg(c);
    CGContextSetRGBFillColor(ctx, r, g, b, a);
    use core_graphics::geometry::{CGPoint, CGSize};
    CGContextFillRect(ctx, CGRect::new(&CGPoint::new(x, y), &CGSize::new(w, h)));
}

extern "C" {
    fn CGContextSaveGState(c: CGContextRef);
    fn CGContextRestoreGState(c: CGContextRef);
    fn CGContextSetRGBFillColor(
        c: CGContextRef,
        red: core_graphics::base::CGFloat,
        green: core_graphics::base::CGFloat,
        blue: core_graphics::base::CGFloat,
        alpha: core_graphics::base::CGFloat,
    );
    fn CGContextFillRect(c: CGContextRef, rect: CGRect);
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
                },
                MenuBarItem {
                    id: WidgetId::new("menu:edit"),
                    label: "&Edit".into(),
                    disabled: false,
                },
                MenuBarItem {
                    id: WidgetId::new("menu:view"),
                    label: "&View".into(),
                    disabled: true,
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
}
