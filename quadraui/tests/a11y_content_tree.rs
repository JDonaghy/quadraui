//! Accessibility-tree snapshots for content primitives.
//!
//! Mirrors `a11y_chrome_tree.rs`: push the same descriptors a backend would
//! paint through `quadraui::testing::a11y::A11yScenario`, then assert the
//! resulting role/state tree as text. Covers the basic role/label/value
//! semantics for editor, text input, text display, terminal, diff view,
//! chart, image, minimap and canvas — see `src/a11y/content.rs`'s module
//! doc for the known scope limits (chart/canvas/image labelling, minimap
//! visibility).
//!
//! `markdown_a11y_children` builds its `Vec<A11yChild>` straight from a
//! `RenderedMarkdown`, with no `Surface` of its own to push through
//! `A11yScenario` — the tests right after it assert on that return value
//! directly. The tests below those wire that output into a real tree
//! through `A11yScenario::with_children`, the same seam an app uses to
//! attach it to a `TextDisplay` rendering markdown, and assert on the
//! resulting snapshot instead.

use quadraui::testing::a11y::A11yScenario;
use quadraui::{
    markdown_a11y_children, render_markdown_to_styled, A11yRole, Canvas, Chart, ChartKind, Color,
    Decoration, DiffEditability, DiffMode, DiffPane, DiffView, Editor, EditorLine, Image, ImageFit,
    ImageSource, Minimap, Rect, Series, StyledSpan, StyledText, Surface, Terminal, TerminalCell,
    TerminalCursorShape, TextDisplay, TextDisplayLine, TextInput, Theme, WidgetId,
};

const VIEWPORT: Rect = Rect {
    x: 0.0,
    y: 0.0,
    width: 80.0,
    height: 24.0,
};

fn rect(height: f32) -> Rect {
    Rect::new(0.0, 0.0, 80.0, height)
}

/// Matches `TerminalCell`'s own test helper in `src/primitives/terminal.rs`
/// — there is no production convenience constructor for this type.
fn cell(ch: char) -> TerminalCell {
    TerminalCell {
        text: ch.to_string(),
        fg: Color::rgb(255, 255, 255),
        bg: Color::rgb(0, 0, 0),
        bold: false,
        italic: false,
        underline: false,
        dim: false,
        selected: false,
        is_cursor: false,
        is_find_match: false,
        is_find_active: false,
        cursor_shape: TerminalCursorShape::Block,
        cursor_blinking: false,
    }
}

