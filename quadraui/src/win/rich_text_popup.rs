//! Direct2D / DirectWrite rasteriser for [`crate::RichTextPopup`]
//! (issue #28).
//!
//! Mirrors `gtk::rich_text_popup`'s structure: `layout` is fully
//! resolved upstream (host calls
//! [`crate::primitives::rich_text_popup::RichTextPopup::layout`]); this
//! module paints it — background, border (accent when `popup.has_focus`),
//! per-visible-line styled spans, a selection-bg fill, the focused
//! link's underline, and the scrollbar when present — and returns the
//! per-link hit rectangles `(Rect, url)` computed from real
//! `DWrite::measure_text` glyph widths, mirroring `gtk::draw_rich_text_popup`'s
//! `index_to_pos`-derived link rects (more accurate than the primitive's
//! own `layout.link_hit_regions`, whose widths come from the host's
//! measure closure rather than this backend's actual glyph advances).
//!
//! Only compiled on `target_os = "windows"` — see `super::mod`'s
//! `#[cfg(target_os = "windows")] mod rich_text_popup;` and
//! `backend.rs`'s module docs.

use windows::Win32::Graphics::Direct2D::ID2D1RenderTarget;

use super::text::{fill_rect, stroke_rect, DWrite};
use crate::event::Rect;
use crate::primitives::rich_text_popup::{RichTextPopup, RichTextPopupLayout, TextSelection};
use crate::theme::Theme;

/// Translate a `TextSelection` (char columns) into the byte range this
/// line contributes to the selection. Returns `(0, 0)` when the line is
/// outside the selection. Verbatim port of `gtk::rich_text_popup`'s
/// private helper of the same name.
fn selection_byte_range(sel: TextSelection, line_idx: usize, line_text: &str) -> (usize, usize) {
    if line_idx < sel.start_line || line_idx > sel.end_line {
        return (0, 0);
    }
    let char_to_byte = |col: usize| -> usize {
        line_text
            .char_indices()
            .nth(col)
            .map(|(b, _)| b)
            .unwrap_or(line_text.len())
    };
    let (start_col, end_col) = if sel.start_line == sel.end_line {
        (sel.start_col, sel.end_col)
    } else if line_idx == sel.start_line {
        (sel.start_col, line_text.chars().count())
    } else if line_idx == sel.end_line {
        (0, sel.end_col)
    } else {
        (0, line_text.chars().count())
    };
    if end_col <= start_col {
        return (0, 0);
    }
    (char_to_byte(start_col), char_to_byte(end_col))
}

