//! TUI rasteriser for [`crate::primitives::diff_view::DiffView`].
//!
//! Paints a two-pane (side-by-side) or single-column (unified) diff onto a
//! [`ratatui::buffer::Buffer`]. Row backgrounds are driven by
//! [`DiffRowKind`]:
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
use crate::primitives::diff_view::{DiffMode, DiffRowKind, DiffView, DiffViewLayout};
use crate::theme::Theme;

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

    let total_rows = view.total_rows();

    match view.mode {
        DiffMode::SideBySide => draw_side_by_side(buf, area, view, theme, total_rows),
        DiffMode::Unified => draw_unified(buf, area, view, theme),
    }
}

// ── Side-by-side ─────────────────────────────────────────────────────────────

fn draw_side_by_side(
    buf: &mut Buffer,
    area: Rect,
    view: &DiffView,
    theme: &Theme,
    total_rows: usize,
) -> DiffViewLayout {
    let has_header = view.left_label.is_some() || view.right_label.is_some();
    let header_rows: u16 = if has_header { 1 } else { 0 };

    let left_w = (area.width.saturating_sub(1)) / 2;
    let right_w = area.width.saturating_sub(1).saturating_sub(left_w);
    let divider_x = area.x + left_w;

    // Draw header row.
    if has_header {
        let hy = area.y;
        let hdr_bg = qc(theme.header_bg);
        let hdr_fg = qc(theme.header_fg);

        // Fill left header.
        for col in 0..left_w {
            set_cell(buf, area.x + col, hy, ' ', hdr_fg, hdr_bg);
        }
        // Fill right header.
        for col in 0..right_w {
            set_cell(buf, divider_x + 1 + col, hy, ' ', hdr_fg, hdr_bg);
        }
        // Divider in header.
        set_cell(buf, divider_x, hy, '│', qc(theme.border_fg), hdr_bg);

        // Draw labels (truncated to fit).
        if let Some(label) = &view.left_label {
            draw_text_in(buf, area.x, hy, left_w, label, hdr_fg, hdr_bg);
        }
        if let Some(label) = &view.right_label {
            draw_text_in(buf, divider_x + 1, hy, right_w, label, hdr_fg, hdr_bg);
        }
    }

    let content_area_h = area.height.saturating_sub(header_rows);
    let visible_rows = content_area_h as usize;

    // Collect all rows across hunks into a flat view.
    let all_rows: Vec<_> = view.hunks.iter().flat_map(|h| h.rows.iter()).collect();

    let start = view.scroll_offset.min(total_rows.saturating_sub(1));
    let end = (start + visible_rows).min(total_rows);

    for (row_idx, row) in all_rows.iter().enumerate().skip(start).take(end - start) {
        let screen_y = area.y + header_rows + (row_idx - start) as u16;
        if screen_y >= area.y + area.height {
            break;
        }

        let (left_fg, left_bg, right_fg, right_bg) = row_colors(row.kind, theme);

        // Fill left pane.
        for col in 0..left_w {
            set_cell(buf, area.x + col, screen_y, ' ', left_fg, left_bg);
        }
        // Fill right pane.
        for col in 0..right_w {
            set_cell(buf, divider_x + 1 + col, screen_y, ' ', right_fg, right_bg);
        }
        // Divider.
        set_cell(buf, divider_x, screen_y, '│', qc(theme.border_fg), left_bg);

        // Draw content text.
        if let Some(text) = &row.left {
            draw_text_in(buf, area.x, screen_y, left_w, text, left_fg, left_bg);
        }
        if let Some(text) = &row.right {
            draw_text_in(
                buf,
                divider_x + 1,
                screen_y,
                right_w,
                text,
                right_fg,
                right_bg,
            );
        }
    }

    // Fill any trailing empty rows.
    let rows_drawn = (end - start) as u16;
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

    DiffViewLayout {
        visible_rows,
        total_rows,
    }
}

// ── Unified ───────────────────────────────────────────────────────────────────

