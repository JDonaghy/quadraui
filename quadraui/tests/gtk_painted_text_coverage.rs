//! Painted-text coverage self-test for the GTK backend (quadraui#489).
//!
//! `GtkDriver::find` / `find_bounds` / `screen_contains` are backed by
//! `GtkBackend::painted_text`. Until #489 only three trait methods
//! (`draw_status_bar`, `draw_data_table`, `draw_pipeline_view`) recorded
//! into it — 2 of 37 rasterisers at the time the issue was filed — which
//! structurally capped the GTK example-driver suite at `PipelineApp`
//! (4 tests, against the TUI suite's 84): the tests could not be
//! *authored*, because no coordinate-free way existed to locate a tree
//! row, a toolbar button, or a table cell.
//!
//! This file is the coverage gate for the fix. Each test paints one
//! exemplar of a text-bearing primitive — through a real example app, so
//! the fixtures cannot drift from what the examples actually render — and
//! asserts the driver can *locate* its text, i.e. that the primitive
//! reports into the painted-text map at all. It doubles as the Tier C0
//! seed data for story B5.
//!
//! Not covered here, deliberately:
//!
//! * **`draw_terminal`** — the cell-grid rasteriser paints one Pango run
//!   *per cell*, so routing it through the recorder would push thousands
//!   of single-character entries into the map per frame. It needs the
//!   cheaper row-coalescing path the issue flags as optional; see
//!   `src/gtk/painted_text.rs`.
//! * **`draw_split` / `draw_split_tree` / `draw_scrollbar` /
//!   `draw_drop_overlay`** — these paint no text at all (dividers,
//!   tracks, tint rects), so there is nothing to record. The labels a
//!   `Split` appears to have belong to whatever the app paints into
//!   `first_bounds` / `second_bounds`.
#![cfg(feature = "gtk")]
// The `examples/common/*` fixtures below are shared with the example
// binaries and the other driver suites, so each carries API this file
// doesn't exercise (`DataTableApp::resolved_column_widths`,
// `ChartApp::last_chart_rect`, …). Dead-code analysis is per compilation
// unit, so that surfaces here as noise about *their* code, not ours.
#![allow(dead_code)]

use quadraui::gtk::testing::{driver_with_shell, GtkDriver};
use quadraui::{
    compute_find_replace_hit_regions, AppLogic, Backend, CommandCenter, CompletionItem,
    CompletionItemMeasure, CompletionKind, Completions, FindReplacePanel, Reaction, Rect,
    ResolvedPlacement, StyledText, Tooltip, TooltipBorder, TooltipChrome, TooltipLayout,
    TooltipPlacement, UiEvent, WidgetId,
};

#[path = "../examples/common/ai_transcript.rs"]
mod ai_transcript;
#[path = "../examples/common/board_app.rs"]
mod board_app;
#[path = "../examples/common/chart_app.rs"]
mod chart_app;
#[path = "../examples/common/data_table_app.rs"]
mod data_table_app;
#[path = "../examples/common/dialog_table_demo.rs"]
mod dialog_table_demo;
#[path = "../examples/common/diff_view_demo.rs"]
mod diff_view_demo;
#[path = "../examples/common/editor_font_demo.rs"]
mod editor_font_demo;
#[path = "../examples/common/form_groups.rs"]
mod form_groups;
#[path = "../examples/common/full_chrome_demo.rs"]
mod full_chrome_demo;
#[path = "../examples/common/indicators_app.rs"]
mod indicators_app;
#[path = "../examples/common/markdown_demo.rs"]
mod markdown_demo;
#[path = "../examples/common/menu_bar_app.rs"]
mod menu_bar_app;
#[path = "../examples/common/multi_tree.rs"]
mod multi_tree;
#[path = "../examples/common/palette_dual_mode_app.rs"]
mod palette_dual_mode_app;
#[path = "../examples/common/panel_app.rs"]
mod panel_app;
#[path = "../examples/common/shell_app.rs"]
mod shell_app;
#[path = "../examples/common/sidebar_panel_app.rs"]
mod sidebar_panel_app;
#[path = "../examples/common/tab_group_demo.rs"]
mod tab_group_demo;
#[path = "../examples/common/text_input_demo.rs"]
mod text_input_demo;
#[path = "../examples/common/toast_app.rs"]
mod toast_app;
#[path = "../examples/common/toolbar_app.rs"]
mod toolbar_app;

const W: i32 = 1000;
const H: i32 = 700;

