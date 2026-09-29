//! criterion benches for [`quadraui::TreeView`] at a 100k-row scale
//! (issue #1118).
//!
//! Same three-stage shape as `data_table_bench.rs` — see that file's
//! module doc for the full rationale. In short: `TreeView::rows` is the
//! app's full pre-flattened row set (module doc on
//! `src/primitives/tree.rs`), so `frame_build` rebuilding all 100k rows
//! is the realistic worst case; `layout` is a flat-cost regression
//! sentinel; `paint` proves the TUI/GTK rasterisers stay bounded by the
//! viewport rather than the row count.
//!
//! Run: `cargo bench --bench tree_bench --features tui,gtk`.

use criterion::{criterion_group, criterion_main, Criterion};
use quadraui::{
    Decoration, SelectionMode, StyledText, TreeRow, TreeRowMeasure, TreeStyle, TreeView, WidgetId,
};
use std::hint::black_box;

const ROWS: usize = 100_000;

fn build_row(i: usize) -> TreeRow {
    TreeRow {
        path: vec![(i / 1000) as u16, (i % 1000) as u16],
        indent: (i % 6) as u16,
        icon: None,
        text: StyledText::plain(format!("node_{i}.rs")),
        badge: None,
        is_expanded: if i.is_multiple_of(7) {
            Some(i.is_multiple_of(14))
        } else {
            None
        },
        decoration: if i.is_multiple_of(1000) {
            Decoration::Header
        } else {
            Decoration::Normal
        },
        edit: None,
    }
}

fn build_tree(nrows: usize) -> TreeView {
    let rows: Vec<TreeRow> = (0..nrows).map(build_row).collect();
    TreeView {
        id: WidgetId::new("bench-tree"),
        rows,
        selection_mode: SelectionMode::Single,
        selected_path: Some(vec![(nrows / 2000) as u16, ((nrows / 2) % 1000) as u16]),
        scroll_offset: nrows / 2,
        style: TreeStyle::default(),
        has_focus: true,
    }
}

fn bench_frame_build(c: &mut Criterion) {
    c.bench_function("tree_frame_build_100k_rows", |b| {
        b.iter(|| black_box(build_tree(black_box(ROWS))))
    });
}

fn bench_layout(c: &mut Criterion) {
    let tree = build_tree(ROWS);
    c.bench_function("tree_layout_100k_rows", |b| {
        b.iter(|| {
            black_box(tree.layout(black_box(120.0), black_box(50.0), |_| {
                TreeRowMeasure::new(1.0)
            }))
        })
    });
}

criterion_group!(core_benches, bench_frame_build, bench_layout);

#[cfg(feature = "tui")]
mod tui_bench {
    use super::*;
    use quadraui::tui::draw_tree;
    use quadraui::Theme;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;

    pub fn bench_paint(c: &mut Criterion) {
        let tree = build_tree(ROWS);
        let theme = Theme::default();
        let area = Rect::new(0, 0, 60, 50);
        c.bench_function("tree_paint_tui_100k_rows", |b| {
            b.iter(|| {
                let mut buf = Buffer::empty(area);
                draw_tree(&mut buf, area, &tree, &theme, false);
                black_box(&buf);
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
    use quadraui::gtk::draw_tree;
    use quadraui::Theme;

    pub fn bench_paint(c: &mut Criterion) {
        let tree = build_tree(ROWS);
        let theme = Theme::default();
        let surface = ImageSurface::create(Format::ARgb32, 600, 800).expect("create ImageSurface");
        let cr = Context::new(&surface).expect("Context::new on headless ImageSurface");
        let pctx = pangocairo::functions::create_context(&cr);
        pctx.set_font_description(&pango::FontDescription::from_string("Sans 11"));
        let layout = pango::Layout::new(&pctx);
        c.bench_function("tree_paint_gtk_100k_rows", |b| {
            b.iter(|| {
                draw_tree(
                    &cr, &layout, 0.0, 0.0, 600.0, 800.0, &tree, &theme, 20.0, false,
                );
                black_box(());
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
