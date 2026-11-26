#![cfg(feature = "tui")]
//! Black-box `TuiDriver` test for `MenuBar` on the widget contract: the
//! strip reaches the frame as a type-erased `Surface::Widget`, is painted
//! through `DynWidget::paint` → `Primitive::paint_cells`, and is
//! registered in the app's `FrameTree` so input is routed through the
//! shared widget-contract engine — `UiEvent` into
//! `quadraui::runtime::preprocess_event`'s widget routing (hit test,
//! click-to-focus, `Interactive::handle`) and back out as the `UiEvent`
//! this test's own `AppLogic::handle` applies onto the owned `MenuBar`,
//! which the next `render` paints.
//!
//! Never a direct call into `MenuBar::handle`/`Primitive::layout` that
//! would skip the routing engine. Two arms the engine cannot deliver are
//! covered by the widget module's own unit tests instead:
//!
//! - **Hover.** `route_widget_hover` only synthesises
//!   `PointerKind::Enter`/`Leave` for a *change of hovered widget* and
//!   deliberately discards their outcome, so the strip's
//!   `PointerKind::Move` arm (which drags an open menu along the bar) is
//!   only reachable under a pointer capture no menu bar takes.
//! - **Assistive-technology `Click`.** `RoutedEvent::Action` has no
//!   `UiEvent` an app can dispatch through the driver.

use quadraui::tui::ratatui_color;
use quadraui::tui::testing::TuiDriver;
use quadraui::{
    AppLogic, Backend, FrameTree, Key, ListViewEvent, MenuBar, MenuBarItem, Modifiers, NamedKey,
    Reaction, Rect, ScreenLayout, Surface, Theme, UiEvent, WidgetId,
};

const COLS: u16 = 40;
const ROWS: u16 = 6;
/// The strip's own row, inset so a click outside it is still inside the
/// terminal.
const BAR: Rect = Rect::new(1.0, 1.0, 30.0, 1.0);

const LABELS: [(&str, &str); 3] = [
    ("menu:file", "&File"),
    ("menu:edit", "&Edit"),
    ("menu:view", "&View"),
];

/// One menu bar, drawn and routed as a `Surface::Widget`, plus a record
/// of what the host was told to do with it.
struct MenuBarApp {
    bar: MenuBar,
    opened: Vec<WidgetId>,
    closed: usize,
}

impl MenuBarApp {
    fn new() -> Self {
        Self {
            bar: MenuBar {
                id: WidgetId::new("menubar:main"),
                items: LABELS
                    .iter()
                    .map(|(id, label)| MenuBarItem {
                        id: WidgetId::new(*id),
                        label: (*label).to_string(),
                        disabled: false,
                        submenu: None,
                    })
                    .collect(),
                open_item: None,
                focused_item: None,
            },
            opened: Vec::new(),
            closed: 0,
        }
    }

    fn index_of(&self, id: &WidgetId) -> Option<usize> {
        self.bar.items.iter().position(|item| &item.id == id)
    }
}

impl AppLogic for MenuBarApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let mut frame = ScreenLayout::new();
        frame.push(Surface::Widget {
            rect: BAR,
            widget: Box::new(self.bar.clone()),
        });
        frame.draw(backend);
    }

    fn frame_tree(&self, _area: ()) -> FrameTree {
        let mut tree = FrameTree::new();
        tree.push(self.bar.id, BAR, Box::new(self.bar.clone()));
        tree
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        match event {
            // A top-level menu asked to open: the strip reports the
            // item's own id, which an app resolves to its index the same
            // way a native menu installer's activation would be.
            UiEvent::MenuActivated(item) => match self.index_of(&item) {
                Some(idx) => {
                    self.opened.push(item);
                    self.bar.open_item = Some(idx);
                    self.bar.focused_item = Some(idx);
                    Reaction::Redraw
                }
                None => Reaction::Continue,
            },
            UiEvent::List(id, ListViewEvent::SelectionChanged { idx }) if id == self.bar.id => {
                self.bar.focused_item = Some(idx);
                Reaction::Redraw
            }
            UiEvent::List(id, ListViewEvent::KeyPressed { .. }) if id == self.bar.id => {
                self.closed += 1;
                self.bar.open_item = None;
                self.bar.focused_item = None;
                Reaction::Redraw
            }
            _ => Reaction::Continue,
        }
    }
}

