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
        DiffMode::Unified => draw_unified(buf, area, view, theme, total_rows),
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

fn draw_unified(
    buf: &mut Buffer,
    area: Rect,
    view: &DiffView,
    theme: &Theme,
    total_rows: usize,
) -> DiffViewLayout {
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

    DiffViewLayout {
        visible_rows,
        total_rows,
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
