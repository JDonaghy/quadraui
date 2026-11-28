#![cfg(feature = "tui")]
//! The app layer (`quadraui::app`) driven headless through `TuiDriver`: a
//! [`View`] wrapped in [`Run`] builds its tree once per frame, lays it out
//! with per-child sizes, routes clicks and keys to the right leaf, and
//! applies the resulting messages through `update`.

use std::cell::Cell;

use quadraui::prelude::*;
use quadraui::tui::testing::TuiDriver;

#[derive(Default)]
struct Counter {
    count: i32,
    builds: Cell<usize>,
    unclaimed: Vec<String>,
}

#[derive(Clone)]
enum Msg {
    Inc,
    Dec,
    Key(String),
    Stop,
}

impl View for Counter {
    type Msg = Msg;
    fn view(&self) -> Node<Msg> {
        self.builds.set(self.builds.get() + 1);
        column([
            text(format!("Count: {}", self.count)).id("count").content(),
            row([
                button("-").id("dec").on_click(Msg::Dec).content(),
                button("+").on_click(Msg::Inc).content(),
                button("stop").id("stop").on_click(Msg::Stop).content(),
            ])
            .gap(8.0)
            .content(),
        ])
        .gap(16.0)
    }
    fn update(&mut self, msg: Msg) -> Command {
        match msg {
            Msg::Inc => self.count += 1,
            Msg::Dec => self.count -= 1,
            Msg::Key(name) => self.unclaimed.push(name),
            Msg::Stop => return Command::Exit,
        }
        Command::None
    }
    fn on_event(&self, event: &UiEvent) -> Option<Msg> {
        match event {
            UiEvent::KeyPressed {
                key: Key::Char(c @ ('a' | 'b')),
                ..
            } => Some(Msg::Key(c.to_string())),
            _ => None,
        }
    }
}

/// The cell where `needle` starts on screen.
fn at(driver: &TuiDriver<Run<impl View>>, needle: &str) -> Option<(usize, usize)> {
    let screen = driver.screen();
    screen
        .lines()
        .enumerate()
        .find_map(|(y, line)| line.find(needle).map(|x| (line[..x].chars().count(), y)))
}

fn click(driver: &mut TuiDriver<Run<impl View>>, label: &str) -> Reaction {
    let (x, y) = driver
        .find(label)
        .unwrap_or_else(|| panic!("{label:?} not on screen:\n{}", driver.screen()));
    driver.click(x, y)
}

#[test]
fn a_message_updates_the_state_and_the_next_frame_shows_it() {
    let mut driver = TuiDriver::new(Run::new(Counter::default()), 30, 5);
    assert!(driver.screen_contains("Count: 0"), "{}", driver.screen());
    click(&mut driver, "[ + ]");
    driver.render();
    assert_eq!(driver.app().count, 1);
    assert!(driver.screen_contains("Count: 1"), "{}", driver.screen());
    click(&mut driver, "[ - ]");
    driver.render();
    assert_eq!(driver.app().count, 0);
    assert!(driver.screen_contains("Count: 0"), "{}", driver.screen());
}

#[test]
fn each_paint_builds_the_view_once_and_routing_reuses_it() {
    let mut driver = TuiDriver::new(Run::new(Counter::default()), 30, 5);
    let after_first_paint = driver.app().builds.get();
    // A press on empty space routes against the kept frame: no rebuild.
    driver.click(25.0, 4.0);
    assert_eq!(driver.app().builds.get(), after_first_paint);
    driver.render();
    assert_eq!(driver.app().builds.get(), after_first_paint + 1);
}

#[test]
fn an_event_routed_before_the_next_paint_sees_the_updated_state() {
    let mut driver = TuiDriver::new(Run::new(Counter::default()), 30, 5);
    // No render between the clicks: the second routes against a tree
    // rebuilt from the state the first one changed.
    click(&mut driver, "[ + ]");
    let (x, y) = driver.find("[ - ]").expect("dec on screen");
    driver.click(x, y);
    assert_eq!(driver.app().count, 0);
}

#[test]
fn per_child_sizes_place_content_sized_children_with_gaps() {
    let driver = TuiDriver::new(Run::new(Counter::default()), 30, 5);
    // The label is one row, a 16px gap is one row, and the content-sized
    // buttons sit side by side with a one-cell (8px) gap.
    assert_eq!(at(&driver, "Count: 0"), Some((0, 0)), "{}", driver.screen());
    assert_eq!(at(&driver, "[ - ]"), Some((0, 2)), "{}", driver.screen());
    assert_eq!(at(&driver, "[ + ]"), Some((6, 2)), "{}", driver.screen());
}

#[test]
fn an_unnamed_leaf_is_routed_by_its_path_id() {
    let mut driver = TuiDriver::new(Run::new(Counter::default()), 30, 5);
    // `+` has no `.id(..)`; it is still addressable and routed.
    click(&mut driver, "[ + ]");
    assert_eq!(driver.app().count, 1);
}

#[test]
fn update_returning_exit_stops_the_app() {
    let mut driver = TuiDriver::new(Run::new(Counter::default()), 30, 5);
    assert_eq!(click(&mut driver, "stop"), Reaction::Exit);
}

#[test]
fn on_event_sees_keys_no_widget_claimed() {
    let mut driver = TuiDriver::new(Run::new(Counter::default()), 30, 5);
    assert_eq!(driver.press(Key::Char('b')), Reaction::Redraw);
    assert_eq!(driver.app().unclaimed, vec!["b".to_string()]);
    assert_eq!(driver.press(Key::Char('z')), Reaction::Continue);
}

