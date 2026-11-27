#![cfg(feature = "tui")]
//! Black-box `TuiDriver` test for `BoardModel` on the widget contract:
//! the board reaches the frame as a type-erased `Surface::Widget`, paints
//! its columns and cards through `Primitive::paint_cells`, and keys and
//! clicks route through `Interactive::handle` back out as the
//! `UiEvent::Board` actions this app records — the host answering each
//! by writing `selected_card_id` back (controlled mode).

use quadraui::tui::testing::TuiDriver;
use quadraui::{
    AppLogic, BadgeStatus, Backend, BoardAction, BoardCard, BoardColumn, BoardModel, CardBadge,
    FrameTree, MoveDir, Reaction, Rect, ScreenLayout, Surface, UiEvent, WidgetId,
};

const COLS: u16 = 60;
const ROWS: u16 = 16;
const AREA: Rect = Rect::new(1.0, 1.0, 50.0, 13.0);

fn board_id() -> WidgetId {
    WidgetId::new("board:issues")
}

fn card(id: &str, title: &str) -> BoardCard {
    BoardCard {
        id: WidgetId::new(id),
        title: title.to_string(),
        labels: vec![],
        badges: vec![CardBadge {
            label: "T".into(),
            status: BadgeStatus::Passed,
        }],
        hint: None,
    }
}

fn model() -> BoardModel {
    BoardModel {
        id: board_id(),
        columns: vec![
            BoardColumn {
                id: WidgetId::new("col:todo"),
                title: "Todo".to_string(),
                cards: vec![card("card:1", "First card"), card("card:2", "Second card")],
                scroll_offset: 0,
            },
            BoardColumn {
                id: WidgetId::new("col:done"),
                title: "Done".to_string(),
                cards: vec![card("card:3", "Third card")],
                scroll_offset: 0,
            },
        ],
        selected_card_id: None,
        col_scroll_offset: 0,
    }
}

struct BoardApp {
    board: BoardModel,
    actions: Vec<BoardAction>,
}

impl AppLogic for BoardApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let mut frame = ScreenLayout::new();
        frame.push(Surface::Widget {
            rect: AREA,
            widget: Box::new(self.board.clone()),
        });
        frame.draw(backend);
    }

    fn frame_tree(&self, _area: ()) -> FrameTree {
        let mut tree = FrameTree::new();
        tree.push(self.board.id, AREA, Box::new(self.board.clone()));
        tree
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        if let UiEvent::Board(id, action) = event {
            if id == board_id() {
                match &action {
                    BoardAction::SelectCard(card) => self.board.selected_card_id = Some(*card),
                    BoardAction::MoveSelection(dir) => self.board.move_selection(*dir),
                    _ => {}
                }
                self.actions.push(action);
                return Reaction::Redraw;
            }
        }
        Reaction::Continue
    }
}

fn driver() -> TuiDriver<BoardApp> {
    TuiDriver::new(
        BoardApp {
            board: model(),
            actions: Vec::new(),
        },
        COLS,
        ROWS,
    )
}

#[test]
fn surface_widget_board_paints_column_titles_and_cards() {
    let driver = driver();
    for needle in ["Todo", "Done", "First card", "Second card", "Third card"] {
        assert!(driver.screen_contains(needle), "{needle}:\n{}", driver.screen());
    }
}

#[test]
fn clicking_a_card_selects_it() {
    let mut driver = driver();
    let (x, y) = driver.find("Third card").expect("card painted");
    driver.click(x, y);
    assert_eq!(
        driver.app().actions,
        vec![BoardAction::SelectCard(WidgetId::new("card:3"))]
    );
}

#[test]
fn keys_route_through_the_focused_board() {
    let mut driver = driver();
    let (x, y) = driver.find("First card").expect("card painted");
    driver.click(x, y);
    driver.type_char('j');
    driver.press_named(quadraui::NamedKey::Enter);
    driver.type_char('G');
    assert_eq!(
        driver.app().actions,
        vec![
            BoardAction::SelectCard(WidgetId::new("card:1")),
            BoardAction::MoveSelection(MoveDir::Down),
            BoardAction::OpenIssue(WidgetId::new("card:2")),
            BoardAction::JumpToBottom,
        ]
    );
}

#[test]
fn double_click_opens_and_right_click_asks_for_a_context_menu() {
    let mut driver = driver();
    let (x, y) = driver.find("Second card").expect("card painted");
    driver.double_click(x, y);
    driver.right_click(x, y);
    let actions = &driver.app().actions;
    assert!(
        actions.contains(&BoardAction::OpenIssue(WidgetId::new("card:2"))),
        "{actions:?}"
    );
    assert!(
        actions.iter().any(|a| matches!(
            a,
            BoardAction::ContextMenu(id, at) if *id == WidgetId::new("card:2") && at.x == x && at.y == y
        )),
        "{actions:?}"
    );
}
