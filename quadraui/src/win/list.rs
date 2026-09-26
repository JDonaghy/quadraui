//! Direct2D / DirectWrite rasteriser for [`crate::ListView`] (issue #26).
//!
//! Content painting (background, title, rows, h/v scrollbars) moved to
//! the shared [`crate::primitives::list::native_surface_paint::paint`]
//! (#1075, `NativeSurface` Phase 4 slice 2/8) — see that fn's module doc
//! for what's shared and what stays per-backend. This module still owns
//! the [`ListView::bordered`] frame itself: a plain (square-cornered)
//! 1-DIP rectangle border painted *after* the shared content paint (see
//! *Scope for #26* below for why it's square rather than GTK's rounded
//! stroke). Painting the border after, not before, matters: the shared
//! `paint` unconditionally fills the whole rect with `base_bg` as its
//! first operation, which would erase a border drawn beforehand — an
//! ordering bug this module briefly had (#1075 review fix) before
//! matching `gtk::list::draw_list`'s post-content border stroke.
//!
//! Only compiled on `target_os = "windows"` — see `super::mod`'s
//! `#[cfg(target_os = "windows")] mod list;` and `backend.rs`'s module
//! docs. See `win::status_bar`'s module doc for why colours come from
//! `Theme::default()` rather than a live `WinBackend` theme field.
//!
//! # Scope for #26
//!
//! `bordered` lists get a plain (square-cornered) 1-DIP rectangle
//! border rather than GTK's rounded-rect stroke — Direct2D's rounded
//! rectangle would need a second brush/geometry path for no visual
//! contract this issue depends on; a follow-up can round the corners.
//! Nerd-Font icon glyphs are not distinguished from ASCII fallbacks —
//! see `win::tree`'s module doc for why.
//!
//! # Known gap: no horizontal scrollbar (#712)
//!
//! Unlike `gtk_list_layout` / `mac_list_layout`, [`win_list_layout`]
//! takes no `char_width` and never reserves a bottom row for an
//! h-scrollbar — `ListView::max_content_width` is silently ignored for
//! layout purposes on this backend (only `h_scroll`'s pixel cursor
//! shift is honoured, for text already scrolled by an app that has no
//! other way to move the viewport). This is a real feature gap, not the
//! #712 measure-vs-paint drift GTK and macOS had: there is nothing here
//! to disagree with itself. Track closing it as its own follow-up
//! rather than folding it into #712's fix, so a future PR doesn't
//! accidentally claim Windows' h-scrollbar support already matches the
//! other two backends.
//!
//! Note: since #1075, [`draw_list`] *does* paint the vertical
//! scrollbar (`ListView::show_v_scrollbar`) — that gap was specific to
//! all three backends never painting it despite
//! `Backend::list_vscrollbar` already returning real geometry, and the
//! shared `paint` fixes it uniformly. The horizontal-scrollbar gap
//! above is unrelated and unchanged by this migration: `win_list_layout`
//! still never reserves a row for it, so `needs_hscrollbar` (computed
//! inside the shared paint from `char_width`) never triggers on this
//! backend, matching the pre-migration silence exactly.

use windows::Win32::Graphics::Direct2D::ID2D1RenderTarget;

use super::text::{fill_rect, DWrite};
use crate::event::Rect;
use crate::primitives::list::{ListItemMeasure, ListView, ListViewLayout};
use crate::theme::Theme;

/// Border thickness (DIPs) for a `bordered` list — matches the 1-unit
/// inset [`ListView::layout`] itself bakes in for bordered lists.
const BORDER_DIP: f32 = 1.0;

/// Compute a [`ListView`]'s layout without painting — the DirectWrite
/// twin of [`draw_list`]'s internal layout call.
pub fn win_list_layout(list: &ListView, rect: Rect, line_height: f32) -> ListViewLayout {
    let title_height = if list.title.is_some() {
        line_height
    } else {
        0.0
    };
    list.layout(rect.width, rect.height, title_height, |_| {
        ListItemMeasure::new(line_height)
    })
}

