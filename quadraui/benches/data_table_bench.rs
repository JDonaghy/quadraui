//! criterion benches for [`quadraui::DataTable`] at a 100k-row scale
//! (issue #1118).
//!
//! Three stages, matching how a real host drives a `DataTable` each
//! frame:
//!
//! 1. **`frame_build`** — construct the declarative `DataTable` snapshot
//!    (100k `DataRow`s). This is the dominant O(n) cost: per the
//!    primitive's own contract (`src/primitives/data_table.rs` module
//!    doc), `DataTable::rows` holds the *full* row set — a host scrolling
//!    a 100k-row table is expected to carry all 100k rows on the struct,
//!    not just the visible slice — so this stage is the realistic
//!    worst case of rebuilding that snapshot from scratch every frame.
//! 2. **`layout`** — [`quadraui::DataTable::layout`] (core, no backend
//!    feature required). Column geometry doesn't depend on row count, so
//!    this stage is a regression sentinel more than a scaling stress
//!    test: it should stay flat regardless of `ROWS`.
//! 3. **`paint`** — the TUI (`quadraui::tui::draw_data_table`) and GTK
//!    (`quadraui::gtk::draw_data_table`, headless in-memory Cairo)
//!    rasterisers. Both only paint `visible_rows` (bounded by the
//!    viewport), so this proves the paint path stays viewport-bound on
//!    a 100k-row table rather than accidentally becoming O(rows).
//!
//! Run: `cargo bench --bench data_table_bench --features tui,gtk`
//! (drop a feature to skip its `paint` stage; `frame_build`/`layout`
//! always run). No regression gate is wired up yet — see the issue's
//! scope note.

use criterion::{criterion_group, criterion_main, Criterion};
use quadraui::{
    Column, ColumnAlign, ColumnMeasure, ColumnWidth, DataRow, DataTable, Decoration, StyledText,
    WidgetId,
};
use std::hint::black_box;

const ROWS: usize = 100_000;
const COLS: usize = 8;

fn build_table(nrows: usize, ncols: usize) -> DataTable {
    let columns: Vec<Column> = (0..ncols)
        .map(|i| Column {
            title: format!("Column {i}"),
            width: ColumnWidth::Flex(1.0),
            align: ColumnAlign::Left,
        })
        .collect();
    let rows: Vec<DataRow> = (0..nrows)
        .map(|r| DataRow {
            cells: (0..ncols)
                .map(|c| StyledText::plain(format!("row {r} / col {c}")))
                .collect(),
            decoration: if r % 500 == 0 {
                Decoration::Header
            } else {
                Decoration::Normal
            },
        })
        .collect();
    DataTable {
        id: WidgetId::new("bench-data-table"),
        columns,
        rows,
        selected_idx: Some(nrows / 2),
        scroll_offset: nrows / 2,
        sort: Some((0, quadraui::SortDirection::Ascending)),
        has_focus: true,
        show_scrollbar: true,
        min_total_width: None,
        h_scroll: 0.0,
        column_overrides: Vec::new(),
        footer: None,
    }
}

fn bench_frame_build(c: &mut Criterion) {
    c.bench_function("data_table_frame_build_100k_rows", |b| {
        b.iter(|| black_box(build_table(black_box(ROWS), black_box(COLS))))
    });
}

fn bench_layout(c: &mut Criterion) {
    let table = build_table(ROWS, COLS);
    c.bench_function("data_table_layout_100k_rows", |b| {
        b.iter(|| {
            black_box(
                table.layout(black_box(120.0), black_box(50.0), 1.0, 1.0, 1.0, |col| {
                    ColumnMeasure::new(col.title.chars().count() as f32)
                }),
            )
        })
    });
}

criterion_group!(core_benches, bench_frame_build, bench_layout);

#[cfg(feature = "tui")]
mod tui_bench {
    use super::*;
    use quadraui::tui::draw_data_table;
    use quadraui::Theme;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;

    pub fn bench_paint(c: &mut Criterion) {
        let table = build_table(ROWS, COLS);
        let theme = Theme::default();
        let area = Rect::new(0, 0, 120, 50);
        c.bench_function("data_table_paint_tui_100k_rows", |b| {
            b.iter(|| {
                let mut buf = Buffer::empty(area);
                black_box(draw_data_table(
                    &mut buf,
                    area,
                    &table,
                    &theme,
                    Some(ROWS / 2),
                ));
            })
        });
    }
}

#[cfg(feature = "tui")]
criterion_group!(tui_benches, tui_bench::bench_paint);

#[cfg(feature = "gtk")]
mod gtk_bench {
    use super::*;
    use gtk4::pango;
    use pangocairo::cairo::{Context, Format, ImageSurface};
    use quadraui::gtk::draw_data_table;
    use quadraui::Theme;

    pub fn bench_paint(c: &mut Criterion) {
        let table = build_table(ROWS, COLS);
        let theme = Theme::default();
        let surface = ImageSurface::create(Format::ARgb32, 1200, 800).expect("create ImageSurface");
        let cr = Context::new(&surface).expect("Context::new on headless ImageSurface");
        let pctx = pangocairo::functions::create_context(&cr);
        pctx.set_font_description(&pango::FontDescription::from_string("Sans 11"));
        let layout = pango::Layout::new(&pctx);
        c.bench_function("data_table_paint_gtk_100k_rows", |b| {
            b.iter(|| {
                black_box(draw_data_table(
                    &cr,
                    &layout,
                    0.0,
                    0.0,
                    1200.0,
                    800.0,
                    &table,
                    &theme,
                    20.0,
                    Some(ROWS / 2),
                ));
            })
        });
    }
}

#[cfg(feature = "gtk")]
criterion_group!(gtk_benches, gtk_bench::bench_paint);

#[cfg(all(feature = "tui", feature = "gtk"))]
criterion_main!(core_benches, tui_benches, gtk_benches);
#[cfg(all(feature = "tui", not(feature = "gtk")))]
criterion_main!(core_benches, tui_benches);
#[cfg(all(feature = "gtk", not(feature = "tui")))]
criterion_main!(core_benches, gtk_benches);
#[cfg(not(any(feature = "tui", feature = "gtk")))]
criterion_main!(core_benches);
