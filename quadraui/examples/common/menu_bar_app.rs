//! Backend-agnostic app code for the menu-bar example
//! ([`tui_menu_bar`] / [`gtk_menu_bar`]).
//!
//! [`MenuBarApp`] demonstrates a complete menu-bar experience: a
//! [`MenuBar`] at the top with dropdown menus via [`ContextMenu`]
//! composition. Hover-to-switch, keyboard navigation (Alt+key,
//! arrows, Enter, Esc), and a [`StatusBar`] at the bottom showing
//! the last activated action.
//!
//! The same `AppLogic` impl drives both backends — the only difference
//! between `tui_menu_bar.rs` and `gtk_menu_bar.rs` is the runner call.
//!
//! Controls:
//! - click menu label         open/close that menu's dropdown
//! - hover another label      switch dropdown (while one is open)
//! - hover in dropdown        highlight item
//! - click dropdown item      activate + close
//! - Alt+F/E/V                open that menu's dropdown
//! - ↑ / ↓                    navigate within dropdown
//! - ← / →                    switch to adjacent menu
//! - Enter                    activate highlighted item + close
//! - Esc                      close dropdown (or quit if none open)
//! - q                        quit (when no dropdown open)

use quadraui::{
    AppLogic, Backend, Color, ContextMenu, ContextMenuHit, ContextMenuItem, ContextMenuItemMeasure,
    ContextMenuLayout, ContextMenuPlacement, Key, MenuBar, MenuBarHit, MenuBarItem, Modifiers,
    NamedKey, Reaction, Rect, StatusBar, StatusBarSegment, StyledText, UiEvent, WidgetId,
};

const DROPDOWN_ID: &str = "menu-dropdown";

pub struct MenuBarApp {
    last_action: Option<String>,
    open_item: Option<usize>,
    focused_item: Option<usize>,
    dropdown_selected: usize,
    menus: Vec<Vec<ContextMenuItem>>,
}

impl MenuBarApp {
    pub fn new() -> Self {
        Self {
            last_action: None,
            open_item: None,
            focused_item: None,
            dropdown_selected: 0,
            menus: vec![
                vec![
                    action("new", "New File"),
                    action("open", "Open File"),
                    action("save", "Save"),
                    separator(),
                    action("quit", "Quit"),
                ],
                vec![
                    action("undo", "Undo"),
                    action("redo", "Redo"),
                    separator(),
                    action("cut", "Cut"),
                    action("copy", "Copy"),
                    action("paste", "Paste"),
                ],
                vec![
                    action("sidebar", "Toggle Sidebar"),
                    action("terminal", "Toggle Terminal"),
                    separator(),
                    action("zoom-in", "Zoom In"),
                    action("zoom-out", "Zoom Out"),
                ],
                // Help is disabled — empty menu
                vec![],
            ],
        }
    }

    fn menu_bar(&self) -> MenuBar {
        MenuBar {
            id: WidgetId::new("menu-bar"),
            items: vec![
                MenuBarItem {
                    id: WidgetId::new("file"),
                    label: "&File".into(),
                    disabled: false,
                },
                MenuBarItem {
                    id: WidgetId::new("edit"),
                    label: "&Edit".into(),
                    disabled: false,
                },
                MenuBarItem {
                    id: WidgetId::new("view"),
                    label: "&View".into(),
                    disabled: false,
                },
                MenuBarItem {
                    id: WidgetId::new("help"),
                    label: "&Help".into(),
                    disabled: true,
                },
            ],
            open_item: self.open_item,
            focused_item: self.focused_item,
        }
    }

    fn status_bar(&self) -> StatusBar {
        let msg = match &self.last_action {
            Some(a) => format!(" last: {a} "),
            None => " click a menu or press Alt+F/E/V — q to quit ".into(),
        };
        let open = match self.open_item {
            Some(i) => {
                let bar = self.menu_bar();
                format!(" open: {} ", bar.items[i].id.as_str())
            }
            None => " open: <none> ".into(),
        };
        StatusBar {
            id: WidgetId::new("status"),
            left_segments: vec![StatusBarSegment {
                text: msg,
                fg: Color::rgb(255, 255, 255),
                bg: Color::rgb(40, 80, 120),
                bold: false,
                action_id: None,
            }],
            right_segments: vec![StatusBarSegment {
                text: open,
                fg: Color::rgb(220, 220, 220),
                bg: Color::rgb(40, 80, 120),
                bold: false,
                action_id: None,
            }],
        }
    }

    fn build_dropdown(&self, menu_idx: usize) -> ContextMenu {
        ContextMenu {
            id: WidgetId::new(DROPDOWN_ID),
            items: self.menus[menu_idx].clone(),
            selected_idx: self.dropdown_selected,
            bg: None,
            placement: ContextMenuPlacement::Below,
        }
    }

