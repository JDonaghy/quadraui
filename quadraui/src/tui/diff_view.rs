//! TUI rasteriser for [`crate::primitives::diff_view::DiffView`].
//!
//! Paints a two-pane (side-by-side) or single-column (unified) diff onto a
//! [`ratatui::buffer::Buffer`]. Row/pane geometry and the scroll-clamped
//! visible-line window come from [`DiffView::layout`] — the shared layout
//! API lifted out of three near-identical backend copies (issue #737).
//! This module only converts the resulting DIP-agnostic `f32` geometry to
//! cell coordinates and paints; it does not re-derive positions.
//!
//! Row backgrounds are driven by [`DiffRowKind`] via
//! [`crate::primitives::diff_view::row_colors`] (side-by-side) /
//! [`crate::primitives::diff_view::unified_row_style`] (unified) — also
//! shared, not a third copy of the colour table:
//!
//! | Kind      | Left bg             | Right bg            |
//! |-----------|---------------------|---------------------|
//! | Same      | `theme.background`  | `theme.background`  |
//! | Changed   | `diff_removed_bg`   | `diff_added_bg`     |
//! | Removed   | `diff_removed_bg`   | `diff_padding_bg`   |
//! | Added     | `diff_padding_bg`   | `diff_added_bg`     |
//!
//! The divider column (`│`) is drawn in `theme.border_fg`.
//!
//! # Unified mode
//!
//! A `@@ -l,n +r,m @@` header is drawn before each hunk using
//! `theme.accent_fg`. Removed lines get a `-` prefix in `theme.git_deleted`;
//! added lines a `+` prefix in `theme.git_added`; same lines a ` ` prefix in
//! `theme.muted_fg`.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::{qc, set_cell};
use crate::event::Rect as ERect;
use crate::primitives::diff_view::{
    row_colors, unified_hunk_header, unified_row_style, unified_row_text, DiffLineContent,
    DiffMode, DiffView, DiffViewGeometry, DiffViewLayout,
};
use crate::theme::Theme;

/// Convert an `f32` DIP-agnostic rect from [`DiffView::layout`] to cell
/// coordinates. Every input is exact-integer-valued by construction
/// (`line_height` is always `1.0` on TUI and every viewport dimension
/// starts as a whole cell count), so truncating casts never lose
/// precision.
fn cell_rect(r: ERect) -> Rect {
    Rect::new(r.x as u16, r.y as u16, r.width as u16, r.height as u16)
}

/// Draw a [`DiffView`] into `area` on `buf`.
///
/// Returns [`DiffViewLayout`] so the caller can clamp `scroll_offset` after
/// a resize:
/// ```ignore
/// let layout = backend.draw_diff_view(rect, &view);
/// view.scroll_offset = view.scroll_offset
///     .min(layout.total_rows.saturating_sub(layout.visible_rows));
/// ```
pub fn draw_diff_view(
    buf: &mut Buffer,
    area: Rect,
    view: &DiffView,
    theme: &Theme,
) -> DiffViewLayout {
    if area.width == 0 || area.height == 0 {
        return DiffViewLayout {
            visible_rows: 0,
            total_rows: view.total_rows(),
        };
    }

    let viewport = ERect::new(
        area.x as f32,
        area.y as f32,
        area.width as f32,
        area.height as f32,
    );
    let geometry = view.layout(viewport, 1.0);

    match view.mode {
        DiffMode::SideBySide => draw_side_by_side(buf, area, view, theme, &geometry),
        DiffMode::Unified => draw_unified(buf, area, view, theme, &geometry),
    }

    geometry.as_layout()
}

// ── Side-by-side ─────────────────────────────────────────────────────────────

