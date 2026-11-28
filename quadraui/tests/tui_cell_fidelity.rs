#![cfg(feature = "tui")]
//! Black-box `TuiDriver` tests for the cell-grid look of `TabBar`,
//! `StatusBar`, `Form`, `TreeView` and `ListView` painted through the
//! widget contract (`Surface::Widget` → `DynWidget::paint` →
//! `Primitive::paint_cells`).
//!
//! Each test pins one rendering a terminal host relies on: the active tab
//! stays on screen without a host-side scroll probe, a tab strip can drop
//! its `│` rules, a status bar never truncates text that fits, a
//! content-sized form control can sit against the row's right edge, a
//! tree can give its last column to rows instead of a scrollbar, and a
//! list row too long for the list can be cut at the edge instead of
//! ending in `…`.

use quadraui::primitive::DynWidget;
use quadraui::tui::testing::TuiDriver;
use quadraui::{
    AppLogic, Backend, CellOverflow, Color, ControlWidth, Decoration, FieldKind, Form, FormField,
    ListItem, ListView, Reaction, Rect, ScreenLayout, SelectionMode, StatusBar, StatusBarSegment,
    Style, StyledText, Surface, TabBar, TabChrome, TabItem, TabSeparator, TreeRow, TreeStyle,
    TreeView, UiEvent, WidgetId,
};

/// Paints one widget into `rect` every frame.
struct OneWidget {
    rect: Rect,
    make: Box<dyn Fn() -> Box<dyn DynWidget>>,
}

impl AppLogic for OneWidget {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let mut frame = ScreenLayout::new();
        frame.push(Surface::Widget {
            rect: self.rect,
            widget: (self.make)(),
        });
        frame.draw(backend);
    }

    fn handle(&mut self, _event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        Reaction::Continue
    }
}

fn driver<W>(cols: u16, rows: u16, rect: Rect, widget: W) -> TuiDriver<OneWidget>
where
    W: DynWidget + Clone + 'static,
{
    let app = OneWidget {
        rect,
        make: Box::new(move || Box::new(widget.clone())),
    };
    TuiDriver::new(app, cols, rows)
}

/// Row `y` of the screen as plain text.
fn row(driver: &TuiDriver<OneWidget>, y: usize) -> String {
    driver
        .screen()
        .lines()
        .nth(y)
        .unwrap_or_default()
        .to_string()
}

// ── TabBar ──────────────────────────────────────────────────────────────

fn tab_bar(labels: &[&str], active: usize, scroll_offset: usize, chrome: TabChrome) -> TabBar {
    TabBar {
        id: WidgetId::new("tabs"),
        tabs: labels
            .iter()
            .enumerate()
            .map(|(i, label)| TabItem {
                label: (*label).to_string(),
                is_active: i == active,
                is_closable: false,
                ..TabItem::default()
            })
            .collect(),
        scroll_offset,
        right_segments: Vec::new(),
        active_accent: None,
        show_tab_close: false,
        compact: true,
        chrome,
        icons: Vec::new(),
        glyphs: Default::default(),
    }
}

#[test]
fn overflowing_tab_bar_paints_its_last_active_tab() {
    let labels = [" alpha ", " bravo ", " charlie ", " delta ", " echo "];
    let bar = tab_bar(&labels, labels.len() - 1, 0, TabChrome::default());
    let d = driver(20, 2, Rect::new(0.0, 0.0, 20.0, 1.0), bar);

    let strip = row(&d, 0);
    assert!(
        strip.contains("echo"),
        "the active last tab must be on screen: {strip:?}"
    );
    assert!(
        !strip.contains("alpha"),
        "the strip must have scrolled past the first tab: {strip:?}"
    );
}

#[test]
fn tab_bar_keeps_a_scroll_offset_that_already_shows_the_active_tab() {
    let labels = [" a ", " b ", " c ", " d ", " e ", " f "];
    // Offset 1 shows tabs b..f in 16 columns (5 * 4 = 20 > 16, so not all),
    // and the active tab c is among them.
    let bar = tab_bar(&labels, 2, 1, TabChrome::default());
    let d = driver(16, 2, Rect::new(0.0, 0.0, 16.0, 1.0), bar);

    let strip = row(&d, 0);
    assert!(
        strip.starts_with(" b "),
        "a stored offset that shows the active tab is honoured: {strip:?}"
    );
}

