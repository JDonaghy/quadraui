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
use crate::types::WidgetId;
use crate::Modifiers;

// ─── Drag state ─────────────────────────────────────────────────────────────

/// What's being dragged, if anything. Backends hold one [`DragState`]
/// (typically on the same struct that owns the [`ModalStack`]) and
/// update it from the click / drag / release handlers via
/// [`DragState::begin`] / [`DragState::end`]. The dispatch functions
/// below consult it to decide which primitive-specific event to emit.
#[derive(Debug, Clone, PartialEq)]
pub enum DragTarget {
    /// A vertical scrollbar drag. `track_start` and `track_length`
    /// are in the backend's native units (pixels for GTK; cells for
    /// TUI) and define the track region the thumb can traverse.
    /// `visible_rows` and `total_items` are counts in rows, so the
    /// dispatcher can compute `max_scroll = total - visible` without
    /// assuming the coordinate system. The dispatcher maps a drag
    /// point's y to a scroll offset via linear interpolation:
    ///
    /// ```text
    /// rel    = (y - track_start) / track_length   (clamped 0..=1)
    /// offset = round(rel * max_scroll)
    /// ```
    ScrollbarY {
        /// Which widget's scrollbar is being dragged. Used to route
        /// the resulting event back to the right primitive.
        widget: WidgetId,
        /// Track top in the backend's native y-coordinate.
        track_start: f32,
        /// Track length in the backend's native units. Must be > 0.
        track_length: f32,
        /// Number of items currently fitting inside the scroll
        /// viewport. Determines `max_scroll = total.saturating_sub(visible)`.
        visible_rows: usize,
        /// Total number of items in the scrolled list.
        total_items: usize,
    },
}

/// One drag in progress, or none. Backends hold one instance; call
/// [`Self::begin`] on mouse-down over a draggable region and
/// [`Self::end`] on mouse-up. The dispatch functions here read it to
/// decide whether a mouse-move should produce a drag-update event.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DragState {
    current: Option<DragTarget>,
}

impl DragState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Start tracking a drag. Overwrites any previous state —
    /// backends are expected to call [`Self::end`] on mouse-up before
    /// beginning the next drag, but overwriting is the safer default
    /// than panicking (spurious duplicate begins do happen in
    /// gesture-heavy paths).
    pub fn begin(&mut self, target: DragTarget) {
        self.current = Some(target);
    }

    /// Clear the drag. No-op if nothing is in progress.
    pub fn end(&mut self) {
        self.current = None;
    }

    pub fn is_active(&self) -> bool {
        self.current.is_some()
    }

    pub fn target(&self) -> Option<&DragTarget> {
        self.current.as_ref()
    }
}

// ─── Dispatch functions ─────────────────────────────────────────────────────

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

/// Translate a mouse-move event. When no drag is in progress, emits a
/// plain [`UiEvent::MouseMoved`]. When a [`DragTarget::ScrollbarY`]
/// drag is active, additionally emits a generic
/// [`UiEvent::ScrollOffsetChanged { widget, new_offset }`] with the
/// derived scroll offset. The app's dispatch matches on the event
/// (and switches on `widget` to route to the right scroll-state
/// field) without needing the track geometry — this function owns
/// the translation.
///
/// # How the offset is computed
///
/// `ratio = ((point.y - track_start) / track_length).clamp(0, 1)`
/// `new_offset = round(ratio * max_scroll)` where
/// `max_scroll = total_items.saturating_sub(visible_rows)` and
/// `visible_rows = track_length as usize` (one row per unit of track).
///
/// This mirrors the math TUI already uses in `mouse.rs`'s
/// `dragging_picker_sb` branch, extended to f32 so it works for
/// pixel-unit backends (GTK, macOS) as well as cell-unit backends
/// (TUI).
pub fn dispatch_mouse_drag(
    drag: &DragState,
    position: Point,
    buttons: crate::event::ButtonMask,
) -> Vec<UiEvent> {
    let mut events = vec![UiEvent::MouseMoved { position, buttons }];

    if let Some(DragTarget::ScrollbarY {
        widget,
        track_start,
        track_length,
        visible_rows,
        total_items,
    }) = drag.target()
    {
        if *track_length > 0.0 && *total_items > 0 {
            // Scroll math accounts for the thumb occupying part of the
            // track. The thumb's height is `visible/total * track`;
            // the mouse can only drive the thumb through the remaining
            // `effective_track = track - thumb` before the thumb hits
            // the bottom. Without this adjustment the mouse feels
            // ~track/(track - thumb) times faster than the thumb itself,
            // which users perceive as laggy drag.
            let thumb_ratio = (*visible_rows as f32 / *total_items as f32).min(1.0);
            let thumb_length = (*track_length * thumb_ratio).max(1.0);
            let effective_track = (*track_length - thumb_length).max(1.0);
            let rel = (position.y - *track_start) / effective_track;
            let clamped = rel.clamp(0.0, 1.0);
            let max_scroll = total_items.saturating_sub(*visible_rows);
            let new_offset = (clamped * max_scroll as f32).round() as usize;
            events.push(UiEvent::ScrollOffsetChanged {
                widget: widget.clone(),
                new_offset,
            });
        }
    }

    events
}

