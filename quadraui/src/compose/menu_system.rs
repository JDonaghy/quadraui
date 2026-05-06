//! `MenuSystem` — a composed controller for MenuBar + ContextMenu
//! dropdown interaction.
//!
//! Owns the full state machine (open/close, keyboard navigation,
//! hover-to-switch, modal stack coordination) so consumers define
//! their menu structure once and match on [`MenuEvent::Activated`].
//!
//! ```ignore
//! // In handle():
//! match self.menu_system.handle(&event, backend, bar_rect) {
//!     MenuEvent::Activated(id) if id.as_str() == "save" => { /* save */ }
//!     MenuEvent::Activated(id) if id.as_str() == "quit" => return Reaction::Exit,
//!     MenuEvent::StateChanged | MenuEvent::Consumed => return Reaction::Redraw,
//!     _ => { /* handle non-menu events */ }
//! }
//! ```

use crate::backend::Backend;
use crate::event::{Rect, UiEvent};
use crate::primitives::context_menu::{
    ContextMenu, ContextMenuHit, ContextMenuItem, ContextMenuItemMeasure, ContextMenuPlacement,
};
use crate::primitives::menu_bar::{MenuBar, MenuBarHit, MenuBarItem};
use crate::types::WidgetId;
use crate::{Key, Modifiers, MouseButton, NamedKey};

/// One top-level menu and its dropdown items.
#[derive(Debug, Clone)]
pub struct MenuDef {
    pub id: WidgetId,
    pub label: String,
    pub disabled: bool,
    pub items: Vec<ContextMenuItem>,
}

/// What happened after [`MenuSystem::handle`] processed an event.
#[derive(Debug, Clone, PartialEq)]
pub enum MenuEvent {
    /// An action item was activated (clicked or Enter'd).
    Activated(WidgetId),
    /// A menu was opened or closed — the app should redraw.
    StateChanged,
    /// The event was consumed (navigation, highlight) — the app should redraw.
    Consumed,
    /// The event was not relevant to the menu system.
    Ignored,
}

pub struct MenuSystem {
    menus: Vec<MenuDef>,
    open_item: Option<usize>,
    focused_item: Option<usize>,
    dropdown_selected: usize,
    dropdown_id: WidgetId,
}

impl MenuSystem {
    pub fn new(menus: Vec<MenuDef>) -> Self {
        Self {
            menus,
            open_item: None,
            focused_item: None,
            dropdown_selected: 0,
            dropdown_id: WidgetId::new("menu-system-dropdown"),
        }
    }

    pub fn is_open(&self) -> bool {
        self.open_item.is_some()
    }

    pub fn set_menus(&mut self, menus: Vec<MenuDef>) {
        self.menus = menus;
    }

    pub fn close(&mut self, backend: &mut dyn Backend) {
        self.open_item = None;
        self.focused_item = None;
        backend.modal_stack_mut().pop(&self.dropdown_id);
    }

    /// Return the current `MenuBar` descriptor without rendering.
    pub fn menu_bar(&self) -> MenuBar {
        self.build_menu_bar()
    }

    /// Draw the menu bar and any open dropdown.
    pub fn render(&self, backend: &mut dyn Backend, bar_rect: Rect) {
        let bar = self.build_menu_bar();
        let _ = backend.draw_menu_bar(bar_rect, &bar);

        if let Some((ctx_menu, layout)) = self.dropdown_layout(backend, bar_rect) {
            let _ = backend.draw_context_menu(&ctx_menu, &layout);
        }
    }

    /// Process an event. Call from `handle()` before other UI routing.
    pub fn handle(
        &mut self,
        event: &UiEvent,
        backend: &mut dyn Backend,
        bar_rect: Rect,
    ) -> MenuEvent {
        match event {
            // ── Keyboard ──────────────────────────────────────────
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Escape),
                ..
            } if self.open_item.is_some() => {
                self.close(backend);
                MenuEvent::StateChanged
            }