fn draw_side_by_side(
    buf: &mut Buffer,
    area: Rect,
    view: &DiffView,
    theme: &Theme,
    geometry: &DiffViewGeometry,
) {
    let flat = view.flat_rows();

    // Header row.
    if let Some(header) = &geometry.header {
        let hdr_bg = qc(theme.header_bg);
        let hdr_fg = qc(theme.header_fg);

        let left_r = cell_rect(header.left);
        let right_r = cell_rect(header.right);
        let divider_r = cell_rect(header.divider);

        for col in 0..left_r.width {
            set_cell(buf, left_r.x + col, left_r.y, ' ', hdr_fg, hdr_bg);
        }
        for col in 0..right_r.width {
            set_cell(buf, right_r.x + col, right_r.y, ' ', hdr_fg, hdr_bg);
        }
        set_cell(
            buf,
            divider_r.x,
            divider_r.y,
            '│',
            qc(theme.border_fg),
            hdr_bg,
        );

        if let Some(label) = &view.left_label {
            draw_text_in(buf, left_r.x, left_r.y, left_r.width, label, hdr_fg, hdr_bg);
        }
        if let Some(label) = &view.right_label {
            draw_text_in(
                buf,
                right_r.x,
                right_r.y,
                right_r.width,
                label,
                hdr_fg,
                hdr_bg,
            );
        }
    }

    for line in &geometry.lines {
        let DiffLineContent::Row { row_idx } = line.content else {
            continue;
        };
        let row = flat[row_idx];
        let (left_fg, left_bg, right_fg, right_bg) = row_colors(row.kind, theme);
        let (left_fg, left_bg, right_fg, right_bg) =
            (qc(left_fg), qc(left_bg), qc(right_fg), qc(right_bg));

        let left_r = cell_rect(line.left.expect("side-by-side row has left bounds"));
        let right_r = cell_rect(line.right.expect("side-by-side row has right bounds"));
        let divider_r = cell_rect(line.divider.expect("side-by-side row has divider bounds"));

        for col in 0..left_r.width {
            set_cell(buf, left_r.x + col, left_r.y, ' ', left_fg, left_bg);
        }
        for col in 0..right_r.width {
            set_cell(buf, right_r.x + col, right_r.y, ' ', right_fg, right_bg);
        }
        set_cell(
            buf,
            divider_r.x,
            divider_r.y,
            '│',
            qc(theme.border_fg),
            left_bg,
        );

        if let Some(text) = &row.left {
            draw_text_in(
                buf,
                left_r.x,
                left_r.y,
                left_r.width,
                text,
                left_fg,
                left_bg,
            );
        }
        if let Some(text) = &row.right {
            draw_text_in(
                buf,
                right_r.x,
                right_r.y,
                right_r.width,
                text,
                right_fg,
                right_bg,
            );
        }
    }

    // Fill any trailing empty rows (fewer display rows than fit on screen).
    if let Some(panes) = &geometry.panes {
        let header_rows: u16 = if geometry.header.is_some() { 1 } else { 0 };
        let content_area_h = area.height.saturating_sub(header_rows);
        let divider_x = panes.divider_x as u16;
        let left_w = panes.left_w as u16;
        let right_w = panes.right_w as u16;
        let rows_drawn = geometry.lines.len() as u16;
        for blank_row in rows_drawn..content_area_h {
            let screen_y = area.y + header_rows + blank_row;
            let bg = qc(theme.background);
            let fg = qc(theme.muted_fg);
            for col in 0..left_w {
                set_cell(buf, area.x + col, screen_y, ' ', fg, bg);
            }
            set_cell(buf, divider_x, screen_y, '│', qc(theme.border_fg), bg);
            for col in 0..right_w {
                set_cell(buf, divider_x + 1 + col, screen_y, ' ', fg, bg);
            }
        }
    }
}

// ── Unified ───────────────────────────────────────────────────────────────────

