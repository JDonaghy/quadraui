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

use super::text::char_cell_width;
use super::{ratatui_color, set_cell, set_cell_styled, set_cell_wide, set_cell_wide_styled};
use crate::primitives::tab_bar::{TabBar, TabBarHits, TabBarLayout};
use crate::theme::Theme;

/// Close-button glyph rendered on each tab. `×` (U+00D7 MULTIPLICATION
/// SIGN) — narrower than `✕` and present in every monospaced terminal
/// font we've encountered.
pub const TAB_CLOSE_CHAR: char = '×';

/// Cell width reserved per tab for the close-button + trailing
/// separator. Apps that build a [`crate::TabMeasure`] for tab-bar
/// layout pass this as the `close_width` so the layout reserves the
/// right amount of trailing space.
pub const TAB_CLOSE_COLS: u16 = 2;

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
/// - **Icon:** never painted by this entry point — it passes an empty
///   icon sidecar. Call [`draw_tab_bar_icons`] to decorate tabs with
///   [`crate::TabIcon`] glyphs.
/// - **Right segments:** painted in `tab_inactive_fg` (or
///   `tab_active_fg` when `seg.is_active`). Double-width glyphs
///   (per `unicode-width`) use [`set_cell_wide`].
/// - **Tab labels:** measured and painted in display columns, not
///   `char`s (#554) — a double-width glyph (per `unicode-width`) uses
///   [`set_cell_wide`]/[`set_cell_wide_styled`] and advances the cursor
///   by 2, mirroring the right-segment loop above. The measure side
///   (`TuiBackend::draw_tab_bar` / `tab_bar_layout` in `backend.rs`)
///   must agree, or a tab is measured narrower than it paints.
///
///   Downstream: vimcode's `tab_hit_width` (`src/render.rs`) is
///   deliberately pinned on the pre-#554 `.chars().count()` measure
///   because it must agree with what this rasteriser paints; its own
///   doc comment names this fix as the unblocker for switching to a
///   `display_width` measure there. coord-tui consumes the same tab
///   bar and should be checked for the equivalent assumption.
pub fn draw_tab_bar(
    buf: &mut Buffer,
    area: Rect,
    bar: &TabBar,
    layout: &TabBarLayout,
    theme: &Theme,
) -> TabBarHits {
    draw_tab_bar_icons(buf, area, bar, &[], layout, theme)
}

