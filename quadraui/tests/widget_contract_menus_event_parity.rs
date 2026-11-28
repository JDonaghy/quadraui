#![cfg(feature = "tui")]
//! The TuiDriver leg of the event parity for the menu widgets: `Toolbar`,
//! `MenuBar` and `ContextMenu` each reach the frame as a `Surface::Widget`
//! registered in the app's `FrameTree`, and a real mouse click through the
//! routing engine must surface exactly the `UiEvent` the
//! assistive-technology leg (`a11y::route_widget_action`, asserted in each
//! widget module's own tests) resolves to.
//!
//! `Toolbar` and `ContextMenu` also have a binding-host leg
//! (`quadraui-binding-core`'s engine tests). The binding layer's
//! `"menu_bar"` is the whole application menu, an `AppMenu`, so its
//! binding-host leg pairs with this file's `AppMenu` test rather than the
//! bare `MenuBar` strip's: a click on a top-level item opens its dropdown,
//! and a click on a dropdown row activates it.

use quadraui::primitive::DynWidget;
use quadraui::tui::testing::TuiDriver;
use quadraui::{
    AppLogic, AppMenu, AppMenuEvent, AppMenuView, Backend, ContextMenu, ContextMenuItem,
    ContextMenuPlacement, FrameTree, MenuBar, MenuBarItem, MenuDef, Reaction, Rect, ScreenLayout,
    StyledText, Surface, Toolbar, ToolbarButton, ToolbarEvent, UiEvent, WidgetId,
};

const COLS: u16 = 40;
const ROWS: u16 = 12;

/// One widget, painted and routed as a `Surface::Widget` at `rect`, plus
/// every event the routing engine handed the app.
struct WidgetApp<W: DynWidget + Clone + 'static> {
    widget: W,
    id: WidgetId,
    rect: Rect,
    events: Vec<UiEvent>,
}

impl<W: DynWidget + Clone + 'static> WidgetApp<W> {
    fn new(widget: W, id: WidgetId, rect: Rect) -> Self {
        Self {
            widget,
            id,
            rect,
            events: Vec::new(),
        }
    }
}

impl<W: DynWidget + Clone + 'static> AppLogic for WidgetApp<W> {
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
        Reaction::Redraw
    }
}

/// Clicks the first cell of `label`'s painted run.
fn click_label<W: DynWidget + Clone + 'static>(driver: &mut TuiDriver<WidgetApp<W>>, label: &str) {
    let bounds = driver
        .find_bounds(label)
        .unwrap_or_else(|| panic!("{label} on screen:\n{}", driver.screen()));
    driver.mouse_down(bounds.x + 0.5, bounds.y);
    driver.mouse_up(bounds.x + 0.5, bounds.y);
}

/// The events the routing engine produced, minus the raw pointer events
/// that bubble alongside them.
fn semantic_events<W: DynWidget + Clone + 'static>(
    driver: &TuiDriver<WidgetApp<W>>,
) -> Vec<UiEvent> {
    driver
        .app()
        .events
        .iter()
        .filter(|e| {
            !matches!(
                e,
                UiEvent::MouseDown { .. } | UiEvent::MouseUp { .. } | UiEvent::MouseMoved { .. }
            )
        })
        .cloned()
        .collect()
}

fn action(id: &str, label: &str, enabled: bool) -> ToolbarButton {
    ToolbarButton::Action {
        id: WidgetId::new(id),
        label: label.to_string(),
        icon: None,
        key_hint: None,
        enabled,
        is_active: false,
        tooltip: String::new(),
        spec: None,
    }
}

/// `[ A ]` `│` `[ B ]` (B disabled) — the same bar shape the a11y leg in
/// `primitives::toolbar::contract`'s tests routes `Click` through.
fn toolbar() -> Toolbar {
    Toolbar::new(WidgetId::new("tb")).with_buttons(vec![
        action("tb:a", "A", true),
        ToolbarButton::Separator,
        action("tb:b", "B", false),
    ])
}

#[test]
fn a_click_on_the_first_toolbar_button_matches_the_parity_event() {
    let bar = toolbar();
    let id = bar.id;
    let mut driver = TuiDriver::new(
        WidgetApp::new(bar, id, Rect::new(3.0, 2.0, 30.0, 1.0)),
        COLS,
        ROWS,
    );
    click_label(&mut driver, "A");
    assert_eq!(
        semantic_events(&driver),
        vec![UiEvent::Toolbar(
            id,
            ToolbarEvent::ButtonClicked {
                id: WidgetId::new("tb:a"),
                is_active: true,
            },
        )]
    );
}

fn menu_item(id: &str, label: &str) -> MenuBarItem {
    MenuBarItem {
        id: WidgetId::new(id),
        label: label.into(),
        disabled: false,
        submenu: None,
    }
}

