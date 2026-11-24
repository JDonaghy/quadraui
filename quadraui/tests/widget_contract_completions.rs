#![cfg(feature = "tui")]
//! Black-box `TuiDriver` test for `Completions` on the widget contract:
//! the popup reaches the frame as a type-erased `Surface::Widget`, is
//! painted through `DynWidget::paint` → `Primitive::paint_cells`, and is
//! registered in the app's `FrameTree` so input is routed through the
//! shared widget-contract engine — `UiEvent` into
//! `quadraui::runtime::preprocess_event`'s widget routing (hit test,
//! click-to-focus, `Interactive::handle`) and back out as the `UiEvent`
//! this test's own `AppLogic::handle` applies onto the owned
//! `Completions`, which the next `render` paints.
//!
//! Never a direct call into `Completions::handle`/`Primitive::layout`
//! that would skip the routing engine. The one arm the engine cannot
//! deliver is the wheel: `route_widget_event`'s `UiEvent` match has no
//! `Scroll` arm, so no `RoutedEvent::Wheel` reaches a widget through it —
//! `Completions` has no wheel arm of its own either, and keyboard paging
//! is covered below.

use quadraui::tui::ratatui_color;
use quadraui::tui::testing::TuiDriver;
use quadraui::{
    AppLogic, Backend, CompletionItem, CompletionKind, Completions, FrameTree, ListViewEvent,
    NamedKey, Reaction, Rect, ScreenLayout, StyledText, Surface, Theme, UiEvent, WidgetId,
};

const COLS: u16 = 40;
const ROWS: u16 = 14;
/// Four rows tall, so a 6-candidate popup overflows it and pages.
const POPUP: Rect = Rect::new(2.0, 2.0, 24.0, 4.0);

const CANDIDATES: [&str; 6] = ["alpha", "beta", "gamma", "delta", "epsilon", "zeta"];

/// One completions popup, drawn and routed as a `Surface::Widget`, plus a
/// record of what the host was told to do with it.
struct CompletionsApp {
    popup: Completions,
    accepted: Vec<usize>,
    dismissed: bool,
}

impl CompletionsApp {
    fn new() -> Self {
        let popup = Completions {
            id: WidgetId::new("completions:editor"),
            items: CANDIDATES
                .iter()
                .map(|label| CompletionItem {
                    label: StyledText::plain(*label),
                    detail: None,
                    documentation: None,
                    kind: CompletionKind::Function,
                    icon: None,
                })
                .collect(),
            has_focus: true,
            ..Completions::default()
        };
        Self {
            popup,
            accepted: Vec::new(),
            dismissed: false,
        }
    }
}

impl AppLogic for CompletionsApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        if self.dismissed {
            return;
        }
        let mut frame = ScreenLayout::new();
        frame.push(Surface::Widget {
            rect: POPUP,
            widget: Box::new(self.popup.clone()),
        });
        frame.draw(backend);
    }

    fn frame_tree(&self, _area: ()) -> FrameTree {
        let mut tree = FrameTree::new();
        if !self.dismissed {
            tree.push(self.popup.id, POPUP, Box::new(self.popup.clone()));
        }
        tree
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        match event {
            UiEvent::CompletionItemActivated(id, idx) if id == self.popup.id => {
                self.accepted.push(idx);
                self.dismissed = true;
                Reaction::Redraw
            }
            UiEvent::List(id, ListViewEvent::SelectionChanged { idx }) if id == self.popup.id => {
                self.popup.selected_idx = idx;
                Reaction::Redraw
            }
            UiEvent::List(id, ListViewEvent::KeyPressed { .. }) if id == self.popup.id => {
                self.dismissed = true;
                Reaction::Redraw
            }
            _ => Reaction::Continue,
        }
    }
}

fn driver() -> TuiDriver<CompletionsApp> {
    TuiDriver::new(CompletionsApp::new(), COLS, ROWS)
}

/// Click the popup once so the widget-contract click-to-focus step runs —
/// `Completions::focus()` is `FocusSpec::Stop`, so nothing routes a key
/// to it until then. Clicking row 0 is a no-op for the selection, which
/// already sits there.
fn focus(driver: &mut TuiDriver<CompletionsApp>) {
    let (x, y) = driver.find("alpha").expect("row 0 visible");
    driver.click(x, y);
    assert_eq!(driver.app().popup.selected_idx, 0);
}

