//! Pure cursor/selection/scroll arithmetic for a rectangular `row_count` x
//! `col_count` grid — the geometry [`crate::compose::CellGridController`]
//! and [`crate::primitives::cell_grid::CellGrid`]'s [`crate::interactive::Interactive`]
//! impl both walk, kept in exactly one place instead of hand-copied into
//! each caller.
//!
//! Every function here takes its inputs by value and returns the new
//! value(s) with no side effects — neither caller's own state (cursor,
//! selection anchor, scroll offsets) is mutated here. The controller
//! applies the result directly to its own fields; the primitive wraps it
//! into a [`crate::primitives::cell_grid::CellGridEvent::Navigated`] for a
//! host to apply instead, since [`crate::interactive::Interactive::handle`]
//! only ever gets `&self`.

use crate::behaviour::scroll::ensure_visible;
use crate::primitives::scrollbar::scroll_by_clamped;

/// Clamp `(row, col)` into `[0, row_count) x [0, col_count)` — saturates to
/// the last valid index on either axis rather than panicking on an empty
/// sheet.
pub(crate) fn clamp_cell(
    row_count: usize,
    col_count: usize,
    (row, col): (usize, usize),
) -> (usize, usize) {
    (
        row.min(row_count.saturating_sub(1)),
        col.min(col_count.saturating_sub(1)),
    )
}

/// Cursor moved by `(d_row, d_col)`, clamped into the sheet.
pub(crate) fn move_by(
    row_count: usize,
    col_count: usize,
    (row, col): (usize, usize),
    d_row: isize,
    d_col: isize,
) -> (usize, usize) {
    let nr = (row as isize + d_row).clamp(0, row_count.saturating_sub(1) as isize) as usize;
    let nc = (col as isize + d_col).clamp(0, col_count.saturating_sub(1) as isize) as usize;
    (nr, nc)
}

/// Tab (`forward`) / Shift+Tab: across the row, wrapping to the start of
/// the next/previous row at a row boundary — never extends the selection,
/// matching every spreadsheet's own Tab convention.
pub(crate) fn move_tab(
    row_count: usize,
    col_count: usize,
    (row, col): (usize, usize),
    forward: bool,
) -> (usize, usize) {
    if forward {
        if col + 1 < col_count {
            (row, col + 1)
        } else if row + 1 < row_count {
            (row + 1, 0)
        } else {
            (row, col)
        }
    } else if col > 0 {
        (row, col - 1)
    } else if row > 0 {
        (row - 1, col_count.saturating_sub(1))
    } else {
        (row, col)
    }
}

/// Enter (`forward`) / Shift+Enter: down/up within the same column.
pub(crate) fn move_enter(
    row_count: usize,
    (row, col): (usize, usize),
    forward: bool,
) -> (usize, usize) {
    let nr = if forward {
        (row + 1).min(row_count.saturating_sub(1))
    } else {
        row.saturating_sub(1)
    };
    (nr, col)
}

/// The row delta a Page Up (`forward` false) / Page Down (`forward` true)
/// should move the cursor by, given how many body rows are visible —
/// always at least one row, even in a viewport that only fits one.
pub(crate) fn page_delta(body_rows: usize, forward: bool) -> isize {
    let page = body_rows.max(1) as isize;
    if forward {
        page
    } else {
        -page
    }
}

/// Select-all's `(cursor, anchor)` pair — `None` when either axis of the
/// sheet is empty, since there is nothing to select.
pub(crate) fn select_all_bounds(
    row_count: usize,
    col_count: usize,
) -> Option<((usize, usize), (usize, usize))> {
    if row_count == 0 || col_count == 0 {
        None
    } else {
        Some(((0, 0), (row_count - 1, col_count - 1)))
    }
}

/// Select an entire `row`'s `(cursor, anchor)` pair — `None` when there
/// are no columns to select across.
pub(crate) fn select_row_bounds(
    row_count: usize,
    col_count: usize,
    row: usize,
) -> Option<((usize, usize), (usize, usize))> {
    if col_count == 0 {
        None
    } else {
        Some((
            clamp_cell(row_count, col_count, (row, 0)),
            clamp_cell(row_count, col_count, (row, col_count - 1)),
        ))
    }
}

/// Select an entire `col`'s `(cursor, anchor)` pair — `None` when there
/// are no rows to select down.
pub(crate) fn select_col_bounds(
    row_count: usize,
    col_count: usize,
    col: usize,
) -> Option<((usize, usize), (usize, usize))> {
    if row_count == 0 {
        None
    } else {
        Some((
            clamp_cell(row_count, col_count, (0, col)),
            clamp_cell(row_count, col_count, (row_count - 1, col)),
        ))
    }
}

