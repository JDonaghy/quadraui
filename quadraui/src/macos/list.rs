//! macOS rasteriser for [`crate::ListView`].
//!
//! Content painting (background, title, rows, h/v scrollbars) moved to
//! the shared [`crate::primitives::list::native_surface_paint::paint`]
//! (#1075, `NativeSurface` Phase 4 slice 2/8) — see that fn's module doc
//! for what's shared and what stays per-backend.
//!
//! ## Scope omissions (follow-up)
//!
//! - **`bordered` mode** — same status as before this migration: no
//!   consumer sets it today. The flat header+rows path is fully
//!   supported; this rasteriser passes `supports_border: false` to the
//!   shared paint, so it always takes the flat title/inset path
//!   regardless of [`crate::ListView::bordered`]. The shared paint's
//!   `base_bg` pick (`surface_bg` when `bordered`, else `background`)
//!   is *not* gated on `supports_border`, so it still honours
//!   [`crate::ListView::bordered`] here even though nothing else does —
//!   unlike the pre-migration `draw_list`, which ignored `bordered`
//!   entirely and always painted `theme.background`. No consumer sets
//!   `bordered: true` on macOS today, so this is currently invisible;
//!   flagged so a future consumer doesn't assume the background colour
//!   is pinned to pre-migration behaviour. Add the rounded-rect frame +
//!   overlay title when a consumer needs full `bordered` support.

use core_graphics::geometry::CGRect;
use core_graphics::sys::CGContextRef;
use core_text::font::CTFont;

use super::text::measure_text;
use crate::primitives::list::{ListView, ListViewLayout};
use crate::theme::Theme;

/// Compute the layout the macOS rasteriser would produce for `list`
/// at `(w, h)` and `line_height`, including the horizontal-scrollbar
/// row reservation (#712). Hosts and tests call this to drive
/// hit-testing without re-deriving row pitch. Title (if any) takes
/// one `line_height` strip; items use the same.
///
/// The reservation math lives in
/// [`crate::primitives::layout_metrics::list_layout`] — shared with
/// `gtk_list_layout` so both backends reserve identically. `draw_list`
/// calls this exact function (with its own live-measured `char_width`)
/// instead of recomputing a second layout at paint time, so paint and
/// no-paint hit-testing can never drift apart (`PRIMITIVE_RULES.md`
/// rule 5). Before #712, `draw_list` recomputed independently and this
/// function had no `char_width` parameter at all, so it could not
/// compute the reservation — `Backend::list_layout` was one row taller
/// than what was actually painted whenever `max_content_width` forced a
/// scrollbar.
///
/// `char_width` is used only for the h-scrollbar-overflow threshold
/// check (`ListView::max_content_width` is in character columns); pass
/// [`crate::Backend::char_width`]'s cached value when no live `CTFont`
/// measurement is available — the same approximation
/// `MacBackend::list_hscrollbar` already uses for this exact check.
///
/// macOS does not support [`ListView::bordered`] yet (see this module's
/// "Scope omissions"), so the border inset passed to the shared fn is
/// always `0.0`.
///
/// Coordinate frame: `visible_items.bounds`, `title_bounds`, and
/// `hit_regions` are in **list-local** coords (origin at 0, 0),
/// matching `tui_list_layout` and `gtk_list_layout`. Hosts must
/// subtract the list's `area.x` / `area.y` from absolute click coords
/// before calling [`ListViewLayout::hit_test`]. The `x` / `y` params
/// are kept in the signature for symmetry with `draw_list` but do not
/// affect output.
pub fn mac_list_layout(
    list: &ListView,
    _x: f64,
    _y: f64,
    w: f64,
    h: f64,
    line_height: f64,
    char_width: f64,
) -> ListViewLayout {
    crate::primitives::layout_metrics::list_layout(list, w, h, line_height, char_width, 0.0)
}

