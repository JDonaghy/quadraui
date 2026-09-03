//! macOS rasteriser for [`crate::ListView`].
//!
//! Mirrors [`crate::gtk::list::draw_list`]: optional title strip at the
//! top, then flat rows from `scroll_offset` until the viewport fills.
//! Selection / header / decoration styling matches the GTK contract.
//!
//! ## Scope omissions (follow-up)
//!
//! - **`bordered` mode** — same status as GTK: no consumer sets it
//!   today. The flat header+rows path is fully supported. Add the
//!   rounded-rect frame + overlay title when a consumer needs it.

use core_graphics::geometry::CGRect;
use core_graphics::sys::CGContextRef;
use core_text::font::CTFont;

use super::text::{draw_text, measure_text};
use crate::primitives::list::{ListView, ListViewLayout};
use crate::theme::Theme;
use crate::types::{Color, Decoration};

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
) -> ListViewLayout {
    // Measure a reference glyph for char-to-pixel conversion up front —
    // `mac_list_layout` needs it for the h-scrollbar-overflow threshold
    // check, and `h_off_px` below reuses the same measurement. `h_scroll`
    // is expressed in character columns (matching TUI cells); macOS
    // works in pixels, so we multiply by `char_w` before offsetting
    // cursor positions.
    let (char_w, _) = measure_text(font, "M");
    let char_w = char_w.max(1.0);

    if w <= 0.0 || h <= 0.0 {
        return mac_list_layout(list, x, y, w.max(0.0), h.max(0.0), line_height, char_w);
    }

    let h_off_px = list.h_scroll as f64 * char_w;

    // #712: `mac_list_layout` is the single source of truth for the
    // h-scrollbar row reservation — no separate recompute here. Before
    // #712 this branched on a locally-recomputed `needs_hscrollbar` and
    // called `list.layout` a second time with a reduced height, which
    // `Backend::list_layout` (routed through `mac_list_layout` alone)
    // had no way to reproduce.
    let needs_hscrollbar = list
        .max_content_width
        .is_some_and(|n| n as f64 * char_w > w);
    let layout = mac_list_layout(list, x, y, w, h, line_height, char_w);

    CGContextSaveGState(ctx);
    // Clip to the list rect so right-aligned detail / scroll-overflow
    // rows don't paint past the viewport.
    CGContextClipToRect(ctx, CGRect::new_xywh(x, y, w, h));

    fill_rect(ctx, x, y, w, h, theme.background);

    if let (Some(title_bounds), Some(title)) = (layout.title_bounds, list.title.as_ref()) {
        // Layout returns local coords; shift to absolute for paint.
        let tx = title_bounds.x as f64 + x;
        let ty = title_bounds.y as f64 + y;
        let th = title_bounds.height as f64;
        fill_rect(ctx, tx, ty, w, th, theme.header_bg);
        let title_text: String = title.spans.iter().map(|s| s.text.as_str()).collect();
        let (_, text_h) = measure_text(font, &title_text);
        draw_text(
            ctx,
            font,
            &title_text,
            tx + 2.0,
            ty + (th - text_h) / 2.0,
            color_to_cg(theme.header_fg),
        );
    }

    for vis in &layout.visible_items {
        let item = &list.items[vis.item_idx];
        // Layout returns local coords; shift to absolute for paint.
        let row_x = vis.bounds.x as f64 + x;
        let row_y = vis.bounds.y as f64 + y;
        let row_w = vis.bounds.width as f64;
        let row_h = vis.bounds.height as f64;

        let is_selected = vis.item_idx == list.selected_idx && list.has_focus;

        let decoration_fg = match item.decoration {
            Decoration::Error => theme.error_fg,
            Decoration::Warning => theme.warning_fg,
            Decoration::Muted => theme.muted_fg,
            Decoration::Header => theme.header_fg,
            _ => theme.surface_fg,
        };
        let row_bg = if is_selected {
            theme.selected_bg
        } else if matches!(item.decoration, Decoration::Header) {
            theme.header_bg
        } else {
            theme.background
        };

        fill_rect(ctx, row_x, row_y, row_w, row_h, row_bg);

        // Per-row clip: with h_scroll the cursor starts to the left of `row_x`,
        // so scrolled-off glyphs would paint outside the row band.  The list-
        // level CGContextClipToRect already clips to the list box, but adding a
        // row-level clip is cheap and makes the intent explicit.
        CGContextSaveGState(ctx);
        CGContextClipToRect(ctx, CGRect::new_xywh(row_x, row_y, row_w, row_h));

        // Shift cursor left by the horizontal scroll offset so that content
        // columns < h_scroll are clipped away by the clip set above.
        let mut cursor_x = row_x + 2.0 - h_off_px;

        let prefix = if is_selected { "▶ " } else { "  " };
        let (pw, _) = measure_text(font, prefix);
        let (_, text_h) = measure_text(font, prefix);
        let text_y = row_y + (row_h - text_h) / 2.0;
        draw_text(
            ctx,
            font,
            prefix,
            cursor_x,
            text_y,
            color_to_cg(decoration_fg),
        );
        cursor_x += pw;

        let detail_info = item.detail.as_ref().map(|d| {
            let detail_text: String = d.spans.iter().map(|s| s.text.as_str()).collect();
            let (dw, _) = measure_text(font, &detail_text);
            (detail_text, dw)
        });
        let detail_reserve = detail_info.as_ref().map(|(_, dw)| *dw + 8.0).unwrap_or(0.0);
        // text_right_limit is in absolute pixel coords; the h_scroll shift of
        // cursor_x does not affect where the detail reserve boundary sits.
        let text_right_limit = row_x + row_w - detail_reserve - 4.0;

        for span in &item.text.spans {
            if cursor_x >= text_right_limit {
                break;
            }
            let span_fg = span.fg.unwrap_or(decoration_fg);
            if let Some(sbg) = span.bg {
                let (sw, _) = measure_text(font, &span.text);
                fill_rect(
                    ctx,
                    cursor_x,
                    row_y,
                    sw.min(text_right_limit - cursor_x),
                    row_h,
                    sbg,
                );
            }
            let (sw, _) = measure_text(font, &span.text);
            draw_text(
                ctx,
                font,
                &span.text,
                cursor_x,
                text_y,
                color_to_cg(span_fg),
            );
            cursor_x += sw;
        }

        // Detail text is pinned to the visible viewport (does not scroll with
        // h_scroll).  Restore the per-row clip before rendering it so the detail
        // slot is unaffected by the h_scroll shift.
        CGContextRestoreGState(ctx);

        if let Some((detail_text, dw)) = detail_info {
            let dx = row_x + row_w - dw - 4.0;
            if dx > cursor_x {
                draw_text(
                    ctx,
                    font,
                    &detail_text,
                    dx,
                    text_y,
                    color_to_cg(theme.muted_fg),
                );
            }
        }
    }

    // ── Horizontal scrollbar ─────────────────────────────────────────────
    // Painted after items so it overlays the bottom row's background fill.
    if needs_hscrollbar {
        let content_px = list.max_content_width.unwrap_or(0) as f64 * char_w;
        let track_y = y + h - line_height;
        let hsb_track =
            crate::event::Rect::new(x as f32, track_y as f32, w as f32, line_height as f32);
        let hsb = crate::primitives::scrollbar::Scrollbar::horizontal(
            list.id.clone(),
            hsb_track,
            list.h_scroll as f32 * char_w as f32,
            content_px as f32,
            w as f32,
            line_height as f32,
        );
        super::draw_scrollbar(ctx, &hsb, theme);
    }

    CGContextRestoreGState(ctx);
    layout
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
    CGContextFillRect(ctx, CGRect::new_xywh(x, y, w, h));
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
    use crate::event::{Rect as QRect, Viewport};
    use crate::primitives::list::{ListItem, ListViewHit};
    use crate::types::{Color, StyledSpan, StyledText, WidgetId};
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
}
