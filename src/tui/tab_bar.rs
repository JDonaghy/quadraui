//! TUI rasteriser for [`crate::TabBar`].
//!
//! Per D6: this function consumes a pre-computed
//! [`crate::TabBarLayout`] (built by the caller via
//! [`crate::TabBar::layout`] with its native cell-width measurer)
//! and paints the resolved `visible_tabs` + `visible_segments`
//! verbatim. Returns the tab-content width (in cells) so the caller
//! can adjust scroll offset for the next frame.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Modifier;

use super::{ratatui_color, set_cell, set_cell_styled, set_cell_wide};
use crate::primitives::tab_bar::{TabBar, TabBarLayout};
use crate::theme::Theme;

/// Close-button glyph rendered on each tab. `×` (U+00D7 MULTIPLICATION
/// SIGN) — narrower than `✕` and present in every monospaced terminal
/// font we've encountered.
pub const TAB_CLOSE_CHAR: char = '×';

/// Narrow hardcoded set of Private Use Area glyphs that render as 2
/// cells in terminals and therefore need [`set_cell_wide`]. The wide-glyph
/// predicate is empirical — extend this list as new wide Nerd Font
/// icons appear in tab bars / status bars.
fn is_nerd_wide(c: char) -> bool {
    matches!(
        c,
        '\u{F0932}' // SPLIT_RIGHT
        | '\u{F0143}' // DIFF_PREV
        | '\u{F0140}' // DIFF_NEXT
        | '\u{F0233}' // DIFF_FOLD
    )
}