#[test]
fn tab_separator_none_paints_space_separated_tabs_with_no_rule() {
    let labels = [" A ", " B ", " C "];
    let chrome = TabChrome::default().with_separator(TabSeparator::None);
    let bar = tab_bar(&labels, 0, 0, chrome);
    let d = driver(20, 2, Rect::new(0.0, 0.0, 20.0, 1.0), bar);

    let strip = row(&d, 0);
    assert!(strip.starts_with(" A  B  C "), "{strip:?}");
    assert!(!strip.contains('│'), "no separator rule: {strip:?}");
}

#[test]
fn tab_separator_defaults_to_a_rule_after_each_tab() {
    let bar = tab_bar(&[" A ", " B "], 0, 0, TabChrome::default());
    let d = driver(20, 2, Rect::new(0.0, 0.0, 20.0, 1.0), bar);

    let strip = row(&d, 0);
    assert!(strip.starts_with(" A │ B │"), "{strip:?}");
}

// ── StatusBar ───────────────────────────────────────────────────────────

fn segment(text: &str) -> StatusBarSegment {
    StatusBarSegment {
        text: text.to_string(),
        fg: Color::rgb(230, 230, 230),
        bg: Color::rgb(40, 80, 120),
        bold: false,
        action_id: None,
    }
}

#[test]
fn status_bar_left_text_that_fits_beside_the_right_group_paints_in_full() {
    // 27 + 12 = 39 cells in a 40-cell bar: both groups fit side by side,
    // only the 2-cell preferred gap does not.
    let left = "⚠ coordinator.yml not found";
    let right = " 12:34 ok   ";
    assert_eq!(left.chars().count() + right.chars().count(), 39);
    let bar = StatusBar {
        id: WidgetId::new("status"),
        left_segments: vec![segment(left)],
        right_segments: vec![segment(right)],
    };
    let d = driver(40, 2, Rect::new(0.0, 0.0, 40.0, 1.0), bar);

    let line = row(&d, 0);
    assert!(
        line.contains("⚠ coordinator.yml not found"),
        "left text must not be ellipsis-truncated: {line:?}"
    );
    assert!(!line.contains('…'), "{line:?}");
    assert!(line.contains("12:34 ok"), "{line:?}");
}

#[test]
fn status_bar_left_text_that_cannot_fit_still_ends_in_an_ellipsis() {
    let left = "a status message far too long for this bar";
    let bar = StatusBar {
        id: WidgetId::new("status"),
        left_segments: vec![segment(left)],
        right_segments: vec![segment(" 1:1 ")],
    };
    let d = driver(30, 2, Rect::new(0.0, 0.0, 30.0, 1.0), bar);

    let line = row(&d, 0);
    assert!(line.contains('…'), "{line:?}");
    assert!(line.contains("1:1"), "{line:?}");
}

/// Paints one status bar every frame under a host-set `Style`.
struct StyledBar {
    bar: StatusBar,
    style: Style,
}

impl AppLogic for StyledBar {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        backend.set_style(self.style);
        let mut frame = ScreenLayout::new();
        frame.push(Surface::Widget {
            rect: Rect::new(0.0, 0.0, 30.0, 1.0),
            widget: Box::new(self.bar.clone()),
        });
        frame.draw(backend);
    }

    fn handle(&mut self, _event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        Reaction::Continue
    }
}

fn overflowing_bar_row(style: Style) -> String {
    let bar = StatusBar {
        id: WidgetId::new("status"),
        left_segments: vec![segment("a status message far too long for this bar")],
        right_segments: vec![segment(" 1:1 ")],
    };
    let d = TuiDriver::new(StyledBar { bar, style }, 30, 2);
    d.screen().lines().next().unwrap_or_default().to_string()
}

#[test]
fn status_bar_clip_with_no_gap_cuts_left_text_flush_against_the_right_group() {
    let style = Style::default()
        .with_status_bar_cell_gap(0.0)
        .with_status_bar_cell_overflow(CellOverflow::Clip);

    assert_eq!(overflowing_bar_row(style), "a status message far too  1:1 ");
}