fn draw_unified(
    buf: &mut Buffer,
    area: Rect,
    view: &DiffView,
    theme: &Theme,
    geometry: &DiffViewGeometry,
) {
    let flat = view.flat_rows();

    for line in &geometry.lines {
        let screen = cell_rect(line.bounds);

        match line.content {
            DiffLineContent::UnifiedHeader { hunk_idx } => {
                let header_text = unified_hunk_header(&view.hunks[hunk_idx]);
                let bg = qc(theme.background);
                let fg = qc(theme.accent_fg);
                for col in 0..screen.width {
                    set_cell(buf, screen.x + col, screen.y, ' ', fg, bg);
                }
                draw_text_in(buf, screen.x, screen.y, screen.width, &header_text, fg, bg);
            }
            DiffLineContent::Row { row_idx } => {
                let row = flat[row_idx];
                let (prefix, fg, bg) = unified_row_style(row.kind, theme);
                let (fg, bg) = (qc(fg), qc(bg));

                // Fill row background.
                for col in 0..screen.width {
                    set_cell(buf, screen.x + col, screen.y, ' ', fg, bg);
                }

                // Prefix character.
                if screen.width > 0 {
                    set_cell(buf, screen.x, screen.y, prefix, fg, bg);
                }

                // Content text (shifted right by 1 for the prefix).
                let text = unified_row_text(row);
                if screen.width > 1 {
                    draw_text_in(
                        buf,
                        screen.x + 1,
                        screen.y,
                        screen.width.saturating_sub(1),
                        text,
                        fg,
                        bg,
                    );
                }
            }
        }
    }

    // Fill trailing blank rows.
    let rows_drawn = geometry.lines.len() as u16;
    for blank in rows_drawn..area.height {
        let screen_y = area.y + blank;
        let bg = qc(theme.background);
        let fg = qc(theme.muted_fg);
        for col in 0..area.width {
            set_cell(buf, area.x + col, screen_y, ' ', fg, bg);
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Write `text` into `buf` starting at `(x, y)`, limited to `max_width`
/// cells.  Characters beyond the limit are silently dropped.
fn draw_text_in(
    buf: &mut Buffer,
    x: u16,
    y: u16,
    max_width: u16,
    text: &str,
    fg: ratatui::style::Color,
    bg: ratatui::style::Color,
) {
    for (col, ch) in (0_u16..).zip(text.chars()) {
        if col >= max_width {
            break;
        }
        set_cell(buf, x + col, y, ch, fg, bg);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::diff_view::{DiffEditability, DiffHunk, DiffPane, DiffRow, DiffRowKind};
    use crate::types::WidgetId;

    fn cell_char(buf: &Buffer, x: u16, y: u16) -> char {
        buf[(x, y)].symbol().chars().next().unwrap_or(' ')
    }

    fn cell_row_str(buf: &Buffer, y: u16, width: u16) -> String {
        (0..width).map(|x| cell_char(buf, x, y)).collect()
    }

    /// Build a minimal `DiffView` with two hunks (2 rows each) in the given mode.
    fn make_view(mode: DiffMode) -> DiffView {
        DiffView {
            id: WidgetId::new("test"),
            left: String::new(),
            right: String::new(),
            left_label: None,
            right_label: None,
            hunks: vec![
                DiffHunk {
                    left_start: 1,
                    right_start: 1,
                    rows: vec![
                        DiffRow {
                            left: Some("alpha".into()),
                            right: Some("ALPHA".into()),
                            kind: DiffRowKind::Changed,
                        },
                        DiffRow {
                            left: Some("beta".into()),
                            right: Some("beta".into()),
                            kind: DiffRowKind::Same,
                        },
                    ],
                },
                DiffHunk {
                    left_start: 10,
                    right_start: 10,
                    rows: vec![
                        DiffRow {
                            left: Some("gamma".into()),
                            right: None,
                            kind: DiffRowKind::Removed,
                        },
                        DiffRow {
                            left: None,
                            right: Some("delta".into()),
                            kind: DiffRowKind::Added,
                        },
                    ],
                },
            ],
            mode,
            editability: DiffEditability::ReadOnly,
            scroll_offset: 0,
            focused_pane: DiffPane::Left,
            has_focus: false,
        }
    }

    // ── Zero-size guard ───────────────────────────────────────────────────────

    /// Zero-width area must not panic and must return empty layout.
    #[test]
    fn zero_size_area_side_by_side_returns_empty_layout_without_panic() {
        let buf_area = Rect::new(0, 0, 40, 10);
        let mut buf = Buffer::empty(buf_area);
        let view = make_view(DiffMode::SideBySide);
        let layout = draw_diff_view(&mut buf, Rect::new(0, 0, 0, 0), &view, &Theme::default());
        assert_eq!(layout.visible_rows, 0);
        assert_eq!(layout.total_rows, view.total_rows());
    }

    /// Zero-height area in unified mode must not panic and must return empty layout.
    #[test]
    fn zero_size_area_unified_returns_empty_layout_without_panic() {
        let buf_area = Rect::new(0, 0, 40, 10);
        let mut buf = Buffer::empty(buf_area);
        let view = make_view(DiffMode::Unified);
        let layout = draw_diff_view(&mut buf, Rect::new(0, 0, 0, 0), &view, &Theme::default());
        assert_eq!(layout.visible_rows, 0);
        assert_eq!(layout.total_rows, view.total_rows());
    }

    // ── Side-by-side scroll ───────────────────────────────────────────────────

    /// With `scroll_offset = 0`, the first row's left text starts on screen row 0.
    #[test]
    fn scroll_offset_zero_paints_first_row_at_top() {
        let area = Rect::new(0, 0, 20, 5);
        let mut buf = Buffer::empty(area);
        let view = make_view(DiffMode::SideBySide);
        draw_diff_view(&mut buf, area, &view, &Theme::default());

        // "alpha" should appear somewhere on row 0 (the left pane).
        let row0 = cell_row_str(&buf, 0, area.width);
        assert!(
            row0.contains("alpha"),
            "expected 'alpha' on row 0, got: {row0:?}"
        );
    }

    /// With `scroll_offset = 1`, the second row's left text starts on screen row 0.
    #[test]
    fn scroll_offset_one_paints_second_row_at_top() {
        let area = Rect::new(0, 0, 20, 5);
        let mut buf = Buffer::empty(area);
        let mut view = make_view(DiffMode::SideBySide);
        view.scroll_offset = 1;
        draw_diff_view(&mut buf, area, &view, &Theme::default());

        // Row 1 is "beta" — it should now be on screen row 0.
        let row0 = cell_row_str(&buf, 0, area.width);
        assert!(
            row0.contains("beta"),
            "expected 'beta' on row 0 at offset=1, got: {row0:?}"
        );
        // "alpha" (offset=0 row) must NOT appear.
        assert!(
            !row0.contains("alpha"),
            "unexpected 'alpha' on row 0 at offset=1: {row0:?}"
        );
    }

    // ── Unified mode ──────────────────────────────────────────────────────────

    /// In unified mode, the `@@` header for the first hunk appears on screen row 0.
    #[test]
    fn unified_hunk_header_appears_before_first_row() {
        let area = Rect::new(0, 0, 30, 6);
        let mut buf = Buffer::empty(area);
        let view = make_view(DiffMode::Unified);
        draw_diff_view(&mut buf, area, &view, &Theme::default());

        let row0 = cell_row_str(&buf, 0, area.width);
        assert!(
            row0.contains("@@"),
            "expected '@@ header' on row 0 in unified mode, got: {row0:?}"
        );
    }

    /// In unified mode, `layout.total_rows` counts hunk headers too, so
    /// scrolling to the last offset exposes the final content row.
    ///
    /// This is the regression guard for review finding #1: `total_rows` was
    /// previously `view.total_rows()` (content only) instead of
    /// `total_display` (content + headers), making the last N rows
    /// unreachable.
    #[test]
    fn unified_scroll_reaches_last_row() {
        // 2 hunks × 2 rows = 4 content rows + 2 headers = 6 display lines.
        // With visible_rows = 2, max valid offset = 6 - 2 = 4.
        let area = Rect::new(0, 0, 30, 2);
        let mut buf = Buffer::empty(area);
        let view = make_view(DiffMode::Unified);

        // First pass: capture the correct total from the layout.
        let layout = draw_diff_view(&mut buf, area, &view, &Theme::default());
        assert_eq!(layout.total_rows, 6, "2 hunks × 2 rows + 2 headers = 6");
        assert_eq!(layout.visible_rows, 2);

        // Scroll to the maximum valid offset and verify last content is visible.
        let max_offset = layout.total_rows.saturating_sub(layout.visible_rows);
        let mut buf2 = Buffer::empty(area);
        let mut view2 = make_view(DiffMode::Unified);
        view2.scroll_offset = max_offset;
        draw_diff_view(&mut buf2, area, &view2, &Theme::default());

        // The last display line is the "delta" Added row (row index 5).
        // At offset = 4, screen row 1 should show "delta".
        let row1 = cell_row_str(&buf2, 1, area.width);
        assert!(
            row1.contains("delta"),
            "expected 'delta' on screen row 1 at max_offset={max_offset}, got: {row1:?}"
        );
    }

    /// Regression guard for the unified `@@` header line counts. A hunk
    /// with 2 removed + 1 added (one padding row) must emit
    /// `@@ -L,2 +R,1 @@`, not `@@ -L,3 +R,3 @@`. The previous
    /// implementation used `hunk.rows.len()` for both numbers, which
    /// inflated both counts whenever padding rows were present.
    #[test]
    fn unified_header_counts_exclude_padding() {
        let view = DiffView {
            id: WidgetId::new("hdr"),
            left: String::new(),
            right: String::new(),
            left_label: None,
            right_label: None,
            hunks: vec![crate::primitives::diff_view::DiffHunk {
                left_start: 5,
                right_start: 7,
                rows: vec![
                    // 2 removed + 1 added = 3 aligned rows (1 padding).
                    DiffRow {
                        left: Some("old1".into()),
                        right: Some("new1".into()),
                        kind: DiffRowKind::Changed,
                    },
                    DiffRow {
                        left: Some("old2".into()),
                        right: None,
                        kind: DiffRowKind::Removed,
                    },
                    DiffRow {
                        left: None,
                        right: Some("new3".into()),
                        kind: DiffRowKind::Added,
                    },
                ],
            }],
            mode: DiffMode::Unified,
            editability: DiffEditability::ReadOnly,
            scroll_offset: 0,
            focused_pane: DiffPane::Left,
            has_focus: false,
        };
        // Left rows = 2 (Changed has left + Removed has left).
        // Right rows = 2 (Changed has right + Added has right).
        let area = Rect::new(0, 0, 40, 4);
        let mut buf = Buffer::empty(area);
        draw_diff_view(&mut buf, area, &view, &Theme::default());

        let row0 = cell_row_str(&buf, 0, area.width);
        assert!(
            row0.contains("@@ -5,2 +7,2 @@"),
            "expected header '@@ -5,2 +7,2 @@', got: {row0:?}"
        );
        // The buggy header would have read "@@ -5,3 +7,3 @@".
        assert!(
            !row0.contains("@@ -5,3 +7,3 @@"),
            "header still reports raw row count: {row0:?}"
        );
    }
}
