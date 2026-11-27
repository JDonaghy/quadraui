//! [`AppMenu`] — an application menu bar with its dropdown and submenu
//! cascade on the widget contract, with a host API
//! ([`AppMenu::handle_ui_event`], [`AppMenu::render`]) for a host that
//! drives it with raw events.
//!
//! An `AppMenu` composes [`MenuBar`] and [`ContextMenu`]: the bar paints
//! and resolves clicks and keys through `MenuBar`'s own [`Primitive`] and
//! [`Interactive`], and each open level of the cascade is a
//! [`ContextMenu`] the widget owns as a popup ([`Interactive::popups`]).
//! The runtime paints the popups above the frame, hit-tests them first
//! and routes their events back as [`RoutedEvent::PopupEvent`]; an
//! outside press or Escape arrives as [`RoutedEvent::PopupDismissed`].
//! Keys while a dropdown is open go to the deepest level's
//! [`ContextMenu::handle_key`]; `AppMenu` only opens or closes the right
//! level in response.
//!
//! Which menu is open, the highlight at each level and the open submenu
//! path are visible state, so they live on the descriptor and the host
//! owns them: the widget reports [`AppMenuEvent::Changed`] with the next
//! [`AppMenuView`] and the host applies it ([`AppMenu::apply`]).
//!
//! Under a native menu style the bar is the platform's own:
//! [`Primitive::native_presentation`] installs it through
//! [`Backend::install_menu_bar`], [`AppMenu::native`] makes paint return
//! early, and AppKit reports activations as [`UiEvent::MenuActivated`].
//!
//! ```ignore
//! // A host driving the menu with raw events, in handle():
//! match self.menu.handle_ui_event(&mut self.menu_state, &event, backend, bar_rect) {
//!     Handled::Event(AppMenuEvent::Activated(id)) if id.as_str() == "save" => { /* save */ }
//!     Handled::Event(AppMenuEvent::Activated(id)) if id.as_str() == "quit" => return Reaction::Exit,
//!     Handled::Ignored => { /* handle non-menu events */ }
//!     _ => return Reaction::Redraw,
//! }
//! ```

use serde::{Deserialize, Deserializer, Serialize};

use super::context_menu_controller::paint_placed_menu;
use crate::a11y::{A11yAction, A11yChild, A11yInfo, A11yPrimitive};
use crate::backend::{Backend, ResolvedMenuStyle};
use crate::behaviour::nav::{FlatNav, NavKey};
use crate::compose::widget_host;
use crate::compose::Handled;
use crate::event::{Point, Rect, UiEvent};
use crate::interactive::{
    EventCx, FocusSpec, Interactive, KeyEvent, Never, PointerKind, RoutedEvent, SubTarget,
    WidgetAction,
};
use crate::popup::{DismissReason, Popup, PopupKey, PopupSide};
use crate::primitive::{LayoutCtx, Primitive, Size, Units};
use crate::primitives::context_menu::{
    item_heights, level_width, ContextMenu, ContextMenuEvent, ContextMenuItem, ContextMenuLayout,
    ContextMenuPlacement, ContextMenuPrimitiveLayout, ContextMenuState, CELL_BORDER_INSET,
};
use crate::primitives::menu_bar::{
    MenuBar, MenuBarEvent, MenuBarHit, MenuBarItem, MenuBarPrimitiveLayout,
};
use crate::types::WidgetId;
use crate::{DrawsPrimitives, Key, MouseButton, NamedKey};

/// One top-level menu and its dropdown items.
///
/// Serialises as a [`MenuBarItem`] does — its dropdown items under
/// `submenu` — so an [`AppMenu`] decodes from a `menu_bar` leaf dict.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MenuDef {
    pub id: WidgetId,
    pub label: String,
    #[serde(default)]
    pub disabled: bool,
    #[serde(rename = "submenu", default, deserialize_with = "items_or_none")]
    pub items: Vec<ContextMenuItem>,
}

/// `submenu: null` (a [`MenuBarItem`] with no dropdown) decodes as no
/// items.
fn items_or_none<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<ContextMenuItem>, D::Error> {
    Ok(Option::<Vec<ContextMenuItem>>::deserialize(d)?.unwrap_or_default())
}

// ── Widget ────────────────────────────────────────────────────────────────

/// Which menu is open and where in it the highlight sits — the visible
/// state an [`AppMenu`] reports through [`AppMenuEvent::Changed`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppMenuView {
    /// The menu whose dropdown is open.
    #[serde(default)]
    pub open_item: Option<usize>,
    /// The bar item keyboard or pointer focus sits on.
    #[serde(default)]
    pub focused_item: Option<usize>,
    /// Highlighted item in the open dropdown.
    #[serde(default)]
    pub dropdown_selected: usize,
    /// Depth-first path of open submenus within the open dropdown:
    /// `submenu_path[d]` is the item at depth `d` whose child is open.
    #[serde(default)]
    pub submenu_path: Vec<usize>,
    /// Highlighted item at each open submenu depth, parallel to
    /// `submenu_path`.
    #[serde(default)]
    pub submenu_selected: Vec<usize>,
}

/// An application menu bar: the top-level menus, which one is open, and
/// its dropdown and submenu cascade. Serialises with the same shape as a
/// [`MenuBar`] (`id`, `items`, `open_item`, `focused_item`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppMenu {
    /// The menu's id, also the painted bar's.
    pub id: WidgetId,
    /// The top-level menus, in bar order.
    #[serde(rename = "items")]
    pub menus: Vec<MenuDef>,
    /// Which menu is open and where its highlight sits.
    #[serde(flatten)]
    pub view: AppMenuView,
    /// Whether the host's effective menu style is native
    /// ([`Backend::effective_menu_style`]): the bar is then the
    /// platform's own, nothing paints in the window and no dropdown
    /// opens. A host sets it from the backend every frame.
    #[serde(default)]
    pub native: bool,
    /// Whether a plain printable key in an open dropdown is type-ahead
    /// (on by default). Off, the dropdown leaves such a key unhandled so
    /// it reaches the host.
    #[serde(default = "crate::primitives::tab_bar::default_true")]
    pub typeahead: bool,
}

/// What an [`AppMenu`] reports.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum AppMenuEvent {
    /// An action item was activated. The menu has already reported
    /// itself closed.
    Activated(WidgetId),
    /// The open menu, a highlight or the submenu path changed: the host
    /// applies the new view ([`AppMenu::apply`]).
    Changed(AppMenuView),
}

impl WidgetAction for AppMenuEvent {
    /// An activation rides [`UiEvent::MenuActivated`] under the item's
    /// own id — the event a native menu reports too, so one handler
    /// covers both. A view change rides [`UiEvent::AppMenu`] under the
    /// menu's id.
    fn into_ui_event(self, id: WidgetId) -> UiEvent {
        match self {
            AppMenuEvent::Activated(item) => UiEvent::MenuActivated(item),
            changed => UiEvent::AppMenu(id, changed),
        }
    }
}

/// [`Interactive::State`] for [`AppMenu`]: the open dropdown's typeahead
/// buffer — the deepest level's [`ContextMenuState`].
#[non_exhaustive]
#[derive(Debug, Default)]
pub struct AppMenuState {
    /// The deepest open level's typeahead state.
    pub menu: ContextMenuState,
}

/// Resolved widths of one menu level and, per item, of the submenu it
/// opens.
#[derive(Debug, Clone, PartialEq)]
struct LevelWidths {
    width: f32,
    children: Vec<Option<LevelWidths>>,
}

impl LevelWidths {
    fn of(ctx: &LayoutCtx, items: &[ContextMenuItem], lh: f32) -> Self {
        Self {
            width: level_width(ctx, items, lh),
            children: items
                .iter()
                .map(|item| item.submenu.as_ref().map(|sub| Self::of(ctx, sub, lh)))
                .collect(),
        }
    }
}

/// The metrics an [`AppMenu`] sizes its dropdown levels with, resolved
/// once per layout.
#[derive(Debug, Clone, PartialEq)]
struct LevelMetrics {
    line_height: f32,
    /// The border ring a cell grid paints outside each level's items.
    inset: f32,
    item_height: f32,
    separator_height: f32,
    /// Per top-level menu.
    widths: Vec<LevelWidths>,
}

impl LevelMetrics {
    /// The width of the level reached by `path` inside menu `open`.
    fn width(&self, open: usize, path: &[usize]) -> f32 {
        let mut level = self.widths.get(open);
        for &i in path {
            level = level
                .and_then(|l| l.children.get(i))
                .and_then(Option::as_ref);
        }
        level.map_or(self.line_height * 8.0, |l| l.width)
    }

    /// The box a level of `items` takes, border ring included.
    fn size(&self, width: f32, items: &[ContextMenuItem]) -> Size {
        let height: f32 = items
            .iter()
            .map(|i| {
                if i.is_separator() {
                    self.separator_height
                } else {
                    self.item_height
                }
            })
            .sum();
        Size::new(width + self.inset * 2.0, height + self.inset * 2.0)
    }
}

/// [`AppMenu`]'s resolved layout: the bar's own layout plus the metrics
/// its dropdown levels are sized with.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub struct AppMenuLayout {
    /// The bar, as [`MenuBar`]'s own layout resolves it.
    pub bar: MenuBarPrimitiveLayout,
    metrics: LevelMetrics,
}

/// One open level of the cascade: the menu it shows and where it attaches.
struct LevelSpec {
    menu: ContextMenu,
    anchor: Rect,
    side: PopupSide,
    size: Size,
}

impl AppMenu {
    /// A closed menu bar over `menus`.
    pub fn new(id: WidgetId, menus: Vec<MenuDef>) -> Self {
        Self {
            id,
            menus,
            view: AppMenuView::default(),
            native: false,
            typeahead: true,
        }
    }

    /// Whether a dropdown is open.
    pub fn is_open(&self) -> bool {
        self.view.open_item.is_some()
    }

