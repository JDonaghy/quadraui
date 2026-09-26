//! Editor selection column-range conformance (quadraui#1082).
//!
//! `EditorSelection::cols_on` is the single shared column-range
//! calculation every backend rasteriser (`tui::editor`, `gtk::editor`,
//! `macos::editor`, `win::editor`) now delegates to — see each module's
//! `draw_visual_selection` / `render_selection` / `paint_selection` doc
//! comment. That makes this file's job different from the rest of the
//! conformance suite: `c0.rs`/`c1` (via `conformance.rs`'s scenario
//! matrix) and `c2.rs` cross scenarios/cases against *per-backend*
//! driver implementations, because each backend genuinely re-implements
//! paint/event translation. Selection column math is not
//! re-implemented per backend any more (that duplication — and the
//! Win-only bugs it hid — is exactly what #1082 fixed), so proving the
//! matrix once against `EditorSelection::cols_on` proves it for every
//! caller. The four backends' own `#[cfg(test)]` modules additionally
//! exercise `cols_on` through a real paint (see
//! `gtk::editor::tests::*_selection*`, `macos::editor::tests::*_selection*`,
//! `win::editor::tests::line_selection_spans_full_row`), so the "does
//! the backend actually call this" half is covered there, not repeated
//! here.
//!
//! ## Matrix
//!
//! Rows are the four line kinds the issue names — `Plain` (an ordinary
//! unwrapped line), `WrappedSegment` (a non-zero `segment_col_offset`
//! visual segment), `GhostContinuation` (`is_ghost_continuation`, no
//! real buffer columns), `DiffPadding` (`diff_status ==
//! Some(DiffLine::Padding)`, no buffer content). Columns are the three
//! `SelectionKind`s. Each cell asserts the exact `cols_on` result
//! (`None`, or `Some((start, end))`) a selection spanning every row
//! produces on that line — the Win backend's pre-#1082
//! `paint_selection` got every `GhostContinuation` and `DiffPadding`
//! cell wrong (painted anyway) and every `WrappedSegment` cell wrong
//! (ignored `segment_col_offset` entirely), which is what "the Win
//! cases RED before" in the issue refers to: these are exactly the
//! cases a naive re-derivation (rather than a shared call) gets wrong.

use quadraui::primitives::editor::{DiffLine, EditorLine, EditorSelection, SelectionKind};

/// One row's line fixture, named for the report.
struct LineCase {
    name: &'static str,
    line: EditorLine,
}

fn base_line(line_idx: usize) -> EditorLine {
    EditorLine {
        raw_text: String::new(),
        gutter_text: String::new(),
        spans: Vec::new(),
        line_idx,
        is_current_line: false,
        is_fold_header: false,
        folded_line_count: 0,
        git_diff: None,
        diff_status: None,
        diagnostics: Vec::new(),
        spell_errors: Vec::new(),
        is_breakpoint: false,
        is_conditional_bp: false,
        is_dap_current: false,
        is_wrap_continuation: false,
        segment_col_offset: 0,
        annotation: None,
        ghost_suffix: None,
        is_ghost_continuation: false,
        indent_guides: Vec::new(),
        colorcolumns: Vec::new(),
    }
}

fn line_cases() -> Vec<LineCase> {
    vec![
        LineCase {
            name: "Plain",
            line: EditorLine {
                raw_text: "0123456789".into(),
                ..base_line(1)
            },
        },
        LineCase {
            name: "WrappedSegment",
            line: EditorLine {
                raw_text: "0123456789".into(),
                segment_col_offset: 5,
                is_wrap_continuation: true,
                ..base_line(1)
            },
        },
        LineCase {
            name: "GhostContinuation",
            line: EditorLine {
                raw_text: String::new(),
                is_ghost_continuation: true,
                ghost_suffix: Some("ai suggestion".into()),
                ..base_line(1)
            },
        },
        LineCase {
            name: "DiffPadding",
            line: EditorLine {
                raw_text: String::new(),
                diff_status: Some(DiffLine::Padding),
                ..base_line(1)
            },
        },
    ]
}

/// One selection fixture per `SelectionKind`, all covering buffer lines
/// `0..=2` (so every `line_cases()` row, all at `line_idx == 1`, falls
/// inside the selection's own line range and only the line-kind itself
/// can suppress the cell) and buffer columns `[2, 7]` inclusive.
fn selection_cases() -> Vec<(&'static str, EditorSelection)> {
    vec![
        (
            "Char",
            EditorSelection {
                kind: SelectionKind::Char,
                start_line: 0,
                start_col: 2,
                end_line: 2,
                end_col: 7,
            },
        ),
        (
            "Line",
            EditorSelection {
                kind: SelectionKind::Line,
                start_line: 0,
                start_col: 0,
                end_line: 2,
                end_col: 0,
            },
        ),
        (
            "Block",
            EditorSelection {
                kind: SelectionKind::Block,
                start_line: 0,
                start_col: 2,
                end_line: 2,
                end_col: 7,
            },
        ),
    ]
}