/// Translate a mouse-up event. Always emits [`UiEvent::MouseUp`] and
/// clears any active drag state. If the click landed inside a modal,
/// the `MouseUp` carries the modal's `widget` id — matching
/// [`dispatch_mouse_down`]'s precedence — so apps can treat a drag
/// that crosses the modal boundary atomically.
pub fn dispatch_mouse_up(
    stack: &ModalStack,
    drag: &mut DragState,
    position: Point,
    button: MouseButton,
) -> Vec<UiEvent> {
    drag.end();
    let widget = stack.hit_test(position).cloned();
    vec![UiEvent::MouseUp {
        widget,
        button,
        position,
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

    // ── Drag tests ────────────────────────────────────────────────────

    fn buttons_mask_left() -> crate::event::ButtonMask {
        crate::event::ButtonMask {
            left: true,
            ..Default::default()
        }
    }

    #[test]
    fn drag_state_begin_and_end() {
        let mut drag = DragState::new();
        assert!(!drag.is_active());
        drag.begin(DragTarget::ScrollbarY {
            widget: id("picker"),
            track_start: 100.0,
            track_length: 200.0,
            visible_rows: 10,
            total_items: 50,
        });
        assert!(drag.is_active());
        match drag.target().unwrap() {
            DragTarget::ScrollbarY { widget, .. } => assert_eq!(widget, &id("picker")),
        }
        drag.end();
        assert!(!drag.is_active());
        assert!(drag.target().is_none());
    }

    #[test]
    fn dispatch_mouse_drag_without_drag_emits_only_moved() {
        let drag = DragState::new();
        let events = dispatch_mouse_drag(&drag, pt(50.0, 50.0), buttons_mask_left());
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], UiEvent::MouseMoved { .. }));
    }

    #[test]
    fn dispatch_mouse_drag_with_scrollbar_emits_scroll_offset_changed() {
        // Track 80 units from y=100; 100 items, viewport shows 20.
        // thumb_ratio = 20/100 = 0.2 → thumb_length = 16
        // effective_track = 64; max_scroll = 80
        // Mouse at y=100+32 (halfway through effective_track) → offset 40.
        let mut drag = DragState::new();
        drag.begin(DragTarget::ScrollbarY {
            widget: id("picker"),
            track_start: 100.0,
            track_length: 80.0,
            visible_rows: 20,
            total_items: 100,
        });
        let events = dispatch_mouse_drag(&drag, pt(500.0, 132.0), buttons_mask_left());
        assert_eq!(events.len(), 2);
        assert!(matches!(events[0], UiEvent::MouseMoved { .. }));
        match &events[1] {
            UiEvent::ScrollOffsetChanged { widget, new_offset } => {
                assert_eq!(widget, &id("picker"));
                assert_eq!(*new_offset, 40);
            }
            other => panic!("expected ScrollOffsetChanged, got {:?}", other),
        }
    }

    #[test]
    fn dispatch_mouse_drag_clamps_above_and_below_track() {
        // Same geometry as above: max_scroll = 80, effective_track = 64.
        let mut drag = DragState::new();
        drag.begin(DragTarget::ScrollbarY {
            widget: id("p"),
            track_start: 100.0,
            track_length: 80.0,
            visible_rows: 20,
            total_items: 100,
        });
        // Above track: offset = 0.
        let events = dispatch_mouse_drag(&drag, pt(0.0, 50.0), buttons_mask_left());
        match &events[1] {
            UiEvent::ScrollOffsetChanged { new_offset, .. } => {
                assert_eq!(*new_offset, 0);
            }
            _ => panic!(),
        }
        // Below effective track: clamped to max_scroll = 80.
        let events = dispatch_mouse_drag(&drag, pt(0.0, 500.0), buttons_mask_left());
        match &events[1] {
            UiEvent::ScrollOffsetChanged { new_offset, .. } => {
                assert_eq!(*new_offset, 80);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn dispatch_mouse_drag_with_zero_track_does_not_crash_or_emit() {
        // Pathological input — no track. Should emit only MouseMoved.
        let mut drag = DragState::new();
        drag.begin(DragTarget::ScrollbarY {
            widget: id("p"),
            track_start: 0.0,
            track_length: 0.0,
            visible_rows: 10,
            total_items: 100,
        });
        let events = dispatch_mouse_drag(&drag, pt(0.0, 0.0), buttons_mask_left());
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], UiEvent::MouseMoved { .. }));
    }

    #[test]
    fn dispatch_mouse_up_clears_drag_state() {
        let mut drag = DragState::new();
        drag.begin(DragTarget::ScrollbarY {
            widget: id("p"),
            track_start: 0.0,
            track_length: 10.0,
            visible_rows: 10,
            total_items: 20,
        });
        let stack = ModalStack::new();
        let events = dispatch_mouse_up(&stack, &mut drag, pt(5.0, 5.0), MouseButton::Left);
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], UiEvent::MouseUp { .. }));
        assert!(!drag.is_active());
    }

    #[test]
    fn dispatch_mouse_up_carries_modal_widget_if_over() {
        let mut stack = ModalStack::new();
        stack.push(id("palette"), rect(0.0, 0.0, 100.0, 100.0));
        let mut drag = DragState::new();
        let events = dispatch_mouse_up(&stack, &mut drag, pt(50.0, 50.0), MouseButton::Left);
        match &events[0] {
            UiEvent::MouseUp { widget, .. } => {
                assert_eq!(widget.as_ref().unwrap(), &id("palette"));
            }
            _ => panic!(),
        }
    }
}