/// An `EditorLine` with everything a painter needs but an a11y value
/// does not care about left at its default — there is no production
/// convenience constructor for this type either.
fn editor_line(text: &str, idx: usize) -> EditorLine {
    EditorLine {
        raw_text: text.to_string(),
        gutter_text: format!("{:>3}", idx + 1),
        spans: Vec::new(),
        line_idx: idx,
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

fn editor_with(lines: Vec<EditorLine>) -> Surface {
    Surface::Editor {
        rect: VIEWPORT,
        editor: Editor::new(WidgetId::from("editor"), rect(10.0)).with_lines(lines),
    }
}

// ----------------------------------------------------------------- editor

#[test]
fn editor_exposes_role_and_joined_line_text() {
    let mut scenario = A11yScenario::new();
    scenario.push(editor_with(vec![editor_line("fn main() {}\n", 0)]));

    scenario.snapshot().assert_matches(
        "\
window \"quadraui\" [focused]
  editor [value=\"fn main() {}\"]",
    );
}

/// `EditorLine::raw_text` is not guaranteed to carry a trailing `\n` —
/// a buffer whose lines omit it must still read as separate lines, not
/// one run-together string.
#[test]
fn editor_with_newline_free_lines_still_reads_as_separate_lines() {
    let mut scenario = A11yScenario::new();
    scenario.push(editor_with(vec![
        editor_line("fn main() {", 0),
        editor_line("    one();", 1),
        editor_line("}", 2),
    ]));

    scenario.snapshot().assert_matches(
        "\
window \"quadraui\" [focused]
  editor [value=\"fn main() {\\n    one();\\n}\"]",
    );
}

/// Soft wrap is a painting decision: the continuation rows of one
/// buffer line share its `line_idx` and are flagged
/// `is_wrap_continuation`. They must rejoin the row they wrap from, or a
/// screen reader announces one logical line as several.
#[test]
fn editor_soft_wrap_continuations_rejoin_the_line_they_wrap_from() {
    let mut wrapped = editor_line("and keeps going", 0);
    wrapped.is_wrap_continuation = true;
    wrapped.segment_col_offset = 11;
    wrapped.gutter_text = "   ".to_string();

    let mut scenario = A11yScenario::new();
    scenario.push(editor_with(vec![
        editor_line("a long line", 0),
        wrapped,
        editor_line("next();", 1),
    ]));

    scenario.snapshot().assert_matches(
        "\
window \"quadraui\" [focused]
  editor [value=\"a long lineand keeps going\\nnext();\"]",
    );
}

/// An inline-completion ghost spills onto `is_ghost_continuation` rows
/// that carry no buffer text of their own. They are suggestion chrome,
/// not content: dropping them keeps a blank line out of the value.
#[test]
fn editor_ghost_continuation_rows_are_not_read_as_blank_lines() {
    let mut ghost_host = editor_line("let x = ", 0);
    ghost_host.ghost_suffix = Some("compute();\nmore();".to_string());
    let mut ghost_row = editor_line("", 0);
    ghost_row.is_ghost_continuation = true;
    ghost_row.gutter_text = "   ".to_string();

    let mut scenario = A11yScenario::new();
    scenario.push(editor_with(vec![
        ghost_host,
        ghost_row,
        editor_line("done();", 1),
    ]));

    scenario.snapshot().assert_matches(
        "\
window \"quadraui\" [focused]
  editor [value=\"let x = \\ndone();\"]",
    );
}

/// An editor with no buffer lines has no textual value at all, so it is
/// announced with just its role rather than an empty `value` token.
#[test]
fn editor_with_no_lines_has_no_value_token() {
    let mut scenario = A11yScenario::new();
    scenario.push(editor_with(Vec::new()));

    scenario.snapshot().assert_matches(
        "\
window \"quadraui\" [focused]
  editor",
    );
}

// ------------------------------------------------------------- text input

/// Typed text wins over the placeholder, and the placeholder never
/// becomes the field's name — it is a value fallback, matching the
/// convention the form-field text entries in `src/a11y/chrome.rs` use.
#[test]
fn text_input_typed_text_is_the_value_and_placeholder_is_not_the_label() {
    let ti = TextInput::new(WidgetId::from("search"))
        .with_lines(vec!["needle".to_string()])
        .with_placeholder("Search");

    let mut scenario = A11yScenario::new();
    scenario.push(Surface::TextInput {
        rect: rect(1.0),
        ti,
    });

    scenario.snapshot().assert_matches(
        "\
window \"quadraui\" [focused]
  text_input [value=\"needle\"]",
    );
}

#[test]
fn text_input_empty_falls_back_to_the_placeholder_as_its_value() {
    let ti = TextInput::new(WidgetId::from("search"))
        .with_lines(vec![String::new()])
        .with_placeholder("Search");

    let mut scenario = A11yScenario::new();
    scenario.push(Surface::TextInput {
        rect: rect(1.0),
        ti,
    });

    scenario.snapshot().assert_matches(
        "\
window \"quadraui\" [focused]
  text_input [value=\"Search\"]",
    );
}

/// An empty text box with no placeholder has no value to announce, so it
/// carries just its role.
#[test]
fn text_input_empty_with_no_placeholder_has_no_value() {
    let ti = TextInput::new(WidgetId::from("search")).with_lines(vec![String::new()]);

    let mut scenario = A11yScenario::new();
    scenario.push(Surface::TextInput {
        rect: rect(1.0),
        ti,
    });

    scenario.snapshot().assert_matches(
        "\
window \"quadraui\" [focused]
  text_input",
    );
}

// ----------------------------------------------------------- text display

#[test]
fn text_display_exposes_title_label_and_lines_as_children() {
    let mut td = TextDisplay::new(WidgetId::from("log"));
    td.title = Some(StyledText::plain("Build output"));
    td.lines.push(TextDisplayLine {
        spans: vec![StyledSpan::plain("Compiling quadraui v0.1.0")],
        decoration: Decoration::default(),
        timestamp: None,
    });
    td.lines.push(TextDisplayLine {
        spans: vec![StyledSpan::plain("Finished in 1.2s")],
        decoration: Decoration::default(),
        timestamp: None,
    });

    let mut scenario = A11yScenario::new();
    scenario.push(Surface::TextDisplay {
        rect: rect(5.0),
        td,
    });

    scenario.snapshot().assert_matches(
        "\
window \"quadraui\" [focused]
  document \"Build output\"
    label \"Compiling quadraui v0.1.0\" [pos=1/2]
    label \"Finished in 1.2s\" [pos=2/2]",
    );
}

/// The snapshot format quotes both labels and values, so text that
/// itself contains a quote, a backslash or a newline is escaped — one
/// node still occupies exactly one line and the delimiters stay
/// unambiguous.
#[test]
fn snapshot_escapes_quotes_backslashes_and_newlines_in_labels_and_values() {
    let mut td = TextDisplay::new(WidgetId::from("log"));
    td.title = Some(StyledText::plain("say \"hi\"\\now"));
    td.lines.push(TextDisplayLine {
        spans: vec![StyledSpan::plain("a \"quoted\" line")],
        decoration: Decoration::default(),
        timestamp: None,
    });

    let mut scenario = A11yScenario::new();
    scenario.push(Surface::TextDisplay {
        rect: rect(5.0),
        td,
    });
    scenario.push(Surface::TextInput {
        rect: rect(1.0),
        ti: TextInput::new(WidgetId::from("search"))
            .with_lines(vec!["say \"hi\"".to_string(), "line\\two".to_string()]),
    });

    scenario.snapshot().assert_matches(
        "\
window \"quadraui\" [focused]
  document \"say \\\"hi\\\"\\\\now\"
    label \"a \\\"quoted\\\" line\" [pos=1/1]
  text_input [value=\"say \\\"hi\\\"\\nline\\\\two\"]",
    );
}

/// A line's timestamp is part of what gets read out, not just what gets
/// painted — it must survive into the accessible child, prefixed exactly
/// as it would appear on screen.
#[test]
fn text_display_line_includes_timestamp_prefix() {
    let mut td = TextDisplay::new(WidgetId::from("log"));
    td.lines.push(TextDisplayLine {
        spans: vec![StyledSpan::plain("server started")],
        decoration: Decoration::default(),
        timestamp: Some("12:34:56".to_string()),
    });

    let mut scenario = A11yScenario::new();
    scenario.push(Surface::TextDisplay {
        rect: rect(5.0),
        td,
    });

    scenario.snapshot().assert_matches(
        "\
window \"quadraui\" [focused]
  document
    label \"12:34:56 server started\" [pos=1/1]",
    );
}

/// With no title set, the log exposes no accessible label — just its
/// document role — while still carrying one child per line.
#[test]
fn text_display_with_no_title_has_no_label() {
    let mut td = TextDisplay::new(WidgetId::from("log"));
    td.lines.push(TextDisplayLine {
        spans: vec![StyledSpan::plain("first")],
        decoration: Decoration::default(),
        timestamp: None,
    });
    td.lines.push(TextDisplayLine {
        spans: vec![StyledSpan::plain("second")],
        decoration: Decoration::default(),
        timestamp: None,
    });

    let mut scenario = A11yScenario::new();
    scenario.push(Surface::TextDisplay {
        rect: rect(5.0),
        td,
    });

    scenario.snapshot().assert_matches(
        "\
window \"quadraui\" [focused]
  document
    label \"first\" [pos=1/2]
    label \"second\" [pos=2/2]",
    );
}

/// A blank line contributes no child of its own, but every buffer line
/// still counts toward the `one of N` position — so the line after a blank
/// keeps the number a sighted user would count to.
#[test]
fn text_display_blank_line_counts_toward_position_but_emits_no_child() {
    let mut td = TextDisplay::new(WidgetId::from("log"));
    td.lines.push(TextDisplayLine {
        spans: vec![StyledSpan::plain("before")],
        decoration: Decoration::default(),
        timestamp: None,
    });
    td.lines.push(TextDisplayLine {
        spans: Vec::new(),
        decoration: Decoration::default(),
        timestamp: None,
    });
    td.lines.push(TextDisplayLine {
        spans: vec![StyledSpan::plain("after")],
        decoration: Decoration::default(),
        timestamp: None,
    });

    let mut scenario = A11yScenario::new();
    scenario.push(Surface::TextDisplay {
        rect: rect(5.0),
        td,
    });

    scenario.snapshot().assert_matches(
        "\
window \"quadraui\" [focused]
  document
    label \"before\" [pos=1/3]
    label \"after\" [pos=3/3]",
    );
}

// --------------------------------------------------------------- terminal

#[test]
fn terminal_exposes_role_and_visible_grid_as_value() {
    let term = Terminal {
        id: "term".into(),
        cells: vec![vec![cell('h'), cell('i')]],
        scrollbar: None,
    };

    let mut scenario = A11yScenario::new();
    scenario.push(Surface::Terminal {
        rect: rect(1.0),
        term,
    });

    scenario.snapshot().assert_matches(
        "\
window \"quadraui\" [focused]
  terminal [value=\"hi\"]",
    );
}

/// Never-written cells default to a single space (vt100 convention) —
/// a row padded out to the grid width must not read as a wall of spaces.
#[test]
fn terminal_trims_trailing_blank_padding_from_each_row() {
    let blank = cell(' ');
    let term = Terminal {
        id: "term".into(),
        cells: vec![
            vec![
                cell('h'),
                cell('i'),
                blank.clone(),
                blank.clone(),
                blank.clone(),
            ],
            vec![
                cell('o'),
                cell('k'),
                blank.clone(),
                blank.clone(),
                blank.clone(),
            ],
        ],
        scrollbar: None,
    };

    let mut scenario = A11yScenario::new();
    scenario.push(Surface::Terminal {
        rect: rect(2.0),
        term,
    });

    scenario.snapshot().assert_matches(
        "\
window \"quadraui\" [focused]
  terminal [value=\"hi\\nok\"]",
    );
}

/// A fullwidth glyph occupies two grid columns: the glyph's own cell
/// plus a spacer cell that normalises to a single space. The spacer is
/// painting geometry, not text — reading it out would turn `日本語` into
/// `日 本 語`.
#[test]
fn terminal_drops_the_spacer_column_after_a_wide_glyph() {
    let term = Terminal {
        id: "term".into(),
        cells: vec![vec![
            cell('日'),
            cell(' '),
            cell('本'),
            cell(' '),
            cell('語'),
            cell(' '),
        ]],
        scrollbar: None,
    };

    let mut scenario = A11yScenario::new();
    scenario.push(Surface::Terminal {
        rect: rect(1.0),
        term,
    });

    scenario.snapshot().assert_matches(
        "\
window \"quadraui\" [focused]
  terminal [value=\"日本語\"]",
    );
}

/// The spacer skip is keyed on the preceding cell's width, so a genuine
/// space between narrow glyphs still reads as a word break.
#[test]
fn terminal_keeps_spaces_between_narrow_glyphs() {
    let term = Terminal {
        id: "term".into(),
        cells: vec![vec![cell('a'), cell(' '), cell('b')]],
        scrollbar: None,
    };

    let mut scenario = A11yScenario::new();
    scenario.push(Surface::Terminal {
        rect: rect(1.0),
        term,
    });

    scenario.snapshot().assert_matches(
        "\
window \"quadraui\" [focused]
  terminal [value=\"a b\"]",
    );
}

/// An empty grid has no visible text, so the terminal is announced with
/// just its role rather than an empty `value`.
#[test]
fn terminal_with_empty_grid_has_no_value() {
    let term = Terminal {
        id: "term".into(),
        cells: Vec::new(),
        scrollbar: None,
    };

    let mut scenario = A11yScenario::new();
    scenario.push(Surface::Terminal {
        rect: rect(1.0),
        term,
    });

    scenario.snapshot().assert_matches(
        "\
window \"quadraui\" [focused]
  terminal",
    );
}

// -------------------------------------------------------------- diff view

#[test]
fn diff_view_exposes_read_only_labelled_panes() {
    let view = DiffView {
        id: "diff".into(),
        left: "old\n".to_string(),
        right: "new\n".to_string(),
        left_label: Some("a/file.rs".to_string()),
        right_label: Some("b/file.rs".to_string()),
        hunks: Vec::new(),
        mode: DiffMode::SideBySide,
        editability: DiffEditability::ReadOnly,
        scroll_offset: 0,
        focused_pane: DiffPane::Left,
        has_focus: false,
    };

    let mut scenario = A11yScenario::new();
    scenario.push(Surface::DiffView {
        rect: rect(10.0),
        view,
    });

    scenario.snapshot().assert_matches(
        "\
window \"quadraui\" [focused]
  document
    document \"a/file.rs\" [readonly value=\"old\"]
    document \"b/file.rs\" [readonly value=\"new\"]",
    );
}

#[test]
fn diff_view_right_editable_pane_drops_readonly_token() {
    let view = DiffView {
        id: "diff".into(),
        left: "old\n".to_string(),
        right: "new\n".to_string(),
        left_label: None,
        right_label: None,
        hunks: Vec::new(),
        mode: DiffMode::SideBySide,
        editability: DiffEditability::RightEditable,
        scroll_offset: 0,
        focused_pane: DiffPane::Left,
        has_focus: false,
    };

    let mut scenario = A11yScenario::new();
    scenario.push(Surface::DiffView {
        rect: rect(10.0),
        view,
    });

    scenario.snapshot().assert_matches(
        "\
window \"quadraui\" [focused]
  document
    document \"Left\" [readonly value=\"old\"]
    document \"Right\" [value=\"new\"]",
    );
}

/// [`DiffEditability::RightEditable`] names the right pane of a
/// side-by-side diff. Unified mode has no right pane — one interleaved
/// column of `-`/`+` rows is not typeable — so it stays read-only.
#[test]
fn diff_view_unified_pane_stays_read_only_when_right_is_editable() {
    let view = DiffView {
        id: "diff".into(),
        left: "old\n".to_string(),
        right: "new\n".to_string(),
        left_label: None,
        right_label: None,
        hunks: Vec::new(),
        mode: DiffMode::Unified,
        editability: DiffEditability::RightEditable,
        scroll_offset: 0,
        focused_pane: DiffPane::Left,
        has_focus: false,
    };

    let mut scenario = A11yScenario::new();
    scenario.push(Surface::DiffView {
        rect: rect(10.0),
        view,
    });

    scenario.snapshot().assert_matches(
        "\
window \"quadraui\" [focused]
  document
    document [readonly]",
    );
}

/// [`DiffMode::Unified`] has only one column to read — the hunk's `@@ … @@`
/// header followed by its `+`/`-`/` ` prefixed rows — matching exactly what
/// every backend's `draw_unified` paints.
#[test]
fn diff_view_unified_mode_exposes_single_pane_with_hunk_text() {
    use quadraui::{DiffHunk, DiffRow, DiffRowKind};

    let view = DiffView {
        id: "diff".into(),
        left: "old\n".to_string(),
        right: "new\n".to_string(),
        left_label: Some("a/file.rs".to_string()),
        right_label: Some("b/file.rs".to_string()),
        hunks: vec![DiffHunk {
            left_start: 1,
            right_start: 1,
            rows: vec![
                DiffRow {
                    left: Some("old".to_string()),
                    right: None,
                    kind: DiffRowKind::Removed,
                },
                DiffRow {
                    left: None,
                    right: Some("new".to_string()),
                    kind: DiffRowKind::Added,
                },
            ],
        }],
        mode: DiffMode::Unified,
        editability: DiffEditability::ReadOnly,
        scroll_offset: 0,
        focused_pane: DiffPane::Left,
        has_focus: false,
    };

    let mut scenario = A11yScenario::new();
    scenario.push(Surface::DiffView {
        rect: rect(10.0),
        view,
    });

    scenario.snapshot().assert_matches(
        "\
window \"quadraui\" [focused]
  document
    document [readonly value=\"@@ -1,1 +1,1 @@\\n-old\\n+new\"]",
    );
}

// ------------------------------------------------------------------ chart

#[test]
fn chart_with_no_app_label_is_unlabelled() {
    let chart = Chart {
        id: "cpu".into(),
        kind: ChartKind::Sparkline,
        series: vec![Series {
            label: "cpu".to_string(),
            data: vec![0.1, 0.4, 0.2],
            color: None,
            fill: false,
        }],
        x_label: None,
        y_label: None,
        y_range: None,
        x_range: None,
        show_legend: false,
        y_ticks: None,
        x_ticks: None,
        show_grid: false,
    };

    let mut scenario = A11yScenario::new();
    scenario.push(Surface::Chart {
        rect: rect(3.0),
        chart,
        hovered_point: None,
        crosshair_x: None,
    });

    // No app-supplied label override: the chart's visual series data has
    // no textual equivalent, so it is announced with just its role.
    scenario.snapshot().assert_matches(
        "\
window \"quadraui\" [focused]
  chart",
    );
}

/// A chart's visual series data has no textual equivalent, so an app that
/// turns on the legend — the only visual cue that series names are meant
/// to be read — gets them joined into the accessible name.
#[test]
fn chart_with_legend_shown_uses_series_labels_as_name() {
    let chart = Chart {
        id: "cpu".into(),
        kind: ChartKind::Sparkline,
        series: vec![
            Series {
                label: "cpu".to_string(),
                data: vec![0.1, 0.4, 0.2],
                color: None,
                fill: false,
            },
            Series {
                label: "memory".to_string(),
                data: vec![0.3, 0.5, 0.2],
                color: None,
                fill: false,
            },
        ],
        x_label: None,
        y_label: None,
        y_range: None,
        x_range: None,
        show_legend: true,
        y_ticks: None,
        x_ticks: None,
        show_grid: false,
    };

    let mut scenario = A11yScenario::new();
    scenario.push(Surface::Chart {
        rect: rect(3.0),
        chart,
        hovered_point: None,
        crosshair_x: None,
    });

    scenario.snapshot().assert_matches(
        "\
window \"quadraui\" [focused]
  chart \"cpu, memory\"",
    );
}

/// An axis label takes priority over the legend fallback, since it is a
/// more deliberate app-authored name than a joined series-label list.
#[test]
fn chart_with_axis_label_prefers_it_over_legend() {
    let chart = Chart {
        id: "cpu".into(),
        kind: ChartKind::Sparkline,
        series: vec![Series {
            label: "cpu".to_string(),
            data: vec![0.1, 0.4, 0.2],
            color: None,
            fill: false,
        }],
        x_label: Some("Time".to_string()),
        y_label: Some("Percent".to_string()),
        y_range: None,
        x_range: None,
        show_legend: true,
        y_ticks: None,
        x_ticks: None,
        show_grid: false,
    };

    let mut scenario = A11yScenario::new();
    scenario.push(Surface::Chart {
        rect: rect(3.0),
        chart,
        hovered_point: None,
        crosshair_x: None,
    });

    scenario.snapshot().assert_matches(
        "\
window \"quadraui\" [focused]
  chart \"Percent\"",
    );
}

/// With no `y_label`, the `x_label` is the next app-authored name in the
/// fallback chain — preferred over the legend even when it is off.
#[test]
fn chart_falls_back_to_x_label_when_no_y_label() {
    let chart = Chart {
        id: "cpu".into(),
        kind: ChartKind::Sparkline,
        series: vec![Series {
            label: "cpu".to_string(),
            data: vec![0.1, 0.4, 0.2],
            color: None,
            fill: false,
        }],
        x_label: Some("Time".to_string()),
        y_label: None,
        y_range: None,
        x_range: None,
        show_legend: false,
        y_ticks: None,
        x_ticks: None,
        show_grid: false,
    };

    let mut scenario = A11yScenario::new();
    scenario.push(Surface::Chart {
        rect: rect(3.0),
        chart,
        hovered_point: None,
        crosshair_x: None,
    });

    scenario.snapshot().assert_matches(
        "\
window \"quadraui\" [focused]
  chart \"Time\"",
    );
}

/// The legend fallback only contributes names that are actually set:
/// a legend of blank series labels leaves the chart unlabelled rather
/// than announcing a run of empty strings.
#[test]
fn chart_with_legend_but_blank_series_labels_is_unlabelled() {
    let chart = Chart {
        id: "cpu".into(),
        kind: ChartKind::Sparkline,
        series: vec![Series {
            label: String::new(),
            data: vec![0.1, 0.4, 0.2],
            color: None,
            fill: false,
        }],
        x_label: None,
        y_label: None,
        y_range: None,
        x_range: None,
        show_legend: true,
        y_ticks: None,
        x_ticks: None,
        show_grid: false,
    };

    let mut scenario = A11yScenario::new();
    scenario.push(Surface::Chart {
        rect: rect(3.0),
        chart,
        hovered_point: None,
        crosshair_x: None,
    });

    scenario.snapshot().assert_matches(
        "\
window \"quadraui\" [focused]
  chart",
    );
}

// ------------------------------------------------------------------ image

#[test]
fn image_falls_back_to_fallback_text_as_label() {
    let image = Image {
        id: "logo".into(),
        source: ImageSource::Path("logo.png".into()),
        intrinsic_size: None,
        fit: ImageFit::Contain,
        fallback_text: "Company logo".to_string(),
    };

    let mut scenario = A11yScenario::new();
    scenario.push(Surface::Image {
        rect: rect(4.0),
        image,
    });

    scenario.snapshot().assert_matches(
        "\
window \"quadraui\" [focused]
  image \"Company logo\"",
    );
}

#[test]
fn image_with_empty_fallback_text_is_unlabelled() {
    let image = Image {
        id: "deco".into(),
        source: ImageSource::Path("deco.png".into()),
        intrinsic_size: None,
        fit: ImageFit::Contain,
        fallback_text: String::new(),
    };

    let mut scenario = A11yScenario::new();
    scenario.push(Surface::Image {
        rect: rect(4.0),
        image,
    });

    scenario.snapshot().assert_matches(
        "\
window \"quadraui\" [focused]
  image",
    );
}

// ---------------------------------------------------------------- minimap

#[test]
fn minimap_is_hidden_presentation() {
    let minimap = Minimap {
        id: "minimap".into(),
        lines: Vec::new(),
        syntax_spans: Vec::new(),
        visible_row_start: 0,
        visible_row_count: 0,
        total_buffer_lines: 0,
    };

    let mut scenario = A11yScenario::new();
    scenario.push(Surface::Minimap {
        rect: rect(10.0),
        minimap,
    });

    scenario.snapshot().assert_matches(
        "\
window \"quadraui\" [focused]
  presentation \"Minimap\" [hidden]",
    );
}

// ----------------------------------------------------------------- canvas

#[test]
fn canvas_with_no_app_label_is_unlabelled() {
    let canvas = Canvas {
        id: "sparkline".into(),
        ops: Vec::new(),
    };

    let mut scenario = A11yScenario::new();
    scenario.push(Surface::Canvas {
        rect: rect(4.0),
        canvas,
    });

    scenario.snapshot().assert_matches(
        "\
window \"quadraui\" [focused]
  canvas",
    );
}

// ----------------------------------------------- markdown a11y children

#[test]
fn markdown_heading_lines_expose_role_and_level() {
    let theme = Theme::default();
    let rendered = render_markdown_to_styled("# Title\n## Section\nbody text", &theme);

    let children = markdown_a11y_children(&rendered);
    assert_eq!(children.len(), 3);

    assert_eq!(children[0].info.a11y_role, Some(A11yRole::Heading));
    assert_eq!(children[0].info.level, Some(1));
    assert_eq!(children[0].info.a11y_label.as_deref(), Some("Title"));

    assert_eq!(children[1].info.a11y_role, Some(A11yRole::Heading));
    assert_eq!(children[1].info.level, Some(2));
    assert_eq!(children[1].info.a11y_label.as_deref(), Some("Section"));

    assert_ne!(children[2].info.a11y_role, Some(A11yRole::Heading));
    assert_eq!(children[2].info.level, None);
    assert_eq!(children[2].info.a11y_label.as_deref(), Some("body text"));
}

#[test]
fn markdown_link_lines_expose_role_and_target() {
    let theme = Theme::default();
    let rendered =
        render_markdown_to_styled("See the [docs](https://example.com/docs) for more.", &theme);

    let children = markdown_a11y_children(&rendered);
    assert_eq!(children.len(), 1);

    let line = &children[0];
    assert_eq!(line.children.len(), 1);

    let link = &line.children[0];
    assert_eq!(link.info.a11y_role, Some(A11yRole::Link));
    assert_eq!(link.info.a11y_label.as_deref(), Some("docs"));
    assert_eq!(link.info.value.as_deref(), Some("https://example.com/docs"));
}

/// Wires `markdown_a11y_children`'s output into a real `A11yTree` the way
/// an app renders markdown into a `TextDisplay`: attach it to that
/// `TextDisplay`'s own widget id via `A11yScenario::with_children`, in
/// place of the plain per-line labels `TextDisplay::a11y_children` would
/// otherwise derive. The snapshot carries the heading's level and the
/// link's target, proving the structure reaches the tree a backend
/// actually publishes rather than staying a value only the mapping
/// function's own caller can see.
#[test]
fn markdown_a11y_children_wires_into_a_text_display_tree() {
    let theme = Theme::default();
    let rendered = render_markdown_to_styled(
        "# Title\nSee the [docs](https://example.com/docs) for more.",
        &theme,
    );
    let children = markdown_a11y_children(&rendered);

    let td = TextDisplay::new(WidgetId::from("notes"));
    let widget = td.id;

    let mut scenario = A11yScenario::new().with_children(widget, children);
    scenario.push(Surface::TextDisplay {
        rect: rect(5.0),
        td,
    });

    scenario.snapshot().assert_matches(
        "\
window \"quadraui\" [focused]
  document
    heading \"Title\" [level=1 pos=1/2]
    label \"See the docs for more.\" [pos=2/2]
      link \"docs\" [value=\"https://example.com/docs\"]",
    );
}
