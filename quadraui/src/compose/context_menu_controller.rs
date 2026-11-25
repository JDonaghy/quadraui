//! `ContextMenuController` — the "one call" context-menu entry point,
//! composed on top of the existing [`ContextMenu`] widget,
//! [`Backend::show_context_menu`] native path, and [`ModalStack`].
//!
//! An app that wants a right-click menu writes exactly one code path,
//! regardless of platform or [`MenuStyle`]:
//! ```ignore
//! // Setup: one field on the app.
//! struct MyApp {
//!     ctx_menu: ContextMenuController,
//!     // ...
//! }
//!
//! // In handle(), before other routing — mirrors `MenuSystem::handle`'s
//! // contract:
//! fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
//!     if let ContextMenuOutcome::Event(ev) = self.ctx_menu.handle(&event, backend) {
//!         return self.handle_menu_event(ev); // same UiEvent::ContextMenuItemActivated
//!                                             // / ContextMenuDismissed either path took
//!     }
//!     match event {
//!         UiEvent::MouseDown { button: MouseButton::Right, position, .. } => {
//!             let menu = self.build_context_menu();
//!             // The one call: native on macOS under `Auto`, painted
//!             // everywhere else — same call either way.
//!             self.ctx_menu.open(menu, position, backend);
//!             Reaction::Redraw
//!         }
//!         // A native-path activation/dismissal also arrives here as a
//!         // top-level event (queued by `Backend::show_context_menu`) —
//!         // route it through the same handler for one code path.
//!         UiEvent::ContextMenuItemActivated(id) => {
//!             self.handle_menu_event(UiEvent::ContextMenuItemActivated(id))
//!         }
//!         UiEvent::ContextMenuDismissed => {
//!             self.handle_menu_event(UiEvent::ContextMenuDismissed)
//!         }
//!         _ => Reaction::Continue,
//!     }
//! }
//!
//! // In render(), after base content:
//! fn render(&self, backend: &mut dyn Backend, area: ()) {
//!     // ... paint the app's own content first ...
//!     self.ctx_menu.render(backend);
//! }
//! ```
//!
//! # Why this exists instead of a `Backend` default method
//!
//! [`Backend::effective_menu_style`] resolves [`MenuStyle`] purely (no
//! state needed), but *acting* on a `Custom` resolution needs somewhere
//! to keep "a menu is open, at this layout, with this selection" between
//! the `MouseDown` that opened it and the click/keypress that closes it.
//! A bare `&mut dyn Backend` default method has no field to put that in
//! without every backend struct growing one. This controller is that
//! state holder — the same shape [`crate::compose::MenuSystem`] already
//! uses for menu-bar dropdowns, applied to a single anchored popup with
//! no bar.
//!
//! # Nested submenus
//!
//! An item's [`crate::ContextMenuItem::submenu`] opens a pull-right
//! child popup beside its parent row, positioned through the same
//! [`crate::primitives::context_menu::ContextMenuLayout::submenu_anchor`]
//! shared layout rule [`crate::compose::MenuSystem`]'s menu-bar submenus
//! use — flips to the parent's left edge near the viewport's right
//! edge. Right/Enter on a selected submenu-parent opens its child;
//! Left/Escape closes the deepest open level (Escape with nothing
//! nested closes the whole menu, same as before); hovering a
//! submenu-parent row opens its child immediately (no timer — same
//! "open on hover" shape [`crate::compose::MenuSystem`] already uses for
//! menu-bar submenus), hovering a leaf closes any deeper level still
//! open. Activation always reports the activated leaf's own
//! [`crate::WidgetId`] via [`ContextMenuOutcome::Event`] regardless of
//! nesting depth — the id uniquely identifies its place in the tree, so
//! submenu depth is transparent to the caller, the same contract
//! [`crate::UiEvent::MenuActivated`]'s own doc states for native menu-bar
//! installers.
//!
//! # Call this from event-handling code only — never from `render`
//!
//! Same contract as [`Backend::show_context_menu`] (see its doc): call
//! [`ContextMenuController::open`] from `AppLogic::handle`, typically on
//! `MouseDown { button: Right, .. }`. Never from `render` — the `Native`
//! path blocks on AppKit's modal pop-up loop, and running that from
//! inside a paint closure re-enters painting while the paint closure
//! still holds the borrows it needs to finish its own frame
//! (`JDonaghy/vimcode#1580`).

use crate::backend::{Backend, DrawsPrimitives, ResolvedMenuStyle};
use crate::compose::Handled;
use crate::event::{Point, Rect, UiEvent};
use crate::interaction::InteractionState;
use crate::interactive::KeyEvent;
use crate::primitives::context_menu::{
    ContextMenu, ContextMenuClick, ContextMenuEvent, ContextMenuHit, ContextMenuItem,
    ContextMenuItemMeasure, ContextMenuLayout, ContextMenuPlacement, ContextMenuState,
};
use crate::types::WidgetId;

/// What [`ContextMenuController::handle`] did with an event.
#[derive(Debug, Clone, PartialEq)]
pub enum ContextMenuOutcome {
    /// Terminal: an item was activated, or the menu was dismissed.
    /// Always [`UiEvent::ContextMenuItemActivated`] or
    /// [`UiEvent::ContextMenuDismissed`] — literally the same two events
    /// the `Native` path delivers via the normal event queue, so a
    /// caller can route both through one match arm.
    Event(UiEvent),
    /// The event was consumed by menu navigation/hit-testing (selection
    /// moved, a click landed on an inert region). The caller should
    /// redraw; there's nothing further to route.
    Consumed,
    /// No menu is open, or the event wasn't relevant to the open one.
    Ignored,
}

