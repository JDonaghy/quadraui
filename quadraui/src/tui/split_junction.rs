//! Shared box-drawing junction-glyph logic for [`super::split::draw_split`]
//! and [`super::split_tree::draw_split_tree`] (quadraui#1067).
//!
//! Both rasterisers paint a straight run of `'│'`/`'─'` for their own
//! divider, then call [`upgrade_junctions`] to read back the run's
//! perpendicular neighbour cells in the [`Buffer`] and upgrade the run's
//! end cells — and any cell where it crosses an already-painted
//! perpendicular divider — to the matching box-drawing junction glyph
//! (`┼ ├ ┤ ┬ ┴`). This is the same read-back vimcode's own ~350-line
//! divider rasteriser did by hand
//! (`vimcode/src/tui_main/render_impl.rs::vertical_separator_cells` /
//! `render_separators`); moving it into the TUI backend means a
//! cell-grid consumer no longer needs a divider rasteriser of its own.

use ratatui::buffer::Buffer;
use ratatui::style::Color as RatatuiColor;

use super::set_cell;

const UP: u8 = 0b0001;
const DOWN: u8 = 0b0010;
const LEFT: u8 = 0b0100;
const RIGHT: u8 = 0b1000;

/// Which cardinal directions does an existing divider glyph `ch` connect
/// to? Non-divider glyphs (blank background, pane content, etc.) connect
/// nowhere, so they never contribute a spurious junction.
fn connections(ch: char) -> u8 {
    match ch {
        '│' => UP | DOWN,
        '─' => LEFT | RIGHT,
        '┼' => UP | DOWN | LEFT | RIGHT,
        '┬' => DOWN | LEFT | RIGHT,
        '┴' => UP | LEFT | RIGHT,
        '├' => UP | DOWN | RIGHT,
        '┤' => UP | DOWN | LEFT,
        _ => 0,
    }
}

/// Pick the box-drawing glyph for a divider cell with the given set of
/// live connections. `vertical` breaks the all-false tie (an isolated
/// single-cell run with no neighbours on either side) toward the run's
/// own axis glyph.
fn glyph_for(up: bool, down: bool, left: bool, right: bool, vertical: bool) -> char {
    match (up, down, left, right) {
        (true, true, true, true) => '┼',
        (false, true, true, true) => '┬',
        (true, false, true, true) => '┴',
        (true, true, false, true) => '├',
        (true, true, true, false) => '┤',
        _ if left || right => '─',
        _ if up || down => '│',
        _ => {
            if vertical {
                '│'
            } else {
                '─'
            }
        }
    }
}

/// Is `(x, y)` inside `buf`'s area? Mirrors [`set_cell`]'s own guard —
/// indexing a `Buffer` outside its area panics, and a divider run may
/// legitimately extend past the buffer edge (see [`upgrade_junctions`]).
fn in_area(buf: &Buffer, x: u16, y: u16) -> bool {
    let area = buf.area;
    x >= area.x && y >= area.y && x < area.x + area.width && y < area.y + area.height
}

/// Read a cell's first glyph, treating anything outside the buffer as
/// blank. Out-of-area reads must not panic: `Buffer`'s `Index` impl does,
/// and a clipped run's neighbour coordinates can land outside.
fn cell_char(buf: &Buffer, x: u16, y: u16) -> char {
    if !in_area(buf, x, y) {
        return ' ';
    }
    buf[(x, y)].symbol().chars().next().unwrap_or(' ')
}

