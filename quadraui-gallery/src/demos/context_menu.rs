//! `Context Menu` demo — merges
//! `quadraui/examples/common/context_menu_style_demo.rs` (the
//! `MenuStyle`/`ContextMenuController` "one call" path, issue #1187) and
//! `quadraui/examples/common/right_click_demo.rs` (the richer Cut/Copy/
//! Paste/Select All item list with key-equivalent accelerators) into one
//! widget (#1347): right-click opens the same menu either way, `m`
//! cycles [`MenuStyle`] (`Auto` → `Native` → `Custom` → `Auto`) at
//! runtime, and the status bar shows both the requested style and
//! [`Backend::effective_menu_style`] so the live resolution is visible.
//!
//! Either path delivers the same two events —
//! [`UiEvent::ContextMenuItemActivated`] / [`UiEvent::ContextMenuDismissed`]
//! — routed through one handler regardless of which path produced them.

use quadraui::{
    Accelerator, AcceleratorId, AcceleratorScope, Backend, BackendCaps, Color, ContextMenu,
    ContextMenuController, ContextMenuItem, ContextMenuOutcome, ContextMenuPlacement,
    InteractionState, Key, KeyBinding, MenuStyle, Reaction, Rect, ResolvedMenuStyle, StatusBar,
    StatusBarSegment, StyledText, UiEvent, WidgetId,
};

use crate::demo::{extract_region, Demo};

const SOURCE: &str = include_str!("context_menu.rs");

// gallery:begin
pub struct ContextMenuDemo {
    ctx_menu: ContextMenuController,
    last_action: Option<String>,
    requested_style: MenuStyle,
}

impl ContextMenuDemo {
    pub fn new() -> Self {
        Self {
            ctx_menu: ContextMenuController::new(),
            last_action: None,
            requested_style: MenuStyle::Auto,
        }
    }

    fn build_context_menu(&self) -> ContextMenu {
        ContextMenu {
            id: WidgetId::new("gallery:context-menu"),
            items: vec![
                action_item("gallery:ctx:cut", "Cut", Some(KeyBinding::Cut)),
                action_item("gallery:ctx:copy", "Copy", Some(KeyBinding::Copy)),
                action_item("gallery:ctx:paste", "Paste", Some(KeyBinding::Paste)),
                ContextMenuItem::default(), // separator
                action_item(
                    "gallery:ctx:select_all",
                    "Select All",
                    Some(KeyBinding::SelectAll),
                ),
                ContextMenuItem::default(),
                action_item("gallery:ctx:about", "About this demo", None),
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

    /// One handler for the context-menu events, reached either from the
    /// top-level `UiEvent` match (native path, queued by
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
            None => " Right-click anywhere — m cycles MenuStyle ".to_string(),
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
            id: WidgetId::new("gallery:context-menu:status"),
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

impl Default for ContextMenuDemo {
    fn default() -> Self {
        Self::new()
    }
}

impl Demo for ContextMenuDemo {
    fn name(&self) -> &'static str {
        "Context Menu"
    }

    fn group(&self) -> &'static str {
        "Overlays"
    }

    fn render(&self, _variant: usize, backend: &mut dyn Backend, area: Rect) {
        let bar = self.status_bar(backend.effective_menu_style());
        let lh = backend.line_height();
        let rect = Rect::new(area.x, area.y + area.height - lh, area.width, lh);
        let _ = backend.draw_status_bar_interactive(rect, &bar, &InteractionState::new());

        // Layer the painted context menu (if one is open) above
        // everything else, per `ContextMenuController`'s contract. A
        // no-op when the native path is in effect — the OS owns that
        // surface.
        self.ctx_menu.render(backend);
    }

    fn handle(
        &mut self,
        _variant: usize,
        event: &UiEvent,
        backend: &mut dyn Backend,
        _area: Rect,
    ) -> Reaction {
        // Route through the controller first — on the `Custom` path this
        // may return one of the same two events the `Native` path
        // delivers later via the top-level match below.
        if let ContextMenuOutcome::Event(ev) = self.ctx_menu.handle(event, backend) {
            return self.handle_menu_event(ev);
        }

        match event {
            UiEvent::MouseDown {
                button: quadraui::MouseButton::Right,
                position,
                ..
            } => {
                let menu = self.build_context_menu();
                // The one call: native on macOS under `Auto`/`Native`,
                // painted everywhere else (and on macOS under `Custom`)
                // — same call either way.
                self.ctx_menu.open(menu, *position, backend);
                Reaction::Redraw
            }
            // Native-path activation/dismissal arrives here as a
            // top-level event, queued by `Backend::show_context_menu`.
            UiEvent::ContextMenuItemActivated(_) | UiEvent::ContextMenuDismissed => {
                self.handle_menu_event(event.clone())
            }
            UiEvent::KeyPressed {
                key: Key::Char('m'),
                ..
            } => {
                self.cycle_style(backend);
                Reaction::Redraw
            }
            _ => Reaction::Continue,
        }
    }

    fn source(&self) -> &'static str {
        extract_region(SOURCE)
    }

    fn data(&self, _variant: usize) -> serde_json::Value {
        serde_json::json!({
            "requested_style": format!("{:?}", self.requested_style),
            "last_action": self.last_action,
        })
    }

    fn caps_note(&self, _variant: usize, caps: &BackendCaps) -> Option<String> {
        if caps.native_menu {
            None
        } else {
            Some(
                "This backend has no native OS menu (BackendCaps::native_menu is false) — \
                 every MenuStyle resolves to the painted ContextMenuController path shown here, \
                 even when 'Native' is requested."
                    .into(),
            )
        }
    }
}
// gallery:end

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cycling_style_wraps_auto_native_custom() {
        let mut demo = ContextMenuDemo::new();
        assert_eq!(demo.requested_style, MenuStyle::Auto);
        let mut backend = quadraui::testing::RecordingBackend::new();
        demo.cycle_style(&mut backend);
        assert_eq!(demo.requested_style, MenuStyle::Native);
        demo.cycle_style(&mut backend);
        assert_eq!(demo.requested_style, MenuStyle::Custom);
        demo.cycle_style(&mut backend);
        assert_eq!(demo.requested_style, MenuStyle::Auto);
    }

    #[test]
    fn build_context_menu_has_the_merged_item_set() {
        let demo = ContextMenuDemo::new();
        let menu = demo.build_context_menu();
        assert_eq!(menu.items.len(), 7);
    }
}
