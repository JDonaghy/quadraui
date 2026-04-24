//! Cross-backend mouse dispatch.
//!
//! Backends call into these free functions with raw (platform-translated)
//! mouse events. Quadraui consults the [`ModalStack`] first, decides
//! which widget — if any — should receive the event, and returns a
//! `Vec<UiEvent>` the backend pushes onto its per-frame event queue.
//!
//! The key guarantee: **events landing inside an open modal cannot fall
//! through to widgets behind it**. This is the contract vimcode's
//! TUI `mouse.rs` enforces inline; centralising it here eliminates the
//! class of bug where a new modal is added to one backend but forgotten
//! in another (issue #192 is the motivating case).
//!
//! # What's here in the pilot
//!
//! [`dispatch_mouse_down`] only. Mouse-up, drag, and scroll dispatch
//! arrive in follow-up commits (per the B.4 event-routing plan).
//!
//! # What's explicitly not here
//!
//! - Inner hit refinement within a modal — `Palette`, `Dialog`, etc.
//!   have their own `*Layout::hit_test` for that. The dispatcher
//!   identifies the topmost modal under the cursor; the app still
//!   calls the primitive's hit test afterward if it needs an
//!   item-level target.
//! - Base-layer hit testing — the editor, sidebar, tabs, and so on.
//!   The pilot leaves base-layer events going through the backend's
//!   existing mouse handlers, which are already per-backend. Later
//!   commits can route them through here too.

use crate::event::{MouseButton, Point, UiEvent};
use crate::modal_stack::ModalStack;
use crate::primitives::palette::PaletteEvent;
use crate::Modifiers;

