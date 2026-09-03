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
//! button). Clicks resolve via [`ToastStackLayout::hit_test`] /
//! [`ToastHit`]: the action button hits `ToastHit::Action`; the
//! dismiss affordance hits `ToastHit::Dismiss`. Toast boxes don't take
//! keyboard focus — they're strictly a notification surface.
//!
//! Stacking direction: bottom-corner toasts grow upward (newest nearest
//! the corner); top-corner toasts grow downward. `Toast::layout()`
//! handles this based on `corner`.

use crate::event::Rect;
use crate::types::{Color, WidgetId};
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

/// Translate a `Rect` by `(dx, dy)`, keeping its size.
fn shift_rect(r: Rect, dx: f32, dy: f32) -> Rect {
    Rect::new(r.x + dx, r.y + dy, r.width, r.height)
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
    /// - `origin_x`, `origin_y` — the top-left corner of the app's
    ///   overlay area, in the backend's absolute (screen/buffer) frame.
    ///   Returned `bounds` / `hit_regions` are absolute — callers pass
    ///   `hit_test` raw click coordinates in that same frame, matching
    ///   the `MenuBar`/`Panel` convention (unlike `TreeView`'s
    ///   viewport-local frame). Pass `(0.0, 0.0)` for an overlay
    ///   anchored at the buffer/window origin.
    /// - `viewport_width`, `viewport_height` — the app's overlay area
    ///   size. Toasts are positioned relative to this.
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
    #[allow(clippy::too_many_arguments)]
    pub fn layout<F>(
        &self,
        origin_x: f32,
        origin_y: f32,
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

        // Shift from the viewport-local frame computed above into the
        // caller's absolute frame. Matches `MenuBar::layout` /
        // `Panel::layout`'s convention (bounds already carry the origin,
        // so paint loops use them verbatim and hosts `hit_test` with raw
        // click coordinates) rather than `TreeView`'s local-frame
        // convention. Before this, `ToastStack::layout` had no origin
        // parameter at all, so every backend's `*_toast_stack_layout`
        // silently dropped `rect.x` / `rect.y` — invisible at the origin
        // (every prior test) and a real drift for any non-zero-origin
        // overlay (quadraui#494 / LESSONS.md "Layout helpers must return
        // coords in the same frame across backends").
        if origin_x != 0.0 || origin_y != 0.0 {
            for vt in &mut visible_toasts {
                vt.bounds = shift_rect(vt.bounds, origin_x, origin_y);
                vt.dismiss_bounds = vt.dismiss_bounds.map(|r| shift_rect(r, origin_x, origin_y));
                vt.action_bounds = vt.action_bounds.map(|r| shift_rect(r, origin_x, origin_y));
            }
            for (rect, _) in &mut hit_regions {
                *rect = shift_rect(*rect, origin_x, origin_y);
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

#[cfg(test)]
mod tests {
    use super::*;

    // ── Toast primitive tests (D6 shape, new B.3 primitive) ───────────

    fn make_toast(id: &str, title: &str) -> ToastItem {
        ToastItem {
            id: WidgetId::new(id),
            title: title.to_string(),
            body: String::new(),
            severity: ToastSeverity::Info,
            action: None,
            accent: None,
        }
    }

    fn make_toast_stack(corner: ToastCorner, toasts: Vec<ToastItem>) -> ToastStack {
        ToastStack {
            id: WidgetId::new("toasts"),
            corner,
            toasts,
        }
    }

    #[test]
    fn toast_layout_empty() {
        let stack = make_toast_stack(ToastCorner::BottomRight, vec![]);
        let layout = stack.layout(0.0, 0.0, 800.0, 600.0, 16.0, 8.0, |_| {
            ToastMeasure::new(300.0, 64.0)
        });
        assert_eq!(layout.visible_toasts.len(), 0);
        assert_eq!(layout.hit_test(100.0, 100.0), ToastHit::Empty);
    }

    #[test]
    fn toast_layout_bottom_right_newest_at_bottom() {
        let stack = make_toast_stack(
            ToastCorner::BottomRight,
            vec![
                make_toast("first", "First"),
                make_toast("second", "Second"),
                make_toast("third", "Third"),
            ],
        );
        let layout = stack.layout(0.0, 0.0, 800.0, 600.0, 16.0, 8.0, |_| {
            ToastMeasure::new(300.0, 64.0)
        });
        assert_eq!(layout.visible_toasts.len(), 3);
        // Newest (idx=2, "third") pinned at the bottom.
        let newest = &layout.visible_toasts[0];
        assert_eq!(newest.toast_idx, 2);
        assert_eq!(newest.id.as_str(), "third");
        // Newest bottom = viewport_height (600) - margin (16) - toast_height (64) = 520
        assert_eq!(newest.bounds.y, 520.0);
        // Right-aligned: x = 800 - 16 - 300 = 484
        assert_eq!(newest.bounds.x, 484.0);
        // Second-newest above with gap.
        assert_eq!(layout.visible_toasts[1].id.as_str(), "second");
        assert_eq!(layout.visible_toasts[1].bounds.y, 520.0 - 8.0 - 64.0);
    }

    #[test]
    fn toast_layout_top_left_newest_at_top() {
        let stack = make_toast_stack(
            ToastCorner::TopLeft,
            vec![make_toast("a", "A"), make_toast("b", "B")],
        );
        let layout = stack.layout(0.0, 0.0, 800.0, 600.0, 10.0, 5.0, |_| {
            ToastMeasure::new(200.0, 50.0)
        });
        assert_eq!(layout.visible_toasts.len(), 2);
        // Iteration is oldest-first for top corners.
        let first = &layout.visible_toasts[0];
        assert_eq!(first.id.as_str(), "a");
        assert_eq!(first.bounds.x, 10.0);
        assert_eq!(first.bounds.y, 10.0);
        let second = &layout.visible_toasts[1];
        assert_eq!(second.bounds.y, 10.0 + 50.0 + 5.0);
    }

    #[test]
    fn toast_layout_action_and_dismiss_regions() {
        let mut toast = make_toast("t1", "Build failed");
        toast.action = Some(ToastAction {
            id: WidgetId::new("open_log"),
            label: "Open log".to_string(),
        });
        let stack = make_toast_stack(ToastCorner::BottomRight, vec![toast]);
        let layout = stack.layout(0.0, 0.0, 800.0, 600.0, 16.0, 8.0, |_| ToastMeasure {
            width: 300.0,
            height: 64.0,
            dismiss_width: 24.0,
            action_width: 80.0,
        });
        let v = &layout.visible_toasts[0];
        assert!(v.dismiss_bounds.is_some());
        assert!(v.action_bounds.is_some());
        let db = v.dismiss_bounds.unwrap();
        let ab = v.action_bounds.unwrap();
        // Dismiss at trailing edge.
        assert_eq!(db.x + db.width, v.bounds.x + v.bounds.width);
        // Action left of dismiss.
        assert_eq!(ab.x + ab.width, db.x);

        // Hit-test on dismiss.
        match layout.hit_test(db.x + 5.0, db.y + 10.0) {
            ToastHit::Dismiss(id) => assert_eq!(id.as_str(), "t1"),
            _ => panic!("expected Dismiss hit"),
        }
        // Hit-test on action.
        match layout.hit_test(ab.x + 5.0, ab.y + 10.0) {
            ToastHit::Action(id) => assert_eq!(id.as_str(), "open_log"),
            _ => panic!("expected Action hit"),
        }
        // Hit-test on body (left part of toast, not on action/dismiss).
        match layout.hit_test(v.bounds.x + 5.0, v.bounds.y + 10.0) {
            ToastHit::Body(id) => assert_eq!(id.as_str(), "t1"),
            _ => panic!("expected Body hit"),
        }
    }

    #[test]
    fn toast_layout_stack_clips_when_out_of_room() {
        // 5 toasts of 64px each, but viewport only has 200 px from margin
        // to top. Should render as many as fit.
        let stack = make_toast_stack(
            ToastCorner::BottomRight,
            (0..5)
                .map(|i| make_toast(&format!("t{i}"), &format!("T{i}")))
                .collect(),
        );
        let layout = stack.layout(0.0, 0.0, 800.0, 200.0, 10.0, 8.0, |_| {
            ToastMeasure::new(300.0, 64.0)
        });
        // Bottom stack. Newest at y = 200 - 10 - 64 = 126. Each subsequent
        // goes up 64+8=72. Next: 126-72=54. Next: 54-72=-18 (would be off-top).
        // So only 2-3 fit. Specifically we break when y_cursor <= 0.
        assert!(layout.visible_toasts.len() >= 2);
        assert!(layout.visible_toasts.len() <= 3);
    }

    /// Non-zero-origin regression guard (quadraui#494 / LESSONS.md):
    /// `ToastStack::layout` previously had no origin parameter at all,
    /// so it could only ever be called with an implicit `(0, 0)`
    /// origin — a shape no `*_toast_stack_layout` backend wrapper could
    /// correct for. Confirms every returned bound (toast, dismiss,
    /// action, hit region) shifts rigidly by `(origin_x, origin_y)`
    /// relative to the origin-`(0, 0)` layout for the same stack.
    #[test]
    fn toast_layout_nonzero_origin_shifts_every_bound() {
        let mut toast = make_toast("t1", "Build failed");
        toast.action = Some(ToastAction {
            id: WidgetId::new("open_log"),
            label: "Open log".to_string(),
        });
        let stack = make_toast_stack(ToastCorner::BottomRight, vec![toast]);
        let measure = |_: usize| ToastMeasure {
            width: 300.0,
            height: 64.0,
            dismiss_width: 24.0,
            action_width: 80.0,
        };
        let origin = stack.layout(0.0, 0.0, 800.0, 600.0, 16.0, 8.0, measure);
        let shifted = stack.layout(7.0, 13.0, 800.0, 600.0, 16.0, 8.0, measure);

        let o = &origin.visible_toasts[0];
        let s = &shifted.visible_toasts[0];
        assert_eq!(s.bounds.x, o.bounds.x + 7.0);
        assert_eq!(s.bounds.y, o.bounds.y + 13.0);
        assert_eq!(s.bounds.width, o.bounds.width);
        assert_eq!(
            s.dismiss_bounds.unwrap().x,
            o.dismiss_bounds.unwrap().x + 7.0
        );
        assert_eq!(
            s.action_bounds.unwrap().y,
            o.action_bounds.unwrap().y + 13.0
        );

        // Round trip: an absolute hit against the shifted layout must
        // resolve the same way the origin layout resolves its local hit.
        let db = s.dismiss_bounds.unwrap();
        assert_eq!(
            shifted.hit_test(db.x + 5.0, db.y + 10.0),
            ToastHit::Dismiss(WidgetId::new("t1")),
        );
    }
}
