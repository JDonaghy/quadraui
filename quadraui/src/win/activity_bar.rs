//! Direct2D / DirectWrite rasteriser for [`crate::ActivityBar`] (issue #25).
//!
//! [`win_activity_bar_layout`] calls [`ActivityBar::layout`] with
//! [`ACTIVITY_ROW_DIP`] as the item height; content painting moved to
//! the shared
//! [`crate::primitives::activity_bar::native_surface_paint::paint`]
//! (#1081, `NativeSurface` Phase 4 slice 5/8), which also **adds this
//! backend's missing right-edge separator**: pre-migration
//! `win::activity_bar` was the only one of the three that never painted
//! the 1px `theme.separator` column GTK and macOS both had — no doc
//! comment explained why, so this reads as an oversight the shared
//! paint now closes. Win still takes the
//! [`crate::Backend::draw_activity_bar_with_style`] default (no
//! [`crate::ActivityBarStyle`] override — this module always paints
//! with `ActivityBarStyle::default()`), same as before.
//!
//! Only compiled on `target_os = "windows"` — see `super::mod`'s
//! `#[cfg(target_os = "windows")] mod activity_bar;` and `backend.rs`'s
//! module docs.
//!
//! Takes the live theme as a `&Theme` parameter (quadraui#789) — the
//! caller ([`crate::win::WinBackend::draw_activity_bar`]) passes
//! `&self.current_theme`, the same field `Backend::set_theme` writes.

use windows::Win32::Graphics::Direct2D::ID2D1RenderTarget;

use super::text::DWrite;
use crate::event::Rect;
use crate::primitives::activity_bar::{native_surface_paint, ActivityBarStyle};
use crate::theme::Theme;
use crate::{ActivityBar, ActivityBarLayout, ActivityBarRowHit};

/// Row height (DIPs) of a single activity-bar item — the DirectWrite twin
/// of [`crate::gtk::activity_bar::ACTIVITY_ROW_PX`]'s 48px value.
pub const ACTIVITY_ROW_DIP: f32 = 48.0;

/// Compute an [`ActivityBar`]'s layout without painting — what
/// [`crate::win::WinBackend::activity_bar_layout`] calls directly, and
/// the twin [`draw_activity_bar`] paints from.
pub fn win_activity_bar_layout(rect: Rect, bar: &ActivityBar) -> ActivityBarLayout {
    bar.layout(rect.width, rect.height, ACTIVITY_ROW_DIP)
}

