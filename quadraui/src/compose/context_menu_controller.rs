//! `ContextMenuController` — the "one call" context-menu entry point
//! issue #1187 asks for, composed on top of the existing
//! [`ContextMenu`] primitive, [`Backend::draw_context_menu`] rasteriser,
//! [`Backend::show_context_menu`] native path, and [`ModalStack`].
//!
//! An app that wants a right-click menu writes exactly one code path,
//! regardless of platform or [`MenuStyle`]:
//!
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
//! # Scope (v1)
//!
//! Single-level menus only — an item's `submenu` field is accepted by
//! [`crate::ContextMenu`] but this controller doesn't open nested
//! pull-right popups for it yet (unlike [`crate::compose::MenuSystem`],
//! which does, for menu-bar dropdowns). A submenu item still renders
//! (with its `▶` affordance where the rasteriser draws one) but clicking
//! it is inert — no [`ContextMenuOutcome::Event`], just `Consumed`. Real
//! nested-submenu support is a follow-up if a consumer's right-click
//! menu needs one; [`crate::compose::MenuSystem`]'s `submenu_path`/
//! `submenu_selected` machinery is the pattern to lift when it lands.
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

use crate::backend::{Backend, ResolvedMenuStyle};
use crate::event::{Point, Rect, UiEvent};
use crate::primitives::context_menu::{ContextMenu, ContextMenuHit, ContextMenuLayout};
use crate::{Key, NamedKey};

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
}

#[derive(Debug, Clone)]
struct OpenMenu {
    menu: ContextMenu,
    anchor: Point,
}

/// Item height / separator height / menu width in line-height units —
/// the same constants [`crate::compose::MenuSystem::dropdown_stack`]
/// uses, kept here rather than shared so this controller has no
/// dependency on `MenuSystem`'s bar-anchored internals.
fn item_measure(lh: f32) -> (f32, f32, f32) {
    let item_h = (lh * 1.4).round().max(lh);
    let sep_h = (lh * 0.5).round().max(1.0);
    let menu_width = 20.0 * lh;
    (item_h, sep_h, menu_width)
}

impl ContextMenuController {
    pub fn new() -> Self {
        Self { open: None }
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
                let id = menu.id.clone();
                let layout = self.layout_for(&menu, anchor, backend);
                backend
                    .modal_stack_handle()
                    .borrow_mut()
                    .push(id, layout.bounds);
                self.open = Some(OpenMenu { menu, anchor });
            }
        }
    }

    /// Close the painted menu, if one is open. A no-op on the `Native`
    /// path (AppKit owns dismissal there) and if nothing is open.
    pub fn close(&mut self, backend: &mut dyn Backend) {
        if let Some(open) = self.open.take() {
            backend.modal_stack_handle().borrow_mut().pop(&open.menu.id);
        }
    }

    /// Process one event against the currently open painted menu.
    /// Returns [`ContextMenuOutcome::Ignored`] immediately if nothing is
    /// open — safe to call unconditionally before an app's own routing,
    /// same contract as [`crate::compose::MenuSystem::handle`].
    pub fn handle(&mut self, event: &UiEvent, backend: &mut dyn Backend) -> ContextMenuOutcome {
        let Some(open) = self.open.clone() else {
            return ContextMenuOutcome::Ignored;
        };

        match event {
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Escape),
                ..
            } => {
                self.close(backend);
                ContextMenuOutcome::Event(UiEvent::ContextMenuDismissed)
            }
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Down),
                ..
            } => {
                let next = open.menu.move_selection(open.menu.selected_idx, 1);
                self.set_selected(next);
                ContextMenuOutcome::Consumed
            }
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Up),
                ..
            } => {
                let next = open.menu.move_selection(open.menu.selected_idx, -1);
                self.set_selected(next);
                ContextMenuOutcome::Consumed
            }
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Enter),
                ..
            } => {
                let item = open.menu.items.get(open.menu.selected_idx);
                match item.and_then(|i| i.id.clone()) {
                    Some(id) => {
                        self.close(backend);
                        ContextMenuOutcome::Event(UiEvent::ContextMenuItemActivated(id))
                    }
                    None => ContextMenuOutcome::Consumed,
                }
            }
            UiEvent::MouseDown { position, .. } => {
                let layout = self.layout_for(&open.menu, open.anchor, backend);
                match layout.hit_test(position.x, position.y) {
                    ContextMenuHit::Item(id) => {
                        self.close(backend);
                        ContextMenuOutcome::Event(UiEvent::ContextMenuItemActivated(id))
                    }
                    ContextMenuHit::Inert => ContextMenuOutcome::Consumed,
                    ContextMenuHit::Empty => {
                        self.close(backend);
                        ContextMenuOutcome::Event(UiEvent::ContextMenuDismissed)
                    }
                }
            }
            _ => ContextMenuOutcome::Ignored,
        }
    }

    /// Paint the currently open painted menu, if any. A no-op on the
    /// `Native` path (nothing to paint — AppKit owns that surface) and
    /// if nothing is open. Call from `render`, after the app's own
    /// content so the menu layers on top.
    pub fn render(&self, backend: &mut dyn Backend) {
        let Some(open) = &self.open else {
            return;
        };
        let layout = self.layout_for(&open.menu, open.anchor, backend);
        let _ = backend.draw_context_menu(&open.menu, &layout);
    }

    fn set_selected(&mut self, idx: usize) {
        if let Some(open) = &mut self.open {
            open.menu.selected_idx = idx;
        }
    }

    fn layout_for(
        &self,
        menu: &ContextMenu,
        anchor: Point,
        backend: &dyn Backend,
    ) -> ContextMenuLayout {
        let lh = backend.line_height();
        let (item_h, sep_h, menu_width) = item_measure(lh);
        let viewport = backend.viewport();
        let vp = Rect::new(0.0, 0.0, viewport.width, viewport.height);
        menu.layout(anchor.x, anchor.y, vp, menu_width, |i| {
            if menu.items[i].is_separator() {
                crate::primitives::context_menu::ContextMenuItemMeasure::new(sep_h)
            } else {
                crate::primitives::context_menu::ContextMenuItemMeasure::new(item_h)
            }
        })
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
    use crate::MouseButton;

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

        // Item "copy" is the first row at the anchor origin.
        let outcome = ctl.handle(
            &UiEvent::MouseDown {
                widget: None,
                button: MouseButton::Left,
                position: Point::new(1.0, 0.0),
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
        // guards the `set_menu_style` plumbing (issue #1187's "Custom
        // always paints in-window" rule) rather than exercising a real
        // native fallback — that combination is covered on the macOS
        // side (`src/macos/backend.rs`).
        let mut backend = TuiBackend::new();
        backend.set_menu_style(crate::backend::MenuStyle::Custom);
        assert_eq!(backend.menu_style(), crate::backend::MenuStyle::Custom);
        assert_eq!(backend.effective_menu_style(), ResolvedMenuStyle::Custom);
    }
}
