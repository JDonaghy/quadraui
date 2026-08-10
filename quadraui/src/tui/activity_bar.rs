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

        // Hit regions are **bar-relative**, not absolute: report the
        // row offset within `area`, not the screen row `y` we painted
        // at. Keeping `area.y` in here double-counted the bar origin,
        // because `AppShell` adds `activity_bar_bounds.y` itself in
        // both its click and hover readers — a shift that was zero
        // (invisible) while the title bar was hidden and became a
        // one-row off-by-one the instant `set_title_bar_visible(true)`
        // revealed it. GTK, macOS, and `backend::activity_bar_hits`
        // all already used this space; the TUI was the lone outlier.
        // Issue #552 (the coordinate-space half of #547's height fix).
        let rel_y = (y - area.y) as f64;
        regions.push(ActivityBarRowHit {
            y_start: rel_y,
            y_end: rel_y + 1.0,
            id: item.id.clone(),
            tooltip: item.tooltip.clone(),
        });

        flat_idx += 1;
    }

    regions
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::activity_bar::ActivityItem;
    use crate::types::WidgetId;

    fn item(id: &str, icon: &str) -> ActivityItem {
        ActivityItem {
            id: WidgetId::new(id),
            icon: icon.into(),
            tooltip: format!("{id} tooltip"),
            is_active: false,
            is_keyboard_selected: false,
        }
    }

    fn bar() -> ActivityBar {
        ActivityBar {
            id: WidgetId::new("activity"),
            top_items: vec![item("explorer", "E"), item("search", "S"), item("git", "G")],
            bottom_items: vec![item("settings", "*")],
            active_accent: None,
            selection_bg: None,
            is_keyboard_focused: false,
        }
    }

    /// The #552 contract, stated as an assertion: hit regions are
    /// **bar-relative**, so the top row starts at `0.0` no matter where
    /// the bar is painted.
    ///
    /// This is the test that fails against pre-fix `develop`: with
    /// `area.y == 3` the rasteriser returned `y_start == 3.0` for the
    /// first top row (its absolute paint row), which `AppShell` then
    /// shifted by `activity_bar_bounds.y` a second time.
    ///
    /// Looks rows up **by id** rather than by position: `visible_items`
    /// (and therefore this list) is bottom-pinned-first, not visual
    /// top-to-bottom order.
    #[test]
    fn hit_regions_are_bar_relative_not_absolute() {
        let b = bar();
        let theme = Theme::default();

        let mut buf = Buffer::empty(Rect::new(0, 0, 40, 20));
        let area = Rect::new(0, 3, 3, 10);
        let hits = draw_activity_bar(&mut buf, area, &b, &theme, None);

        let span = |id: &str| {
            let h = hits
                .iter()
                .find(|h| h.id.as_str() == id)
                .unwrap_or_else(|| panic!("no hit region for {id:?}"));
            (h.y_start, h.y_end)
        };

        assert_eq!(
            span("explorer"),
            (0.0, 1.0),
            "the top row must start at 0.0 relative to the bar, not at \
             area.y ({}) — folding the origin in here double-counts it \
             against AppShell's own `+ activity_bar_bounds.y` (issue #552)",
            area.y
        );
        assert_eq!(span("search"), (1.0, 2.0), "second row is one row down");
        assert_eq!(span("git"), (2.0, 3.0));
        // Bottom-pinned: `area.height` (10) - 1 row, still measured from
        // the bar's own top edge, not the screen's.
        assert_eq!(span("settings"), (9.0, 10.0));
    }

    /// The same regions must come back for *any* `area.y`. Pinning
    /// invariance rather than a single value is what stops the bug from
    /// reappearing: the pre-fix output was correct at `area.y == 0` (title
    /// bar hidden) and wrong everywhere else, which is exactly why every
    /// existing static test passed.
    #[test]
    fn hit_regions_do_not_move_when_the_bar_does() {
        let b = bar();
        let theme = Theme::default();

        let spans = |y: u16| {
            let mut buf = Buffer::empty(Rect::new(0, 0, 40, 30));
            let hits = draw_activity_bar(&mut buf, Rect::new(0, y, 3, 10), &b, &theme, None);
            hits.iter()
                .map(|h| (h.y_start, h.y_end))
                .collect::<Vec<_>>()
        };

        let at_zero = spans(0);
        assert!(!at_zero.is_empty(), "sanity: some rows were painted");
        for y in [1u16, 2, 5] {
            assert_eq!(
                spans(y),
                at_zero,
                "hit regions must be independent of the bar's screen origin; \
                 they differed at area.y = {y} (issue #552)"
            );
        }
    }

    /// Painting still uses the absolute row — the fix must not move the
    /// glyphs, only the reported regions. The operator's note on
    /// vimcode#634 was explicit that rendering was already correct and
    /// only the click mapping was wrong.
    #[test]
    fn icons_still_paint_at_the_absolute_row() {
        let b = bar();
        let theme = Theme::default();
        let mut buf = Buffer::empty(Rect::new(0, 0, 40, 20));
        let area = Rect::new(0, 3, 4, 10);
        draw_activity_bar(&mut buf, area, &b, &theme, None);

        let row_text = |y: u16| {
            (area.x..area.x + area.width)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect::<String>()
        };
        assert!(
            row_text(3).contains('E'),
            "first icon should paint on the bar's first *screen* row (3), \
             got {:?}",
            row_text(3)
        );
        assert!(
            row_text(4).contains('S'),
            "second icon on screen row 4, got {:?}",
            row_text(4)
        );
    }
}