#[test]
fn a_surface_widget_popup_paints_the_rows_that_fit_its_box() {
    let driver = driver();
    for label in ["alpha", "beta", "gamma", "delta"] {
        assert!(
            driver.screen_contains(label),
            "{label} missing:\n{}",
            driver.screen()
        );
    }
    // The box only holds four rows, so the tail is scrolled out.
    assert!(!driver.screen_contains("epsilon"), "{}", driver.screen());

    // Side borders frame every row, one column either side of the box.
    let first = driver.find_bounds("alpha").expect("row 0 visible");
    let row: Vec<char> = driver
        .screen()
        .lines()
        .nth(first.y as usize)
        .map(|l| l.chars().collect())
        .unwrap_or_default();
    assert_eq!(row[POPUP.x as usize], '│', "{}", driver.screen());
    assert_eq!(
        row[(POPUP.x + POPUP.width) as usize - 1],
        '│',
        "{}",
        driver.screen()
    );
}

#[test]
fn the_kind_icon_paints_ahead_of_every_label() {
    let driver = driver();
    let icon = CompletionKind::Function.default_icon().fallback;
    let label = driver.find_bounds("alpha").expect("row 0 visible");
    let row: Vec<char> = driver
        .screen()
        .lines()
        .nth(label.y as usize)
        .map(|l| l.chars().collect())
        .unwrap_or_default();
    let painted: String = (0..icon.chars().count())
        .map(|i| row[POPUP.x as usize + 1 + i])
        .collect();
    assert_eq!(painted, icon, "{}", driver.screen());
}

#[test]
fn the_selected_row_is_tinted_through_the_painted_widget() {
    let mut driver = driver();
    focus(&mut driver);
    driver.press_named(NamedKey::Down);

    let selected = driver.find_bounds("beta").expect("row 1 visible");
    let style = driver
        .style_at(selected.x as u16, selected.y as u16)
        .expect("cell inside the popup");
    assert_eq!(
        style.bg,
        ratatui_color(Theme::default().completion_selected_bg)
    );

    let other = driver.find_bounds("gamma").expect("row 2 visible");
    let plain = driver
        .style_at(other.x as u16, other.y as u16)
        .expect("cell inside the popup");
    assert_eq!(plain.bg, ratatui_color(Theme::default().completion_bg));
}

#[test]
fn arrow_keys_move_the_highlight_through_the_real_routing_engine() {
    let mut driver = driver();
    focus(&mut driver);

    driver.press_named(NamedKey::Down);
    assert_eq!(driver.app().popup.selected_idx, 1);
    driver.press_named(NamedKey::Down);
    assert_eq!(driver.app().popup.selected_idx, 2);
    driver.press_named(NamedKey::Up);
    assert_eq!(driver.app().popup.selected_idx, 1);
}

#[test]
fn page_and_edge_keys_navigate_the_whole_candidate_list() {
    let mut driver = driver();
    focus(&mut driver);

    driver.press_named(NamedKey::End);
    assert_eq!(driver.app().popup.selected_idx, 5);
    driver.press_named(NamedKey::Home);
    assert_eq!(driver.app().popup.selected_idx, 0);
    // Four rows fit the box, so one page forward lands on candidate 4.
    driver.press_named(NamedKey::PageDown);
    assert_eq!(driver.app().popup.selected_idx, 4);
}

/// Paging past the window's edge scrolls the highlighted candidate back
/// into view — the popup derives that offset itself, so the host never
/// writes a scroll field.
#[test]
fn navigating_past_the_window_scrolls_the_candidate_into_view() {
    let mut driver = driver();
    focus(&mut driver);
    driver.press_named(NamedKey::End);

    assert_eq!(driver.app().popup.scroll_offset, 0, "host set no scroll");
    assert!(driver.screen_contains("zeta"), "{}", driver.screen());
    assert!(!driver.screen_contains("alpha"), "{}", driver.screen());
}

#[test]
fn a_click_selects_the_row_under_the_pointer() {
    let mut driver = driver();
    let (x, y) = driver.find("gamma").expect("row 2 visible");
    driver.click(x, y);
    assert_eq!(driver.app().popup.selected_idx, 2);
    assert!(driver.app().accepted.is_empty(), "a click only selects");
}

#[test]
fn a_double_click_accepts_the_row_under_the_pointer() {
    let mut driver = driver();
    let (x, y) = driver.find("delta").expect("row 3 visible");
    driver.double_click(x, y);
    assert_eq!(driver.app().accepted, vec![3]);
}

#[test]
fn enter_accepts_the_highlighted_candidate() {
    let mut driver = driver();
    focus(&mut driver);
    driver.press_named(NamedKey::Down);
    driver.press_named(NamedKey::Enter);
    assert_eq!(driver.app().accepted, vec![1]);
    assert!(driver.app().dismissed);
}

#[test]
fn escape_dismisses_the_popup_without_accepting_anything() {
    let mut driver = driver();
    focus(&mut driver);
    driver.press_named(NamedKey::Escape);
    assert!(driver.app().dismissed);
    assert!(driver.app().accepted.is_empty());
    assert!(!driver.screen_contains("alpha"), "{}", driver.screen());
}