fn driver() -> TuiDriver<MenuBarApp> {
    TuiDriver::new(MenuBarApp::new(), COLS, ROWS)
}

/// Click the strip's own background so the widget-contract
/// click-to-focus step runs — `MenuBar::focus()` is `FocusSpec::Stop`,
/// so nothing routes a key to it until then — without opening a menu.
fn focus(driver: &mut TuiDriver<MenuBarApp>) {
    driver.click(BAR.x + BAR.width - 1.0, BAR.y);
    assert!(driver.app().bar.open_item.is_none());
    assert!(driver.app().opened.is_empty());
}

#[test]
fn a_surface_widget_strip_paints_every_label_on_one_row() {
    let driver = driver();
    for label in ["File", "Edit", "View"] {
        assert!(
            driver.screen_contains(label),
            "{label} missing:\n{}",
            driver.screen()
        );
        let bounds = driver.find_bounds(label).expect("label painted");
        assert_eq!(bounds.y, BAR.y, "{}", driver.screen());
    }
    // One padding column ahead of the leading label.
    let file = driver.find_bounds("File").expect("File painted");
    assert_eq!(file.x, BAR.x + 1.0, "{}", driver.screen());
    // The `&` mnemonic marker itself never paints.
    assert!(!driver.screen_contains("&"), "{}", driver.screen());
}

#[test]
fn a_click_on_a_menu_opens_it_through_the_real_routing_engine() {
    let mut driver = driver();
    let (x, y) = driver.find("Edit").expect("Edit painted");
    driver.click(x, y);

    assert_eq!(driver.app().opened, vec![WidgetId::new("menu:edit")]);
    assert_eq!(driver.app().bar.open_item, Some(1));
}

#[test]
fn the_open_menu_is_tinted_through_the_painted_widget() {
    let mut driver = driver();
    let (x, y) = driver.find("Edit").expect("Edit painted");
    driver.click(x, y);

    // While the button is held, the pressed tint outranks the open fill.
    let theme = Theme::default();
    let edit = driver.find_bounds("Edit").expect("Edit painted");
    let held = driver
        .style_at(edit.x as u16, edit.y as u16)
        .expect("cell inside the strip");
    assert_eq!(
        held.bg,
        ratatui_color(theme.menu_item_pressed_bg(theme.tab_bar_bg))
    );

    // Released, the pointer still rests on the open menu, whose fill
    // hover must not weaken.
    driver.mouse_up(x, y);
    let style = driver
        .style_at(edit.x as u16, edit.y as u16)
        .expect("cell inside the strip");
    assert_eq!(style.bg, ratatui_color(theme.tab_active_bg));
    assert_eq!(style.fg, ratatui_color(theme.tab_active_fg));

    let file = driver.find_bounds("File").expect("File painted");
    let resting = driver
        .style_at(file.x as u16, file.y as u16)
        .expect("cell inside the strip");
    assert_eq!(resting.bg, ratatui_color(theme.tab_bar_bg));
    assert_eq!(resting.fg, ratatui_color(theme.tab_inactive_fg));
}

#[test]
fn a_second_click_on_the_open_menu_closes_it() {
    let mut driver = driver();
    // Two separate single clicks on the same label, not one double
    // click: the toggle is a `MouseDown` story.
    driver.set_double_click_folding(false);
    let (x, y) = driver.find("Edit").expect("Edit painted");
    driver.click(x, y);
    assert_eq!(driver.app().bar.open_item, Some(1));

    driver.click(x, y);
    assert_eq!(driver.app().closed, 1);
    assert_eq!(driver.app().bar.open_item, None);
}

