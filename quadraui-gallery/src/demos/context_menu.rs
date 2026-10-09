//! `Context Menu` demo — merges
//! `quadraui/examples/common/context_menu_style_demo.rs` (the
//! `MenuStyle`/`ContextMenuController` "one call" path) and
//! `quadraui/examples/common/right_click_demo.rs` (the richer Cut/Copy/
//! Paste/Select All item list with key-equivalent accelerators) into one
//! widget: right-click opens the same menu either way, and three
//! variants pick the requested [`MenuStyle`] — `Auto`, `Native`,
//! `Custom` — so the status bar's live
//! [`Backend::effective_menu_style`] reading shows how each resolves on
//! the running backend.
//!
//! Either path delivers the same two events —
//! [`UiEvent::ContextMenuItemActivated`] / [`UiEvent::ContextMenuDismissed`]
//! — routed through one handler regardless of which path produced them.

use quadraui::{
    Accelerator, AcceleratorId, AcceleratorScope, Backend, BackendCaps, Color, ContextMenu,
    ContextMenuController, ContextMenuItem, ContextMenuOutcome, ContextMenuPlacement,
    InteractionState, KeyBinding, MenuStyle, Reaction, Rect, ResolvedMenuStyle, StatusBar,
    StatusBarSegment, StyledText, UiEvent, WidgetId,
};

use crate::demo::{extract_region, Demo};

const SOURCE: &str = include_str!("context_menu.rs");

// gallery:begin
pub struct ContextMenuDemo {
    ctx_menu: ContextMenuController,
    last_action: Option<String>,
}

impl ContextMenuDemo {
    pub fn new() -> Self {
        Self {
            ctx_menu: ContextMenuController::new(),
            last_action: None,
        }
    }

    /// The [`MenuStyle`] each variant requests.
    fn requested_style(variant: usize) -> MenuStyle {
        match variant {
            0 => MenuStyle::Auto,
            1 => MenuStyle::Native,
            _ => MenuStyle::Custom,
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

    /// One handler for the context-menu events, reached either from the
    /// top-level `UiEvent` match (native path, queued by
    /// `Backend::show_context_menu`) or from `ContextMenuOutcome::Event`
    /// (painted path, returned synchronously by `ContextMenuController::
    /// handle`) — one code path regardless of which path produced the
    /// event.
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

    fn status_bar(&self, variant: usize, effective: ResolvedMenuStyle) -> StatusBar {
        let left = match &self.last_action {
            Some(s) => format!(" Last action: {s} "),
            None => " Right-click anywhere ".to_string(),
        };
        let style_name = match Self::requested_style(variant) {
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

    fn variants(&self) -> &'static [&'static str] {
        &["Auto", "Native", "Custom"]
    }

    fn render(&self, variant: usize, backend: &mut dyn Backend, area: Rect) {
        // Reasserted every frame so the active variant's requested style
        // is always the one in effect while this demo is showing —
        // switching the variant picker is the one way to change it.
        backend.set_menu_style(Self::requested_style(variant));
        let bar = self.status_bar(variant, backend.effective_menu_style());
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
        match self.ctx_menu.handle(event, backend) {
            ContextMenuOutcome::Event(ev) => return self.handle_menu_event(ev),
            // Navigation/hit-testing inside the open menu (e.g. arrow
            // keys moving the selection) — nothing to route, but the
            // new selection needs painting.
            ContextMenuOutcome::Consumed => return Reaction::Redraw,
            ContextMenuOutcome::Ignored => {}
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
            _ => Reaction::Continue,
        }
    }

    fn source(&self) -> &'static str {
        extract_region(SOURCE)
    }

    fn data(&self, variant: usize) -> serde_json::Value {
        serde_json::json!({
            "requested_style": format!("{:?}", Self::requested_style(variant)),
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
    fn requested_style_matches_each_variant() {
        assert_eq!(ContextMenuDemo::requested_style(0), MenuStyle::Auto);
        assert_eq!(ContextMenuDemo::requested_style(1), MenuStyle::Native);
        assert_eq!(ContextMenuDemo::requested_style(2), MenuStyle::Custom);
    }

    #[test]
    fn build_context_menu_has_the_merged_item_set() {
        let demo = ContextMenuDemo::new();
        let menu = demo.build_context_menu();
        let ids: Vec<Option<String>> = menu
            .items
            .iter()
            .map(|i| i.id.as_ref().map(|id| id.as_str().to_string()))
            .collect();
        assert_eq!(
            ids,
            vec![
                Some("gallery:ctx:cut".to_string()),
                Some("gallery:ctx:copy".to_string()),
                Some("gallery:ctx:paste".to_string()),
                None, // separator
                Some("gallery:ctx:select_all".to_string()),
                None, // separator
                Some("gallery:ctx:about".to_string()),
            ]
        );
    }
}