/// Assert every `(primitive, needle)` pair resolves through
/// `find_bounds`, and that the rect it resolves to is one `click` could
/// actually use: non-degenerate and on-surface.
///
/// `find`'s whole contract is *locate targets with `find`, never hardcode
/// coords* — a recorded rect of zero size, or one off the surface, would
/// satisfy "is recorded" while still being useless to a driver test.
fn assert_locatable<A: AppLogic>(driver: &GtkDriver<A>, cases: &[(&str, &str)]) {
    let missing: Vec<&str> = cases
        .iter()
        .filter(|(_, needle)| driver.find_bounds(needle).is_none())
        .map(|(primitive, _)| *primitive)
        .collect();
    assert!(
        missing.is_empty(),
        "no locatable painted text for: {missing:?}\npainted: {:?}",
        driver.painted_texts()
    );

    for (primitive, needle) in cases {
        let b = driver.find_bounds(needle).expect("asserted above");
        assert!(
            b.width > 0.0 && b.height > 0.0,
            "{primitive}: {needle:?} recorded a degenerate rect {b:?}"
        );
        let (cx, cy) = driver.find(needle).expect("asserted above");
        assert!(
            cx >= 0.0 && cx <= W as f32 && cy >= 0.0 && cy <= H as f32,
            "{primitive}: find({needle:?}) centre ({cx}, {cy}) is off the {W}x{H} surface"
        );
    }
}

// ─── Chrome: activity bar, tree, status bar, sidebar header ─────────────────

#[test]
fn shell_app_paints_locatable_chrome_and_tree_rows() {
    let driver = GtkDriver::new(shell_app::ShellApp::new(), W, H);
    assert_locatable(
        &driver,
        &[
            // AppShell paints the panel title + main-content label as
            // StatusBars — the one primitive that already recorded.
            ("draw_status_bar", "EXPLORER"),
            ("draw_status_bar", "Selected: nothing selected"),
            // `draw_tree`: sidebar rows, including a collapsed-branch
            // chevron row and a nested leaf.
            ("draw_tree", "OPEN EDITORS"),
            ("draw_tree", "backend.rs"),
            ("draw_tree", "PROJECT"),
            // `draw_activity_bar` paints icon glyphs (its labels are
            // tooltips, which are never painted). Recorded through the
            // rasteriser's `cr.translate(rect.x, rect.y)`, so this also
            // pins the user→device coordinate conversion.
            ("draw_activity_bar", "E"),
        ],
    );
}

/// The activity bar is painted under an active `cr.translate(...)` (see
/// `GtkBackend::draw_activity_bar`), which is the one place a naive
/// current-point read would record bar-local instead of surface
/// coordinates. Its glyphs must land inside the bar's own column.
#[test]
fn activity_bar_glyphs_record_absolute_surface_coordinates() {
    let driver = GtkDriver::new(shell_app::ShellApp::new(), W, H);

    let explorer = driver
        .find_bounds("E")
        .expect("activity bar explorer glyph should be painted");
    let settings = driver
        .find_bounds("*")
        .expect("bottom-pinned settings glyph should be painted");

    // ShellApp reserves 3 line-heights for the bar at the far left.
    assert!(
        explorer.x < 64.0,
        "explorer glyph should sit in the left-hand activity bar, got {explorer:?}"
    );
    // Bottom-pinned items paint near the bottom of the surface — the
    // give-away that the translate was applied (a bar-local y would put
    // this at the same y as the top items).
    assert!(
        settings.y > explorer.y,
        "bottom-pinned settings glyph ({settings:?}) must record below the \
         top-pinned explorer glyph ({explorer:?})"
    );
}

// ─── Toolbar / menu bar / tab bar / command line ────────────────────────────

#[test]
fn toolbar_app_paints_locatable_buttons_and_labels() {
    let driver = GtkDriver::new(toolbar_app::ToolbarApp::new(), W, H);
    assert_locatable(
        &driver,
        &[
            ("draw_toolbar", "Continue"),
            ("draw_toolbar", "Pause"),
            ("draw_toolbar", "Filter"),
            // Disabled action: painted dimmed, still locatable.
            ("draw_toolbar", "Debug"),
            // `ToolbarButton::Label` (non-clickable state text).
            ("draw_toolbar", "running"),
        ],
    );
}

