//! Direct2D / DirectWrite rasteriser for [`crate::MenuBar`] (issue #25).
//!
//! [`win_menu_bar_layout`] stays here — pure geometry generic over
//! [`crate::primitives::layout_metrics::TextMeasure`], no Direct2D/
//! DirectWrite type in its signature, so it compiles and runs everywhere,
//! including a plain `cargo test --features win` on Linux (issue #1078;
//! `super::mod`'s `mod menu_bar;` is no longer whole-module gated — see
//! `backend.rs`'s module docs). Content painting (background,
//! active/disabled colouring, label text, Alt-key underline) moved to
//! the shared [`crate::primitives::menu_bar::native_surface_paint::paint`]
//! (#1081, `NativeSurface` Phase 4 slice 5/8) — see that fn's module doc
//! for the underline-mechanism drift it resolved (this backend's own
//! manual-rectangle underline was the one every backend adopted; GTK's
//! Pango per-character attribute and macOS's total absence of underline
//! both had to give way to it).
//!
//! Takes the live theme as a `&Theme` parameter (quadraui#789) — the
//! caller ([`crate::win::WinBackend::draw_menu_bar`]) passes
//! `&self.current_theme`, the same field `Backend::set_theme` writes.

#[cfg(target_os = "windows")]
use windows::Win32::Graphics::Direct2D::ID2D1RenderTarget;

#[cfg(target_os = "windows")]
use super::text::DWrite;
use crate::event::Rect;
use crate::primitives::layout_metrics::TextMeasure;
#[cfg(target_os = "windows")]
use crate::primitives::menu_bar::native_surface_paint;
#[cfg(target_os = "windows")]
use crate::theme::Theme;
use crate::{MenuBar, MenuBarItemMeasure, MenuBarLayout};

/// Horizontal padding (DIPs) reserved on each side of an item's label —
/// mirrors `gtk::menu_bar::gtk_menu_bar_layout`'s `+ 16.0` (8px each
/// side).
const ITEM_H_PADDING_DIP: f32 = 16.0;

/// Strip `&` markers from a label for display — mirrors
/// `gtk::menu_bar::display_text`.
fn display_text(label: &str) -> String {
    label.chars().filter(|&c| c != '&').collect()
}

/// Compute the [`MenuBar`]'s layout without painting — the measurer twin
/// of [`draw_menu_bar`], and what
/// [`crate::win::WinBackend::menu_bar_layout`] calls directly. Pure
/// geometry over [`TextMeasure`] (issue #1078) — `measure` may be a live
/// `&DWrite` (when painting) or [`super::backend`]'s nominal measurer (no
/// surface yet).
pub fn win_menu_bar_layout(measure: &dyn TextMeasure, rect: Rect, bar: &MenuBar) -> MenuBarLayout {
    bar.layout(rect, |i| {
        let text = display_text(&bar.items[i].label);
        MenuBarItemMeasure::new(measure.width_of(&text) + ITEM_H_PADDING_DIP)
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
/// - **Alt-underline:** a 2px-tall bar under the character following
///   `&` in the raw label, in the label's own foreground colour. No `&`
///   in the label ⇒ no underline at all. See
///   [`crate::primitives::menu_bar::native_surface_paint::paint`] for
///   the exact geometry.
#[cfg(target_os = "windows")]
pub fn draw_menu_bar(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    rect: Rect,
    bar: &MenuBar,
    theme: &Theme,
) -> MenuBarLayout {
    let layout = win_menu_bar_layout(dwrite, rect, bar);

    let mut surface = super::surface::D2dSurface {
        target,
        dwrite: Some(dwrite),
    };
    native_surface_paint::paint(bar, &layout, &mut surface, theme);

    layout
}

// #1078: the paint↔click round-trip tests below need a real
// `DWrite`/`HeadlessSurface` — gated the same way the whole module used
// to be.
#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "windows")]
    mod windows_only {
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
            let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
            let bar = bar();
            let rect = Rect::new(0.0, 0.0, W, H);
            let theme = Theme::default();

            surface
                .paint(|target| {
                    draw_menu_bar(target, &dwrite, rect, &bar, &theme);
                })
                .expect("paint menu bar");

            let layout = win_menu_bar_layout(&dwrite, rect, &bar);
            assert_eq!(layout.visible_items.len(), 2, "both items should fit");

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
            let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
            let bar = bar();
            let rect = Rect::new(3.0, 0.0, W, H);

            let surface = HeadlessSurface::new((W + 3.0) as u32, H as u32).expect("create surface");
            let mut painted = None;
            surface
                .paint(|target| {
                    painted = Some(draw_menu_bar(
                        target,
                        &dwrite,
                        rect,
                        &bar,
                        &Theme::default(),
                    ));
                })
                .expect("paint");
            let painted = painted.expect("draw_menu_bar ran");
            let no_paint = win_menu_bar_layout(&dwrite, rect, &bar);

            assert_eq!(painted, no_paint);
        }
    }

    // `alt_char_index`'s own tests (none-without-ampersand,
    // char-after-ampersand, empty/trailing-marker) moved with it to
    // `crate::primitives::menu_bar::native_surface_paint::tests` (#1081)
    // — `display_text` is the only helper still local to this module
    // (still needed by `win_menu_bar_layout`).
    #[test]
    fn display_text_strips_ampersand() {
        assert_eq!(display_text("&File"), "File");
        assert_eq!(display_text("File"), "File");
    }
}