    /// Apply what this menu reported: a view change replaces
    /// [`Self::view`]; an activation changes nothing here (the menu has
    /// already reported itself closed).
    pub fn apply(&mut self, event: &AppMenuEvent) {
        if let AppMenuEvent::Changed(view) = event {
            self.view = view.clone();
        }
    }

    /// The bar this menu paints, with the open and focused items from
    /// [`Self::view`]. Each item carries its menu's dropdown items as
    /// `submenu`, which a native installer bakes into the platform menu
    /// and the bar's accessibility tree walks.
    pub fn build_menu_bar(&self) -> MenuBar {
        MenuBar {
            id: self.id,
            items: self
                .menus
                .iter()
                .map(|m| MenuBarItem {
                    id: m.id,
                    label: m.label.clone(),
                    disabled: m.disabled,
                    submenu: Some(m.items.clone()),
                })
                .collect(),
            open_item: self.view.open_item,
            focused_item: self.view.focused_item,
        }
    }

    /// The id of the cascade level at `depth`.
    fn level_id(&self, depth: usize) -> WidgetId {
        if depth == 0 {
            WidgetId::new(format!("{}:dropdown", self.id.as_str()))
        } else {
            WidgetId::new(format!("{}:submenu-{}", self.id.as_str(), depth - 1))
        }
    }

    /// The root dropdown of menu `menu_idx`, highlighting
    /// [`AppMenuView::dropdown_selected`].
    fn build_dropdown(&self, menu_idx: usize) -> ContextMenu {
        ContextMenu {
            id: self.level_id(0),
            items: self.menus[menu_idx].items.clone(),
            selected_idx: self.view.dropdown_selected,
            bg: None,
            placement: ContextMenuPlacement::Below,
            glyphs: Default::default(),
        }
    }

    /// Drop `open_item`/`submenu_path` levels that no longer have a
    /// corresponding entry in `menus`, so a host rebuilding `menus` from
    /// per-frame data (and handing over a shorter list while a dropdown is
    /// open) never leaves an index pointing past the end.
    fn clamp_open_state(&mut self) {
        let Some(open_idx) = self.view.open_item else {
            return;
        };
        if open_idx >= self.menus.len() {
            self.close();
            return;
        }
        let mut items = &self.menus[open_idx].items;
        let mut valid_depth = self.view.submenu_path.len();
        for (depth, &path_idx) in self.view.submenu_path.iter().enumerate() {
            match items.get(path_idx).and_then(|item| item.submenu.as_ref()) {
                Some(sub) => items = sub,
                None => {
                    valid_depth = depth;
                    break;
                }
            }
        }
        self.view.submenu_path.truncate(valid_depth);
        self.view.submenu_selected.truncate(valid_depth);
    }

    /// Close every level and clear the bar's focus.
    fn close(&mut self) {
        self.view = AppMenuView::default();
    }

    /// Open menu `idx`'s dropdown, highlighting its first selectable item.
    fn open_menu(&mut self, idx: usize) {
        self.view = AppMenuView {
            open_item: Some(idx),
            focused_item: Some(idx),
            ..AppMenuView::default()
        };
        self.view.dropdown_selected = self.build_dropdown(idx).first_selectable();
    }

    /// Open the submenu of `item_idx` at the current deepest level,
    /// highlighting its first selectable item.
    fn open_submenu(&mut self, item_idx: usize) {
        let depth = self.view.submenu_path.len();
        let Some(sub_items) = self
            .items_at_depth(depth)
            .and_then(|items| items.into_iter().nth(item_idx))
            .and_then(|item| item.submenu)
        else {
            return;
        };
        let first_sel = sub_items
            .iter()
            .position(ContextMenuItem::is_selectable)
            .unwrap_or(0);
        self.view.submenu_path.push(item_idx);
        self.view.submenu_selected.push(first_sel);
    }

    /// Close the deepest open submenu (pop one level).
    fn close_deepest_submenu(&mut self) {
        self.view.submenu_path.pop();
        self.view.submenu_selected.pop();
    }

    /// Keep only the levels down to `depth` open.
    fn truncate_to(&mut self, depth: usize) {
        self.view.submenu_path.truncate(depth);
        self.view.submenu_selected.truncate(depth);
    }

    /// The next enabled menu `delta` steps from `from` in bar order,
    /// wrapping. `from` counts as navigable even when it is itself
    /// disabled, so a move off a disabled menu steps to its neighbour
    /// instead of seeding at either end of the bar.
    fn next_enabled_menu(&self, from: usize, delta: isize) -> usize {
        let key = if delta >= 0 { NavKey::Down } else { NavKey::Up };
        FlatNav::mv(self.menus.len(), Some(from), key, true, |i| {
            i == from || !self.menus[i].disabled
        })
        .unwrap_or(from)
    }

    /// Switch the open dropdown to the menu `delta` steps along the bar.
    fn switch_menu(&mut self, delta: isize) {
        if let Some(cur) = self.view.open_item {
            let next = self.next_enabled_menu(cur, delta);
            self.open_menu(next);
        }
    }

    /// The highlighted item at the deepest open level.
    #[cfg(test)]
    fn current_selected(&self) -> usize {
        self.selected_at(self.view.submenu_path.len())
    }

    /// The highlighted item at `depth`.
    fn selected_at(&self, depth: usize) -> usize {
        if depth == 0 {
            self.view.dropdown_selected
        } else {
            self.view
                .submenu_selected
                .get(depth - 1)
                .copied()
                .unwrap_or(0)
        }
    }

    /// Highlight `idx` at `depth`.
    fn select_at(&mut self, depth: usize, idx: usize) {
        if depth == 0 {
            self.view.dropdown_selected = idx;
        } else if let Some(s) = self.view.submenu_selected.get_mut(depth - 1) {
            *s = idx;
        }
    }

    /// The items at `depth` (0 = the root dropdown), or `None` when no
    /// menu is open or the path does not resolve.
    fn items_at_depth(&self, depth: usize) -> Option<Vec<ContextMenuItem>> {
        let open_idx = self.view.open_item?;
        let mut items = self.menus.get(open_idx)?.items.clone();
        for &path_idx in self.view.submenu_path.iter().take(depth) {
            items = items.into_iter().nth(path_idx)?.submenu?;
        }
        Some(items)
    }

    /// Move the deepest level's highlight one selectable item, wrapping.
    /// The current item counts as navigable even on a separator or a
    /// disabled item, so the move steps to its neighbour in list order.
    #[cfg(test)]
    fn move_selection(&mut self, delta: i32) {
        let depth = self.view.submenu_path.len();
        let current = self.current_selected();
        let Some(items) = self.items_at_depth(depth) else {
            return;
        };
        let key = if delta >= 0 { NavKey::Down } else { NavKey::Up };
        let new_sel = FlatNav::mv(items.len(), Some(current), key, true, |i| {
            i == current || items[i].is_selectable()
        })
        .unwrap_or(current);
        self.select_at(depth, new_sel);
    }

    /// The [`ContextMenu`] the level at `depth` shows.
    fn level_menu(&self, depth: usize) -> Option<ContextMenu> {
        let items = self.items_at_depth(depth)?;
        if depth > self.view.submenu_path.len() {
            return None;
        }
        Some(ContextMenu {
            id: self.level_id(depth),
            items,
            selected_idx: self.selected_at(depth),
            bg: None,
            placement: if depth == 0 {
                ContextMenuPlacement::Below
            } else {
                ContextMenuPlacement::AnchorPoint
            },
            glyphs: Default::default(),
        })
    }

    /// The root dropdown's spec, anchored under the open bar item.
    fn root_spec(&self, layout: &AppMenuLayout) -> Option<LevelSpec> {
        if self.native {
            return None;
        }
        let open = self.view.open_item?;
        let menu = self.level_menu(0)?;
        if menu.items.is_empty() {
            return None;
        }
        let raw = layout.bar.layout.visible_items.get(open)?.bounds;
        let area = layout.bar.area;
        let m = &layout.metrics;
        let pad = (m.line_height * 0.15).max(1.0);
        let anchor = Rect::new(
            raw.x - area.x + pad - m.inset,
            raw.y - area.y,
            raw.width,
            raw.height + pad - m.inset,
        );
        let size = m.size(m.width(open, &[]), &menu.items);
        Some(LevelSpec {
            menu,
            anchor,
            side: PopupSide::Below,
            size,
        })
    }

    /// The spec of the level below `depth`, anchored beside the parent
    /// row in the parent level's own local space, or `None` when no
    /// submenu is open there.
    fn child_spec(
        &self,
        depth: usize,
        parent: &ContextMenuPrimitiveLayout,
        metrics: &LevelMetrics,
    ) -> Option<LevelSpec> {
        let open = self.view.open_item?;
        let parent_row = *self.view.submenu_path.get(depth)?;
        let menu = self.level_menu(depth + 1)?;
        let row = parent
            .layout
            .visible_items
            .iter()
            .find(|v| v.item_idx == parent_row)?
            .bounds;
        let inset = metrics.inset;
        let anchor = Rect::new(
            row.x - parent.area.x,
            row.y - parent.area.y - inset,
            row.width + inset,
            row.height,
        );
        let size = metrics.size(
            metrics.width(open, &self.view.submenu_path[..=depth]),
            &menu.items,
        );
        Some(LevelSpec {
            menu,
            anchor,
            side: PopupSide::After,
            size,
        })
    }

    /// The popup a level spec becomes.
    fn level_popup(&self, depth: usize, spec: LevelSpec, metrics: &LevelMetrics) -> Popup {
        let content = AppMenuLevel {
            menu: self.clone(),
            depth,
            level: spec.menu,
            metrics: metrics.clone(),
        };
        Popup::new(PopupKey(depth as u64), spec.anchor, spec.size, content).side(spec.side)
    }

