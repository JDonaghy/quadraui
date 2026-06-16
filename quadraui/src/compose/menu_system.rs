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
    ContextMenu, ContextMenuHit, ContextMenuItem, ContextMenuItemMeasure, ContextMenuLayout,
    ContextMenuPlacement,
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
    /// Selected item index in the root dropdown.
    dropdown_selected: usize,
    dropdown_id: WidgetId,
    /// Depth-first path of open submenus within the current dropdown.
    /// `submenu_path[d]` = `item_idx` in the menu at depth `d` whose child
    /// submenu is open.  Empty ⟹ no submenus open.
    submenu_path: Vec<usize>,
    /// Selected item index at each submenu depth.
    /// `submenu_selected[d]` corresponds to the submenu opened by
    /// `submenu_path[d]`.
    submenu_selected: Vec<usize>,
}

impl MenuSystem {
    pub fn new(menus: Vec<MenuDef>) -> Self {
        Self {
            menus,
            open_item: None,
            focused_item: None,
            dropdown_selected: 0,
            dropdown_id: WidgetId::new("menu-system-dropdown"),
            submenu_path: Vec::new(),
            submenu_selected: Vec::new(),
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
        self.submenu_path.clear();
        self.submenu_selected.clear();
        backend.modal_stack_mut().pop(&self.dropdown_id);
    }

    /// Return the current `MenuBar` descriptor without rendering.
    pub fn menu_bar(&self) -> MenuBar {
        self.build_menu_bar()
    }

    /// Draw the menu bar and any open dropdown (including nested submenus).
    pub fn render(&self, backend: &mut dyn Backend, bar_rect: Rect) {
        let bar = self.build_menu_bar();
        let _ = backend.draw_menu_bar(bar_rect, &bar);

        let stack = self.dropdown_stack(backend, bar_rect);
        for (ctx_menu, layout) in &stack {
            let _ = backend.draw_context_menu(ctx_menu, layout);
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

            // Esc: close deepest open submenu; if none, close whole menu.
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Escape),
                ..
            } if self.open_item.is_some() => {
                if !self.submenu_path.is_empty() {
                    self.close_deepest_submenu();
                    MenuEvent::StateChanged
                } else {
                    self.close(backend);
                    MenuEvent::StateChanged
                }
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

            // Right: open submenu if selected item has one; otherwise switch
            // top-level menu (only at root level with no open submenus).
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Right),
                ..
            } if self.open_item.is_some() => {
                let depth = self.submenu_path.len();
                let sel = self.current_selected();
                let has_sub = self
                    .items_at_depth(depth)
                    .and_then(|items| items.into_iter().nth(sel))
                    .map(|item| item.submenu.is_some())
                    .unwrap_or(false);

                if has_sub {
                    self.open_submenu(sel);
                    MenuEvent::StateChanged
                } else if depth == 0 {
                    // At root with no submenu open → switch top-level menu.
                    let next = self.next_enabled_menu(self.open_item.unwrap(), 1);
                    self.close(backend);
                    self.open_menu(next, backend, bar_rect);
                    MenuEvent::StateChanged
                } else {
                    // Inside a submenu, non-submenu item → no-op.
                    MenuEvent::Ignored
                }
            }

