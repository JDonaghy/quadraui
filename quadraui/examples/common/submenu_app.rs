//! Backend-agnostic app code for the submenu example (`tui_submenu`).
//!
//! Demonstrates pull-right cascading submenus in both:
//!
//! 1. **Menu bar** — `View → Export → {PNG, SVG}` (≥2 levels deep).
//! 2. **In-window right-click context menu** — `Refactor → {Rename, Extract}`.
//!
//! Both menus are navigable by keyboard (`Up`/`Down`/`Left`/`Right`/`Enter`/`Esc`)
//! and mouse (hover opens submenus; click activates or toggles).  The status bar
//! echoes the last-activated item id so the round-trip is visible.
//!
//! Controls:
//! - click / Alt+F/V to open a menu-bar dropdown
//! - Right-click anywhere in the body to open the context menu
//! - ↑ / ↓            navigate within the open menu level
//! - → / Enter        open a submenu (or activate a leaf)
//! - ← / Esc          close deepest submenu (Esc at root closes the whole menu)
//! - q / Esc (no menu open) → quit

use quadraui::{
    resolve_bar_height, AppLogic, AppMenu, AppMenuEvent, AppMenuState, Backend, Color, ContextMenu,
    ContextMenuController, ContextMenuItem, ContextMenuOutcome, ContextMenuPlacement,
    DrawsPrimitives, Handled, Key, MenuDef, MouseButton, NamedKey, Point, Reaction, Rect,
    StatusBar, StatusBarSegment, StyledText, UiEvent, WidgetId,
};

// ── App ───────────────────────────────────────────────────────────────────────

pub struct SubmenuApp {
    menu: AppMenu,
    menu_state: AppMenuState,
    ctx_menu: ContextMenuController,
    last_action: Option<String>,
}

impl Default for SubmenuApp {
    fn default() -> Self {
        Self::new()
    }
}

impl SubmenuApp {
    pub fn new() -> Self {
        Self {
            menu: AppMenu::new(
                WidgetId::new("menu-bar"),
                vec![
                    MenuDef {
                        id: WidgetId::new("file"),
                        label: "&File".into(),
                        disabled: false,
                        items: vec![
                            mi_action("new", "New File"),
                            mi_action("open", "Open File"),
                            mi_action("save", "Save"),
                            mi_sep(),
                            mi_action("quit", "Quit"),
                        ],
                    },
                    MenuDef {
                        id: WidgetId::new("view"),
                        label: "&View".into(),
                        disabled: false,
                        items: vec![
                            mi_action("sidebar", "Toggle Sidebar"),
                            mi_action("terminal", "Toggle Terminal"),
                            mi_sep(),
                            // Submenu depth 1: Export → PNG / SVG
                            ContextMenuItem {
                                id: Some(WidgetId::new("export")),
                                label: StyledText::plain("Export"),
                                submenu: Some(vec![
                                    // Submenu depth 2 on PNG: export-png → {lossless, compressed}
                                    ContextMenuItem {
                                        id: Some(WidgetId::new("export-png")),
                                        label: StyledText::plain("PNG"),
                                        submenu: Some(vec![
                                            mi_action("export-png-lossless", "Lossless"),
                                            mi_action("export-png-compressed", "Compressed"),
                                        ]),
                                        ..Default::default()
                                    },
                                    mi_action("export-svg", "SVG"),
                                ]),
                                ..Default::default()
                            },
                        ],
                    },
                ],
            ),
            menu_state: AppMenuState::default(),
            ctx_menu: ContextMenuController::new(),
            last_action: None,
        }
    }

