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
    AppLogic, Backend, Color, ContextMenu, ContextMenuHit, ContextMenuItem, ContextMenuItemMeasure,
    ContextMenuLayout, ContextMenuPlacement, Key, MenuDef, MenuEvent, MenuSystem, MouseButton,
    NamedKey, Point, Reaction, Rect, StatusBar, StatusBarSegment, StyledText, UiEvent, WidgetId,
};

// ── Context-menu state ────────────────────────────────────────────────────────

/// State for an open in-window right-click context menu, including any open
/// nested submenus.
struct CtxState {
    /// The root context menu (its `selected_idx` holds the root selection).
    menu: ContextMenu,
    /// Top-left anchor for the root popup.
    anchor: Point,
    /// Depth-first path of open submenus within the root.
    /// `submenu_path[d]` = item index in the menu at depth `d` with an open child.
    submenu_path: Vec<usize>,
    /// Selected item index at each submenu depth.
    submenu_selected: Vec<usize>,
}

impl CtxState {
    /// Selected index at the current deepest open level.
    fn current_selected(&self) -> usize {
        if self.submenu_path.is_empty() {
            self.menu.selected_idx
        } else {
            *self.submenu_selected.last().unwrap_or(&0)
        }
    }

    /// Items at depth `depth` (0 = root dropdown items), or `None` if unavailable.
    fn items_at_depth(&self, depth: usize) -> Option<Vec<ContextMenuItem>> {
        let mut items = self.menu.items.clone();
        for &path_idx in self.submenu_path.iter().take(depth) {
            items = items.into_iter().nth(path_idx)?.submenu?;
        }
        Some(items)
    }

    /// Set selection at the current deepest level.
    fn set_current_selected(&mut self, new: usize) {
        if self.submenu_path.is_empty() {
            self.menu.selected_idx = new;
        } else if let Some(s) = self.submenu_selected.last_mut() {
            *s = new;
        }
    }

    /// Open a submenu at the current depth for the item at `item_idx`.
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

    /// Move selection at the current deepest level by `delta` steps,
    /// skipping separators and disabled items.
    fn move_selection(&mut self, delta: i32) {
        let depth = self.submenu_path.len();
        let current = self.current_selected();
        let items = match self.items_at_depth(depth) {
            Some(items) => items,
            None => return,
        };
        let n = items.len() as i32;
        if n == 0 {
            return;
        }
        let mut idx = current as i32;
        for _ in 0..items.len() {
            idx = (idx + delta).rem_euclid(n);
            if !items[idx as usize].is_separator() && !items[idx as usize].disabled {
                self.set_current_selected(idx as usize);
                return;
            }
        }
    }
}

// ── App ───────────────────────────────────────────────────────────────────────

