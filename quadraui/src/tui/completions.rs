//! TUI rasteriser for [`crate::Completions`].
//!
//! Thin vertical list with side borders, matching the pre-migration
//! `render_completion_popup` chrome — selected row gets a tinted
//! background, side `│` borders frame each row. Row content (kind
//! icon, every label span, right-aligned `detail`) degrades onto the
//! cell grid the same content `primitives::completions::
//! native_surface_paint::paint` paints onto a pixel surface — same
//! data, cell glyphs instead of pixel glyphs (see
//! [`crate::paint_surface`]'s module doc, "Why TUI stays out", for why
//! that's a separate painter rather than a shared `PaintSurface` call).
//!
//! Per D6: the caller invokes `completions.layout(...)` to get
//! [`crate::CompletionsLayout`] (bounds + visible_items), and this
//! rasteriser paints them verbatim.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::{draw_styled_text, ratatui_color, set_cell};
use crate::primitives::completions::{Completions, CompletionsLayout};
use crate::theme::Theme;
use crate::types::Decoration;

/// Draw a [`Completions`] popup at its resolved layout.
///
/// Background uses [`Theme::completion_bg`], selected row uses
/// [`Theme::completion_selected_bg`], item text uses
/// [`Theme::completion_fg`], muted text (the kind icon's trailing pad
/// has none, but `detail` does) uses [`Theme::muted_fg`], and side
/// borders use [`Theme::completion_border`].
///
/// Each row paints, left to right: the kind icon
/// ([`crate::primitives::completions::CompletionItem::resolved_icon`]'s
/// `fallback` — TUI has no Nerd Font glyph distinct from it to offer),
/// every span of `label`, and — right-aligned, pinned to the row's own
/// width — `detail`, muted.
pub fn draw_completions(
    buf: &mut Buffer,
    completions: &Completions,
    layout: &CompletionsLayout,
    theme: &Theme,
) {
    let bg = ratatui_color(theme.completion_bg);
    let sel_bg = ratatui_color(theme.completion_selected_bg);
    let fg = ratatui_color(theme.completion_fg);
    let dim_fg = ratatui_color(theme.muted_fg);
    let border = ratatui_color(theme.completion_border);

    let x = layout.bounds.x.round() as u16;
    let y = layout.bounds.y.round() as u16;
    let w = layout.bounds.width.round() as u16;
    if w < 3 {
        return;
    }

    for vis in &layout.visible_items {
        let item = &completions.items[vis.item_idx];
        let row_y = y + (vis.bounds.y - layout.bounds.y).round() as u16;
        let is_selected = vis.item_idx == completions.selected_idx;
        let row_bg = if is_selected { sel_bg } else { bg };

        // Fill the row background.
        for col in 0..w {
            set_cell(buf, x + col, row_y, ' ', fg, row_bg);
        }
        // Left + right borders.
        set_cell(buf, x, row_y, '│', border, bg);
        set_cell(buf, x + w - 1, row_y, '│', border, bg);

        // Row content lives between the two side borders.
        let content_x = x + 1;
        let content_w = w.saturating_sub(2);
        let content_area = Rect {
            x: content_x,
            y: row_y,
            width: content_w,
            height: 1,
        };

        // Kind icon + trailing space, then the label's full styled
        // text — every completion paints an icon, since
        // `resolved_icon` always has one (`item.icon` or the kind's
        // own default).
        let icon = item.resolved_icon();
        let mut col = 0usize;
        for ch in icon.fallback.chars() {
            if col >= content_w as usize {
                break;
            }
            set_cell(buf, content_x + col as u16, row_y, ch, fg, row_bg);
            col += 1;
        }
        if !icon.fallback.is_empty() && col < content_w as usize {
            col += 1;
        }

        let label_end = draw_styled_text(
            buf,
            content_area,
            row_y,
            col,
            &item.label,
            fg,
            row_bg,
            Decoration::Normal,
            dim_fg,
        );

        // Detail text — right-aligned within the row, muted, painted
        // only if it would not collide with the label just drawn.
        if let Some(ref detail) = item.detail {
            let detail_w: usize = detail.spans.iter().map(|s| s.text.chars().count()).sum();
            let start = (content_w as usize).saturating_sub(detail_w + 1);
            if start > label_end {
                draw_styled_text(
                    buf,
                    content_area,
                    row_y,
                    start,
                    detail,
                    dim_fg,
                    row_bg,
                    Decoration::Muted,
                    dim_fg,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::Rect as QRect;
    use crate::primitives::completions::{CompletionItem, CompletionItemMeasure, CompletionKind};
    use crate::types::{StyledSpan, StyledText, WidgetId};

    fn cell_char(buf: &Buffer, x: u16, y: u16) -> char {
        buf[(x, y)].symbol().chars().next().unwrap_or(' ')
    }

    fn cell_at(buf: &Buffer, x: u16, y: u16) -> &ratatui::buffer::Cell {
        &buf[(x, y)]
    }

    fn completions_with(items: Vec<CompletionItem>, selected_idx: usize) -> Completions {
        Completions {
            id: WidgetId::new("c"),
            items,
            selected_idx,
            scroll_offset: 0,
            has_focus: true,
        }
    }

    /// Anchors the cursor one line above the viewport's top edge, so
    /// `below_y = cursor_y + line_height` lands the popup's first row
    /// at buffer row `0` — the tests below index `buf` directly by row,
    /// so they need a fixed, known `bounds.y`.
    fn layout_for(c: &Completions, w: u16, h: u16) -> CompletionsLayout {
        let viewport = QRect::new(0.0, 0.0, w as f32, h as f32);
        c.layout(0.0, -1.0, 1.0, viewport, w as f32, h as f32, |_| {
            CompletionItemMeasure::new(1.0)
        })
    }

    #[test]
    fn paints_the_kind_icon_ahead_of_the_label() {
        let items = vec![CompletionItem {
            label: StyledText::plain("map"),
            detail: None,
            documentation: None,
            kind: CompletionKind::Function,
            icon: None,
        }];
        let c = completions_with(items, 0);
        let l = layout_for(&c, 20, 3);
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 3));
        draw_completions(&mut buf, &c, &l, &Theme::default());

        let icon = CompletionKind::Function.default_icon();
        let painted: String = icon
            .fallback
            .chars()
            .enumerate()
            .map(|(i, _)| cell_char(&buf, 1 + i as u16, 0))
            .collect();
        assert_eq!(painted, icon.fallback);
    }

    #[test]
    fn paints_every_label_span() {
        let items = vec![CompletionItem {
            label: StyledText {
                spans: vec![StyledSpan::plain("fn "), StyledSpan::plain("main")],
            },
            detail: None,
            documentation: None,
            kind: CompletionKind::Text,
            icon: None,
        }];
        let c = completions_with(items, 0);
        let l = layout_for(&c, 20, 3);
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 3));
        draw_completions(&mut buf, &c, &l, &Theme::default());

        // Icon ("tx") + space occupies cols 1..3; label starts at col 4.
        let label_start = 1 + CompletionKind::Text.default_icon().fallback.chars().count() + 1;
        let painted: String = "fn main"
            .chars()
            .enumerate()
            .map(|(i, _)| cell_char(&buf, label_start as u16 + i as u16, 0))
            .collect();
        assert_eq!(painted, "fn main");
    }

    #[test]
    fn paints_detail_right_aligned_and_muted() {
        let items = vec![CompletionItem {
            label: StyledText::plain("map"),
            detail: Some(StyledText::plain("fn(T) -> U")),
            documentation: None,
            kind: CompletionKind::Method,
            icon: None,
        }];
        let c = completions_with(items, 0);
        let w = 24u16;
        let l = layout_for(&c, w, 3);
        let mut buf = Buffer::empty(Rect::new(0, 0, w, 3));
        let theme = Theme::default();
        draw_completions(&mut buf, &c, &l, &theme);

        let detail = "fn(T) -> U";
        // Detail is right-aligned within the row, one cell before the
        // right border.
        let start = (w - 2) as usize - detail.chars().count() - 1 + 1; // content_x offset
        let painted: String = detail
            .chars()
            .enumerate()
            .map(|(i, _)| cell_char(&buf, start as u16 + i as u16, 0))
            .collect();
        assert_eq!(painted, detail);
        let fg = cell_at(&buf, start as u16, 0).fg;
        assert_eq!(fg, ratatui_color(theme.muted_fg));
    }

    #[test]
    fn selected_row_gets_completion_selected_bg() {
        let items = vec![
            CompletionItem {
                label: StyledText::plain("a"),
                detail: None,
                documentation: None,
                kind: CompletionKind::Text,
                icon: None,
            },
            CompletionItem {
                label: StyledText::plain("b"),
                detail: None,
                documentation: None,
                kind: CompletionKind::Text,
                icon: None,
            },
        ];
        let c = completions_with(items, 1);
        let l = layout_for(&c, 20, 3);
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 3));
        let theme = Theme::default();
        draw_completions(&mut buf, &c, &l, &theme);

        assert_eq!(
            cell_at(&buf, 2, 1).bg,
            ratatui_color(theme.completion_selected_bg)
        );
        assert_eq!(cell_at(&buf, 2, 0).bg, ratatui_color(theme.completion_bg));
    }
}
