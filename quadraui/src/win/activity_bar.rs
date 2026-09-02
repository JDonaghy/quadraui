//! Direct2D / DirectWrite rasteriser for [`crate::ActivityBar`] (issue #25).
//!
//! Calls [`ActivityBar::layout`] with [`ACTIVITY_ROW_DIP`] as the item
//! height, then paints from the resolved [`crate::ActivityBarLayout`].
//! Paint and hit-test both derive from the one layout call. Mirrors
//! `gtk::activity_bar`'s row-fill / hover-tint / accent-line contract,
//! minus the opt-in [`crate::ActivityBarStyle`] knob (#658) — Win takes
//! the [`crate::Backend::draw_activity_bar_with_style`] default, same as
//! every backend that hasn't opted in.
//!
//! Only compiled on `target_os = "windows"` — see `super::mod`'s
//! `#[cfg(target_os = "windows")] mod activity_bar;` and `backend.rs`'s
//! module docs.
//!
//! See `win::status_bar`'s module doc for why colours come from
//! [`Theme::default`] rather than a live `WinBackend` theme field —
//! nothing has wired real theme plumbing through to this backend yet.

use windows::Win32::Graphics::Direct2D::ID2D1RenderTarget;

use super::text::{fill_rect, DWrite};
use crate::event::Rect;
use crate::theme::Theme;
use crate::{ActivityBar, ActivityBarLayout, ActivityBarRowHit, ActivitySide};

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
) -> Vec<ActivityBarRowHit> {
    let theme = Theme::default();
    let _ = fill_rect(target, rect, theme.tab_bar_bg);

    let layout = win_activity_bar_layout(rect, bar);
    let mut regions: Vec<ActivityBarRowHit> = Vec::new();

    for (flat_idx, vi) in layout.visible_items.iter().enumerate() {
        let item = match vi.side {
            ActivitySide::Top => &bar.top_items[vi.item_idx],
            ActivitySide::Bottom => &bar.bottom_items[vi.item_idx],
        };
        let row_rect = Rect::new(
            rect.x + vi.bounds.x,
            rect.y + vi.bounds.y,
            vi.bounds.width,
            vi.bounds.height,
        );
        let is_hovered = hovered_idx == Some(flat_idx);

        // Selection wins over hover when both apply (matches GTK: the
        // brighter selection tint paints after, and thus over, the dimmer
        // hover tint).
        if item.is_keyboard_selected {
            let sel_bg = bar
                .selection_bg
                .unwrap_or_else(|| theme.tab_bar_bg.lighten(0.20));
            let _ = fill_rect(target, row_rect, sel_bg);
        } else if is_hovered {
            let _ = fill_rect(target, row_rect, theme.tab_bar_bg.lighten(0.10));
        }

        // #658: no theme fallback — `None` paints zero accent pixels.
        if item.is_active {
            if let Some(accent) = bar.active_accent {
                let accent_rect = Rect::new(row_rect.x, row_rect.y, 2.0, row_rect.height);
                let _ = fill_rect(target, accent_rect, accent);
            }
        }

        let fg = if item.is_active || is_hovered || item.is_keyboard_selected {
            theme.foreground
        } else {
            theme.inactive_fg
        };
        // Uses the ASCII `fallback` — this rasteriser doesn't take a
        // per-frame `nerd_fonts_enabled` toggle yet (issue #683 scoped
        // TUI/GTK/macOS only; Win-GUI has no Nerd Font wiring at all,
        // matching `macos::tree`/`macos::form`'s same fallback-only
        // posture until #25's icon-font plumbing lands here too).
        let icon_str = item.icon.fallback.as_str();
        let (iw, ih) = dwrite.measure_text(icon_str).unwrap_or((0.0, 0.0));
        let icon_rect = Rect::new(
            row_rect.x + (row_rect.width - iw) / 2.0,
            row_rect.y + (row_rect.height - ih) / 2.0,
            iw.max(1.0),
            ih.max(1.0),
        );
        let _ = dwrite.draw_text(target, icon_str, icon_rect, fg);

        regions.push(ActivityBarRowHit {
            y_start: vi.bounds.y as f64,
            y_end: (vi.bounds.y + vi.bounds.height) as f64,
            id: item.id.clone(),
            tooltip: item.tooltip.clone(),
        });
    }

    regions
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::activity_bar::{ActivityBarHit, ActivityItem};
    use crate::types::{Color, WidgetId};
    use crate::win::testing::HeadlessSurface;

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
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
        let bar = bar();
        let rect = Rect::new(0.0, 0.0, W, H);

        surface
            .paint(|target| {
                draw_activity_bar(target, &dwrite, rect, &bar, None);
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
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
        let bar = bar();
        let surface = HeadlessSurface::new(W as u32, (H + 40.0) as u32).expect("create surface");
        let rect = Rect::new(0.0, 40.0, W, H);

        let mut hits = None;
        surface
            .paint(|target| {
                hits = Some(draw_activity_bar(target, &dwrite, rect, &bar, None));
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
}