#[test]
fn full_chrome_demo_paints_locatable_menu_bar_and_command_line() {
    let driver = driver_with_shell(
        full_chrome_demo::FullChromeDemo::new(),
        full_chrome_demo::FullChromeDemo::config(),
        W,
        H,
    );
    assert_locatable(
        &driver,
        &[
            ("draw_menu_bar", "File"),
            ("draw_menu_bar", "Help"),
            ("draw_command_line", ":command-line-slot"),
            ("draw_status_bar", "Ln 1, Col 1"),
        ],
    );
}

#[test]
fn multi_tree_paints_locatable_section_headers_and_rows() {
    let driver = GtkDriver::new(multi_tree::DebugSidebar::new(), W, H);
    assert_locatable(
        &driver,
        &[
            ("draw_multi_section_view", "VARIABLES"),
            ("draw_multi_section_view", "WATCH"),
            ("draw_multi_section_view", "CALL STACK"),
            ("draw_tree", "frame0"),
            // The fourth section header ("BREAKPOINTS") is deliberately
            // absent: at this surface height the app paints it straddling
            // the bottom edge, so its recorded rect is legitimately
            // (partly) off-surface and `assert_locatable`'s clickability
            // check would — correctly — reject it.
        ],
    );
}

// ─── Data / content primitives ──────────────────────────────────────────────

#[test]
fn data_table_app_paints_locatable_header_body_and_footer_cells() {
    let driver = GtkDriver::new(data_table_app::DataTableApp::new(), W, H);
    assert_locatable(
        &driver,
        &[
            ("draw_data_table", "Restarts"),
            ("draw_data_table", "nginx-7d9b8c66b-x2j4k"),
            ("draw_data_table", "CrashLoopBackOff"),
            ("draw_data_table", "20 pods"),
        ],
    );
}

#[test]
fn board_app_paints_locatable_columns_cards_and_hints() {
    let driver = GtkDriver::new(board_app::BoardApp::new(), W, H);
    assert_locatable(
        &driver,
        &[
            ("draw_board", "Backlog"),
            ("draw_board", "In Progress"),
            ("draw_board", "Add dark-mode toggle"),
            ("draw_board", "Waiting for design sign-off"),
        ],
    );
}

#[test]
fn diff_view_demo_paints_locatable_pane_labels_and_rows() {
    let driver = GtkDriver::new(diff_view_demo::DiffViewApp::new(), W, H);
    assert_locatable(
        &driver,
        &[
            ("draw_diff_view", "original"),
            ("draw_diff_view", "modified"),
            ("draw_diff_view", "(a + b) as i64"),
        ],
    );
}

#[test]
fn editor_font_demo_paints_locatable_gutter_and_code_lines() {
    let driver = driver_with_shell(
        editor_font_demo::EditorFontDemo,
        editor_font_demo::EditorFontDemo::config(),
        W,
        H,
    );
    assert_locatable(
        &driver,
        &[
            ("draw_editor", "The quick brown fox jumps over the lazy dog"),
            ("draw_editor", "set_editor_font() painted this"),
        ],
    );
}

#[test]
fn ai_transcript_paints_locatable_message_rows_and_input() {
    let driver = GtkDriver::new(ai_transcript::AiTranscript::new(), W, H);
    assert_locatable(
        &driver,
        &[
            ("draw_message_list", "Connected."),
            ("draw_text_input", "Type a message…"),
        ],
    );
}

// `MarkdownDemo::render` paints exclusively through
// `backend.draw_rich_text_popup` (there is no `draw_message_list` call
// anywhere in that file) — this doubles as the `draw_rich_text_popup`
// coverage exemplar.
#[test]
fn markdown_demo_paints_locatable_rich_text_rows() {
    let driver = GtkDriver::new(markdown_demo::MarkdownDemo::new(), W, H);
    assert_locatable(
        &driver,
        &[
            ("draw_rich_text_popup", "Headings scale up"),
            ("draw_rich_text_popup", "no raw backticks here"),
        ],
    );
}

#[test]
fn tab_group_demo_paints_locatable_tab_labels() {
    let driver = GtkDriver::new(tab_group_demo::TabGroupDemo::new(), W, H);
    assert_locatable(
        &driver,
        &[
            ("draw_tab_bar", "main.rs"),
            ("draw_tab_bar", "lib.rs"),
            ("draw_tab_bar", "Cargo.toml"),
        ],
    );
}