/// [`draw_tab_bar`] plus per-tab icon glyphs (#620).
///
/// `icons` is a sidecar slice parallel to `bar.tabs` (see
/// [`crate::Backend::draw_tab_bar_icons`]); resolve entries with
/// [`crate::tab_icon_at`] / [`crate::tab_icon_cols`] so a short slice
/// means "no icon" rather than a panic. Passing `&[]` reproduces
/// [`draw_tab_bar`] cell for cell.
///
/// Each decorated tab paints its glyph at the tab's leading edge in
/// [`crate::TabIcon::color`] — independent of the tab's
/// active/inactive foreground, so the icon keeps its identity colour on
/// an inactive tab — followed by a 1-column gap before the label. That
/// reservation is [`crate::tab_icon_cols`], exactly what
/// `TuiBackend::draw_tab_bar_icons` / `tab_bar_layout_icons`
/// (`backend.rs`) add to the tab's measured width, so paint and
/// measurement cannot drift.
pub fn draw_tab_bar_icons(
    buf: &mut Buffer,
    area: Rect,
    bar: &TabBar,
    icons: &[Option<crate::primitives::tab_bar::TabIcon>],
    layout: &TabBarLayout,
    theme: &Theme,
) -> TabBarHits {
    if area.width == 0 || area.height == 0 {
        return TabBarHits::default();
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
    let mut right_segment_bounds: Vec<(f64, f64)> = Vec::with_capacity(bar.right_segments.len());
    for vs in &layout.visible_segments {
        let seg = &bar.right_segments[vs.segment_idx];
        let fg = if seg.is_active { btn_fg_active } else { btn_fg };
        let bx = area.x + vs.bounds.x.round() as u16;
        let seg_end = bx + seg.width_cells;
        right_segment_bounds.push((bx as f64, seg_end as f64));
        let mut cx = bx;
        for ch in seg.text.chars() {
            if cx >= seg_end {
                break;
            }
            let w = char_cell_width(ch);
            if w == 2 {
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

    let mut slot_positions: Vec<(f64, f64)> = Vec::with_capacity(bar.tabs.len());
    let mut close_bounds: Vec<Option<(f64, f64)>> = Vec::with_capacity(bar.tabs.len());
    for _ in 0..bar.scroll_offset.min(bar.tabs.len()) {
        slot_positions.push((0.0, 0.0));
        close_bounds.push(None);
    }
    for vt in &layout.visible_tabs {
        let tab = &bar.tabs[vt.tab_idx];
        let tab_x = area.x + vt.bounds.x.round() as u16;
        let tab_w = vt.bounds.width.round() as u16;
        slot_positions.push((tab_x as f64, (tab_x + tab_w) as f64));
        // Layout carries close_bounds in primitive (cell) coords; offset by area.x.
        close_bounds.push(vt.close_bounds.map(|cb| {
            let cx = area.x as f64 + cb.x as f64;
            (cx, cx + cb.width as f64)
        }));

        let (fg, bg) = match (tab.is_active, tab.is_preview) {
            (true, true) => (preview_active_fg, active_bg),
            (true, false) => (active_fg, active_bg),
            (false, true) => (preview_inactive_fg, bar_bg),
            (false, false) => (inactive_fg, bar_bg),
        };

        // Icon glyph, if this tab has one — painted before the label in
        // its own colour, independent of the tab's active/inactive fg.
        // `tab_icon_cols` (glyph width + 1-column gap) is exactly what
        // `TuiBackend::draw_tab_bar_icons`/`tab_bar_layout_icons`
        // (backend.rs) added to this tab's measured width, so the label
        // loop below can start right after it without re-deriving the
        // reservation.
        let icon_cols = crate::primitives::tab_bar::tab_icon_cols(icons, vt.tab_idx);
        if let Some(icon) = crate::primitives::tab_bar::tab_icon_at(icons, vt.tab_idx) {
            let icon_fg = ratatui_color(icon.color);
            let icon_end = tab_x + icon_cols;
            let mut cx = tab_x;
            for ch in icon.glyph.chars() {
                if cx >= icon_end {
                    break;
                }
                let w = char_cell_width(ch);
                if w == 2 {
                    set_cell_wide(buf, cx, area.y, ch, icon_fg, bg);
                    cx += 2;
                } else {
                    set_cell(buf, cx, area.y, ch, icon_fg, bg);
                    cx += 1;
                }
            }
        }

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

        // Stride by display width, not by `char` (#554): a double-width
        // glyph (CJK, many emoji) must land in two columns, mirroring the
        // right-segment loop above. `TuiBackend::draw_tab_bar` /
        // `tab_bar_layout` (backend.rs) measure `label_end`'s budget with
        // the same `display_width` function, so the two sides agree.
        let mut x = tab_x + icon_cols;
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
            let w = char_cell_width(ch);
            if w == 2 {
                if x + 1 < label_end + 1 {
                    set_cell_wide_styled(buf, x, area.y, ch, fg, bg, cell_mod, ul);
                    x += 2;
                } else {
                    // Not enough budget left for the glyph's second column
                    // — skip rather than paint half of a wide char.
                    x += 1;
                }
            } else {
                set_cell_styled(buf, x, area.y, ch, fg, bg, cell_mod, ul);
                x += 1;
            }
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

    TabBarHits {
        slot_positions,
        close_bounds,
        right_segment_bounds,
        available_cols: tab_content_width,
        // TUI's char-based fit is exact; no correction needed.
        correct_scroll_offset: bar.scroll_offset,
    }
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
                    is_closable: true,
                },
                TabItem {
                    label: "lib.rs".into(),
                    is_active: active_idx == 1,
                    is_dirty: true,
                    is_preview: false,
                    is_closable: true,
                },
            ],
            right_segments: vec![],
            active_accent: None,
            scroll_offset: 0,
            show_tab_close: true,
            compact: false,
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
        let hits = draw_tab_bar(
            &mut buf,
            Rect::new(0, 0, 30, 1),
            &bar,
            &layout,
            &Theme::default(),
        );
        assert_eq!(hits.available_cols, 30);
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
                is_closable: true,
            }],
            right_segments: vec![TabBarSegment {
                id: Some(WidgetId::new("seg:0")),
                text: "[+]".into(),
                width_cells: 3,
                is_active: false,
            }],
            active_accent: None,
            scroll_offset: 0,
            show_tab_close: true,
            compact: false,
        };
        let layout = bar.layout(
            30.0,
            1.0,
            0.0,
            |_| TabMeasure::new(5.0, 0.0),
            |_| SegmentMeasure::new(3.0),
        );
        let hits = draw_tab_bar(
            &mut buf,
            Rect::new(0, 0, 30, 1),
            &bar,
            &layout,
            &Theme::default(),
        );
        assert_eq!(hits.available_cols, 27);
        // Right-segment bounds: 1 segment "[+]" 3 cells wide ending at col 30.
        assert_eq!(hits.right_segment_bounds.len(), 1);
        let (start, end) = hits.right_segment_bounds[0];
        assert_eq!((start, end), (27.0, 30.0));
    }

    /// Regression guard for #554: a CJK label must paint every glyph in
    /// its own columns, not lose glyphs to a flat `x += 1` stride. This
    /// reconstructs the painted row the way a real terminal (and
    /// `TuiDriver::row_cells`) reads it — stepping by each cell's own
    /// display width and skipping the blank continuation cell that
    /// `set_cell_wide`/`set_cell_wide_styled` leave after a double-width
    /// glyph — rather than concatenating every stored cell verbatim
    /// (which would hide the bug; see contract.md §2 for the ms-11 slice
    /// this mirrors at the acceptance layer).
    #[test]
    fn wide_glyph_label_paints_every_char_across_its_own_columns() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 40, 1));
        let label = "日本語.rs";
        let bar = TabBar {
            id: WidgetId::new("tabs"),
            tabs: vec![TabItem {
                label: label.into(),
                is_active: true,
                is_dirty: false,
                is_preview: false,
                is_closable: false,
            }],
            right_segments: vec![],
            active_accent: None,
            scroll_offset: 0,
            show_tab_close: false,
            compact: false,
        };
        // Measure with the same `display_width` function the fixed
        // `TuiBackend` measure side uses, so the label's budget covers
        // every column it actually paints (9: 3 wide glyphs x2 + 3 narrow).
        let width = crate::tui::display_width(label) as f32;
        assert_eq!(width, 9.0);
        let layout = bar.layout(
            40.0,
            1.0,
            0.0,
            |_| TabMeasure::new(width, 0.0),
            |_| SegmentMeasure::new(0.0),
        );
        draw_tab_bar(
            &mut buf,
            Rect::new(0, 0, 40, 1),
            &bar,
            &layout,
            &Theme::default(),
        );

        let mut rendered = String::new();
        let mut x = 0u16;
        while x < 40 {
            let sym = buf[(x, 0)].symbol();
            if sym.is_empty() {
                // Blank continuation cell of a double-width glyph — the
                // terminal doesn't read this as a separate character.
                x += 1;
                continue;
            }
            rendered.push_str(sym);
            let w = sym.chars().next().map(char_cell_width).unwrap_or(1).max(1);
            x += w;
        }

        assert!(
            rendered.starts_with(label),
            "expected {label:?} to paint intact (no dropped glyphs), got {rendered:?}"
        );
    }

    #[test]
    fn zero_size_is_a_no_op() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 1));
        let bar = make_bar(0);
        let layout = bar.layout(10.0, 1.0, 0.0, measure_tab, |_| SegmentMeasure::new(0.0));
        // Zero-width area: function must return defaults without panicking.
        let hits = draw_tab_bar(
            &mut buf,
            Rect::new(0, 0, 0, 1),
            &bar,
            &layout,
            &Theme::default(),
        );
        assert_eq!(hits.available_cols, 0);
        assert!(hits.slot_positions.is_empty());
    }

    /// #620: an icon glyph paints at the tab's leading edge in its own
    /// [`crate::TabIcon::color`], and the label starts right after it +
    /// the 1-column gap [`crate::tab_icon_cols`] reserves — matching what
    /// `TuiBackend::draw_tab_bar_icons`/`tab_bar_layout_icons`
    /// (backend.rs) add to the tab's measured width.
    #[test]
    fn icon_glyph_paints_before_label_and_widens_measured_tab() {
        use crate::primitives::tab_bar::TabIcon;
        use crate::types::Color;

        let icon = TabIcon {
            glyph: "\u{f09b}".into(), // single-column PUA glyph
            color: Color::rgb(240, 150, 60),
        };
        let icons = vec![Some(icon.clone())];
        let bar = TabBar {
            id: WidgetId::new("tabs"),
            tabs: vec![TabItem {
                label: "main.rs".into(),
                is_active: true,
                is_closable: false,
                ..Default::default()
            }],
            right_segments: vec![],
            active_accent: None,
            scroll_offset: 0,
            show_tab_close: false,
            compact: false,
        };

        let icon_cols = crate::tab_icon_cols(&icons, 0);
        assert_eq!(
            icon_cols, 2,
            "1-column glyph + 1-column gap before the label"
        );

        let label_w = crate::text_util::display_width(&bar.tabs[0].label) as f32;

        let mut buf = Buffer::empty(Rect::new(0, 0, 40, 1));
        let layout = bar.layout(
            40.0,
            1.0,
            0.0,
            |_| TabMeasure::new(icon_cols as f32 + label_w, 0.0),
            |_| SegmentMeasure::new(0.0),
        );
        draw_tab_bar_icons(
            &mut buf,
            Rect::new(0, 0, 40, 1),
            &bar,
            &icons,
            &layout,
            &Theme::default(),
        );

        assert_eq!(
            cell_char(&buf, 0, 0),
            '\u{f09b}',
            "icon glyph should paint at the tab's leading column"
        );
        assert_eq!(
            buf[(0, 0)].fg,
            ratatui_color(icon.color),
            "icon glyph should paint in TabIcon::color"
        );
        assert_eq!(
            cell_char(&buf, icon_cols, 0),
            'm',
            "label should start right after icon_cols (glyph + 1-column gap)"
        );
    }

    /// #620: an empty icon sidecar must paint exactly what the icon-less
    /// [`draw_tab_bar`] entry point paints — that equivalence is what
    /// lets every backend route both entry points through one rasteriser
    /// without changing a single existing pixel.
    #[test]
    fn empty_icon_sidecar_paints_identically_to_draw_tab_bar() {
        let bar = make_bar(0);
        let area = Rect::new(0, 0, 40, 1);
        let layout = bar.layout(40.0, 1.0, 0.0, measure_tab, |_| SegmentMeasure::new(0.0));

        let mut plain = Buffer::empty(area);
        let plain_hits = draw_tab_bar(&mut plain, area, &bar, &layout, &Theme::default());

        let mut sidecar = Buffer::empty(area);
        let sidecar_hits =
            draw_tab_bar_icons(&mut sidecar, area, &bar, &[], &layout, &Theme::default());

        assert_eq!(plain, sidecar, "empty sidecar must not change any cell");
        assert_eq!(
            plain_hits.slot_positions, sidecar_hits.slot_positions,
            "empty sidecar must not move any tab slot"
        );
        assert_eq!(
            plain_hits.close_bounds, sidecar_hits.close_bounds,
            "empty sidecar must not move any close button"
        );
    }
}
