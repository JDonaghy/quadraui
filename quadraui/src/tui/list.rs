//! TUI rasteriser for [`crate::ListView`].
//!
//! Per D6: this function asks the primitive for a [`crate::ListViewLayout`]
//! (one cell per item; title row 1 cell when present) and paints the
//! resolved positions verbatim. Apps that need their own measurer
//! (variable-height items, e.g.) can compute the layout externally —
//! this rasteriser computes it inline because TUI list rows are
//! always uniform 1 cell tall.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color as RatatuiColor;

use super::{draw_styled_text, ratatui_color, set_cell};
use crate::primitives::list::{ListItemMeasure, ListView};
use crate::theme::Theme;
use crate::types::{Decoration, StyledText};

/// Draw a [`ListView`] into `area` on `buf`. Honours
/// [`ListView::bordered`] (rounded box border + title overlay) and
/// [`ListView::has_focus`] (the selected row only highlights when the
/// list has focus).
///
/// # Visual contract
///
/// - **Bordered:** filled with [`Theme::surface_bg`], rounded
///   `╭─╮ │ ╰─╯` glyphs in [`Theme::border_fg`], optional title
///   centred-ish on the top border in [`Theme::title_fg`].
/// - **Non-bordered with title:** the title row is painted as a flat
///   [`Theme::header_bg`] / [`Theme::header_fg`] strip.
/// - **Selected row:** [`Theme::selected_bg`] background and a `▶`
///   selection prefix in the row's foreground.
/// - **Per-item decoration → fg:** `Error → error_fg`, `Warning →
///   warning_fg`, `Muted → muted_fg`, others → [`Theme::surface_fg`].
/// - **Detail span:** right-aligned in [`Theme::muted_fg`], skipped
///   when there isn't room past the main text.
///
/// `nerd_fonts_enabled` controls which icon variant gets painted —
/// pass `crate::icons::nerd_fonts_enabled()` from the consumer's icon
/// registry, or `false` to always use ASCII fallbacks.
pub fn draw_list(
    buf: &mut Buffer,
    area: Rect,
    list: &ListView,
    theme: &Theme,
    nerd_fonts_enabled: bool,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let hdr_fg = ratatui_color(theme.header_fg);
    let hdr_bg = ratatui_color(theme.header_bg);
    let fg = ratatui_color(theme.surface_fg);
    let sel_bg = ratatui_color(theme.selected_bg);
    let row_bg = if list.bordered {
        ratatui_color(theme.surface_bg)
    } else {
        ratatui_color(theme.background)
    };
    let dim_fg = ratatui_color(theme.muted_fg);
    let error_fg = ratatui_color(theme.error_fg);
    let warn_fg = ratatui_color(theme.warning_fg);
    let border_fg = ratatui_color(theme.border_fg);
    let title_fg = ratatui_color(theme.title_fg);
    let h_scroll = list.h_scroll;

    // Visible item-content width (accounts for bordered inset).
    let inner_w: usize = if list.bordered {
        (area.width as usize).saturating_sub(2)
    } else {
        area.width as usize
    };

    // Reserve the bottom row for a horizontal scrollbar when content
    // overflows. In bordered mode the scrollbar sits inside the box
    // (above the bottom border); in flat mode it occupies the last row.
    let needs_hscrollbar = list
        .max_content_width
        .map(|mcw| mcw > inner_w)
        .unwrap_or(false);

    let viewport_h = if needs_hscrollbar {
        (area.height as f32 - 1.0).max(0.0)
    } else {
        area.height as f32
    };

    let title_h = if list.title.is_some() { 1.0 } else { 0.0 };
    let layout = list.layout(area.width as f32, viewport_h, title_h, |_| {
        ListItemMeasure::new(1.0)
    });

    if list.bordered {
        let top_y = area.y;
        for col in 0..area.width {
            let cx = area.x + col;
            let ch = if col == 0 {
                '╭'
            } else if col + 1 == area.width {
                '╮'
            } else {
                '─'
            };
            set_cell(buf, cx, top_y, ch, border_fg, row_bg);
        }
        if let Some(ref title) = list.title {
            let title_text: String = title.spans.iter().map(|s| s.text.as_str()).collect();
            let label = format!(" {} ", title_text.trim());
            for (i, ch) in label.chars().enumerate() {
                let cx = area.x + 2 + i as u16;
                if cx + 1 >= area.x + area.width {
                    break;
                }
                set_cell(buf, cx, top_y, ch, title_fg, row_bg);
            }
        }
        if area.height >= 2 {
            let bot_y = area.y + area.height - 1;
            for col in 0..area.width {
                let cx = area.x + col;
                let ch = if col == 0 {
                    '╰'
                } else if col + 1 == area.width {
                    '╯'
                } else {
                    '─'
                };
                set_cell(buf, cx, bot_y, ch, border_fg, row_bg);
            }
        }
        for row in (area.y + 1)..(area.y + area.height - 1) {
            set_cell(buf, area.x, row, '│', border_fg, row_bg);
            set_cell(buf, area.x + area.width - 1, row, '│', border_fg, row_bg);
            for col in 1..(area.width - 1) {
                set_cell(buf, area.x + col, row, ' ', fg, row_bg);
            }
        }
    } else if let Some(title_bounds) = layout.title_bounds {
        if let Some(ref title) = list.title {
            let y = area.y + title_bounds.y.round() as u16;
            for x in area.x..area.x + area.width {
                set_cell(buf, x, y, ' ', hdr_fg, hdr_bg);
            }
            draw_styled_text(
                buf,
                area,
                y,
                1,
                title,
                hdr_fg,
                hdr_bg,
                Decoration::Normal,
                dim_fg,
            );
        }
    }

    for visible_item in &layout.visible_items {
        let item = &list.items[visible_item.item_idx];
        let y = area.y + visible_item.bounds.y.round() as u16;
        let item_x = area.x + visible_item.bounds.x.round() as u16;
        let item_w = visible_item.bounds.width.round() as u16;
        let item_area = Rect {
            x: item_x,
            y,
            width: item_w,
            height: 1,
        };
        let is_selected = visible_item.item_idx == list.selected_idx && list.has_focus;
        let bg = if is_selected { sel_bg } else { row_bg };
        let decoration_fg = match item.decoration {
            Decoration::Error => error_fg,
            Decoration::Warning => warn_fg,
            Decoration::Muted => dim_fg,
            _ => fg,
        };

        // Fill the entire visible row with the background colour first.
        for x in item_x..item_x + item_w {
            set_cell(buf, x, y, ' ', decoration_fg, bg);
        }

        // `vcol` is the virtual column index within the full item content
        // (prefix + icon + text). Characters at vcol < h_scroll are skipped;
        // at vcol >= h_scroll they map to buf col (vcol - h_scroll).
        let mut vcol: usize = 0;

        // Selection prefix "▶ " (selected) or "  " (not selected).
        let prefix = if is_selected { "▶ " } else { "  " };
        for ch in prefix.chars() {
            vcol = put_char_scrolled(
                buf,
                vcol,
                h_scroll,
                item_x,
                item_w,
                y,
                ch,
                decoration_fg,
                bg,
            );
        }

        // Optional icon glyph + trailing space.
        if let Some(ref icon) = item.icon {
            let glyph = if nerd_fonts_enabled {
                icon.glyph.as_str()
            } else {
                icon.fallback.as_str()
            };
            for ch in glyph.chars() {
                vcol = put_char_scrolled(
                    buf,
                    vcol,
                    h_scroll,
                    item_x,
                    item_w,
                    y,
                    ch,
                    decoration_fg,
                    bg,
                );
            }
            vcol = put_char_scrolled(
                buf,
                vcol,
                h_scroll,
                item_x,
                item_w,
                y,
                ' ',
                decoration_fg,
                bg,
            );
        }

        // Main item text — rendered with h_scroll awareness.
        let vcol_after_text = render_spans_scrolled(
            buf,
            vcol,
            h_scroll,
            item_x,
            item_w,
            y,
            &item.text,
            decoration_fg,
            bg,
            item.decoration,
            dim_fg,
        );

        // Buffer column past the last text character (clamped to item_w).
        // Used to ensure detail text doesn't overwrite main text.
        let text_buf_col_end = if vcol_after_text > h_scroll {
            (vcol_after_text - h_scroll).min(item_w as usize)
        } else {
            0
        };

        // Detail text — right-aligned and pinned to the visible viewport
        // (does not scroll with h_scroll, so it remains readable at all offsets).
        if let Some(ref detail) = item.detail {
            let detail_w: usize = detail.spans.iter().map(|s| s.text.chars().count()).sum();
            let start = (item_w as usize).saturating_sub(detail_w + 1);
            if start > text_buf_col_end + 1 {
                draw_styled_text(
                    buf,
                    item_area,
                    y,
                    start,
                    detail,
                    dim_fg,
                    bg,
                    Decoration::Muted,
                    dim_fg,
                );
            }
        }
    }

    // ── Horizontal scrollbar ──────────────────────────────────────────────
    // Drawn after items so it overlays the bottom row's background fill.
    // In bordered mode it sits inside the box (above bottom border);
    // in flat mode it occupies the final row of the area.
    //
    // Hit-test / zone wiring: DataTable wires its h-scrollbar via
    // `FrameZone`; ListView callers own state and handle Left/Right keys
    // themselves. Follow the DataTable `FrameZone` pattern if you need
    // mouse-drag on this scrollbar.
    if needs_hscrollbar {
        let mcw = list.max_content_width.unwrap_or(0) as f32;
        let visible_w = inner_w as f32;
        let (hsb_y, track_x, track_w, hsb_bg) = if list.bordered {
            // Inside the box, one row above the bottom border.
            (
                area.y + area.height.saturating_sub(2),
                area.x + 1,
                (area.width as usize).saturating_sub(2) as f32,
                theme.surface_bg,
            )
        } else {
            (
                area.y + area.height - 1,
                area.x,
                area.width as f32,
                theme.background,
            )
        };
        let hsb_track = crate::event::Rect::new(track_x as f32, hsb_y as f32, track_w, 1.0);
        let hsb = crate::primitives::scrollbar::Scrollbar::horizontal(
            list.id.clone(),
            hsb_track,
            h_scroll as f32,
            mcw,
            visible_w,
            1.0,
        );
        super::draw_scrollbar(buf, &hsb, theme, hsb_bg);
    }
}