/// Draw a [`ListView`] into `(x, y, w, h)` on `ctx`. Returns the same
/// layout `mac_list_layout` would produce — callers route clicks
/// against this to consume one layout per frame.
///
/// `nerd_fonts_enabled` controls which icon variant an item's
/// [`crate::types::Icon`] paints — see
/// [`crate::primitives::list::native_surface_paint::paint`]'s doc.
/// Before this parameter existed, this rasteriser never painted
/// `ListItem::icon` at all (a real omission, not a nerd-font policy
/// choice — see `MacBackend::draw_list`'s call site for how it
/// threads `self.nerd_fonts_enabled` through now, matching GTK/Windows).
///
/// # Safety
///
/// `ctx` must be a valid `CGContextRef` borrowed for the duration of
/// the call (typical: the frame-scope pointer on
/// [`super::MacBackend`]). Calling with a freed or null pointer is UB.
#[allow(clippy::too_many_arguments)]
pub unsafe fn draw_list(
    ctx: CGContextRef,
    font: &CTFont,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    list: &ListView,
    theme: &Theme,
    line_height: f64,
    nerd_fonts_enabled: bool,
) -> ListViewLayout {
    // Measure a reference glyph for char-to-pixel conversion up front —
    // `mac_list_layout` needs it for the h-scrollbar-overflow threshold
    // check, and the shared paint re-derives the same measurement for
    // its own `h_scroll` cursor shift.
    let (char_w, _) = measure_text(font, "M");
    let char_w = char_w.max(1.0);

    if w <= 0.0 || h <= 0.0 {
        return mac_list_layout(list, x, y, w.max(0.0), h.max(0.0), line_height, char_w);
    }

    let layout = mac_list_layout(list, x, y, w, h, line_height, char_w);

    CGContextSaveGState(ctx);
    // Clip to the list rect so right-aligned detail / scroll-overflow
    // rows don't paint past the viewport.
    CGContextClipToRect(ctx, CGRect::new_xywh(x, y, w, h));

    let area = crate::event::Rect::new(x as f32, y as f32, w as f32, h as f32);
    let mut surface = super::surface::CgSurface {
        ctx,
        font: Some(font),
    };
    crate::primitives::list::native_surface_paint::paint(
        list,
        area,
        &layout,
        line_height as f32,
        nerd_fonts_enabled,
        /* supports_border */ false,
        /* supports_hscrollbar */ true,
        &mut surface,
        theme,
    );

    CGContextRestoreGState(ctx);
    layout
}

trait CGRectExt {
    fn new_xywh(x: f64, y: f64, w: f64, h: f64) -> Self;
}
impl CGRectExt for CGRect {
    fn new_xywh(x: f64, y: f64, w: f64, h: f64) -> Self {
        use core_graphics::geometry::{CGPoint, CGSize};
        CGRect::new(&CGPoint::new(x, y), &CGSize::new(w, h))
    }
}

extern "C" {
    fn CGContextSaveGState(c: CGContextRef);
    fn CGContextRestoreGState(c: CGContextRef);
    fn CGContextClipToRect(c: CGContextRef, rect: CGRect);
}

#[cfg(test)]
mod tests {
    use super::super::headless::BitmapSurface;
    use super::super::text::make_font;
    use super::super::MacBackend;
    use super::*;
    use crate::event::{Rect as QRect, Viewport};
    use crate::primitives::list::{ListItem, ListViewHit};
    use crate::types::{Color, Decoration, StyledSpan, StyledText, WidgetId};
    use crate::Backend;

    const W: u32 = 240;
    const H: u32 = 160;

    fn font() -> CTFont {
        make_font("Menlo", 14.0).expect("Menlo installed")
    }

    fn sample_item(label: &str, bg: Color) -> ListItem {
        ListItem {
            text: StyledText {
                spans: vec![StyledSpan {
                    text: label.into(),
                    fg: Some(Color::rgb(255, 255, 255)),
                    bg: Some(bg),
                    bold: false,
                    italic: false,
                    underline: false,
                }],
            },
            icon: None,
            detail: None,
            decoration: Decoration::Normal,
        }
    }