#[test]
fn q_escape_and_ctrl_c_quit_by_default() {
    for key in [Key::Char('q'), Key::Named(NamedKey::Escape)] {
        let mut driver = TuiDriver::new(Run::new(Counter::default()), 30, 5);
        assert_eq!(driver.press(key.clone()), Reaction::Exit, "{key:?}");
    }
    let mut driver = TuiDriver::new(Run::new(Counter::default()), 30, 5);
    assert_eq!(driver.ctrl_char('c'), Reaction::Exit);
}

struct NoQuit;

impl View for NoQuit {
    type Msg = ();
    fn view(&self) -> Node<()> {
        text("stays open")
    }
    fn update(&mut self, _: ()) -> Command {
        Command::None
    }
    fn quits_on(&self, _: &UiEvent) -> bool {
        false
    }
}

#[test]
fn a_view_can_opt_out_of_the_quit_keys() {
    let mut driver = TuiDriver::new(Run::new(NoQuit), 20, 2);
    assert_eq!(driver.press(Key::Char('q')), Reaction::Continue);
    assert!(driver.screen_contains("stays open"));
}

struct Sizes;

impl View for Sizes {
    type Msg = ();
    fn view(&self) -> Node<()> {
        column([
            row([
                text("AAAA").id("a").fixed(32),
                text("BBBB").id("b").weight(1.0),
                text("CCCC").id("c").weight(3.0),
            ])
            .content(),
            row([text("DDDD").id("d").max(32.0), text("EEEE").id("e")]),
            row([text("FFFF").id("f").min(240.0), text("GGGG").id("g")]),
        ])
    }
    fn update(&mut self, _: ()) -> Command {
        Command::None
    }
}

#[test]
fn fixed_weight_min_and_max_resolve_through_the_stack_solver() {
    let driver = TuiDriver::new(Run::new(Sizes), 44, 3);
    let tree = driver.app().frame_tree(());
    let rect = |id: &str| {
        let node = tree.find(&WidgetId::new(id)).expect("leaf routed");
        (node.rect.x, node.rect.y, node.rect.width, node.rect.height)
    };
    // Row 1 is content-sized (one row): `fixed(32)` is four cells and the
    // 40 left split 1:3.
    assert_eq!(rect("a"), (0.0, 0.0, 4.0, 1.0));
    assert_eq!(rect("b"), (4.0, 0.0, 10.0, 1.0));
    assert_eq!(rect("c"), (14.0, 0.0, 30.0, 1.0));
    // Rows 2 and 3 share the two rows left and split 44 cells equally
    // before the clamps: `max(32.0)` caps D at four cells, so E follows it
    // at once, and `min(240.0)` raises F to thirty.
    assert_eq!(rect("d"), (0.0, 1.0, 4.0, 1.0));
    assert_eq!(rect("e"), (4.0, 1.0, 22.0, 1.0));
    assert_eq!(rect("f"), (0.0, 2.0, 30.0, 1.0));
    assert_eq!(rect("g"), (30.0, 2.0, 22.0, 1.0));
}

#[derive(Default)]
struct Todo {
    draft: String,
    items: Vec<String>,
    selected: usize,
}

#[derive(Clone)]
enum TodoMsg {
    Edit(String),
    Add,
    Select(usize),
}

impl View for Todo {
    type Msg = TodoMsg;
    fn view(&self) -> Node<TodoMsg> {
        let field = text_field("draft", &self.draft).label("New");
        column([
            form([field.on_change(TodoMsg::Edit).on_submit(|_| TodoMsg::Add)]).id("form"),
            list(&self.items)
                .selected(self.selected)
                .id("items")
                .on_select(TodoMsg::Select),
        ])
    }
    fn update(&mut self, msg: TodoMsg) -> Command {
        match msg {
            TodoMsg::Edit(text) => self.draft = text,
            TodoMsg::Add => self.items.push(std::mem::take(&mut self.draft)),
            TodoMsg::Select(idx) => self.selected = idx,
        }
        Command::None
    }
}

#[test]
fn a_form_field_reports_edits_and_submit_and_keeps_its_focus() {
    let mut driver = TuiDriver::new(Run::new(Todo::default()), 30, 8);
    let (x, y) = driver.find("New").expect("field label on screen");
    driver.click(x + 10.0, y);
    // Typed without a paint in between: each key lands on the field the
    // previous one updated.
    for c in "Tea".chars() {
        driver.press(Key::Char(c));
    }
    assert_eq!(driver.app().draft, "Tea");
    driver.press(Key::Named(NamedKey::Enter));
    driver.render();
    assert_eq!(driver.app().items, vec!["Tea".to_string()]);
    assert!(driver.screen_contains("Tea"), "{}", driver.screen());
}

#[test]
fn a_list_click_reports_the_selected_row() {
    let todo = Todo {
        items: vec!["one".into(), "two".into()],
        ..Todo::default()
    };
    let mut driver = TuiDriver::new(Run::new(todo), 30, 8);
    click(&mut driver, "two");
    assert_eq!(driver.app().selected, 1);
}

#[cfg(not(any(
    feature = "gtk",
    all(feature = "macos", target_os = "macos"),
    all(feature = "win", target_os = "windows")
)))]
#[test]
fn run_native_without_a_native_backend_reports_unsupported() {
    let err = run_native(NoQuit).expect_err("no native backend in this build");
    assert!(
        matches!(
            err,
            quadraui::RunError::Backend(quadraui::BackendError::Unsupported)
        ),
        "{err}"
    );
}