/// Owns zero-or-one open [`ContextMenu`] and routes it through either
/// the backend's native pop-up or an in-window painted popup, per
/// [`Backend::effective_menu_style`]. See the module doc for the full
/// recipe.
#[derive(Debug, Default)]
pub struct ContextMenuController {
    open: Option<OpenMenu>,
    /// Hover/pressed state for the `Custom`-painted popup's own items,
    /// fed from mouse events in [`Self::handle`], so no caller writes any
    /// hover-tracking code of its own.
    interaction: InteractionState,
    /// The open popup's own widget state — the debounced typeahead buffer
    /// [`ContextMenu::handle_key`] extends, which is the only thing a
    /// keystroke needs to remember between events.
    typeahead: ContextMenuState,
}

#[derive(Debug, Clone)]
struct OpenMenu {
    menu: ContextMenu,
    anchor: Point,
    /// Depth-first path of open submenus: `submenu_path[d]` = the
    /// `item_idx` in the menu at depth `d` whose child submenu is open.
    /// Empty ⟹ no submenus open. Mirrors [`crate::compose::MenuSystem`]'s
    /// own field of the same name.
    submenu_path: Vec<usize>,
    /// Selected item index at each submenu depth; `submenu_selected[d]`
    /// corresponds to the submenu opened by `submenu_path[d]`.
    submenu_selected: Vec<usize>,
}

/// Item height / separator height in line-height units — the widget's own
/// [`crate::primitives::context_menu::item_heights`], which
/// [`crate::primitive::Primitive::layout`] resolves its rows at too.
///
/// [`crate::compose::MenuSystem`]'s own dropdown layout
/// (`dropdown_layout`/`dropdown_stack`) reaches it through this name, so
/// the two menu styles — free-floating context menu here, bar-anchored
/// dropdown there — stay pixel-identical without either controller
/// depending on the other's state. Menu *width* isn't included: this
/// controller uses a fixed `20.0 * lh`, while `MenuSystem` measures its
/// dropdown's actual item text via `dropdown_width`, so the two don't
/// share that part.
pub(crate) fn menu_item_heights(lh: f32) -> (f32, f32) {
    crate::primitives::context_menu::item_heights(lh)
}

/// The line height a painted popup's rows derive from: the same row
/// height [`DrawsPrimitives::draw_dyn`] resolves for the backend, so a
/// layout placed here and the widget contract's own re-layout at paint
/// time agree on every row's position.
pub(crate) fn menu_line_height(backend: &dyn Backend) -> f32 {
    crate::backend::primitive_layout_ctx(backend).line_height
}

/// The rect a host hands [`ContextMenu::layout`]/[`ContextMenu::layout_at`]
/// as its viewport: `backend`'s whole surface, inset on every side by the
/// border ring [`ContextMenuPrimitiveLayout::border_inset`] reserves, so a
/// popup clamped flush against an edge still has room for the chrome a
/// cell grid paints outside its bounds.
///
/// [`ContextMenuPrimitiveLayout::border_inset`]: crate::primitives::context_menu::ContextMenuPrimitiveLayout::border_inset
pub(crate) fn menu_viewport(backend: &dyn Backend, menu: &ContextMenu) -> Rect {
    let vp = backend.viewport();
    backend
        .layout_primitive(Rect::new(0.0, 0.0, vp.width, vp.height), menu)
        .viewport
}

/// Paints `menu` at its already-placed `layout` through the widget
/// contract. The box handed to [`DrawsPrimitives::draw_dyn`] is
/// `layout.bounds` grown by the border ring, so the popup's own re-layout
/// inside that box lands every row exactly where `layout` placed it.
pub(crate) fn paint_placed_menu(
    backend: &mut dyn Backend,
    menu: &ContextMenu,
    layout: &ContextMenuLayout,
) {
    let inset = backend.layout_primitive(layout.bounds, menu).border_inset;
    let b = layout.bounds;
    let rect = Rect::new(
        b.x - inset,
        b.y - inset,
        b.width + inset * 2.0,
        b.height + inset * 2.0,
    );
    let _ = backend.draw_dyn(rect, menu, &());
}

impl ContextMenuController {
    pub fn new() -> Self {
        Self {
            open: None,
            interaction: InteractionState::new(),
            typeahead: ContextMenuState::default(),
        }
    }

