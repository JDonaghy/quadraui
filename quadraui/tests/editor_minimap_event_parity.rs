#![cfg(feature = "tui")]
//! The `TuiDriver` leg of the `Editor` and `Minimap` event-parity checks.
//!
//! Each widget reaches the frame as a `Surface::Widget` filling the
//! viewport and registered in the app's `FrameTree`, so a gesture routes
//! through the shared widget-contract engine (hit test, click-to-focus,
//! `Interactive::handle`) and comes back out as the `UiEvent` this app
//! records. The binding-host leg (`quadraui-binding-core`'s
//! `InteractionEngine` over an `"editor"`/`"minimap"` layout leaf of the
//! same size) lives in that crate's engine tests. Neither widget exposes
//! accessibility actions, so there is no `a11y` leg. Both legs resolve the
//! same gesture to the same `UiEvent`:
//!
//! | widget    | gesture                               | event                                              |
//! |-----------|---------------------------------------|----------------------------------------------------|
//! | `Editor`  | press at text column 3 on view row 1  | `Editor(ed, Pointer { BufferPos { 11, 3 }, 3, Down(Left) })` |
//! | `Minimap` | press at the strip's vertical middle  | `Minimap(mm, Seek { fraction: 0.5 })`              |

use quadraui::interactive::PointerKind;
use quadraui::tui::testing::TuiDriver;
use quadraui::{
    AppLogic, Backend, Editor, EditorEvent, EditorHit, EditorLine, FrameTree, Minimap,
    MinimapEvent, MinimapLine, Modifiers, MouseButton, Reaction, Rect, ScreenLayout, Surface,
    UiEvent, WidgetId,
};

/// The editor viewport: twenty columns, six rows, a four-column gutter.
const EDITOR_COLS: u16 = 20;
const EDITOR_ROWS: u16 = 6;
/// The minimap viewport: four cells wide, eight rows of 32 buffer lines.
const MINIMAP_COLS: u16 = 4;
const MINIMAP_ROWS: u16 = 8;

fn editor_line(idx: usize, text: &str) -> EditorLine {
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

fn editor() -> Editor {
    let area = Rect::new(0.0, 0.0, EDITOR_COLS as f32, EDITOR_ROWS as f32);
    Editor::new(WidgetId::new("ed"), area)
        .with_lines(vec![
            editor_line(10, "fn main() {"),
            editor_line(11, "    hello();"),
            editor_line(12, "}"),
        ])
        .with_scroll_top(10)
        .with_total_lines(60)
        .with_gutter_char_width(4)
        .with_is_active(true)
}

fn minimap() -> Minimap {
    Minimap {
        id: WidgetId::new("mm"),
        lines: (0..32)
            .map(|i| MinimapLine {
                text: "fn main() {}".to_string(),
                line_idx: i,
            })
            .collect(),
        syntax_spans: vec![],
        visible_row_start: 0,
        visible_row_count: 8,
        total_buffer_lines: 32,
        scale: Default::default(),
        sizing: None,
    }
}

/// One widget drawn over the whole viewport and routed as a
/// `Surface::Widget`; `events` records every `UiEvent` the routing engine
/// hands the app.
struct WidgetApp<W> {
    id: WidgetId,
    rect: Rect,
    widget: W,
    events: Vec<UiEvent>,
}

impl<W> AppLogic for WidgetApp<W>
where
    W: quadraui::primitive::DynWidget + Clone + 'static,
{
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let mut frame = ScreenLayout::new();
        frame.push(Surface::Widget {
            rect: self.rect,
            widget: Box::new(self.widget.clone()),
        });
        frame.draw(backend);
    }

    fn frame_tree(&self, _area: ()) -> FrameTree {
        let mut tree = FrameTree::new();
        tree.push(self.id, self.rect, Box::new(self.widget.clone()));
        tree
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        self.events.push(event);
        Reaction::Continue
    }
}

fn driver<W>(id: &str, widget: W, cols: u16, rows: u16) -> TuiDriver<WidgetApp<W>>
where
    W: quadraui::primitive::DynWidget + Clone + 'static,
{
    TuiDriver::new(
        WidgetApp {
            id: WidgetId::new(id),
            rect: Rect::new(0.0, 0.0, cols as f32, rows as f32),
            widget,
            events: Vec::new(),
        },
        cols,
        rows,
    )
}

#[test]
fn a_press_on_editor_text_matches_the_parity_event() {
    let mut driver = driver("ed", editor(), EDITOR_COLS, EDITOR_ROWS);
    // The text area starts after the four-column gutter.
    driver.click(4.0 + 3.0, 1.0);
    let pointer: Vec<UiEvent> = driver
        .app()
        .events
        .iter()
        .filter(|e| matches!(e, UiEvent::Editor(..)))
        .cloned()
        .collect();
    assert_eq!(
        pointer,
        vec![UiEvent::Editor(
            WidgetId::new("ed"),
            EditorEvent::Pointer {
                hit: EditorHit::BufferPos { line: 11, col: 3 },
                col: 3,
                kind: PointerKind::Down(MouseButton::Left),
                mods: Modifiers::default(),
            }
        )]
    );
}

#[test]
fn a_press_at_the_minimap_middle_matches_the_parity_event() {
    let mut driver = driver("mm", minimap(), MINIMAP_COLS, MINIMAP_ROWS);
    driver.click(1.0, MINIMAP_ROWS as f32 / 2.0);
    let seeks: Vec<UiEvent> = driver
        .app()
        .events
        .iter()
        .filter(|e| matches!(e, UiEvent::Minimap(..)))
        .cloned()
        .collect();
    assert_eq!(
        seeks,
        vec![UiEvent::Minimap(
            WidgetId::new("mm"),
            MinimapEvent::Seek { fraction: 0.5 }
        )]
    );
}