pub struct SubmenuApp {
    menu_system: MenuSystem,
    ctx_menu: Option<CtxState>,
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
            menu_system: MenuSystem::new(vec![
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
            ]),
            ctx_menu: None,
            last_action: None,
        }
    }

    fn status_bar(&self) -> StatusBar {
        let left = match &self.last_action {
            Some(a) => format!(" activated: {a} "),
            None => " right-click body for ctx menu | Alt+F/V for menu bar | q to quit ".into(),
        };
        let right = if self.ctx_menu.is_some() {
            " ctx menu open ".into()
        } else if self.menu_system.is_open() {
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
        if self.menu_system.is_open() {
            self.menu_system.close(backend);
        }
        let lh = backend.line_height();
        let viewport = backend.viewport();
        let vp = Rect::new(0.0, 0.0, viewport.width, viewport.height);
        let menu = ContextMenu {
            id: WidgetId::new("ctx-menu"),
            items: Self::ctx_menu_items(),
            selected_idx: 0,
            bg: None,
            placement: ContextMenuPlacement::AnchorPoint,
        };
        // Push to modal stack with root bounds so clicks inside don't leak.
        let layout = menu.layout(anchor.x, anchor.y, vp, 20.0 * lh, |i| {
            ContextMenuItemMeasure::new(if menu.items[i].is_separator() {
                (lh * 0.5).max(1.0)
            } else {
                lh
            })
        });
        backend
            .modal_stack_mut()
            .push(WidgetId::new("ctx-menu"), layout.bounds);
        self.ctx_menu = Some(CtxState {
            menu,
            anchor,
            submenu_path: Vec::new(),
            submenu_selected: Vec::new(),
        });
    }

    fn close_ctx_menu(&mut self, backend: &mut dyn Backend) {
        backend.modal_stack_mut().pop(&WidgetId::new("ctx-menu"));
        self.ctx_menu = None;
    }

    /// Render the in-window context menu (root + all open submenus).
    fn render_ctx_menu(state: &CtxState, backend: &mut dyn Backend) {
        let lh = backend.line_height();
        let viewport = backend.viewport();
        let vp = Rect::new(0.0, 0.0, viewport.width, viewport.height);
        let menu_w = 20.0 * lh;
        let item_h = lh;
        let sep_h = (lh * 0.5_f32).max(1.0_f32);

        let root_layout = state
            .menu
            .layout(state.anchor.x, state.anchor.y, vp, menu_w, |i| {
                ContextMenuItemMeasure::new(if state.menu.items[i].is_separator() {
                    sep_h
                } else {
                    item_h
                })
            });
        let _ = backend.draw_context_menu(&state.menu, &root_layout);

        // Draw each open submenu level.
        let mut parent_items: Vec<ContextMenuItem> = state.menu.items.clone();
        let mut parent_bounds = root_layout.bounds;
        let mut parent_vis = root_layout.visible_items.clone();

        for (d, &path_idx) in state.submenu_path.iter().enumerate() {
            let sub_items = match parent_items.get(path_idx).and_then(|i| i.submenu.clone()) {
                Some(items) => items,
                None => break,
            };

            // Pull-right with left-flip on overflow.
            let preferred_x = parent_bounds.x + parent_bounds.width + 1.0;
            let flipped_x = parent_bounds.x - menu_w - 1.0;
            let anchor_x = if preferred_x + menu_w <= vp.x + vp.width {
                preferred_x
            } else if flipped_x >= vp.x {
                flipped_x
            } else {
                (vp.x + vp.width - menu_w).max(vp.x)
            };
            let anchor_y = parent_vis
                .iter()
                .find(|v| v.item_idx == path_idx)
                .map(|v| v.bounds.y)
                .unwrap_or(parent_bounds.y);

            let selected = state.submenu_selected.get(d).copied().unwrap_or(0);
            let sub_menu = ContextMenu {
                id: WidgetId::new("ctx-submenu"),
                items: sub_items.clone(),
                selected_idx: selected,
                bg: None,
                placement: ContextMenuPlacement::AnchorPoint,
            };
            let sub_layout = sub_menu.layout(anchor_x, anchor_y, vp, menu_w, |i| {
                ContextMenuItemMeasure::new(if sub_items[i].is_separator() {
                    sep_h
                } else {
                    item_h
                })
            });
            let _ = backend.draw_context_menu(&sub_menu, &sub_layout);

            parent_items = sub_items;
            parent_bounds = sub_layout.bounds;
            parent_vis = sub_layout.visible_items;
        }
    }

    /// Handle a keyboard or mouse event directed at the in-window context menu.
    /// Returns `Some(Reaction)` if the event was consumed; `None` if not.
    fn handle_ctx_event(&mut self, event: &UiEvent, backend: &mut dyn Backend) -> Option<Reaction> {
        let state = self.ctx_menu.as_mut()?;

        match event {
            UiEvent::KeyPressed { key, .. } => match key {
                Key::Named(NamedKey::Escape) => {
                    let has_sub = !state.submenu_path.is_empty();
                    if has_sub {
                        state.close_deepest_submenu();
                    } else {
                        // Release the borrow before calling close_ctx_menu.
                        let _ = state;
                        self.close_ctx_menu(backend);
                    }
                    Some(Reaction::Redraw)
                }
                Key::Named(NamedKey::Down) => {
                    state.move_selection(1);
                    Some(Reaction::Redraw)
                }
                Key::Named(NamedKey::Up) => {
                    state.move_selection(-1);
                    Some(Reaction::Redraw)
                }
                Key::Named(NamedKey::Left) => {
                    if !state.submenu_path.is_empty() {
                        state.close_deepest_submenu();
                        Some(Reaction::Redraw)
                    } else {
                        None
                    }
                }
                Key::Named(NamedKey::Right) | Key::Named(NamedKey::Enter) => {
                    let depth = state.submenu_path.len();
                    let sel = state.current_selected();
                    let item_opt = state
                        .items_at_depth(depth)
                        .and_then(|items| items.into_iter().nth(sel));
                    match item_opt {
                        Some(item) if item.submenu.is_some() => {
                            state.open_submenu(sel);
                            Some(Reaction::Redraw)
                        }
                        Some(item) if matches!(key, Key::Named(NamedKey::Enter)) => {
                            if let Some(id) = item.id {
                                self.last_action = Some(id.as_str().to_owned());
                            }
                            self.close_ctx_menu(backend);
                            Some(Reaction::Redraw)
                        }
                        _ => None,
                    }
                }
                _ => None,
            },

            UiEvent::MouseDown {
                button: MouseButton::Left,
                position,
                ..
            } => {
                // Hit-test against each open level (deepest first).
                let lh = backend.line_height();
                let viewport = backend.viewport();
                let vp = Rect::new(0.0, 0.0, viewport.width, viewport.height);
                let menu_w = 20.0 * lh;
                let item_h = lh;
                let sep_h = (lh * 0.5_f32).max(1.0_f32);

                // Build the stack of (items, anchor_x, anchor_y) for each level.
                let mut levels: Vec<(Vec<ContextMenuItem>, f32, f32)> = Vec::new();
                {
                    let s = self.ctx_menu.as_ref().unwrap();
                    levels.push((s.menu.items.clone(), s.anchor.x, s.anchor.y));
                    let mut par_items = s.menu.items.clone();
                    let root_layout = s.menu.layout(s.anchor.x, s.anchor.y, vp, menu_w, |i| {
                        ContextMenuItemMeasure::new(if par_items[i].is_separator() {
                            sep_h
                        } else {
                            item_h
                        })
                    });
                    let mut par_bounds = root_layout.bounds;
                    let mut par_vis = root_layout.visible_items;

                    for &path_idx in &self.ctx_menu.as_ref().unwrap().submenu_path.clone() {
                        let sub_items =
                            match par_items.get(path_idx).and_then(|i| i.submenu.clone()) {
                                Some(items) => items,
                                None => break,
                            };
                        let preferred_px = par_bounds.x + par_bounds.width + 1.0;
                        let flipped_px = par_bounds.x - menu_w - 1.0;
                        let px = if preferred_px + menu_w <= vp.x + vp.width {
                            preferred_px
                        } else if flipped_px >= vp.x {
                            flipped_px
                        } else {
                            (vp.x + vp.width - menu_w).max(vp.x)
                        };
                        let py = par_vis
                            .iter()
                            .find(|v| v.item_idx == path_idx)
                            .map(|v| v.bounds.y)
                            .unwrap_or(par_bounds.y);
                        let sub_layout = ContextMenu {
                            id: WidgetId::new("t"),
                            items: sub_items.clone(),
                            selected_idx: 0,
                            bg: None,
                            placement: ContextMenuPlacement::AnchorPoint,
                        }
                        .layout(px, py, vp, menu_w, |i| {
                            ContextMenuItemMeasure::new(if sub_items[i].is_separator() {
                                sep_h
                            } else {
                                item_h
                            })
                        });
                        par_bounds = sub_layout.bounds;
                        par_vis = sub_layout.visible_items;
                        levels.push((sub_items.clone(), px, py));
                        par_items = sub_items;
                    }
                }

                // Hit-test deepest-first.
                for (depth_idx, (ref items, ax, ay)) in levels.iter().enumerate().rev() {
                    let menu = ContextMenu {
                        id: WidgetId::new("t"),
                        items: items.clone(),
                        selected_idx: 0,
                        bg: None,
                        placement: ContextMenuPlacement::AnchorPoint,
                    };
                    let layout = menu.layout(*ax, *ay, vp, menu_w, |i| {
                        ContextMenuItemMeasure::new(if items[i].is_separator() {
                            sep_h
                        } else {
                            item_h
                        })
                    });
                    match layout.hit_test(position.x, position.y) {
                        ContextMenuHit::Item(ref id) => {
                            let item_idx = layout
                                .visible_items
                                .iter()
                                .find(|v| v.clickable && items[v.item_idx].id.as_ref() == Some(id))
                                .map(|v| v.item_idx);
                            if let Some(idx) = item_idx {
                                let state = self.ctx_menu.as_mut().unwrap();
                                if items[idx].submenu.is_some() {
                                    state.submenu_path.truncate(depth_idx);
                                    state.submenu_selected.truncate(depth_idx);
                                    state.open_submenu(idx);
                                    return Some(Reaction::Redraw);
                                } else {
                                    self.last_action = Some(id.as_str().to_owned());
                                    self.close_ctx_menu(backend);
                                    return Some(Reaction::Redraw);
                                }
                            }
                            return Some(Reaction::Redraw);
                        }
                        ContextMenuHit::Inert => return Some(Reaction::Redraw),
                        ContextMenuHit::Empty => continue,
                    }
                }

                // Click outside all levels → close.
                self.close_ctx_menu(backend);
                Some(Reaction::Redraw)
            }

            UiEvent::MouseMoved { position, .. } => {
                // Update hover selection at deepest level containing cursor.
                let lh = backend.line_height();
                let viewport = backend.viewport();
                let vp = Rect::new(0.0, 0.0, viewport.width, viewport.height);
                let menu_w = 20.0 * lh;
                let item_h = lh;
                let sep_h = (lh * 0.5_f32).max(1.0_f32);

                let state = self.ctx_menu.as_mut().unwrap();
                let mut par_items = state.menu.items.clone();
                let root_layout =
                    state
                        .menu
                        .layout(state.anchor.x, state.anchor.y, vp, menu_w, |i| {
                            ContextMenuItemMeasure::new(if par_items[i].is_separator() {
                                sep_h
                            } else {
                                item_h
                            })
                        });
                let mut levels: Vec<(Vec<ContextMenuItem>, ContextMenuLayout)> =
                    vec![(state.menu.items.clone(), root_layout)];

                for &path_idx in &state.submenu_path.clone() {
                    let par_b = levels.last().unwrap().1.bounds;
                    let par_v = levels.last().unwrap().1.visible_items.clone();
                    let sub_items = match par_items.get(path_idx).and_then(|i| i.submenu.clone()) {
                        Some(items) => items,
                        None => break,
                    };
                    let preferred_px = par_b.x + par_b.width + 1.0;
                    let flipped_px = par_b.x - menu_w - 1.0;
                    let px = if preferred_px + menu_w <= vp.x + vp.width {
                        preferred_px
                    } else if flipped_px >= vp.x {
                        flipped_px
                    } else {
                        (vp.x + vp.width - menu_w).max(vp.x)
                    };
                    let py = par_v
                        .iter()
                        .find(|v| v.item_idx == path_idx)
                        .map(|v| v.bounds.y)
                        .unwrap_or(par_b.y);
                    let sub_menu = ContextMenu {
                        id: WidgetId::new("t"),
                        items: sub_items.clone(),
                        selected_idx: 0,
                        bg: None,
                        placement: ContextMenuPlacement::AnchorPoint,
                    };
                    let sub_layout = sub_menu.layout(px, py, vp, menu_w, |i| {
                        ContextMenuItemMeasure::new(if sub_items[i].is_separator() {
                            sep_h
                        } else {
                            item_h
                        })
                    });
                    par_items = sub_items.clone();
                    levels.push((sub_items, sub_layout));
                }

                for (depth_idx, (ref items, ref layout)) in levels.iter().enumerate() {
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
                            let state = self.ctx_menu.as_mut().unwrap();
                            if items[item_idx].submenu.is_some() {
                                state.submenu_path.truncate(depth_idx);
                                state.submenu_selected.truncate(depth_idx);
                                state.open_submenu(item_idx);
                            } else {
                                if depth_idx == 0 {
                                    state.menu.selected_idx = item_idx;
                                } else if let Some(s) =
                                    state.submenu_selected.get_mut(depth_idx - 1)
                                {
                                    *s = item_idx;
                                }
                                if state.submenu_path.len() > depth_idx {
                                    state.submenu_path.truncate(depth_idx);
                                    state.submenu_selected.truncate(depth_idx);
                                }
                            }
                            return Some(Reaction::Redraw);
                        }
                    }
                }
                None
            }

            _ => None,
        }
    }
}