    fn sample_list() -> ListView {
        ListView {
            id: WidgetId::new("lv"),
            title: Some(StyledText::plain("Quick fix")),
            items: vec![
                sample_item("alpha", Color::rgb(10, 20, 30)),
                sample_item("beta", Color::rgb(10, 20, 30)),
                sample_item("gamma", Color::rgb(10, 20, 30)),
                sample_item("delta", Color::rgb(10, 20, 30)),
            ],
            selected_idx: 1,
            scroll_offset: 0,
            has_focus: true,
            bordered: false,
            h_scroll: 0,
            max_content_width: None,
            // Field added to `ListView` after this fixture was written;
            // it never compiled until quadraui#484 first built the macOS
            // test target. `false` preserves the fixture's behaviour.
            show_v_scrollbar: false,
        }
    }

    fn paint_via_backend(list: &ListView) -> (BitmapSurface, ListViewLayout) {
        let surface = BitmapSurface::new(W, H);
        surface.fill(0.0, 0.0, 0.0, 0.0);
        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        let layout = std::cell::RefCell::new(None);
        let rect = QRect::new(0.0, 0.0, W as f32, H as f32);
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            b.draw_list(rect, list);
            // Go through `Backend::list_layout` — the same no-paint
            // resolver a click router calls — rather than the raw
            // `mac_list_layout` free fn, so this fixture doubles as
            // coverage that the trait method agrees with `draw_list`.
            let l = b.list_layout(rect, list);
            *layout.borrow_mut() = Some(l);
        });
        backend.end_frame();
        (surface, layout.into_inner().unwrap())
    }

    #[test]
    fn title_strip_paints_header_bg() {
        let list = sample_list();
        let (surface, layout) = paint_via_backend(&list);
        let theme = Theme::default();
        let tb = layout.title_bounds.expect("title present");
        // Probe at the right edge of the title strip (no glyph there).
        let px = (tb.x + tb.width - 2.0) as u32;
        let py = (tb.y + tb.height / 2.0) as u32;
        let (r, g, b, _) = surface.pixel(px, py);
        assert_eq!(
            (r, g, b),
            (theme.header_bg.r, theme.header_bg.g, theme.header_bg.b),
        );
    }

    #[test]
    fn selected_row_paints_selected_bg() {
        let list = sample_list();
        let (surface, layout) = paint_via_backend(&list);
        let theme = Theme::default();
        // selected_idx = 1 → second visible row (after title).
        let sel = layout
            .visible_items
            .iter()
            .find(|v| v.item_idx == 1)
            .expect("selected row visible");
        // Probe near right edge to dodge the "▶ " prefix glyphs.
        let px = (sel.bounds.x + sel.bounds.width - 2.0) as u32;
        let py = (sel.bounds.y + sel.bounds.height / 2.0) as u32;
        let (r, g, b, _) = surface.pixel(px, py);
        assert_eq!(
            (r, g, b),
            (
                theme.selected_bg.r,
                theme.selected_bg.g,
                theme.selected_bg.b
            ),
        );
    }

    #[test]
    fn unselected_row_paints_background() {
        let list = sample_list();
        let (surface, layout) = paint_via_backend(&list);
        let theme = Theme::default();
        let other = layout
            .visible_items
            .iter()
            .find(|v| v.item_idx == 2)
            .expect("non-selected row visible");
        let px = (other.bounds.x + other.bounds.width - 2.0) as u32;
        let py = (other.bounds.y + other.bounds.height / 2.0) as u32;
        let (r, g, b, _) = surface.pixel(px, py);
        assert_eq!(
            (r, g, b),
            (theme.background.r, theme.background.g, theme.background.b),
        );
    }

    #[test]
    fn hit_test_resolves_painted_rows() {
        let list = sample_list();
        let (_surface, layout) = paint_via_backend(&list);
        for vis in &layout.visible_items {
            let cx = vis.bounds.x + vis.bounds.width * 0.5;
            let cy = vis.bounds.y + vis.bounds.height * 0.5;
            assert_eq!(
                layout.hit_test(cx, cy),
                ListViewHit::Item(vis.item_idx),
                "row {} hit-test",
                vis.item_idx,
            );
        }
    }

    #[test]
    fn layout_returns_local_coords_when_area_offset() {
        // Cross-backend contract: visible_items.bounds, title_bounds,
        // and hit_regions are in list-local coords (origin 0, 0),
        // regardless of where `mac_list_layout` is called with as its
        // (x, y) — matching `tui_list_layout` and `gtk_list_layout`.
        // Hosts subtract area.x/area.y from absolute click coords
        // before hit_test.
        //
        // Regression for #190: prior to the fix, mac_list_layout
        // shifted hit_regions to absolute coords. Latent today (no
        // `Backend::list_layout` trait method exposes the layout to
        // consumers), but ready to bite the moment one is added —
        // same shape as #44's tree/form click drift.
        let list = sample_list();
        // Area offset by (0, 60) — typical when a list lives below
        // a header / search input.
        let area_x: f64 = 0.0;
        let area_y: f64 = 60.0;
        let layout = mac_list_layout(&list, area_x, area_y, W as f64, H as f64, 16.0, 8.0);
        // Locality: title_bounds.y must be 0, not 60.
        let tb = layout.title_bounds.expect("title present");
        assert_eq!(
            tb.y, 0.0,
            "title_bounds.y must be local (0.0), got {}",
            tb.y,
        );
        // Round-trip: simulate a click at the absolute centre of each
        // painted row, localise the way AppLogic does, and assert it
        // hits the right row. Pre-fix this returned the wrong row.
        for vi in &layout.visible_items {
            let abs_x = area_x as f32 + vi.bounds.x + vi.bounds.width * 0.5;
            let abs_y = area_y as f32 + vi.bounds.y + vi.bounds.height * 0.5;
            let local_x = abs_x - area_x as f32;
            let local_y = abs_y - area_y as f32;
            assert_eq!(
                layout.hit_test(local_x, local_y),
                ListViewHit::Item(vi.item_idx),
                "row {} click → wrong hit (coord-frame drift)",
                vi.item_idx,
            );
        }
    }

    #[test]
    fn hit_test_below_last_row_is_empty() {
        // Two short items in a tall viewport — clicks past the last
        // row's bottom must return Empty, not the last row.
        let list = ListView {
            items: vec![
                sample_item("alpha", Color::rgb(10, 20, 30)),
                sample_item("beta", Color::rgb(10, 20, 30)),
            ],
            ..sample_list()
        };
        let (_surface, layout) = paint_via_backend(&list);
        let last = layout.visible_items.last().expect("at least one row");
        let cx = last.bounds.x + last.bounds.width * 0.5;
        let below_y = last.bounds.y + last.bounds.height + 4.0;
        assert_eq!(layout.hit_test(cx, below_y), ListViewHit::Empty);
    }

    /// Regression for #712: before this fix, `mac_list_layout` had no
    /// `char_width` parameter and so could not compute the h-scrollbar
    /// reservation at all, while `draw_list` recomputed a *second*,
    /// reduced-height layout only at paint time. That meant
    /// `Backend::list_layout` (routed through `mac_list_layout` alone)
    /// was one row taller than what `draw_list` actually painted
    /// whenever `max_content_width` forced a scrollbar — a click router
    /// driven purely by `list_layout` would mis-resolve the bottom row.
    ///
    /// Force that overflow and assert the no-paint `Backend::list_layout`
    /// call agrees byte-for-byte with the layout `draw_list` resolved
    /// internally while painting (`paint_via_backend` itself now goes
    /// through `Backend::list_layout`, so this also proves the trait
    /// method and `draw_list` never drift apart), and that the
    /// reservation actually took effect.
    #[test]
    fn hscrollbar_reservation_matches_layout_and_paint() {
        let mut list = sample_list();
        // Wide enough (in chars) that, multiplied by any plausible char
        // width, it overflows the W-px viewport and forces a scrollbar.
        list.max_content_width = Some(1000);

        let (_surface, painted_layout) = paint_via_backend(&list);

        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        let no_paint_layout =
            Backend::list_layout(&backend, QRect::new(0.0, 0.0, W as f32, H as f32), &list);

        assert_eq!(
            no_paint_layout, painted_layout,
            "Backend::list_layout must equal the layout draw_list actually \
             painted once an h-scrollbar row is reserved"
        );

        // Sanity: the scrollbar really was reserved — the last visible
        // row must stop at or before the reserved bottom row's top edge,
        // not fill the full H-px viewport as it would if the reservation
        // were silently dropped.
        let last = no_paint_layout
            .visible_items
            .last()
            .expect("at least one row visible");
        assert!(
            last.bounds.y + last.bounds.height <= H as f32 - backend.line_height(),
            "content must stop before the reserved h-scrollbar row: \
             last row bottom = {}, viewport H = {H}, line_height = {}",
            last.bounds.y + last.bounds.height,
            backend.line_height(),
        );
    }

    /// #1075 review fix: `gtk::list` and `win::list` each gained a
    /// driver-tier regression test proving `ListView::show_v_scrollbar`
    /// is actually painted (it wasn't, on any of the three backends,
    /// pre-migration); `macos::list` had none. Paints through the real
    /// `MacBackend::draw_list` with 30 rows in a viewport that can only
    /// fit a handful, and probes a pixel inside the track rect
    /// `Backend::list_vscrollbar` resolves — this would fail (background
    /// colour, nothing painted) against a `draw_list` that never called
    /// the shared scrollbar paint.
    #[test]
    fn paints_vertical_scrollbar_track_when_enabled() {
        let mut list = ListView {
            title: None,
            items: (0..30)
                .map(|i| sample_item(&format!("row-{i}"), Color::rgb(10, 20, 30)))
                .collect(),
            show_v_scrollbar: true,
            ..sample_list()
        };
        list.selected_idx = 0;

        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        let rect = QRect::new(0.0, 0.0, W as f32, H as f32);
        let expected = backend
            .list_vscrollbar(rect, &list)
            .expect("30 rows in a small viewport must need a v-scrollbar");

        let (surface, _layout) = paint_via_backend(&list);
        let theme = Theme::default();
        let probe_x = (expected.track.x + expected.track.width / 2.0) as u32;
        let probe_y = (expected.track.y + expected.track.height / 2.0) as u32;
        let (r, g, b, _) = surface.pixel(probe_x.min(W - 1), probe_y.min(H - 1));
        assert_ne!(
            (r, g, b),
            (theme.background.r, theme.background.g, theme.background.b),
            "expected the v-scrollbar track at ({probe_x}, {probe_y}) to be painted \
             (non-background) — the #1075 regression this test guards"
        );
    }

    /// #1075 review fix: before this migration, `macos::list::draw_list`
    /// never painted `ListItem::icon` at all — a real omission called out
    /// in this module's own doc comment, but left uncovered by any test.
    /// Compares the inked (non-background) pixel width of a row's band
    /// with vs. without an icon set; the icon-bearing row must paint
    /// strictly more ink, proving the shared paint's icon branch actually
    /// ran through `MacBackend::draw_list`.
    #[test]
    fn paints_item_icon_when_present() {
        use crate::types::Icon;

        let inked_width = |icon: Option<Icon>| -> u32 {
            let list = ListView {
                title: None,
                items: vec![ListItem {
                    text: StyledText::plain("x"),
                    icon,
                    detail: None,
                    decoration: Decoration::Normal,
                }],
                selected_idx: usize::MAX, // no row selected
                ..sample_list()
            };
            let (surface, layout) = paint_via_backend(&list);
            let row = layout.visible_items.first().expect("row painted");
            let mid_y = (row.bounds.y + row.bounds.height / 2.0) as u32;
            let theme = Theme::default();
            let bg = (theme.background.r, theme.background.g, theme.background.b);
            let mut left = None;
            let mut right = None;
            for x in row.bounds.x as u32..(row.bounds.x + row.bounds.width) as u32 {
                let (r, g, b, _) = surface.pixel(x.min(W - 1), mid_y.min(H - 1));
                if (r, g, b) != bg {
                    left.get_or_insert(x);
                    right = Some(x);
                }
            }
            match (left, right) {
                (Some(l), Some(r)) => r - l + 1,
                _ => 0,
            }
        };

        let without_icon = inked_width(None);
        let with_icon = inked_width(Some(Icon::new("WWWW", "WWWW")));
        assert!(
            with_icon > without_icon,
            "a row with an icon should paint strictly more ink ({with_icon}px) than \
             the same row without one ({without_icon}px) — the #1075 icon-paint \
             regression this test guards"
        );
    }
}
