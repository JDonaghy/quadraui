//! Cell painter for [`DiffView`], written against [`CellSurface`].
//!
//! Paints the rows of an already-computed [`DiffViewGeometry`]: each
//! side-by-side row as two tinted panes split by a `│` divider, each
//! unified row as a prefix column plus text, and blank rows below the
//! content in the panel background.

use crate::cell_surface::{solid_style, CellSurface};
use crate::event::Rect;
use crate::primitives::diff_view::{
    row_colors, unified_hunk_header, unified_row_style, unified_row_text, DiffLineContent,
    DiffMode, DiffView, DiffViewGeometry,
};
use crate::theme::Theme;
use crate::types::Color;

/// A geometry rect as whole cells.
struct CellRect {
    x: u16,
    y: u16,
    width: u16,
}

fn cell_rect(r: Rect) -> CellRect {
    CellRect {
        x: r.x as u16,
        y: r.y as u16,
        width: r.width as u16,
    }
}

/// Paints `view` over `geometry`, which [`DiffView::layout`] computed at
/// one cell per row.
pub(crate) fn paint(
    cells: &mut dyn CellSurface,
    view: &DiffView,
    geometry: &DiffViewGeometry,
    theme: &Theme,
) {
    let area = geometry.bounds;
    if area.width < 1.0 || area.height < 1.0 {
        return;
    }
    match view.mode {
        DiffMode::SideBySide => paint_side_by_side(cells, view, theme, geometry),
        DiffMode::Unified => paint_unified(cells, view, theme, geometry),
    }
}

fn paint_side_by_side(
    cells: &mut dyn CellSurface,
    view: &DiffView,
    theme: &Theme,
    geometry: &DiffViewGeometry,
) {
    let flat = view.flat_rows();

    if let Some(header) = &geometry.header {
        let (hdr_fg, hdr_bg) = (theme.header_fg, theme.header_bg);
        let left_r = cell_rect(header.left);
        let right_r = cell_rect(header.right);
        let divider_r = cell_rect(header.divider);

        blank_run(cells, left_r.x, left_r.y, left_r.width, hdr_fg, hdr_bg);
        blank_run(cells, right_r.x, right_r.y, right_r.width, hdr_fg, hdr_bg);
        cells.put_char(
            divider_r.x,
            divider_r.y,
            '│',
            solid_style(theme.border_fg, hdr_bg),
        );
        if let Some(label) = &view.left_label {
            text_in(
                cells,
                left_r.x,
                left_r.y,
                left_r.width,
                label,
                hdr_fg,
                hdr_bg,
            );
        }
        if let Some(label) = &view.right_label {
            text_in(
                cells,
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

        let left_r = cell_rect(line.left.expect("side-by-side row has left bounds"));
        let right_r = cell_rect(line.right.expect("side-by-side row has right bounds"));
        let divider_r = cell_rect(line.divider.expect("side-by-side row has divider bounds"));

        blank_run(cells, left_r.x, left_r.y, left_r.width, left_fg, left_bg);
        blank_run(
            cells,
            right_r.x,
            right_r.y,
            right_r.width,
            right_fg,
            right_bg,
        );
        cells.put_char(
            divider_r.x,
            divider_r.y,
            '│',
            solid_style(theme.border_fg, left_bg),
        );
        if let Some(text) = &row.left {
            text_in(
                cells,
                left_r.x,
                left_r.y,
                left_r.width,
                text,
                left_fg,
                left_bg,
            );
        }
        if let Some(text) = &row.right {
            text_in(
                cells,
                right_r.x,
                right_r.y,
                right_r.width,
                text,
                right_fg,
                right_bg,
            );
        }
    }

    // Rows below the last display line keep the pane split visible.
    if let Some(panes) = &geometry.panes {
        let area_x = geometry.bounds.x as u16;
        let area_y = geometry.bounds.y as u16;
        let area_h = geometry.bounds.height as u16;
        let header_rows: u16 = if geometry.header.is_some() { 1 } else { 0 };
        let content_area_h = area_h.saturating_sub(header_rows);
        let divider_x = panes.divider_x as u16;
        let left_w = panes.left_w as u16;
        let right_w = panes.right_w as u16;
        let rows_drawn = geometry.lines.len() as u16;
        let (fg, bg) = (theme.muted_fg, theme.background);
        for blank_row in rows_drawn..content_area_h {
            let screen_y = area_y + header_rows + blank_row;
            blank_run(cells, area_x, screen_y, left_w, fg, bg);
            cells.put_char(divider_x, screen_y, '│', solid_style(theme.border_fg, bg));
            blank_run(cells, divider_x + 1, screen_y, right_w, fg, bg);
        }
    }
}

fn paint_unified(
    cells: &mut dyn CellSurface,
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
                let (fg, bg) = (theme.accent_fg, theme.background);
                blank_run(cells, screen.x, screen.y, screen.width, fg, bg);
                text_in(
                    cells,
                    screen.x,
                    screen.y,
                    screen.width,
                    &header_text,
                    fg,
                    bg,
                );
            }
            DiffLineContent::Row { row_idx } => {
                let row = flat[row_idx];
                let (prefix, fg, bg) = unified_row_style(row.kind, theme);
                blank_run(cells, screen.x, screen.y, screen.width, fg, bg);
                if screen.width > 0 {
                    cells.put_char(screen.x, screen.y, prefix, solid_style(fg, bg));
                }
                if screen.width > 1 {
                    text_in(
                        cells,
                        screen.x + 1,
                        screen.y,
                        screen.width.saturating_sub(1),
                        unified_row_text(row),
                        fg,
                        bg,
                    );
                }
            }
        }
    }

    let area_x = geometry.bounds.x as u16;
    let area_y = geometry.bounds.y as u16;
    let area_w = geometry.bounds.width as u16;
    let area_h = geometry.bounds.height as u16;
    let rows_drawn = geometry.lines.len() as u16;
    for blank in rows_drawn..area_h {
        blank_run(
            cells,
            area_x,
            area_y + blank,
            area_w,
            theme.muted_fg,
            theme.background,
        );
    }
}

/// `width` blank cells from `(x, y)` in `bg`.
fn blank_run(cells: &mut dyn CellSurface, x: u16, y: u16, width: u16, fg: Color, bg: Color) {
    for col in 0..width {
        cells.put_char(x + col, y, ' ', solid_style(fg, bg));
    }
}

/// `text` from `(x, y)`, one char per cell, cut at `max_width` cells.
fn text_in(
    cells: &mut dyn CellSurface,
    x: u16,
    y: u16,
    max_width: u16,
    text: &str,
    fg: Color,
    bg: Color,
) {
    for (col, ch) in (0_u16..).zip(text.chars()) {
        if col >= max_width {
            break;
        }
        cells.put_char(x + col, y, ch, solid_style(fg, bg));
    }
}
