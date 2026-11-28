//! Host-side hover trackers: convert `MouseMoved` events into the
//! "which item is under the cursor" answer a primitive's rasteriser
//! needs, without the host hand-rolling screen-space-to-layout
//! conversion itself.
//!
//! [`ToolbarHoverTracker`] is a thin, item-typed wrapper over the
//! private [`HoverCore`] engine: feed it a layout's `hit_test` result
//! on every mouse move, and it tells you whether the hovered item
//! changed (so you know to redraw) and what it is now.
//!
//! ## Usage
//!
//! ```ignore
//! struct App {
//!     hover: ToolbarHoverTracker,
//!     // ...
//! }
//!
//! impl AppLogic for App {
//!     fn handle(&mut self, ev: UiEvent, backend: &mut dyn Backend) -> Reaction {
//!         if let UiEvent::MouseMoved { position, .. } = ev {
//!             // The widget contract's layout is bar-local.
//!             let rect = self.bar_rect(backend);
//!             let layout = backend.layout_primitive(rect, &self.bar()).layout;
//!             if self.hover.update(&layout, position.x - rect.x, position.y - rect.y) {
//!                 return Reaction::Redraw;
//!             }
//!         }
//!         // ...
//!     }
//!
//!     fn render(&self, backend: &mut dyn Backend, _area: ()) {
//!         backend.draw_dyn(self.bar_rect(backend), &self.bar(), &());
//!     }
//! }
//! ```

use crate::primitives::toolbar::ToolbarLayout;
use crate::types::WidgetId;

/// Shared bookkeeping every tracker in this module wraps: "what's the
/// current item, and did it just change?" — one `Option<T>` plus the
/// equality check every `update`/`clear` call needs. Not `pub`: each
/// primitive's tracker is the public-facing, correctly-typed wrapper.
#[derive(Debug, Clone)]
struct HoverCore<T: Clone + PartialEq> {
    current: Option<T>,
}

// Manual `Default` rather than `#[derive(Default)]`: the derive would
// require `T: Default` even though `Option<T>`'s own `None` default
// doesn't need one — and every tracker's item type here (`WidgetId`,
// `usize`) is `Copy`/`Clone` but not all are `Default`.
impl<T: Clone + PartialEq> Default for HoverCore<T> {
    fn default() -> Self {
        Self { current: None }
    }
}

impl<T: Clone + PartialEq> HoverCore<T> {
    fn new() -> Self {
        Self { current: None }
    }

    /// Returns `true` if `new` differs from the stored value (caller
    /// should redraw).
    fn set(&mut self, new: Option<T>) -> bool {
        if new != self.current {
            self.current = new;
            true
        } else {
            false
        }
    }

    fn clear(&mut self) -> bool {
        self.set(None)
    }

    fn current(&self) -> Option<&T> {
        self.current.as_ref()
    }
}

/// Tracks the toolbar button currently under the mouse. Cheap to keep
/// per toolbar (or per panel, if a panel hosts multiple) — one
/// `Option<WidgetId>` plus a single `hit_test` call per mouse-move.
#[derive(Debug, Default, Clone)]
pub struct ToolbarHoverTracker {
    core: HoverCore<WidgetId>,
}

impl ToolbarHoverTracker {
    /// Construct an empty tracker. No button is hovered initially.
    pub fn new() -> Self {
        Self {
            core: HoverCore::new(),
        }
    }

    /// Update the hover state from a mouse-position + the current
    /// toolbar layout. Returns `true` if the hovered button changed
    /// (caller should redraw), `false` if it stayed the same.
    pub fn update(&mut self, layout: &ToolbarLayout, x: f32, y: f32) -> bool {
        // The widget's own hit-to-hovered-button classification, shared
        // with `impl Interactive for Toolbar`'s pointer arms so a host on
        // the legacy `Backend` path and one on the widget contract can
        // never highlight different cells — see that function's doc for
        // which hits hover and which don't.
        self.core.set(crate::primitives::toolbar::hover_target(
            &layout.hit_test(x, y),
        ))
    }

    /// Clear the hover state — call from `MouseLeft` handlers (or
    /// when the toolbar is dismissed) so the highlight doesn't stick
    /// after the cursor has left.
    pub fn clear(&mut self) -> bool {
        self.core.clear()
    }

    /// Borrowed view of the current hovered id, ready to pass straight
    /// to `Backend::draw_toolbar(..., hovered_id, ...)`.
    pub fn hovered_id(&self) -> Option<&WidgetId> {
        self.core.current()
    }

    /// Owned copy of the current hovered id. Convenience for hosts
    /// that need to compare against other state.
    pub fn current(&self) -> Option<WidgetId> {
        self.core.current().copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::toolbar::{Toolbar, ToolbarButton, ToolbarItemMeasure};

    fn mk_action(id: &str, label: &str) -> ToolbarButton {
        ToolbarButton::Action {
            spec: None,
            id: WidgetId::new(id),
            label: label.to_string(),
            icon: None,
            key_hint: None,
            enabled: true,
            is_active: false,
            tooltip: String::new(),
        }
    }

    fn sample_layout() -> ToolbarLayout {
        let bar = Toolbar {
            id: WidgetId::new("tb"),
            buttons: vec![mk_action("a", "A"), mk_action("b", "B")],
            bg: None,
            focused_index: None,
            ..Default::default()
        };
        bar.layout(0.0, 0.0, 40.0, 1.0, |_| ToolbarItemMeasure::new(6.0))
    }

    #[test]
    fn update_records_button_under_cursor() {
        let layout = sample_layout();
        let mut t = ToolbarHoverTracker::new();
        assert!(t.update(&layout, 1.0, 0.0));
        assert_eq!(t.hovered_id().map(|i| i.as_str()), Some("a"));
    }

    #[test]
    fn update_returns_false_when_unchanged() {
        let layout = sample_layout();
        let mut t = ToolbarHoverTracker::new();
        // First update changes (None -> Some(a)).
        assert!(t.update(&layout, 1.0, 0.0));
        // Second update with the same position should not signal change.
        assert!(!t.update(&layout, 2.0, 0.0));
    }

    #[test]
    fn update_returns_true_when_moving_between_buttons() {
        let layout = sample_layout();
        let mut t = ToolbarHoverTracker::new();
        assert!(t.update(&layout, 1.0, 0.0)); // hits "a"
        assert!(t.update(&layout, 7.0, 0.0)); // hits "b"
        assert_eq!(t.hovered_id().map(|i| i.as_str()), Some("b"));
    }

    #[test]
    fn update_clears_when_cursor_leaves_buttons() {
        let layout = sample_layout();
        let mut t = ToolbarHoverTracker::new();
        assert!(t.update(&layout, 1.0, 0.0));
        // Far past the right edge of the painted items.
        assert!(t.update(&layout, 30.0, 0.0));
        assert!(t.hovered_id().is_none());
    }

    #[test]
    fn clear_resets_hover_state() {
        let layout = sample_layout();
        let mut t = ToolbarHoverTracker::new();
        t.update(&layout, 1.0, 0.0);
        assert!(t.clear());
        assert!(t.hovered_id().is_none());
        // Clearing an already-empty tracker is a no-op (returns false).
        assert!(!t.clear());
    }
}