fn draw_unified(buf: &mut Buffer, area: Rect, view: &DiffView, theme: &Theme) -> DiffViewLayout {
    // In unified mode every hunk contributes its rows plus a `@@` header line.
    // We build a flat list of "lines to display" (header or content).
    #[derive(Clone)]
    enum UnifiedLine<'a> {
        Header(String),
        Content(&'a crate::primitives::diff_view::DiffRow),
    }

    let mut lines: Vec<UnifiedLine<'_>> = Vec::new();
    for hunk in &view.hunks {
        let header = format!(
            "@@ -{},{} +{},{} @@",
            hunk.left_start,
            hunk.rows.len(),
            hunk.right_start,
            hunk.rows.len()
        );
        lines.push(UnifiedLine::Header(header));
        for row in &hunk.rows {
            lines.push(UnifiedLine::Content(row));
        }
    }

    let total_display = lines.len();
    let visible_rows = area.height as usize;
    let start = view.scroll_offset.min(total_display.saturating_sub(1));
    let end = (start + visible_rows).min(total_display);

    for (i, line) in lines.iter().enumerate().skip(start).take(end - start) {
        let screen_y = area.y + (i - start) as u16;
        if screen_y >= area.y + area.height {
            break;
        }

        match line {
            UnifiedLine::Header(h) => {
                let bg = qc(theme.background);
                let fg = qc(theme.accent_fg);
                for col in 0..area.width {
                    set_cell(buf, area.x + col, screen_y, ' ', fg, bg);
                }
                draw_text_in(buf, area.x, screen_y, area.width, h, fg, bg);
            }
            UnifiedLine::Content(row) => {
                let (prefix, fg, bg) = match row.kind {
                    DiffRowKind::Same => (' ', qc(theme.muted_fg), qc(theme.background)),
                    DiffRowKind::Removed | DiffRowKind::Changed => {
                        ('-', qc(theme.git_deleted), qc(theme.diff_removed_bg))
                    }
                    DiffRowKind::Added => ('+', qc(theme.git_added), qc(theme.diff_added_bg)),
                };

                // Fill row background.
                for col in 0..area.width {
                    set_cell(buf, area.x + col, screen_y, ' ', fg, bg);
                }

                // Prefix character.
                if area.width > 0 {
                    set_cell(buf, area.x, screen_y, prefix, fg, bg);
                }

                // Content text (shifted right by 1 for the prefix).
                let text = match row.kind {
                    DiffRowKind::Removed => row.left.as_deref().unwrap_or(""),
                    _ => row.right.as_deref().or(row.left.as_deref()).unwrap_or(""),
                };
                if area.width > 1 {
                    draw_text_in(
                        buf,
                        area.x + 1,
                        screen_y,
                        area.width.saturating_sub(1),
                        text,
                        fg,
                        bg,
                    );
                }
            }
        }
    }

    // Fill trailing blank rows.
    let rows_drawn = (end - start) as u16;
    for blank in rows_drawn..area.height {
        let screen_y = area.y + blank;
        let bg = qc(theme.background);
        let fg = qc(theme.muted_fg);
        for col in 0..area.width {
            set_cell(buf, area.x + col, screen_y, ' ', fg, bg);
        }
    }

    // Return total_display (content rows + one @@ header per hunk) so
    // callers can clamp scroll_offset correctly in unified mode.
    DiffViewLayout {
        visible_rows,
        total_rows: total_display,
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Background and foreground colours for each pane given a row kind.
fn row_colors(
    kind: DiffRowKind,
    theme: &Theme,
) -> (
    ratatui::style::Color,
    ratatui::style::Color,
    ratatui::style::Color,
    ratatui::style::Color,
) {
    match kind {
        DiffRowKind::Same => (
            qc(theme.muted_fg),
            qc(theme.background),
            qc(theme.muted_fg),
            qc(theme.background),
        ),
        DiffRowKind::Changed => (
            qc(theme.git_deleted),
            qc(theme.diff_removed_bg),
            qc(theme.git_added),
            qc(theme.diff_added_bg),
        ),
        DiffRowKind::Removed => (
            qc(theme.git_deleted),
            qc(theme.diff_removed_bg),
            qc(theme.muted_fg),
            qc(theme.diff_padding_bg),
        ),
        DiffRowKind::Added => (
            qc(theme.muted_fg),
            qc(theme.diff_padding_bg),
            qc(theme.git_added),
            qc(theme.diff_added_bg),
        ),
    }
}

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
}