#[test]
fn f10_then_right_walks_the_strip_without_opening_anything() {
    let mut driver = driver();
    focus(&mut driver);

    driver.press_named(NamedKey::F(10));
    assert_eq!(driver.app().bar.focused_item, Some(0));

    driver.press_named(NamedKey::Right);
    assert_eq!(driver.app().bar.focused_item, Some(1));
    driver.press_named(NamedKey::Left);
    assert_eq!(driver.app().bar.focused_item, Some(0));
    // Left again wraps to the trailing menu.
    driver.press_named(NamedKey::Left);
    assert_eq!(driver.app().bar.focused_item, Some(2));
    assert!(driver.app().bar.open_item.is_none());
    assert!(driver.app().opened.is_empty());
}

#[test]
fn down_opens_the_focused_menu_and_right_carries_the_open_state_along() {
    let mut driver = driver();
    focus(&mut driver);

    driver.press_named(NamedKey::F(10));
    driver.press_named(NamedKey::Down);
    assert_eq!(driver.app().bar.open_item, Some(0));

    driver.press_named(NamedKey::Right);
    assert_eq!(driver.app().bar.open_item, Some(1));
    assert_eq!(
        driver.app().opened,
        vec![WidgetId::new("menu:file"), WidgetId::new("menu:edit")]
    );
}

#[test]
fn alt_plus_a_mnemonic_opens_that_menu() {
    let mut driver = driver();
    focus(&mut driver);

    driver.dispatch(UiEvent::KeyPressed {
        key: Key::Char('v'),
        modifiers: Modifiers {
            alt: true,
            ..Modifiers::default()
        },
        repeat: false,
    });
    assert_eq!(driver.app().bar.open_item, Some(2));
    assert_eq!(driver.app().opened, vec![WidgetId::new("menu:view")]);
}

#[test]
fn escape_closes_the_open_menu_and_leaves_the_strip() {
    let mut driver = driver();
    let (x, y) = driver.find("View").expect("View painted");
    driver.click(x, y);
    assert_eq!(driver.app().bar.open_item, Some(2));

    driver.press_named(NamedKey::Escape);
    assert_eq!(driver.app().closed, 1);
    assert_eq!(driver.app().bar.open_item, None);
    assert_eq!(driver.app().bar.focused_item, None);

    // The strip is still painted, just with nothing open. The pointer
    // still rests on View, so it reads as hovered until it moves off.
    let theme = Theme::default();
    driver.mouse_up(x, y);
    let view = driver.find_bounds("View").expect("View painted");
    let hovered = driver
        .style_at(view.x as u16, view.y as u16)
        .expect("cell inside the strip");
    assert_eq!(
        hovered.bg,
        ratatui_color(theme.menu_item_hover_bg(theme.tab_bar_bg))
    );
    driver.mouse_move(x, f32::from(ROWS) - 1.0);
    let view = driver.find_bounds("View").expect("View painted");
    let style = driver
        .style_at(view.x as u16, view.y as u16)
        .expect("cell inside the strip");
    assert_eq!(style.bg, ratatui_color(theme.tab_bar_bg));
}

#[test]
fn a_disabled_menu_is_dimmed_and_unclickable() {
    let mut driver = TuiDriver::new(
        {
            let mut app = MenuBarApp::new();
            app.bar.items[1].disabled = true;
            app
        },
        COLS,
        ROWS,
    );

    let theme = Theme::default();
    let edit = driver.find_bounds("Edit").expect("Edit painted");
    let style = driver
        .style_at(edit.x as u16, edit.y as u16)
        .expect("cell inside the strip");
    assert_eq!(style.fg, ratatui_color(theme.muted_fg));

    driver.click(edit.x, edit.y);
    assert!(driver.app().opened.is_empty());
    assert_eq!(driver.app().bar.open_item, None);
}
