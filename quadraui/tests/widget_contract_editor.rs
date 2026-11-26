#![cfg(feature = "tui")]
//! Black-box `TuiDriver` test for `Editor` on the widget contract: the
//! editor reaches the frame as a type-erased `Surface::Widget`, is painted
//! through `DynWidget::paint` → `Primitive::paint_cells`, and is
//! registered in the app's `FrameTree` so every gesture routes through the
//! shared widget-contract engine — hit test, click-to-focus,
//! `Interactive::handle` — and comes back out as the
//! `UiEvent::Editor(_, EditorEvent::..)` this app records.

use quadraui::interactive::PointerKind;
use quadraui::tui::testing::TuiDriver;
use quadraui::{
    AppLogic, Backend, Editor, EditorCursor, EditorCursorPos, EditorCursorShape, EditorEvent,
    EditorHit, EditorLine, FrameTree, Key, MouseButton, Point, Reaction, Rect, ScreenLayout,
    ScrollDelta, Surface, UiEvent, WidgetId,
};

const COLS: u16 = 30;
const ROWS: u16 = 10;
/// A twenty-column, six-row editor with a four-column gutter, so its text
/// starts at column 6.
const AREA: Rect = Rect::new(2.0, 1.0, 20.0, 6.0);
const TEXT_X: f32 = AREA.x + 4.0;
/// The first visible buffer line.
const SCROLL_TOP: usize = 10;

fn editor_id() -> WidgetId {
    WidgetId::new("editor:main")
}

fn line(idx: usize, text: &str) -> EditorLine {
    EditorLine {
        raw_text: text.to_string(),
        gutter_text: format!("{:>3} ", idx + 1),
        spans: vec![],
        line_idx: idx,
        is_current_line: false,
        is_fold_header: false,
        folded_line_count: 0,
        git_diff: None,
        diff_status: None,
        diagnostics: vec![],
        spell_errors: vec![],
        is_breakpoint: false,
        is_conditional_bp: false,
        is_dap_current: false,
        is_wrap_continuation: false,
        segment_col_offset: 0,
        annotation: None,
        ghost_suffix: None,
        is_ghost_continuation: false,
        indent_guides: vec![],
        colorcolumns: vec![],
    }
}

fn editor(shape: EditorCursorShape) -> Editor {
    Editor::new(editor_id(), AREA)
        .with_lines(vec![
            line(SCROLL_TOP, "fn main() {"),
            line(SCROLL_TOP + 1, "    hello();"),
            line(SCROLL_TOP + 2, "}"),
        ])
        .with_scroll_top(SCROLL_TOP)
        .with_total_lines(60)
        .with_gutter_char_width(4)
        .with_is_active(true)
        .with_cursor(EditorCursor {
            pos: EditorCursorPos {
                view_line: 1,
                col: 4,
            },
            shape,
        })
}

/// One editor drawn and routed as a `Surface::Widget`. `events` records
/// every `UiEvent::Editor`; `keys` every key that bubbled to the app.
struct EditorApp {
    editor: Editor,
    events: Vec<EditorEvent>,
    keys: Vec<Key>,
}

impl EditorApp {
    fn new(shape: EditorCursorShape) -> Self {
        Self {
            editor: editor(shape),
            events: Vec::new(),
            keys: Vec::new(),
        }
    }

    /// The `(hit, col, kind)` of every recorded pointer event.
    fn pointer_events(&self) -> Vec<(EditorHit, usize, PointerKind)> {
        self.events
            .iter()
            .filter_map(|e| match e {
                EditorEvent::Pointer { hit, col, kind, .. } => Some((*hit, *col, *kind)),
                _ => None,
            })
            .collect()
    }
}

impl AppLogic for EditorApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let mut frame = ScreenLayout::new();
        frame.push(Surface::Widget {
            rect: AREA,
            widget: Box::new(self.editor.clone()),
        });
        frame.draw(backend);
    }

    fn frame_tree(&self, _area: ()) -> FrameTree {
        let mut tree = FrameTree::new();
        tree.push(self.editor.id, AREA, Box::new(self.editor.clone()));
        tree
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        match event {
            UiEvent::Editor(id, ev) if id == editor_id() => self.events.push(ev),
            UiEvent::KeyPressed { key, .. } => self.keys.push(key),
            _ => {}
        }
        Reaction::Continue
    }
}

fn row_text(driver: &TuiDriver<EditorApp>, y: u16) -> String {
    driver.styled_row(y).into_iter().map(|(ch, _)| ch).collect()
}

fn driver(shape: EditorCursorShape) -> TuiDriver<EditorApp> {
    let mut driver = TuiDriver::new(EditorApp::new(shape), COLS, ROWS);
    driver.set_double_click_folding(false);
    driver
}

