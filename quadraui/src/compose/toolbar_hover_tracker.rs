//! `ToolbarHoverTracker` — host-side state holder that converts
//! `MouseMoved` events into `hovered_id: Option<WidgetId>` for the
//! `Toolbar` primitive.
//!
//! The toolbar rasterisers accept a `hovered_id` so they can tint the
//! matching button's background, but the host has to track which
//! button the mouse is over across mouse-move events — the rasteriser
//! itself is stateless. Doing that by hand means converting from
//! screen-space coordinates back through a `ToolbarLayout` per move
//! event, plus remembering to clear the hover when the cursor leaves
//! the bar. Easy to forget; easy to get wrong.
//!
//! This helper owns that bookkeeping: feed it mouse-move events
//! together with the current layout, ask it for the active
//! `hovered_id` at render time, and pass that straight to
//! `Backend::draw_toolbar`.
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
//!             let layout = backend.toolbar_layout(self.bar_rect(backend), &self.bar());
//!             if self.hover.update(&layout, position.x, position.y) {
//!                 return Reaction::Redraw;
//!             }
//!         }
//!         // ...
//!     }
//!
//!     fn render(&self, backend: &mut dyn Backend, _area: ()) {
//!         backend.draw_toolbar(
//!             self.bar_rect(backend),
//!             &self.bar(),
//!             self.hover.hovered_id(),
//!             None,
//!         );
//!     }
//! }
//! ```

use crate::primitives::toolbar::{ToolbarHit, ToolbarLayout};
use crate::types::WidgetId;

/// Tracks the toolbar button currently under the mouse. Cheap to keep
/// per toolbar (or per panel, if a panel hosts multiple) — one
/// `Option<WidgetId>` plus a single `hit_test` call per mouse-move.
#[derive(Debug, Default, Clone)]
pub struct ToolbarHoverTracker {
    hovered: Option<WidgetId>,
}

impl ToolbarHoverTracker {
    /// Construct an empty tracker. No button is hovered initially.
    pub fn new() -> Self {
        Self { hovered: None }
    }

    /// Update the hover state from a mouse-position + the current
    /// toolbar layout. Returns `true` if the hovered button changed
    /// (caller should redraw), `false` if it stayed the same.
    pub fn update(&mut self, layout: &ToolbarLayout, x: f32, y: f32) -> bool {
        let new_hover = match layout.hit_test(x, y) {
            ToolbarHit::Button(id) => Some(id),
            ToolbarHit::Empty => None,
        };
        if new_hover != self.hovered {
            self.hovered = new_hover;
            true
        } else {
            false
        }
    }

    /// Clear the hover state — call from `MouseLeft` handlers (or
    /// when the toolbar is dismissed) so the highlight doesn't stick
    /// after the cursor has left.
    pub fn clear(&mut self) -> bool {
        if self.hovered.is_some() {
            self.hovered = None;
            true
        } else {
            false
        }
    }

    /// Borrowed view of the current hovered id, ready to pass straight
    /// to `Backend::draw_toolbar(..., hovered_id, ...)`.
    pub fn hovered_id(&self) -> Option<&WidgetId> {
        self.hovered.as_ref()
    }

    /// Owned copy of the current hovered id. Convenience for hosts
    /// that need to compare against other state.
    pub fn current(&self) -> Option<WidgetId> {
        self.hovered.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::Rect;
    use crate::primitives::toolbar::{Toolbar, ToolbarButton, ToolbarItemMeasure};

    fn mk_action(id: &str, label: &str) -> ToolbarButton {
        ToolbarButton::Action {
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

    #[test]
    fn _layout_unused_warning_silencer() {
        // Reference Rect so the import isn't flagged unused on builds
        // that disable some test arms.
        let _r = Rect::new(0.0, 0.0, 1.0, 1.0);
    }
}