            // Left: close deepest submenu if one is open; otherwise switch
            // top-level menu (preserves pre-submenu behaviour).
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Left),
                ..
            } if self.open_item.is_some() => {
                if !self.submenu_path.is_empty() {
                    self.close_deepest_submenu();
                    MenuEvent::StateChanged
                } else {
                    let prev = self.next_enabled_menu(self.open_item.unwrap(), -1);
                    self.close(backend);
                    self.open_menu(prev, backend, bar_rect);
                    MenuEvent::StateChanged
                }
            }

            // Enter: open submenu if item is a submenu parent; otherwise activate.
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Enter),
                ..
            } if self.open_item.is_some() => {
                let depth = self.submenu_path.len();
                let sel = self.current_selected();
                let item_opt = self
                    .items_at_depth(depth)
                    .and_then(|items| items.into_iter().nth(sel));

                match item_opt {
                    Some(item) if item.submenu.is_some() => {
                        self.open_submenu(sel);
                        MenuEvent::StateChanged
                    }
                    Some(item) => {
                        if let Some(id) = item.id {
                            self.close(backend);
                            MenuEvent::Activated(id)
                        } else {
                            self.close(backend);
                            MenuEvent::Consumed
                        }
                    }
                    None => {
                        self.close(backend);
                        MenuEvent::Consumed
                    }
                }
            }

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

                if self.open_item.is_some() {
                    // Walk the submenu stack deepest-first so that a click on
                    // an item in a child popup doesn't fall through to the parent.
                    let stack = self.dropdown_stack(backend, bar_rect);
                    for (depth_idx, (ref menu, ref layout)) in stack.iter().enumerate().rev() {
                        match layout.hit_test(position.x, position.y) {
                            ContextMenuHit::Item(ref id) => {
                                // Resolve item_idx from the id.
                                let item_idx_opt = layout
                                    .visible_items
                                    .iter()
                                    .find(|v| {
                                        v.clickable
                                            && menu.items[v.item_idx].id.as_ref() == Some(id)
                                    })
                                    .map(|v| v.item_idx);

                                if let Some(item_idx) = item_idx_opt {
                                    if menu.items[item_idx].submenu.is_some() {
                                        // Toggle: close any deeper submenus,
                                        // then open/close this one.
                                        self.submenu_path.truncate(depth_idx);
                                        self.submenu_selected.truncate(depth_idx);
                                        self.open_submenu(item_idx);
                                        return MenuEvent::StateChanged;
                                    } else {
                                        let id = id.clone();
                                        self.close(backend);
                                        return MenuEvent::Activated(id);
                                    }
                                }
                                return MenuEvent::Consumed;
                            }
                            ContextMenuHit::Inert => return MenuEvent::Consumed,
                            ContextMenuHit::Empty => {
                                // Click outside this depth — try shallower.
                                continue;
                            }
                        }
                    }

                    // Click outside all open menu levels → close.
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
                    // Hovering a different top-level menu label → switch.
                    if let MenuBarHit::Item(i) = bar_layout.hit_test(position.x, position.y) {
                        if !bar.items[i].disabled && self.open_item != Some(i) {
                            self.close(backend);
                            self.open_menu(i, backend, bar_rect);
                            return MenuEvent::StateChanged;
                        }
                    }

                    // Walk the stack shallowest-first to find the deepest level
                    // that the cursor is inside. When we find an item:
                    //   • Update the selection at that level.
                    //   • If it's a submenu parent, open its child (closing any
                    //     previously-open sibling submenu at the same depth).
                    //   • Close any submenus deeper than the matched level.
                    let stack = self.dropdown_stack(backend, bar_rect);
                    for (depth_idx, (ref menu, ref layout)) in stack.iter().enumerate() {
                        for vis in &layout.visible_items {
                            if !vis.clickable {
                                continue;
                            }
                            if position.x >= vis.bounds.x
                                && position.x < vis.bounds.x + vis.bounds.width
                                && position.y >= vis.bounds.y
                                && position.y < vis.bounds.y + vis.bounds.height
                            {
                                let item_idx = vis.item_idx;
                                let has_sub = menu.items[item_idx].submenu.is_some();

                                // Update selection at this depth.
                                let sel_changed = if depth_idx == 0 {
                                    let old = self.dropdown_selected;
                                    self.dropdown_selected = item_idx;
                                    old != item_idx
                                } else {
                                    let sub_d = depth_idx - 1;
                                    let old =
                                        self.submenu_selected.get(sub_d).copied().unwrap_or(0);
                                    if let Some(s) = self.submenu_selected.get_mut(sub_d) {
                                        *s = item_idx;
                                    }
                                    old != item_idx
                                };

                                if has_sub {
                                    // Trim any deeper submenus and open this one.
                                    self.submenu_path.truncate(depth_idx);
                                    self.submenu_selected.truncate(depth_idx);
                                    self.open_submenu(item_idx);
                                    return MenuEvent::StateChanged;
                                } else if self.submenu_path.len() > depth_idx {
                                    // Close deeper submenus when hovering a leaf.
                                    self.submenu_path.truncate(depth_idx);
                                    self.submenu_selected.truncate(depth_idx);
                                    return MenuEvent::StateChanged;
                                } else if sel_changed {
                                    return MenuEvent::Consumed;
                                } else {
                                    return MenuEvent::Ignored;
                                }
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
                    // MenuSystem manages dropdowns out-of-band via
                    // build_dropdown(), not through the declarative submenu
                    // field. Leaving this `None` keeps the existing render
                    // path unchanged on TUI/GTK.
                    submenu: None,
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
    ) -> Option<(ContextMenu, ContextMenuLayout)> {
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

    /// Build the full stack of (ContextMenu, ContextMenuLayout) for every
    /// currently-open menu level: index 0 = root dropdown, index 1 = first
    /// open submenu, etc.  Returns an empty vec when no menu is open.
    fn dropdown_stack(
        &self,
        backend: &dyn Backend,
        bar_rect: Rect,
    ) -> Vec<(ContextMenu, ContextMenuLayout)> {
        let mut stack: Vec<(ContextMenu, ContextMenuLayout)> = Vec::new();

        let Some((root_menu, root_layout)) = self.dropdown_layout(backend, bar_rect) else {
            return stack;
        };
        stack.push((root_menu, root_layout));

        let lh = backend.line_height();
        let item_h = (lh * 1.4).round().max(lh);
        let sep_h = (lh * 0.5).round().max(1.0);
        let menu_width = 20.0 * lh;
        let viewport = backend.viewport();
        let vp = Rect::new(0.0, 0.0, viewport.width, viewport.height);

        for (depth, &path_idx) in self.submenu_path.iter().enumerate() {
            // Extract parent data before we push so the borrow checker is happy.
            let sub_items_opt: Option<Vec<ContextMenuItem>> = stack[depth]
                .0
                .items
                .get(path_idx)
                .and_then(|item| item.submenu.clone());
            let parent_bounds = stack[depth].1.bounds;
            let anchor_y = stack[depth]
                .1
                .visible_items
                .iter()
                .find(|v| v.item_idx == path_idx)
                .map(|v| v.bounds.y)
                .unwrap_or(parent_bounds.y);

            let Some(sub_items) = sub_items_opt else {
                break;
            };

            // Pull-right anchor with left-flip on overflow.
            let preferred_x = parent_bounds.x + parent_bounds.width + 1.0;
            let flipped_x = parent_bounds.x - menu_width - 1.0;
            let anchor_x = if preferred_x + menu_width <= vp.x + vp.width {
                preferred_x
            } else if flipped_x >= vp.x {
                flipped_x
            } else {
                (vp.x + vp.width - menu_width).max(vp.x)
            };

            let selected = self.submenu_selected.get(depth).copied().unwrap_or(0);

            let sub_menu = ContextMenu {
                id: WidgetId::new("menu-system-submenu"),
                items: sub_items.clone(),
                selected_idx: selected,
                bg: None,
                placement: ContextMenuPlacement::AnchorPoint,
            };

            let sub_layout = sub_menu.layout(anchor_x, anchor_y, vp, menu_width, |i| {
                if sub_items[i].is_separator() {
                    ContextMenuItemMeasure::new(sep_h)
                } else {
                    ContextMenuItemMeasure::new(item_h)
                }
            });

            stack.push((sub_menu, sub_layout));
        }

        stack
    }

    fn open_menu(&mut self, idx: usize, backend: &mut dyn Backend, bar_rect: Rect) {
        self.open_item = Some(idx);
        self.focused_item = Some(idx);
        self.submenu_path.clear();
        self.submenu_selected.clear();
        self.dropdown_selected = self.build_dropdown(idx).first_selectable();
        if let Some((_, layout)) = self.dropdown_layout(backend, bar_rect) {
            backend
                .modal_stack_mut()
                .push(self.dropdown_id.clone(), layout.bounds);
        }
    }

    /// Open the submenu belonging to `item_idx` at the current deepest level.
    /// Sets the initial selection to the first selectable item.
    fn open_submenu(&mut self, item_idx: usize) {
        let depth = self.submenu_path.len();
        let sub_items = match self
            .items_at_depth(depth)
            .and_then(|items| items.into_iter().nth(item_idx))
            .and_then(|item| item.submenu)
        {
            Some(items) => items,
            None => return,
        };
        let first_sel = sub_items
            .iter()
            .position(|i| !i.is_separator() && !i.disabled)
            .unwrap_or(0);
        self.submenu_path.push(item_idx);
        self.submenu_selected.push(first_sel);
    }

    /// Close the deepest open submenu (pop one level).
    fn close_deepest_submenu(&mut self) {
        self.submenu_path.pop();
        self.submenu_selected.pop();
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

    /// Return the selected item index at the currently-deepest open level.
    fn current_selected(&self) -> usize {
        if self.submenu_path.is_empty() {
            self.dropdown_selected
        } else {
            *self.submenu_selected.last().unwrap_or(&0)
        }
    }

    /// Return the items at depth `depth` (0 = root dropdown items).
    /// Returns `None` when `open_item` is unset or a submenu is missing.
    fn items_at_depth(&self, depth: usize) -> Option<Vec<ContextMenuItem>> {
        let open_idx = self.open_item?;
        let mut items = self.menus[open_idx].items.clone();
        for &path_idx in self.submenu_path.iter().take(depth) {
            items = items.into_iter().nth(path_idx)?.submenu?;
        }
        Some(items)
    }

    fn move_selection(&mut self, delta: i32) {
        let depth = self.submenu_path.len();
        let current = self.current_selected();

        let Some(items) = self.items_at_depth(depth) else {
            return;
        };

        let temp = ContextMenu {
            id: WidgetId::new("_move"),
            items,
            selected_idx: current,
            bg: None,
            placement: ContextMenuPlacement::AnchorPoint,
        };
        let new_sel = temp.move_selection(current, delta);

        if depth == 0 {
            self.dropdown_selected = new_sel;
        } else if let Some(s) = self.submenu_selected.last_mut() {
            *s = new_sel;
        }
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
            ..Default::default()
        }
    }

    fn separator() -> ContextMenuItem {
        ContextMenuItem::default()
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

    // ── Submenu state helpers ─────────────────────────────────────────

    fn menus_with_submenu() -> Vec<MenuDef> {
        vec![MenuDef {
            id: WidgetId::new("view"),
            label: "&View".into(),
            disabled: false,
            items: vec![
                action("sidebar", "Toggle Sidebar"),
                ContextMenuItem {
                    id: Some(WidgetId::new("export")),
                    label: StyledText::plain("Export"),
                    submenu: Some(vec![
                        action("export-png", "PNG"),
                        action("export-svg", "SVG"),
                    ]),
                    ..Default::default()
                },
                action("zoom", "Zoom In"),
            ],
        }]
    }

    #[test]
    fn items_at_depth_zero_returns_root_items() {
        let mut ms = MenuSystem::new(menus_with_submenu());
        ms.open_item = Some(0);
        let items = ms.items_at_depth(0).unwrap();
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].id, Some(WidgetId::new("sidebar")));
    }

    #[test]
    fn items_at_depth_one_returns_submenu_items() {
        let mut ms = MenuSystem::new(menus_with_submenu());
        ms.open_item = Some(0);
        ms.submenu_path = vec![1]; // item 1 ("Export") has a submenu
        let items = ms.items_at_depth(1).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].id, Some(WidgetId::new("export-png")));
    }

    #[test]
    fn open_submenu_pushes_path_and_first_selectable() {
        let mut ms = MenuSystem::new(menus_with_submenu());
        ms.open_item = Some(0);
        ms.dropdown_selected = 1; // "Export" is selected
        ms.open_submenu(1);
        assert_eq!(ms.submenu_path, vec![1]);
        assert_eq!(ms.submenu_selected, vec![0]); // first selectable in PNG/SVG
    }

    #[test]
    fn close_deepest_submenu_pops_one_level() {
        let mut ms = MenuSystem::new(menus_with_submenu());
        ms.open_item = Some(0);
        ms.submenu_path = vec![1];
        ms.submenu_selected = vec![0];
        ms.close_deepest_submenu();
        assert!(ms.submenu_path.is_empty());
        assert!(ms.submenu_selected.is_empty());
    }

    #[test]
    fn current_selected_returns_root_when_no_submenus() {
        let mut ms = MenuSystem::new(menus_with_submenu());
        ms.open_item = Some(0);
        ms.dropdown_selected = 2;
        assert_eq!(ms.current_selected(), 2);
    }

    #[test]
    fn current_selected_returns_deepest_submenu_selection() {
        let mut ms = MenuSystem::new(menus_with_submenu());
        ms.open_item = Some(0);
        ms.dropdown_selected = 1;
        ms.submenu_path = vec![1];
        ms.submenu_selected = vec![1];
        assert_eq!(ms.current_selected(), 1);
    }
}