#[test]
fn surface_widget_editor_paints_gutter_text_and_scrollbar() {
    let driver = driver(EditorCursorShape::Bar);
    let row = row_text(&driver, AREA.y as u16);
    assert_eq!(&row[2..17], " 11 fn main() {", "{}", driver.screen());
    assert!(
        row_text(&driver, AREA.y as u16 + 1).contains(" 12     hello();"),
        "{}",
        driver.screen()
    );

    // Sixty lines in a six-row viewport: the right column is a scrollbar.
    let sb_x = (AREA.x + AREA.width - 1.0) as usize;
    let column: Vec<char> = (1..7)
        .map(|y| row_text(&driver, y).chars().nth(sb_x).unwrap())
        .collect();
    assert!(column.iter().all(|c| *c == '█' || *c == '░'), "{column:?}");
}

#[test]
fn a_bar_caret_places_the_terminal_cursor() {
    let mut driver = driver(EditorCursorShape::Bar);
    assert_eq!(
        driver.terminal_cursor_position(),
        Some((TEXT_X as u16 + 4, AREA.y as u16 + 1))
    );
}

#[test]
fn a_block_caret_inverts_its_cell() {
    let driver = driver(EditorCursorShape::Block);
    let theme = quadraui::Theme::default();
    let style = driver
        .style_at(TEXT_X as u16 + 4, AREA.y as u16 + 1)
        .expect("caret cell");
    assert_eq!(
        style.bg,
        quadraui::tui::ratatui_color(theme.cursor),
        "the block caret's cell takes the cursor colour"
    );
}

#[test]
fn clicking_text_focuses_the_editor_and_reports_the_buffer_position() {
    let mut driver = driver(EditorCursorShape::Bar);
    // `click` delivers the press alone.
    driver.click(TEXT_X + 3.0, AREA.y + 1.0);

    assert_eq!(
        driver.backend().focus_tree().focused().map(|(id, _)| *id),
        Some(editor_id())
    );
    let hit = EditorHit::BufferPos {
        line: SCROLL_TOP + 1,
        col: 3,
    };
    assert_eq!(
        driver.app().pointer_events(),
        vec![(hit, 3, PointerKind::Down(MouseButton::Left))]
    );

    driver.type_char('x');
    assert_eq!(driver.app().keys, vec![Key::Char('x')]);
}

#[test]
fn clicking_the_gutter_reports_a_gutter_hit() {
    let mut driver = driver(EditorCursorShape::Bar);
    driver.click(AREA.x + 1.0, AREA.y + 2.0);
    let first = driver.app().pointer_events()[0];
    assert_eq!(
        first.0,
        EditorHit::Gutter {
            line_idx: SCROLL_TOP + 2,
            gutter_col: 1
        }
    );
}

#[test]
fn a_drag_reports_moves_clamped_onto_the_text_area() {
    let mut driver = driver(EditorCursorShape::Bar);
    driver.mouse_down(TEXT_X + 1.0, AREA.y);
    driver.mouse_move(TEXT_X + 5.0, AREA.y + 1.0);
    // Past the bottom edge: clamped onto the last text row.
    driver.mouse_move(TEXT_X + 2.0, AREA.y + 20.0);
    driver.mouse_up(TEXT_X + 2.0, AREA.y + 20.0);

    let last_row = SCROLL_TOP + AREA.height as usize - 1;
    assert_eq!(
        driver.app().pointer_events(),
        vec![
            (
                EditorHit::BufferPos {
                    line: SCROLL_TOP,
                    col: 1
                },
                1,
                PointerKind::Down(MouseButton::Left)
            ),
            (
                EditorHit::BufferPos {
                    line: SCROLL_TOP + 1,
                    col: 5
                },
                5,
                PointerKind::Move
            ),
            (
                EditorHit::BufferPos {
                    line: last_row,
                    col: 2
                },
                2,
                PointerKind::Move
            ),
            (
                EditorHit::BufferPos {
                    line: last_row,
                    col: 2
                },
                2,
                PointerKind::Up(MouseButton::Left)
            ),
        ]
    );
}

#[test]
fn a_move_without_a_drag_reports_nothing() {
    let mut driver = driver(EditorCursorShape::Bar);
    driver.mouse_move(TEXT_X + 2.0, AREA.y + 1.0);
    assert!(driver.app().events.is_empty(), "{:?}", driver.app().events);
}

#[test]
fn a_wheel_notch_over_the_editor_reports_a_wheel_event() {
    let mut driver = driver(EditorCursorShape::Bar);
    driver.dispatch(UiEvent::Scroll {
        widget: None,
        delta: ScrollDelta::new(0.0, 1.0),
        position: Point::new(TEXT_X + 2.0, AREA.y + 2.0),
    });
    assert!(
        matches!(
            driver.app().events.as_slice(),
            [EditorEvent::Wheel { dy, .. }] if *dy > 0.0
        ),
        "{:?}",
        driver.app().events
    );
}
