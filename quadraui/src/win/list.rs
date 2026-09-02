//! Direct2D / DirectWrite rasteriser for [`crate::ListView`] (issue #26).
//!
//! Mirrors `gtk::list`'s structure: [`ListView::layout`] (the D6 layout
//! API) resolves title/item positions; this module measures (via
//! DirectWrite) and paints (via [`super::text::fill_rect`] +
//! [`DWrite::draw_text`]). Paint and hit-test both derive from one
//! [`win_list_layout`] call.
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

use windows::Win32::Graphics::Direct2D::ID2D1RenderTarget;

use super::text::{fill_rect, pop_clip, push_clip, DWrite};
use crate::event::Rect;
use crate::primitives::list::{ListItemMeasure, ListView, ListViewLayout};
use crate::theme::Theme;
use crate::types::Decoration;

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
pub fn draw_list(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    rect: Rect,
    list: &ListView,
    line_height: f32,
) -> ListViewLayout {
    let theme = Theme::default();
    let base_bg = if list.bordered {
        theme.surface_bg
    } else {
        theme.background
    };
    let _ = fill_rect(target, rect, base_bg);

    let layout = win_list_layout(list, rect, line_height);
    let char_w = dwrite
        .measure_text("M")
        .map(|(w, _)| w)
        .unwrap_or(1.0)
        .max(1.0);
    let h_off_px = list.h_scroll as f32 * char_w;

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

    if let Some(title_bounds) = layout.title_bounds {
        let title = list.title.as_ref().expect("title_bounds implies title");
        let title_text: String = title.spans.iter().map(|s| s.text.as_str()).collect();
        let tb = Rect::new(
            rect.x + title_bounds.x,
            rect.y + title_bounds.y,
            title_bounds.width,
            title_bounds.height,
        );
        if list.bordered {
            let (tw, th) = dwrite.measure_text(&title_text).unwrap_or((0.0, 0.0));
            let label_rect = Rect::new(tb.x + 8.0, tb.y + (tb.height - th) / 2.0, tw, th);
            let _ = fill_rect(
                target,
                Rect::new(label_rect.x - 2.0, tb.y, tw + 4.0, tb.height),
                base_bg,
            );
            let _ = dwrite.draw_text(target, &title_text, label_rect, theme.title_fg);
        } else {
            let _ = fill_rect(target, tb, theme.header_bg);
            let (_, th) = dwrite.measure_text(&title_text).unwrap_or((0.0, 0.0));
            let label_rect = Rect::new(tb.x + 2.0, tb.y + (tb.height - th) / 2.0, tb.width, th);
            let _ = dwrite.draw_text(target, &title_text, label_rect, theme.header_fg);
        }
    }

    let border_inset = if list.bordered { BORDER_DIP } else { 0.0 };
    let item_x_offset = rect.x + border_inset;

    for vis_item in &layout.visible_items {
        let item = &list.items[vis_item.item_idx];
        let row_rect = Rect::new(
            item_x_offset,
            rect.y + vis_item.bounds.y,
            vis_item.bounds.width,
            vis_item.bounds.height,
        );

        let is_selected = vis_item.item_idx == list.selected_idx && list.has_focus;
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
            base_bg
        };
        let _ = fill_rect(target, row_rect, row_bg);

        push_clip(target, row_rect);
        let mut cursor_x = row_rect.x + 2.0 - h_off_px;

        if let Some(ref icon) = item.icon {
            let glyph = icon.fallback.as_str();
            let (iw, ih) = dwrite.measure_text(glyph).unwrap_or((0.0, 0.0));
            let iy = row_rect.y + (row_rect.height - ih) / 2.0;
            let _ = dwrite.draw_text(
                target,
                glyph,
                Rect::new(cursor_x, iy, iw, ih),
                decoration_fg,
            );
            cursor_x += iw + 6.0;
        }

        let detail_info = item.detail.as_ref().map(|detail| {
            let text: String = detail.spans.iter().map(|s| s.text.as_str()).collect();
            let (dw, _) = dwrite.measure_text(&text).unwrap_or((0.0, 0.0));
            (text, dw)
        });
        let detail_reserve = detail_info.as_ref().map(|(_, dw)| *dw + 8.0).unwrap_or(0.0);
        let text_right_limit = row_rect.x + row_rect.width - detail_reserve - 4.0;

        for span in &item.text.spans {
            if cursor_x >= text_right_limit {
                break;
            }
            let span_fg = span.fg.unwrap_or(decoration_fg);
            let (sw, sh) = dwrite
                .measure_text_styled(&span.text, span.bold)
                .unwrap_or((0.0, 0.0));
            let sy = row_rect.y + (row_rect.height - sh) / 2.0;
            let _ = dwrite.draw_text_styled(
                target,
                &span.text,
                Rect::new(cursor_x, sy, sw, sh),
                span_fg,
                span.bold,
            );
            cursor_x += sw;
        }

        if let Some((dtext, dw)) = detail_info {
            let dx = row_rect.x + row_rect.width - dw - 4.0;
            if dx > cursor_x {
                let (_, dh) = dwrite.measure_text(&dtext).unwrap_or((0.0, 0.0));
                let dy = row_rect.y + (row_rect.height - dh) / 2.0;
                let _ = dwrite.draw_text(target, &dtext, Rect::new(dx, dy, dw, dh), theme.muted_fg);
            }
        }
        pop_clip(target);
    }

    layout
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::list::{ListItem, ListViewHit};
    use crate::types::{StyledText, WidgetId};
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

    /// Paint↔click round trip: the selected row's background must be
    /// painted at its own bounds, and clicking each visible row's
    /// centre must hit_test back to that row.
    #[test]
    fn paint_and_hit_test_round_trip() {
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
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
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
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
}
