#![cfg(any(feature = "tui", all(feature = "macos", target_os = "macos")))]
//! Black-box tests for `AppMenu` on the widget contract: the menu bar
//! reaches the frame as a `Surface::Widget` and paints through
//! `MenuBar`'s own painter, and each open level of the dropdown cascade
//! is a widget-owned popup (`Interactive::popups`) the runner paints
//! above the rest of the frame and hit-tests first. View changes arrive
//! as `UiEvent::AppMenu`, which the app applies onto its descriptor; an
//! activated item arrives as `UiEvent::MenuActivated`. Driven on the TUI
//! backend and on the macOS pixel backend.

use quadraui::{
    AppLogic, AppMenu, Backend, Color, ContextMenuItem, DrawsPrimitives, FrameTree, MenuDef,
    Reaction, Rect, StatusBar, StatusBarSegment, StyledText, UiEvent, WidgetId,
};

fn action(id: &str, label: &str) -> ContextMenuItem {
    ContextMenuItem {
        id: Some(WidgetId::new(id)),
        label: StyledText::plain(label),
        ..Default::default()
    }
}

fn menus() -> Vec<MenuDef> {
    vec![
        MenuDef {
            id: WidgetId::new("file"),
            label: "&File".into(),
            disabled: false,
            items: vec![
                action("new", "New File"),
                action("save", "Save"),
                ContextMenuItem {
                    id: Some(WidgetId::new("export")),
                    label: StyledText::plain("Export"),
                    submenu: Some(vec![action("png", "PNG"), action("svg", "SVG")]),
                    ..Default::default()
                },
                action("quit", "Quit"),
            ],
        },
        MenuDef {
            id: WidgetId::new("edit"),
            label: "&Edit".into(),
            disabled: false,
            items: vec![action("undo", "Undo"), action("redo", "Redo")],
        },
    ]
}

/// A menu bar over a status bar its dropdown drops across; the app owns
/// the menu's view and records what was activated.
struct MenuApp {
    menu: AppMenu,
    bar: Rect,
    base: StatusBar,
    base_rect: Rect,
    activated: Vec<WidgetId>,
}

impl MenuApp {
    fn new(bar: Rect, base_rect: Rect) -> Self {
        Self {
            menu: AppMenu::new(WidgetId::new("app-menu"), menus()),
            bar,
            base: StatusBar {
                id: WidgetId::new("base"),
                left_segments: vec![StatusBarSegment {
                    text: "#".repeat(200),
                    fg: Color::rgb(200, 200, 200),
                    bg: Color::rgb(30, 30, 30),
                    bold: false,
                    action_id: None,
                }],
                right_segments: Vec::new(),
            },
            base_rect,
            activated: Vec::new(),
        }
    }
}

impl AppLogic for MenuApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        backend.draw_dyn(self.bar, &self.menu, &());
        // Painted after the bar, so only the runner's popup pass can put
        // the dropdown above it.
        backend.draw_dyn(self.base_rect, &self.base, &());
    }

    fn frame_tree(&self, _area: ()) -> FrameTree {
        let mut tree = FrameTree::new();
        tree.push(self.menu.id, self.bar, Box::new(self.menu.clone()));
        tree.push(self.base.id, self.base_rect, Box::new(self.base.clone()));
        tree
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        match event {
            UiEvent::AppMenu(id, change) if id == self.menu.id => self.menu.apply(&change),
            UiEvent::MenuActivated(item) => self.activated.push(item),
            _ => return Reaction::Continue,
        }
        Reaction::Redraw
    }
}

#[cfg(feature = "tui")]
mod tui {
    use super::*;
    use quadraui::tui::testing::TuiDriver;
    use quadraui::NamedKey;

    const COLS: u16 = 60;
    const ROWS: u16 = 16;
    const BAR: Rect = Rect::new(0.0, 0.0, COLS as f32, 1.0);
    const BASE: Rect = Rect::new(0.0, 3.0, COLS as f32, 1.0);

    fn driver() -> TuiDriver<MenuApp> {
        let mut driver = TuiDriver::new(MenuApp::new(BAR, BASE), COLS, ROWS);
        driver.set_double_click_folding(false);
        driver
    }

    fn open_file(driver: &mut TuiDriver<MenuApp>) {
        let (x, y) = driver.find("File").expect("File on the bar");
        driver.click(x, y);
        assert_eq!(driver.app().menu.view.open_item, Some(0));
    }

    #[test]
    fn a_closed_menu_paints_only_its_bar() {
        let driver = driver();
        assert!(driver.screen_contains("File"), "{}", driver.screen());
        assert!(driver.screen_contains("Edit"), "{}", driver.screen());
        assert!(!driver.screen_contains("New File"), "{}", driver.screen());
        assert!(quadraui::popup::painted_popups().is_empty());
    }