    fn status_bar(&self) -> StatusBar {
        let left = match &self.last_action {
            Some(a) => format!(" activated: {a} "),
            None => " right-click body for ctx menu | Alt+F/V for menu bar | q to quit ".into(),
        };
        let right = if self.ctx_menu.is_open() {
            " ctx menu open ".into()
        } else if self.menu.is_open() {
            " menu bar open ".into()
        } else {
            " idle ".into()
        };
        StatusBar {
            id: WidgetId::new("status"),
            left_segments: vec![StatusBarSegment {
                text: left,
                fg: Color::rgb(255, 255, 255),
                bg: Color::rgb(40, 80, 120),
                bold: false,
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

    // ── Context-menu helpers ──────────────────────────────────────────

    fn ctx_menu_items() -> Vec<ContextMenuItem> {
        vec![
            mi_action("cut", "Cut"),
            mi_action("copy", "Copy"),
            mi_action("paste", "Paste"),
            mi_sep(),
            ContextMenuItem {
                id: Some(WidgetId::new("refactor")),
                label: StyledText::plain("Refactor"),
                submenu: Some(vec![
                    mi_action("rename", "Rename"),
                    mi_action("extract", "Extract"),
                ]),
                ..Default::default()
            },
        ]
    }

    fn open_ctx_menu(&mut self, backend: &mut dyn Backend, anchor: Point) {
        // Close any open menu-bar dropdown first.
        if self.menu.is_open() {
            self.menu.dismiss(backend);
        }
        let menu = ContextMenu {
            id: WidgetId::new("ctx-menu"),
            items: Self::ctx_menu_items(),
            selected_idx: 0,
            bg: None,
            placement: ContextMenuPlacement::AnchorPoint,
            glyphs: Default::default(),
        };
        self.ctx_menu.open(menu, anchor, backend);
    }

    /// Record what the context menu reported: an activated item's id, or
    /// nothing for a dismissal.
    fn on_ctx_event(&mut self, event: UiEvent) -> Reaction {
        if let UiEvent::ContextMenuItemActivated(id) = event {
            self.last_action = Some(id.as_str().to_owned());
        }
        Reaction::Redraw
    }
}

impl AppLogic for SubmenuApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let viewport = backend.viewport();
        let lh = backend.line_height();

        // Menu bar across the top row.
        let bar_rect = Rect::new(
            0.0,
            0.0,
            viewport.width,
            resolve_bar_height(backend.chrome_line_height()),
        );
        self.menu.render(backend, bar_rect);

        // In-window context menu (if open).
        self.ctx_menu.render(backend);

        // Status bar at the bottom.
        let status_rect = Rect::new(0.0, viewport.height - lh, viewport.width, lh);
        let _ = backend.draw_dyn(status_rect, &self.status_bar(), &());
    }

    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
        // Right-click anywhere → open in-window context menu.
        if let UiEvent::MouseDown {
            button: MouseButton::Right,
            position,
            ..
        } = event
        {
            self.open_ctx_menu(backend, position);
            return Reaction::Redraw;
        }

        // Context menu takes priority when open. Unhandled events fall
        // through so e.g. window-resize still triggers a redraw.
        match self.ctx_menu.handle(&event, backend) {
            ContextMenuOutcome::Event(ev) => return self.on_ctx_event(ev),
            ContextMenuOutcome::Consumed => return Reaction::Redraw,
            ContextMenuOutcome::Ignored => {}
        }
        // A native context menu reports through the event queue.
        if let UiEvent::ContextMenuItemActivated(_) | UiEvent::ContextMenuDismissed = event {
            return self.on_ctx_event(event);
        }

        // Menu bar / dropdown handling.
        let viewport = backend.viewport();
        let bar_rect = Rect::new(
            0.0,
            0.0,
            viewport.width,
            resolve_bar_height(backend.chrome_line_height()),
        );
        match self
            .menu
            .handle_ui_event(&mut self.menu_state, &event, backend, bar_rect)
        {
            Handled::Event(AppMenuEvent::Activated(id)) => {
                if id.as_str() == "quit" {
                    return Reaction::Exit;
                }
                self.last_action = Some(id.as_str().to_owned());
                return Reaction::Redraw;
            }
            Handled::Ignored => {}
            _ => return Reaction::Redraw,
        }

        // Fallback event handling.
        match event {
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
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn mi_action(id: &str, label: &str) -> ContextMenuItem {
    ContextMenuItem {
        id: Some(WidgetId::new(id)),
        label: StyledText::plain(label),
        ..Default::default()
    }
}

fn mi_sep() -> ContextMenuItem {
    ContextMenuItem::default()
}
