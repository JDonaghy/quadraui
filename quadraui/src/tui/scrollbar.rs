//! TUI rasteriser for [`crate::Scrollbar`].
//!
//! Vertical scrollbars use full-cell glyphs (`█` thumb, `░` track) and
//! occupy a single column at the right edge of their owning area.
//! Horizontal scrollbars use half-block glyphs (`▄` thumb, `▁` track) so
//! the bar sits at the bottom of its row without obscuring content
//! above. The cell background is supplied by the caller via `cell_bg` —
//! the vertical scrollbar typically uses [`Theme::background`], the
//! horizontal scrollbar typically uses the owning window's active
//! background so the row blends in with the editor area above.

use ratatui::buffer::Buffer;

use super::{ratatui_color, set_cell};
use crate::primitives::scrollbar::{ScrollAxis, Scrollbar};
use crate::theme::Theme;
use crate::types::Color;

/// Draw a [`Scrollbar`] into the buffer.
///
/// The track and thumb are read from `scrollbar` directly — no math
/// happens here. Cell coordinates come from `scrollbar.track` (treated
/// as cell-precise integers via `.round()`) and `scrollbar.thumb_start`
/// / `.thumb_len`.
///
/// `cell_bg` controls the cell background drawn behind both the track
/// glyph and the thumb glyph. For vertical scrollbars at the right edge
/// of an editor area, pass [`Theme::background`]; for horizontal
/// scrollbars at the bottom of a window with a coloured active
/// background, pass that window's bg so the row blends into the editor
/// area above.
///
/// `hovered` and `dragging` state on the primitive are not currently
/// reflected in the TUI rasteriser (TUI doesn't have alpha — the active
/// state is conveyed through user feedback in surrounding chrome).
pub fn draw_scrollbar(buf: &mut Buffer, scrollbar: &Scrollbar, theme: &Theme, cell_bg: Color) {
    let track_fg = ratatui_color(theme.scrollbar_track);
    let thumb_fg = ratatui_color(theme.scrollbar_thumb);
    let bg = ratatui_color(cell_bg);

    let track = scrollbar.track;
    match scrollbar.axis {
        ScrollAxis::Vertical => {
            let track_h = track.height.round() as u16;
            if track_h == 0 || track.width <= 0.0 {
                return;
            }
            let thumb_top = scrollbar.thumb_start.floor() as u16;
            let thumb_size = scrollbar.thumb_len.ceil().max(1.0) as u16;
            let x = track.x.round() as u16;
            let y0 = track.y.round() as u16;
            for dy in 0..track_h {
                let y = y0 + dy;
                let in_thumb = dy >= thumb_top && dy < thumb_top.saturating_add(thumb_size);
                let (ch, fg) = if in_thumb {
                    ('█', thumb_fg)
                } else {
                    ('░', track_fg)
                };
                set_cell(buf, x, y, ch, fg, bg);
            }
        }
        ScrollAxis::Horizontal => {
            let track_w = track.width.round() as u16;
            if track_w == 0 || track.height <= 0.0 {
                return;
            }
            let thumb_left = scrollbar.thumb_start.floor() as u16;
            let thumb_size = scrollbar.thumb_len.ceil().max(1.0) as u16;
            let x0 = track.x.round() as u16;
            let y = track.y.round() as u16;
            for dx in 0..track_w {
                let x = x0 + dx;
                let in_thumb = dx >= thumb_left && dx < thumb_left.saturating_add(thumb_size);
                let (ch, fg) = if in_thumb {
                    ('▄', thumb_fg)
                } else {
                    ('▁', track_fg)
                };
                set_cell(buf, x, y, ch, fg, bg);
            }
        }
    }
}