/// Draw an [`ActivityBar`] into `rect` (DIPs) on `target`. Returns
/// per-row hit regions **relative to `rect`** (first row's `y_start` is
/// always `0.0`, per [`crate::Backend::draw_activity_bar`]'s coordinate
/// contract — issue #552).
///
/// # Visual contract
///
/// - **Background:** filled with `theme.tab_bar_bg`.
/// - **Keyboard-selected row:** `bar.selection_bg`, or
///   `theme.tab_bar_bg.lighten(0.20)` when unset.
/// - **Hovered row** (and not keyboard-selected): `theme.tab_bar_bg.lighten(0.10)`.
/// - **Active item's accent line:** 2 DIP left-edge strip in
///   `bar.active_accent`, painted only when that field is `Some` — no
///   theme fallback (matches every other backend as of #658).
/// - **Icon glyph:** centred in the row; `theme.foreground` for
///   active/hovered/selected rows, `theme.inactive_fg` otherwise.
pub fn draw_activity_bar(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    rect: Rect,
    bar: &ActivityBar,
    hovered_idx: Option<usize>,
    theme: &Theme,
) -> Vec<ActivityBarRowHit> {
    // `win_activity_bar_layout` (like `ActivityBar::layout` itself) is
    // already bar-relative — `rect.x`/`rect.y` never reach it. Pre-#1081
    // this fn baked `rect.x`/`rect.y` into each `row_rect` by hand;
    // `native_surface_paint::paint` instead expects bar-relative
    // coordinates (it paints `(0, 0, width, height)`, matching
    // `gtk::activity_bar` / `macos::activity_bar`'s contract), so this
    // now translates the render target itself via
    // `super::text::with_translation` — the exact same shape as
    // `GtkBackend::draw_activity_bar`'s `cr.translate(rect.x, rect.y)`
    // and `MacBackend::draw_activity_bar`'s `CGContextTranslateCTM`
    // (see that module's doc, "What #552's audit missed").
    let layout = win_activity_bar_layout(rect, bar);

    let mut surface = super::surface::D2dSurface {
        target,
        dwrite: Some(dwrite),
    };
    super::text::with_translation(target, rect.x, rect.y, || {
        native_surface_paint::paint(
            bar,
            &layout,
            &ActivityBarStyle::default(),
            &mut surface,
            theme,
            hovered_idx,
            false,
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::activity_bar::{ActivityBarHit, ActivityItem};
    use crate::types::{Color, WidgetId};
    use crate::win::testing::HeadlessSurface;
    use crate::ActivitySide;

    const W: f32 = 48.0;
    const H: f32 = 200.0;

    fn bar() -> ActivityBar {
        ActivityBar {
            id: WidgetId::new("activity"),
            top_items: vec![
                ActivityItem {
                    id: WidgetId::new("activity:explorer"),
                    icon: "E".into(),
                    tooltip: "Explorer".into(),
                    is_active: true,
                    is_keyboard_selected: false,
                },
                ActivityItem {
                    id: WidgetId::new("activity:search"),
                    icon: "S".into(),
                    tooltip: "Search".into(),
                    is_active: false,
                    is_keyboard_selected: false,
                },
            ],
            bottom_items: vec![ActivityItem {
                id: WidgetId::new("activity:settings"),
                icon: "G".into(),
                tooltip: "Settings".into(),
                is_active: false,
                is_keyboard_selected: false,
            }],
            active_accent: Some(Color::rgb(80, 140, 255)),
            selection_bg: None,
            is_keyboard_focused: false,
        }
    }

    /// Paint↔click round trip: the active item's 2px accent strip must be
    /// painted at its row's own bounds, and a click at the centre of each
    /// row (per the independently-computed no-paint layout) must
    /// `hit_test` back to that row's `WidgetId`.
    #[test]
    fn paint_and_hit_test_round_trip() {
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let bar = bar();
        let rect = Rect::new(0.0, 0.0, W, H);

        surface
            .paint(|target| {
                draw_activity_bar(target, &dwrite, rect, &bar, None, &Theme::default());
            })
            .expect("paint activity bar");

        let layout = win_activity_bar_layout(rect, &bar);

        // Explorer (top_items[0], active) is the first row painted at
        // y in [0, ACTIVITY_ROW_DIP) — its accent strip must show up at
        // x in [0, 2).
        let accent = Color::rgb(80, 140, 255);
        let mid_y = (ACTIVITY_ROW_DIP / 2.0) as u32;
        let px = surface.pixel_at(0, mid_y);
        assert_eq!(
            (px.r, px.g, px.b),
            (accent.r, accent.g, accent.b),
            "active row's accent strip should be visible at x=0"
        );

        for item in bar.top_items.iter().chain(bar.bottom_items.iter()) {
            let hit = layout
                .visible_items
                .iter()
                .find(|vi| {
                    let candidate = match vi.side {
                        ActivitySide::Top => &bar.top_items[vi.item_idx],
                        ActivitySide::Bottom => &bar.bottom_items[vi.item_idx],
                    };
                    candidate.id == item.id
                })
                .expect("item is visible");
            let cy = hit.bounds.y + hit.bounds.height / 2.0;
            assert_eq!(
                layout.hit_test(1.0, cy),
                ActivityBarHit::Item(item.id.clone()),
                "row centre for {:?} should hit-test back to itself",
                item.id,
            );
        }
    }

    /// Row spans are bar-relative — the first visible row always starts
    /// at `y_start == 0.0`, regardless of where `rect` sits (issue #552).
    #[test]
    fn hit_regions_are_bar_relative_not_absolute() {
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let bar = bar();
        let surface = HeadlessSurface::new(W as u32, (H + 40.0) as u32).expect("create surface");
        let rect = Rect::new(0.0, 40.0, W, H);

        let mut hits = None;
        surface
            .paint(|target| {
                hits = Some(draw_activity_bar(
                    target,
                    &dwrite,
                    rect,
                    &bar,
                    None,
                    &Theme::default(),
                ));
            })
            .expect("paint");
        let hits = hits.expect("draw_activity_bar ran");

        // `ActivityBarLayout::visible_items` puts bottom-pinned items
        // first (see that field's doc), so `hits[0]` is the *settings*
        // row, not the visually-topmost one. Look up the top-pinned
        // explorer item explicitly rather than assuming array order.
        let explorer_id = bar.top_items[0].id.clone();
        let explorer_hit = hits
            .iter()
            .find(|h| h.id == explorer_id)
            .expect("explorer row is visible");
        assert_eq!(
            explorer_hit.y_start, 0.0,
            "the topmost row must start at bar-relative y=0 regardless of rect.y"
        );
    }

    /// #1081 regression: pre-migration `win::activity_bar` was the only
    /// one of the three backends that never painted the right-edge
    /// separator column GTK/macOS both have. The shared
    /// `native_surface_paint::paint` now fills it uniformly.
    #[test]
    fn right_edge_has_separator_pixel() {
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let bar = bar();
        let rect = Rect::new(0.0, 0.0, W, H);
        let theme = Theme::default();

        surface
            .paint(|target| {
                draw_activity_bar(target, &dwrite, rect, &bar, None, &theme);
            })
            .expect("paint activity bar");

        let px = surface.pixel_at(W as u32 - 1, H as u32 / 2);
        assert_eq!(
            (px.r, px.g, px.b),
            (theme.separator.r, theme.separator.g, theme.separator.b),
            "right-edge column should paint theme.separator (quadraui#1081 \
             — this used to be a no-op on Windows)",
        );
    }

    /// The `with_translation`-based repaint at a non-zero `rect` origin
    /// (#1081 — replacing the old per-coordinate `rect.x`/`rect.y`
    /// addition) must still land the bar's own background and accent
    /// strip at `rect`'s actual on-screen position, not at the surface's
    /// literal `(0, 0)`.
    #[test]
    fn paints_at_rect_origin_not_at_surface_origin() {
        const ORIGIN_X: f32 = 10.0;
        const ORIGIN_Y: f32 = 6.0;
        let surface = HeadlessSurface::new((W + ORIGIN_X) as u32, (H + ORIGIN_Y) as u32)
            .expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let bar = bar();
        let rect = Rect::new(ORIGIN_X, ORIGIN_Y, W, H);
        let theme = Theme::default();

        surface
            .paint(|target| {
                draw_activity_bar(target, &dwrite, rect, &bar, None, &theme);
            })
            .expect("paint activity bar");

        // Explorer (top_items[0], active) is the first row — its accent
        // strip must show up at the *shifted* x ∈ [ORIGIN_X, ORIGIN_X+2),
        // y centred in the first row below ORIGIN_Y.
        let accent = Color::rgb(80, 140, 255);
        let mid_y = ORIGIN_Y as u32 + (ACTIVITY_ROW_DIP / 2.0) as u32;
        let px = surface.pixel_at(ORIGIN_X as u32, mid_y);
        assert_eq!(
            (px.r, px.g, px.b),
            (accent.r, accent.g, accent.b),
            "active row's accent strip should shift with rect's own origin",
        );

        // The bar's background must not have painted the surface's
        // literal top-left corner (still outside `rect`).
        let corner = surface.pixel_at(0, 0);
        assert_ne!(
            (corner.r, corner.g, corner.b),
            (theme.tab_bar_bg.r, theme.tab_bar_bg.g, theme.tab_bar_bg.b),
            "activity bar background must not leak past rect's own origin",
        );
    }
}