impl AppLogic for SubmenuApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let viewport = backend.viewport();
        let lh = backend.line_height();

        // Menu bar across the top row.
        let bar_rect = Rect::new(0.0, 0.0, viewport.width, lh);
        self.menu_system.render(backend, bar_rect);

        // In-window context menu (if open).
        if let Some(ref state) = self.ctx_menu {
            Self::render_ctx_menu(state, backend);
        }

        // Status bar at the bottom.
        let status_rect = Rect::new(0.0, viewport.height - lh, viewport.width, lh);
        let _ = backend.draw_status_bar(status_rect, &self.status_bar(), None, None);
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

        // Context menu takes priority when open.
        if self.ctx_menu.is_some() {
            if let Some(reaction) = self.handle_ctx_event(&event, backend) {
                return reaction;
            }
            // Unhandled keys while ctx menu is open: fall through so e.g.
            // window-resize still triggers a redraw.
        }

        // Menu bar / dropdown handling.
        let viewport = backend.viewport();
        let lh = backend.line_height();
        let bar_rect = Rect::new(0.0, 0.0, viewport.width, lh);
        match self.menu_system.handle(&event, backend, bar_rect) {
            MenuEvent::Activated(id) => {
                if id.as_str() == "quit" {
                    return Reaction::Exit;
                }
                self.last_action = Some(id.as_str().to_owned());
                return Reaction::Redraw;
            }
            MenuEvent::StateChanged | MenuEvent::Consumed => return Reaction::Redraw,
            MenuEvent::Ignored => {}
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
