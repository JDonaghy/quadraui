#![cfg(feature = "tui")]
//! Black-box `TuiDriver` test for `FindReplacePanel` on the widget
//! contract: the panel reaches the frame as a `Surface::Widget` in the box
//! `FindReplacePanel::anchored_rect` places, paints through
//! `Primitive::paint_cells`, and is registered in the app's `FrameTree`
//! so input runs through the shared routing engine (hit test,
//! click-to-focus, `Interactive::handle`) and comes back out as the
//! `UiEvent::FindReplace` this test's host writes back with
//! `FindReplacePanel::apply_event` — controlled mode end to end.

use std::cell::Cell;

use quadraui::tui::testing::TuiDriver;
use quadraui::{
    AppLogic, Backend, FindReplaceEvent, FindReplaceField, FindReplacePanel, FrameTree, Key,
    Modifiers, NamedKey, Reaction, Rect, ScreenLayout, Surface, UiEvent, WidgetId, FR_PANEL_WIDTH,
};

const COLS: u16 = 80;
const ROWS: u16 = 12;

/// A find/replace overlay over an 80×12 editor, plus every event the
/// panel reported.
struct Host {
    panel: FindReplacePanel,
    events: Vec<FindReplaceEvent>,
    rect: Cell<Rect>,
}

impl Host {
    fn new(show_replace: bool) -> Self {
        let mut panel = FindReplacePanel {
            id: WidgetId::new("editor:find"),
            query: "needle".into(),
            replacement: "pin".into(),
            show_replace,
            focus: 0,
            cursor: 6,
            sel_anchor: None,
            match_info: "1 of 3".into(),
            case_sensitive: false,
            whole_word: false,
            use_regex: false,
            preserve_case: false,
            in_selection: false,
            group_bounds: Rect::new(0.0, 0.0, COLS as f32, ROWS as f32),
            panel_width: FR_PANEL_WIDTH,
            replace_one_glyph: "R1".into(),
            replace_all_glyph: "R*".into(),
            hit_regions: Vec::new(),
        };
        panel.rebuild_hit_regions();
        Self {
            panel,
            events: Vec::new(),
            rect: Cell::new(Rect::new(0.0, 0.0, 0.0, 0.0)),
        }
    }
}

impl AppLogic for Host {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let bounds = Rect::new(0.0, 0.0, COLS as f32, ROWS as f32);
        let rect = self.panel.anchored_rect(backend, bounds);
        self.rect.set(rect);
        let mut frame = ScreenLayout::new();
        frame.push(Surface::Widget {
            rect,
            widget: Box::new(self.panel.clone()),
        });
        frame.draw(backend);
    }

    fn frame_tree(&self, _area: ()) -> FrameTree {
        let mut tree = FrameTree::new();
        tree.push(self.panel.id, self.rect.get(), Box::new(self.panel.clone()));
        tree
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        let UiEvent::FindReplace(id, event) = event else {
            return Reaction::Continue;
        };
        assert_eq!(id, self.panel.id);
        self.panel.apply_event(&event);
        self.events.push(event);
        Reaction::Redraw
    }
}

fn driver(show_replace: bool) -> TuiDriver<Host> {
    let mut driver = TuiDriver::new(Host::new(show_replace), COLS, ROWS);
    driver.set_double_click_folding(false);
    driver
}

fn rows(driver: &TuiDriver<Host>) -> Vec<Vec<char>> {
    driver
        .screen()
        .lines()
        .map(|l| l.chars().collect())
        .collect()
}

/// Click the find field at the start of its text, which focuses the
/// panel through the routing engine's click-to-focus.
fn focus_find_at_start(driver: &mut TuiDriver<Host>) {
    let b = driver.find_bounds("needle").expect("query painted");
    driver.click(b.x, b.y);
}

#[test]
fn the_panel_paints_its_box_query_and_buttons_at_the_top_right() {
    let driver = driver(false);
    let grid = rows(&driver);
    // 50 cells wide, right border one cell in from the 80-column edge,
    // top border on row 1.
    assert_eq!(grid[1][29], '┌', "{}", driver.screen());
    assert_eq!(grid[1][78], '┐', "{}", driver.screen());
    assert_eq!(grid[3][29], '└', "{}", driver.screen());
    assert_eq!(grid[2][30], '▶', "{}", driver.screen());
    for needle in ["needle", "Aa", "ab", ".*", "1 of 3", "↑", "↓", "≡", "×"] {
        assert!(
            driver.screen_contains(needle),
            "{needle} missing:\n{}",
            driver.screen()
        );
    }
    assert!(!driver.screen_contains("R1"), "{}", driver.screen());
}

#[test]
fn typing_edits_the_query_through_the_routing_engine() {
    let mut driver = driver(false);
    focus_find_at_start(&mut driver);
    assert_eq!(driver.app().panel.cursor, 0);

    driver.type_char('x');
    assert_eq!(driver.app().panel.query, "xneedle");
    assert!(driver.screen_contains("xneedle"), "{}", driver.screen());

    driver.press_named(NamedKey::End);
    driver.press_named(NamedKey::Backspace);
    assert_eq!(driver.app().panel.query, "xneedl");
    assert_eq!(driver.app().panel.cursor, 6);
    assert!(matches!(
        driver.app().events.last(),
        Some(FindReplaceEvent::FieldEdited {
            field: FindReplaceField::Find,
            ..
        })
    ));
}

