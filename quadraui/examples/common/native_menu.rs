//! Backend-agnostic app code for the macOS native NSMenu smoke
//! ([`macos_native_menu`]). Demonstrates the new
//! `Backend::install_menu_bar` path (#184): the menu lives in the
//! system menu bar at the top of the screen, ⌘-shortcuts dispatch
//! through AppKit's native machinery, and checked-state toggles
//! reflect live in the menu.
//!
//! Layout: a single full-window `StatusBar` shows the last activated
//! menu item plus the current state of the two toggles. Clicking a
//! menu item or hitting its ⌘-shortcut updates the state and
//! re-installs the menu so the `✓` next to the toggle items flips.
//!
//! Runs on TUI / GTK too, but on those backends `install_menu_bar`
//! is a no-op — the example just shows a status bar saying so. The
//! visible payoff is `cargo run --example macos_native_menu --features macos`.

use quadraui::{
    AppLogic, Backend, Color, ContextMenuItem, Key, MenuBar, MenuBarItem, NamedKey, Reaction, Rect,
    StatusBar, StatusBarSegment, StyledText, UiEvent, WidgetId,
};

use quadraui::accelerator::{Accelerator, AcceleratorId, AcceleratorScope, KeyBinding};

pub struct NativeMenuApp {
    sidebar_visible: bool,
    panel_visible: bool,
    last_action: Option<String>,
}

impl Default for NativeMenuApp {
    fn default() -> Self {
        Self::new()
    }
}

impl NativeMenuApp {
    pub fn new() -> Self {
        Self {
            sidebar_visible: true,
            panel_visible: false,
            last_action: None,
        }
    }

    /// Build the declarative menu structure. Re-called whenever a
    /// toggle flips so the rendered `✓` reflects current state.
    fn build_menu(&self) -> MenuBar {
        MenuBar {
            id: WidgetId::new("native-menu"),
            items: vec![
                // ── File ──────────────────────────────────────────
                MenuBarItem {
                    id: WidgetId::new("file"),
                    label: "File".into(),
                    disabled: false,
                    submenu: Some(vec![
                        menu_item("file.new", "New", Some(KeyBinding::New)),
                        menu_item("file.open", "Open…", Some(KeyBinding::Open)),
                        menu_item("file.save", "Save", Some(KeyBinding::Save)),
                    ]),
                },
                // ── Edit ──────────────────────────────────────────
                MenuBarItem {
                    id: WidgetId::new("edit"),
                    label: "Edit".into(),
                    disabled: false,
                    submenu: Some(vec![
                        menu_item("edit.undo", "Undo", Some(KeyBinding::Undo)),
                        menu_item("edit.redo", "Redo", Some(KeyBinding::Redo)),
                        ContextMenuItem::default(), // separator
                        menu_item("edit.cut", "Cut", Some(KeyBinding::Cut)),
                        menu_item("edit.copy", "Copy", Some(KeyBinding::Copy)),
                        menu_item("edit.paste", "Paste", Some(KeyBinding::Paste)),
                    ]),
                },
                // ── View ──────────────────────────────────────────
                MenuBarItem {
                    id: WidgetId::new("view"),
                    label: "View".into(),
                    disabled: false,
                    submenu: Some(vec![
                        toggle_item(
                            "view.toggle_sidebar",
                            "Toggle Sidebar",
                            self.sidebar_visible,
                            Some(KeyBinding::Literal("Cmd+B".into())),
                        ),
                        toggle_item(
                            "view.toggle_panel",
                            "Toggle Panel",
                            self.panel_visible,
                            Some(KeyBinding::Literal("Cmd+J".into())),
                        ),
                        ContextMenuItem::default(),
                        ContextMenuItem {
                            id: Some(WidgetId::new("view.appearance")),
                            label: StyledText::plain("Appearance"),
                            submenu: Some(vec![
                                menu_item("view.zoom_in", "Zoom In", None),
                                menu_item("view.zoom_out", "Zoom Out", None),
                                menu_item("view.zoom_reset", "Reset Zoom", None),
                            ]),
                            ..Default::default()
                        },
                    ]),
                },
            ],
            open_item: None,
            focused_item: None,
        }
    }

    fn status_bar(&self) -> StatusBar {
        let left = match &self.last_action {
            Some(s) => format!(" Last action: {s} "),
            None => " Pick a menu item — File / Edit / View ".to_string(),
        };
        let right = format!(
            " sidebar={} panel={} ",
            if self.sidebar_visible { "on" } else { "off" },
            if self.panel_visible { "on" } else { "off" },
        );
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

fn menu_item(id: &str, label: &str, binding: Option<KeyBinding>) -> ContextMenuItem {
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

fn toggle_item(id: &str, label: &str, on: bool, binding: Option<KeyBinding>) -> ContextMenuItem {
    ContextMenuItem {
        id: Some(WidgetId::new(id)),
        label: StyledText::plain(label),
        checked: Some(on),
        key_equivalent: binding.map(|b| Accelerator {
            id: AcceleratorId::new(id),
            binding: b,
            scope: AcceleratorScope::Global,
            label: None,
        }),
        ..Default::default()
    }
}

impl AppLogic for NativeMenuApp {
    type AreaId = ();

    fn setup(&mut self, backend: &mut dyn Backend) {
        backend.install_menu_bar(&self.build_menu());
    }

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let bar = self.status_bar();
        let viewport = backend.viewport();
        let row_h = 28.0_f32;
        let rect = Rect::new(0.0, viewport.height - row_h, viewport.width, row_h);
        let _ = backend.draw_status_bar(rect, &bar, None, None);
    }

    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
        match event {
            UiEvent::MenuActivated(id) => {
                self.last_action = Some(id.as_str().to_string());
                match id.as_str() {
                    "view.toggle_sidebar" => {
                        self.sidebar_visible = !self.sidebar_visible;
                        // Re-install so the `✓` reflects the new state.
                        backend.install_menu_bar(&self.build_menu());
                    }
                    "view.toggle_panel" => {
                        self.panel_visible = !self.panel_visible;
                        backend.install_menu_bar(&self.build_menu());
                    }
                    _ => {}
                }
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