    /// Resolve one keystroke while a dropdown is open, against `next` —
    /// a copy of this menu the keystroke's transitions are applied to.
    fn key_while_open(
        &self,
        next: &mut AppMenu,
        state: &mut AppMenuState,
        key: &KeyEvent,
    ) -> Handled<AppMenuEvent> {
        if let Key::Char(ch) = key.key {
            if key.modifiers.alt {
                let bar = self.build_menu_bar();
                return match bar.find_alt_target(ch) {
                    Some(idx) if Some(idx) == self.view.open_item => {
                        next.close();
                        Handled::Consumed
                    }
                    Some(idx) => {
                        next.open_menu(idx);
                        Handled::Consumed
                    }
                    None => Handled::Ignored,
                };
            }
        }
        if matches!(key.key, Key::Named(NamedKey::Escape)) {
            if next.view.submenu_path.is_empty() {
                next.close();
            } else {
                next.close_deepest_submenu();
            }
            return Handled::Consumed;
        }
        if let Key::Char(_) = key.key {
            let accelerator = key.modifiers.ctrl || key.modifiers.cmd;
            if !self.typeahead && !accelerator {
                return Handled::Ignored;
            }
        }
        let depth = self.view.submenu_path.len();
        let Some(level) = self.level_menu(depth) else {
            return Handled::Ignored;
        };
        let page = level.items.len().max(1);
        match level.handle_key(key, page, &mut state.menu) {
            Handled::Event(ContextMenuEvent::SelectionChanged { idx }) => {
                next.select_at(depth, idx);
                Handled::Consumed
            }
            Handled::Event(ContextMenuEvent::SubmenuOpened { idx }) => {
                next.open_submenu(idx);
                Handled::Consumed
            }
            Handled::Event(ContextMenuEvent::SubmenuClosed) => {
                if depth > 0 {
                    next.close_deepest_submenu();
                } else {
                    next.switch_menu(-1);
                }
                Handled::Consumed
            }
            Handled::Event(ContextMenuEvent::Activated { item }) => {
                next.close();
                Handled::Event(AppMenuEvent::Activated(item))
            }
            Handled::Event(ContextMenuEvent::Dismissed) => {
                next.close();
                Handled::Consumed
            }
            Handled::Consumed => {
                // Enter on a row with nothing to fire closes the menu.
                if matches!(key.key, Key::Named(NamedKey::Enter)) {
                    next.close();
                }
                Handled::Consumed
            }
            Handled::Ignored => match key.key {
                // Right on a leaf of the root dropdown walks the bar.
                Key::Named(NamedKey::Right) if depth == 0 => {
                    next.switch_menu(1);
                    Handled::Consumed
                }
                _ => Handled::Ignored,
            },
        }
    }

    /// Resolve an event that landed on the level at `depth`.
    fn level_event(
        &self,
        next: &mut AppMenu,
        depth: usize,
        event: &RoutedEvent,
    ) -> Handled<AppMenuEvent> {
        let RoutedEvent::Pointer { kind, sub, .. } = event else {
            return Handled::Consumed;
        };
        let items = self.items_at_depth(depth).unwrap_or_default();
        let item = sub.and_then(|s| items.get(s.0 as usize).map(|item| (s.0 as usize, item)));
        match (kind, item) {
            // Hover moves the highlight and opens a submenu parent's child
            // at once; hovering a leaf closes anything opened deeper.
            (PointerKind::Move, Some((idx, item))) if item.is_selectable() => {
                next.select_at(depth, idx);
                if item.submenu.is_some() {
                    if self.view.submenu_path.get(depth) != Some(&idx) {
                        next.truncate_to(depth);
                        next.open_submenu(idx);
                    }
                } else {
                    next.truncate_to(depth);
                }
                if next.view == self.view {
                    Handled::Ignored
                } else {
                    Handled::Consumed
                }
            }
            (PointerKind::Down(MouseButton::Left), Some((idx, item))) => {
                if !item.is_selectable() {
                    Handled::Consumed
                } else if item.submenu.is_some() {
                    next.truncate_to(depth);
                    next.open_submenu(idx);
                    Handled::Consumed
                } else if let Some(id) = item.id {
                    next.close();
                    Handled::Event(AppMenuEvent::Activated(id))
                } else {
                    Handled::Consumed
                }
            }
            (PointerKind::Move, _) => Handled::Ignored,
            _ => Handled::Consumed,
        }
    }

    /// Resolve a pointer event on the bar itself.
    fn bar_event(
        &self,
        next: &mut AppMenu,
        event: &RoutedEvent,
        layout: &AppMenuLayout,
        cx: &mut EventCx,
    ) -> Handled<AppMenuEvent> {
        let RoutedEvent::Pointer { kind, local, .. } = event else {
            return Handled::Ignored;
        };
        let hit = layout.bar.hit_test_local(local.x, local.y);
        let on_label = matches!(hit, MenuBarHit::Item(_));
        match kind {
            // While closed, hover tracks the focused item and leaving the
            // items clears it. A move off the labels stays unhandled so it
            // also reaches whatever sits under the pointer.
            PointerKind::Move if !self.is_open() => {
                let focus = match hit {
                    MenuBarHit::Item(i) if !self.menus[i].disabled => Some(i),
                    _ => None,
                };
                next.view.focused_item = focus;
                if focus.is_none() || focus == self.view.focused_item {
                    Handled::Ignored
                } else {
                    Handled::Consumed
                }
            }
            // While closed, the strip past and between the labels is not
            // the menu's: the host's own chrome sits there.
            _ if !self.is_open() && !on_label => Handled::Ignored,
            // A press on the strip's background closes an open menu.
            PointerKind::Down(MouseButton::Left) if hit == MenuBarHit::Bar && self.is_open() => {
                next.close();
                Handled::Consumed
            }
            _ => {
                let bar = self.build_menu_bar();
                match Interactive::handle(&bar, &mut (), event, &layout.bar, cx) {
                    Handled::Event(MenuBarEvent::FocusMoved { idx }) => {
                        next.view.focused_item = Some(idx);
                        Handled::Consumed
                    }
                    Handled::Event(MenuBarEvent::MenuOpened { idx, .. }) => {
                        next.open_menu(idx);
                        Handled::Consumed
                    }
                    Handled::Event(MenuBarEvent::Closed) => {
                        next.close();
                        Handled::Consumed
                    }
                    Handled::Consumed => Handled::Consumed,
                    Handled::Ignored => Handled::Ignored,
                }
            }
        }
    }
}

impl A11yPrimitive for AppMenu {
    fn a11y_info(&self) -> A11yInfo {
        self.build_menu_bar().a11y_info()
    }

    fn a11y_children(&self) -> Vec<A11yChild> {
        self.build_menu_bar().a11y_children()
    }
}

impl Primitive for AppMenu {
    type Layout = AppMenuLayout;
    type Event = AppMenuEvent;

    fn id(&self) -> Option<&WidgetId> {
        Some(&self.id)
    }

    /// Under [`ResolvedMenuStyle::Native`] the bar is installed as the
    /// platform's own menu bar and nothing paints in the window.
    fn native_presentation(&self, backend: &mut dyn Backend, rect: Rect) -> bool {
        Primitive::native_presentation(&self.build_menu_bar(), backend, rect)
    }

    fn capture(&self) -> crate::frame::OwnedContent {
        Primitive::capture(&self.build_menu_bar())
    }

    fn measure(&self, ctx: &LayoutCtx, avail: Size) -> crate::NaturalSize {
        Primitive::measure(&self.build_menu_bar(), ctx, avail)
    }

    /// The bar's own layout for `rect`, plus every dropdown level's width
    /// measured through `ctx`, so the cascade sizes itself from the same
    /// metrics the bar was laid out with.
    fn layout(&self, ctx: &LayoutCtx, rect: Rect) -> AppMenuLayout {
        let line_height = if ctx.line_height > 0.0 {
            ctx.line_height
        } else {
            1.0
        };
        let (item_height, separator_height) = item_heights(line_height);
        let inset = match ctx.units() {
            Units::Cells => CELL_BORDER_INSET,
            _ => 0.0,
        };
        AppMenuLayout {
            bar: Primitive::layout(&self.build_menu_bar(), ctx, rect),
            metrics: LevelMetrics {
                line_height,
                inset,
                item_height,
                separator_height,
                widths: self
                    .menus
                    .iter()
                    .map(|m| LevelWidths::of(ctx, &m.items, line_height))
                    .collect(),
            },
        }
    }

    /// The bar through [`MenuBar`]'s own painter; nothing under a native
    /// menu style. The open levels are popups the runtime paints.
    fn paint(
        &self,
        layout: &AppMenuLayout,
        surface: &mut dyn crate::paint_surface::PaintSurface,
        env: &crate::PaintEnv,
    ) {
        if self.native {
            return;
        }
        Primitive::paint(&self.build_menu_bar(), &layout.bar, surface, env);
    }

    #[cfg(feature = "tui")]
    fn paint_cells(
        &self,
        layout: &AppMenuLayout,
        cells: &mut dyn crate::CellSurface,
        env: &crate::PaintEnv,
    ) {
        if self.native {
            return;
        }
        Primitive::paint_cells(&self.build_menu_bar(), &layout.bar, cells, env);
    }
}

impl Interactive for AppMenu {
    type State = AppMenuState;
    type Action = AppMenuEvent;

    const OPENS_POPUPS: bool = true;

    /// One stop for the whole bar; none for an empty or native one.
    fn focus(&self) -> FocusSpec {
        if self.menus.is_empty() || self.native {
            FocusSpec::None
        } else {
            FocusSpec::Stop
        }
    }

    fn sub_regions(&self, layout: &AppMenuLayout) -> Vec<(Rect, SubTarget)> {
        Interactive::sub_regions(&self.build_menu_bar(), &layout.bar)
    }

    fn a11y_actions(&self, _state: &AppMenuState) -> Vec<A11yAction> {
        Interactive::a11y_actions(&self.build_menu_bar(), &())
    }

