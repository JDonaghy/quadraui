//! Backend-agnostic app code for the `MenuStyle` demo (issue #1187).
//!
//! Demonstrates the framework-level `MenuStyle { Auto, Native, Custom }`
//! setting + the single [`ContextMenuController::open`] call that routes
//! to native or painted without the app branching on backend:
//!
//! - Right-click anywhere to open a context menu. On macOS under `Auto`
//!   (the default) it's a real `NSMenu`; on every other backend (and on
//!   macOS under `Custom`) it's painted in-window via
//!   [`ContextMenuController`] — same call either way.
//! - Press `m` to cycle [`MenuStyle`] (`Auto` → `Native` → `Custom` →
//!   `Auto`) at runtime via `Backend::set_menu_style`. The status bar
//!   shows both the requested style and [`Backend::effective_menu_style`]
//!   so a test (or a human) can see the live resolution.
//!
//! Either path delivers the same two events —
//! [`UiEvent::ContextMenuItemActivated`] / [`UiEvent::ContextMenuDismissed`]
//! — routed through one handler (`handle_menu_event`) regardless of which
//! path produced them.

use quadraui::{
    AppLogic, Backend, Color, ContextMenu, ContextMenuController, ContextMenuItem,
    ContextMenuOutcome, ContextMenuPlacement, InteractionState, Key, MenuStyle, MouseButton,
    NamedKey, Reaction, Rect, ResolvedMenuStyle, StatusBar, StatusBarSegment, StyledText, UiEvent,
    WidgetId,
};

pub struct ContextMenuStyleDemo {
    ctx_menu: ContextMenuController,
    last_action: Option<String>,
    requested_style: MenuStyle,
}

impl Default for ContextMenuStyleDemo {
    fn default() -> Self {
        Self::new()
    }
}

impl ContextMenuStyleDemo {
    pub fn new() -> Self {
        Self {
            ctx_menu: ContextMenuController::new(),
            last_action: None,
            requested_style: MenuStyle::Auto,
        }
    }

    fn build_context_menu(&self) -> ContextMenu {
        ContextMenu {
            id: WidgetId::new("menu-style-demo"),
            items: vec![
                action_item("ctx.cut", "Cut"),
                action_item("ctx.copy", "Copy"),
                action_item("ctx.paste", "Paste"),
                ContextMenuItem::default(), // separator
                action_item("ctx.about", "About this demo"),
            ],
            selected_idx: 0,
            bg: None,
            placement: ContextMenuPlacement::AnchorPoint,
        }
    }

    fn cycle_style(&mut self, backend: &mut dyn Backend) {
        self.requested_style = match self.requested_style {
            MenuStyle::Auto => MenuStyle::Native,
            MenuStyle::Native => MenuStyle::Custom,
            MenuStyle::Custom => MenuStyle::Auto,
        };
        backend.set_menu_style(self.requested_style);
    }

    /// One handler for the terminal context-menu events, reached either
    /// from the top-level `UiEvent` match (native path, queued by
    /// `Backend::show_context_menu`) or from `ContextMenuOutcome::Event`
    /// (painted path, returned synchronously by `ContextMenuController::
    /// handle`) — the "one code path" issue #1187 asks for.
    fn handle_menu_event(&mut self, event: UiEvent) -> Reaction {
        match event {
            UiEvent::ContextMenuItemActivated(id) => {
                self.last_action = Some(id.as_str().to_string());
                Reaction::Redraw
            }
            UiEvent::ContextMenuDismissed => Reaction::Redraw,
            _ => Reaction::Continue,
        }
    }

    fn status_bar(&self, effective: ResolvedMenuStyle) -> StatusBar {
        let left = match &self.last_action {
            Some(s) => format!(" Last action: {s} "),
            None => " Right-click anywhere — m cycles MenuStyle, q quits ".to_string(),
        };
        let style_name = match self.requested_style {
            MenuStyle::Auto => "Auto",
            MenuStyle::Native => "Native",
            MenuStyle::Custom => "Custom",
        };
        let resolved_name = match effective {
            ResolvedMenuStyle::Native => "native",
            ResolvedMenuStyle::Custom => "custom",
        };
        let right = format!(" style={style_name} effective={resolved_name} ");
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

fn action_item(id: &str, label: &str) -> ContextMenuItem {
    ContextMenuItem {
        id: Some(WidgetId::new(id)),
        label: StyledText::plain(label),
        ..Default::default()
    }
}

impl AppLogic for ContextMenuStyleDemo {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let bar = self.status_bar(backend.effective_menu_style());
        let viewport = backend.viewport();
        let row_h = backend.line_height();
        let rect = Rect::new(0.0, viewport.height - row_h, viewport.width, row_h);
        let _ = backend.draw_status_bar_interactive(rect, &bar, &InteractionState::new());

        // Layer the painted context menu (if one is open) above
        // everything else, per `ContextMenuController`'s contract. A
        // no-op when the native path is in effect — AppKit owns that
        // surface.
        self.ctx_menu.render(backend);
    }

    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
        // Route through the controller first — on the `Custom` path
        // this may return one of the same two events the `Native` path
        // delivers later via the top-level match below.
        if let ContextMenuOutcome::Event(ev) = self.ctx_menu.handle(&event, backend) {
            return self.handle_menu_event(ev);
        }

        match event {
            UiEvent::MouseDown {
                button: MouseButton::Right,
                position,
                ..
            } => {
                let menu = self.build_context_menu();
                // The one call: native on macOS under `Auto`/`Native`,
                // painted everywhere else (and on macOS under
                // `Custom`) — same call either way.
                self.ctx_menu.open(menu, position, backend);
                Reaction::Redraw
            }
            // Native-path activation/dismissal arrives here as a
            // top-level event, queued by `Backend::show_context_menu`.
            UiEvent::ContextMenuItemActivated(_) | UiEvent::ContextMenuDismissed => {
                self.handle_menu_event(event)
            }
            UiEvent::KeyPressed { key, .. } => match key {
                Key::Char('m') => {
                    self.cycle_style(backend);
                    Reaction::Redraw
                }
                Key::Char('q') | Key::Named(NamedKey::Escape) => Reaction::Exit,
                _ => Reaction::Continue,
            },
            _ => Reaction::Continue,
        }
    }
}
