//! `Toast` primitive: a transient corner notification with optional
//! severity tint and optional action button. Used for "File saved",
//! "LSP disconnected", "3 errors in src/foo.rs", etc.
//!
//! Toasts are ephemeral — the app owns their lifecycle (show, auto-dismiss
//! after a duration, manual dismiss) and passes the primitive the current
//! set of visible toasts each frame. The primitive itself does not tick
//! time or auto-dismiss; those are app concerns.
//!
//! # Backend contract
//!
//! **Declarative + overlay.** Render toasts stacked in the configured
//! `corner`, with each toast a box of (title, body, optional action
//! button). Clicks on the action button emit
//! `ToastEvent::ActionClicked { id }`; clicks on the dismiss affordance
//! emit `Dismissed { id }`. Toast boxes don't take keyboard focus —
//! they're strictly a notification surface.
//!
//! Stacking direction: bottom-corner toasts grow upward (newest nearest
//! the corner); top-corner toasts grow downward. `Toast::layout()`
//! handles this based on `corner`.

use crate::event::Rect;
use crate::types::{Color, Modifiers, WidgetId};
use serde::{Deserialize, Serialize};

/// Declarative description of a toast stack for one corner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToastStack {
    pub id: WidgetId,
    /// Which corner of the viewport the stack occupies.
    pub corner: ToastCorner,
    /// Toasts in temporal order — oldest first. Visual order depends on
    /// `corner` (bottom corners stack upward, top corners stack downward).
    pub toasts: Vec<ToastItem>,
}

/// Corner placement for a `ToastStack`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ToastCorner {
    #[default]
    BottomRight,
    BottomLeft,
    TopRight,
    TopLeft,
}

/// One toast notification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToastItem {
    pub id: WidgetId,
    pub title: String,
    /// Body text. Can be empty for minimal "File saved" style toasts.
    #[serde(default)]
    pub body: String,
    /// Visual severity — backends tint the box accordingly.
    #[serde(default)]
    pub severity: ToastSeverity,
    /// Optional action button. `None` = no action shown; just the
    /// dismiss affordance is clickable.
    #[serde(default)]
    pub action: Option<ToastAction>,
    /// Override severity's default tint. Most toasts use `None` and let
    /// the theme decide.
    #[serde(default)]
    pub accent: Option<Color>,
}

/// Severity level of a `ToastItem`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ToastSeverity {
    #[default]
    Info,
    Success,
    Warning,
    Error,
}

/// Action button on a toast.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToastAction {
    pub id: WidgetId,
    pub label: String,
}

/// Events a `ToastStack` emits back to the app.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToastEvent {
    /// User clicked a toast's action button.
    ActionClicked { id: WidgetId },
    /// User clicked a toast's dismiss affordance (the × or a swipe).
    Dismissed { id: WidgetId },
    /// Click landed anywhere on the toast body (not action, not dismiss).
    /// Apps may interpret this as "focus the originating surface."
    Clicked { id: WidgetId },
    /// A key was pressed while a toast notionally had focus. Rare —
    /// toasts don't normally take keyboard focus.
    KeyPressed { key: String, modifiers: Modifiers },
}

// ── D6 Layout API ───────────────────────────────────────────────────────────
//
// First new B.3 primitive on D6. Toasts stack in a corner with uniform
// spacing; per-toast sizes are backend-supplied (a "body"-less toast is
// shorter than one with a multi-line body).

/// Per-toast measurement supplied by the backend.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ToastMeasure {
    /// Full width of the toast box in the backend's unit.
    pub width: f32,
    /// Full height of the toast box.
    pub height: f32,
    /// Width of the dismiss affordance at the trailing edge. `0.0` if
    /// no dismiss UI is drawn.
    pub dismiss_width: f32,
    /// Width of the action button (at the trailing edge, before
    /// dismiss). `0.0` if the toast has no action.
    pub action_width: f32,
}

impl ToastMeasure {
    pub fn new(width: f32, height: f32) -> Self {
        Self {
            width,
            height,
            dismiss_width: 0.0,
            action_width: 0.0,
        }
    }
}

/// Resolved position of one visible toast after layout.
#[derive(Debug, Clone, PartialEq)]
pub struct VisibleToast {
    /// Index into `ToastStack.toasts`.
    pub toast_idx: usize,
    pub id: WidgetId,
    /// Full toast box bounds.
    pub bounds: Rect,
    /// Dismiss affordance (if present).
    pub dismiss_bounds: Option<Rect>,
    /// Action button (if present).
    pub action_bounds: Option<Rect>,
}

/// Classification of a hit-test result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToastHit {
    /// Click landed on a toast's action button.
    Action(WidgetId),
    /// Click landed on a toast's dismiss affordance.
    Dismiss(WidgetId),
    /// Click landed on a toast's body (not action or dismiss).
    Body(WidgetId),
    /// Click landed outside any toast.
    Empty,
}

/// Fully-resolved toast-stack layout.
#[derive(Debug, Clone, PartialEq)]
pub struct ToastStackLayout {
    pub viewport_width: f32,
    pub viewport_height: f32,
    pub visible_toasts: Vec<VisibleToast>,
    pub hit_regions: Vec<(Rect, ToastHit)>,
}

