//! criterion benches for [`quadraui::CellGrid`]'s two-axis
//! virtualisation at a 1,000,000 × 16,384 sheet scale.
//!
//! Unlike `data_table_bench.rs`/`tree_bench.rs` (whose primitives hold
//! their *full* row set on the descriptor), `CellGrid::cells` is already
//! only ever a viewport-sized window (see `src/primitives/cell_grid.rs`'s
//! module doc) — so `frame_build` here constructs a realistic
//! `WINDOW_ROWS`×`WINDOW_COLS` window, not `TOTAL_ROWS`×`TOTAL_COLS`
//! cells. The point of every bench in this file is the same: cost stays
//! pinned to the small window regardless of how large `row_count`/
//! `col_count` (the sheet totals) are, matching the issue's own
//! acceptance bar of "a 100×100 window against a 1M-row total".
//!
//! 1. **`frame_build`** — construct the declarative `CellGrid` snapshot
//!    with its `WINDOW_ROWS`×`WINDOW_COLS` window of `Cell`s. O(window),
//!    never O(total).
//! 2. **`layout`** — [`quadraui::CellGrid::layout`]. `resolve_spans`
//!    (the shared row/column stacker both axes go through) stops as
//!    soon as the viewport is full, so this stays flat regardless of
//!    `TOTAL_ROWS`/`TOTAL_COLS`.
//! 3. **`cells_requested`** — [`quadraui::CellGrid::cells_requested`],
//!    the per-frame "does the loaded window still cover the viewport"
//!    check two-axis virtualisation adds. O(visible rows + visible
//!    columns), never O(total).
//! 4. **`paint`** — [`quadraui::tui::draw_cell_grid`]. Only the resolved
//!    viewport's rows/columns are painted, so this proves the TUI
//!    rasteriser stays viewport-bound rather than becoming O(total).
//!
//! Run: `cargo bench --bench cell_grid_bench --features tui`.

use criterion::{criterion_group, criterion_main, Criterion};
use quadraui::{Cell, CellGrid, WidgetId};
use std::hint::black_box;

const TOTAL_ROWS: usize = 1_000_000;
const TOTAL_COLS: usize = 16_384;
const WINDOW_ROWS: usize = 100;
const WINDOW_COLS: usize = 100;

fn build_grid(
    total_rows: usize,
    total_cols: usize,
    window_rows: usize,
    window_cols: usize,
) -> CellGrid {
    let cells: Vec<Vec<Cell>> = (0..window_rows)
        .map(|r| {
            (0..window_cols)
                .map(|c| Cell::plain(format!("r{r}c{c}")))
                .collect()
        })
        .collect();
    CellGrid {
        id: WidgetId::new("bench-cell-grid"),
        row_count: total_rows,
        col_count: total_cols,
        frozen_rows: 1,
        frozen_cols: 1,
        row_scroll: total_rows / 2,
        col_scroll: total_cols / 2,
        default_row_height: 1.0,
        default_col_width: 8.0,
        row_heights: Default::default(),
        col_widths: Default::default(),
        show_row_headers: true,
        show_col_headers: true,
        row_labels: Default::default(),
        col_labels: Default::default(),
        cursor: (total_rows / 2, total_cols / 2),
        selection_anchor: None,
        cells,
        has_focus: true,
        show_scrollbar: true,
    }
}

fn bench_frame_build(c: &mut Criterion) {
    c.bench_function("cell_grid_frame_build_100x100_window_of_1m_rows", |b| {
        b.iter(|| {
            black_box(build_grid(
                black_box(TOTAL_ROWS),
                black_box(TOTAL_COLS),
                black_box(WINDOW_ROWS),
                black_box(WINDOW_COLS),
            ))
        })
    });
}

fn bench_layout(c: &mut Criterion) {
    let grid = build_grid(TOTAL_ROWS, TOTAL_COLS, WINDOW_ROWS, WINDOW_COLS);
    c.bench_function("cell_grid_layout_100x100_window_of_1m_rows", |b| {
        b.iter(|| {
            black_box(grid.layout(
                black_box(120.0),
                black_box(50.0),
                black_box(1.0),
                black_box(6.0),
                black_box(1.0),
                black_box(1.0),
            ))
        })
    });
}

fn bench_cells_requested(c: &mut Criterion) {
    let grid = build_grid(TOTAL_ROWS, TOTAL_COLS, WINDOW_ROWS, WINDOW_COLS);
    let layout = grid.layout(120.0, 50.0, 1.0, 6.0, 1.0, 1.0);
    c.bench_function("cell_grid_cells_requested_100x100_window_of_1m_rows", |b| {
        b.iter(|| black_box(grid.cells_requested(black_box(&layout))))
    });
}

criterion_group!(
    core_benches,
    bench_frame_build,
    bench_layout,
    bench_cells_requested
);

#[cfg(feature = "tui")]
mod tui_bench {
    use super::*;
    use quadraui::tui::draw_cell_grid;
    use quadraui::Theme;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;

    pub fn bench_paint(c: &mut Criterion) {
        let grid = build_grid(TOTAL_ROWS, TOTAL_COLS, WINDOW_ROWS, WINDOW_COLS);
        let theme = Theme::default();
        let area = Rect::new(0, 0, 120, 50);
        c.bench_function("cell_grid_paint_tui_100x100_window_of_1m_rows", |b| {
            b.iter(|| {
                let mut buf = Buffer::empty(area);
                black_box(draw_cell_grid(&mut buf, area, &grid, &theme));
            })
        });
    }
}

#[cfg(feature = "tui")]
criterion_group!(tui_benches, tui_bench::bench_paint);

#[cfg(feature = "tui")]
criterion_main!(core_benches, tui_benches);
#[cfg(not(feature = "tui"))]
criterion_main!(core_benches);