#[test]
fn menu_bar_app_paints_locatable_context_menu_items() {
    let mut driver = GtkDriver::new(menu_bar_app::MenuBarApp::new(), W, H);
    let (fx, fy) = driver
        .find("File")
        .expect("File label should be painted by draw_menu_bar");
    driver.click(fx, fy);
    assert_locatable(
        &driver,
        &[
            ("draw_context_menu", "New File"),
            ("draw_context_menu", "Open File"),
            ("draw_context_menu", "Save"),
            ("draw_context_menu", "Quit"),
        ],
    );
}

#[test]
fn text_input_demo_paints_locatable_placeholder() {
    let driver = GtkDriver::new(text_input_demo::TextInputDemo::new(), W, H);
    assert_locatable(&driver, &[("draw_text_input", "Type something.")]);
}

#[test]
fn chart_app_paints_locatable_axis_ticks_and_legend() {
    // The default Sparkline kind has no axes or legend to paint (nothing
    // to record — correctly). '2' switches to Line, which paints y ticks,
    // both axis titles, and the series legend.
    let mut driver = GtkDriver::new(chart_app::ChartApp::new(), W, H);
    driver.type_char('2');
    assert_locatable(
        &driver,
        &[
            ("draw_chart", "CPU"),
            ("draw_chart", "Memory"),
            ("draw_chart", "Time (s)"),
            ("draw_chart", "Usage"),
            ("draw_chart", "100"),
        ],
    );
}

// ─── Overlays / chrome extras ───────────────────────────────────────────────

#[test]
fn dialog_table_demo_paints_locatable_title_table_and_buttons() {
    let driver = GtkDriver::new(dialog_table_demo::DialogTableDemo::new(), W, H);
    assert_locatable(
        &driver,
        &[
            ("draw_dialog", "Source Control — Keybindings"),
            ("draw_dialog", "Toggle inline diff"),
            ("draw_dialog", "Close"),
        ],
    );
}

#[test]
fn toast_app_paints_locatable_toast_title_and_body() {
    let driver = GtkDriver::new(toast_app::ToastApp::new(), W, H);
    assert_locatable(
        &driver,
        &[
            ("draw_toast_stack", "Welcome"),
            ("draw_toast_stack", "Press 1-4 to add toasts"),
        ],
    );
}

#[test]
fn palette_dual_mode_paints_locatable_title_query_and_items() {
    let driver = GtkDriver::new(palette_dual_mode_app::PaletteDualModeApp::new(), W, H);
    assert_locatable(
        &driver,
        &[
            ("draw_palette", "Switch Branch"),
            ("draw_palette", "feature/gtk-rasteriser"),
            ("draw_palette", "docs/testing-guide"),
        ],
    );
}

#[test]
fn panel_app_paints_locatable_title_bar_and_actions() {
    let driver = GtkDriver::new(panel_app::PanelApp::new(), W, H);
    assert_locatable(&driver, &[("draw_panel", "Demo Panel")]);
}

#[test]
fn sidebar_panel_app_paints_locatable_toolbar_and_list_rows() {
    let driver = GtkDriver::new(sidebar_panel_app::SidebarPanelApp::new(), W, H);
    assert_locatable(
        &driver,
        &[
            ("draw_sidebar_panel", "Filter (f)"),
            ("draw_sidebar_panel", "Clear (c)"),
            ("draw_list", "Review PR #257"),
        ],
    );
}

#[test]
fn form_groups_paints_locatable_field_labels_and_values() {
    let driver = GtkDriver::new(form_groups::FormGroupsApp::new(), W, H);
    assert_locatable(
        &driver,
        &[
            ("draw_form", "Find"),
            ("draw_form", "Replace All"),
            ("draw_form", "Workspace"),
        ],
    );
}

#[test]
fn indicators_app_paints_locatable_spinner_and_progress_labels() {
    let driver = GtkDriver::new(indicators_app::IndicatorsApp::new(), W, H);
    assert_locatable(
        &driver,
        &[("draw_spinner", "Loading..."), ("draw_progress", "30%")],
    );
}

// ─── Overlays with no example-app call site (in-test fixtures) ─────────────
//
// `draw_tooltip` / `draw_command_center` / `draw_completions` /
// `draw_find_replace` aren't reached by *any* example app's `AppLogic`
// today — grepping `examples/common/*.rs` for `Tooltip`, `CommandCenter`,
// `Completions`, and `FindReplacePanel` turns up zero constructions, so
// there is no "through a real example app" fixture to route through yet.
// These four mirror `gtk::testing::tests::StatusBarApp`'s pattern instead:
// a minimal `AppLogic` that builds the primitive directly and hands it to
// the backend, sourced from the equivalent fixtures already committed for
// the macOS backend (`src/macos/{tooltip,command_center,completions,
// find_replace}.rs`).

