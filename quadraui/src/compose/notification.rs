//! `notify_or_toast` — send a system [`Notification`] where the backend
//! has one, degrade to the in-canvas [`ToastItem`] primitive where it
//! doesn't (issue #955).
//!
//! [`BackendCaps::notifications`] is `true` only on backends with a real
//! native facility behind [`PlatformServices::send_notification`] (GTK
//! today; see `gtk::services`'s module doc). Every other backend —
//! notably TUI, which has no OS notification-area concept at all — would
//! otherwise silently drop the notification (GTK's own pre-#955
//! `send_notification` was exactly that empty no-op, the gap this issue
//! closes). This helper is the single place that branches on the
//! capability, so app code calls one function instead of repeating the
//! `if backend.backend_caps().notifications { .. } else { .. }` check at
//! every call site.

use crate::backend::{Backend, Notification};
use crate::primitives::toast::{ToastAction, ToastItem, ToastSeverity};
use crate::types::WidgetId;

/// Dispatch `n` as a real system notification when `backend` supports
/// one; otherwise return a [`ToastItem`] the caller should push onto its
/// own [`crate::primitives::toast::ToastStack`] instead.
///
/// Returns `None` when `n` was sent natively — there is nothing left for
/// the caller to do. Returns `Some(item)` on the degrade path; the
/// caller decides the toast's corner, stacking, and dismiss timing (this
/// helper builds the descriptor, not the stack it lives in — same split
/// [`crate::primitives::toast`]'s own module doc describes between the
/// primitive and the app-owned lifecycle).
///
/// ## What survives the degrade
///
/// `title`, `body`, and `urgent` (mapped to
/// [`ToastSeverity::Warning`]/[`ToastSeverity::Info`]) carry over
/// directly. [`Notification::tag`], when set, becomes the toast's
/// [`WidgetId`] — falling back to the title when there is no tag, so two
/// untagged notifications with different titles still get distinct
/// toast ids. Every one of [`Notification::actions`] survives into
/// [`ToastItem::actions`] (#1185 — before the toast primitive supported
/// more than one action button, only the first of `Notification`'s
/// `Vec` made it across; now the shapes match 1:1, none marked
/// [`ToastAction::primary`] since `Notification` carries no such
/// distinction). `icon` and `silent` have no [`ToastItem`] equivalent
/// and are dropped — a toast is always silent (no OS notification sound
/// to suppress) and paints with the app's own chrome, not a
/// caller-supplied icon.
pub fn notify_or_toast(backend: &dyn Backend, n: Notification) -> Option<ToastItem> {
    if backend.backend_caps().notifications {
        backend.services().send_notification(n);
        return None;
    }
    let id = match n.tag() {
        Some(tag) => WidgetId::new(tag),
        None => WidgetId::new(n.title.as_str()),
    };
    let severity = if n.urgent {
        ToastSeverity::Warning
    } else {
        ToastSeverity::Info
    };
    let actions = n
        .actions()
        .iter()
        .map(|(action_id, label)| ToastAction {
            id: action_id.clone(),
            label: label.clone(),
            primary: false,
        })
        .collect();
    Some(ToastItem {
        id,
        title: n.title,
        body: n.body,
        severity,
        actions,
        accent: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::toast::ToastHit;
    use crate::testing::RecordingBackend;

    #[test]
    fn no_native_notifications_degrades_to_some() {
        // `RecordingBackend::backend_caps()` is always `BackendCaps::empty()`
        // (notifications: false), so this only ever exercises the degrade
        // path — the native-send path (`backend.services().send_notification`)
        // is covered by `gtk::services`'s own tests against a real GTK
        // backend, which is the only backend that reports
        // `notifications: true` today.
        let backend = RecordingBackend::new();
        assert!(!backend.backend_caps().notifications);
        let result = notify_or_toast(&backend, Notification::new("t", "b"));
        assert!(result.is_some());
    }

    #[test]
    fn degrades_to_toast_with_title_body_and_severity() {
        let backend = RecordingBackend::new();
        let item = notify_or_toast(
            &backend,
            Notification::new("Build failed", "3 errors")
                .with_action(WidgetId::new("open-problems"), "Show"),
        )
        .expect("no native notifications on RecordingBackend");
        assert_eq!(item.title, "Build failed");
        assert_eq!(item.body, "3 errors");
        assert_eq!(item.severity, ToastSeverity::Info);
        assert_eq!(
            item.actions.first().map(|a| a.id.clone()),
            Some(WidgetId::new("open-problems"))
        );
    }

    #[test]
    fn urgent_maps_to_warning_severity() {
        let backend = RecordingBackend::new();
        // `urgent` is a genuinely `pub` field (unlike `icon`/`actions`/
        // `silent`/`tag`), so direct assignment works from any module —
        // unlike a `Notification { urgent: true, ..base }` literal, which
        // needs visibility into every field `base` would otherwise fill,
        // including the private ones.
        let mut n = Notification::new("t", "b");
        n.urgent = true;
        let item = notify_or_toast(&backend, n).unwrap();
        assert_eq!(item.severity, ToastSeverity::Warning);
    }

    #[test]
    fn tag_becomes_toast_id_falling_back_to_title() {
        let backend = RecordingBackend::new();
        let tagged = notify_or_toast(
            &backend,
            Notification::new("t", "b").with_tag("build-status"),
        )
        .unwrap();
        assert_eq!(tagged.id, WidgetId::new("build-status"));

        let untagged = notify_or_toast(&backend, Notification::new("untagged title", "b")).unwrap();
        assert_eq!(untagged.id, WidgetId::new("untagged title"));
    }

    /// #1185: every `Notification::actions` entry survives into
    /// `ToastItem::actions`, not just the first — the toast primitive
    /// dropped its one-action limit alongside this issue's multi-action
    /// support.
    #[test]
    fn every_action_survives_degrade() {
        let backend = RecordingBackend::new();
        let n = Notification::new("t", "b")
            .with_action(WidgetId::new("first"), "First")
            .with_action(WidgetId::new("second"), "Second");
        let item = notify_or_toast(&backend, n).unwrap();
        let ids: Vec<WidgetId> = item.actions.iter().map(|a| a.id.clone()).collect();
        assert_eq!(ids, vec![WidgetId::new("first"), WidgetId::new("second")]);
    }

    /// The degraded [`ToastItem`] round-trips through the real layout +
    /// hit-test machinery — not just field equality — so this helper's
    /// output is provably clickable, not merely structurally plausible.
    #[test]
    fn degraded_toast_action_is_hit_testable() {
        use crate::primitives::toast::{ToastCorner, ToastMeasure, ToastStack};

        let backend = RecordingBackend::new();
        let item = notify_or_toast(
            &backend,
            Notification::new("t", "b").with_action(WidgetId::new("retry"), "Retry"),
        )
        .unwrap();
        let stack = ToastStack {
            id: WidgetId::new("toasts"),
            corner: ToastCorner::BottomRight,
            toasts: vec![item],
            focus: None,
        };
        let layout = stack.layout(0.0, 0.0, 400.0, 300.0, 8.0, 8.0, |_| {
            let mut m = ToastMeasure::new(220.0, 48.0);
            m.action_rects = vec![crate::event::Rect::new(140.0, 28.0, 60.0, 20.0)];
            m.dismiss_rect = Some(crate::event::Rect::new(200.0, 0.0, 20.0, 20.0));
            m
        });
        let action_bounds = *layout.visible_toasts[0]
            .action_bounds
            .first()
            .expect("toast with an action must lay out an action rect");
        let cx = action_bounds.x + action_bounds.width / 2.0;
        let cy = action_bounds.y + action_bounds.height / 2.0;
        assert_eq!(
            layout.hit_test(cx, cy),
            ToastHit::Action(WidgetId::new("retry"))
        );
    }
}