/// Expected `cols_on` result for one (line kind, selection kind) cell.
/// `None` cells are the ghost/padding rows, which must never be
/// selected regardless of selection kind. `Some` cells give the exact
/// `(start, end)` this multi-line, interior-row (`line_idx == 1`, so
/// neither the selection's own `start_line` nor `end_line`) selection
/// produces:
///
/// - `Plain`/`WrappedSegment` + `Char`: an interior line of a multi-line
///   Char selection is fully covered, `[0, char_count)` — for
///   `WrappedSegment` that's segment-local columns `[0, 10)`, not
///   shifted by its `segment_col_offset` of 5 (quadraui#1082's core
///   fix: the caller never re-adds the offset).
/// - `Plain`/`WrappedSegment` + `Line`: always the full row.
/// - `Plain` + `Block`: the same buffer columns `[start_col, end_col] =
///   [2, 7]` on every row, exclusive-end `[2, 8)`.
/// - `WrappedSegment` + `Block`: `Block`'s `[2, 8)` is in *buffer*
///   columns on every row (unlike `Char`, it never widens to the whole
///   segment) — this segment's own buffer columns are `[segment_col_offset,
///   segment_col_offset + char_count) = [5, 15)`, so the overlap with
///   `[2, 8)` is buffer columns `[5, 8)`, which shifted into this
///   segment's local space (`- segment_col_offset`) is `[0, 3)`.
fn expected(line_name: &str, sel_name: &str) -> Option<(usize, usize)> {
    match (line_name, sel_name) {
        ("GhostContinuation", _) | ("DiffPadding", _) => None,
        ("Plain", "Char") => Some((0, 10)),
        ("Plain", "Line") => Some((0, 10)),
        ("Plain", "Block") => Some((2, 8)),
        ("WrappedSegment", "Char") => Some((0, 10)),
        ("WrappedSegment", "Line") => Some((0, 10)),
        ("WrappedSegment", "Block") => Some((0, 3)),
        _ => unreachable!("unhandled cell ({line_name}, {sel_name})"),
    }
}

/// The full line-kind × selection-kind matrix, asserted in one table so
/// a failure names the exact cell instead of an assert deep in a loop.
#[test]
fn editor_selection_cols_on_matrix() {
    let lines = line_cases();
    let sels = selection_cases();

    let name_w = lines.iter().map(|l| l.name.len()).max().unwrap_or(0);
    let mut table = format!("{:<name_w$}", "line \\ selection");
    for (sel_name, _) in &sels {
        table.push_str(&format!("  {sel_name:<8}"));
    }
    table.push('\n');

    let mut failures: Vec<String> = Vec::new();
    for lc in &lines {
        table.push_str(&format!("{:<name_w$}", lc.name));
        for (sel_name, sel) in &sels {
            let want = expected(lc.name, sel_name);
            let got = sel.cols_on(&lc.line).map(|c| (c.start, c.end));
            let cell = if got == want { "pass" } else { "FAIL" };
            table.push_str(&format!("  {cell:<8}"));
            if got != want {
                failures.push(format!(
                    "({}, {}): want {:?}, got {:?}",
                    lc.name, sel_name, want, got
                ));
            }
        }
        table.push('\n');
    }

    println!("{table}");
    assert!(
        failures.is_empty(),
        "editor_selection_cols_on_matrix: {} cell(s) wrong:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// A ghost-continuation row's `line_idx` typically repeats the owning
/// buffer line's index (it's a virtual extra row, not a new buffer
/// line) — reproduced directly here since `line_cases()` above uses a
/// fixed `line_idx` for every row and doesn't exercise this shape.
#[test]
fn ghost_continuation_row_excluded_even_when_line_idx_matches_a_real_selected_line() {
    let ghost = EditorLine {
        raw_text: String::new(),
        is_ghost_continuation: true,
        ghost_suffix: Some("suggestion".into()),
        ..base_line(3)
    };
    let sel = EditorSelection {
        kind: SelectionKind::Line,
        start_line: 3,
        start_col: 0,
        end_line: 3,
        end_col: 0,
    };
    assert_eq!(sel.cols_on(&ghost), None);
}

/// A diff-padding row's `line_idx` likewise typically repeats the
/// nearest real buffer line (it's a filler row with no buffer content
/// of its own).
#[test]
fn diff_padding_row_excluded_even_when_line_idx_matches_a_real_selected_line() {
    let padding = EditorLine {
        raw_text: String::new(),
        diff_status: Some(DiffLine::Padding),
        ..base_line(3)
    };
    let sel = EditorSelection {
        kind: SelectionKind::Block,
        start_line: 3,
        start_col: 0,
        end_line: 3,
        end_col: 5,
    };
    assert_eq!(sel.cols_on(&padding), None);
}
