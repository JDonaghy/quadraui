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
                    let Some(cur) = self.open_item else {
                        return MenuEvent::Ignored;
                    };
                    let next = self.next_enabled_menu(cur, 1);
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
                    let Some(cur) = self.open_item else {
                        return MenuEvent::Ignored;
                    };
                    let prev = self.next_enabled_menu(cur, -1);
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
                                        // Trim any deeper submenus that were open, then open
                                        // this submenu (always open — clicking a submenu parent
                                        // re-opens even if already visible).
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

            // ── Mouse release ─────────────────────────────────────
            // Some terminal/multiplexer combinations (Alacritty+tmux,
            // certain gnome-terminal configs) drop `MouseDown(Left)` and
            // only ever deliver `MouseUp(Left)` for a click. Without this
            // arm, an outside click on those terminals never runs the
            // "click outside all open menu levels → close" check the
            // `MouseDown` arm's tail performs, so a dropdown never
            // dismisses on outside click there.
            //
            // On terminals that deliver both `Down` and `Up` normally,
            // `MouseDown` has already fully handled the click by the time
            // `Up` arrives (opened/closed/switched the menu, or activated
            // an item and closed). So this arm only needs to cover the
            // "outside everything" case; it must NOT re-run bar-item
            // toggling or item activation, or it would immediately
            // re-close a menu that `Down` just opened, or double-fire
            // `Activated`.
            UiEvent::MouseUp {
                button: MouseButton::Left,
                position,
                ..
            } => {
                if self.open_item.is_none() {
                    return MenuEvent::Ignored;
                }

                let bar = self.build_menu_bar();
                let bar_layout = backend.menu_bar_layout(bar_rect, &bar);

                // Clicks landing on the menu bar itself (open/close/switch)
                // are `MouseDown`'s job — leave them alone here.
                if !matches!(
                    bar_layout.hit_test(position.x, position.y),
                    MenuBarHit::Outside
                ) {
                    return MenuEvent::Ignored;
                }

                // Walk the open dropdown stack deepest-first. If the
                // position lands inside any open level (item or inert
                // region), that click was already handled by `MouseDown` —
                // do nothing here to avoid double-activating.
                let stack = self.dropdown_stack(backend, bar_rect);
                for (_, layout) in stack.iter().rev() {
                    if !matches!(
                        layout.hit_test(position.x, position.y),
                        ContextMenuHit::Empty
                    ) {
                        return MenuEvent::Ignored;
                    }
                }

                // Outside the bar and every open dropdown level → close.
                self.close(backend);
                MenuEvent::StateChanged
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

    // ── Minimal mock backend for handle()-level tests ─────────────────────────

    struct MockBackend {
        modal_stack: std::rc::Rc<std::cell::RefCell<crate::ModalStack>>,
        drag_state: std::rc::Rc<std::cell::RefCell<crate::DragState>>,
    }

    impl MockBackend {
        fn new() -> Self {
            Self {
                modal_stack: std::rc::Rc::new(std::cell::RefCell::new(crate::ModalStack::new())),
                drag_state: std::rc::Rc::new(std::cell::RefCell::new(crate::DragState::new())),
            }
        }
    }

    impl crate::backend::Backend for MockBackend {
        fn viewport(&self) -> crate::event::Viewport {
            crate::event::Viewport::new(80.0, 24.0, 1.0)
        }
        fn begin_frame(&mut self, _: crate::event::Viewport) {}
        fn end_frame(&mut self) {}
        fn poll_events(&mut self) -> Vec<UiEvent> {
            Vec::new()
        }
        fn wait_events(&mut self, _: std::time::Duration) -> Vec<UiEvent> {
            Vec::new()
        }
        fn register_accelerator(&mut self, _: &crate::accelerator::Accelerator) {}
        fn unregister_accelerator(&mut self, _: &crate::accelerator::AcceleratorId) {}
        fn modal_stack_mut(&mut self) -> &mut crate::ModalStack {
            // SAFETY: same leak-via-`Rc::as_ptr` pattern as
            // `GtkBackend::modal_stack_mut` — this mock is
            // single-threaded test code that never reentrantly calls
            // back into itself mid-borrow.
            unsafe {
                let cell_ptr = std::rc::Rc::as_ptr(&self.modal_stack);
                &mut *(*cell_ptr).as_ptr()
            }
        }
        fn drag_and_modal_mut(&mut self) -> (&mut crate::DragState, &mut crate::ModalStack) {
            unsafe {
                let drag_ptr = std::rc::Rc::as_ptr(&self.drag_state);
                let modal_ptr = std::rc::Rc::as_ptr(&self.modal_stack);
                (&mut *(*drag_ptr).as_ptr(), &mut *(*modal_ptr).as_ptr())
            }
        }
        fn modal_stack_handle(&self) -> std::rc::Rc<std::cell::RefCell<crate::ModalStack>> {
            self.modal_stack.clone()
        }
        fn drag_state_handle(&self) -> std::rc::Rc<std::cell::RefCell<crate::DragState>> {
            self.drag_state.clone()
        }
        fn services(&self) -> &dyn crate::backend::PlatformServices {
            unimplemented!()
        }
        fn backend_caps(&self) -> crate::backend::BackendCaps {
            crate::backend::BackendCaps::empty()
        }
        fn line_height(&self) -> f32 {
            1.0
        }
        fn char_width(&self) -> f32 {
            1.0
        }
        fn menu_bar_layout(&self, rect: Rect, bar: &crate::MenuBar) -> crate::MenuBarLayout {
            bar.layout(rect, |_| crate::MenuBarItemMeasure::new(10.0))
        }
        fn draw_menu_bar(&mut self, rect: Rect, bar: &crate::MenuBar) -> crate::MenuBarLayout {
            bar.layout(rect, |_| crate::MenuBarItemMeasure::new(10.0))
        }
        fn draw_tree(&mut self, _r: Rect, _t: &crate::TreeView) {}
        fn draw_list(&mut self, _r: Rect, _l: &crate::ListView) {}
        fn draw_data_table(
            &mut self,
            _r: Rect,
            _t: &crate::DataTable,
            _h: Option<usize>,
        ) -> crate::DataTableLayout {
            unimplemented!()
        }
        fn data_table_layout(&self, _r: Rect, _t: &crate::DataTable) -> crate::DataTableLayout {
            unimplemented!()
        }
        fn list_hscrollbar(&self, _r: Rect, _l: &crate::ListView) -> Option<crate::Scrollbar> {
            None
        }
        fn list_vscrollbar(&self, _r: Rect, _l: &crate::ListView) -> Option<crate::Scrollbar> {
            None
        }
        fn list_layout(&self, r: Rect, l: &crate::ListView) -> crate::ListViewLayout {
            l.layout(r.width, r.height, 0.0, |_| {
                crate::primitives::list::ListItemMeasure::new(1.0)
            })
        }
        fn draw_form(&mut self, _r: Rect, _f: &crate::Form) {}
        fn draw_palette(&mut self, _r: Rect, _p: &crate::Palette) {}
        fn draw_settings_chrome(
            &mut self,
            _r: Rect,
            _header_text: &str,
            _query: &str,
            _placeholder: &str,
            _active: bool,
        ) {
        }
        fn draw_status_bar(
            &mut self,
            _r: Rect,
            _b: &crate::primitives::status_bar::StatusBar,
            _hovered_id: Option<&WidgetId>,
            _pressed_id: Option<&WidgetId>,
        ) -> crate::StatusBarLayout {
            unimplemented!()
        }
        fn draw_tab_bar(
            &mut self,
            _r: Rect,
            _b: &crate::TabBar,
            _h: Option<usize>,
        ) -> crate::TabBarHits {
            unimplemented!()
        }
        fn draw_tab_bar_icons(
            &mut self,
            _r: Rect,
            _b: &crate::TabBar,
            _icons: &[Option<crate::TabIcon>],
            _h: Option<usize>,
        ) -> crate::TabBarHits {
            unimplemented!()
        }
        fn draw_activity_bar(
            &mut self,
            _r: Rect,
            _b: &crate::primitives::activity_bar::ActivityBar,
            _h: Option<usize>,
        ) -> Vec<crate::primitives::activity_bar::ActivityBarRowHit> {
            unimplemented!()
        }
        fn draw_terminal(&mut self, _r: Rect, _t: &crate::Terminal) {}
        fn draw_terminal_divider(&mut self, _r: Rect) {}
        fn draw_text_display(&mut self, _r: Rect, _t: &crate::TextDisplay) {}
        fn draw_command_line(&mut self, _r: Rect, _c: &crate::CommandLine) {}
        fn command_line_layout(
            &self,
            _r: Rect,
            _c: &crate::CommandLine,
        ) -> crate::primitives::command_line::CommandLineLayout {
            Default::default()
        }
        fn status_bar_layout(&self, _r: Rect, _b: &crate::StatusBar) -> crate::StatusBarLayout {
            crate::StatusBarLayout {
                bar_width: 0.0,
                bar_height: 0.0,
                visible_segments: Vec::new(),
                hit_regions: Vec::new(),
                resolved_right_start: 0,
            }
        }
        fn tab_bar_layout(&self, _r: Rect, _b: &crate::TabBar) -> crate::TabBarHits {
            crate::TabBarHits::default()
        }
        fn tab_bar_layout_icons(
            &self,
            _r: Rect,
            _b: &crate::TabBar,
            _icons: &[Option<crate::TabIcon>],
        ) -> crate::TabBarHits {
            crate::TabBarHits::default()
        }
        fn activity_bar_layout(
            &self,
            _r: Rect,
            _b: &crate::primitives::activity_bar::ActivityBar,
        ) -> Vec<crate::ActivityBarRowHit> {
            Vec::new()
        }
        fn text_display_layout(
            &self,
            _r: Rect,
            _t: &crate::TextDisplay,
        ) -> crate::TextDisplayLayout {
            unimplemented!()
        }
        fn draw_text_input(&mut self, _r: Rect, _t: &crate::TextInput) -> crate::TextInputLayout {
            unimplemented!()
        }
        fn text_input_layout(&self, _r: Rect, _t: &crate::TextInput) -> crate::TextInputLayout {
            unimplemented!()
        }
        fn draw_tooltip(&mut self, _t: &crate::Tooltip, _l: &crate::TooltipLayout) {}
        fn draw_context_menu(
            &mut self,
            _m: &crate::ContextMenu,
            _l: &crate::ContextMenuLayout,
        ) -> Vec<(Rect, WidgetId)> {
            Vec::new()
        }
        fn draw_dialog(&mut self, _d: &crate::Dialog, _l: &crate::DialogLayout) -> Vec<Rect> {
            unimplemented!()
        }
        fn draw_multi_section_view(&mut self, _r: Rect, _v: &crate::MultiSectionView) {}
        fn msv_layout(
            &self,
            _r: Rect,
            _v: &crate::MultiSectionView,
        ) -> crate::MultiSectionViewLayout {
            unimplemented!()
        }
        fn msv_metrics(&self) -> crate::primitives::multi_section_view::LayoutMetrics {
            unimplemented!()
        }
        fn tree_layout(
            &self,
            rect: Rect,
            tree: &crate::TreeView,
        ) -> crate::primitives::tree::TreeViewLayout {
            let lh = self.line_height();
            let indent_cells = tree.style.indent as f32;
            let chevron_w = if tree.style.show_chevrons {
                tree.style.chevron_expanded.chars().count() as f32 + 1.0
            } else {
                0.0
            };
            tree.layout(rect.width, rect.height, |i| {
                let row = &tree.rows[i];
                let chevron_end_x = if row.is_expanded.is_some() && chevron_w > 0.0 {
                    Some(row.indent as f32 * indent_cells + chevron_w)
                } else {
                    None
                };
                crate::primitives::tree::TreeRowMeasure {
                    height: lh,
                    chevron_end_x,
                }
            })
        }
        fn form_layout(&self, _r: Rect, _f: &crate::Form) -> crate::primitives::form::FormLayout {
            unimplemented!()
        }
        fn draw_editor(
            &mut self,
            _r: Rect,
            _e: &crate::primitives::editor::Editor,
        ) -> crate::backend::EditorPaintResult {
            Default::default()
        }
        fn draw_message_list(
            &mut self,
            _r: Rect,
            _l: &crate::primitives::message_list::MessageList,
        ) {
        }
        fn draw_rich_text_popup(
            &mut self,
            _p: &crate::RichTextPopup,
            _l: &crate::primitives::rich_text_popup::RichTextPopupLayout,
        ) {
        }
        fn draw_find_replace(
            &mut self,
            _r: Rect,
            _p: &crate::primitives::find_replace::FindReplacePanel,
        ) {
        }
        fn draw_completions(
            &mut self,
            _c: &crate::Completions,
            _l: &crate::primitives::completions::CompletionsLayout,
        ) {
        }
        fn draw_scrollbar(&mut self, _r: Rect, _s: &crate::Scrollbar) {}
        fn draw_drop_overlay(&mut self, _o: &crate::primitives::drop_zone::DropOverlay) {}
        fn draw_split(&mut self, _r: Rect, _s: &crate::Split) -> crate::SplitLayout {
            unimplemented!()
        }
        fn split_layout(&self, _r: Rect, _s: &crate::Split) -> crate::SplitLayout {
            unimplemented!()
        }
        fn draw_split_tree(&mut self, _r: Rect, _t: &crate::SplitTree) -> crate::SplitTreeLayout {
            unimplemented!()
        }
        fn split_tree_layout(&self, _r: Rect, _t: &crate::SplitTree) -> crate::SplitTreeLayout {
            unimplemented!()
        }
        fn draw_panel(&mut self, _r: Rect, _p: &crate::Panel) -> crate::PanelLayout {
            unimplemented!()
        }
        fn panel_layout(&self, _r: Rect, _p: &crate::Panel) -> crate::PanelLayout {
            unimplemented!()
        }
        fn draw_toast_stack(
            &mut self,
            _r: Rect,
            _s: &crate::ToastStack,
        ) -> crate::ToastStackLayout {
            unimplemented!()
        }
        fn toast_stack_layout(&self, _r: Rect, _s: &crate::ToastStack) -> crate::ToastStackLayout {
            unimplemented!()
        }
        fn draw_pipeline_view(
            &mut self,
            _r: Rect,
            _v: &crate::PipelineView,
        ) -> crate::PipelineViewLayout {
            unimplemented!()
        }
        fn pipeline_view_layout(
            &self,
            _r: Rect,
            _v: &crate::PipelineView,
        ) -> crate::PipelineViewLayout {
            unimplemented!()
        }
        fn draw_progress(&mut self, _r: Rect, _b: &crate::ProgressBar) -> crate::ProgressBarLayout {
            unimplemented!()
        }
        fn progress_layout(&self, _r: Rect, _b: &crate::ProgressBar) -> crate::ProgressBarLayout {
            unimplemented!()
        }
        fn draw_spinner(&mut self, _r: Rect, _s: &crate::Spinner) -> crate::SpinnerLayout {
            unimplemented!()
        }
        fn spinner_layout(&self, _r: Rect, _s: &crate::Spinner) -> crate::SpinnerLayout {
            unimplemented!()
        }
        fn draw_command_center(
            &mut self,
            _r: Rect,
            _c: &crate::CommandCenter,
        ) -> crate::CommandCenterLayout {
            unimplemented!()
        }
        fn command_center_layout(
            &self,
            _r: Rect,
            _c: &crate::CommandCenter,
        ) -> crate::CommandCenterLayout {
            unimplemented!()
        }
        fn draw_toolbar(
            &mut self,
            _r: Rect,
            _b: &crate::primitives::toolbar::Toolbar,
            _h: Option<&WidgetId>,
            _p: Option<&WidgetId>,
        ) -> crate::primitives::toolbar::ToolbarLayout {
            unimplemented!()
        }
        fn toolbar_layout(
            &self,
            _r: Rect,
            _b: &crate::primitives::toolbar::Toolbar,
        ) -> crate::primitives::toolbar::ToolbarLayout {
            unimplemented!()
        }
        fn draw_sidebar_panel(
            &mut self,
            _r: Rect,
            _p: &crate::primitives::sidebar_panel::SidebarPanel,
            _h: Option<&WidgetId>,
            _pr: Option<&WidgetId>,
        ) -> crate::primitives::sidebar_panel::SidebarPanelLayout {
            unimplemented!()
        }
        fn sidebar_panel_layout(
            &self,
            _r: Rect,
            _p: &crate::primitives::sidebar_panel::SidebarPanel,
        ) -> crate::primitives::sidebar_panel::SidebarPanelLayout {
            unimplemented!()
        }
        fn draw_diff_view(
            &mut self,
            _r: Rect,
            view: &crate::primitives::diff_view::DiffView,
        ) -> crate::primitives::diff_view::DiffViewLayout {
            crate::primitives::diff_view::DiffViewLayout {
                visible_rows: 0,
                total_rows: view.total_rows(),
            }
        }
        fn draw_chart(
            &mut self,
            _r: Rect,
            _c: &crate::primitives::chart::Chart,
            _h: Option<(usize, usize)>,
            _x: Option<f64>,
        ) -> crate::primitives::chart::ChartLayout {
            unimplemented!()
        }
        fn chart_layout(
            &self,
            _r: Rect,
            _c: &crate::primitives::chart::Chart,
        ) -> crate::primitives::chart::ChartLayout {
            unimplemented!()
        }

        fn draw_board(
            &mut self,
            _r: Rect,
            _m: &crate::primitives::board::BoardModel,
        ) -> crate::primitives::board::BoardLayout {
            crate::primitives::board::BoardLayout {
                bounds: crate::event::Rect::new(_r.x, _r.y, _r.width, _r.height),
                columns: vec![],
            }
        }

        fn board_layout(
            &self,
            _r: Rect,
            _m: &crate::primitives::board::BoardModel,
        ) -> crate::primitives::board::BoardLayout {
            crate::primitives::board::BoardLayout {
                bounds: crate::event::Rect::new(_r.x, _r.y, _r.width, _r.height),
                columns: vec![],
            }
        }

        fn draw_minimap(
            &mut self,
            _r: Rect,
            _m: &crate::primitives::minimap::Minimap,
        ) -> crate::backend::MinimapPaintResult {
            crate::backend::MinimapPaintResult::default()
        }

        fn minimap_layout(
            &self,
            _r: Rect,
            _m: &crate::primitives::minimap::Minimap,
        ) -> crate::primitives::minimap::MinimapLayout {
            crate::primitives::minimap::MinimapLayout::default()
        }

        fn draw_image(
            &mut self,
            _r: Rect,
            _i: &crate::primitives::image::Image,
        ) -> crate::backend::ImagePaintResult {
            crate::backend::ImagePaintResult::Unsupported
        }
    }

    // Helper: construct a `KeyPressed` event with no modifiers.
    fn key_ev(key: Key) -> UiEvent {
        UiEvent::KeyPressed {
            key,
            modifiers: Modifiers::default(),
            repeat: false,
        }
    }

    // Standard bar rect used by all handle() tests.
    fn bar_rect() -> Rect {
        Rect::new(0.0, 0.0, 80.0, 1.0)
    }

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

    // ── Keyboard-nav interaction tests (flow through handle()) ────────────────

    // Fixture: menus_with_submenu(), but the root dropdown is "open" so that
    // keyboard events are processed.  We set open_item directly (bypassing the
    // modal-stack push that open_menu does) because these tests focus on
    // navigation state, not on rendering.
    fn open_ms() -> MenuSystem {
        let mut ms = MenuSystem::new(menus_with_submenu());
        ms.open_item = Some(0);
        ms.dropdown_selected = 1; // pre-select "Export" (item 1, has submenu)
        ms
    }

    /// `Right` on a submenu-parent item must push the path and return `StateChanged`.
    #[test]
    fn handle_right_on_submenu_parent_opens_submenu() {
        let mut ms = open_ms();
        let mut backend = MockBackend::new();
        let ev = key_ev(Key::Named(NamedKey::Right));

        let result = ms.handle(&ev, &mut backend, bar_rect());

        assert_eq!(
            result,
            MenuEvent::StateChanged,
            "Right must return StateChanged"
        );
        assert_eq!(ms.submenu_path, vec![1], "submenu_path must grow to [1]");
        assert!(
            ms.open_item.is_some(),
            "menu must still be open after Right"
        );
    }

    /// `Left` with a submenu open must pop one level and return `StateChanged`.
    #[test]
    fn handle_left_with_submenu_open_closes_deepest() {
        let mut ms = open_ms();
        ms.submenu_path = vec![1];
        ms.submenu_selected = vec![0];
        let mut backend = MockBackend::new();
        let ev = key_ev(Key::Named(NamedKey::Left));

        let result = ms.handle(&ev, &mut backend, bar_rect());

        assert_eq!(
            result,
            MenuEvent::StateChanged,
            "Left must return StateChanged"
        );
        assert!(
            ms.submenu_path.is_empty(),
            "submenu_path must shrink back to []"
        );
        assert!(ms.open_item.is_some(), "top-level menu must remain open");
    }

    /// `Esc` with a submenu open must close only the deepest level — NOT the
    /// whole menu.
    #[test]
    fn handle_esc_with_submenu_closes_deepest_not_whole_menu() {
        let mut ms = open_ms();
        ms.submenu_path = vec![1];
        ms.submenu_selected = vec![0];
        let mut backend = MockBackend::new();
        let ev = key_ev(Key::Named(NamedKey::Escape));

        let result = ms.handle(&ev, &mut backend, bar_rect());

        assert_eq!(result, MenuEvent::StateChanged);
        assert!(ms.submenu_path.is_empty(), "deepest submenu must be closed");
        assert!(ms.open_item.is_some(), "top-level menu must still be open");
    }

    /// `Esc` with no submenus open must close the whole menu.
    #[test]
    fn handle_esc_with_no_submenu_closes_whole_menu() {
        let mut ms = open_ms();
        // No submenus open — submenu_path is empty.
        let mut backend = MockBackend::new();
        let ev = key_ev(Key::Named(NamedKey::Escape));

        let result = ms.handle(&ev, &mut backend, bar_rect());

        assert_eq!(result, MenuEvent::StateChanged);
        assert!(ms.open_item.is_none(), "whole menu must be closed");
        assert!(ms.submenu_path.is_empty());
    }

    /// `Enter` on a submenu-parent must open the child — NOT activate the item.
    #[test]
    fn handle_enter_on_submenu_parent_opens_child() {
        let mut ms = open_ms(); // dropdown_selected = 1 ("Export" with submenu)
        let mut backend = MockBackend::new();
        let ev = key_ev(Key::Named(NamedKey::Enter));

        let result = ms.handle(&ev, &mut backend, bar_rect());

        assert_eq!(
            result,
            MenuEvent::StateChanged,
            "Enter on parent must return StateChanged"
        );
        assert_eq!(ms.submenu_path, vec![1], "submenu must open for item 1");
        assert!(ms.open_item.is_some(), "menu must remain open");
    }

    /// `Enter` on a leaf item inside an open submenu must activate it and close
    /// the whole menu.
    #[test]
    fn handle_enter_on_leaf_in_submenu_activates_and_closes() {
        let mut ms = open_ms();
        // Open the submenu for "Export" (item 1) and select "PNG" (item 0 inside).
        ms.submenu_path = vec![1];
        ms.submenu_selected = vec![0]; // "PNG" (id = "export-png")
        let mut backend = MockBackend::new();
        let ev = key_ev(Key::Named(NamedKey::Enter));

        let result = ms.handle(&ev, &mut backend, bar_rect());

        assert_eq!(
            result,
            MenuEvent::Activated(WidgetId::new("export-png")),
            "Enter on leaf must return Activated with the item id"
        );
        assert!(ms.open_item.is_none(), "menu must close after activation");
        assert!(
            ms.submenu_path.is_empty(),
            "submenu_path must be cleared on close"
        );
    }

    // ── MouseUp outside-click dismiss (#429) ───────────────────────────────

    fn mouse_up_ev(position: crate::event::Point) -> UiEvent {
        UiEvent::MouseUp {
            widget: None,
            button: MouseButton::Left,
            position,
        }
    }

    /// `MouseUp(Left)` landing outside the menu bar and outside every open
    /// dropdown level must close the menu — this is the fix for terminals
    /// that drop `MouseDown(Left)` and only deliver `MouseUp(Left)`.
    #[test]
    fn handle_mouse_up_outside_everything_closes_menu() {
        let mut ms = MenuSystem::new(sample_menus());
        let mut backend = MockBackend::new();
        ms.open_menu(0, &mut backend, bar_rect());
        assert!(ms.open_item.is_some());

        // Far outside the bar (height 1) and outside the viewport-clamped
        // dropdown.
        let ev = mouse_up_ev(crate::event::Point::new(999.0, 999.0));
        let result = ms.handle(&ev, &mut backend, bar_rect());

        assert_eq!(result, MenuEvent::StateChanged);
        assert!(ms.open_item.is_none(), "menu must close on outside MouseUp");
    }

    /// `MouseUp(Left)` with no menu open must be a no-op `Ignored` — nothing
    /// to close, and no panic touching layout state.
    #[test]
    fn handle_mouse_up_with_no_menu_open_is_ignored() {
        let mut ms = MenuSystem::new(sample_menus());
        let mut backend = MockBackend::new();
        let ev = mouse_up_ev(crate::event::Point::new(999.0, 999.0));

        let result = ms.handle(&ev, &mut backend, bar_rect());

        assert_eq!(result, MenuEvent::Ignored);
        assert!(ms.open_item.is_none());
    }

    /// `MouseUp(Left)` on the menu-bar item that `MouseDown` just opened
    /// must NOT immediately re-close the menu — that zone belongs to
    /// `MouseDown`, and closing here would defeat every normal
    /// (Down-then-Up) click-to-open.
    #[test]
    fn handle_mouse_up_on_bar_item_does_not_close() {
        let mut ms = MenuSystem::new(sample_menus());
        let mut backend = MockBackend::new();
        ms.open_menu(0, &mut backend, bar_rect());

        // Item 0 ("File") occupies x in [0, 10), y in [0, 1) per the mock
        // backend's 10.0-wide, single-row layout.
        let ev = mouse_up_ev(crate::event::Point::new(5.0, 0.0));
        let result = ms.handle(&ev, &mut backend, bar_rect());

        assert_eq!(result, MenuEvent::Ignored);
        assert!(
            ms.open_item.is_some(),
            "menu must remain open after Up on the bar item that opened it"
        );
    }

    /// `MouseUp(Left)` landing on an actual open dropdown item must be
    /// ignored, not re-activated — `MouseDown` already owns item
    /// activation, so this arm must not double-fire `Activated`.
    #[test]
    fn handle_mouse_up_on_open_dropdown_item_is_ignored() {
        let mut ms = MenuSystem::new(sample_menus());
        let mut backend = MockBackend::new();
        ms.open_menu(0, &mut backend, bar_rect());

        let stack = ms.dropdown_stack(&backend, bar_rect());
        let item_bounds = stack[0].1.visible_items[0].bounds;
        let pos = crate::event::Point::new(
            item_bounds.x + item_bounds.width / 2.0,
            item_bounds.y + item_bounds.height / 2.0,
        );

        let ev = mouse_up_ev(pos);
        let result = ms.handle(&ev, &mut backend, bar_rect());

        assert_eq!(result, MenuEvent::Ignored);
        assert!(
            ms.open_item.is_some(),
            "menu must remain open — MouseUp must not activate or close"
        );
    }
}