    fn dropdown_layout(&self, backend: &dyn Backend) -> Option<(ContextMenu, ContextMenuLayout)> {
        let open_idx = self.open_item?;
        if self.menus[open_idx].is_empty() {
            return None;
        }
        let viewport = backend.viewport();
        let lh = backend.line_height();
        let menu_rect = Rect::new(0.0, 0.0, viewport.width, lh);
        let bar = self.menu_bar();
        let bar_layout = backend.menu_bar_layout(menu_rect, &bar);
        let raw_anchor = bar_layout.visible_items[open_idx].bounds;
        // Pad the anchor so the TUI rasteriser's 1-cell border outside
        // layout.bounds doesn't overwrite the menu bar. The x-offset
        // avoids clipping at x=0. The height offset is 1 cell on TUI
        // (~1.0) and ~2px on GTK to keep the dropdown tight.
        let anchor = Rect::new(
            raw_anchor.x + (lh * 0.15).max(1.0),
            raw_anchor.y,
            raw_anchor.width,
            raw_anchor.height + (lh * 0.15).max(1.0),
        );
        let viewport_rect = Rect::new(0.0, 0.0, viewport.width, viewport.height);
        let menu_width = 20.0 * lh;
        let ctx_menu = self.build_dropdown(open_idx);
        let layout = ctx_menu.layout_at(anchor, viewport_rect, menu_width, |i| {
            if ctx_menu.items[i].is_separator() {
                ContextMenuItemMeasure::new(lh * 0.5)
            } else {
                ContextMenuItemMeasure::new(lh)
            }
        });
        Some((ctx_menu, layout))
    }

    fn open_menu(&mut self, idx: usize, backend: &mut dyn Backend) {
        self.open_item = Some(idx);
        self.focused_item = Some(idx);
        self.dropdown_selected = self.first_selectable(idx);
        if let Some((_menu, layout)) = self.dropdown_layout(backend) {
            backend
                .modal_stack_mut()
                .push(WidgetId::new(DROPDOWN_ID), layout.bounds);
        }
    }

    fn close_menu(&mut self, backend: &mut dyn Backend) {
        self.open_item = None;
        self.focused_item = None;
        backend.modal_stack_mut().pop(&WidgetId::new(DROPDOWN_ID));
    }

    fn next_enabled_menu(&self, from: usize, delta: isize) -> usize {
        let bar = self.menu_bar();
        let n = bar.items.len() as isize;
        let mut idx = from as isize;
        for _ in 0..bar.items.len() {
            idx = (idx + delta).rem_euclid(n);
            if !bar.items[idx as usize].disabled {
                return idx as usize;
            }
        }
        from
    }

    fn first_selectable(&self, menu_idx: usize) -> usize {
        for (i, item) in self.menus[menu_idx].iter().enumerate() {
            if !item.is_separator() && !item.disabled {
                return i;
            }
        }
        0
    }

    fn move_selection(&mut self, delta: isize) {
        let Some(open_idx) = self.open_item else {
            return;
        };
        let items = &self.menus[open_idx];
        if items.is_empty() {
            return;
        }
        let n = items.len() as isize;
        let mut idx = self.dropdown_selected as isize;
        for _ in 0..items.len() {
            idx = (idx + delta).rem_euclid(n);
            if !items[idx as usize].is_separator() && !items[idx as usize].disabled {
                self.dropdown_selected = idx as usize;
                return;
            }
        }
    }

    fn activate_selected(&mut self, backend: &mut dyn Backend) {
        let Some(open_idx) = self.open_item else {
            return;
        };
        let items = &self.menus[open_idx];
        if let Some(item) = items.get(self.dropdown_selected) {
            if let Some(ref id) = item.id {
                self.last_action = Some(id.as_str().to_string());
            }
        }
        self.close_menu(backend);
    }
}

impl Default for MenuBarApp {
    fn default() -> Self {
        Self::new()
    }
}