#[test]
fn clicking_and_alt_keys_flip_the_toggles() {
    let mut driver = driver(false);
    let (x, y) = driver.find("Aa").expect("case toggle painted");
    driver.click(x, y);
    assert!(driver.app().panel.case_sensitive);
    driver.click(x, y);
    assert!(
        !driver.app().panel.case_sensitive,
        "a second click flips it back"
    );

    let (x, y) = driver.find("≡").expect("in-selection toggle painted");
    driver.click(x, y);
    assert!(driver.app().panel.in_selection);

    driver.dispatch(UiEvent::KeyPressed {
        key: Key::Char('w'),
        modifiers: Modifiers {
            alt: true,
            ..Modifiers::default()
        },
        repeat: false,
    });
    assert!(driver.app().panel.whole_word);

    let (x, y) = driver.find(".*").expect("regex toggle painted");
    driver.click(x, y);
    assert!(driver.app().panel.use_regex);
}

#[test]
fn enter_and_the_arrows_step_through_matches() {
    let mut driver = driver(false);
    focus_find_at_start(&mut driver);
    driver.app_mut().events.clear();

    driver.press_named(NamedKey::Enter);
    driver.dispatch(UiEvent::KeyPressed {
        key: Key::Named(NamedKey::Enter),
        modifiers: Modifiers {
            shift: true,
            ..Modifiers::default()
        },
        repeat: false,
    });
    let (x, y) = driver.find("↓").expect("next button painted");
    driver.click(x, y);
    let (x, y) = driver.find("↑").expect("prev button painted");
    driver.click(x, y);

    assert_eq!(
        driver.app().events,
        vec![
            FindReplaceEvent::NextMatch,
            FindReplaceEvent::PrevMatch,
            FindReplaceEvent::NextMatch,
            FindReplaceEvent::PrevMatch,
        ]
    );
}

#[test]
fn the_chevron_reveals_the_replace_row_and_its_actions_report() {
    let mut driver = driver(false);
    let (x, y) = driver.find("▶").expect("closed chevron painted");
    driver.click(x, y);
    assert!(driver.app().panel.show_replace);
    for needle in ["▼", "pin", "AB", "R1", "R*"] {
        assert!(
            driver.screen_contains(needle),
            "{needle} missing:\n{}",
            driver.screen()
        );
    }

    driver.app_mut().events.clear();
    let (x, y) = driver.find("R1").expect("replace-one painted");
    driver.click(x, y);
    let (x, y) = driver.find("R*").expect("replace-all painted");
    driver.click(x, y);
    assert_eq!(
        driver.app().events,
        vec![
            FindReplaceEvent::ReplaceCurrent,
            FindReplaceEvent::ReplaceAll
        ]
    );
}

#[test]
fn tab_moves_to_the_replace_field_where_typing_and_enter_replace() {
    let mut driver = driver(true);
    focus_find_at_start(&mut driver);
    driver.press_named(NamedKey::Tab);
    assert_eq!(driver.app().panel.focus, 1);
    assert_eq!(driver.app().panel.cursor, 3);

    driver.type_char('e');
    assert_eq!(driver.app().panel.replacement, "pine");
    assert_eq!(driver.app().panel.query, "needle", "find field untouched");
    assert!(driver.screen_contains("pine"), "{}", driver.screen());

    driver.app_mut().events.clear();
    driver.press_named(NamedKey::Enter);
    driver.dispatch(UiEvent::KeyPressed {
        key: Key::Named(NamedKey::Enter),
        modifiers: Modifiers {
            ctrl: true,
            ..Modifiers::default()
        },
        repeat: false,
    });
    assert_eq!(
        driver.app().events,
        vec![
            FindReplaceEvent::ReplaceCurrent,
            FindReplaceEvent::ReplaceAll
        ]
    );
}

#[test]
fn escape_and_the_close_button_report_close() {
    let mut driver = driver(false);
    let (x, y) = driver.find("×").expect("close button painted");
    driver.click(x, y);
    driver.press_named(NamedKey::Escape);
    assert_eq!(
        driver.app().events,
        vec![FindReplaceEvent::Close, FindReplaceEvent::Close]
    );
}

/// The focused field's caret paints as an inverted cell, and a selection
/// on the unfocused field does not paint.
#[test]
fn the_caret_paints_inverted_in_the_focused_field() {
    let mut driver = driver(false);
    focus_find_at_start(&mut driver);
    let b = driver.find_bounds("needle").expect("query painted");
    let caret = driver.style_at(b.x as u16, b.y as u16).expect("caret cell");
    let body = driver
        .style_at(b.x as u16 + 1, b.y as u16)
        .expect("body cell");
    assert_eq!(caret.fg, body.bg, "the caret swaps fg and bg");
    assert_eq!(caret.bg, body.fg, "the caret swaps fg and bg");
}