/// Draw a [`ListView`] into `rect` (DIPs) on `target`. Returns the
/// resolved [`ListViewLayout`] for host click dispatch (list-local
/// coordinates, matching every other backend's `draw_list` contract).
///
/// # Visual contract
///
/// - **Background:** `Theme::surface_bg` when `bordered`, else
///   `Theme::background`.
/// - **Selected row:** `Theme::selected_bg`.
/// - **Header-decorated row:** `Theme::header_bg` / `header_fg`.
/// - **Muted / Error / Warning rows:** `muted_fg` / `error_fg` /
///   `warning_fg` on the row's own background.
/// - **Detail span:** right-aligned in `muted_fg`, skipped when there
///   isn't room past the main text.
/// - **Vertical scrollbar:** painted via
///   [`crate::primitives::scrollbar::native_surface_paint::paint`] when
///   [`ListView::show_v_scrollbar`] calls for one (#1075) — previously
///   never painted on any of the three pixel backends despite
///   `Backend::list_vscrollbar` already returning real geometry.
pub fn draw_list(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    rect: Rect,
    list: &ListView,
    line_height: f32,
) -> ListViewLayout {
    let theme = Theme::default();
    let layout = win_list_layout(list, rect, line_height);

    let mut surface = super::surface::D2dSurface {
        target,
        dwrite: Some(dwrite),
    };
    crate::primitives::list::native_surface_paint::paint(
        list,
        rect,
        &layout,
        line_height,
        // Nerd-Font icon glyphs: this rasteriser has never distinguished
        // `Icon::glyph` from `Icon::fallback` (unlike `win::tree`, wired
        // in #804) — extending it is separate, unstarted scope (see this
        // module's doc). `false` preserves that exactly.
        /* nerd_fonts_enabled */
        false,
        /* supports_border */ true,
        /* supports_hscrollbar */ false,
        &mut surface,
        &theme,
    );

    // The `bordered` frame is drawn *after* the shared content paint so it
    // isn't erased by `native_surface_paint::paint`'s own initial
    // full-`rect` background fill (`surface_fill_rect(area, base_bg)`,
    // `primitives/list.rs`) — mirroring `gtk::list::draw_list`, which
    // strokes its border after `cr.restore()` for the same reason (#1075
    // review fix: this order used to be reversed here, so a bordered
    // Win-GUI list silently lost its border).
    if list.bordered {
        let br = theme.border_fg;
        let _ = fill_rect(
            target,
            Rect::new(rect.x, rect.y, rect.width, BORDER_DIP),
            br,
        );
        let _ = fill_rect(
            target,
            Rect::new(
                rect.x,
                rect.y + rect.height - BORDER_DIP,
                rect.width,
                BORDER_DIP,
            ),
            br,
        );
        let _ = fill_rect(
            target,
            Rect::new(rect.x, rect.y, BORDER_DIP, rect.height),
            br,
        );
        let _ = fill_rect(
            target,
            Rect::new(
                rect.x + rect.width - BORDER_DIP,
                rect.y,
                BORDER_DIP,
                rect.height,
            ),
            br,
        );
    }

    layout
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::list::{ListItem, ListViewHit};
    use crate::types::{Decoration, StyledText, WidgetId};
    use crate::win::testing::HeadlessSurface;

    const W: f32 = 200.0;
    const H: f32 = 100.0;
    const LINE_HEIGHT: f32 = 14.0;

    fn item(label: &str) -> ListItem {
        ListItem {
            text: StyledText::plain(label.to_string()),
            icon: None,
            detail: None,
            decoration: Decoration::Normal,
        }
    }

    fn make_list(items: Vec<ListItem>) -> ListView {
        ListView {
            id: WidgetId::new("list"),
            title: None,
            items,
            selected_idx: 0,
            scroll_offset: 0,
            has_focus: true,
            bordered: false,
            h_scroll: 0,
            max_content_width: None,
            show_v_scrollbar: false,
        }
    }

    fn make_bordered_list(items: Vec<ListItem>) -> ListView {
        ListView {
            id: WidgetId::new("list"),
            title: None,
            items,
            selected_idx: 0,
            scroll_offset: 0,
            has_focus: true,
            bordered: true,
            h_scroll: 0,
            max_content_width: None,
            show_v_scrollbar: false,
        }
    }

    /// Paint↔click round trip: the selected row's background must be
    /// painted at its own bounds, and clicking each visible row's
    /// centre must hit_test back to that row.
    #[test]
    fn paint_and_hit_test_round_trip() {
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let list = make_list(vec![item("alpha"), item("beta"), item("gamma")]);
        let rect = Rect::new(0.0, 0.0, W, H);

        let layout = surface
            .paint(|target| {
                draw_list(target, &dwrite, rect, &list, LINE_HEIGHT);
            })
            .map(|_| win_list_layout(&list, rect, LINE_HEIGHT))
            .expect("paint list");

        assert_eq!(layout.visible_items.len(), 3);
        for vis in &layout.visible_items {
            let hit = layout.hit_test(
                vis.bounds.x + vis.bounds.width / 2.0,
                vis.bounds.y + vis.bounds.height / 2.0,
            );
            assert_eq!(hit, ListViewHit::Item(vis.item_idx));
        }

        let theme = Theme::default();
        let sel_bounds = layout.visible_items[0].bounds;
        let px = surface.pixel_at((sel_bounds.x + 1.0) as u32, (sel_bounds.y + 1.0) as u32);
        assert_eq!(
            (px.r, px.g, px.b),
            (
                theme.selected_bg.r,
                theme.selected_bg.g,
                theme.selected_bg.b
            ),
            "selected row (idx 0) should paint selected_bg at its own bounds"
        );
    }

    /// Scroll-offset round trip.
    #[test]
    fn scroll_offset_paint_and_click_agree() {
        let mut list = make_list((0..6).map(|i| item(&format!("row-{i}"))).collect());
        list.scroll_offset = 2;
        let rect = Rect::new(0.0, 0.0, W, H);
        let layout = win_list_layout(&list, rect, LINE_HEIGHT);
        let first = layout.visible_items.first().expect("has items");
        assert_eq!(first.item_idx, 2);
        let hit = layout.hit_test(
            first.bounds.x + 5.0,
            first.bounds.y + first.bounds.height / 2.0,
        );
        assert_eq!(hit, ListViewHit::Item(2));
    }

    /// A click below the last item returns `Empty`.
    #[test]
    fn click_below_last_item_returns_empty() {
        let list = make_list(vec![item("a"), item("b")]);
        let rect = Rect::new(0.0, 0.0, W, H);
        let layout = win_list_layout(&list, rect, LINE_HEIGHT);
        let last = layout.visible_items.last().expect("has items");
        let hit = layout.hit_test(10.0, last.bounds.y + last.bounds.height + 5.0);
        assert_eq!(hit, ListViewHit::Empty);
    }

    /// No-paint layout must agree byte-for-byte with what `draw_list`
    /// painted, including a title row.
    #[test]
    fn no_paint_layout_matches_paint_layout() {
        let mut list = make_list(vec![item("alpha"), item("beta")]);
        list.title = Some(StyledText::plain("Files"));
        let rect = Rect::new(0.0, 0.0, W, H);
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");

        let painted = surface
            .paint(|target| {
                draw_list(target, &dwrite, rect, &list, LINE_HEIGHT);
            })
            .map(|_| win_list_layout(&list, rect, LINE_HEIGHT))
            .expect("paint");
        let no_paint = win_list_layout(&list, rect, LINE_HEIGHT);
        assert_eq!(painted, no_paint);
    }

    /// #1075 regression: before the `native_surface_paint` migration,
    /// `draw_list` never painted `ListView::show_v_scrollbar`'s
    /// track/thumb on any of the three pixel backends, even though
    /// `Backend::list_vscrollbar` already returned real geometry for
    /// hit-testing. Paints through the real `draw_list` and probes a
    /// pixel inside the resolved track rect — this would have failed
    /// (background colour, nothing painted) against the pre-#1075 body.
    #[test]
    fn paints_vertical_scrollbar_track_when_enabled() {
        let mut list = make_list((0..30).map(|i| item(&format!("row-{i}"))).collect());
        list.show_v_scrollbar = true;
        let rect = Rect::new(0.0, 0.0, W, H);
        let expected = list
            .vscrollbar(rect, LINE_HEIGHT)
            .expect("30 rows in a 100px / 14px viewport must need a v-scrollbar");

        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
        surface
            .paint(|target| {
                draw_list(target, &dwrite, rect, &list, LINE_HEIGHT);
            })
            .expect("paint list");

        let theme = Theme::default();
        let probe_x = (expected.track.x + expected.track.width / 2.0) as u32;
        let probe_y = (expected.track.y + expected.track.height / 2.0) as u32;
        let px = surface.pixel_at(probe_x.min(W as u32 - 1), probe_y.min(H as u32 - 1));
        assert_ne!(
            (px.r, px.g, px.b),
            (theme.background.r, theme.background.g, theme.background.b),
            "expected the v-scrollbar track at ({probe_x}, {probe_y}) to be painted \
             (non-background) — the #1075 regression this test guards"
        );
    }

    /// #1075 review fix: the `native_surface_paint::paint` migration
    /// briefly drew the `ListView::bordered` frame *before* the shared
    /// content paint, which unconditionally fills the whole `rect` with
    /// `base_bg` as its first paint operation — silently erasing the
    /// border. Paints a bordered list and probes all four edges,
    /// asserting they still carry `theme.border_fg` after the shared
    /// paint has run.
    #[test]
    fn bordered_frame_survives_shared_content_paint() {
        let list = make_bordered_list(vec![item("alpha"), item("beta"), item("gamma")]);
        let rect = Rect::new(0.0, 0.0, W, H);

        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
        surface
            .paint(|target| {
                draw_list(target, &dwrite, rect, &list, LINE_HEIGHT);
            })
            .expect("paint list");

        let theme = Theme::default();
        let border = (theme.border_fg.r, theme.border_fg.g, theme.border_fg.b);
        let mid_x = (W / 2.0) as u32;
        let mid_y = (H / 2.0) as u32;

        let top = surface.pixel_at(mid_x, 0);
        let bottom = surface.pixel_at(mid_x, H as u32 - 1);
        let left = surface.pixel_at(0, mid_y);
        let right = surface.pixel_at(W as u32 - 1, mid_y);

        for (label, px) in [
            ("top", top),
            ("bottom", bottom),
            ("left", left),
            ("right", right),
        ] {
            assert_eq!(
                (px.r, px.g, px.b),
                border,
                "{label} border edge should still be theme.border_fg after the shared \
                 content paint ran (#1075 review regression: the border was being \
                 painted before, then erased by native_surface_paint::paint's initial \
                 full-rect background fill)"
            );
        }
    }
}