            UiEvent::KeyPressed {
                key: Key::Char(c),
                modifiers: Modifiers { alt: true, .. },
                ..
            } => {
                let bar = self.build_menu_bar();
                if let Some(idx) = bar.find_alt_target(*c) {
                    if self.open_item == Some(idx) {
                        self.close(backend);
                    } else {
                        self.open_menu(idx, backend, bar_rect);
                    }
                    MenuEvent::StateChanged
                } else {
                    MenuEvent::Ignored
                }
            }

            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Down),
                ..
            } if self.open_item.is_some() => {
                self.move_selection(1);
                MenuEvent::Consumed
            }

            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Up),
                ..
            } if self.open_item.is_some() => {
                self.move_selection(-1);
                MenuEvent::Consumed
            }

            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Right),
                ..
            } if self.open_item.is_some() => {
                let next = self.next_enabled_menu(self.open_item.unwrap(), 1);
                self.close(backend);
                self.open_menu(next, backend, bar_rect);
                MenuEvent::StateChanged
            }

            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Left),
                ..
            } if self.open_item.is_some() => {
                let prev = self.next_enabled_menu(self.open_item.unwrap(), -1);
                self.close(backend);
                self.open_menu(prev, backend, bar_rect);
                MenuEvent::StateChanged
            }

            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Enter),
                ..
            } if self.open_item.is_some() => self.activate_selected(backend),

            // ── Mouse click ───────────────────────────────────────
            UiEvent::MouseDown {
                button: MouseButton::Left,
                position,
                ..
            } => {
                let bar = self.build_menu_bar();
                let bar_layout = backend.menu_bar_layout(bar_rect, &bar);

                match bar_layout.hit_test(position.x, position.y) {
                    MenuBarHit::Item(i) => {
                        if self.open_item == Some(i) {
                            self.close(backend);
                        } else {
                            self.close(backend);
                            self.open_menu(i, backend, bar_rect);
                        }
                        return MenuEvent::StateChanged;
                    }
                    MenuBarHit::Bar => {
                        if self.open_item.is_some() {
                            self.close(backend);
                            return MenuEvent::StateChanged;
                        }
                        return MenuEvent::Ignored;
                    }
                    MenuBarHit::Outside => {}
                }

                if let Some((_, ref layout)) = self.dropdown_layout(backend, bar_rect) {
                    match layout.hit_test(position.x, position.y) {
                        ContextMenuHit::Item(ref id) => {
                            let id = id.clone();
                            self.close(backend);
                            return MenuEvent::Activated(id);
                        }
                        ContextMenuHit::Inert => return MenuEvent::Consumed,
                        ContextMenuHit::Empty => {
                            self.close(backend);
                            return MenuEvent::StateChanged;
                        }
                    }
                }

                if self.open_item.is_some() {
                    self.close(backend);
                    return MenuEvent::StateChanged;
                }
                MenuEvent::Ignored
            }

            // ── Mouse hover ───────────────────────────────────────
            UiEvent::MouseMoved { position, .. } => {
                let bar = self.build_menu_bar();
                let bar_layout = backend.menu_bar_layout(bar_rect, &bar);

                if self.open_item.is_some() {
                    if let MenuBarHit::Item(i) = bar_layout.hit_test(position.x, position.y) {
                        if !bar.items[i].disabled && self.open_item != Some(i) {
                            self.close(backend);
                            self.open_menu(i, backend, bar_rect);
                            return MenuEvent::StateChanged;
                        }
                    }
                    if let Some((_, ref layout)) = self.dropdown_layout(backend, bar_rect) {
                        for vi in &layout.visible_items {
                            if vi.clickable
                                && position.x >= vi.bounds.x
                                && position.x < vi.bounds.x + vi.bounds.width
                                && position.y >= vi.bounds.y
                                && position.y < vi.bounds.y + vi.bounds.height
                            {
                                if self.dropdown_selected != vi.item_idx {
                                    self.dropdown_selected = vi.item_idx;
                                    return MenuEvent::Consumed;
                                }
                                return MenuEvent::Ignored;
                            }
                        }
                    }
                    MenuEvent::Ignored
                } else {
                    let new_focus = match bar_layout.hit_test(position.x, position.y) {
                        MenuBarHit::Item(i) if !bar.items[i].disabled => Some(i),
                        _ => None,
                    };
                    if new_focus != self.focused_item {
                        self.focused_item = new_focus;
                        MenuEvent::Consumed
                    } else {
                        MenuEvent::Ignored
                    }
                }
            }

            _ => MenuEvent::Ignored,
        }
    }

    // ── Internal helpers ──────────────────────────────────────────────

    fn build_menu_bar(&self) -> MenuBar {
        MenuBar {
            id: WidgetId::new("menu-system-bar"),
            items: self
                .menus
                .iter()
                .map(|m| MenuBarItem {
                    id: m.id.clone(),
                    label: m.label.clone(),
                    disabled: m.disabled,
                })
                .collect(),
            open_item: self.open_item,
            focused_item: self.focused_item,
        }
    }

    fn build_dropdown(&self, menu_idx: usize) -> ContextMenu {
        ContextMenu {
            id: self.dropdown_id.clone(),
            items: self.menus[menu_idx].items.clone(),
            selected_idx: self.dropdown_selected,
            bg: None,
            placement: ContextMenuPlacement::Below,
        }
    }

    fn dropdown_layout(
        &self,
        backend: &dyn Backend,
        bar_rect: Rect,
    ) -> Option<(
        ContextMenu,
        crate::primitives::context_menu::ContextMenuLayout,
    )> {
        let open_idx = self.open_item?;
        if self.menus[open_idx].items.is_empty() {
            return None;
        }
        let lh = backend.line_height();
        let bar = self.build_menu_bar();
        let bar_layout = backend.menu_bar_layout(bar_rect, &bar);
        let raw_anchor = bar_layout.visible_items[open_idx].bounds;
        let pad = (lh * 0.15).max(1.0);
        let anchor = Rect::new(
            raw_anchor.x + pad,
            raw_anchor.y,
            raw_anchor.width,
            raw_anchor.height + pad,
        );
        let viewport = backend.viewport();
        let viewport_rect = Rect::new(0.0, 0.0, viewport.width, viewport.height);
        let menu_width = 20.0 * lh;
        let ctx_menu = self.build_dropdown(open_idx);
        let item_h = (lh * 1.4).round().max(lh);
        let sep_h = (lh * 0.5).round().max(1.0);
        let layout = ctx_menu.layout_at(anchor, viewport_rect, menu_width, |i| {
            if ctx_menu.items[i].is_separator() {
                ContextMenuItemMeasure::new(sep_h)
            } else {
                ContextMenuItemMeasure::new(item_h)
            }
        });
        Some((ctx_menu, layout))
    }

    fn open_menu(&mut self, idx: usize, backend: &mut dyn Backend, bar_rect: Rect) {
        self.open_item = Some(idx);
        self.focused_item = Some(idx);
        self.dropdown_selected = self.build_dropdown(idx).first_selectable();
        if let Some((_, layout)) = self.dropdown_layout(backend, bar_rect) {
            backend
                .modal_stack_mut()
                .push(self.dropdown_id.clone(), layout.bounds);
        }
    }

    fn next_enabled_menu(&self, from: usize, delta: isize) -> usize {
        let n = self.menus.len() as isize;
        let mut idx = from as isize;
        for _ in 0..self.menus.len() {
            idx = (idx + delta).rem_euclid(n);
            if !self.menus[idx as usize].disabled {
                return idx as usize;
            }
        }
        from
    }

    fn move_selection(&mut self, delta: i32) {
        let Some(open_idx) = self.open_item else {
            return;
        };
        let dropdown = self.build_dropdown(open_idx);
        self.dropdown_selected = dropdown.move_selection(self.dropdown_selected, delta);
    }

    fn activate_selected(&mut self, backend: &mut dyn Backend) -> MenuEvent {
        let Some(open_idx) = self.open_item else {
            return MenuEvent::Ignored;
        };
        let items = &self.menus[open_idx].items;
        let result = if let Some(item) = items.get(self.dropdown_selected) {
            if let Some(ref id) = item.id {
                MenuEvent::Activated(id.clone())
            } else {
                MenuEvent::Consumed
            }
        } else {
            MenuEvent::Consumed
        };
        self.close(backend);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::context_menu::ContextMenuItem;
    use crate::types::StyledText;

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
            label: StyledText::default(),
            detail: None,
            disabled: false,
        }
    }

    fn sample_menus() -> Vec<MenuDef> {
        vec![
            MenuDef {
                id: WidgetId::new("file"),
                label: "&File".into(),
                disabled: false,
                items: vec![
                    action("new", "New File"),
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
            MenuDef {
                id: WidgetId::new("help"),
                label: "&Help".into(),
                disabled: true,
                items: vec![],
            },
        ]
    }

    #[test]
    fn new_menu_system_starts_closed() {
        let ms = MenuSystem::new(sample_menus());
        assert!(!ms.is_open());
        assert_eq!(ms.open_item, None);
        assert_eq!(ms.focused_item, None);
    }

    #[test]
    fn set_menus_replaces_definitions() {
        let mut ms = MenuSystem::new(sample_menus());
        assert_eq!(ms.menus.len(), 3);
        ms.set_menus(vec![MenuDef {
            id: WidgetId::new("only"),
            label: "Only".into(),
            disabled: false,
            items: vec![action("a", "A")],
        }]);
        assert_eq!(ms.menus.len(), 1);
    }

    #[test]
    fn next_enabled_menu_skips_disabled() {
        let ms = MenuSystem::new(sample_menus());
        // help (idx 2) is disabled, so from edit (1) forward wraps to file (0)
        assert_eq!(ms.next_enabled_menu(1, 1), 0);
        // from file (0) backward wraps past help to edit (1)
        assert_eq!(ms.next_enabled_menu(0, -1), 1);
    }

    #[test]
    fn build_menu_bar_reflects_state() {
        let mut ms = MenuSystem::new(sample_menus());
        ms.open_item = Some(1);
        ms.focused_item = Some(1);
        let bar = ms.build_menu_bar();
        assert_eq!(bar.items.len(), 3);
        assert_eq!(bar.open_item, Some(1));
        assert_eq!(bar.focused_item, Some(1));
        assert!(bar.items[2].disabled);
    }

    #[test]
    fn build_dropdown_uses_selected_idx() {
        let mut ms = MenuSystem::new(sample_menus());
        ms.dropdown_selected = 2;
        let dd = ms.build_dropdown(0);
        assert_eq!(dd.selected_idx, 2);
        assert_eq!(dd.items.len(), 4);
    }
}