impl AppLogic for MenuBarApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let viewport = backend.viewport();
        let lh = backend.line_height();

        let menu_rect = Rect::new(0.0, 0.0, viewport.width, lh);
        let bar = self.menu_bar();
        let _ = backend.draw_menu_bar(menu_rect, &bar);

        let status_rect = Rect::new(0.0, viewport.height - lh, viewport.width, lh);
        let status = self.status_bar();
        let _ = backend.draw_status_bar(status_rect, &status);

        if let Some((ctx_menu, layout)) = self.dropdown_layout(backend) {
            let _ = backend.draw_context_menu(&ctx_menu, &layout);
        }
    }

    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
        match event {
            // ── Keyboard ──────────────────────────────────────────────
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Escape),
                ..
            } => {
                if self.open_item.is_some() {
                    self.close_menu(backend);
                    Reaction::Redraw
                } else {
                    Reaction::Exit
                }
            }
            UiEvent::KeyPressed {
                key: Key::Char('q'),
                ..
            } if self.open_item.is_none() => Reaction::Exit,
            UiEvent::KeyPressed {
                key: Key::Char(c),
                modifiers: Modifiers { alt: true, .. },
                ..
            } => {
                let bar = self.menu_bar();
                if let Some(idx) = bar.find_alt_target(c) {
                    if self.open_item == Some(idx) {
                        self.close_menu(backend);
                    } else {
                        self.open_menu(idx, backend);
                    }
                }
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Down),
                ..
            } if self.open_item.is_some() => {
                self.move_selection(1);
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Up),
                ..
            } if self.open_item.is_some() => {
                self.move_selection(-1);
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Right),
                ..
            } if self.open_item.is_some() => {
                let next = self.next_enabled_menu(self.open_item.unwrap(), 1);
                self.close_menu(backend);
                self.open_menu(next, backend);
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Left),
                ..
            } if self.open_item.is_some() => {
                let prev = self.next_enabled_menu(self.open_item.unwrap(), -1);
                self.close_menu(backend);
                self.open_menu(prev, backend);
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Enter),
                ..
            } if self.open_item.is_some() => {
                self.activate_selected(backend);
                Reaction::Redraw
            }

            // ── Mouse ─────────────────────────────────────────────────
            UiEvent::MouseDown { position, .. } => {
                let viewport = backend.viewport();
                let lh = backend.line_height();
                let menu_rect = Rect::new(0.0, 0.0, viewport.width, lh);
                let bar = self.menu_bar();
                let bar_layout = backend.menu_bar_layout(menu_rect, &bar);

                match bar_layout.hit_test(position.x, position.y) {
                    MenuBarHit::Item(i) => {
                        if self.open_item == Some(i) {
                            self.close_menu(backend);
                        } else {
                            self.close_menu(backend);
                            self.open_menu(i, backend);
                        }
                        return Reaction::Redraw;
                    }
                    MenuBarHit::Bar => {
                        if self.open_item.is_some() {
                            self.close_menu(backend);
                        }
                        return Reaction::Redraw;
                    }
                    MenuBarHit::Outside => {}
                }

                if let Some((_, ref layout)) = self.dropdown_layout(backend) {
                    match layout.hit_test(position.x, position.y) {
                        ContextMenuHit::Item(ref id) => {
                            self.last_action = Some(id.as_str().to_string());
                            self.close_menu(backend);
                            return Reaction::Redraw;
                        }
                        ContextMenuHit::Inert => return Reaction::Continue,
                        ContextMenuHit::Empty => {
                            self.close_menu(backend);
                            return Reaction::Redraw;
                        }
                    }
                }

                if self.open_item.is_some() {
                    self.close_menu(backend);
                    return Reaction::Redraw;
                }
                Reaction::Continue
            }

            UiEvent::MouseMoved { position, .. } => {
                let viewport = backend.viewport();
                let lh = backend.line_height();
                let menu_rect = Rect::new(0.0, 0.0, viewport.width, lh);
                let bar = self.menu_bar();
                let bar_layout = backend.menu_bar_layout(menu_rect, &bar);

                if self.open_item.is_some() {
                    match bar_layout.hit_test(position.x, position.y) {
                        MenuBarHit::Item(i)
                            if !bar.items[i].disabled && self.open_item != Some(i) =>
                        {
                            self.close_menu(backend);
                            self.open_menu(i, backend);
                            return Reaction::Redraw;
                        }
                        _ => {}
                    }
                    if let Some((ctx_menu, ref layout)) = self.dropdown_layout(backend) {
                        for vi in &layout.visible_items {
                            if vi.clickable
                                && position.x >= vi.bounds.x
                                && position.x < vi.bounds.x + vi.bounds.width
                                && position.y >= vi.bounds.y
                                && position.y < vi.bounds.y + vi.bounds.height
                            {
                                if self.dropdown_selected != vi.item_idx {
                                    self.dropdown_selected = vi.item_idx;
                                    return Reaction::Redraw;
                                }
                                return Reaction::Continue;
                            }
                        }
                        let _ = ctx_menu;
                    }
                    Reaction::Continue
                } else {
                    let new_focus = match bar_layout.hit_test(position.x, position.y) {
                        MenuBarHit::Item(i) if !bar.items[i].disabled => Some(i),
                        _ => None,
                    };
                    if new_focus != self.focused_item {
                        self.focused_item = new_focus;
                        Reaction::Redraw
                    } else {
                        Reaction::Continue
                    }
                }
            }

            UiEvent::WindowResized { .. } => Reaction::Redraw,
            _ => Reaction::Continue,
        }
    }
}

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
        label: StyledText::plain(""),
        detail: None,
        disabled: false,
    }
}