impl ToastStackLayout {
    pub fn hit_test(&self, x: f32, y: f32) -> ToastHit {
        for (rect, hit) in &self.hit_regions {
            if x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height {
                return hit.clone();
            }
        }
        ToastHit::Empty
    }
}

impl ToastStack {
    /// Compute the rendering + hit-test layout for the stack.
    ///
    /// # Arguments
    ///
    /// - `viewport_width`, `viewport_height` — the app's overlay area.
    ///   Toasts are positioned relative to this.
    /// - `margin` — spacing between the stack and the viewport edges.
    /// - `gap` — vertical gap between consecutive toasts.
    /// - `measure_toast(i)` — per-toast width/height/sub-region widths.
    ///
    /// # Stacking direction
    ///
    /// - `BottomRight` / `BottomLeft`: newest toast is nearest the
    ///   corner; older toasts stack upward.
    /// - `TopRight` / `TopLeft`: newest toast is nearest the corner;
    ///   older toasts stack downward.
    ///
    /// Toasts are iterated oldest-first (matching `self.toasts` order);
    /// the layout positions them in reverse of that for bottom corners
    /// so the newest stays pinned.
    pub fn layout<F>(
        &self,
        viewport_width: f32,
        viewport_height: f32,
        margin: f32,
        gap: f32,
        measure_toast: F,
    ) -> ToastStackLayout
    where
        F: Fn(usize) -> ToastMeasure,
    {
        let mut visible_toasts: Vec<VisibleToast> = Vec::new();
        let mut hit_regions: Vec<(Rect, ToastHit)> = Vec::new();

        if self.toasts.is_empty() {
            return ToastStackLayout {
                viewport_width,
                viewport_height,
                visible_toasts,
                hit_regions,
            };
        }

        let is_right = matches!(
            self.corner,
            ToastCorner::BottomRight | ToastCorner::TopRight
        );
        let is_bottom = matches!(
            self.corner,
            ToastCorner::BottomRight | ToastCorner::BottomLeft
        );

        // Iteration order: bottom corners show newest nearest the corner,
        // so we iterate newest-first and stack upward from the bottom.
        // Top corners show newest nearest the corner (top edge) and
        // stack downward.
        let ordered: Vec<(usize, ToastMeasure)> = if is_bottom {
            // Newest (highest index) nearest bottom — iterate in reverse.
            (0..self.toasts.len())
                .rev()
                .map(|i| (i, measure_toast(i)))
                .collect()
        } else {
            (0..self.toasts.len())
                .map(|i| (i, measure_toast(i)))
                .collect()
        };

        // Starting y: bottom edge - margin for bottom corners; margin for top.
        let mut y_cursor = if is_bottom {
            viewport_height - margin
        } else {
            margin
        };

        for (i, m) in ordered {
            if m.width <= 0.0 || m.height <= 0.0 {
                continue;
            }
            let x = if is_right {
                (viewport_width - margin - m.width).max(0.0)
            } else {
                margin
            };
            let y = if is_bottom {
                (y_cursor - m.height).max(0.0)
            } else {
                y_cursor
            };

            // Skip if the toast would render off-screen.
            if (is_bottom && y >= y_cursor) || (!is_bottom && y + m.height > viewport_height) {
                break;
            }

            let bounds = Rect::new(x, y, m.width, m.height);

            // Sub-regions at the trailing edge (right edge of the toast,
            // regardless of corner side).
            let dismiss_bounds = if m.dismiss_width > 0.0 {
                Some(Rect::new(
                    bounds.x + bounds.width - m.dismiss_width,
                    bounds.y,
                    m.dismiss_width,
                    bounds.height,
                ))
            } else {
                None
            };
            let action_bounds = if m.action_width > 0.0 {
                let offset_from_right = m.dismiss_width + m.action_width;
                Some(Rect::new(
                    bounds.x + bounds.width - offset_from_right,
                    bounds.y,
                    m.action_width,
                    bounds.height,
                ))
            } else {
                None
            };

            let toast_id = self.toasts[i].id.clone();
            visible_toasts.push(VisibleToast {
                toast_idx: i,
                id: toast_id.clone(),
                bounds,
                dismiss_bounds,
                action_bounds,
            });

            // Register hit regions in specificity order: dismiss, action, body.
            if let Some(db) = dismiss_bounds {
                hit_regions.push((db, ToastHit::Dismiss(toast_id.clone())));
            }
            if let Some(ab) = action_bounds {
                // Action carries the action's id (not the toast's) so the
                // app can dispatch the intended action directly from the
                // hit result.
                if let Some(act) = &self.toasts[i].action {
                    hit_regions.push((ab, ToastHit::Action(act.id.clone())));
                }
            }
            hit_regions.push((bounds, ToastHit::Body(toast_id)));

            // Advance the cursor for the next toast.
            if is_bottom {
                y_cursor = y - gap;
                if y_cursor <= 0.0 {
                    break;
                }
            } else {
                y_cursor = y + m.height + gap;
                if y_cursor >= viewport_height {
                    break;
                }
            }
        }

        ToastStackLayout {
            viewport_width,
            viewport_height,
            visible_toasts,
            hit_regions,
        }
    }
}