/// Scroll offsets that bring `cursor` into view against a viewport of
/// `body_rows` x `body_cols` (the non-frozen body, already measured by the
/// caller), independently per axis and skipped entirely for a cursor
/// inside the frozen rows/columns — those are always visible by
/// construction, scrolled or not.
#[allow(clippy::too_many_arguments)]
pub(crate) fn ensure_visible_scroll(
    row_count: usize,
    col_count: usize,
    frozen_rows: usize,
    frozen_cols: usize,
    body_rows: usize,
    body_cols: usize,
    row_scroll: usize,
    col_scroll: usize,
    cursor: (usize, usize),
) -> (usize, usize) {
    let frozen_rows = frozen_rows.min(row_count);
    let frozen_cols = frozen_cols.min(col_count);
    let (cr, cc) = cursor;

    let row_scroll = if cr >= frozen_rows {
        let rank = cr - frozen_rows;
        // A viewport that fits zero body rows (frozen rows fill the rect)
        // gives `ensure_visible` no window to measure against, so it
        // no-ops; a backwards move still pulls the offset back to the
        // cursor's own rank there.
        let scroll = if body_rows == 0 {
            row_scroll.min(rank)
        } else {
            ensure_visible(row_scroll, body_rows, rank)
        };
        let max_scroll = (row_count.saturating_sub(frozen_rows)).saturating_sub(body_rows);
        scroll_by_clamped(scroll, 0, max_scroll)
    } else {
        row_scroll
    };
    let col_scroll = if cc >= frozen_cols {
        let rank = cc - frozen_cols;
        // Same zero-width-body case as the row axis above.
        let scroll = if body_cols == 0 {
            col_scroll.min(rank)
        } else {
            ensure_visible(col_scroll, body_cols, rank)
        };
        let max_scroll = (col_count.saturating_sub(frozen_cols)).saturating_sub(body_cols);
        scroll_by_clamped(scroll, 0, max_scroll)
    } else {
        col_scroll
    };
    (row_scroll, col_scroll)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_cell_saturates_each_axis_independently() {
        assert_eq!(clamp_cell(5, 5, (10, 2)), (4, 2));
        assert_eq!(clamp_cell(5, 5, (2, 10)), (2, 4));
        assert_eq!(clamp_cell(0, 0, (3, 3)), (0, 0));
    }

    #[test]
    fn move_by_clamps_at_the_sheet_edges() {
        assert_eq!(move_by(5, 5, (0, 0), -1, -1), (0, 0));
        assert_eq!(move_by(5, 5, (4, 4), 1, 1), (4, 4));
        assert_eq!(move_by(5, 5, (2, 2), 1, -1), (3, 1));
    }

    #[test]
    fn move_tab_wraps_rows_forward_and_backward() {
        assert_eq!(move_tab(3, 3, (0, 2), true), (1, 0));
        assert_eq!(move_tab(3, 3, (2, 2), true), (2, 2));
        assert_eq!(move_tab(3, 3, (1, 0), false), (0, 2));
        assert_eq!(move_tab(3, 3, (0, 0), false), (0, 0));
    }

    #[test]
    fn move_enter_moves_within_the_column() {
        assert_eq!(move_enter(5, (2, 1), true), (3, 1));
        assert_eq!(move_enter(5, (0, 1), false), (0, 1));
        assert_eq!(move_enter(5, (4, 1), true), (4, 1));
    }

    #[test]
    fn page_delta_is_never_narrower_than_one_row() {
        assert_eq!(page_delta(0, true), 1);
        assert_eq!(page_delta(0, false), -1);
        assert_eq!(page_delta(10, true), 10);
    }

    #[test]
    fn select_bounds_report_none_on_the_empty_axis() {
        assert_eq!(select_all_bounds(0, 5), None);
        assert_eq!(select_row_bounds(5, 0, 2), None);
        assert_eq!(select_col_bounds(0, 5, 2), None);
        assert_eq!(select_all_bounds(3, 3), Some(((0, 0), (2, 2))));
        assert_eq!(select_row_bounds(3, 4, 1), Some(((1, 0), (1, 3))));
        assert_eq!(select_col_bounds(4, 3, 1), Some(((0, 1), (3, 1))));
    }

    #[test]
    fn ensure_visible_scroll_skips_the_frozen_prefix() {
        // Cursor sits inside the frozen row/column — neither offset moves
        // no matter how far the body viewport already scrolled.
        let (row_scroll, col_scroll) = ensure_visible_scroll(10, 10, 1, 1, 4, 4, 3, 3, (0, 0));
        assert_eq!((row_scroll, col_scroll), (3, 3));
    }

    #[test]
    fn ensure_visible_scroll_brings_a_body_cursor_into_view() {
        // Body cursor at absolute (9, 9) with a 4x4 body viewport and no
        // scroll yet — the offset must move enough to include rank 8.
        let (row_scroll, col_scroll) = ensure_visible_scroll(10, 10, 1, 1, 4, 4, 0, 0, (9, 9));
        assert_eq!(row_scroll, 5);
        assert_eq!(col_scroll, 5);
    }
}