/// Translate a raw mouse-down event into a `Vec<UiEvent>`, consulting
/// the modal stack first.
///
/// # Returns
///
/// Three cases:
///
/// 1. **Click landed on an open modal.** Emits
///    `[UiEvent::MouseDown { widget: Some(id), .. }]`. The app's
///    dispatch matches on the widget id and routes to the modal's
///    inner hit-test if it needs finer resolution.
/// 2. **Click landed outside every open modal, but a modal is open.**
///    Emits `[UiEvent::MouseDown { widget: None, .. }, UiEvent::Palette(id, Closed)]`
///    where `id` is the topmost modal (the one the backdrop click
///    dismisses). This is the "click outside to close" convention every
///    desktop platform follows. **Base-layer widgets must not receive
///    the event** — the event vec doesn't include a second
///    `MouseDown` for a base widget, and the caller should consume on
///    the emission of `PaletteEvent::Closed`.
/// 3. **No modal open.** Emits `[UiEvent::MouseDown { widget: None, .. }]`
///    with no primitive event; the backend's existing base-layer
///    mouse handlers deal with it as they did before the pilot.
///
/// Callers that only care about case 1 (e.g. a GTK event handler that
/// wants to stop the drag from leaking to the editor) can check the
/// returned vec:
///
/// ```ignore
/// let events = dispatch_mouse_down(&stack, pos, button, mods);
/// if events.iter().any(|e| matches!(e, UiEvent::MouseDown { widget: Some(_), .. })
///     || matches!(e, UiEvent::Palette(_, PaletteEvent::Closed)))
/// {
///     // modal consumed the click — don't run base-layer dispatch
///     return;
/// }
/// ```
///
/// # Note on `PaletteEvent::Closed`
///
/// Today this dispatcher always emits [`PaletteEvent::Closed`] for
/// backdrop clicks regardless of which primitive type is topmost on
/// the stack. That's a pilot-scope simplification — the palette is
/// the only consumer wired up in commit 1. When a second modal type
/// needs the backdrop-dismiss behaviour (e.g. Dialog), we'll generalise
/// to a `ModalDismissed(WidgetId)` event or per-primitive variants.
pub fn dispatch_mouse_down(
    stack: &ModalStack,
    position: Point,
    button: MouseButton,
    modifiers: Modifiers,
) -> Vec<UiEvent> {
    // Case 1: click landed inside an open modal.
    if let Some(widget_id) = stack.hit_test(position) {
        return vec![UiEvent::MouseDown {
            widget: Some(widget_id.clone()),
            button,
            position,
            modifiers,
        }];
    }

    // Case 2: modal(s) open but click was outside them → dismiss topmost.
    if let Some(top) = stack.top() {
        return vec![
            UiEvent::MouseDown {
                widget: None,
                button,
                position,
                modifiers,
            },
            UiEvent::Palette(top.id.clone(), PaletteEvent::Closed),
        ];
    }

    // Case 3: no modals open. Event belongs to the base layer.
    vec![UiEvent::MouseDown {
        widget: None,
        button,
        position,
        modifiers,
    }]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::Rect;
    use crate::types::WidgetId;

    fn id(s: &str) -> WidgetId {
        WidgetId::new(s)
    }

    fn pt(x: f32, y: f32) -> Point {
        Point { x, y }
    }

    fn rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect {
            x,
            y,
            width: w,
            height: h,
        }
    }

    #[test]
    fn click_inside_modal_emits_single_mousedown_with_widget() {
        let mut stack = ModalStack::new();
        stack.push(id("palette"), rect(10.0, 10.0, 100.0, 100.0));
        let events = dispatch_mouse_down(
            &stack,
            pt(50.0, 50.0),
            MouseButton::Left,
            Modifiers::default(),
        );
        assert_eq!(events.len(), 1);
        match &events[0] {
            UiEvent::MouseDown {
                widget, button, ..
            } => {
                assert_eq!(widget.as_ref().unwrap(), &id("palette"));
                assert_eq!(*button, MouseButton::Left);
            }
            _ => panic!("expected MouseDown, got {:?}", events[0]),
        }
    }

    #[test]
    fn click_outside_open_modal_dismisses_topmost() {
        let mut stack = ModalStack::new();
        stack.push(id("palette"), rect(10.0, 10.0, 50.0, 50.0));
        // Click well outside the palette's bounds.
        let events = dispatch_mouse_down(
            &stack,
            pt(500.0, 500.0),
            MouseButton::Left,
            Modifiers::default(),
        );
        assert_eq!(events.len(), 2);
        // First event: MouseDown with widget None (backdrop click).
        assert!(matches!(
            &events[0],
            UiEvent::MouseDown { widget: None, .. }
        ));
        // Second event: palette Closed.
        match &events[1] {
            UiEvent::Palette(wid, PaletteEvent::Closed) => {
                assert_eq!(wid, &id("palette"));
            }
            _ => panic!("expected Palette::Closed, got {:?}", events[1]),
        }
    }

    #[test]
    fn click_when_no_modal_open_emits_single_mousedown_with_no_widget() {
        let stack = ModalStack::new();
        let events = dispatch_mouse_down(
            &stack,
            pt(100.0, 100.0),
            MouseButton::Left,
            Modifiers::default(),
        );
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            UiEvent::MouseDown { widget: None, .. }
        ));
    }

    #[test]
    fn stacked_modals_click_inside_top_targets_top() {
        // Palette open; then a dialog on top of it. Click in the
        // overlap region should target the dialog, not the palette.
        let mut stack = ModalStack::new();
        stack.push(id("palette"), rect(0.0, 0.0, 200.0, 200.0));
        stack.push(id("dialog"), rect(50.0, 50.0, 100.0, 100.0));
        let events = dispatch_mouse_down(
            &stack,
            pt(100.0, 100.0),
            MouseButton::Left,
            Modifiers::default(),
        );
        match &events[0] {
            UiEvent::MouseDown { widget, .. } => {
                assert_eq!(widget.as_ref().unwrap(), &id("dialog"));
            }
            _ => panic!(),
        }
    }

    #[test]
    fn stacked_modals_click_inside_lower_targets_lower() {
        // Click lands in palette's bounds but outside the dialog on top.
        // The lower-modal id is what should be reported — the click is
        // still inside a modal, so no backdrop-dismiss.
        let mut stack = ModalStack::new();
        stack.push(id("palette"), rect(0.0, 0.0, 200.0, 200.0));
        stack.push(id("dialog"), rect(50.0, 50.0, 100.0, 100.0));
        let events = dispatch_mouse_down(
            &stack,
            pt(10.0, 10.0),
            MouseButton::Left,
            Modifiers::default(),
        );
        assert_eq!(events.len(), 1);
        match &events[0] {
            UiEvent::MouseDown { widget, .. } => {
                assert_eq!(widget.as_ref().unwrap(), &id("palette"));
            }
            _ => panic!(),
        }
    }
}
