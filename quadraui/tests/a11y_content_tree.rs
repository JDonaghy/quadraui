//! Accessibility-tree snapshots for content primitives.
//!
//! Mirrors `a11y_chrome_tree.rs`: push the same descriptors a backend would
//! paint through `quadraui::testing::a11y::A11yScenario`, then assert the
//! resulting role/state tree as text. Covers the basic role/label/value
//! semantics for editor, text input, text display, terminal, diff view,
//! chart, image, minimap and canvas — see `src/a11y/content.rs`'s module
//! doc for the known scope limits (markdown structure, chart/canvas/image
//! labelling, minimap visibility).

use quadraui::testing::a11y::A11yScenario;
use quadraui::{
    Canvas, Chart, ChartKind, Color, Decoration, DiffEditability, DiffMode, DiffPane, DiffView,
    Editor, EditorLine, Image, ImageFit, ImageSource, Minimap, Rect, Series, StyledSpan,
    StyledText, Surface, Terminal, TerminalCell, TerminalCursorShape, TextDisplay, TextDisplayLine,
    TextInput, WidgetId,
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

// ----------------------------------------------------------------- editor

#[test]
fn editor_exposes_role_and_joined_line_text() {
    let editor = Editor::new(WidgetId::from("editor"), rect(10.0)).with_lines(vec![EditorLine {
        raw_text: "fn main() {}\n".to_string(),
        gutter_text: "  1".to_string(),
        spans: Vec::new(),
        line_idx: 0,
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
    }]);

    let mut scenario = A11yScenario::new();
    scenario.push(Surface::Editor {
        rect: VIEWPORT,
        editor,
    });

    scenario.snapshot().assert_matches(
        "\
window \"quadraui\" [focused]
  editor [value=\"fn main() {}\n\"]",
    );
}

// ------------------------------------------------------------- text input

#[test]
fn text_input_exposes_role_placeholder_label_and_value() {
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
  text_input \"Search\" [value=\"needle\"]",
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
    label \"Compiling quadraui v0.1.0\"
    label \"Finished in 1.2s\"",
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
    document \"a/file.rs\" [readonly value=\"old\n\"]
    document \"b/file.rs\" [readonly value=\"new\n\"]",
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
    document \"Left\" [readonly value=\"old\n\"]
    document \"Right\" [value=\"new\n\"]",
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
