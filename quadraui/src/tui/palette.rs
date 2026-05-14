//! TUI rasteriser for [`crate::Palette`].
//!
//! Modal-style fuzzy picker with a title bar, query-input row, and a
//! scrollable result list. Renders in cell-art glyphs:
//!
//! ```text
//! ╭ Title  N/M ──╮
//! │ > query      │
//! ├──────────────┤
//! │  Item 1       │
//! │  Item 2       │
//! │  Item 3 detail│
//! ╰───────────────╯
//! ```
//!
//! Per-item `match_positions` (byte offsets) get highlighted with
//! [`Theme::match_fg`] for fuzzy-search emphasis.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::{ratatui_color, set_cell};
use crate::primitives::palette::Palette;
use crate::theme::Theme;

/// Draw a [`Palette`] modal into `area` on `buf`.
///
/// `nerd_fonts_enabled` selects between item icons' Nerd-Font glyph
/// and ASCII fallback. Caller is responsible for sizing / centring
/// the popup within the editor area.
pub fn draw_palette(
    buf: &mut Buffer,
    area: Rect,
    palette: &Palette,
    theme: &Theme,
    nerd_fonts_enabled: bool,
) {
    if area.width < 4 || area.height < 4 {
        return;
    }

    let bg = ratatui_color(theme.surface_bg);
    let fg = ratatui_color(theme.surface_fg);
    let query_fg = ratatui_color(theme.query_fg);
    let border_fg = ratatui_color(theme.border_fg);
    let title_fg = ratatui_color(theme.title_fg);
    let match_fg = ratatui_color(theme.match_fg);
    let sel_bg = ratatui_color(theme.selected_bg);
    let dim_fg = ratatui_color(theme.muted_fg);

    let x0 = area.x;
    let y0 = area.y;
    let w = area.width;
    let h = area.height;
    let y_end = y0 + h;

    // Clear popup so cycling between pickers with different content
    // lengths doesn't leave stale characters behind.
    for y in y0..y_end {
        for x in x0..x0 + w {
            set_cell(buf, x, y, ' ', fg, bg);
        }
    }

    // Top border with title overlay.
    for col in 0..w {
        let ch = if col == 0 {
            '╭'
        } else if col == w - 1 {
            '╮'
        } else {
            '─'
        };
        set_cell(buf, x0 + col, y0, ch, border_fg, bg);
    }
    let title_text = if palette.total_count > 0 {
        format!(
            " {}  {}/{} ",
            palette.title,
            palette.items.len(),
            palette.total_count
        )
    } else {
        format!(" {} ", palette.title)
    };
    for (i, ch) in title_text.chars().enumerate() {
        let col = 2 + i as u16;
        if col + 1 >= w {
            break;
        }
        set_cell(buf, x0 + col, y0, ch, title_fg, bg);
    }

    // Preview pane splits the popup horizontally when present.
    let has_preview = palette.preview.is_some();
    let list_w = if has_preview {
        ((w as f32) * 0.4).round() as u16
    } else {
        w
    };
    let preview_x0 = x0 + list_w;

    // Query line + separator (skipped when show_query is false).
    let items_row0 = if palette.show_query {
        if h >= 3 {
            let row = y0 + 1;
            set_cell(buf, x0, row, '│', border_fg, bg);
            if w >= 2 {
                set_cell(buf, x0 + w - 1, row, '│', border_fg, bg);
            }
            let prompt = "> ";
            let mut col = 1u16;
            for ch in prompt.chars() {
                if col + 1 >= w {
                    break;
                }
                set_cell(buf, x0 + col, row, ch, query_fg, bg);
                col += 1;
            }
            let query_start = col;
            for ch in palette.query.chars() {
                if col + 1 >= w {
                    break;
                }
                set_cell(buf, x0 + col, row, ch, query_fg, bg);
                col += 1;
            }
            let mut byte = 0usize;
            let mut char_idx = 0usize;
            for ch in palette.query.chars() {
                if byte >= palette.query_cursor {
                    break;
                }
                byte += ch.len_utf8();
                char_idx += 1;
            }
            let cursor_col = query_start + char_idx as u16;
            if cursor_col + 1 < w {
                let ch = palette.query.chars().nth(char_idx).unwrap_or(' ');
                set_cell(buf, x0 + cursor_col, row, ch, bg, query_fg);
            }
        }

        if h >= 4 {
            let row = y0 + 2;
            for col in 0..w {
                let ch = if col == 0 {
                    '├'
                } else if col == w - 1 {
                    '┤'
                } else if has_preview && x0 + col == preview_x0 {
                    '┬'
                } else {
                    '─'
                };
                set_cell(buf, x0 + col, row, ch, border_fg, bg);
            }
        }

        y0 + 3
    } else {
        y0 + 1
    };

    // Result rows.
    let has_create = palette.create_label.is_some();
    let create_reserved = if has_create { 1u16 } else { 0 };
    let items_row_end = (y_end - 1).saturating_sub(create_reserved);
    let visible_rows = items_row_end.saturating_sub(items_row0) as usize;
    let total = palette.items.len();
    let has_scrollbar = total > visible_rows;
    let item_end_col = if has_scrollbar {
        list_w - 1
    } else if has_preview {
        list_w
    } else {
        w - 1
    };

    // Clamp scroll_offset so the selected item is always visible AND
    // the visible window stays full when there are enough items to
    // fill it. The engine updates scroll_top with a conservative
    // heuristic that doesn't know the actual renderer row count, so
    // the renderer is authoritative here.
    let max_offset = total.saturating_sub(visible_rows);
    let effective_offset = if visible_rows == 0 {
        0
    } else if palette.selected_idx < palette.scroll_offset {
        palette.selected_idx
    } else if palette.selected_idx >= palette.scroll_offset + visible_rows {
        palette.selected_idx + 1 - visible_rows
    } else {
        palette.scroll_offset
    };
    // Don't leave empty rows when there are items above we could show.
    let effective_offset = effective_offset.min(max_offset);

    for (vis_i, item) in palette
        .items
        .iter()
        .enumerate()
        .skip(effective_offset)
        .take(visible_rows)
    {
        let row = items_row0 + (vis_i - effective_offset) as u16;
        if row >= items_row_end {
            break;
        }
        let is_selected = vis_i == palette.selected_idx && palette.has_focus;
        let row_bg = if is_selected { sel_bg } else { bg };

        set_cell(buf, x0, row, '│', border_fg, bg);
        if !has_preview && w >= 2 {
            set_cell(buf, x0 + w - 1, row, '│', border_fg, bg);
        }
        for col in 1..item_end_col {
            set_cell(buf, x0 + col, row, ' ', fg, row_bg);
        }

        let mut col = 1u16;

        // Selection prefix (▶ when focused, two spaces otherwise — keeps
        // non-selected text aligned with selected text).
        let prefix = if is_selected { "▶ " } else { "  " };
        for ch in prefix.chars() {
            if col >= item_end_col {
                break;
            }
            set_cell(buf, x0 + col, row, ch, fg, row_bg);
            col += 1;
        }

        // Icon.
        if let Some(ref icon) = item.icon {
            let glyph = if nerd_fonts_enabled {
                icon.glyph.as_str()
            } else {
                icon.fallback.as_str()
            };
            for ch in glyph.chars() {
                if col >= item_end_col {
                    break;
                }
                set_cell(buf, x0 + col, row, ch, fg, row_bg);
                col += 1;
            }
            if col < item_end_col {
                set_cell(buf, x0 + col, row, ' ', fg, row_bg);
                col += 1;
            }
        }

        // Text — per-character match highlighting based on byte offsets.
        let full_text: String = item.text.spans.iter().map(|s| s.text.as_str()).collect();
        let mut byte = 0usize;
        for ch in full_text.chars() {
            if col >= item_end_col {
                break;
            }
            let is_match = item.match_positions.contains(&byte);
            let ch_fg = if is_match { match_fg } else { fg };
            set_cell(buf, x0 + col, row, ch, ch_fg, row_bg);
            col += 1;
            byte += ch.len_utf8();
        }
        let text_end_col = col;

        // Detail (right-aligned, dimmed).
        if let Some(ref detail) = item.detail {
            let detail_text: String = detail.spans.iter().map(|s| s.text.as_str()).collect();
            let detail_w = detail_text.chars().count() as u16;
            if item_end_col > text_end_col + detail_w + 1 {
                let start = item_end_col.saturating_sub(detail_w + 1);
                let mut dcol = start;
                for ch in detail_text.chars() {
                    if dcol >= item_end_col {
                        break;
                    }
                    set_cell(buf, x0 + dcol, row, ch, dim_fg, row_bg);
                    dcol += 1;
                }
            }
        }

        // Scrollbar.
        if has_scrollbar {
            let sb_col = list_w - 1;
            let track_len = visible_rows;
            let thumb_len = (visible_rows * visible_rows / total.max(1)).max(1);
            let thumb_start = effective_offset * track_len / total.max(1);
            let vi_off = vis_i - effective_offset;
            let in_thumb = vi_off >= thumb_start && vi_off < thumb_start + thumb_len;
            let ch = if in_thumb { '█' } else { '░' };
            set_cell(buf, x0 + sb_col, row, ch, border_fg, bg);
        }
    }

    // Empty rows between last item and bottom border.
    let drawn = total.saturating_sub(effective_offset).min(visible_rows) as u16;
    for row in items_row0 + drawn..items_row_end {
        set_cell(buf, x0, row, '│', border_fg, bg);
        if !has_preview && w >= 2 {
            set_cell(buf, x0 + w - 1, row, '│', border_fg, bg);
        }
    }

    // ── Create action row (pinned below items) ────────────────────────
    if let Some(ref label) = palette.create_label {
        let create_row = items_row_end;
        if create_row < y_end - 1 {
            let accent_fg = ratatui_color(theme.accent_fg);
            set_cell(buf, x0, create_row, '│', border_fg, bg);
            if !has_preview && w >= 2 {
                set_cell(buf, x0 + w - 1, create_row, '│', border_fg, bg);
            }
            let end_col = if has_preview { list_w } else { w - 1 };
            for col in 1..end_col {
                set_cell(buf, x0 + col, create_row, ' ', fg, bg);
            }
            let prefix = "+ ";
            let mut col = 1u16;
            for ch in prefix.chars() {
                if col >= end_col {
                    break;
                }
                set_cell(buf, x0 + col, create_row, ch, accent_fg, bg);
                col += 1;
            }
            for ch in label.chars() {
                if col >= end_col {
                    break;
                }
                set_cell(buf, x0 + col, create_row, ch, accent_fg, bg);
                col += 1;
            }
        }
    }

    // ── Preview pane ─────────────────────────────────────────────────
    if let Some(ref preview) = palette.preview {
        let sep_col = preview_x0;
        let preview_content_x = sep_col + 1;
        let _preview_w = (x0 + w).saturating_sub(preview_content_x + 1);

        // Vertical separator + right border for all item/preview rows.
        for row in items_row0..items_row_end {
            set_cell(buf, sep_col, row, '│', border_fg, bg);
            set_cell(buf, x0 + w - 1, row, '│', border_fg, bg);
        }

        // Preview title row (reuses separator row area in the preview
        // half, or first content row if no title).
        let mut preview_row0 = items_row0;

        if let Some(ref title_text) = preview.title {
            let row = items_row0;
            if row < items_row_end {
                for col in preview_content_x..x0 + w - 1 {
                    set_cell(buf, col, row, ' ', fg, bg);
                }
                let mut col = preview_content_x + 1;
                for ch in title_text.chars() {
                    if col + 1 >= x0 + w - 1 {
                        break;
                    }
                    set_cell(buf, col, row, ch, dim_fg, bg);
                    col += 1;
                }
                preview_row0 = items_row0 + 1;
            }
        }

        // Preview content lines.
        let highlight_bg = ratatui_color(theme.selected_bg);
        let preview_visible = items_row_end.saturating_sub(preview_row0) as usize;
        for (vi, line_idx) in (preview.scroll_offset..).take(preview_visible).enumerate() {
            let row = preview_row0 + vi as u16;
            if row >= items_row_end {
                break;
            }
            let is_highlight = preview.highlight_line == Some(line_idx);
            let line_bg = if is_highlight { highlight_bg } else { bg };

            for col in preview_content_x..x0 + w - 1 {
                set_cell(buf, col, row, ' ', fg, line_bg);
            }

            if line_idx < preview.lines.len() {
                let line = &preview.lines[line_idx];
                let mut col = preview_content_x + 1;
                for span in &line.spans {
                    let span_fg = span.fg.map(ratatui_color).unwrap_or(fg);
                    for ch in span.text.chars() {
                        if col + 1 >= x0 + w - 1 {
                            break;
                        }
                        set_cell(buf, col, row, ch, span_fg, line_bg);
                        col += 1;
                    }
                }
            }
        }
    }

    // Bottom border.
    let row = y_end - 1;
    for col in 0..w {
        let ch = if col == 0 {
            '╰'
        } else if col == w - 1 {
            '╯'
        } else if has_preview && x0 + col == preview_x0 {
            '┴'
        } else {
            '─'
        };
        set_cell(buf, x0 + col, row, ch, border_fg, bg);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::palette::{Palette, PaletteItem};
    use crate::types::{Color, StyledSpan, StyledText, WidgetId};

    fn item(text: &str) -> PaletteItem {
        PaletteItem {
            text: StyledText {
                spans: vec![StyledSpan::plain(text)],
            },
            detail: None,
            icon: None,
            match_positions: Vec::new(),
            depth: 0,
            expandable: false,
            expanded: false,
        }
    }

    fn make_palette() -> Palette {
        Palette {
            id: WidgetId::new("p"),
            title: "Search".into(),
            query: "fo".into(),
            query_cursor: 2,
            items: vec![item("foo"), item("food"), item("foggy")],
            selected_idx: 0,
            scroll_offset: 0,
            has_focus: true,
            show_query: true,
            create_label: None,
            total_count: 3,
            preview: None,
        }
    }

    fn cell_char(buf: &Buffer, x: u16, y: u16) -> char {
        buf[(x, y)].symbol().chars().next().unwrap_or(' ')
    }

    #[test]
    fn paints_top_and_bottom_borders_with_corners() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 10));
        let p = make_palette();
        draw_palette(
            &mut buf,
            Rect::new(0, 0, 20, 10),
            &p,
            &Theme::default(),
            false,
        );
        assert_eq!(cell_char(&buf, 0, 0), '╭');
        assert_eq!(cell_char(&buf, 19, 0), '╮');
        assert_eq!(cell_char(&buf, 0, 9), '╰');
        assert_eq!(cell_char(&buf, 19, 9), '╯');
    }

    #[test]
    fn paints_query_with_prompt() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 10));
        let p = make_palette();
        draw_palette(
            &mut buf,
            Rect::new(0, 0, 20, 10),
            &p,
            &Theme::default(),
            false,
        );
        // Query row is y=1; prompt "> " starts at col 1, query "fo" follows.
        let row1: String = (1..6).map(|x| cell_char(&buf, x, 1)).collect();
        assert_eq!(row1, "> fo ");
    }

    #[test]
    fn match_positions_use_match_fg() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 10));
        let mut p = make_palette();
        // Highlight bytes 0 and 2 of "foo".
        p.items[0].match_positions = vec![0, 2];
        let theme = Theme {
            match_fg: Color::rgb(99, 99, 99),
            ..Theme::default()
        };
        draw_palette(&mut buf, Rect::new(0, 0, 20, 10), &p, &theme, false);
        // Items start at row 3. Scan the row's cells for the first 'f' to
        // get the column index in cells (NOT byte offsets — `▶` takes 1
        // cell but 3 bytes when collected into a String).
        let f_col = (0..20)
            .find(|&x| buf[(x, 3u16)].symbol().starts_with('f'))
            .expect("expected 'f' painted in row 3");
        let fg0 = buf[(f_col, 3u16)].fg;
        assert_eq!(fg0, ratatui::style::Color::Rgb(99, 99, 99));
        // 'o' at byte 2 of "foo" → second 'o' → f_col + 2.
        let fg2 = buf[(f_col + 2, 3u16)].fg;
        assert_eq!(fg2, ratatui::style::Color::Rgb(99, 99, 99));
    }

    #[test]
    fn preview_pane_paints_content_and_separator() {
        use crate::primitives::palette::PalettePreview;
        // Wide enough for 40/60 split: 40 cells → list_w=16, preview=24.
        let mut buf = Buffer::empty(Rect::new(0, 0, 40, 12));
        let mut p = make_palette();
        p.preview = Some(PalettePreview {
            lines: vec![
                StyledText::plain("line one"),
                StyledText::plain("line two"),
                StyledText::plain("line three"),
            ],
            title: Some("preview.rs".into()),
            scroll_offset: 0,
            highlight_line: Some(1),
        });
        draw_palette(
            &mut buf,
            Rect::new(0, 0, 40, 12),
            &p,
            &Theme::default(),
            false,
        );
        let list_w = ((40.0_f32) * 0.4).round() as u16; // 16
                                                        // Vertical separator on item rows.
        assert_eq!(cell_char(&buf, list_w, 3), '│');
        // Separator junction on the separator row.
        assert_eq!(cell_char(&buf, list_w, 2), '┬');
        // Bottom border junction.
        assert_eq!(cell_char(&buf, list_w, 11), '┴');
        // Preview title painted (first char of "preview.rs").
        let title_col = list_w + 2;
        assert_eq!(cell_char(&buf, title_col, 3), 'p');
        // Preview content line 0 ("line one") at row 4 (after title).
        let content_row = 4;
        let content_col = list_w + 2;
        assert_eq!(cell_char(&buf, content_col, content_row), 'l');
        // Highlight line (line 1) uses selected_bg.
        let highlight_row = 5;
        let highlight_bg = buf[(content_col, highlight_row)].bg;
        let sel_bg_color = ratatui_color(Theme::default().selected_bg);
        assert_eq!(highlight_bg, sel_bg_color);
    }

    #[test]
    fn preview_pane_respects_scroll_offset() {
        use crate::primitives::palette::PalettePreview;
        let mut buf = Buffer::empty(Rect::new(0, 0, 40, 12));
        let mut p = make_palette();
        p.preview = Some(PalettePreview {
            lines: vec![
                StyledText::plain("line zero"),
                StyledText::plain("line one"),
                StyledText::plain("visible"),
            ],
            title: None,
            scroll_offset: 2,
            highlight_line: None,
        });
        draw_palette(
            &mut buf,
            Rect::new(0, 0, 40, 12),
            &p,
            &Theme::default(),
            false,
        );
        let list_w = ((40.0_f32) * 0.4).round() as u16;
        let content_col = list_w + 2;
        // No title → content starts at items_row0 (row 3).
        // scroll_offset=2 → first visible line is "visible".
        assert_eq!(cell_char(&buf, content_col, 3), 'v');
    }

    #[test]
    fn show_query_false_hides_query_and_separator() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 10));
        let mut p = make_palette();
        p.show_query = false;
        draw_palette(
            &mut buf,
            Rect::new(0, 0, 20, 10),
            &p,
            &Theme::default(),
            false,
        );
        // Row 1 should be items, not the query prompt.
        // With show_query=false, items start at y0+1 (right after title).
        let row1_chars: String = (1..6).map(|x| cell_char(&buf, x, 1)).collect();
        assert_ne!(row1_chars, "> fo ");
        // First item "foo" should appear at row 1 (with selection prefix).
        let has_item = (0..20).any(|x| cell_char(&buf, x, 1) == 'f');
        assert!(has_item, "expected item text at row 1 when query hidden");
        // No separator row (├─┤) should exist at row 2.
        assert_ne!(cell_char(&buf, 0, 2), '├');
    }

    #[test]
    fn create_label_renders_pinned_row() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 30, 10));
        let mut p = make_palette();
        p.create_label = Some("Create 'foo'".into());
        draw_palette(
            &mut buf,
            Rect::new(0, 0, 30, 10),
            &p,
            &Theme::default(),
            false,
        );
        // Create row is pinned at items_row_end (y_end - 1 - 1 = row 8).
        // It should show "+ Create 'foo'" with accent_fg.
        let create_row = 8u16;
        let row_text: String = (1..20).map(|x| cell_char(&buf, x, create_row)).collect();
        assert!(
            row_text.contains("+ Create"),
            "expected create label at pinned row, got: {row_text:?}"
        );
    }

    #[test]
    fn too_small_is_a_no_op() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 10));
        let p = make_palette();
        draw_palette(
            &mut buf,
            Rect::new(0, 0, 3, 3),
            &p,
            &Theme::default(),
            false,
        );
        // No corner glyphs anywhere — function returned early.
        for y in 0..10 {
            for x in 0..10 {
                let ch = cell_char(&buf, x, y);
                assert_ne!(ch, '╭');
                assert_ne!(ch, '╯');
            }
        }
    }
}