/// Draw a [`TabBar`] into `area` on `buf`. Returns the **tab-content
/// width** in cells (`area.width - reserved_by_right_segments`) so
/// the caller can decide how many tabs fit and what scroll offset to
/// use on the next frame.
///
/// # Visual contract
///
/// - **Bar background:** filled with `theme.tab_bar_bg`.
/// - **Active tab:** `tab_active_fg` + `tab_active_bg`. When
///   [`TabBar::active_accent`] is `Some`, the filename portion (chars
///   after the last `": "` in `tab.label`) gets a
///   [`Modifier::UNDERLINED`] with that accent colour.
/// - **Dirty tab:** the close cell shows `●` in `theme.foreground`
///   instead of `×`.
/// - **Preview tab:** `*_preview_*_fg` and [`Modifier::ITALIC`]; combines
///   with the underline accent when active.
/// - **Right segments:** painted in `tab_inactive_fg` (or
///   `tab_active_fg` when `seg.is_active`). Glyphs in the
///   wide-Nerd-Font set use [`set_cell_wide`].
pub fn draw_tab_bar(
    buf: &mut Buffer,
    area: Rect,
    bar: &TabBar,
    layout: &TabBarLayout,
    theme: &Theme,
) -> usize {
    if area.width == 0 || area.height == 0 {
        return 0;
    }

    let bar_bg = ratatui_color(theme.tab_bar_bg);
    let btn_fg = ratatui_color(theme.tab_inactive_fg);
    let btn_fg_active = ratatui_color(theme.tab_active_fg);
    let foreground = ratatui_color(theme.foreground);

    // Fill bar background.
    for x in area.x..area.x + area.width {
        set_cell(buf, x, area.y, ' ', bar_bg, bar_bg);
    }

    // Tab-content width (engine feedback): bar minus reserved right area.
    let reserved: u16 = bar.right_segments.iter().map(|s| s.width_cells).sum();
    let tab_content_width = if area.width >= reserved {
        (area.width - reserved) as usize
    } else {
        area.width as usize
    };

    // ── Right-aligned segments (from layout) ───────────────────────────
    for vs in &layout.visible_segments {
        let seg = &bar.right_segments[vs.segment_idx];
        let fg = if seg.is_active { btn_fg_active } else { btn_fg };
        let bx = area.x + vs.bounds.x.round() as u16;
        let seg_end = bx + seg.width_cells;
        let mut cx = bx;
        for ch in seg.text.chars() {
            if cx >= seg_end {
                break;
            }
            if ch == ' ' {
                set_cell(buf, cx, area.y, ' ', fg, bar_bg);
                cx += 1;
            } else if is_nerd_wide(ch) {
                if cx + 1 < seg_end + 1 {
                    set_cell_wide(buf, cx, area.y, ch, fg, bar_bg);
                    cx += 2;
                } else {
                    cx += 1;
                }
            } else {
                set_cell(buf, cx, area.y, ch, fg, bar_bg);
                cx += 1;
            }
        }
    }

    // ── Tabs (from layout) ─────────────────────────────────────────────
    let accent = bar.active_accent.map(ratatui_color);
    let active_fg = ratatui_color(theme.tab_active_fg);
    let active_bg = ratatui_color(theme.tab_active_bg);
    let preview_active_fg = ratatui_color(theme.tab_preview_active_fg);
    let inactive_fg = ratatui_color(theme.tab_inactive_fg);
    let preview_inactive_fg = ratatui_color(theme.tab_preview_inactive_fg);
    let separator = ratatui_color(theme.separator);

    for vt in &layout.visible_tabs {
        let tab = &bar.tabs[vt.tab_idx];
        let tab_x = area.x + vt.bounds.x.round() as u16;

        let (fg, bg) = match (tab.is_active, tab.is_preview) {
            (true, true) => (preview_active_fg, active_bg),
            (true, false) => (active_fg, active_bg),
            (false, true) => (preview_inactive_fg, bar_bg),
            (false, false) => (inactive_fg, bar_bg),
        };

        let mut modifier = Modifier::empty();
        if tab.is_preview {
            modifier |= Modifier::ITALIC;
        }
        if tab.is_active && accent.is_some() {
            modifier |= Modifier::UNDERLINED;
        }
        let prefix_mod = if tab.is_preview {
            Modifier::ITALIC
        } else {
            Modifier::empty()
        };

        // Layout carries total tab width; close_bounds (when present)
        // covers the trailing close-glyph + separator cells. Label
        // occupies the leading cells up to close_bounds.x.
        let tab_width = vt.bounds.width.round() as u16;
        let label_width = match vt.close_bounds {
            Some(cb) => (cb.x - vt.bounds.x).round() as u16,
            None => tab_width,
        };
        let tab_end = tab_x + tab_width;
        let label_end = tab_x + label_width;

        // Filename (after the last ": ") carries the underline accent.
        let prefix_len = tab.label.rfind(": ").map(|p| p + 2).unwrap_or(0);

        let mut x = tab_x;
        for (ci, ch) in tab.label.chars().enumerate() {
            if x >= label_end {
                break;
            }
            let in_filename = ci >= prefix_len;
            let cell_mod = if in_filename { modifier } else { prefix_mod };
            let ul = if in_filename && tab.is_active {
                accent
            } else {
                None
            };
            set_cell_styled(buf, x, area.y, ch, fg, bg, cell_mod, ul);
            x += 1;
        }

        // Close glyph: ● for dirty, × otherwise.
        if vt.close_bounds.is_some() && x < tab_end {
            let (close_ch, close_fg) = if tab.is_dirty {
                ('●', foreground)
            } else if tab.is_active {
                (TAB_CLOSE_CHAR, active_fg)
            } else {
                (TAB_CLOSE_CHAR, separator)
            };
            set_cell(buf, x, area.y, close_ch, close_fg, bg);
            x += 1;
        }
        // Trailing separator space (within tab bounds, uses bar bg).
        if x < tab_end {
            set_cell(buf, x, area.y, ' ', bar_bg, bar_bg);
        }
    }

    tab_content_width
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::tab_bar::{SegmentMeasure, TabBar, TabBarSegment, TabItem, TabMeasure};
    use crate::types::WidgetId;

    fn make_bar(active_idx: usize) -> TabBar {
        TabBar {
            id: WidgetId::new("tabs"),
            tabs: vec![
                TabItem {
                    label: "main.rs".into(),
                    is_active: active_idx == 0,
                    is_dirty: false,
                    is_preview: false,
                },
                TabItem {
                    label: "lib.rs".into(),
                    is_active: active_idx == 1,
                    is_dirty: true,
                    is_preview: false,
                },
            ],
            right_segments: vec![],
            active_accent: None,
            scroll_offset: 0,
        }
    }

    fn cell_char(buf: &Buffer, x: u16, y: u16) -> char {
        buf[(x, y)].symbol().chars().next().unwrap_or(' ')
    }

    /// Each tab is 12 cells total with a 1-cell close button on the right.
    fn measure_tab(_idx: usize) -> TabMeasure {
        TabMeasure::new(12.0, 1.0)
    }

    #[test]
    fn paints_two_tabs_with_close_glyph() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 40, 1));
        let bar = make_bar(0);
        let layout = bar.layout(40.0, 1.0, 0.0, measure_tab, |_| SegmentMeasure::new(0.0));
        draw_tab_bar(
            &mut buf,
            Rect::new(0, 0, 40, 1),
            &bar,
            &layout,
            &Theme::default(),
        );

        // First tab is active and not dirty — its close glyph is '×';
        // second tab is dirty — its close glyph is '●'.
        let mut found_x_close = false;
        let mut found_dirty_dot = false;
        for x in 0..40 {
            match cell_char(&buf, x, 0) {
                '×' => found_x_close = true,
                '●' => found_dirty_dot = true,
                _ => {}
            }
        }
        assert!(found_x_close, "expected '×' close glyph somewhere");
        assert!(found_dirty_dot, "expected '●' dirty glyph somewhere");
    }

    #[test]
    fn returns_full_width_when_no_right_segments() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 30, 1));
        let bar = make_bar(0);
        let layout = bar.layout(30.0, 1.0, 0.0, measure_tab, |_| SegmentMeasure::new(0.0));
        let content_w = draw_tab_bar(
            &mut buf,
            Rect::new(0, 0, 30, 1),
            &bar,
            &layout,
            &Theme::default(),
        );
        assert_eq!(content_w, 30);
    }

    #[test]
    fn reserves_right_segment_width() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 30, 1));
        let bar = TabBar {
            id: WidgetId::new("tabs"),
            tabs: vec![TabItem {
                label: "x".into(),
                is_active: true,
                is_dirty: false,
                is_preview: false,
            }],
            right_segments: vec![TabBarSegment {
                id: Some(WidgetId::new("seg:0")),
                text: "[+]".into(),
                width_cells: 3,
                is_active: false,
            }],
            active_accent: None,
            scroll_offset: 0,
        };
        let layout = bar.layout(
            30.0,
            1.0,
            0.0,
            |_| TabMeasure::new(5.0, 0.0),
            |_| SegmentMeasure::new(3.0),
        );
        let content_w = draw_tab_bar(
            &mut buf,
            Rect::new(0, 0, 30, 1),
            &bar,
            &layout,
            &Theme::default(),
        );
        assert_eq!(content_w, 27);
    }

    #[test]
    fn zero_size_is_a_no_op() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 1));
        let bar = make_bar(0);
        let layout = bar.layout(10.0, 1.0, 0.0, measure_tab, |_| SegmentMeasure::new(0.0));
        // Zero-width area: function must return 0 without panicking.
        let content_w = draw_tab_bar(
            &mut buf,
            Rect::new(0, 0, 0, 1),
            &bar,
            &layout,
            &Theme::default(),
        );
        assert_eq!(content_w, 0);
    }
}
