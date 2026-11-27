#![cfg(any(
    feature = "tui",
    feature = "gtk",
    all(feature = "macos", target_os = "macos")
))]
//! Black-box tests for a host that drives an `AppMenu` itself: what the
//! menu leaves to the host (a pointer on the strip past its labels while
//! closed, a printable key with type-ahead off) reaches the host's own
//! handling, and the painted strip underlines a label's `&` mnemonic on
//! every pixel backend that paints it in-window.

use quadraui::{
    AppLogic, AppMenu, AppMenuState, Backend, ContextMenuItem, Handled, MenuDef, Reaction, Rect,
    StyledText, UiEvent, WidgetId,
};

fn action(id: &str, label: &str) -> ContextMenuItem {
    ContextMenuItem {
        id: Some(WidgetId::new(id)),
        label: StyledText::plain(label),
        ..Default::default()
    }
}

fn menus(file_label: &str) -> Vec<MenuDef> {
    vec![
        MenuDef {
            id: WidgetId::new("file"),
            label: file_label.into(),
            disabled: false,
            items: vec![action("new", "New File"), action("save", "Save")],
        },
        MenuDef {
            id: WidgetId::new("edit"),
            label: "&Edit".into(),
            disabled: false,
            items: vec![action("undo", "Undo"), action("redo", "Redo")],
        },
    ]
}

/// A host with a menu strip across the top of its window; it records
/// every event the menu leaves unhandled.
struct HostApp {
    menu: AppMenu,
    menu_state: AppMenuState,
    to_host: Vec<UiEvent>,
}

impl HostApp {
    fn new(file_label: &str) -> Self {
        Self {
            menu: AppMenu::new(WidgetId::new("menu-bar"), menus(file_label)),
            menu_state: AppMenuState::default(),
            to_host: Vec::new(),
        }
    }

    fn bar(backend: &dyn Backend) -> Rect {
        let height = backend.chrome_line_height().max(1.0);
        Rect::new(0.0, 0.0, backend.viewport().width, height)
    }
}

impl AppLogic for HostApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let bar = Self::bar(backend);
        self.menu.render(backend, bar);
    }

    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
        let bar = Self::bar(backend);
        match self
            .menu
            .handle_ui_event(&mut self.menu_state, &event, backend, bar)
        {
            Handled::Ignored => {
                if matches!(
                    event,
                    UiEvent::MouseDown { .. } | UiEvent::KeyPressed { .. }
                ) {
                    self.to_host.push(event);
                }
                Reaction::Continue
            }
            _ => Reaction::Redraw,
        }
    }
}

#[cfg(feature = "tui")]
mod tui {
    use super::HostApp;
    use quadraui::tui::testing::TuiDriver;
    use quadraui::{Key, Modifiers, NamedKey, UiEvent};

    fn alt_f() -> UiEvent {
        UiEvent::KeyPressed {
            key: Key::Char('f'),
            modifiers: Modifiers {
                alt: true,
                ..Modifiers::default()
            },
            repeat: false,
        }
    }

    #[test]
    fn a_click_past_the_last_label_reaches_the_host_while_closed() {
        let mut driver = TuiDriver::new(HostApp::new("&File"), 60, 10);
        let (edit_x, bar_y) = driver.find("Edit").expect("Edit label painted");

        driver.click(edit_x + 20.0, bar_y);

        assert!(!driver.app().menu.is_open());
        assert!(
            matches!(driver.app().to_host.as_slice(), [UiEvent::MouseDown { .. }]),
            "the click must reach the host, got {:?}",
            driver.app().to_host
        );
    }

    #[test]
    fn a_click_on_a_label_still_opens_its_menu() {
        let mut driver = TuiDriver::new(HostApp::new("&File"), 60, 10);
        let (file_x, bar_y) = driver.find("File").expect("File label painted");

        driver.click(file_x, bar_y);

        assert!(driver.app().menu.is_open());
        assert!(driver.app().to_host.is_empty());
    }

    #[test]
    fn with_typeahead_off_a_typed_letter_reaches_the_host() {
        let mut driver = TuiDriver::new(HostApp::new("&File"), 60, 10);
        driver.app_mut().menu.typeahead = false;
        driver.dispatch(alt_f());
        assert!(driver.app().menu.is_open());

        driver.type_char('s');

        assert!(
            matches!(
                driver.app().to_host.as_slice(),
                [UiEvent::KeyPressed {
                    key: Key::Char('s'),
                    ..
                }]
            ),
            "the letter must reach the host, got {:?}",
            driver.app().to_host
        );
        driver.press_named(NamedKey::Escape);
        assert!(!driver.app().menu.is_open());
    }

    #[test]
    fn with_typeahead_on_a_typed_letter_stays_in_the_dropdown() {
        let mut driver = TuiDriver::new(HostApp::new("&File"), 60, 10);
        driver.dispatch(alt_f());

        driver.type_char('s');

        assert!(driver.app().menu.is_open());
        assert!(driver.app().to_host.is_empty());
    }
}

/// The pixels of `bounds` grown by `pad` on every side, row-major.
#[cfg(any(feature = "gtk", all(feature = "macos", target_os = "macos")))]
fn region(bounds: Rect, pad: f32) -> (i32, i32, i32, i32) {
    (
        (bounds.x - pad).floor() as i32,
        (bounds.y - pad).floor() as i32,
        (bounds.x + bounds.width + pad).ceil() as i32,
        (bounds.y + bounds.height + pad).ceil() as i32,
    )
}

#[cfg(feature = "gtk")]
mod gtk {
    use super::{region, HostApp};
    use quadraui::gtk::testing::GtkDriver;

    fn file_label_pixels(label: &str) -> Vec<(u8, u8, u8)> {
        let mut driver = GtkDriver::new(HostApp::new(label), 400, 120);
        driver.render();
        let bounds = driver.find_bounds("File").expect("File label painted");
        let (x0, y0, x1, y1) = region(bounds, 3.0);
        let mut pixels = Vec::new();
        for y in y0.max(0)..y1 {
            for x in x0.max(0)..x1 {
                pixels.push(driver.pixel(x, y));
            }
        }
        pixels
    }

    #[test]
    fn an_ampersand_label_paints_its_mnemonic_underline() {
        assert_ne!(
            file_label_pixels("&File"),
            file_label_pixels("File"),
            "\"&File\" must paint an underline that \"File\" does not"
        );
    }
}

#[cfg(all(feature = "macos", target_os = "macos"))]
mod macos {
    use super::{region, HostApp};
    use quadraui::macos::testing::MacDriver;
    use quadraui::{Backend, MenuStyle};

    fn file_label_pixels(label: &str) -> Vec<(u8, u8, u8, u8)> {
        let mut driver = MacDriver::new(HostApp::new(label), 400, 120);
        driver.parts_mut().0.set_menu_style(MenuStyle::Custom);
        driver.render();
        let bounds = driver.find_bounds("File").expect("File label painted");
        let (x0, y0, x1, y1) = region(bounds, 3.0);
        let mut pixels = Vec::new();
        for y in y0.max(0)..y1 {
            for x in x0.max(0)..x1 {
                pixels.push(driver.pixel(x as u32, y as u32));
            }
        }
        pixels
    }

    #[test]
    fn an_in_window_ampersand_label_paints_its_mnemonic_underline() {
        assert_ne!(
            file_label_pixels("&File"),
            file_label_pixels("File"),
            "\"&File\" must paint an underline that \"File\" does not"
        );
    }
}