    /// Is a `Custom`-style menu currently open and painted? `false` for
    /// the `Native` path (that popup is AppKit's own modal state, not
    /// this controller's).
    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }

    /// **The one call.** Open `menu` at `anchor` (view-local
    /// coordinates). Resolves [`Backend::effective_menu_style`] and
    /// routes to [`Backend::show_context_menu`] (`Native`) or in-window
    /// painted state (`Custom`) — the caller doesn't branch on either.
    ///
    /// See the module doc's "Call this from event-handling code only"
    /// section — this must be called from `AppLogic::handle`, never
    /// `render`.
    pub fn open(&mut self, menu: ContextMenu, anchor: Point, backend: &mut dyn Backend) {
        match backend.effective_menu_style() {
            ResolvedMenuStyle::Native => {
                self.close(backend);
                backend.show_context_menu(&menu, anchor);
            }
            ResolvedMenuStyle::Custom => {
                let id = menu.id;
                let layout = self.layout_for(&menu, anchor, backend);
                backend
                    .modal_stack_handle()
                    .borrow_mut()
                    .push(id, layout.bounds);
                self.open = Some(OpenMenu {
                    menu,
                    anchor,
                    submenu_path: Vec::new(),
                    submenu_selected: Vec::new(),
                });
            }
        }
    }

    /// Close the painted menu, if one is open. A no-op on the `Native`
    /// path (AppKit owns dismissal there) and if nothing is open.
    pub fn close(&mut self, backend: &mut dyn Backend) {
        if let Some(open) = self.open.take() {
            backend.modal_stack_handle().borrow_mut().pop(&open.menu.id);
        }
        self.interaction.clear();
    }

    /// Process one event against the currently open painted menu.
    /// Returns [`ContextMenuOutcome::Ignored`] immediately if nothing is
    /// open — safe to call unconditionally before an app's own routing,
    /// same contract as [`crate::compose::MenuSystem::handle`].
    pub fn handle(&mut self, event: &UiEvent, backend: &mut dyn Backend) -> ContextMenuOutcome {
        let Some(open) = self.open.clone() else {
            return ContextMenuOutcome::Ignored;
        };

        // Feed hover/pressed before the event-shape match below: a
        // `MouseMoved`/`MouseDown`/`MouseUp` that also changes
        // navigation/selection state still needs its hover/pressed
        // side effect recorded. Gated on the same three event shapes
        // `handle_mouse` itself matches (see `interaction.rs`'s doc) so
        // every other shape (`KeyPressed`, etc) skips the stack-layout
        // work below rather than paying for it and then hitting
        // `handle_mouse`'s own no-op match arm.
        if matches!(
            event,
            UiEvent::MouseMoved { .. } | UiEvent::MouseDown { .. } | UiEvent::MouseUp { .. }
        ) {
            let stack = self.menu_stack(&open, backend);
            self.interaction.handle_mouse(event, |x, y| {
                // Deepest level first — a submenu painted on top of its
                // parent should win a hover/press hit over whatever the
                // parent's own (now-covered) row underneath it reports.
                stack
                    .iter()
                    .rev()
                    .find_map(|(_, layout)| match layout.hit_test(x, y) {
                        ContextMenuHit::Item(id) => Some(id),
                        ContextMenuHit::Inert | ContextMenuHit::Empty => None,
                    })
            });
        }

        match event {
            // Every keystroke's meaning comes from the widget's own
            // `ContextMenu::handle_key` — one navigation/typeahead/
            // activation rule shared with `Interactive::handle`, resolved
            // against the deepest open level. This controller only maps
            // the resulting `ContextMenuEvent` onto its submenu stack,
            // which is the one thing a single widget cannot know about.
            UiEvent::KeyPressed {
                key,
                modifiers,
                repeat,
            } => {
                let stack = self.menu_stack(&open, backend);
                let Some((menu, layout)) = stack.last() else {
                    return ContextMenuOutcome::Ignored;
                };
                // `menu_stack` stops at the first path entry that does
                // not name a submenu parent, so the depth of the level
                // it actually built is what a selection move applies
                // to — reading `submenu_path.len()` instead would
                // address a level that is not there and drop the move.
                let depth = stack.len() - 1;
                let page = layout.visible_items.len().max(1);
                let event = KeyEvent {
                    key: key.clone(),
                    modifiers: *modifiers,
                    repeat: *repeat,
                };
                match menu.handle_key(&event, page, &mut self.typeahead) {
                    Handled::Event(ContextMenuEvent::SelectionChanged { idx }) => {
                        self.set_selected_at_depth(depth, idx);
                        ContextMenuOutcome::Consumed
                    }
                    Handled::Event(ContextMenuEvent::SubmenuOpened { idx }) => {
                        self.open_submenu(idx);
                        ContextMenuOutcome::Consumed
                    }
                    // Left closes the deepest open level; with nothing
                    // nested it is the host's key, not the menu's.
                    Handled::Event(ContextMenuEvent::SubmenuClosed) => {
                        if depth == 0 {
                            ContextMenuOutcome::Ignored
                        } else {
                            self.close_deepest_submenu();
                            ContextMenuOutcome::Consumed
                        }
                    }
                    Handled::Event(ContextMenuEvent::Activated { item }) => {
                        self.close(backend);
                        ContextMenuOutcome::Event(UiEvent::ContextMenuItemActivated(item))
                    }
                    // Escape: closes the deepest nested level, or the
                    // whole menu when nothing is nested.
                    Handled::Event(ContextMenuEvent::Dismissed) => {
                        if depth == 0 {
                            self.close(backend);
                            ContextMenuOutcome::Event(UiEvent::ContextMenuDismissed)
                        } else {
                            self.close_deepest_submenu();
                            ContextMenuOutcome::Consumed
                        }
                    }
                    Handled::Consumed => ContextMenuOutcome::Consumed,
                    Handled::Ignored => ContextMenuOutcome::Ignored,
                }
            }
            UiEvent::MouseDown { position, .. } => {
                let stack = self.menu_stack(&open, backend);
                // Deepest level first — a click inside a submenu must not
                // fall through and hit whatever the parent's covered row
                // underneath reports. Which row a point belongs to, and
                // whether activating it fires an id or opens a child, is
                // the widget's own `ContextMenu::resolve_click`.
                for (depth_idx, (menu, layout)) in stack.iter().enumerate().rev() {
                    match menu.resolve_click(layout, position.x, position.y) {
                        ContextMenuClick::Submenu { idx } => {
                            self.truncate_submenus(depth_idx);
                            self.open_submenu(idx);
                            return ContextMenuOutcome::Consumed;
                        }
                        ContextMenuClick::Item { id, .. } => {
                            self.close(backend);
                            return ContextMenuOutcome::Event(UiEvent::ContextMenuItemActivated(
                                id,
                            ));
                        }
                        ContextMenuClick::Inert => return ContextMenuOutcome::Consumed,
                        ContextMenuClick::Outside => continue,
                    }
                }
                // Outside every open level → dismiss.
                self.close(backend);
                ContextMenuOutcome::Event(UiEvent::ContextMenuDismissed)
            }
            UiEvent::MouseMoved { position, .. } => {
                let stack = self.menu_stack(&open, backend);
                // Deepest level first — same rule as `MouseDown` above: a
                // narrow viewport can clamp a child popup so it overlaps
                // its parent, and the pointer must hit the (visually
                // topmost) child row before it falls through to whatever
                // parent row is underneath.
                for (depth_idx, (menu, layout)) in stack.iter().enumerate().rev() {
                    let (item_idx, is_parent) =
                        match menu.resolve_click(layout, position.x, position.y) {
                            ContextMenuClick::Submenu { idx } => (idx, true),
                            ContextMenuClick::Item { idx, .. } => (idx, false),
                            ContextMenuClick::Inert | ContextMenuClick::Outside => continue,
                        };
                    let already_open_here = open.submenu_path.get(depth_idx) == Some(&item_idx);
                    self.set_selected_at_depth(depth_idx, item_idx);
                    if is_parent {
                        if !already_open_here {
                            // Re-seeding the child's selection on every
                            // jitter over an already-open parent row
                            // would clobber a keyboard selection made
                            // inside it since the last hover.
                            self.truncate_submenus(depth_idx);
                            self.open_submenu(item_idx);
                        }
                    } else if open.submenu_path.len() > depth_idx {
                        self.truncate_submenus(depth_idx);
                    }
                    return ContextMenuOutcome::Consumed;
                }
                ContextMenuOutcome::Ignored
            }
            _ => ContextMenuOutcome::Ignored,
        }
    }

    /// Paint the currently open painted menu and any open submenu
    /// levels, if any. A no-op on the `Native` path (nothing to paint —
    /// AppKit owns that surface) and if nothing is open. Call from
    /// `render`, after the app's own content so the menu layers on top.
    pub fn render(&self, backend: &mut dyn Backend) {
        let Some(open) = &self.open else {
            return;
        };
        for (menu, layout) in self.menu_stack(open, backend) {
            paint_placed_menu(backend, &menu, &layout);
        }
    }

    fn layout_for(
        &self,
        menu: &ContextMenu,
        anchor: Point,
        backend: &dyn Backend,
    ) -> ContextMenuLayout {
        let lh = menu_line_height(backend);
        let (item_h, sep_h) = menu_item_heights(lh);
        let menu_width = 20.0 * lh;
        let vp = menu_viewport(backend, menu);
        menu.layout(anchor.x, anchor.y, vp, menu_width, |i| {
            if menu.items[i].is_separator() {
                ContextMenuItemMeasure::new(sep_h)
            } else {
                ContextMenuItemMeasure::new(item_h)
            }
        })
    }

    /// Build the full stack of `(ContextMenu, ContextMenuLayout)` for the
    /// root popup plus every currently open submenu level, deepest last.
    /// Each submenu is positioned beside its parent item via
    /// [`ContextMenuLayout::submenu_anchor`] — the shared layout rule
    /// [`crate::compose::MenuSystem`]'s own `dropdown_stack` uses for
    /// menu-bar submenus.
    fn menu_stack(
        &self,
        open: &OpenMenu,
        backend: &dyn Backend,
    ) -> Vec<(ContextMenu, ContextMenuLayout)> {
        let mut stack = Vec::new();
        let root_layout = self.layout_for(&open.menu, open.anchor, backend);
        stack.push((open.menu.clone(), root_layout));

        let lh = menu_line_height(backend);
        let (item_h, sep_h) = menu_item_heights(lh);
        let menu_width = 20.0 * lh;
        let vp = menu_viewport(backend, &open.menu);

        for (depth, &path_idx) in open.submenu_path.iter().enumerate() {
            let sub_items_opt: Option<Vec<ContextMenuItem>> = stack[depth]
                .0
                .items
                .get(path_idx)
                .and_then(|item| item.submenu.clone());
            let Some(sub_items) = sub_items_opt else {
                break;
            };
            let anchor = stack[depth].1.submenu_anchor(path_idx, menu_width, vp);
            let selected = open.submenu_selected.get(depth).copied().unwrap_or(0);
            let sub_menu = ContextMenu {
                id: WidgetId::new(format!("context-menu-controller-submenu-{depth}")),
                items: sub_items.clone(),
                selected_idx: selected,
                bg: None,
                placement: ContextMenuPlacement::AnchorPoint,
                // A child popup paints the root's own marks, so a
                // glyph override set once on the opened menu applies
                // to every level of the cascade.
                glyphs: open.menu.glyphs.clone(),
            };
            let sub_layout = sub_menu.layout(anchor.x, anchor.y, vp, menu_width, |i| {
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

    /// Items at depth `depth` (0 = root menu items); `depth > 0` descends
    /// `open.submenu_path[0..depth]`. Returns an empty `Vec` if a level
    /// along the path is missing its submenu (stale state).
    fn items_at_depth(open: &OpenMenu, depth: usize) -> Vec<ContextMenuItem> {
        let mut items = open.menu.items.clone();
        for &path_idx in open.submenu_path.iter().take(depth) {
            match items.into_iter().nth(path_idx).and_then(|i| i.submenu) {
                Some(sub) => items = sub,
                None => return Vec::new(),
            }
        }
        items
    }

    /// Set the selected index at depth `depth` (0 = root). Depths beyond
    /// the currently open submenu stack are ignored.
    fn set_selected_at_depth(&mut self, depth: usize, idx: usize) {
        let Some(open) = &mut self.open else {
            return;
        };
        if depth == 0 {
            open.menu.selected_idx = idx;
        } else if let Some(s) = open.submenu_selected.get_mut(depth - 1) {
            *s = idx;
        }
    }

    /// Open the submenu belonging to `item_idx` at the current deepest
    /// level. No-op if that item has no submenu.
    fn open_submenu(&mut self, item_idx: usize) {
        let Some(open) = &self.open else { return };
        let depth = open.submenu_path.len();
        let sub_items = match Self::items_at_depth(open, depth)
            .get(item_idx)
            .and_then(|item| item.submenu.clone())
        {
            Some(items) => items,
            None => return,
        };
        let first_sel = sub_items
            .iter()
            .position(|i| !i.is_separator() && !i.disabled)
            .unwrap_or(0);
        let Some(open) = &mut self.open else { return };
        open.submenu_path.push(item_idx);
        open.submenu_selected.push(first_sel);
    }

    /// Close the deepest open submenu (pop one level). No-op if none open.
    fn close_deepest_submenu(&mut self) {
        if let Some(open) = &mut self.open {
            open.submenu_path.pop();
            open.submenu_selected.pop();
        }
    }

    /// Drop every open submenu level at depth `>= depth`.
    fn truncate_submenus(&mut self, depth: usize) {
        if let Some(open) = &mut self.open {
            open.submenu_path.truncate(depth);
            open.submenu_selected.truncate(depth);
        }
    }
}

// Unit tests below drive a real `TuiBackend` (not a hand-rolled mock —
// `Backend` has dozens of required `draw_*`/`*_layout` methods with no
// default body, so a mock covering only what this controller calls
// doesn't satisfy the trait; see `crate::testing::RecordingBackend` for
// the same trade-off spelled out in its own doc). `TuiBackend::new()`
// is cheap and needs no real terminal for anything this controller
// touches (`modal_stack_handle`, `viewport`, `line_height`,
// `effective_menu_style`, `show_context_menu`) — the one thing it
// *can't* do headless is `draw_context_menu` (needs a live
// `ratatui::Frame` via `enter_frame_scope`), so `render()` itself is
// covered by the `TuiDriver`-based example-driver test instead
// (`tests/tui_example_driver.rs`), which does have one.
//
// `native_menu` is always `false` on `TuiBackend::backend_caps()`, so
// `MenuStyle::Auto`/`Native` both resolve `Custom` here — exactly the
// path these tests exercise. The `Native`-resolves-to-`Native` half of
// the matrix is covered by `MenuStyle::resolve`'s own unit tests in
// `backend.rs` (pure, no backend needed) and by the macOS-caps test in
// `src/macos/backend.rs`.
#[cfg(all(test, feature = "tui"))]
mod tests {
    use super::*;
    use crate::primitives::context_menu::ContextMenuItem;
    use crate::tui::TuiBackend;
    use crate::types::{Modifiers, StyledText, WidgetId};
    use crate::{Key, MouseButton, NamedKey};

    fn item(id: &str, label: &str) -> ContextMenuItem {
        ContextMenuItem {
            id: Some(WidgetId::new(id)),
            label: StyledText::plain(label),
            ..Default::default()
        }
    }

    fn menu() -> ContextMenu {
        ContextMenu {
            id: WidgetId::new("ctx"),
            items: vec![item("copy", "Copy"), item("paste", "Paste")],
            selected_idx: 0,
            bg: None,
            placement: crate::primitives::context_menu::ContextMenuPlacement::AnchorPoint,
            glyphs: Default::default(),
        }
    }

    #[test]
    fn open_paints_when_resolved_custom() {
        let mut backend = TuiBackend::new();
        assert_eq!(backend.effective_menu_style(), ResolvedMenuStyle::Custom);
        let mut ctl = ContextMenuController::new();
        ctl.open(menu(), Point::new(5.0, 5.0), &mut backend);
        assert!(ctl.is_open());
        assert_eq!(
            backend
                .modal_stack_handle()
                .borrow()
                .top()
                .map(|e| e.id.clone()),
            Some(WidgetId::new("ctx"))
        );
    }

    #[test]
    fn click_on_item_activates_and_closes() {
        let mut backend = TuiBackend::new();
        let mut ctl = ContextMenuController::new();
        ctl.open(menu(), Point::new(0.0, 0.0), &mut backend);

        // Anchored flush at the viewport's top-left corner: `TuiBackend`'s
        // `context_menu_viewport` keeps row/column 0 for the box border,
        // so the first item ("copy") lands at (1.0, 1.0), not at the raw
        // anchor origin.
        let outcome = ctl.handle(
            &UiEvent::MouseDown {
                widget: None,
                button: MouseButton::Left,
                position: Point::new(1.0, 1.0),
                modifiers: Modifiers::default(),
            },
            &mut backend,
        );
        assert_eq!(
            outcome,
            ContextMenuOutcome::Event(UiEvent::ContextMenuItemActivated(WidgetId::new("copy")))
        );
        assert!(!ctl.is_open());
    }

    #[test]
    fn mouse_moved_over_an_item_sets_hovered() {
        let mut backend = TuiBackend::new();
        let mut ctl = ContextMenuController::new();
        ctl.open(menu(), Point::new(0.0, 0.0), &mut backend);
        assert_eq!(ctl.interaction.hovered(), None);

        // Same anchored-at-origin geometry `click_on_item_activates_and_closes`
        // documents: the first item ("copy") lands at (1.0, 1.0).
        ctl.handle(
            &UiEvent::MouseMoved {
                position: Point::new(1.0, 1.0),
                buttons: Default::default(),
            },
            &mut backend,
        );
        assert_eq!(ctl.interaction.hovered(), Some(&WidgetId::new("copy")));
    }

    #[test]
    fn moving_off_every_item_clears_hovered() {
        let mut backend = TuiBackend::new();
        let mut ctl = ContextMenuController::new();
        ctl.open(menu(), Point::new(0.0, 0.0), &mut backend);
        ctl.handle(
            &UiEvent::MouseMoved {
                position: Point::new(1.0, 1.0),
                buttons: Default::default(),
            },
            &mut backend,
        );
        assert_eq!(ctl.interaction.hovered(), Some(&WidgetId::new("copy")));

        // Moving to a point outside the menu's bounds entirely.
        ctl.handle(
            &UiEvent::MouseMoved {
                position: Point::new(500.0, 500.0),
                buttons: Default::default(),
            },
            &mut backend,
        );
        assert_eq!(ctl.interaction.hovered(), None);
    }

    #[test]
    fn click_outside_dismisses() {
        let mut backend = TuiBackend::new();
        let mut ctl = ContextMenuController::new();
        ctl.open(menu(), Point::new(10.0, 10.0), &mut backend);

        let outcome = ctl.handle(
            &UiEvent::MouseDown {
                widget: None,
                button: MouseButton::Left,
                position: Point::new(0.0, 0.0),
                modifiers: Modifiers::default(),
            },
            &mut backend,
        );
        assert_eq!(
            outcome,
            ContextMenuOutcome::Event(UiEvent::ContextMenuDismissed)
        );
        assert!(!ctl.is_open());
    }

    #[test]
    fn escape_dismisses() {
        let mut backend = TuiBackend::new();
        let mut ctl = ContextMenuController::new();
        ctl.open(menu(), Point::new(0.0, 0.0), &mut backend);

        let outcome = ctl.handle(
            &UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Escape),
                modifiers: Modifiers::default(),
                repeat: false,
            },
            &mut backend,
        );
        assert_eq!(
            outcome,
            ContextMenuOutcome::Event(UiEvent::ContextMenuDismissed)
        );
        assert!(!ctl.is_open());
    }

    #[test]
    fn handle_ignores_when_nothing_open() {
        let mut backend = TuiBackend::new();
        let mut ctl = ContextMenuController::new();
        let outcome = ctl.handle(
            &UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Escape),
                modifiers: Modifiers::default(),
                repeat: false,
            },
            &mut backend,
        );
        assert_eq!(outcome, ContextMenuOutcome::Ignored);
    }

    #[test]
    fn arrows_from_a_separator_selection_step_to_the_neighbouring_items() {
        let mut backend = TuiBackend::new();
        let mut ctl = ContextMenuController::new();
        // An app-supplied `selected_idx` pointing at the separator: the
        // arrows step to the item either side of it rather than seeding at
        // an end of the menu.
        let m = ContextMenu {
            id: WidgetId::new("ctx"),
            items: vec![
                item("copy", "Copy"),
                ContextMenuItem::default(),
                item("paste", "Paste"),
            ],
            selected_idx: 1,
            bg: None,
            placement: crate::primitives::context_menu::ContextMenuPlacement::AnchorPoint,
            glyphs: Default::default(),
        };
        ctl.open(m.clone(), Point::new(0.0, 0.0), &mut backend);
        ctl.handle(&key_pressed(NamedKey::Down), &mut backend);
        assert_eq!(
            ctl.handle(&key_pressed(NamedKey::Enter), &mut backend),
            ContextMenuOutcome::Event(UiEvent::ContextMenuItemActivated(WidgetId::new("paste")))
        );

        ctl.open(m, Point::new(0.0, 0.0), &mut backend);
        ctl.handle(&key_pressed(NamedKey::Up), &mut backend);
        assert_eq!(
            ctl.handle(&key_pressed(NamedKey::Enter), &mut backend),
            ContextMenuOutcome::Event(UiEvent::ContextMenuItemActivated(WidgetId::new("copy")))
        );
    }

    fn key_pressed(key: NamedKey) -> UiEvent {
        UiEvent::KeyPressed {
            key: Key::Named(key),
            modifiers: Modifiers::default(),
            repeat: false,
        }
    }

    #[test]
    fn down_then_enter_activates_second_item() {
        let mut backend = TuiBackend::new();
        let mut ctl = ContextMenuController::new();
        ctl.open(menu(), Point::new(0.0, 0.0), &mut backend);

        let down = ctl.handle(
            &UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Down),
                modifiers: Modifiers::default(),
                repeat: false,
            },
            &mut backend,
        );
        assert_eq!(down, ContextMenuOutcome::Consumed);

        let outcome = ctl.handle(
            &UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Enter),
                modifiers: Modifiers::default(),
                repeat: false,
            },
            &mut backend,
        );
        assert_eq!(
            outcome,
            ContextMenuOutcome::Event(UiEvent::ContextMenuItemActivated(WidgetId::new("paste")))
        );
    }

    #[test]
    fn set_menu_style_custom_stays_painted_even_with_native_caps() {
        // `TuiBackend` never declares `native_menu`, so this mainly
        // guards the `set_menu_style` plumbing (Custom always paints
        // in-window) rather than exercising a real native fallback —
        // that combination is covered on the macOS side.
        let mut backend = TuiBackend::new();
        backend.set_menu_style(crate::backend::MenuStyle::Custom);
        assert_eq!(backend.menu_style(), crate::backend::MenuStyle::Custom);
        assert_eq!(backend.effective_menu_style(), ResolvedMenuStyle::Custom);
    }

    // ── Nested submenus ────────────────────────────────────────────────

    fn menu_with_submenu() -> ContextMenu {
        ContextMenu {
            id: WidgetId::new("ctx"),
            items: vec![ContextMenuItem {
                id: Some(WidgetId::new("view")),
                label: StyledText::plain("View"),
                submenu: Some(vec![item("toggle_sidebar", "Toggle Sidebar")]),
                ..Default::default()
            }],
            selected_idx: 0,
            bg: None,
            placement: crate::primitives::context_menu::ContextMenuPlacement::AnchorPoint,
            glyphs: Default::default(),
        }
    }

    /// A path entry naming an item that is not a submenu parent builds
    /// no level for itself, so a keystroke applies to the deepest level
    /// that *was* built rather than being discarded against a depth the
    /// stack never reached.
    #[test]
    fn a_path_entry_with_no_submenu_still_moves_the_level_below_it() {
        let mut backend = TuiBackend::new();
        let mut ctl = ContextMenuController::new();
        ctl.open(menu(), Point::new(0.0, 0.0), &mut backend);
        // "Copy" has no submenu, so this names a level that cannot exist.
        ctl.open.as_mut().unwrap().submenu_path = vec![0];

        let outcome = ctl.handle(
            &UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Down),
                modifiers: Modifiers::default(),
                repeat: false,
            },
            &mut backend,
        );
        assert_eq!(outcome, ContextMenuOutcome::Consumed);
        assert_eq!(
            ctl.open.as_ref().unwrap().menu.selected_idx,
            1,
            "the root menu's own highlight must move, not be dropped"
        );
    }

    #[test]
    fn right_arrow_opens_submenu_then_enter_activates_nested_item() {
        let mut backend = TuiBackend::new();
        let mut ctl = ContextMenuController::new();
        ctl.open(menu_with_submenu(), Point::new(0.0, 0.0), &mut backend);

        let opened = ctl.handle(
            &UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Right),
                modifiers: Modifiers::default(),
                repeat: false,
            },
            &mut backend,
        );
        assert_eq!(opened, ContextMenuOutcome::Consumed);
        assert!(ctl.is_open());

        let outcome = ctl.handle(
            &UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Enter),
                modifiers: Modifiers::default(),
                repeat: false,
            },
            &mut backend,
        );
        // Activation reports the nested leaf's own id regardless of
        // nesting depth.
        assert_eq!(
            outcome,
            ContextMenuOutcome::Event(UiEvent::ContextMenuItemActivated(WidgetId::new(
                "toggle_sidebar"
            )))
        );
        assert!(!ctl.is_open());
    }

    #[test]
    fn hover_over_submenu_parent_opens_it() {
        let mut backend = TuiBackend::new();
        let mut ctl = ContextMenuController::new();
        ctl.open(menu_with_submenu(), Point::new(0.0, 0.0), &mut backend);

        // Same anchored-at-origin geometry other tests document: the
        // sole item ("View") lands at (1.0, 1.0).
        let outcome = ctl.handle(
            &UiEvent::MouseMoved {
                position: Point::new(1.0, 1.0),
                buttons: Default::default(),
            },
            &mut backend,
        );
        assert_eq!(outcome, ContextMenuOutcome::Consumed);
        assert_eq!(ctl.open.as_ref().unwrap().submenu_path, vec![0]);

        // Enter now activates the nested item hover opened, not the
        // parent — proof the hover-opened submenu is the active level.
        let outcome = ctl.handle(
            &UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Enter),
                modifiers: Modifiers::default(),
                repeat: false,
            },
            &mut backend,
        );
        assert_eq!(
            outcome,
            ContextMenuOutcome::Event(UiEvent::ContextMenuItemActivated(WidgetId::new(
                "toggle_sidebar"
            )))
        );
    }

    #[test]
    fn rehovering_an_already_open_submenu_parent_preserves_its_selection() {
        let mut backend = TuiBackend::new();
        let mut ctl = ContextMenuController::new();
        let menu = ContextMenu {
            id: WidgetId::new("ctx"),
            items: vec![ContextMenuItem {
                id: Some(WidgetId::new("view")),
                label: StyledText::plain("View"),
                submenu: Some(vec![
                    item("toggle_sidebar", "Toggle Sidebar"),
                    item("toggle_minimap", "Toggle Minimap"),
                ]),
                ..Default::default()
            }],
            selected_idx: 0,
            bg: None,
            placement: crate::primitives::context_menu::ContextMenuPlacement::AnchorPoint,
            glyphs: Default::default(),
        };
        ctl.open(menu, Point::new(0.0, 0.0), &mut backend);

        // Hover opens the submenu at its first selectable item.
        ctl.handle(
            &UiEvent::MouseMoved {
                position: Point::new(1.0, 1.0),
                buttons: Default::default(),
            },
            &mut backend,
        );
        assert_eq!(ctl.open.as_ref().unwrap().submenu_selected, vec![0]);

        // Move the keyboard selection to the second nested item.
        ctl.handle(
            &UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Down),
                modifiers: Modifiers::default(),
                repeat: false,
            },
            &mut backend,
        );
        assert_eq!(ctl.open.as_ref().unwrap().submenu_selected, vec![1]);

        // Jitter the mouse back over the still-open parent row — this
        // must not re-seed the child's selection back to its first item.
        let outcome = ctl.handle(
            &UiEvent::MouseMoved {
                position: Point::new(1.0, 1.0),
                buttons: Default::default(),
            },
            &mut backend,
        );
        assert_eq!(outcome, ContextMenuOutcome::Consumed);
        assert_eq!(
            ctl.open.as_ref().unwrap().submenu_path,
            vec![0],
            "the submenu must stay open"
        );
        assert_eq!(
            ctl.open.as_ref().unwrap().submenu_selected,
            vec![1],
            "re-hovering an already-open parent must not reset the child's selection"
        );
    }

    #[test]
    fn left_arrow_closes_deepest_submenu_without_dismissing_the_menu() {
        let mut backend = TuiBackend::new();
        let mut ctl = ContextMenuController::new();
        ctl.open(menu_with_submenu(), Point::new(0.0, 0.0), &mut backend);
        ctl.handle(
            &UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Right),
                modifiers: Modifiers::default(),
                repeat: false,
            },
            &mut backend,
        );
        assert_eq!(ctl.open.as_ref().unwrap().submenu_path, vec![0]);

        let outcome = ctl.handle(
            &UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Left),
                modifiers: Modifiers::default(),
                repeat: false,
            },
            &mut backend,
        );
        assert_eq!(outcome, ContextMenuOutcome::Consumed);
        assert!(ctl.open.as_ref().unwrap().submenu_path.is_empty());
        assert!(ctl.is_open(), "the root menu stays open");
    }

    #[test]
    fn escape_closes_deepest_submenu_first_then_the_whole_menu() {
        let mut backend = TuiBackend::new();
        let mut ctl = ContextMenuController::new();
        ctl.open(menu_with_submenu(), Point::new(0.0, 0.0), &mut backend);
        ctl.handle(
            &UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Right),
                modifiers: Modifiers::default(),
                repeat: false,
            },
            &mut backend,
        );

        let first_escape = ctl.handle(
            &UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Escape),
                modifiers: Modifiers::default(),
                repeat: false,
            },
            &mut backend,
        );
        assert_eq!(first_escape, ContextMenuOutcome::Consumed);
        assert!(ctl.is_open(), "only the submenu level closed");

        let second_escape = ctl.handle(
            &UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Escape),
                modifiers: Modifiers::default(),
                repeat: false,
            },
            &mut backend,
        );
        assert_eq!(
            second_escape,
            ContextMenuOutcome::Event(UiEvent::ContextMenuDismissed)
        );
        assert!(!ctl.is_open());
    }

    #[test]
    fn click_on_submenu_parent_opens_child_and_click_on_nested_leaf_activates_it() {
        let mut backend = TuiBackend::new();
        let mut ctl = ContextMenuController::new();
        ctl.open(menu_with_submenu(), Point::new(0.0, 0.0), &mut backend);

        let opened = ctl.handle(
            &UiEvent::MouseDown {
                widget: None,
                button: MouseButton::Left,
                position: Point::new(1.0, 1.0),
                modifiers: Modifiers::default(),
            },
            &mut backend,
        );
        assert_eq!(opened, ContextMenuOutcome::Consumed);

        // The child popup opened beside the parent's right edge
        // (`ContextMenuLayout::submenu_anchor`'s default pull-right
        // side), one row down from the parent menu's top (same y as the
        // parent item it opened from).
        let stack = ctl.menu_stack(ctl.open.as_ref().unwrap(), &backend);
        assert_eq!(stack.len(), 2);
        let child_bounds = stack[1].1.bounds;

        let outcome = ctl.handle(
            &UiEvent::MouseDown {
                widget: None,
                button: MouseButton::Left,
                position: Point::new(child_bounds.x + 1.0, child_bounds.y + 0.5),
                modifiers: Modifiers::default(),
            },
            &mut backend,
        );
        assert_eq!(
            outcome,
            ContextMenuOutcome::Event(UiEvent::ContextMenuItemActivated(WidgetId::new(
                "toggle_sidebar"
            )))
        );
        assert!(!ctl.is_open());
    }

    #[test]
    fn submenu_flips_left_when_parent_is_near_the_right_edge() {
        let mut backend = TuiBackend::new();
        let mut ctl = ContextMenuController::new();
        // Anchored far enough right that the parent popup itself clamps
        // flush against the viewport's right edge, leaving no room for
        // the child to pull right of it.
        ctl.open(menu_with_submenu(), Point::new(60.0, 0.0), &mut backend);
        ctl.handle(
            &UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Right),
                modifiers: Modifiers::default(),
                repeat: false,
            },
            &mut backend,
        );

        let stack = ctl.menu_stack(ctl.open.as_ref().unwrap(), &backend);
        assert_eq!(stack.len(), 2);
        let parent_bounds = stack[0].1.bounds;
        let child_bounds = stack[1].1.bounds;
        // Flipped left: child's right edge sits at the parent's left
        // edge (minus the 1-unit gap `submenu_anchor` leaves).
        assert!(child_bounds.x + child_bounds.width <= parent_bounds.x);
    }
}