#[test]
fn a_click_on_a_menu_bar_item_matches_the_parity_event() {
    let bar = MenuBar {
        id: WidgetId::new("menubar:main"),
        items: vec![
            menu_item("menu:file", "&File"),
            menu_item("menu:edit", "&Edit"),
        ],
        open_item: None,
        focused_item: Some(0),
    };
    let id = bar.id;
    let mut driver = TuiDriver::new(
        WidgetApp::new(bar, id, Rect::new(0.0, 0.0, 40.0, 1.0)),
        COLS,
        ROWS,
    );
    click_label(&mut driver, "File");
    assert_eq!(
        semantic_events(&driver),
        vec![UiEvent::MenuActivated(WidgetId::new("menu:file"))]
    );
}

fn context_item(id: &str, label: &str) -> ContextMenuItem {
    ContextMenuItem {
        id: Some(WidgetId::new(id)),
        label: StyledText::plain(label),
        ..Default::default()
    }
}

#[test]
fn a_click_on_a_context_menu_row_matches_the_parity_event() {
    let menu = ContextMenu {
        id: WidgetId::new("context_menu:editor"),
        items: vec![context_item("copy", "Copy"), context_item("paste", "Paste")],
        selected_idx: 0,
        bg: None,
        placement: ContextMenuPlacement::AnchorPoint,
        glyphs: Default::default(),
    };
    let id = menu.id;
    let mut driver = TuiDriver::new(
        WidgetApp::new(menu, id, Rect::new(2.0, 1.0, 22.0, 6.0)),
        COLS,
        ROWS,
    );
    click_label(&mut driver, "Copy");
    let events = semantic_events(&driver);
    assert!(
        events.contains(&UiEvent::ContextMenuItemActivated(WidgetId::new("copy"))),
        "a click on the highlighted row activates it: {events:?}"
    );
}

/// An `AppMenu` painted and routed as a `Surface::Widget`, whose app
/// applies every view change it reports — the controlled contract.
struct AppMenuApp {
    menu: AppMenu,
    rect: Rect,
    events: Vec<UiEvent>,
}

impl AppLogic for AppMenuApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let mut frame = ScreenLayout::new();
        frame.push(Surface::Widget {
            rect: self.rect,
            widget: Box::new(self.menu.clone()),
        });
        frame.draw(backend);
    }

    fn frame_tree(&self, _area: ()) -> FrameTree {
        let mut tree = FrameTree::new();
        tree.push(self.menu.id, self.rect, Box::new(self.menu.clone()));
        tree
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        if let UiEvent::AppMenu(_, change) = &event {
            self.menu.apply(change);
        }
        self.events.push(event);
        Reaction::Redraw
    }
}

/// The TuiDriver leg of the application menu's event parity: a click on
/// "File" reports the opened view, and a click on "Save" in the dropdown
/// popup reports the closed view, then the activation. The binding host's
/// `"menu_bar"` leaf reports the same events for the same clicks
/// (`quadraui-binding-core`'s
/// `menu_bar_click_open_then_activate_matches_the_widget_contract_parity_events`).
#[test]
fn a_click_open_then_activate_on_an_app_menu_matches_the_parity_events() {
    let id = WidgetId::new("menubar:main");
    let menu = AppMenu::new(
        id,
        vec![
            MenuDef {
                id: WidgetId::new("menu:file"),
                label: "&File".into(),
                disabled: false,
                items: vec![context_item("menu:file:save", "Save")],
            },
            MenuDef {
                id: WidgetId::new("menu:edit"),
                label: "&Edit".into(),
                disabled: false,
                items: vec![context_item("menu:edit:undo", "Undo")],
            },
        ],
    );
    let mut driver = TuiDriver::new(
        AppMenuApp {
            menu,
            rect: Rect::new(0.0, 0.0, 40.0, 3.0),
            events: Vec::new(),
        },
        COLS,
        ROWS,
    );
    let (x, y) = driver.find("File").expect("the bar paints its items");
    driver.click(x, y);
    let (x, y) = driver
        .find("Save")
        .unwrap_or_else(|| panic!("the open dropdown paints its rows:\n{}", driver.screen()));
    driver.click(x, y);

    let events: Vec<UiEvent> = driver
        .app()
        .events
        .iter()
        .filter(|e| matches!(e, UiEvent::AppMenu(..) | UiEvent::MenuActivated(_)))
        .cloned()
        .collect();
    assert_eq!(
        events,
        vec![
            UiEvent::AppMenu(
                id,
                AppMenuEvent::Changed(AppMenuView {
                    open_item: Some(0),
                    focused_item: Some(0),
                    ..Default::default()
                }),
            ),
            UiEvent::AppMenu(id, AppMenuEvent::Changed(AppMenuView::default())),
            UiEvent::MenuActivated(WidgetId::new("menu:file:save")),
        ]
    );
}
