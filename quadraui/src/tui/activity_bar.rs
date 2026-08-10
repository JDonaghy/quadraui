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
    // Explicit keyboard-selection background when the bar sets `selection_bg`.
    // `None` → fall back to `Modifier::REVERSED` after the icon paint (terminal-
    // agnostic inversion that works in every colour scheme).
    let sel_explicit_bg: Option<ratatui::style::Color> = bar.selection_bg.map(qc);

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
        // Effective row background priority: explicit keyboard-selection >
        // hover > default. When keyboard-selected without an explicit
        // `selection_bg`, the row uses the default bg and REVERSED is applied
        // after the icon paint (so all cells invert, giving a clear cursor
        // in any terminal colour scheme).
        let row_bg = if item.is_keyboard_selected {
            sel_explicit_bg.unwrap_or(if is_hovered { hover_bg } else { bg })
        } else if is_hovered {
            hover_bg
        } else {
            bg
        };
        let fg = if item.is_active || is_hovered || item.is_keyboard_selected {
            active_fg
        } else {
            inactive_fg
        };

        // Row background fill (hover tint or explicit keyboard-selection bg).
        // Both use the same `row_bg` computed above, but we only fill when
        // there is actually a tint to apply.
        if is_hovered || (item.is_keyboard_selected && sel_explicit_bg.is_some()) {
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

        // Keyboard selection highlight — REVERSED fallback when no explicit
        // `selection_bg`. Applied last so it overrides any earlier colour.
        if item.is_keyboard_selected && sel_explicit_bg.is_none() {
            for col in area.x..sep_col {
                let cell = &mut buf[(col, y)];
                cell.modifier |= Modifier::REVERSED;
            }
        }

        // Hit rows are RECT-RELATIVE (`y - area.y`), matching the GTK and
        // macOS rasterisers and the shared no-paint helper
        // (`backend::activity_bar_hits`). Consumers — notably
        // `AppShell::cached_activity_hit`/`update_hover` — add the cached
        // bar bounds' own `y` back on top, so returning absolute rows here
        // double-counted `area.y` and shifted every hit region down by the
        // bar's offset the moment the bar stopped starting at row 0 (e.g.
        // after `set_title_bar_visible(true)` reserved the title-bar row):
        // clicking icon N activated icon N-1 while paint stayed correct
        // (vimcode#634's recurring smoke bug).
        regions.push(ActivityBarRowHit {
            y_start: (y - area.y) as f64,
            y_end: (y - area.y + 1) as f64,
            id: item.id.clone(),
            tooltip: item.tooltip.clone(),
        });

        flat_idx += 1;
    }

    regions
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::WidgetId;

    fn item(id: &str, icon: &str) -> crate::primitives::activity_bar::ActivityItem {
        crate::primitives::activity_bar::ActivityItem {
            id: WidgetId::new(id),
            icon: icon.into(),
            tooltip: String::new(),
            is_active: false,
            is_keyboard_selected: false,
        }
    }

    fn sample_bar() -> ActivityBar {
        ActivityBar {
            id: WidgetId::new("activity"),
            top_items: vec![item("activity:explorer", "E"), item("activity:search", "S")],
            bottom_items: vec![item("activity:settings", "*")],
            active_accent: None,
            selection_bg: None,
            is_keyboard_focused: false,
        }
    }

    /// Hit rows must be RECT-RELATIVE — same convention as the GTK and
    /// macOS rasterisers and `backend::activity_bar_hits` — even when the
    /// bar is painted at a non-zero `area.y`. Regression test for
    /// vimcode#634: absolute rows here made `AppShell::cached_activity_hit`
    /// (which adds the bar bounds' `y` itself) double-count the offset the
    /// moment a runtime-revealed title bar pushed the bar off row 0, so a
    /// click on icon N's painted row activated icon N-1.
    #[test]
    fn row_hits_are_rect_relative_even_with_offset_area() {
        let bar = sample_bar();
        let theme = Theme::default();
        // Bar painted 3 rows down / 2 cols in — e.g. below a title bar.
        let area = Rect::new(2, 3, 3, 10);
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 16));
        let regions = draw_activity_bar(&mut buf, area, &bar, &theme, None);

        // `ActivityBar::layout` emits bottom-pinned items first (they win
        // on collision), then top items — regions follow that order.
        assert_eq!(regions.len(), 3);
        // Bottom-pinned item hugs the rect's bottom edge (height 10).
        assert_eq!(regions[0].y_start, 9.0);
        assert_eq!(regions[0].y_end, 10.0);
        assert_eq!(regions[0].id, WidgetId::new("activity:settings"));
        // Top items start at the top of the RECT, not of the screen.
        assert_eq!(regions[1].y_start, 0.0);
        assert_eq!(regions[1].y_end, 1.0);
        assert_eq!(regions[1].id, WidgetId::new("activity:explorer"));
        assert_eq!(regions[2].y_start, 1.0);
        assert_eq!(regions[2].y_end, 2.0);
        assert_eq!(regions[2].id, WidgetId::new("activity:search"));
    }
}