// ── Private rendering helpers ─────────────────────────────────────────────

/// Write a single character at virtual column `vcol` within a horizontally-
/// scrolled item row. Characters whose `vcol < h_scroll` are silently
/// dropped; the rest are mapped to buffer column `vcol - h_scroll` inside
/// the item's `[item_x, item_x + item_w)` band.
///
/// Returns `vcol + 1` so callers can do `vcol = put_char_scrolled(...)`.
#[allow(clippy::too_many_arguments)]
fn put_char_scrolled(
    buf: &mut Buffer,
    vcol: usize,
    h_scroll: usize,
    item_x: u16,
    item_w: u16,
    y: u16,
    ch: char,
    fg: RatatuiColor,
    bg: RatatuiColor,
) -> usize {
    if vcol >= h_scroll {
        let buf_col = (vcol - h_scroll) as u16;
        if buf_col < item_w {
            set_cell(buf, item_x + buf_col, y, ch, fg, bg);
        }
    }
    vcol + 1
}

/// Render a [`StyledText`]'s spans starting at virtual column `vcol`,
/// skipping the first `h_scroll` virtual columns. Returns the virtual column
/// past the last character of `text` (i.e. the next `vcol` for subsequent
/// content in the same item row).
///
/// Span foreground / background and `decoration` semantics mirror
/// [`super::draw_styled_text`].
#[allow(clippy::too_many_arguments)]
fn render_spans_scrolled(
    buf: &mut Buffer,
    vcol_start: usize,
    h_scroll: usize,
    item_x: u16,
    item_w: u16,
    y: u16,
    text: &StyledText,
    default_fg: RatatuiColor,
    bg: RatatuiColor,
    decoration: Decoration,
    dim_fg: RatatuiColor,
) -> usize {
    let mut vcol = vcol_start;
    for span in &text.spans {
        let span_fg = if let Some(c) = span.fg {
            ratatui_color(c)
        } else if matches!(decoration, Decoration::Muted) {
            dim_fg
        } else {
            default_fg
        };
        let span_bg = span.bg.map(ratatui_color).unwrap_or(bg);
        for ch in span.text.chars() {
            vcol = put_char_scrolled(buf, vcol, h_scroll, item_x, item_w, y, ch, span_fg, span_bg);
        }
    }
    vcol
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::list::{ListItem, ListView};
    use crate::types::{Color, StyledSpan, StyledText, WidgetId};

    fn item(text: &str, dec: Decoration) -> ListItem {
        ListItem {
            text: StyledText {
                spans: vec![StyledSpan::plain(text)],
            },
            detail: None,
            icon: None,
            decoration: dec,
        }
    }

    fn make_list(selected: usize) -> ListView {
        ListView {
            id: WidgetId::new("list"),
            title: None,
            items: vec![
                item("alpha", Decoration::Normal),
                item("beta", Decoration::Normal),
                item("gamma", Decoration::Normal),
            ],
            selected_idx: selected,
            scroll_offset: 0,
            has_focus: true,
            bordered: false,
            h_scroll: 0,
            max_content_width: None,
        }
    }

    fn cell_char(buf: &Buffer, x: u16, y: u16) -> char {
        buf[(x, y)].symbol().chars().next().unwrap_or(' ')
    }

    #[test]
    fn paints_three_items_with_selection_marker() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 5));
        let list = make_list(1);
        draw_list(
            &mut buf,
            Rect::new(0, 0, 20, 5),
            &list,
            &Theme::default(),
            false,
        );

        // Selection marker '▶' is on row 1 (the second item).
        assert_eq!(cell_char(&buf, 0, 1), '▶');
        // First and third rows show ' ' selection placeholder.
        assert_eq!(cell_char(&buf, 0, 0), ' ');
        assert_eq!(cell_char(&buf, 0, 2), ' ');
    }

    #[test]
    fn no_selection_marker_when_unfocused() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 5));
        let mut list = make_list(1);
        list.has_focus = false;
        draw_list(
            &mut buf,
            Rect::new(0, 0, 20, 5),
            &list,
            &Theme::default(),
            false,
        );
        // Row 1 should NOT have the '▶' marker.
        for y in 0..3 {
            assert_ne!(cell_char(&buf, 0, y), '▶');
        }
    }

    #[test]
    fn bordered_paints_corner_glyphs() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 5));
        let mut list = make_list(0);
        list.bordered = true;
        list.title = Some(StyledText {
            spans: vec![StyledSpan::plain("Picker")],
        });
        draw_list(
            &mut buf,
            Rect::new(0, 0, 10, 5),
            &list,
            &Theme::default(),
            false,
        );

        assert_eq!(cell_char(&buf, 0, 0), '╭');
        assert_eq!(cell_char(&buf, 9, 0), '╮');
        assert_eq!(cell_char(&buf, 0, 4), '╰');
        assert_eq!(cell_char(&buf, 9, 4), '╯');
    }

    #[test]
    fn decoration_error_uses_error_fg() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 3));
        let list = ListView {
            id: WidgetId::new("list"),
            title: None,
            items: vec![item("oops", Decoration::Error)],
            selected_idx: 0,
            scroll_offset: 0,
            has_focus: false,
            bordered: false,
            h_scroll: 0,
            max_content_width: None,
        };
        let theme = Theme {
            error_fg: Color::rgb(255, 0, 0),
            ..Theme::default()
        };
        draw_list(&mut buf, Rect::new(0, 0, 20, 3), &list, &theme, false);
        // The 'o' of "oops" should be drawn in error_fg.
        // Selection marker is ' ' (no focus); icon prefix runs cols 0..2;
        // text starts at col 2.
        assert_eq!(cell_char(&buf, 2, 0), 'o');
        let fg = buf[(2u16, 0u16)].fg;
        assert_eq!(fg, ratatui::style::Color::Rgb(255, 0, 0));
    }

    #[test]
    fn zero_size_is_a_no_op() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 5));
        let list = make_list(0);
        draw_list(
            &mut buf,
            Rect::new(0, 0, 0, 5),
            &list,
            &Theme::default(),
            false,
        );
        assert_eq!(cell_char(&buf, 0, 0), ' ');
    }

    // ── h_scroll tests ───────────────────────────────────────────────────────

    #[test]
    fn hscroll_zero_renders_identically_to_default() {
        // h_scroll=0 with max_content_width=None must produce the same output
        // as the original no-scroll rasteriser.
        let mut buf_ref = Buffer::empty(Rect::new(0, 0, 20, 3));
        let mut buf_new = Buffer::empty(Rect::new(0, 0, 20, 3));

        let list_ref = make_list(0); // h_scroll=0, max_content_width=None by construction
        let list_new = ListView {
            h_scroll: 0,
            max_content_width: None,
            ..make_list(0)
        };

        draw_list(
            &mut buf_ref,
            Rect::new(0, 0, 20, 3),
            &list_ref,
            &Theme::default(),
            false,
        );
        draw_list(
            &mut buf_new,
            Rect::new(0, 0, 20, 3),
            &list_new,
            &Theme::default(),
            false,
        );

        assert_eq!(buf_ref, buf_new, "h_scroll=0 must produce identical output");
        // Sanity: selected row 0 still has '▶' at col 0, 'a' of "alpha" at col 2.
        assert_eq!(cell_char(&buf_new, 0, 0), '▶');
        assert_eq!(cell_char(&buf_new, 2, 0), 'a');
    }

    #[test]
    fn hscroll_skips_prefix_and_reveals_text_at_col_zero() {
        // With h_scroll=2 the two-char "  " prefix of a non-selected item
        // is entirely scrolled off; the first char of the item text appears
        // at buffer column 0.
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 3));
        let mut list = make_list(1); // item 0 is NOT selected, no focus marker
        list.h_scroll = 2;
        draw_list(
            &mut buf,
            Rect::new(0, 0, 20, 3),
            &list,
            &Theme::default(),
            false,
        );

        // Row 0: item "alpha" — "  " prefix (vcol 0,1) scrolled off → 'a' at buf col 0.
        assert_eq!(
            cell_char(&buf, 0, 0),
            'a',
            "first text char at buf col 0 after skipping prefix"
        );
        // Row 1: item "beta" is selected (selected_idx=1), prefix '▶'(vcol 0) is
        // scrolled off, ' '(vcol 1) is scrolled off → 'b' of "beta" at buf col 0.
        assert_eq!(
            cell_char(&buf, 0, 1),
            'b',
            "selected item text visible at buf col 0"
        );
    }

    #[test]
    fn hscroll_partial_prefix_visible() {
        // h_scroll=1: first prefix char scrolled off, second (space) appears at col 0.
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 3));
        let mut list = make_list(1); // row 0 not selected
        list.h_scroll = 1;
        draw_list(
            &mut buf,
            Rect::new(0, 0, 20, 3),
            &list,
            &Theme::default(),
            false,
        );

        // Row 0 non-selected: prefix "  " → vcol 0 skipped, vcol 1 (' ') at buf col 0.
        assert_eq!(cell_char(&buf, 0, 0), ' ');
        // 'a' of "alpha" at buf col 1.
        assert_eq!(cell_char(&buf, 1, 0), 'a');
    }

    #[test]
    fn hscrollbar_visible_when_content_wider_than_area() {
        // max_content_width=20 with area width=10 → scrollbar appears on last row.
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 5));
        let mut list = make_list(0);
        list.max_content_width = Some(20);
        draw_list(
            &mut buf,
            Rect::new(0, 0, 10, 5),
            &list,
            &Theme::default(),
            false,
        );

        // Bottom row (y=4) must contain at least one scrollbar glyph.
        let has_sb = (0..10u16).any(|x| matches!(cell_char(&buf, x, 4), '▁' | '▄'));
        assert!(
            has_sb,
            "horizontal scrollbar expected on last row when content overflows"
        );
        // Items still appear: row 0 should have the selection marker.
        assert_eq!(cell_char(&buf, 0, 0), '▶');
    }

    #[test]
    fn hscrollbar_hidden_when_content_fits() {
        // max_content_width=5 with area width=20 → no scrollbar.
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 5));
        let mut list = make_list(0);
        list.max_content_width = Some(5);
        draw_list(
            &mut buf,
            Rect::new(0, 0, 20, 5),
            &list,
            &Theme::default(),
            false,
        );

        let has_sb = (0..20u16).any(|x| matches!(cell_char(&buf, x, 4), '▁' | '▄'));
        assert!(
            !has_sb,
            "no horizontal scrollbar expected when content fits in viewport"
        );
    }

    #[test]
    fn hscrollbar_inside_bordered_box() {
        // In bordered mode the scrollbar sits at area.height-2 (above bottom border).
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 6));
        let mut list = make_list(0);
        list.bordered = true;
        list.max_content_width = Some(30);
        draw_list(
            &mut buf,
            Rect::new(0, 0, 10, 6),
            &list,
            &Theme::default(),
            false,
        );

        // Bottom border corners still intact.
        assert_eq!(cell_char(&buf, 0, 5), '╰');
        assert_eq!(cell_char(&buf, 9, 5), '╯');
        // Scrollbar at y=4 in inner columns (x=1..9).
        let has_sb = (1..9u16).any(|x| matches!(cell_char(&buf, x, 4), '▁' | '▄'));
        assert!(
            has_sb,
            "horizontal scrollbar expected at y=4 inside bordered box"
        );
    }

    #[test]
    fn hscrollbar_max_content_none_never_shows_scrollbar() {
        // max_content_width=None → never show scrollbar regardless of actual content.
        let mut buf = Buffer::empty(Rect::new(0, 0, 5, 3));
        let mut list = make_list(0);
        list.max_content_width = None;
        // Item "alpha" (5 chars + 2 prefix = 7) would overflow the 5-wide area,
        // but without max_content_width set the rasteriser can't know that.
        draw_list(
            &mut buf,
            Rect::new(0, 0, 5, 3),
            &list,
            &Theme::default(),
            false,
        );

        let has_sb = (0..5u16).any(|x| matches!(cell_char(&buf, x, 2), '▁' | '▄'));
        assert!(!has_sb, "no scrollbar when max_content_width is None");
    }
}
