//! criterion benches for [`quadraui::Editor`] against a 4k-line buffer
//! (issue #1118).
//!
//! Unlike `DataTable`/`TreeView`, [`quadraui::Editor::lines`] is
//! documented as "visible lines, one per row" — the *viewport* slice,
//! not the whole buffer (`Editor::total_lines` carries the full count
//! separately; see `src/primitives/editor.rs`'s field doc). So a 4k-line
//! buffer's `frame_build` cost is bounded by the viewport height, not
//! 4000 — the "4k" instead stresses the geometry math that *does* scale
//! with `total_lines` (gutter width, scrollbar placement) while keeping
//! per-frame line construction at its realistic, viewport-bound cost.
//!
//! Same three stages as the other two benches:
//!
//! 1. **`frame_build`** — construct one viewport's worth of
//!    `EditorLine`s (syntax spans, gutter text, diagnostics) from a
//!    4000-line buffer.
//! 2. **`layout`** — [`quadraui::Editor::layout`] (core).
//! 3. **`paint`** — TUI (`quadraui::tui::draw_editor`) / GTK
//!    (`quadraui::gtk::draw_editor`, headless in-memory Cairo).
//!
//! Run: `cargo bench --bench editor_bench --features tui,gtk`.

use criterion::{criterion_group, criterion_main, Criterion};
use quadraui::{
    Color, Editor, EditorCursor, EditorCursorPos, EditorCursorShape, EditorLine, EditorStyle,
    EditorStyledSpan, Rect, WidgetId,
};
use std::hint::black_box;

const TOTAL_LINES: usize = 4_000;
const VISIBLE_LINES: usize = 60;
const MAX_COL: usize = 88;

fn build_line(i: usize) -> EditorLine {
    let text = format!("fn function_{i}(arg: usize) -> usize {{ arg.wrapping_mul({i}) }}");
    let len = text.len();
    EditorLine {
        raw_text: text,
        gutter_text: format!("{:>5}", i + 1),
        spans: vec![
            EditorStyledSpan {
                start_byte: 0,
                end_byte: 2,
                style: EditorStyle {
                    fg: Color::rgb(198, 120, 221),
                    bg: None,
                    bold: true,
                    italic: false,
                    font_scale: 1.0,
                },
            },
            EditorStyledSpan {
                start_byte: len.min(20),
                end_byte: len,
                style: EditorStyle {
                    fg: Color::rgb(171, 178, 191),
                    bg: None,
                    bold: false,
                    italic: false,
                    font_scale: 1.0,
                },
            },
        ],
        line_idx: i,
        is_current_line: i == 0,
        is_fold_header: false,
        folded_line_count: 0,
        git_diff: None,
        diff_status: None,
        diagnostics: Vec::new(),
        spell_errors: Vec::new(),
        is_breakpoint: i.is_multiple_of(25),
        is_conditional_bp: false,
        is_dap_current: false,
        is_wrap_continuation: false,
        segment_col_offset: 0,
        annotation: None,
        ghost_suffix: None,
        is_ghost_continuation: false,
        indent_guides: vec![0, 4],
        colorcolumns: vec![80],
    }
}

fn build_editor(scroll_top: usize) -> Editor {
    let lines: Vec<EditorLine> = (scroll_top..scroll_top + VISIBLE_LINES)
        .map(build_line)
        .collect();
    Editor::new(
        WidgetId::new("bench-editor"),
        Rect::new(0.0, 0.0, 800.0, VISIBLE_LINES as f32 * 18.0),
    )
    .with_lines(lines)
    .with_cursor(EditorCursor {
        pos: EditorCursorPos {
            view_line: 0,
            col: 4,
        },
        shape: EditorCursorShape::Block,
    })
    .with_scroll_top(scroll_top)
    .with_total_lines(TOTAL_LINES)
    .with_max_col(MAX_COL)
    .with_gutter_char_width(5)
    .with_is_active(true)
    .with_show_active_bg(true)
    .with_has_git_diff(true)
    .with_has_breakpoints(true)
    .with_active_indent_col(4)
    .with_cursorline(true)
    .with_lightbulb_glyph('\u{f0eb}')
}

fn bench_frame_build(c: &mut Criterion) {
    c.bench_function("editor_frame_build_4k_lines", |b| {
        b.iter(|| black_box(build_editor(black_box(TOTAL_LINES / 2))))
    });
}

fn bench_layout(c: &mut Criterion) {
    let editor = build_editor(TOTAL_LINES / 2);
    let viewport = Rect::new(0.0, 0.0, 800.0, VISIBLE_LINES as f32 * 18.0);
    c.bench_function("editor_layout_4k_lines", |b| {
        b.iter(|| black_box(editor.layout(black_box(viewport), 8.0, 18.0)))
    });
}

criterion_group!(core_benches, bench_frame_build, bench_layout);

#[cfg(feature = "tui")]
mod tui_bench {
    use super::*;
    use quadraui::tui::draw_editor;
    use quadraui::Theme;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect as TuiRect;

    pub fn bench_paint(c: &mut Criterion) {
        let editor = build_editor(TOTAL_LINES / 2);
        let theme = Theme::default();
        let area = TuiRect::new(0, 0, 90, VISIBLE_LINES as u16);
        c.bench_function("editor_paint_tui_4k_lines", |b| {
            b.iter(|| {
                let mut buf = Buffer::empty(area);
                black_box(draw_editor(&mut buf, area, &editor, &theme));
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
    use quadraui::gtk::draw_editor;
    use quadraui::Theme;

    pub fn bench_paint(c: &mut Criterion) {
        let editor = build_editor(TOTAL_LINES / 2);
        let theme = Theme::default();
        let height = VISIBLE_LINES as i32 * 18;
        let surface =
            ImageSurface::create(Format::ARgb32, 800, height).expect("create ImageSurface");
        let cr = Context::new(&surface).expect("Context::new on headless ImageSurface");
        let pctx = pangocairo::functions::create_context(&cr);
        let font_desc = pango::FontDescription::from_string("Monospace 12");
        pctx.set_font_description(&font_desc);
        let layout = pango::Layout::new(&pctx);
        let metrics = pctx.metrics(Some(&font_desc), None);
        c.bench_function("editor_paint_gtk_4k_lines", |b| {
            b.iter(|| {
                draw_editor(&cr, &layout, &metrics, &editor, &theme, 8.0, 18.0);
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