    /// The bar's clicks, hover and keys go through [`MenuBar`]; an event
    /// on an open level arrives as a popup event naming the row under the
    /// pointer; keys while a dropdown is open go to the deepest level's
    /// [`ContextMenu::handle_key`]. A dismissal closes the dismissed level
    /// and everything below it.
    fn handle(
        &self,
        state: &mut AppMenuState,
        event: &RoutedEvent,
        layout: &AppMenuLayout,
        cx: &mut EventCx,
    ) -> Handled<AppMenuEvent> {
        if self.native {
            return Handled::Ignored;
        }
        let mut next = self.clone();
        let handled = match event {
            RoutedEvent::PopupEvent { popup, event } => {
                self.level_event(&mut next, popup.0 as usize, event)
            }
            RoutedEvent::PopupDismissed { popup, .. } => {
                match popup.0 as usize {
                    0 => next.close(),
                    depth => next.truncate_to(depth - 1),
                }
                Handled::Consumed
            }
            RoutedEvent::Key(key) if self.is_open() => self.key_while_open(&mut next, state, key),
            RoutedEvent::Key(_) | RoutedEvent::Action(_) => {
                let bar = self.build_menu_bar();
                match Interactive::handle(&bar, &mut (), event, &layout.bar, cx) {
                    Handled::Event(MenuBarEvent::FocusMoved { idx }) => {
                        next.view.focused_item = Some(idx);
                        Handled::Consumed
                    }
                    Handled::Event(MenuBarEvent::MenuOpened { idx, .. }) => {
                        next.open_menu(idx);
                        Handled::Consumed
                    }
                    Handled::Event(MenuBarEvent::Closed) => {
                        next.close();
                        Handled::Consumed
                    }
                    Handled::Consumed => Handled::Consumed,
                    Handled::Ignored => Handled::Ignored,
                }
            }
            RoutedEvent::Pointer { .. } => self.bar_event(&mut next, event, layout, cx),
            _ => Handled::Ignored,
        };
        if next.view == self.view {
            return handled;
        }
        let changed = AppMenuEvent::Changed(next.view);
        match handled {
            // An activation, or an event left to the host that still moved
            // the view (a hover leaving the labels), reports the view
            // change alongside.
            Handled::Event(_) | Handled::Ignored => {
                cx.emit(changed.into_ui_event(self.id));
                handled
            }
            Handled::Consumed => Handled::Event(changed),
        }
    }

    /// The open dropdown, under its bar item; its open submenus are the
    /// dropdown's own popups, beside their parent rows.
    fn popups(&self, _state: &AppMenuState, layout: &AppMenuLayout) -> Vec<Popup> {
        self.root_spec(layout)
            .map(|spec| self.level_popup(0, spec, &layout.metrics))
            .into_iter()
            .collect()
    }
}

/// One open level of an [`AppMenu`]'s cascade, as popup content: it paints
/// its [`ContextMenu`] and names that menu's rows; every event over it
/// reaches the owning `AppMenu`. A level whose submenu is open answers
/// that submenu as its own popup.
#[derive(Debug, Clone)]
struct AppMenuLevel {
    menu: AppMenu,
    depth: usize,
    level: ContextMenu,
    metrics: LevelMetrics,
}

impl A11yPrimitive for AppMenuLevel {
    fn a11y_info(&self) -> A11yInfo {
        self.level.a11y_info()
    }

    fn a11y_children(&self) -> Vec<A11yChild> {
        self.level.a11y_children()
    }
}

impl Primitive for AppMenuLevel {
    type Layout = ContextMenuPrimitiveLayout;
    type Event = ();

    fn id(&self) -> Option<&WidgetId> {
        Some(&self.level.id)
    }

    fn paint_bleed(&self, style: &crate::Style) -> f32 {
        Primitive::paint_bleed(&self.level, style)
    }

    fn layout(&self, ctx: &LayoutCtx, rect: Rect) -> ContextMenuPrimitiveLayout {
        Primitive::layout(&self.level, ctx, rect)
    }

    fn paint(
        &self,
        layout: &ContextMenuPrimitiveLayout,
        surface: &mut dyn crate::paint_surface::PaintSurface,
        env: &crate::PaintEnv,
    ) {
        Primitive::paint(&self.level, layout, surface, env);
    }

    #[cfg(feature = "tui")]
    fn paint_cells(
        &self,
        layout: &ContextMenuPrimitiveLayout,
        cells: &mut dyn crate::CellSurface,
        env: &crate::PaintEnv,
    ) {
        Primitive::paint_cells(&self.level, layout, cells, env);
    }
}

impl Interactive for AppMenuLevel {
    type State = ();
    type Action = Never;

    const OPENS_POPUPS: bool = true;

    fn handle(
        &self,
        _state: &mut (),
        _event: &RoutedEvent,
        _layout: &ContextMenuPrimitiveLayout,
        _cx: &mut EventCx,
    ) -> Handled<Never> {
        Handled::Ignored
    }

    fn sub_regions(&self, layout: &ContextMenuPrimitiveLayout) -> Vec<(Rect, SubTarget)> {
        Interactive::sub_regions(&self.level, layout)
    }

    fn popups(&self, _state: &(), layout: &ContextMenuPrimitiveLayout) -> Vec<Popup> {
        self.menu
            .child_spec(self.depth, layout, &self.metrics)
            .map(|spec| self.menu.level_popup(self.depth + 1, spec, &self.metrics))
            .into_iter()
            .collect()
    }
}

// ── Host API ──────────────────────────────────────────────────────────────

/// Driving an [`AppMenu`] from a host whose events do not pass through the
/// runtime's routing engine — one that calls it with raw [`UiEvent`]s and
/// the rect it paints the bar at. The host keeps the menu and its
/// [`AppMenuState`]; [`AppMenu::handle_ui_event`] applies every view change
/// itself, [`AppMenu::render`] paints the bar and the open cascade, and the
/// open dropdown sits on the backend's [`crate::ModalStack`] so other
/// modal-aware code gives it hit-test precedence.
impl AppMenu {
    /// Replace the menu definitions. Any open level whose index no longer
    /// resolves against `menus` is dropped, so a host rebuilding `menus`
    /// from per-frame application data may hand this a shorter list while
    /// the previous frame's dropdown is still open.
    pub fn set_menus(&mut self, menus: Vec<MenuDef>) {
        self.menus = menus;
        self.clamp_open_state();
    }

    /// Close every level, and take the dropdown off `backend`'s modal
    /// stack.
    pub fn dismiss(&mut self, backend: &mut dyn Backend) {
        self.close();
        backend
            .modal_stack_handle()
            .borrow_mut()
            .pop(&self.level_id(0));
    }

    /// This menu as `backend` presents it: [`AppMenu::native`] follows the
    /// backend's effective menu style.
    fn presented(&self, backend: &dyn Backend) -> AppMenu {
        AppMenu {
            native: backend.effective_menu_style() == ResolvedMenuStyle::Native,
            ..self.clone()
        }
    }

    /// Every open level and where it sits for `bar_rect`, root first —
    /// resolved through the same popup specs and placement the runtime
    /// gives a `Surface::Widget` menu.
    fn levels(
        &self,
        backend: &dyn Backend,
        bar_rect: Rect,
    ) -> Vec<(ContextMenu, ContextMenuPrimitiveLayout)> {
        let menu = self.presented(backend);
        let layout = backend.layout_primitive(bar_rect, &menu);
        let viewport = crate::backend::viewport_rect(backend.viewport());
        let mut out = Vec::new();
        let mut owner = bar_rect;
        let mut spec = menu.root_spec(&layout);
        let mut depth = 0;
        while let Some(s) = spec {
            let level = s.menu.clone();
            let rect = menu
                .level_popup(depth, s, &layout.metrics)
                .resolve(owner, viewport);
            let level_layout = backend.layout_primitive(rect, &level);
            spec = menu.child_spec(depth, &level_layout, &layout.metrics);
            out.push((level, level_layout));
            owner = rect;
            depth += 1;
        }
        out
    }

    /// Every open level with its placed layout, root first.
    fn dropdown_stack(
        &self,
        backend: &dyn Backend,
        bar_rect: Rect,
    ) -> Vec<(ContextMenu, ContextMenuLayout)> {
        self.levels(backend, bar_rect)
            .into_iter()
            .map(|(menu, layout)| (menu, layout.layout))
            .collect()
    }

    /// Keep the open dropdown's modal-stack entry in step with a view
    /// change from `was`.
    fn sync_modal(&self, was: &AppMenuView, backend: &mut dyn Backend, bar_rect: Rect) {
        if was.open_item == self.view.open_item {
            return;
        }
        let id = self.level_id(0);
        backend.modal_stack_handle().borrow_mut().pop(&id);
        if let Some((_, layout)) = self.dropdown_stack(backend, bar_rect).into_iter().next() {
            backend
                .modal_stack_handle()
                .borrow_mut()
                .push(id, layout.bounds);
        }
    }

    /// Draw the menu bar and any open dropdown (including nested
    /// submenus) at `bar_rect`.
    ///
    /// Under [`ResolvedMenuStyle::Native`] the bar is installed as the
    /// platform's own menu bar through [`Backend::install_menu_bar`]
    /// (which skips an unchanged bar, so this runs every frame) and
    /// nothing paints in the window; otherwise the strip paints through
    /// the widget contract, then each open level through
    /// [`ContextMenu`]'s.
    pub fn render(&self, backend: &mut dyn Backend, bar_rect: Rect) {
        let _ = self.build_menu_bar().present(backend, bar_rect);
        for (menu, layout) in self.dropdown_stack(backend, bar_rect) {
            paint_placed_menu(backend, &menu, &layout);
        }
    }