#[test]
fn status_bar_gap_sets_where_an_ellipsised_left_group_stops() {
    let no_gap = overflowing_bar_row(Style::default().with_status_bar_cell_gap(0.0));
    assert_eq!(no_gap, "a status message far too… 1:1 ");

    let default_gap = overflowing_bar_row(Style::default());
    assert_eq!(default_gap, "a status message far t…   1:1 ");
}

// ── Form ────────────────────────────────────────────────────────────────

fn plain(text: &str) -> StyledText {
    StyledText::plain(text)
}

fn field(id: &str, label: &str, kind: FieldKind) -> FormField {
    FormField {
        id: WidgetId::new(id),
        label: plain(label),
        kind,
        hint: plain(""),
        disabled: false,
        validation: None,
        label_position: None,
        label_align: None,
        label_width: None,
        tab_index: None,
        control_width: None,
        min_control_width: None,
        row_align: None,
    }
}

fn form(fields: Vec<FormField>, control_width: ControlWidth) -> Form {
    Form {
        id: WidgetId::new("settings"),
        fields,
        focused_field: None,
        scroll_offset: 0,
        has_focus: false,
        label_position: Default::default(),
        label_align: Default::default(),
        label_width: None,
        control_width,
        min_control_width: None,
        spacing: Default::default(),
    }
}

const FORM_COLS: usize = 40;

fn form_driver(control_width: ControlWidth) -> TuiDriver<OneWidget> {
    let f = form(
        vec![
            field("wrap", "Wrap", FieldKind::Toggle { value: true }),
            field(
                "name",
                "Name",
                FieldKind::TextInput {
                    value: "coord".into(),
                    placeholder: String::new(),
                    cursor: None,
                    selection_anchor: None,
                },
            ),
            field(
                "ver",
                "Version",
                FieldKind::ReadOnly {
                    value: plain("0.12"),
                },
            ),
            field("go", "Apply", FieldKind::Button),
        ],
        control_width,
    );
    driver(
        FORM_COLS as u16,
        5,
        Rect::new(0.0, 0.0, FORM_COLS as f32, 4.0),
        f,
    )
}

/// Column just past the last non-blank character of row `y`.
fn painted_end(driver: &TuiDriver<OneWidget>, y: usize) -> usize {
    row(driver, y).trim_end().chars().count()
}

#[test]
fn content_end_form_controls_end_two_cells_before_the_row_edge() {
    let d = form_driver(ControlWidth::ContentEnd);

    assert!(row(&d, 0).trim_end().ends_with("[x]"), "{:?}", row(&d, 0));
    assert_eq!(
        painted_end(&d, 0),
        FORM_COLS - 2,
        "toggle: {:?}",
        row(&d, 0)
    );

    assert!(
        row(&d, 1).trim_end().ends_with("[coord]"),
        "{:?}",
        row(&d, 1)
    );
    assert_eq!(
        painted_end(&d, 1),
        FORM_COLS - 1,
        "text box: its `]` sits in the first of the two margin cells: {:?}",
        row(&d, 1)
    );

    assert!(row(&d, 2).trim_end().ends_with("0.12"), "{:?}", row(&d, 2));
    assert_eq!(
        painted_end(&d, 2),
        FORM_COLS - 2,
        "read-only: {:?}",
        row(&d, 2)
    );

    assert!(
        row(&d, 3).trim_end().ends_with("< Apply >"),
        "{:?}",
        row(&d, 3)
    );
    assert_eq!(
        painted_end(&d, 3),
        FORM_COLS - 1,
        "button: {:?}",
        row(&d, 3)
    );
}

#[test]
fn content_form_controls_still_start_after_the_label_column() {
    let d = form_driver(ControlWidth::Content);

    // Labels "Wrap"/"Name"/"Version"/"Apply" share a 7-cell column that
    // starts at column 1, so controls start at column 9.
    assert_eq!(row(&d, 0).find("[x]"), Some(9), "{:?}", row(&d, 0));
    assert_eq!(row(&d, 1).find("[coord]"), Some(9), "{:?}", row(&d, 1));
}

// ── TreeView ────────────────────────────────────────────────────────────

