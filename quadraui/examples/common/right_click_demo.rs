//! Backend-agnostic app code for the macOS right-click context-menu
//! smoke ([`macos_right_click_demo`]). Demonstrates the new
//! `Backend::show_context_menu` path (#185): on
//! `MouseDown { button: Right }` the app builds a `ContextMenu` and
//! calls `backend.show_context_menu(&menu, position)`. On macOS the
//! menu appears as a native AppKit pop-up with system fonts, accent
//! colour, Dark Mode etc. On TUI / GTK the call is a no-op today —
//! apps that want a painted right-click menu on those backends
//! continue managing their own ContextMenu state.
//!
//! Layout: a single full-window `StatusBar` shows the last activated
//! menu item. Right-click anywhere in the window to open the menu.

use quadraui::{
    AppLogic, Backend, Color, ContextMenu, ContextMenuItem, ContextMenuPlacement, Key, MouseButton,
    NamedKey, Reaction, Rect, StatusBar, StatusBarSegment, StyledText, UiEvent, WidgetId,
};

use quadraui::accelerator::{Accelerator, AcceleratorId, AcceleratorScope, KeyBinding};

pub struct RightClickDemo {
    last_action: Option<String>,
    menu_open: bool,
}

impl Default for RightClickDemo {
    fn default() -> Self {
        Self::new()
    }
}

impl RightClickDemo {
    pub fn new() -> Self {
        Self {
            last_action: None,
            menu_open: false,
        }
    }

    fn build_context_menu(&self) -> ContextMenu {
        ContextMenu {
            id: WidgetId::new("right-click"),
            items: vec![
                action_item("ctx.cut", "Cut", Some(KeyBinding::Cut)),
                action_item("ctx.copy", "Copy", Some(KeyBinding::Copy)),
                action_item("ctx.paste", "Paste", Some(KeyBinding::Paste)),
                ContextMenuItem::default(), // separator
                action_item("ctx.select_all", "Select All", Some(KeyBinding::SelectAll)),
                ContextMenuItem::default(),
                action_item("ctx.about", "About this Demo", None),
            ],
            selected_idx: 0,
            bg: None,
            placement: ContextMenuPlacement::AnchorPoint,
        }
    }

    fn status_bar(&self) -> StatusBar {
        let left = match &self.last_action {
            Some(s) => format!(" Last action: {s} "),
            None => " Right-click anywhere — q to quit ".to_string(),
        };
        let right = format!(" menu={} ", if self.menu_open { "open" } else { "closed" },);
        StatusBar {
            id: WidgetId::new("status:bar"),
            left_segments: vec![StatusBarSegment {
                text: left,
                fg: Color::rgb(255, 255, 255),
                bg: Color::rgb(40, 80, 120),
                bold: true,
                action_id: None,
            }],
            right_segments: vec![StatusBarSegment {
                text: right,
                fg: Color::rgb(220, 220, 220),
                bg: Color::rgb(40, 80, 120),
                bold: false,
                action_id: None,
            }],
        }
    }
}

fn action_item(id: &str, label: &str, binding: Option<KeyBinding>) -> ContextMenuItem {
    ContextMenuItem {
        id: Some(WidgetId::new(id)),
        label: StyledText::plain(label),
        key_equivalent: binding.map(|b| Accelerator {
            id: AcceleratorId::new(id),
            binding: b,
            scope: AcceleratorScope::Global,
            label: None,
        }),
        ..Default::default()
    }
}

impl AppLogic for RightClickDemo {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let bar = self.status_bar();
        let viewport = backend.viewport();
        let row_h = 28.0_f32;
        let rect = Rect::new(0.0, viewport.height - row_h, viewport.width, row_h);
        let _ = backend.draw_status_bar(rect, &bar, None, None);
    }

    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
        match event {
            UiEvent::MouseDown {
                button: MouseButton::Right,
                position,
                ..
            } => {
                self.menu_open = true;
                let menu = self.build_context_menu();
                // Blocks on AppKit modal popup until dismissed. Returns
                // queued events (ContextMenuItemActivated then
                // ContextMenuDismissed) drained on the next paint.
                backend.show_context_menu(&menu, position);
                Reaction::Redraw
            }
            UiEvent::ContextMenuItemActivated(id) => {
                self.last_action = Some(id.as_str().to_string());
                Reaction::Redraw
            }
            UiEvent::ContextMenuDismissed => {
                self.menu_open = false;
                Reaction::Redraw
            }
            UiEvent::KeyPressed { key, .. } => {
                if matches!(key, Key::Char('q')) || matches!(key, Key::Named(NamedKey::Escape)) {
                    return Reaction::Exit;
                }
                Reaction::Continue
            }
            _ => Reaction::Continue,
        }
    }
}
