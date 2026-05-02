//! Backend-agnostic app code for the menu-bar example
//! ([`tui_menu_bar`] / [`gtk_menu_bar`]).
//!
//! [`MenuBarApp`] demonstrates a [`MenuBar`] at the top of the window
//! with a [`StatusBar`] at the bottom showing the last activated item.
//! The same `AppLogic` impl drives both backends — the only difference
//! between `tui_menu_bar.rs` and `gtk_menu_bar.rs` is the runner call.

use quadraui::{
    AppLogic, Backend, Color, Key, MenuBar, MenuBarHit, MenuBarItem, Modifiers, NamedKey, Reaction,
    Rect, StatusBar, StatusBarSegment, UiEvent, WidgetId,
};

pub struct MenuBarApp {
    last_activated: Option<usize>,
    open_item: Option<usize>,
}

impl MenuBarApp {
    pub fn new() -> Self {
        Self {
            last_activated: None,
            open_item: None,
        }
    }

    fn menu_bar(&self) -> MenuBar {
        MenuBar {
            id: WidgetId::new("menu-bar"),
            items: vec![
                MenuBarItem {
                    id: WidgetId::new("file"),
                    label: "&File".into(),
                    disabled: false,
                },
                MenuBarItem {
                    id: WidgetId::new("edit"),
                    label: "&Edit".into(),
                    disabled: false,
                },
                MenuBarItem {
                    id: WidgetId::new("view"),
                    label: "&View".into(),
                    disabled: false,
                },
                MenuBarItem {
                    id: WidgetId::new("help"),
                    label: "&Help".into(),
                    disabled: true,
                },
            ],
            open_item: self.open_item,
            focused_item: None,
        }
    }

    fn status_bar(&self) -> StatusBar {
        let bar = self.menu_bar();
        let msg = match self.last_activated {
            Some(i) => format!(" last: {} ", bar.items[i].id.as_str()),
            None => " click a menu item or press Alt+key — q to quit ".into(),
        };
        let open = match self.open_item {
            Some(i) => format!(" open: {} ", bar.items[i].id.as_str()),
            None => " open: <none> ".into(),
        };
        StatusBar {
            id: WidgetId::new("status"),
            left_segments: vec![StatusBarSegment {
                text: msg,
                fg: Color::rgb(255, 255, 255),
                bg: Color::rgb(40, 80, 120),
                bold: false,
                action_id: None,
            }],
            right_segments: vec![StatusBarSegment {
                text: open,
                fg: Color::rgb(220, 220, 220),
                bg: Color::rgb(40, 80, 120),
                bold: false,
                action_id: None,
            }],
        }
    }
}

impl Default for MenuBarApp {
    fn default() -> Self {
        Self::new()
    }
}

impl AppLogic for MenuBarApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let viewport = backend.viewport();
        let lh = backend.line_height();

        let menu_rect = Rect::new(0.0, 0.0, viewport.width, lh);
        let bar = self.menu_bar();
        let _ = backend.draw_menu_bar(menu_rect, &bar);

        let status_rect = Rect::new(0.0, viewport.height - lh, viewport.width, lh);
        let status = self.status_bar();
        let _ = backend.draw_status_bar(status_rect, &status);
    }

    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
        match event {
            UiEvent::KeyPressed {
                key: Key::Char('q'),
                ..
            }
            | UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Escape),
                ..
            } => Reaction::Exit,
            UiEvent::KeyPressed {
                key: Key::Char(c),
                modifiers: Modifiers { alt: true, .. },
                ..
            } => {
                let bar = self.menu_bar();
                if let Some(idx) = bar.find_alt_target(c) {
                    self.last_activated = Some(idx);
                    self.open_item = Some(idx);
                }
                Reaction::Redraw
            }
            UiEvent::MouseDown { position, .. } => {
                let viewport = backend.viewport();
                let lh = backend.line_height();
                let menu_rect = Rect::new(0.0, 0.0, viewport.width, lh);
                let bar = self.menu_bar();
                let layout = backend.menu_bar_layout(menu_rect, &bar);
                match layout.hit_test(position.x, position.y) {
                    MenuBarHit::Item(i) => {
                        self.last_activated = Some(i);
                        self.open_item = Some(i);
                    }
                    MenuBarHit::Bar | MenuBarHit::Outside => {
                        self.open_item = None;
                    }
                }
                Reaction::Redraw
            }
            UiEvent::WindowResized { .. } => Reaction::Redraw,
            _ => Reaction::Continue,
        }
    }
}