/// Upgrade every cell in a just-painted straight divider run to its
/// correct junction glyph in place.
///
/// `cells` must be the exact sequence of `(x, y)` coordinates painted for
/// the run, in run order — the first and last elements are treated as the
/// run's ends, which is where a `┬`/`┴`/`├`/`┤` most commonly forms (e.g.
/// a vertical divider ending on a horizontal one, or on a status-bar
/// row). `vertical` says whether the run itself travels along the y-axis
/// (a `'│'` column, so its *own* connections are UP/DOWN) or the x-axis
/// (a `'─'` row, own connections LEFT/RIGHT); perpendicular neighbours are
/// always read from the other axis.
///
/// Safe to call regardless of paint order relative to other divider runs
/// sharing the same buffer: it only ever *reads* a cell's perpendicular
/// neighbours, and every junction glyph in [`connections`] is a superset
/// of the plain `'│'`/`'─'` a cell may have started as — so a second run's
/// own upgrade pass can only add connections at a shared cell, never lose
/// the ones the first run already recorded there (see quadraui#1067).
///
/// `cells` may extend past the buffer's area — rounding in a caller's
/// divider geometry routinely puts the last cell of a run one column or
/// row outside `buf.area`, and `set_cell` silently drops those. Cells
/// outside the area are skipped here for the same reason (indexing a
/// `Buffer` out of area panics), while still counting as a run-order
/// connection for their in-area neighbour — so a run clipped at the edge
/// keeps a plain `'│'`/`'─'` at its last *visible* cell rather than
/// sprouting a spurious tee where the buffer merely ran out.
pub(super) fn upgrade_junctions(
    buf: &mut Buffer,
    cells: &[(u16, u16)],
    vertical: bool,
    fg: RatatuiColor,
    bg: RatatuiColor,
) {
    for (i, &(x, y)) in cells.iter().enumerate() {
        // Out-of-area run cells were never painted (`set_cell` guards),
        // so there is nothing to upgrade — and reading their neighbours
        // would index the buffer out of area and panic.
        if !in_area(buf, x, y) {
            continue;
        }

        let (mut up, mut down, mut left, mut right) = if vertical {
            (i > 0, i + 1 < cells.len(), false, false)
        } else {
            (false, false, i > 0, i + 1 < cells.len())
        };

        // `cell_char` clamps out-of-area reads to a blank, so only the
        // `- 1` underflows need guarding here.
        if y > 0 {
            up = up || (connections(cell_char(buf, x, y - 1)) & DOWN) != 0;
        }
        down = down || (connections(cell_char(buf, x, y + 1)) & UP) != 0;
        if x > 0 {
            left = left || (connections(cell_char(buf, x - 1, y)) & RIGHT) != 0;
        }
        right = right || (connections(cell_char(buf, x + 1, y)) & LEFT) != 0;

        let glyph = glyph_for(up, down, left, right, vertical);
        set_cell(buf, x, y, glyph, fg, bg);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::layout::Rect as RRect;
    use ratatui::style::Color;

    fn buf(w: u16, h: u16) -> Buffer {
        Buffer::empty(RRect::new(0, 0, w, h))
    }

    fn paint_run(
        buf: &mut Buffer,
        cells: &[(u16, u16)],
        base: char,
        fg: RatatuiColor,
        bg: RatatuiColor,
    ) {
        for &(x, y) in cells {
            set_cell(buf, x, y, base, fg, bg);
        }
    }

    #[test]
    fn plain_runs_stay_plain_when_isolated() {
        let mut b = buf(5, 5);
        let fg = Color::White;
        let bg = Color::Black;
        let vcells: Vec<(u16, u16)> = (0..5).map(|y| (2, y)).collect();
        paint_run(&mut b, &vcells, '│', fg, bg);
        upgrade_junctions(&mut b, &vcells, true, fg, bg);
        for &(x, y) in &vcells {
            assert_eq!(cell_char(&b, x, y), '│');
        }
    }

    /// Regression (quadraui#1067 smoke failure): a caller's rounded
    /// divider geometry can put run cells one column/row *outside* the
    /// buffer — `set_cell` silently drops those, so `upgrade_junctions`
    /// must skip them instead of indexing the buffer out of area (which
    /// panics: "index outside of buffer"). The last *visible* cell of a
    /// clipped run stays a plain axis glyph, not a spurious tee.
    #[test]
    fn run_clipped_past_the_far_edge_does_not_panic() {
        let fg = Color::White;
        let bg = Color::Black;

        // Vertical run one row too tall, at a column one past the right
        // edge for its final cell — both overshoot axes at once.
        let mut b = buf(5, 5);
        let vcells: Vec<(u16, u16)> = (0..6).map(|y| (2, y)).collect();
        paint_run(&mut b, &vcells, '│', fg, bg);
        upgrade_junctions(&mut b, &vcells, true, fg, bg);
        assert_eq!(cell_char(&b, 2, 4), '│');

        // Horizontal run overshooting the right edge — the out-of-area
        // column x=5 is skipped, x=4 stays plain.
        let mut b = buf(5, 5);
        let hcells: Vec<(u16, u16)> = (0..6).map(|x| (x, 2)).collect();
        paint_run(&mut b, &hcells, '─', fg, bg);
        upgrade_junctions(&mut b, &hcells, false, fg, bg);
        assert_eq!(cell_char(&b, 4, 2), '─');

        // A run entirely outside the area is a no-op, not a panic.
        let mut b = buf(5, 5);
        let outside: Vec<(u16, u16)> = (7..10).map(|y| (9, y)).collect();
        upgrade_junctions(&mut b, &outside, true, fg, bg);
        assert_eq!(cell_char(&b, 4, 4), ' ');
    }

    /// Same guard on the near edge: a buffer whose area does not start at
    /// the origin (an inset frame) must not have its `x - 1` / `y - 1`
    /// neighbour reads underflow, and run cells before the area's origin
    /// are skipped like `set_cell` skips them.
    #[test]
    fn run_clipped_before_the_area_origin_does_not_panic() {
        let fg = Color::White;
        let bg = Color::Black;
        let mut b = Buffer::empty(RRect::new(5, 5, 5, 5));

        // Starts two rows above the area's origin.
        let vcells: Vec<(u16, u16)> = (3..10).map(|y| (7, y)).collect();
        paint_run(&mut b, &vcells, '│', fg, bg);
        upgrade_junctions(&mut b, &vcells, true, fg, bg);
        assert_eq!(cell_char(&b, 7, 5), '│');

        // A horizontal run reaching left of the area's origin, crossing
        // the column above: the shared cell still resolves to a junction.
        let hcells: Vec<(u16, u16)> = (2..11).map(|x| (x, 7)).collect();
        paint_run(&mut b, &hcells, '─', fg, bg);
        upgrade_junctions(&mut b, &hcells, false, fg, bg);
        assert_eq!(cell_char(&b, 7, 7), '┼');
        assert_eq!(cell_char(&b, 5, 7), '─');
    }

    #[test]
    fn crossing_runs_form_a_plus() {
        let mut b = buf(5, 5);
        let fg = Color::White;
        let bg = Color::Black;
        let vcells: Vec<(u16, u16)> = (0..5).map(|y| (2, y)).collect();
        let hcells: Vec<(u16, u16)> = (0..5).map(|x| (x, 2)).collect();

        paint_run(&mut b, &vcells, '│', fg, bg);
        upgrade_junctions(&mut b, &vcells, true, fg, bg);

        paint_run(&mut b, &hcells, '─', fg, bg);
        upgrade_junctions(&mut b, &hcells, false, fg, bg);

        assert_eq!(cell_char(&b, 2, 2), '┼');
        assert_eq!(cell_char(&b, 2, 0), '│');
        assert_eq!(cell_char(&b, 0, 2), '─');
    }

    #[test]
    fn crossing_order_is_symmetric() {
        // Same cross, but the horizontal run's upgrade pass runs first —
        // the result must be identical (module doc: order-independent).
        let mut b = buf(5, 5);
        let fg = Color::White;
        let bg = Color::Black;
        let vcells: Vec<(u16, u16)> = (0..5).map(|y| (2, y)).collect();
        let hcells: Vec<(u16, u16)> = (0..5).map(|x| (x, 2)).collect();

        paint_run(&mut b, &hcells, '─', fg, bg);
        upgrade_junctions(&mut b, &hcells, false, fg, bg);

        paint_run(&mut b, &vcells, '│', fg, bg);
        upgrade_junctions(&mut b, &vcells, true, fg, bg);

        assert_eq!(cell_char(&b, 2, 2), '┼');
    }

    #[test]
    fn t_junction_each_side() {
        let fg = Color::White;
        let bg = Color::Black;

        // ┬: vertical run starts where a horizontal run passes above it.
        let mut b = buf(5, 5);
        let hcells: Vec<(u16, u16)> = (0..5).map(|x| (x, 0)).collect();
        paint_run(&mut b, &hcells, '─', fg, bg);
        upgrade_junctions(&mut b, &hcells, false, fg, bg);
        let vcells: Vec<(u16, u16)> = (0..3).map(|y| (2, y)).collect();
        paint_run(&mut b, &vcells, '│', fg, bg);
        upgrade_junctions(&mut b, &vcells, true, fg, bg);
        assert_eq!(cell_char(&b, 2, 0), '┬');

        // ┴: vertical run's last cell IS a horizontal run's row (the
        // vertical pane runs all the way down to, and including, the
        // row the horizontal rule occupies — exactly how a real
        // `SplitTree`'s outer divider spans the full cross-axis extent
        // that a shorter divider only reaches the edge of).
        let mut b = buf(5, 5);
        let hcells: Vec<(u16, u16)> = (0..5).map(|x| (x, 4)).collect();
        paint_run(&mut b, &hcells, '─', fg, bg);
        upgrade_junctions(&mut b, &hcells, false, fg, bg);
        let vcells: Vec<(u16, u16)> = (0..5).map(|y| (2, y)).collect();
        paint_run(&mut b, &vcells, '│', fg, bg);
        upgrade_junctions(&mut b, &vcells, true, fg, bg);
        assert_eq!(cell_char(&b, 2, 4), '┴');

        // ├: horizontal run starts where a vertical run passes to its right.
        let mut b = buf(5, 5);
        let vcells: Vec<(u16, u16)> = (0..5).map(|y| (0, y)).collect();
        paint_run(&mut b, &vcells, '│', fg, bg);
        upgrade_junctions(&mut b, &vcells, true, fg, bg);
        let hcells: Vec<(u16, u16)> = (0..3).map(|x| (x, 2)).collect();
        paint_run(&mut b, &hcells, '─', fg, bg);
        upgrade_junctions(&mut b, &hcells, false, fg, bg);
        assert_eq!(cell_char(&b, 0, 2), '├');

        // ┤: horizontal run ends where a vertical run passes to its left.
        let mut b = buf(5, 5);
        let vcells: Vec<(u16, u16)> = (0..5).map(|y| (4, y)).collect();
        paint_run(&mut b, &vcells, '│', fg, bg);
        upgrade_junctions(&mut b, &vcells, true, fg, bg);
        let hcells: Vec<(u16, u16)> = (2..5).map(|x| (x, 2)).collect();
        paint_run(&mut b, &hcells, '─', fg, bg);
        upgrade_junctions(&mut b, &hcells, false, fg, bg);
        assert_eq!(cell_char(&b, 4, 2), '┤');
    }

    #[test]
    fn run_ending_on_a_status_bar_row_forms_a_tee() {
        // A vertical divider whose bottom row IS a status bar's own rule
        // row (the pane column runs all the way down into that row, the
        // same way a real `SplitTree`'s cross-axis reaches the full
        // extent of its bounds) becomes a '┴', not a dangling '│'.
        let mut b = buf(5, 5);
        let fg = Color::White;
        let bg = Color::Black;
        let status_row: Vec<(u16, u16)> = (0..5).map(|x| (x, 4)).collect();
        paint_run(&mut b, &status_row, '─', fg, bg);
        upgrade_junctions(&mut b, &status_row, false, fg, bg);

        let vcells: Vec<(u16, u16)> = (0..5).map(|y| (1, y)).collect();
        paint_run(&mut b, &vcells, '│', fg, bg);
        upgrade_junctions(&mut b, &vcells, true, fg, bg);

        assert_eq!(cell_char(&b, 1, 4), '┴');
        // Untouched columns of the status row stay a plain rule.
        assert_eq!(cell_char(&b, 3, 4), '─');
    }
}