fn tree(show_scrollbar: bool) -> TreeView {
    TreeView {
        id: WidgetId::new("tree"),
        rows: (0..10)
            .map(|i| TreeRow {
                path: vec![i],
                indent: 0,
                icon: None,
                text: plain(&format!("row {i} abcdefghij")),
                badge: None,
                is_expanded: None,
                decoration: Decoration::Normal,
                edit: None,
            })
            .collect(),
        selection_mode: SelectionMode::Single,
        selected_path: None,
        scroll_offset: 0,
        items_window: None,
        scroll_px: None,
        style: TreeStyle {
            show_scrollbar,
            ..TreeStyle::default()
        },
        has_focus: false,
    }
}

#[test]
fn tree_without_a_scrollbar_paints_row_text_in_its_last_column() {
    // 3 visible rows of 10 overflow. Each row paints a 2-cell gutter plus
    // 16 cells of text, exactly the tree's 18-cell width, so the text
    // reaches the last column only when no scrollbar takes it.
    let width = 18u16;
    let d = driver(
        width,
        4,
        Rect::new(0.0, 0.0, width as f32, 3.0),
        tree(false),
    );

    let first = row(&d, 0);
    assert!(
        first.trim_end().ends_with('j'),
        "row text must reach the last column: {first:?}"
    );
    assert_eq!(
        first.trim_end().chars().count(),
        width as usize,
        "{first:?}"
    );
}

#[test]
fn tree_with_a_scrollbar_keeps_its_last_column_for_the_track() {
    let width = 18u16;
    let d = driver(width, 4, Rect::new(0.0, 0.0, width as f32, 3.0), tree(true));

    let first = row(&d, 0);
    assert!(
        !first.contains("abcdefghij"),
        "the scrollbar column truncates the row: {first:?}"
    );
}

// ── ListView ────────────────────────────────────────────────────────────

/// Paints one `ListView` under `style` every frame.
struct StyledList {
    list: ListView,
    style: Style,
}

impl AppLogic for StyledList {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        backend.set_style(self.style);
        let mut frame = ScreenLayout::new();
        frame.push(Surface::Widget {
            rect: Rect::new(0.0, 0.0, 12.0, 2.0),
            widget: Box::new(self.list.clone()),
        });
        frame.draw(backend);
    }

    fn handle(&mut self, _event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        Reaction::Continue
    }
}

/// The first row of a 12-cell list whose only row is far wider than it.
fn overlong_list_row(style: Style) -> String {
    let list = ListView {
        id: WidgetId::new("list"),
        title: None,
        items: vec![ListItem {
            text: plain("abcdefghijklmnopqrstuvwxyz"),
            icon: None,
            detail: None,
            decoration: Decoration::default(),
        }],
        selected_idx: 0,
        scroll_offset: 0,
        items_window: None,
        scroll_px: None,
        has_focus: false,
        bordered: false,
        h_scroll: 0,
        max_content_width: None,
        show_v_scrollbar: false,
        striped: false,
    };
    let d = TuiDriver::new(StyledList { list, style }, 12, 2);
    d.screen().lines().next().unwrap_or_default().to_string()
}

#[test]
fn list_row_under_clip_is_cut_at_the_last_cell_with_no_ellipsis() {
    let clipped = overlong_list_row(Style::default().with_list_cell_overflow(CellOverflow::Clip));
    let ellipsised = overlong_list_row(Style::default());

    assert_eq!(clipped.chars().count(), 12, "{clipped:?}");
    assert!(!clipped.contains('…'), "{clipped:?}");
    assert!(
        clipped.trim_start().starts_with("abc"),
        "the row text starts the row: {clipped:?}"
    );
    let last = clipped.chars().last().unwrap();
    assert!(
        last.is_ascii_lowercase(),
        "the last cell holds the text that reached it: {clipped:?}"
    );

    // Ellipsis stays the default: the same row, its last cell an `…`.
    assert!(ellipsised.ends_with('…'), "{ellipsised:?}");
    let keep = clipped.chars().count() - 1;
    assert_eq!(
        ellipsised.chars().take(keep).collect::<String>(),
        clipped.chars().take(keep).collect::<String>()
    );
}
