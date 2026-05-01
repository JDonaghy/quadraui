//! TUI rasteriser for [`crate::Completions`].
//!
//! Thin vertical list with side borders, matching the pre-migration
//! `render_completion_popup` chrome — selected row gets a tinted
//! background, side `│` borders frame each row, candidate label
//! renders inside a 1-cell left pad.
//!
//! Per D6: the caller invokes `completions.layout(...)` to get
//! [`crate::CompletionsLayout`] (bounds + visible_items), and this
//! rasteriser paints them verbatim.

use ratatui::buffer::Buffer;

use super::{ratatui_color, set_cell};
use crate::primitives::completions::{Completions, CompletionsLayout};
use crate::theme::Theme;

/// Draw a [`Completions`] popup at its resolved layout.
///
/// Background uses [`Theme::completion_bg`], selected row uses
/// [`Theme::completion_selected_bg`], item text uses
/// [`Theme::completion_fg`], and side borders use
/// [`Theme::completion_border`].
pub fn draw_completions(
    buf: &mut Buffer,
    completions: &Completions,
    layout: &CompletionsLayout,
    theme: &Theme,
) {
    let bg = ratatui_color(theme.completion_bg);
    let sel_bg = ratatui_color(theme.completion_selected_bg);
    let fg = ratatui_color(theme.completion_fg);
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

        // Render the candidate text starting at col 2 (after border + space).
        let label = item
            .label
            .spans
            .first()
            .map(|s| s.text.as_str())
            .unwrap_or("");
        let display = format!(" {label}");
        for (j, ch) in display.chars().enumerate() {
            let col = x + 1 + j as u16;
            if col + 1 >= x + w {
                break;
            }
            set_cell(buf, col, row_y, ch, fg, row_bg);
        }
    }
}
