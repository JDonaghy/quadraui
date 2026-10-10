//! `MenuBar` demo — adapted from `quadraui/examples/common/menu_bar_app.rs`
//! and the menu-bar half of `submenu_app.rs`.
//!
//! [`MenuSystem`] is the high-level compose helper that owns dropdown
//! open/close, hover-to-switch, arrow-key navigation and Alt+key
//! activation. Variant 0 is a flat dropdown; variant 1 nests a pull-right
//! submenu (`View → Export → {PNG, SVG}`) — `MenuSystem` resolves cascading
//! submenus on its own, with no extra app-side state.

use quadraui::{
    Backend, BackendCaps, Color, ContextMenuItem, InteractionState, MenuDef, MenuEvent, MenuSystem,
    Reaction, Rect, StatusBar, StatusBarSegment, StyledText, UiEvent, WidgetId,
};

use crate::demo::{extract_region, Demo};

const SOURCE: &str = include_str!("menu_bar.rs");

// gallery:begin
fn action(id: &str, label: &str) -> ContextMenuItem {
    ContextMenuItem {
        id: Some(WidgetId::new(id)),
        label: StyledText::plain(label),
        ..Default::default()
    }
}

fn separator() -> ContextMenuItem {
    ContextMenuItem::default()
}

fn flat_menus() -> Vec<MenuDef> {
    vec![
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
            items: vec![action("undo", "Undo"), action("redo", "Redo")],
        },
    ]
}

fn submenu_menus() -> Vec<MenuDef> {
    vec![MenuDef {
        id: WidgetId::new("view"),
        label: "&View".into(),
        disabled: false,
        items: vec![
            action("sidebar", "Toggle Sidebar"),
            separator(),
            ContextMenuItem {
                id: Some(WidgetId::new("export")),
                label: StyledText::plain("Export"),
                submenu: Some(vec![
                    action("export-png", "PNG"),
                    action("export-svg", "SVG"),
                ]),
                ..Default::default()
            },
        ],
    }]
}

pub struct MenuBarDemo {
    menus: [MenuSystem; 2],
    last_action: [String; 2],
}

impl MenuBarDemo {
    pub fn new() -> Self {
        Self {
            menus: [
                MenuSystem::new(flat_menus()),
                MenuSystem::new(submenu_menus()),
            ],
            last_action: [
                "click a menu or press Alt+F/E".into(),
                "click View, then hover Export".into(),
            ],
        }
    }

    fn bar_rect(area: Rect, backend: &dyn Backend) -> Rect {
        Rect::new(area.x, area.y, area.width, backend.line_height())
    }

    fn hint_bar(&self, variant: usize) -> StatusBar {
        StatusBar {
            id: WidgetId::new(format!("gallery:menu-bar:{variant}:hint")),
            left_segments: vec![StatusBarSegment {
                text: format!(" {} ", self.last_action[variant]),
                fg: Color::rgb(220, 220, 220),
                bg: Color::rgb(30, 30, 30),
                bold: false,
                action_id: None,
            }],
            right_segments: vec![],
        }
    }
}

impl Default for MenuBarDemo {
    fn default() -> Self {
        Self::new()
    }
}

impl Demo for MenuBarDemo {
    fn name(&self) -> &'static str {
        "Menu Bar"
    }

    fn group(&self) -> &'static str {
        "Chrome"
    }

    fn variants(&self) -> &'static [&'static str] {
        &["Dropdown", "Submenu cascade"]
    }

    fn render(&self, variant: usize, backend: &mut dyn Backend, area: Rect) {
        let bar_rect = Self::bar_rect(area, backend);
        self.menus[variant].render(backend, bar_rect);

        let lh = backend.line_height();
        let hint_rect = Rect::new(area.x, area.y + lh, area.width, lh);
        let _ = backend.draw_status_bar_interactive(
            hint_rect,
            &self.hint_bar(variant),
            &InteractionState::new(),
        );
    }

    fn handle(
        &mut self,
        variant: usize,
        event: &UiEvent,
        backend: &mut dyn Backend,
        area: Rect,
    ) -> Reaction {
        let bar_rect = Self::bar_rect(area, backend);
        match self.menus[variant].handle(event, backend, bar_rect) {
            MenuEvent::Activated(id) => {
                self.last_action[variant] = format!("activated: {}", id.as_str());
                Reaction::Redraw
            }
            MenuEvent::StateChanged | MenuEvent::Consumed => Reaction::Redraw,
            MenuEvent::Ignored => Reaction::Continue,
        }
    }

    fn source(&self) -> &'static str {
        extract_region(SOURCE)
    }

    fn data(&self, variant: usize) -> serde_json::Value {
        serde_json::json!({
            "menu_open": self.menus[variant].is_open(),
            "last_action": self.last_action[variant],
        })
    }

    fn caps_note(&self, _variant: usize, _caps: &BackendCaps) -> Option<String> {
        None
    }
}
// gallery:end

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hint_bar_shows_the_last_action() {
        let demo = MenuBarDemo::new();
        assert!(demo.hint_bar(0).left_segments[0].text.contains("Alt+F"));
    }
}