/// Draw a [`RichTextPopup`] at its resolved `layout`. Returns per-link
/// hit regions `(Rect, url)`.
pub fn draw_rich_text_popup(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    popup: &RichTextPopup,
    layout: &RichTextPopupLayout,
) -> Vec<(Rect, String)> {
    let bounds = layout.bounds;
    if bounds.width <= 0.0 || bounds.height <= 0.0 {
        return Vec::new();
    }

    let theme = Theme::default();
    let bg = popup.bg.unwrap_or(theme.hover_bg);
    let fg = popup.fg.unwrap_or(theme.hover_fg);
    let border = if popup.has_focus {
        theme.link_fg
    } else {
        theme.hover_border
    };

    let _ = fill_rect(target, bounds, bg);
    let _ = stroke_rect(target, bounds, border, 1.0);

    let mut link_rects: Vec<(Rect, String)> = Vec::new();

    for vis in &layout.visible_lines {
        let line_idx = vis.line_idx;
        let raw_text = popup
            .line_text
            .get(line_idx)
            .map(String::as_str)
            .unwrap_or("");
        let Some(styled) = popup.lines.get(line_idx) else {
            continue;
        };

        let (sel_start, sel_end) = popup
            .selection
            .map(|sel| selection_byte_range(sel, line_idx, raw_text))
            .unwrap_or((0, 0));
        if sel_end > sel_start {
            // Selection bg is painted as a single rect spanning the byte
            // range's measured width, ahead of the text itself.
            let pre_w = dwrite
                .measure_text(&raw_text[..sel_start])
                .map(|(w, _)| w)
                .unwrap_or(0.0);
            let sel_w = dwrite
                .measure_text(&raw_text[sel_start..sel_end])
                .map(|(w, _)| w)
                .unwrap_or(0.0);
            let sel_rect = Rect::new(
                vis.bounds.x + pre_w,
                vis.bounds.y,
                sel_w.max(1.0),
                vis.bounds.height,
            );
            let _ = fill_rect(target, sel_rect, popup.fg.unwrap_or(theme.foreground));
        }

        let focused_underline_range = if popup.has_focus {
            popup.focused_link.and_then(|idx| {
                popup
                    .links
                    .get(idx)
                    .filter(|link| link.line == line_idx)
                    .map(|link| (link.start_byte, link.end_byte))
            })
        } else {
            None
        };

        let mut byte_pos = 0usize;
        let mut x = vis.bounds.x;
        for span in &styled.spans {
            let start = byte_pos;
            let end = byte_pos + span.text.len();
            let in_selection = sel_end > sel_start && start >= sel_start && end <= sel_end;
            let color = if in_selection {
                popup.bg.unwrap_or(theme.background)
            } else {
                span.fg.unwrap_or(fg)
            };
            let (w, _) = dwrite.measure_text(&span.text).unwrap_or((0.0, 0.0));
            let rect = Rect::new(x, vis.bounds.y, w.max(1.0), vis.bounds.height);
            let _ = dwrite.draw_text_styled(target, &span.text, rect, color, span.bold);

            // Underline the whole span when it overlaps the focused
            // link's byte range. Coarser than GTK's per-substring
            // underline (which uses Pango's `index_to_pos` to underline
            // exactly the link's own characters within a mixed span),
            // but avoids re-slicing `span.text` at arbitrary byte
            // offsets that aren't guaranteed to land on this span's own
            // char boundaries.
            if let Some((us, ue)) = focused_underline_range {
                if start < ue && end > us {
                    let uy = vis.bounds.y + vis.bounds.height - 2.0;
                    let _ = super::text::draw_line(target, x, uy, x + w, uy, border, 1.0);
                }
            }

            x += w;
            byte_pos = end;
        }

        for link in popup.links.iter().filter(|l| l.line == line_idx) {
            let pre_w = dwrite
                .measure_text(&raw_text[..link.start_byte])
                .map(|(w, _)| w)
                .unwrap_or(0.0);
            let span_w = dwrite
                .measure_text(&raw_text[link.start_byte..link.end_byte])
                .map(|(w, _)| w)
                .unwrap_or(0.0);
            let rect = Rect::new(
                vis.bounds.x + pre_w,
                vis.bounds.y,
                span_w.max(1.0),
                vis.bounds.height,
            );
            link_rects.push((rect, link.url.clone()));
        }
    }

    if let Some(sb) = layout.scrollbar {
        let _ = fill_rect(target, sb.track, theme.muted_fg);
        let _ = fill_rect(target, sb.thumb, border);
    }

    link_rects
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::rich_text_popup::{PopupPlacement, RichTextLink, RichTextPopupMeasure};
    use crate::types::{StyledText, WidgetId};
    use crate::win::testing::HeadlessSurface;

    fn popup() -> RichTextPopup {
        RichTextPopup {
            id: WidgetId::new("rtp"),
            lines: vec![StyledText::plain("see docs")],
            line_text: vec!["see docs".into()],
            line_scales: vec![],
            scroll_top: 0,
            max_visible_rows: 10,
            has_focus: false,
            selection: None,
            links: vec![RichTextLink {
                line: 0,
                start_byte: 4,
                end_byte: 8,
                url: "https://example.com".into(),
            }],
            focused_link: None,
            placement: PopupPlacement::Below,
            padding: 2.0,
            fg: None,
            bg: None,
        }
    }

    #[test]
    fn paints_and_returns_link_hit_regions() {
        let surface = HeadlessSurface::new(300, 200).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
        let p = popup();
        let viewport = Rect::new(0.0, 0.0, 300.0, 200.0);
        let measure = RichTextPopupMeasure::new(200.0, 16.0);
        let layout = p.layout(20.0, 100.0, viewport, measure, |_, s, e| (e - s) as f32);

        let mut links = Vec::new();
        surface
            .paint(|target| {
                links = draw_rich_text_popup(target, &dwrite, &p, &layout);
            })
            .expect("paint rich text popup");

        assert_eq!(links.len(), 1);
        assert_eq!(links[0].1, "https://example.com");

        let theme = Theme::default();
        let b = layout.bounds;
        let inner = surface.pixel_at((b.x + b.width - 3.0) as u32, (b.y + b.height - 3.0) as u32);
        assert_eq!(
            (inner.r, inner.g, inner.b),
            (theme.hover_bg.r, theme.hover_bg.g, theme.hover_bg.b)
        );
    }
}
