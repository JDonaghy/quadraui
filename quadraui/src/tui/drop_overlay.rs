//! TUI rasteriser for [`crate::DropOverlay`].
//!
//! Highlight: tints background cells with `theme.accent_fg`.
//! Insertion bar: renders `│` characters in `theme.accent_fg`.

use ratatui::buffer::Buffer;

use super::{qc, set_cell};
use crate::primitives::drop_zone::DropOverlay;
use crate::theme::Theme;
use crate::types::Color;

pub fn draw_drop_overlay(buf: &mut Buffer, overlay: &DropOverlay, theme: &Theme) {
    let accent = theme.accent_fg;
    let tint = Color::rgb(
        accent.r.saturating_add(30),
        accent.g.saturating_add(30),
        accent.b.saturating_add(60),
    );
    let tint_bg = qc(tint);
    let accent_fg = qc(accent);

    if let Some(h) = overlay.highlight {
        let x0 = h.x.round() as u16;
        let y0 = h.y.round() as u16;
        let x1 = (h.x + h.width).round() as u16;
        let y1 = (h.y + h.height).round() as u16;
        for y in y0..y1 {
            for x in x0..x1 {
                let area = buf.area;
                if x < area.x + area.width && y < area.y + area.height {
                    buf[(x, y)].set_bg(tint_bg);
                }
            }
        }
    }

    if let Some(bar) = overlay.insertion_bar {
        let x = bar.x.round() as u16;
        let y0 = bar.y.round() as u16;
        let y1 = (bar.y + bar.height).round() as u16;
        let bg = qc(theme.background);
        for y in y0..y1 {
            set_cell(buf, x, y, '│', accent_fg, bg);
        }
    }
}