    /// The routed event `event` becomes for the menu at `bar_rect`, or
    /// `None` when it is not the menu's: a press or hover on an open level
    /// becomes that level's popup event, a press outside everything while
    /// open dismisses the cascade, Escape dismisses the deepest open
    /// level, and the Alt mnemonics reach the bar while it is closed — the
    /// routing the runtime performs for a `Surface::Widget` menu that
    /// holds focus.
    ///
    /// A release outside everything also dismisses: some terminal and
    /// multiplexer combinations drop `MouseDown(Left)` and deliver only
    /// `MouseUp(Left)` for a click. A release anywhere else belongs to the
    /// press that preceded it.
    fn route(&self, event: &UiEvent, backend: &dyn Backend, bar_rect: Rect) -> Option<RoutedEvent> {
        let open = self.is_open();
        let pointer = |kind: PointerKind, position: Point| -> Option<RoutedEvent> {
            for (depth, (menu, layout)) in self.levels(backend, bar_rect).iter().enumerate().rev() {
                let rect = layout.area;
                if !rect.contains(position) {
                    continue;
                }
                if matches!(kind, PointerKind::Up(_)) {
                    return None;
                }
                let local = Point::new(position.x - rect.x, position.y - rect.y);
                let sub = Interactive::sub_regions(menu, layout)
                    .into_iter()
                    .find(|(r, _)| r.contains(local))
                    .map(|(_, sub)| sub);
                return Some(RoutedEvent::PopupEvent {
                    popup: PopupKey(depth as u64),
                    event: Box::new(RoutedEvent::Pointer {
                        kind,
                        sub,
                        local,
                        mods: crate::Modifiers::default(),
                    }),
                });
            }
            let layout = backend.layout_primitive(bar_rect, &self.presented(backend));
            let on_bar = !matches!(
                layout.bar.layout.hit_test(position.x, position.y),
                MenuBarHit::Outside
            );
            match kind {
                PointerKind::Move => {}
                PointerKind::Down(_) | PointerKind::Up(_) if !on_bar && open => {
                    return Some(RoutedEvent::PopupDismissed {
                        popup: PopupKey(0),
                        reason: DismissReason::OutsideClick,
                    });
                }
                PointerKind::Up(_) => return None,
                _ if !on_bar => return None,
                _ => {}
            }
            Some(RoutedEvent::Pointer {
                kind,
                sub: None,
                local: Point::new(position.x - bar_rect.x, position.y - bar_rect.y),
                mods: crate::Modifiers::default(),
            })
        };
        match event {
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Escape),
                ..
            } if open => Some(RoutedEvent::PopupDismissed {
                popup: PopupKey(self.view.submenu_path.len() as u64),
                reason: DismissReason::Escape,
            }),
            // While closed, only the Alt mnemonics reach the menu: every
            // other key belongs to the app.
            UiEvent::KeyPressed { key, modifiers, .. }
                if open || (modifiers.alt && matches!(key, Key::Char(_))) =>
            {
                widget_host::routed_event(event, bar_rect, &[])
            }
            UiEvent::MouseDown {
                button: MouseButton::Left,
                position,
                ..
            } => pointer(PointerKind::Down(MouseButton::Left), *position),
            UiEvent::MouseUp {
                button: MouseButton::Left,
                position,
                ..
            } => pointer(PointerKind::Up(MouseButton::Left), *position),
            UiEvent::MouseMoved { position, .. } => pointer(PointerKind::Move, *position),
            _ => None,
        }
    }

    /// Process one raw event for the menu painted at `bar_rect`, applying
    /// any view change to `self`. Call from the host's event handler
    /// before its other routing.
    ///
    /// Answers [`AppMenuEvent::Activated`] for an activated item,
    /// [`AppMenuEvent::Changed`] (already applied) when the open menu, a
    /// highlight or the submenu path moved, `Consumed` for an event the
    /// menu took without a visible change, and `Ignored` for one that is
    /// not the menu's.
    ///
    /// A native menu bar's activation arrives as
    /// [`UiEvent::MenuActivated`] (AppKit reports a click there, not a hit
    /// on `bar_rect`); it answers the same [`AppMenuEvent::Activated`] the
    /// painted path produces, so one match arm covers both.
    ///
    /// While no dropdown is open, a pointer event on the strip outside its
    /// labels is `Ignored`, so the host's own chrome in that band receives
    /// it. A move there still clears the label hover: the host that
    /// repaints only for a handled event compares [`AppMenu::view`] across
    /// the call to catch that.
    pub fn handle_ui_event(
        &mut self,
        state: &mut AppMenuState,
        event: &UiEvent,
        backend: &mut dyn Backend,
        bar_rect: Rect,
    ) -> Handled<AppMenuEvent> {
        if let UiEvent::MenuActivated(id) = event {
            return Handled::Event(AppMenuEvent::Activated(*id));
        }
        self.native = backend.effective_menu_style() == ResolvedMenuStyle::Native;
        if self.native {
            return Handled::Ignored;
        }
        let Some(routed) = self.route(event, backend, bar_rect) else {
            return Handled::Ignored;
        };
        let layout = backend.layout_primitive(bar_rect, &*self);
        let units = crate::backend::primitive_layout_ctx(backend).units();
        let driven = widget_host::drive(&*self, self.id, state, &routed, &layout, units);
        let was = self.view.clone();
        for emitted in driven.emitted {
            if let UiEvent::AppMenu(_, change) = emitted {
                self.apply(&change);
            }
        }
        if let Handled::Event(change @ AppMenuEvent::Changed(_)) = &driven.handled {
            self.apply(change);
        }
        self.sync_modal(&was, backend, bar_rect);
        driven.handled
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::context_menu::ContextMenuItem;
    use crate::types::StyledText;
    use crate::Modifiers;

    /// What one [`AppMenu::handle_ui_event`] call amounted to, with a
    /// view change split into an open/close or submenu move and a
    /// highlight move.
    #[derive(Debug, PartialEq)]
    enum Outcome {
        Activated(WidgetId),
        StateChanged,
        Consumed,
        Ignored,
    }

    /// A host driving an [`AppMenu`] with raw events.
    struct Ms {
        menu: AppMenu,
        state: AppMenuState,
        view_changed: bool,
    }

    impl Ms {
        fn new(menus: Vec<MenuDef>) -> Self {
            Self {
                menu: AppMenu::new(WidgetId::new("menu-system-bar"), menus),
                state: AppMenuState::default(),
                view_changed: false,
            }
        }

        fn handle(
            &mut self,
            event: &UiEvent,
            backend: &mut dyn Backend,
            bar_rect: Rect,
        ) -> Outcome {
            let was = self.menu.view.clone();
            let handled = self
                .menu
                .handle_ui_event(&mut self.state, event, backend, bar_rect);
            self.view_changed = was != self.menu.view;
            match handled {
                Handled::Event(AppMenuEvent::Activated(id)) => Outcome::Activated(id),
                Handled::Event(AppMenuEvent::Changed(view)) => {
                    assert_eq!(view, self.menu.view, "the reported view is applied");
                    if was.open_item != view.open_item || was.submenu_path != view.submenu_path {
                        Outcome::StateChanged
                    } else {
                        Outcome::Consumed
                    }
                }
                Handled::Consumed => Outcome::Consumed,
                _ => Outcome::Ignored,
            }
        }

        /// Open menu `idx` and register its dropdown on the modal stack.
        fn open_menu(&mut self, idx: usize, backend: &mut dyn Backend, bar_rect: Rect) {
            let was = self.menu.view.clone();
            self.menu.open_menu(idx);
            self.menu.sync_modal(&was, backend, bar_rect);
        }

        fn dropdown_layout(
            &self,
            backend: &dyn Backend,
            bar_rect: Rect,
        ) -> Option<(ContextMenu, ContextMenuLayout)> {
            self.menu
                .dropdown_stack(backend, bar_rect)
                .into_iter()
                .next()
        }
    }

    // ── Minimal mock backend for handle()-level tests ─────────────────────────

    struct MockBackend {
        modal_stack: std::rc::Rc<std::cell::RefCell<crate::ModalStack>>,
        drag_state: std::rc::Rc<std::cell::RefCell<crate::DragState>>,
        focus_tree: crate::focus::FocusTree,
        /// Declares `BackendCaps::native_menu`, so the trait's default
        /// `MenuStyle::Auto` resolves `ResolvedMenuStyle::Native` and
        /// the menu-bar layout reports the zero-height, item-less shape
        /// a platform-owned menu bar has in-window (macOS, by default).
        native_menu_bar: bool,
        invalidation_log: crate::backend::InvalidationLog,
        services: crate::testing::NullPlatformServices,
    }

    impl MockBackend {
        fn new() -> Self {
            Self {
                modal_stack: std::rc::Rc::new(std::cell::RefCell::new(crate::ModalStack::new())),
                drag_state: std::rc::Rc::new(std::cell::RefCell::new(crate::DragState::new())),
                focus_tree: crate::focus::FocusTree::new(),
                native_menu_bar: false,
                invalidation_log: crate::backend::InvalidationLog::new(),
                services: crate::testing::NullPlatformServices::new(),
            }
        }

        /// A backend whose menu bar is the platform's own: `native_menu`
        /// capability, and an in-window layout with nothing in it.
        fn with_native_menu_bar() -> Self {
            Self {
                native_menu_bar: true,
                ..Self::new()
            }
        }
    }

    impl crate::backend::sealed::Sealed for MockBackend {}

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
        fn waker(&self) -> std::sync::Arc<dyn Fn(crate::UserPayload) + Send + Sync> {
            // No event loop for a wake to reach — matches `poll_events`/
            // `wait_events` above.
            std::sync::Arc::new(|_payload| {})
        }
        fn request_frame_in(&self, _delay: std::time::Duration) {
            // No event loop to wake, same rationale as `waker` above.
        }
        fn register_accelerator(&mut self, _: &crate::accelerator::Accelerator) {}
        fn unregister_accelerator(&mut self, _: &crate::accelerator::AcceleratorId) {}
        fn invalidate_rect(&mut self, rect: crate::event::Rect) {
            self.invalidation_log.push(rect);
        }
        fn invalidation_log(&self) -> &crate::backend::InvalidationLog {
            &self.invalidation_log
        }
        fn modal_stack_handle(&self) -> std::rc::Rc<std::cell::RefCell<crate::ModalStack>> {
            self.modal_stack.clone()
        }
        fn drag_state_handle(&self) -> std::rc::Rc<std::cell::RefCell<crate::DragState>> {
            self.drag_state.clone()
        }
        fn focus_tree(&self) -> &crate::focus::FocusTree {
            &self.focus_tree
        }
        fn set_focus(&mut self, id: Option<WidgetId>) -> bool {
            self.focus_tree.set_focus(id.map(|id| (id, None)))
        }
        fn draw_focus_ring(&mut self, _r: Rect) {}
        fn services(&self) -> &dyn crate::backend::PlatformServices {
            &self.services
        }
        fn backend_caps(&self) -> crate::backend::BackendCaps {
            crate::backend::BackendCaps {
                native_menu: self.native_menu_bar,
                ..crate::backend::BackendCaps::empty()
            }
        }
        fn line_height(&self) -> f32 {
            1.0
        }
        fn char_width(&self) -> f32 {
            1.0
        }
        fn list_char_width(&self) -> f32 {
            1.0
        }
        fn measure_text(&self, text: &str, _role: crate::FontRole) -> (f32, f32) {
            (text.chars().count() as f32, 1.0)
        }
        fn default_fonts(&self) -> crate::backend::PlatformFontDefaults {
            crate::backend::PlatformFontDefaults {
                editor_family: String::new(),
                editor_size_pt: 0.0,
                ui_family: String::new(),
                ui_size_pt: 0.0,
            }
        }
        fn draw_list(&mut self, _r: Rect, _l: &crate::ListView) {}
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
        fn draw_scrollbar(&mut self, _r: Rect, _s: &crate::Scrollbar) {}
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
    }

    /// A cell-grid ctx with no measurer, the metrics a TUI dropdown
    /// sizes against.
    fn cells_ctx() -> LayoutCtx<'static> {
        LayoutCtx::new(1.0, 1.0, false)
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

    /// `level_width` must track the real content of each item it's
    /// given: a menu with a much longer label must come back wider.
    #[test]
    fn level_width_tracks_the_longest_label() {
        let short = [action("a", "Undo")];
        let long = [action("a", "Undo Last 20 Changes")];

        let short_w = level_width(&cells_ctx(), &short, 1.0);
        let long_w = level_width(&cells_ctx(), &long, 1.0);
        assert!(
            long_w > short_w,
            "a longer label must widen the dropdown: short={short_w}, long={long_w}"
        );
    }

    /// A `checked` item's leading `✓ ` slot and a submenu item's trailing
    /// `▶` arrow must each widen the result beyond the bare label width —
    /// otherwise the glyph would overlap the label or get clipped.
    #[test]
    fn level_width_reserves_space_for_checked_prefix_and_submenu_arrow() {
        let plain = [action("a", "Word Wrap")];
        let mut checked = plain.clone();
        checked[0].checked = Some(true);
        let mut with_submenu = plain.clone();
        with_submenu[0].submenu = Some(vec![action("b", "On")]);

        let plain_w = level_width(&cells_ctx(), &plain, 1.0);
        let checked_w = level_width(&cells_ctx(), &checked, 1.0);
        let submenu_w = level_width(&cells_ctx(), &with_submenu, 1.0);

        assert!(checked_w > plain_w, "checked={checked_w}, plain={plain_w}");
        assert!(submenu_w > plain_w, "submenu={submenu_w}, plain={plain_w}");
    }

    /// An `icon`-only item (no `checked` state) reserves the same leading
    /// gutter width a `checked` item does — menu-bar dropdowns paint
    /// through this same `native_surface_paint` path, which prefixes an
    /// icon'd item's label with the same two-space gutter a checked
    /// item's `✓ ` uses, so a dropdown that doesn't reserve for `icon`
    /// would let that shifted label clip at the right edge.
    #[test]
    fn level_width_reserves_space_for_icon_same_as_checked() {
        let plain = [action("a", "Reveal in Explorer")];
        let mut with_icon = plain.clone();
        with_icon[0].icon = Some(crate::types::Icon::new("G", "F"));
        let mut checked = plain.clone();
        checked[0].checked = Some(true);

        let plain_w = level_width(&cells_ctx(), &plain, 1.0);
        let icon_w = level_width(&cells_ctx(), &with_icon, 1.0);
        let checked_w = level_width(&cells_ctx(), &checked, 1.0);

        assert!(icon_w > plain_w, "icon={icon_w}, plain={plain_w}");
        assert_eq!(
            icon_w, checked_w,
            "icon and checked reserve the same shared gutter width"
        );
    }

    /// Separators never gate the width computation on their own (empty)
    /// label — a menu that is otherwise all separators plus one real item
    /// must size off that one item, not panic or report `0.0`.
    #[test]
    fn level_width_ignores_separators() {
        let items = [separator(), action("a", "Save All"), separator()];
        let w = level_width(&cells_ctx(), &items, 1.0);
        assert!(w > 0.0);
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

    /// A native `NSMenu` activation (`UiEvent::MenuActivated`,
    /// what AppKit reports once `AppMenu::render` installs rather
    /// than paints) must translate to the same `AppMenuEvent::Activated` the
    /// painted dropdown path produces — same shape as
    /// `ContextMenuController` already uses for its own native event —
    /// so consumer code written against `AppMenuEvent::Activated` works
    /// unchanged under either `MenuStyle`. The menu need not be "open"
    /// in the menu's own sense at all; the native path has no
    /// in-widget open/dropdown state, only AppKit's own.
    #[test]
    fn handle_routes_native_menu_activated_to_menu_event_activated() {
        let mut ms = Ms::new(sample_menus());
        let mut backend = MockBackend::new();
        assert!(
            !ms.menu.is_open(),
            "native path never opens the struct's own dropdown state"
        );

        let ev = UiEvent::MenuActivated(WidgetId::new("save"));
        let result = ms.handle(&ev, &mut backend, bar_rect());

        assert_eq!(result, Outcome::Activated(WidgetId::new("save")));
    }

    fn alt_key_ev(c: char) -> UiEvent {
        UiEvent::KeyPressed {
            key: Key::Char(c),
            modifiers: Modifiers {
                alt: true,
                ..Modifiers::default()
            },
            repeat: false,
        }
    }

    /// On a backend that paints its own menu bar, the mnemonic
    /// underlines are real, so Alt+the access key opens that menu's
    /// in-window dropdown.
    #[test]
    fn alt_char_opens_the_painted_menu_bars_dropdown() {
        let mut ms = Ms::new(sample_menus());
        let mut backend = MockBackend::new();

        let result = ms.handle(&alt_key_ev('f'), &mut backend, bar_rect());

        assert_eq!(result, Outcome::StateChanged);
        assert_eq!(ms.menu.view.open_item, Some(0));
        assert!(
            ms.dropdown_layout(&backend, bar_rect()).is_some(),
            "an opened menu must have a dropdown to paint"
        );
    }

    /// With a platform-owned menu bar there are no painted mnemonic
    /// underlines and no in-window strip to anchor a dropdown under, so
    /// Alt+char is reported `Ignored` and leaves the system closed —
    /// rather than opening an invisible dropdown whose anchor item does
    /// not exist in the layout.
    #[test]
    fn alt_char_is_ignored_when_the_menu_bar_is_native() {
        let mut ms = Ms::new(sample_menus());
        let mut backend = MockBackend::with_native_menu_bar();
        assert_eq!(
            crate::backend::Backend::effective_menu_style(&backend),
            ResolvedMenuStyle::Native,
            "mock must resolve the native style for this to cover anything"
        );

        let result = ms.handle(&alt_key_ev('f'), &mut backend, bar_rect());

        assert_eq!(result, Outcome::Ignored);
        assert!(!ms.menu.is_open());
        assert_eq!(ms.menu.view.open_item, None);
    }

    /// A host may switch menu style at runtime (`Backend::set_menu_style`)
    /// while a painted dropdown is open. The next frame's layout then has
    /// no visible items at all, and both rendering and layout must
    /// degrade to "no dropdown" instead of indexing past that empty list.
    #[test]
    fn menu_bar_turning_native_while_open_paints_no_dropdown() {
        let mut ms = Ms::new(sample_menus());
        let mut backend = MockBackend::new();
        ms.handle(&alt_key_ev('f'), &mut backend, bar_rect());
        assert!(ms.menu.is_open());

        backend.native_menu_bar = true;

        ms.menu.render(&mut backend, bar_rect());
        assert!(
            ms.dropdown_layout(&backend, bar_rect()).is_none(),
            "no visible bar item to anchor under means no dropdown"
        );
        assert!(
            ms.menu.dropdown_stack(&backend, bar_rect()).is_empty(),
            "an empty root dropdown must not leave submenu levels behind"
        );
    }

    /// A `menu_bar` leaf dict — a serialised [`MenuBar`] — decodes as an
    /// [`AppMenu`]: each item's `submenu` becomes the menu's items, a
    /// `null` submenu decodes as none, and the open/focused items carry
    /// over.
    #[test]
    fn a_serialised_menu_bar_decodes_as_an_app_menu() {
        let mut bar = Ms::new(sample_menus()).menu.build_menu_bar();
        bar.items[2].submenu = None;
        bar.open_item = Some(1);
        bar.focused_item = Some(1);
        let json = serde_json::to_value(&bar).expect("serialise");
        let menu: AppMenu = serde_json::from_value(json).expect("decode");
        assert_eq!(menu.id, bar.id);
        assert_eq!(menu.menus[0].items, sample_menus()[0].items);
        assert!(menu.menus[2].items.is_empty());
        assert!(menu.menus[2].disabled);
        assert_eq!(menu.view.open_item, Some(1));
        assert_eq!(menu.view.focused_item, Some(1));
        assert!(menu.view.submenu_path.is_empty());
    }

    #[test]
    fn a_new_app_menu_starts_closed() {
        let ms = Ms::new(sample_menus());
        assert!(!ms.menu.is_open());
        assert_eq!(ms.menu.view.open_item, None);
        assert_eq!(ms.menu.view.focused_item, None);
    }

    #[test]
    fn set_menus_replaces_definitions() {
        let mut ms = Ms::new(sample_menus());
        assert_eq!(ms.menu.menus.len(), 3);
        ms.menu.set_menus(vec![MenuDef {
            id: WidgetId::new("only"),
            label: "Only".into(),
            disabled: false,
            items: vec![action("a", "A")],
        }]);
        assert_eq!(ms.menu.menus.len(), 1);
    }

    /// A shorter `set_menus` call while a top-level dropdown is open must
    /// close it rather than leave `open_item` pointing past the end of the
    /// new list — `dropdown_layout`/`build_dropdown` index `self.menus`
    /// directly, so a stale out-of-range `open_item` would panic.
    #[test]
    fn set_menus_closes_dropdown_left_pointing_past_the_new_list() {
        let mut ms = Ms::new(sample_menus());
        let backend = MockBackend::new();
        ms.menu.view.open_item = Some(2); // "Help" — the last menu, dropped below
        ms.menu.view.focused_item = Some(2);

        ms.menu.set_menus(vec![MenuDef {
            id: WidgetId::new("file"),
            label: "&File".into(),
            disabled: false,
            items: vec![action("new", "New File")],
        }]);

        assert_eq!(ms.menu.view.open_item, None);
        assert_eq!(ms.menu.view.focused_item, None);
        assert!(ms.dropdown_layout(&backend, bar_rect()).is_none());
    }

    /// Shrinking the open menu's own item list so a deeper submenu's
    /// `submenu_path` entry no longer resolves must drop that entry (and
    /// anything nested under it) rather than leave it dangling —
    /// `items_at_depth`/`dropdown_stack` walk `submenu_path` against the
    /// new items and would otherwise be handed a stale index.
    #[test]
    fn set_menus_truncates_submenu_path_left_pointing_past_new_items() {
        let mut ms = Ms::new(vec![MenuDef {
            id: WidgetId::new("file"),
            label: "&File".into(),
            disabled: false,
            items: vec![{
                let mut item = action("export", "Export");
                item.submenu = Some(vec![action("png", "PNG"), action("svg", "SVG")]);
                item
            }],
        }]);
        let backend = MockBackend::new();
        ms.menu.view.open_item = Some(0);
        ms.menu.view.submenu_path = vec![0]; // "Export"'s submenu is open

        // New "File" menu's one item has no submenu at all, so depth-0 of
        // the old `submenu_path` no longer resolves.
        ms.menu.set_menus(vec![MenuDef {
            id: WidgetId::new("file"),
            label: "&File".into(),
            disabled: false,
            items: vec![action("new", "New File")],
        }]);

        assert_eq!(ms.menu.view.open_item, Some(0));
        assert!(ms.menu.view.submenu_path.is_empty());
        assert!(ms.dropdown_layout(&backend, bar_rect()).is_some());
        assert!(ms.menu.dropdown_stack(&backend, bar_rect()).len() <= 1);
    }

    #[test]
    fn next_enabled_menu_skips_disabled() {
        let ms = Ms::new(sample_menus());
        // help (idx 2) is disabled, so from edit (1) forward wraps to file (0)
        assert_eq!(ms.menu.next_enabled_menu(1, 1), 0);
        // from file (0) backward wraps past help to edit (1)
        assert_eq!(ms.menu.next_enabled_menu(0, -1), 1);
    }

    #[test]
    fn next_enabled_menu_steps_on_from_a_disabled_menu() {
        let menu = |id: &str, disabled: bool| MenuDef {
            id: WidgetId::new(id),
            label: id.into(),
            disabled,
            items: vec![action("x", "X")],
        };
        // The middle menu is disabled and is where the move starts from
        // (Alt-activating a disabled menu label, say): the move steps to
        // its neighbour in bar order, not to an end of the bar.
        let ms = Ms::new(vec![menu("a", false), menu("b", true), menu("c", false)]);
        assert_eq!(ms.menu.next_enabled_menu(1, 1), 2);
        assert_eq!(ms.menu.next_enabled_menu(1, -1), 0);
    }

    #[test]
    fn move_selection_off_a_separator_steps_to_the_neighbouring_item() {
        let mut ms = Ms::new(sample_menus());
        // The file dropdown is `new, save, separator, quit`, and index 2 —
        // the separator — is the current selection.
        ms.menu.view.open_item = Some(0);
        ms.menu.view.dropdown_selected = 2;
        ms.menu.move_selection(1);
        assert_eq!(ms.menu.view.dropdown_selected, 3, "forward lands on quit");

        ms.menu.view.dropdown_selected = 2;
        ms.menu.move_selection(-1);
        assert_eq!(ms.menu.view.dropdown_selected, 1, "backward lands on save");
    }

    #[test]
    fn build_menu_bar_reflects_state() {
        let mut ms = Ms::new(sample_menus());
        ms.menu.view.open_item = Some(1);
        ms.menu.view.focused_item = Some(1);
        let bar = ms.menu.build_menu_bar();
        assert_eq!(bar.items.len(), 3);
        assert_eq!(bar.open_item, Some(1));
        assert_eq!(bar.focused_item, Some(1));
        assert!(bar.items[2].disabled);
    }

    /// `build_menu_bar`'s `MenuBarItem::submenu` must carry this
    /// `MenuDef`'s real dropdown items, not the stale `None` a native
    /// backend's `Backend::install_menu_bar` would silently bake into an
    /// empty `NSMenu`/`HMENU` submenu — `AppMenu::render` installs
    /// this `MenuBar` under the native style, so this is the value a
    /// native install actually ends up showing.
    #[test]
    fn build_menu_bar_carries_each_menus_own_dropdown_items_as_submenu() {
        let ms = Ms::new(sample_menus());
        let bar = ms.menu.build_menu_bar();
        assert_eq!(
            bar.items[0].submenu.as_deref(),
            Some(sample_menus()[0].items.as_slice()),
        );
        assert_eq!(
            bar.items[1].submenu.as_deref(),
            Some(sample_menus()[1].items.as_slice()),
        );
        // "help" has no dropdown items, but the field is still `Some`
        // (an empty submenu), never `None` — `None` would mean "no
        // dropdown at all" to `A11yPrimitive for MenuBar` and to a
        // native backend's install, which is wrong for a disabled menu
        // that still has a (empty) `MenuDef::items` list.
        assert_eq!(bar.items[2].submenu.as_deref(), Some([].as_slice()));
    }

    #[test]
    fn build_dropdown_uses_selected_idx() {
        let mut ms = Ms::new(sample_menus());
        ms.menu.view.dropdown_selected = 2;
        let dd = ms.menu.build_dropdown(0);
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
        let mut ms = Ms::new(menus_with_submenu());
        ms.menu.view.open_item = Some(0);
        let items = ms.menu.items_at_depth(0).unwrap();
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].id, Some(WidgetId::new("sidebar")));
    }

    #[test]
    fn items_at_depth_one_returns_submenu_items() {
        let mut ms = Ms::new(menus_with_submenu());
        ms.menu.view.open_item = Some(0);
        ms.menu.view.submenu_path = vec![1]; // item 1 ("Export") has a submenu
        let items = ms.menu.items_at_depth(1).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].id, Some(WidgetId::new("export-png")));
    }

    #[test]
    fn open_submenu_pushes_path_and_first_selectable() {
        let mut ms = Ms::new(menus_with_submenu());
        ms.menu.view.open_item = Some(0);
        ms.menu.view.dropdown_selected = 1; // "Export" is selected
        ms.menu.open_submenu(1);
        assert_eq!(ms.menu.view.submenu_path, vec![1]);
        assert_eq!(ms.menu.view.submenu_selected, vec![0]); // first selectable in PNG/SVG
    }

    #[test]
    fn close_deepest_submenu_pops_one_level() {
        let mut ms = Ms::new(menus_with_submenu());
        ms.menu.view.open_item = Some(0);
        ms.menu.view.submenu_path = vec![1];
        ms.menu.view.submenu_selected = vec![0];
        ms.menu.close_deepest_submenu();
        assert!(ms.menu.view.submenu_path.is_empty());
        assert!(ms.menu.view.submenu_selected.is_empty());
    }

    #[test]
    fn current_selected_returns_root_when_no_submenus() {
        let mut ms = Ms::new(menus_with_submenu());
        ms.menu.view.open_item = Some(0);
        ms.menu.view.dropdown_selected = 2;
        assert_eq!(ms.menu.current_selected(), 2);
    }

    #[test]
    fn current_selected_returns_deepest_submenu_selection() {
        let mut ms = Ms::new(menus_with_submenu());
        ms.menu.view.open_item = Some(0);
        ms.menu.view.dropdown_selected = 1;
        ms.menu.view.submenu_path = vec![1];
        ms.menu.view.submenu_selected = vec![1];
        assert_eq!(ms.menu.current_selected(), 1);
    }

    // ── Keyboard-nav interaction tests (flow through handle()) ────────────────

    // Fixture: menus_with_submenu(), but the root dropdown is "open" so that
    // keyboard events are processed.  We set open_item directly (bypassing the
    // modal-stack push that open_menu does) because these tests focus on
    // navigation state, not on rendering.
    fn open_ms() -> Ms {
        let mut ms = Ms::new(menus_with_submenu());
        ms.menu.view.open_item = Some(0);
        ms.menu.view.dropdown_selected = 1; // pre-select "Export" (item 1, has submenu)
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
            Outcome::StateChanged,
            "Right must return StateChanged"
        );
        assert_eq!(
            ms.menu.view.submenu_path,
            vec![1],
            "submenu_path must grow to [1]"
        );
        assert!(
            ms.menu.view.open_item.is_some(),
            "menu must still be open after Right"
        );
    }

    /// `Left` with a submenu open must pop one level and return `StateChanged`.
    #[test]
    fn handle_left_with_submenu_open_closes_deepest() {
        let mut ms = open_ms();
        ms.menu.view.submenu_path = vec![1];
        ms.menu.view.submenu_selected = vec![0];
        let mut backend = MockBackend::new();
        let ev = key_ev(Key::Named(NamedKey::Left));

        let result = ms.handle(&ev, &mut backend, bar_rect());

        assert_eq!(
            result,
            Outcome::StateChanged,
            "Left must return StateChanged"
        );
        assert!(
            ms.menu.view.submenu_path.is_empty(),
            "submenu_path must shrink back to []"
        );
        assert!(
            ms.menu.view.open_item.is_some(),
            "top-level menu must remain open"
        );
    }

    /// `Esc` with a submenu open must close only the deepest level — NOT the
    /// whole menu.
    #[test]
    fn handle_esc_with_submenu_closes_deepest_not_whole_menu() {
        let mut ms = open_ms();
        ms.menu.view.submenu_path = vec![1];
        ms.menu.view.submenu_selected = vec![0];
        let mut backend = MockBackend::new();
        let ev = key_ev(Key::Named(NamedKey::Escape));

        let result = ms.handle(&ev, &mut backend, bar_rect());

        assert_eq!(result, Outcome::StateChanged);
        assert!(
            ms.menu.view.submenu_path.is_empty(),
            "deepest submenu must be closed"
        );
        assert!(
            ms.menu.view.open_item.is_some(),
            "top-level menu must still be open"
        );
    }

    /// `Esc` with no submenus open must close the whole menu.
    #[test]
    fn handle_esc_with_no_submenu_closes_whole_menu() {
        let mut ms = open_ms();
        // No submenus open — submenu_path is empty.
        let mut backend = MockBackend::new();
        let ev = key_ev(Key::Named(NamedKey::Escape));

        let result = ms.handle(&ev, &mut backend, bar_rect());

        assert_eq!(result, Outcome::StateChanged);
        assert!(
            ms.menu.view.open_item.is_none(),
            "whole menu must be closed"
        );
        assert!(ms.menu.view.submenu_path.is_empty());
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
            Outcome::StateChanged,
            "Enter on parent must return StateChanged"
        );
        assert_eq!(
            ms.menu.view.submenu_path,
            vec![1],
            "submenu must open for item 1"
        );
        assert!(ms.menu.view.open_item.is_some(), "menu must remain open");
    }

    /// `Enter` on a leaf item inside an open submenu must activate it and close
    /// the whole menu.
    #[test]
    fn handle_enter_on_leaf_in_submenu_activates_and_closes() {
        let mut ms = open_ms();
        // Open the submenu for "Export" (item 1) and select "PNG" (item 0 inside).
        ms.menu.view.submenu_path = vec![1];
        ms.menu.view.submenu_selected = vec![0]; // "PNG" (id = "export-png")
        let mut backend = MockBackend::new();
        let ev = key_ev(Key::Named(NamedKey::Enter));

        let result = ms.handle(&ev, &mut backend, bar_rect());

        assert_eq!(
            result,
            Outcome::Activated(WidgetId::new("export-png")),
            "Enter on leaf must return Activated with the item id"
        );
        assert!(
            ms.menu.view.open_item.is_none(),
            "menu must close after activation"
        );
        assert!(
            ms.menu.view.submenu_path.is_empty(),
            "submenu_path must be cleared on close"
        );
    }

    // ── MouseUp outside-click dismiss ──────────────────────────────────────

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
        let mut ms = Ms::new(sample_menus());
        let mut backend = MockBackend::new();
        ms.open_menu(0, &mut backend, bar_rect());
        assert!(ms.menu.view.open_item.is_some());

        // Far outside the bar (height 1) and outside the viewport-clamped
        // dropdown.
        let ev = mouse_up_ev(crate::event::Point::new(999.0, 999.0));
        let result = ms.handle(&ev, &mut backend, bar_rect());

        assert_eq!(result, Outcome::StateChanged);
        assert!(
            ms.menu.view.open_item.is_none(),
            "menu must close on outside MouseUp"
        );
    }

    /// `MouseUp(Left)` with no menu open must be a no-op `Ignored` — nothing
    /// to close, and no panic touching layout state.
    #[test]
    fn handle_mouse_up_with_no_menu_open_is_ignored() {
        let mut ms = Ms::new(sample_menus());
        let mut backend = MockBackend::new();
        let ev = mouse_up_ev(crate::event::Point::new(999.0, 999.0));

        let result = ms.handle(&ev, &mut backend, bar_rect());

        assert_eq!(result, Outcome::Ignored);
        assert!(ms.menu.view.open_item.is_none());
    }

    /// `MouseUp(Left)` on the menu-bar item that `MouseDown` just opened
    /// must NOT immediately re-close the menu — that zone belongs to
    /// `MouseDown`, and closing here would defeat every normal
    /// (Down-then-Up) click-to-open.
    #[test]
    fn handle_mouse_up_on_bar_item_does_not_close() {
        let mut ms = Ms::new(sample_menus());
        let mut backend = MockBackend::new();
        ms.open_menu(0, &mut backend, bar_rect());

        // Item 0 ("File") occupies x in [0, 10), y in [0, 1) per the mock
        // backend's 10.0-wide, single-row layout.
        let ev = mouse_up_ev(crate::event::Point::new(5.0, 0.0));
        let result = ms.handle(&ev, &mut backend, bar_rect());

        assert_eq!(result, Outcome::Ignored);
        assert!(
            ms.menu.view.open_item.is_some(),
            "menu must remain open after Up on the bar item that opened it"
        );
    }

    /// `MouseUp(Left)` landing on an actual open dropdown item must be
    /// ignored, not re-activated — `MouseDown` already owns item
    /// activation, so this arm must not double-fire `Activated`.
    #[test]
    fn handle_mouse_up_on_open_dropdown_item_is_ignored() {
        let mut ms = Ms::new(sample_menus());
        let mut backend = MockBackend::new();
        ms.open_menu(0, &mut backend, bar_rect());

        let stack = ms.menu.dropdown_stack(&backend, bar_rect());
        let item_bounds = stack[0].1.visible_items[0].bounds;
        let pos = crate::event::Point::new(
            item_bounds.x + item_bounds.width / 2.0,
            item_bounds.y + item_bounds.height / 2.0,
        );

        let ev = mouse_up_ev(pos);
        let result = ms.handle(&ev, &mut backend, bar_rect());

        assert_eq!(result, Outcome::Ignored);
        assert!(
            ms.menu.view.open_item.is_some(),
            "menu must remain open — MouseUp must not activate or close"
        );
    }

    /// The x a quarter-unit past the right edge of the bar's last label.
    fn past_last_label(ms: &Ms, backend: &MockBackend) -> f32 {
        let layout = backend.layout_primitive(bar_rect(), &ms.menu);
        let last = layout
            .bar
            .layout
            .visible_items
            .last()
            .expect("the bar lays out its labels")
            .bounds;
        let x = last.x + last.width + 0.25;
        assert!(x < bar_rect().width, "the bar must extend past its labels");
        x
    }

    fn mouse_moved_ev(x: f32, y: f32) -> UiEvent {
        crate::event::mouse_moved(x, y, crate::event::ButtonMask::default())
    }

    #[test]
    fn closed_click_past_the_last_label_is_ignored() {
        let mut ms = Ms::new(sample_menus());
        let mut backend = MockBackend::new();
        let x = past_last_label(&ms, &backend);
        let down = crate::event::mouse_down(MouseButton::Left, x, 0.0, crate::Modifiers::default());

        assert_eq!(ms.handle(&down, &mut backend, bar_rect()), Outcome::Ignored);
        assert_eq!(
            ms.handle(&mouse_up_ev(Point::new(x, 0.0)), &mut backend, bar_rect()),
            Outcome::Ignored
        );
        assert!(!ms.menu.is_open());
    }

    #[test]
    fn closed_hover_leaving_the_labels_clears_focus_and_is_ignored() {
        let mut ms = Ms::new(sample_menus());
        let mut backend = MockBackend::new();
        let x = past_last_label(&ms, &backend);

        let on_file = ms.handle(&mouse_moved_ev(1.0, 0.0), &mut backend, bar_rect());
        assert_eq!(on_file, Outcome::Consumed);
        assert_eq!(ms.menu.build_menu_bar().focused_item, Some(0));

        let off = ms.handle(&mouse_moved_ev(x, 0.0), &mut backend, bar_rect());
        assert_eq!(off, Outcome::Ignored);
        assert_eq!(ms.menu.build_menu_bar().focused_item, None);
        assert!(ms.view_changed, "clearing the hover asks for a repaint");

        let again = ms.handle(&mouse_moved_ev(x, 0.0), &mut backend, bar_rect());
        assert_eq!(again, Outcome::Ignored);
        assert!(!ms.view_changed, "a no-op move asks for nothing");
    }

    #[test]
    fn open_click_past_the_last_label_still_closes_the_menu() {
        let mut ms = Ms::new(sample_menus());
        let mut backend = MockBackend::new();
        let x = past_last_label(&ms, &backend);
        ms.open_menu(0, &mut backend, bar_rect());
        let down = crate::event::mouse_down(MouseButton::Left, x, 0.0, crate::Modifiers::default());

        assert_eq!(
            ms.handle(&down, &mut backend, bar_rect()),
            Outcome::StateChanged
        );
        assert!(!ms.menu.is_open());
    }

    #[test]
    fn an_open_dropdown_takes_a_plain_char_as_typeahead_by_default() {
        let mut ms = Ms::new(sample_menus());
        let mut backend = MockBackend::new();
        ms.open_menu(0, &mut backend, bar_rect());

        let result = ms.handle(&key_ev(Key::Char('s')), &mut backend, bar_rect());

        assert_ne!(result, Outcome::Ignored);
        assert_eq!(ms.menu.view.dropdown_selected, 1, "typeahead moved to Save");
    }

    #[test]
    fn typeahead_off_lets_a_plain_char_reach_the_host() {
        let mut ms = Ms::new(sample_menus());
        ms.menu.typeahead = false;
        let mut backend = MockBackend::new();
        ms.open_menu(0, &mut backend, bar_rect());

        let result = ms.handle(&key_ev(Key::Char('s')), &mut backend, bar_rect());

        assert_eq!(result, Outcome::Ignored);
        assert_eq!(ms.menu.view.dropdown_selected, 0, "the highlight stays put");
        let down = ms.handle(
            &key_ev(Key::Named(NamedKey::Down)),
            &mut backend,
            bar_rect(),
        );
        assert_ne!(
            down,
            Outcome::Ignored,
            "navigation keys still belong to the menu"
        );
    }
}
