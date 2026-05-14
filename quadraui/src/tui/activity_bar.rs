//! TUI rasteriser for [`crate::ActivityBar`].
//!
//! Cell-based equivalent of the GTK activity-bar drawing path. Uses the
//! primitive's [`crate::ActivityBarLayout`] for positioning and paints
//! into a ratatui buffer. Activity bar width is caller-determined (typically
//! 3 cells: 1 accent + 2 icon).

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Modifier;

use super::{qc, set_cell, set_cell_wide};
use crate::primitives::activity_bar::{ActivityBar, ActivityBarRowHit, ActivitySide};
use crate::theme::Theme;

pub fn draw_activity_bar(
    buf: &mut Buffer,
    area: Rect,
    bar: &ActivityBar,
    theme: &Theme,
    hovered_idx: Option<usize>,
) -> Vec<ActivityBarRowHit> {
    if area.width == 0 || area.height == 0 {
        return Vec::new();
    }

    let bg = qc(theme.tab_bar_bg);
    let sep = qc(theme.separator);
    let accent = bar
        .active_accent
        .map(qc)
        .unwrap_or_else(|| qc(theme.accent_fg));
    let active_fg = qc(theme.foreground);
    let inactive_fg = qc(theme.inactive_fg);
    let hover_bg = qc(theme.tab_bar_bg.lighten(0.10));

    // Fill background.
    for row in area.y..area.y + area.height {
        for col in area.x..area.x + area.width {
            set_cell(buf, col, row, ' ', bg, bg);
        }
    }

    // Right-edge separator column.
    let sep_col = area.x + area.width - 1;
    for row in area.y..area.y + area.height {
        set_cell(buf, sep_col, row, '│', sep, bg);
    }

    let layout = bar.layout(area.width as f32, area.height as f32, 1.0);

    let mut regions: Vec<ActivityBarRowHit> = Vec::new();
    let mut flat_idx: usize = 0;

    for vi in &layout.visible_items {
        let y = area.y + vi.bounds.y.round() as u16;
        if y >= area.y + area.height {
            continue;
        }

        let item = match vi.side {
            ActivitySide::Top => &bar.top_items[vi.item_idx],
            ActivitySide::Bottom => &bar.bottom_items[vi.item_idx],
        };

        let is_hovered = hovered_idx == Some(flat_idx);
        let row_bg = if is_hovered { hover_bg } else { bg };
        let fg = if item.is_active || is_hovered {
            active_fg
        } else {
            inactive_fg
        };

        // Row background (hover tint).
        if is_hovered {
            for col in area.x..sep_col {
                set_cell(buf, col, y, ' ', fg, row_bg);
            }
        }

        // Left-edge accent bar for active items.
        if item.is_active {
            set_cell(buf, area.x, y, '▎', accent, row_bg);
        }

        // Icon glyph — centered in the available width (excluding accent
        // column and separator column).
        let icon_ch = item.icon.chars().next().unwrap_or(' ');
        let content_start = area.x + 1; // after accent column
        let content_end = sep_col; // before separator
        let content_w = content_end.saturating_sub(content_start);
        if content_w >= 2 {
            let icon_x = content_start + (content_w - 2) / 2;
            set_cell_wide(buf, icon_x, y, icon_ch, fg, row_bg);
        } else if content_w >= 1 {
            set_cell(buf, content_start, y, icon_ch, fg, row_bg);
        }

        // Keyboard selection highlight.
        if item.is_keyboard_selected {
            for col in area.x..sep_col {
                let cell = &mut buf[(col, y)];
                cell.modifier |= Modifier::REVERSED;
            }
        }

        regions.push(ActivityBarRowHit {
            y_start: y as f64,
            y_end: (y + 1) as f64,
            id: item.id.clone(),
            tooltip: item.tooltip.clone(),
        });

        flat_idx += 1;
    }

    regions
}