struct TooltipFixtureApp;

impl AppLogic for TooltipFixtureApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let tooltip = Tooltip::new(WidgetId::new("tip"), "Hover hint")
            .with_placement(TooltipPlacement::Bottom);
        let layout = TooltipLayout {
            bounds: Rect::new(10.0, 10.0, 120.0, 24.0),
            resolved_placement: ResolvedPlacement::Bottom,
        };
        backend.draw_tooltip_with_chrome(
            &tooltip,
            &layout,
            &TooltipChrome::new(TooltipBorder::default()),
        );
    }

    fn handle(&mut self, _event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        Reaction::Continue
    }
}

#[test]
fn tooltip_fixture_paints_locatable_hover_text() {
    let driver = GtkDriver::new(TooltipFixtureApp, W, H);
    assert_locatable(&driver, &[("draw_tooltip", "Hover hint")]);
}

struct CommandCenterFixtureApp;

impl AppLogic for CommandCenterFixtureApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let cc = CommandCenter {
            id: WidgetId::new("cc"),
            back_enabled: true,
            forward_enabled: false,
            search_label: "my-project".into(),
        };
        backend.draw_command_center(Rect::new(0.0, 0.0, W as f32, 32.0), &cc);
    }

    fn handle(&mut self, _event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        Reaction::Continue
    }
}

#[test]
fn command_center_fixture_paints_locatable_search_label() {
    let driver = GtkDriver::new(CommandCenterFixtureApp, W, H);
    assert_locatable(&driver, &[("draw_command_center", "my-project")]);
}

struct CompletionsFixtureApp;

impl AppLogic for CompletionsFixtureApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let completions = Completions {
            id: WidgetId::new("comp"),
            items: vec![
                CompletionItem {
                    label: StyledText::plain("len"),
                    detail: None,
                    documentation: None,
                    kind: CompletionKind::Method,
                    icon: None,
                },
                CompletionItem {
                    label: StyledText::plain("clone"),
                    detail: None,
                    documentation: None,
                    kind: CompletionKind::Method,
                    icon: None,
                },
                CompletionItem {
                    label: StyledText::plain("map"),
                    detail: None,
                    documentation: None,
                    kind: CompletionKind::Method,
                    icon: None,
                },
            ],
            selected_idx: 1,
            scroll_offset: 0,
            has_focus: true,
        };
        let layout = completions.layout(
            20.0,
            20.0,
            16.0,
            Rect::new(0.0, 0.0, W as f32, H as f32),
            120.0,
            80.0,
            |_| CompletionItemMeasure::new(16.0),
        );
        backend.draw_completions(&completions, &layout);
    }

    fn handle(&mut self, _event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        Reaction::Continue
    }
}

#[test]
fn completions_fixture_paints_locatable_item_labels() {
    let driver = GtkDriver::new(CompletionsFixtureApp, W, H);
    assert_locatable(
        &driver,
        &[
            ("draw_completions", "len"),
            ("draw_completions", "clone"),
            ("draw_completions", "map"),
        ],
    );
}

struct FindReplaceFixtureApp;

impl AppLogic for FindReplaceFixtureApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let (hit_regions, _input_width) = compute_find_replace_hit_regions(50, false, "", 2, 2);
        let panel = FindReplacePanel {
            query: "needle".into(),
            replacement: String::new(),
            show_replace: false,
            focus: 0,
            cursor: 6,
            sel_anchor: None,
            match_info: "1 of 3".into(),
            case_sensitive: false,
            whole_word: false,
            use_regex: false,
            preserve_case: false,
            in_selection: false,
            group_bounds: Rect::new(0.0, 0.0, W as f32, H as f32),
            panel_width: 50,
            replace_one_glyph: "R1".into(),
            replace_all_glyph: "R*".into(),
            hit_regions,
        };
        backend.draw_find_replace(Rect::new(0.0, 0.0, W as f32, H as f32), &panel);
    }

    fn handle(&mut self, _event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        Reaction::Continue
    }
}

#[test]
fn find_replace_fixture_paints_locatable_query_and_match_info() {
    let driver = GtkDriver::new(FindReplaceFixtureApp, W, H);
    assert_locatable(
        &driver,
        &[
            ("draw_find_replace", "needle"),
            ("draw_find_replace", "1 of 3"),
        ],
    );
}