    #[test]
    fn clicking_a_menu_opens_its_dropdown_above_later_content() {
        let mut driver = driver();
        open_file(&mut driver);
        assert!(driver.screen_contains("New File"), "{}", driver.screen());
        let popups = quadraui::popup::painted_popups();
        assert_eq!(popups.len(), 1);
        assert_eq!(popups[0].owner, WidgetId::new("app-menu"));
        assert_eq!(popups[0].rect.y, 1.0, "directly under the bar");

        // The status bar painted after the menu is covered where the
        // dropdown lies.
        let row: String = driver
            .screen()
            .lines()
            .nth(BASE.y as usize)
            .unwrap_or_default()
            .to_string();
        let inside: String = row
            .chars()
            .skip(popups[0].rect.x as usize + 1)
            .take(popups[0].rect.width as usize - 2)
            .collect();
        assert!(!inside.contains('#'), "{}", driver.screen());
    }

    #[test]
    fn clicking_an_item_activates_it_and_closes_the_menu() {
        let mut driver = driver();
        open_file(&mut driver);
        let (x, y) = driver.find("Save").expect("Save in the dropdown");
        driver.click(x, y);
        assert_eq!(driver.app().activated, vec![WidgetId::new("save")]);
        assert_eq!(driver.app().menu.view.open_item, None);
        assert!(!driver.screen_contains("New File"), "{}", driver.screen());
    }

    #[test]
    fn hovering_a_submenu_parent_opens_its_submenu_as_a_nested_popup() {
        let mut driver = driver();
        open_file(&mut driver);
        let (x, y) = driver.find("Export").expect("Export row");
        driver.mouse_move(x, y);
        assert_eq!(driver.app().menu.view.submenu_path, vec![2]);
        assert!(driver.screen_contains("PNG"), "{}", driver.screen());
        assert_eq!(quadraui::popup::painted_popups().len(), 2);

        let (x, y) = driver.find("SVG").expect("SVG row");
        driver.click(x, y);
        assert_eq!(driver.app().activated, vec![WidgetId::new("svg")]);
        assert_eq!(driver.app().menu.view.open_item, None);
    }

    #[test]
    fn escape_closes_the_deepest_level_first() {
        let mut driver = driver();
        open_file(&mut driver);
        let (x, y) = driver.find("Export").expect("Export row");
        driver.mouse_move(x, y);
        assert!(driver.screen_contains("PNG"), "{}", driver.screen());

        driver.press_named(NamedKey::Escape);
        assert!(driver.app().menu.view.submenu_path.is_empty());
        assert_eq!(driver.app().menu.view.open_item, Some(0));
        assert!(!driver.screen_contains("PNG"), "{}", driver.screen());

        driver.press_named(NamedKey::Escape);
        assert_eq!(driver.app().menu.view.open_item, None);
    }

    #[test]
    fn an_outside_click_dismisses_the_cascade() {
        let mut driver = driver();
        open_file(&mut driver);
        driver.click(55.0, 14.0);
        assert_eq!(driver.app().menu.view.open_item, None);
        assert!(driver.app().activated.is_empty());
    }

    #[test]
    fn arrow_keys_walk_the_dropdown_and_the_bar() {
        let mut driver = driver();
        open_file(&mut driver);
        driver.press_named(NamedKey::Down);
        assert_eq!(driver.app().menu.view.dropdown_selected, 1);
        driver.press_named(NamedKey::Right);
        assert_eq!(
            driver.app().menu.view.open_item,
            Some(1),
            "Right walks the bar"
        );
        assert!(driver.screen_contains("Undo"), "{}", driver.screen());
        driver.press_named(NamedKey::Down);
        driver.press_named(NamedKey::Enter);
        assert_eq!(driver.app().activated, vec![WidgetId::new("redo")]);
    }
}

#[cfg(all(feature = "macos", target_os = "macos"))]
mod mac {
    use super::*;
    use quadraui::macos::testing::MacDriver;

    const W: u32 = 640;
    const H: u32 = 360;
    const BAR: Rect = Rect::new(0.0, 0.0, W as f32, 30.0);
    const BASE: Rect = Rect::new(0.0, 80.0, W as f32, 24.0);

    #[test]
    fn opens_the_dropdown_and_activates_an_item() {
        let mut driver = MacDriver::new(MenuApp::new(BAR, BASE), W, H);
        assert!(
            driver.screen_contains("File"),
            "{:?}",
            driver.painted_texts()
        );
        assert!(!driver.screen_contains("New File"));
        let (x, y) = driver.find("File").expect("File painted");
        driver.click(x, y);
        assert!(
            driver.screen_contains("New File"),
            "{:?}",
            driver.painted_texts()
        );
        let (x, y) = driver.find("Save").expect("Save painted");
        driver.click(x, y);
        assert_eq!(driver.app().activated, vec![WidgetId::new("save")]);
        assert!(!driver.screen_contains("New File"));
    }
}
