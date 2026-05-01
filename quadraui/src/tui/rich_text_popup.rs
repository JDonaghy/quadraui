//! TUI rasteriser for [`crate::RichTextPopup`].
//!
//! Per D6: the caller invokes `popup.layout(...)` to get
//! [`crate::RichTextPopupLayout`] (bounds + visible_lines + scrollbar +
//! link_hit_regions), and this rasteriser paints them verbatim.
//!
//! Renders: bordered box with focus-tinted border colour, per-cell
//! styled spans (fg + bold + italic + underline), text-selection
//! inversion, focused-link underline, and a thumb scrollbar on the
//! right border when content overflows.

use ratatui::buffer::Buffer;

use super::{ratatui_color, set_cell};
use crate::primitives::rich_text_popup::{RichTextPopup, RichTextPopupLayout};
use crate::theme::Theme;
use crate::types::Color;

fn qc(c: Color) -> ratatui::style::Color {
    ratatui_color(c)
}

/// Draw a [`RichTextPopup`] into the buffer at its resolved layout.
///
/// The frame border uses [`Theme::link_fg`] when `popup.has_focus`,
/// otherwise [`Theme::hover_border`]. Per-popup `popup.bg` / `popup.fg`
/// overrides win over [`Theme::hover_bg`] / [`Theme::hover_fg`].
pub fn draw_rich_text_popup(
    buf: &mut Buffer,
    popup: &RichTextPopup,
    layout: &RichTextPopupLayout,
    theme: &Theme,
) {
    let bx = layout.bounds.x.round() as u16;
    let by = layout.bounds.y.round() as u16;
    let bw = layout.bounds.width.round() as u16;
    let bh = layout.bounds.height.round() as u16;
    if bw == 0 || bh == 0 {
        return;
    }

    let bg = popup.bg.map(qc).unwrap_or_else(|| qc(theme.hover_bg));
    let fg = popup.fg.map(qc).unwrap_or_else(|| qc(theme.hover_fg));
    let border = if popup.has_focus {
        qc(theme.link_fg)
    } else {
        qc(theme.hover_border)
    };

    // Top border with corners.
    for c in 0..bw {
        let cx = bx + c;
        let ch = if c == 0 {
            '┌'
        } else if c == bw - 1 {
            '┐'
        } else {
            '─'
        };
        set_cell(buf, cx, by, ch, border, bg);
    }
    // Bottom border.
    for c in 0..bw {
        let cx = bx + c;
        let ch = if c == 0 {
            '└'
        } else if c == bw - 1 {
            '┘'
        } else {
            '─'
        };
        set_cell(buf, cx, by + bh - 1, ch, border, bg);
    }
    // Side borders + content fill for inner rows.
    for row in 1..bh - 1 {
        set_cell(buf, bx, by + row, '│', border, bg);
        set_cell(buf, bx + bw - 1, by + row, '│', border, bg);
        for col in 1..bw - 1 {
            set_cell(buf, bx + col, by + row, ' ', fg, bg);
        }
    }

    // Visible lines: walk styled spans char-by-char.
    for vis in &layout.visible_lines {
        let row_y = vis.bounds.y.round() as u16;
        let line_x = vis.bounds.x.round() as u16;
        let line_w = vis.bounds.width.round() as u16;
        if row_y >= by + bh - 1 {
            break;
        }

        let line_idx = vis.line_idx;
        let styled = popup.lines.get(line_idx);

        let focused_link_range = popup.focused_link.and_then(|fi| {
            popup
                .links
                .get(fi)
                .filter(|l| l.line == line_idx)
                .map(|l| (l.start_byte, l.end_byte))
        });

        let mut col_off: u16 = 0;
        let mut byte_pos: usize = 0;
        if let Some(styled) = styled {
            for span in &styled.spans {
                let span_fg = span.fg.map(qc).unwrap_or(fg);
                let span_bg = span.bg.map(qc).unwrap_or(bg);
                for ch in span.text.chars() {
                    if col_off >= line_w {
                        break;
                    }
                    let cx = line_x + col_off;
                    let char_col = col_off as usize;

                    // Selection inversion.
                    let in_selection = popup
                        .selection
                        .map(|s| s.contains(line_idx, char_col))
                        .unwrap_or(false);
                    let (cell_fg, cell_bg) = if in_selection {
                        (bg, span_fg)
                    } else {
                        (span_fg, span_bg)
                    };
                    set_cell(buf, cx, row_y, ch, cell_fg, cell_bg);

                    // Focused-link underline.
                    if popup.has_focus
                        && !in_selection
                        && focused_link_range
                            .map(|(s, e)| byte_pos >= s && byte_pos < e)
                            .unwrap_or(false)
                    {
                        if let Some(cell) =
                            buf.cell_mut(ratatui::prelude::Position { x: cx, y: row_y })
                        {
                            cell.set_style(
                                cell.style()
                                    .add_modifier(ratatui::style::Modifier::UNDERLINED),
                            );
                        }
                    }

                    col_off += 1;
                    byte_pos += ch.len_utf8();
                }
            }
        }
        // Pad the rest of the line with bg.
        while col_off < line_w {
            let cx = line_x + col_off;
            let in_selection = popup
                .selection
                .map(|s| s.contains(line_idx, col_off as usize))
                .unwrap_or(false);
            let cell_bg = if in_selection { fg } else { bg };
            set_cell(buf, cx, row_y, ' ', fg, cell_bg);
            col_off += 1;
        }
    }

    // Scrollbar thumb on the right border, when present.
    if let Some(sb) = layout.scrollbar {
        let track_x = sb.track.x.round() as u16;
        let track_y = sb.track.y.round() as u16;
        let track_h = sb.track.height.round() as u16;
        let thumb_y = sb.thumb.y.round() as u16;
        let thumb_h = sb.thumb.height.round() as u16;
        for r in 0..track_h {
            let cy = track_y + r;
            let in_thumb = cy >= thumb_y && cy < thumb_y + thumb_h;
            let ch = if in_thumb { '█' } else { '░' };
            let cell_fg = if in_thumb { border } else { fg };
            set_cell(buf, track_x, cy, ch, cell_fg, bg);
        }
    }
}
