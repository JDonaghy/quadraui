//! Backend-agnostic app code for the menu-bar example
//! ([`tui_menu_bar`] / [`gtk_menu_bar`]).
//!
//! Demonstrates [`MenuSystem`] — the high-level compose helper that
//! handles MenuBar + ContextMenu dropdown interaction (open/close,
//! arrow-key navigation, hover-to-switch, Alt+key activation, modal
//! stack coordination). The app defines menu structure and matches
//! on [`MenuEvent::Activated`].
//!
//! Controls:
//! - click menu label         open/close that menu's dropdown
//! - hover another label      switch dropdown (while one is open)
//! - hover in dropdown        highlight item
//! - click dropdown item      activate + close
//! - Alt+F/E/V                open that menu's dropdown
//! - ↑ / ↓                    navigate within dropdown
//! - ← / →                    switch to adjacent menu
//! - Enter                    activate highlighted item + close
//! - Esc                      close dropdown (or quit if none open)
//! - q                        quit (when no dropdown open)

use quadraui::{
    AppLogic, Backend, Color, ContextMenuItem, Key, MenuDef, MenuEvent, MenuSystem, NamedKey,
    Reaction, Rect, StatusBar, StatusBarSegment, StyledText, UiEvent, WidgetId,
};

pub struct MenuBarApp {
    menu_system: MenuSystem,
    last_action: Option<String>,
}

impl MenuBarApp {
    pub fn new() -> Self {
        Self {
            menu_system: MenuSystem::new(vec![
                MenuDef {
                    id: WidgetId::new("file"),
                    label: "&File".into(),
                    disabled: false,
                    items: vec![
                        action("new", "New File"),
                        action("open", "Open File"),
                        action("save", "Save"),
                        separator(),
                        action("quit", "Quit"),
                    ],
                },
                MenuDef {
                    id: WidgetId::new("edit"),
                    label: "&Edit".into(),
                    disabled: false,
                    items: vec![
                        action("undo", "Undo"),
                        action("redo", "Redo"),
                        separator(),
                        action("cut", "Cut"),
                        action("copy", "Copy"),
                        action("paste", "Paste"),
                    ],
                },
                MenuDef {
                    id: WidgetId::new("view"),
                    label: "&View".into(),
                    disabled: false,
                    items: vec![
                        action("sidebar", "Toggle Sidebar"),
                        action("terminal", "Toggle Terminal"),
                        separator(),
                        action("zoom-in", "Zoom In"),
                        action("zoom-out", "Zoom Out"),
                    ],
                },
                MenuDef {
                    id: WidgetId::new("help"),
                    label: "&Help".into(),
                    disabled: true,
                    items: vec![],
                },
            ]),
            last_action: None,
        }
    }

    fn status_bar(&self) -> StatusBar {
        let msg = match &self.last_action {
            Some(a) => format!(" last: {a} "),
            None => " click a menu or press Alt+F/E/V — q to quit ".into(),
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
                text: format!(
                    " {} ",
                    if self.menu_system.is_open() {
                        "menu open"
                    } else {
                        "menu closed"
                    }
                ),
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

        let bar_rect = Rect::new(0.0, 0.0, viewport.width, lh);
        self.menu_system.render(backend, bar_rect);

        let status_rect = Rect::new(0.0, viewport.height - lh, viewport.width, lh);
        let _ = backend.draw_status_bar(status_rect, &self.status_bar(), None, None);
    }

    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
        let viewport = backend.viewport();
        let lh = backend.line_height();
        let bar_rect = Rect::new(0.0, 0.0, viewport.width, lh);

        match self.menu_system.handle(&event, backend, bar_rect) {
            MenuEvent::Activated(id) => match id.as_str() {
                "quit" => Reaction::Exit,
                "zoom-in" => {
                    self.last_action = Some("Zoomed in (not really)".into());
                    Reaction::Redraw
                }
                "zoom-out" => {
                    self.last_action = Some("Zoomed out (not really)".into());
                    Reaction::Redraw
                }
                other => {
                    self.last_action = Some(format!("activated: {other}"));
                    Reaction::Redraw
                }
            },
            MenuEvent::StateChanged | MenuEvent::Consumed => Reaction::Redraw,
            MenuEvent::Ignored => match event {
                UiEvent::KeyPressed {
                    key: Key::Char('q'),
                    ..
                }
                | UiEvent::KeyPressed {
                    key: Key::Named(NamedKey::Escape),
                    ..
                } => Reaction::Exit,
                UiEvent::WindowResized { .. } => Reaction::Redraw,
                _ => Reaction::Continue,
            },
        }
    }
}

fn action(id: &str, label: &str) -> ContextMenuItem {
    ContextMenuItem {
        id: Some(WidgetId::new(id)),
        label: StyledText::plain(label),
        detail: None,
        disabled: false,
    }
}

fn separator() -> ContextMenuItem {
    ContextMenuItem {
        id: None,
        label: StyledText::plain(""),
        detail: None,
        disabled: false,
    }
}
