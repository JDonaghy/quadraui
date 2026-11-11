//! [`HoverTooltipTracker`] — the delay + auto-dismiss state machine a
//! hover-driven tooltip needs on top of the plain "what's under the
//! cursor right now" answer [`crate::compose::ToolbarHoverTracker`]/
//! [`crate::compose::ListHoverTracker`]/[`crate::compose::TreeHoverTracker`]
//! already give.
//!
//! Hovering a widget shouldn't show its tooltip the instant the pointer
//! crosses into its bounds — VS Code (and every other desktop toolkit)
//! waits [`crate::Style::tooltip_delay_ms`] first, so a cursor merely
//! passing through on its way elsewhere never flashes a tooltip. This
//! tracker is that wait, plus the three ways a shown tooltip goes away
//! again: the pointer leaving, an explicit dismiss (a click, an
//! `Escape`), or a timeout while the pointer never moved.
//!
//! # Headless-clock contract
//!
//! Every method here takes time as a plain `u64` millisecond value the
//! *caller* supplies, never a wall-clock read internally — the same
//! shape [`crate::transition::Transition`] uses for chrome motion, just
//! in whole milliseconds rather than an [`std::time::Instant`] so it
//! also crosses a binding's FFI boundary (a Python/JS integer) without
//! needing a platform clock type on the other side. A live backend
//! feeds it `backend`'s own monotonic millisecond counter; a test feeds
//! it hand-picked values with no sleeping at all — see this module's
//! own tests for the pattern.
//!
//! # Usage
//!
//! ```
//! use quadraui::compose::{HoverTooltipTracker, ToolbarHoverTracker};
//! use quadraui::WidgetId;
//!
//! let mut hover = ToolbarHoverTracker::new();
//! let mut tooltip: HoverTooltipTracker<WidgetId> = HoverTooltipTracker::new();
//! let delay_ms = 500;
//!
//! // Frame 1: pointer lands on a button. `hover.update(...)` would feed
//! // the real toolbar layout; simulated directly here.
//! tooltip.on_hover(Some(WidgetId::new("tb:save")), 0);
//! assert_eq!(tooltip.visible_item(), None); // not yet -- still pending
//!
//! // Frame N, 500ms later: the delay has elapsed.
//! let became_visible = tooltip.tick(delay_ms, 500);
//! assert!(became_visible);
//! assert_eq!(tooltip.visible_item(), Some(&WidgetId::new("tb:save")));
//!
//! // The pointer leaves.
//! let hid = tooltip.on_hover(None, 600);
//! assert!(hid);
//! assert_eq!(tooltip.visible_item(), None);
//! ```

use std::time::Duration;

/// Internal phase [`HoverTooltipTracker`] walks through. Not `pub` —
/// [`HoverTooltipTracker::visible_item`] is the only way a caller reads
/// it, so the three-state shape can change without that being a public
/// API break.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Phase<T> {
    /// Nothing hovered, nothing pending, nothing shown.
    Idle,
    /// `item` has been hovered continuously since `since_ms`, but the
    /// configured delay hasn't elapsed yet — nothing paints.
    Pending { item: T, since_ms: u64 },
    /// `item`'s tooltip is showing, and has been since `since_ms` (used
    /// by [`HoverTooltipTracker::tick_auto_dismiss`]'s timeout).
    Visible { item: T, since_ms: u64 },
    /// `item`'s tooltip auto-dismissed while the pointer was still
    /// hovering it. A latch, not a reset to [`Phase::Idle`]: as long as
    /// the pointer keeps hovering the same `item`, [`Self::on_hover`]
    /// stays in this phase instead of re-arming [`Phase::Pending`], so
    /// the tooltip stays hidden rather than showing again
    /// `tooltip_delay_ms` later and auto-dismissing again in an endless
    /// cycle. Hovering a *different* item, or leaving every hoverable
    /// zone, clears the latch the same way either would reset any other
    /// phase.
    Dismissed { item: T },
}

/// Delay + auto-dismiss state machine for a hover-driven tooltip, generic
/// over whatever item type identifies "what's hovered" — a
/// [`crate::WidgetId`] for a toolbar button, a row index for a list, or
/// the tooltip text itself for a single-widget caller that has no
/// separate id to look up. See the module doc for the full contract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HoverTooltipTracker<T: Clone + PartialEq> {
    phase: Phase<T>,
}

impl<T: Clone + PartialEq> Default for HoverTooltipTracker<T> {
    fn default() -> Self {
        Self { phase: Phase::Idle }
    }
}

impl<T: Clone + PartialEq> HoverTooltipTracker<T> {
    /// Construct an idle tracker. No tooltip is pending or visible.
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed the item currently under the pointer (`None` once it's left
    /// every hoverable zone), and the current time. Call this every
    /// time the underlying hover answer changes (e.g. whenever a
    /// `ToolbarHoverTracker::update` call returns `true`), not every
    /// frame — [`Self::tick`] is the per-frame half that notices the
    /// delay has elapsed while hovering the same item.
    ///
    /// Returns `true` if a tooltip that was visible just hid (hovering
    /// a different item restarts its delay from zero rather than
    /// showing instantly, the same way a fresh hover does) — the caller
    /// should redraw. Returns `false` while nothing was visible to
    /// begin with, including the "just started hovering, still
    /// pending" case.
    pub fn on_hover(&mut self, item: Option<T>, now_ms: u64) -> bool {
        match (&self.phase, item) {
            (Phase::Idle, None) => false,
            (Phase::Pending { .. }, None) => {
                self.phase = Phase::Idle;
                false
            }
            (Phase::Visible { .. }, None) => {
                self.phase = Phase::Idle;
                true
            }
            (Phase::Dismissed { .. }, None) => {
                self.phase = Phase::Idle;
                false
            }
            (Phase::Idle, Some(item)) => {
                self.phase = Phase::Pending {
                    item,
                    since_ms: now_ms,
                };
                false
            }
            (Phase::Pending { item: cur, .. }, Some(item)) => {
                if *cur != item {
                    self.phase = Phase::Pending {
                        item,
                        since_ms: now_ms,
                    };
                }
                false
            }
            (Phase::Visible { item: cur, .. }, Some(item)) => {
                if *cur == item {
                    false
                } else {
                    self.phase = Phase::Pending {
                        item,
                        since_ms: now_ms,
                    };
                    true
                }
            }
            (Phase::Dismissed { item: cur }, Some(item)) => {
                if *cur == item {
                    // Still hovering the item whose tooltip auto-dismissed --
                    // stay latched shut rather than re-arming the delay.
                    false
                } else {
                    self.phase = Phase::Pending {
                        item,
                        since_ms: now_ms,
                    };
                    false
                }
            }
        }
    }

    /// Advance the pending timer without a new hover signal — call once
    /// per frame (or just before painting) so a [`Phase::Pending`] that
    /// has waited out `delay_ms` flips to [`Phase::Visible`]. `delay_ms`
    /// is normally [`crate::Style::tooltip_delay_ms`].
    ///
    /// Returns `true` if a tooltip just became visible (the caller
    /// should redraw); `false` if nothing changed, including the
    /// already-visible and idle cases.
    pub fn tick(&mut self, delay_ms: u64, now_ms: u64) -> bool {
        if let Phase::Pending { item, since_ms } = &self.phase {
            if now_ms.saturating_sub(*since_ms) >= delay_ms {
                self.phase = Phase::Visible {
                    item: item.clone(),
                    since_ms: now_ms,
                };
                return true;
            }
        }
        false
    }

    /// Hide a [`Phase::Visible`] tooltip once it has been showing for
    /// `timeout_ms`, even though the pointer never moved off it — the
    /// third dismissal path the module doc names alongside pointer-leave
    /// and an explicit [`Self::dismiss`]. Returns `true` if a tooltip
    /// was just auto-dismissed.
    ///
    /// Transitions to [`Phase::Dismissed`], not [`Phase::Idle`]: the
    /// pointer hasn't moved, so a plain reset would let the very next
    /// [`Self::on_hover`] call for the same item re-arm [`Phase::Pending`]
    /// and show the tooltip again `delay_ms` later, auto-dismissing again
    /// after that — a permanent flicker rather than a dismiss. The latch
    /// only clears once the pointer hovers a different item or leaves.
    pub fn tick_auto_dismiss(&mut self, timeout_ms: u64, now_ms: u64) -> bool {
        if let Phase::Visible { item, since_ms } = &self.phase {
            if now_ms.saturating_sub(*since_ms) >= timeout_ms {
                self.phase = Phase::Dismissed { item: item.clone() };
                return true;
            }
        }
        false
    }

    /// Convenience wrapper around [`Self::tick_auto_dismiss`] for a
    /// caller already holding a [`Duration`] (e.g. from
    /// [`crate::Style`]'s sibling pixel tokens) rather than a raw
    /// millisecond count.
    pub fn tick_auto_dismiss_for(&mut self, timeout: Duration, now_ms: u64) -> bool {
        self.tick_auto_dismiss(timeout.as_millis() as u64, now_ms)
    }

    /// Explicit dismiss — a click anywhere, or `Escape` — hides a
    /// pending or visible tooltip immediately regardless of its timer.
    /// Returns `true` if there was anything to hide.
    pub fn dismiss(&mut self) -> bool {
        match self.phase {
            Phase::Idle => false,
            _ => {
                self.phase = Phase::Idle;
                true
            }
        }
    }

    /// The currently *visible* tooltip's item, or `None` while idle or
    /// still pending (waiting out the delay) — the one query a render
    /// call needs to decide whether to paint anything at all.
    pub fn visible_item(&self) -> Option<&T> {
        match &self.phase {
            Phase::Visible { item, .. } => Some(item),
            Phase::Idle | Phase::Pending { .. } | Phase::Dismissed { .. } => None,
        }
    }

    /// Whether a tooltip is currently waiting out its delay — distinct
    /// from [`Self::visible_item`]`.is_some()`, which is `false` during
    /// this phase too; callers that need to distinguish "nothing
    /// hovered" from "hovered, not shown yet" (e.g. to know whether to
    /// keep calling [`Self::tick`]) read this instead.
    pub fn is_pending(&self) -> bool {
        matches!(self.phase, Phase::Pending { .. })
    }

    /// Milliseconds until a [`Phase::Pending`] tooltip is due to flip to
    /// [`Phase::Visible`] — `0` once the deadline has already passed but
    /// [`Self::tick`] hasn't been called yet to notice, `None` while idle
    /// or already showing. This is the deadline a host needs to turn
    /// "something is pending" into an exact `Reaction::RedrawAfter(Duration)`
    /// wake (`runner.rs`'s own "time-driven state should return the exact
    /// interval" rule) instead of guessing a poll cadence or waiting for
    /// some unrelated event to call [`Self::tick`] again.
    ///
    /// `now_ms`/`delay_ms` are the same caller-supplied clock value and
    /// [`crate::Style::tooltip_delay_ms`] [`Self::tick`] itself takes — see
    /// this module's "headless-clock contract".
    pub fn next_wake_ms(&self, now_ms: u64, delay_ms: u64) -> Option<u64> {
        match &self.phase {
            Phase::Pending { since_ms, .. } => {
                let deadline = since_ms.saturating_add(delay_ms);
                Some(deadline.saturating_sub(now_ms))
            }
            Phase::Idle | Phase::Visible { .. } | Phase::Dismissed { .. } => None,
        }
    }

    /// Milliseconds until a [`Phase::Visible`] tooltip's
    /// [`Self::tick_auto_dismiss`] timeout fires — the `Visible`-phase
    /// sibling of [`Self::next_wake_ms`], for a host that wants the exact
    /// `Reaction::RedrawAfter(Duration)` wake a shown tooltip needs to
    /// hide itself on schedule, instead of waiting for some unrelated
    /// event to call [`Self::tick_auto_dismiss`] again. `None` while idle
    /// or still pending — callers that treat `timeout_ms == 0` as
    /// "auto-dismiss disabled" (e.g. [`crate::Style::tooltip_auto_dismiss_ms`]'s
    /// own default) should not call this at all rather than scheduling a
    /// wake for a timeout that will never fire.
    pub fn next_auto_dismiss_wake_ms(&self, now_ms: u64, timeout_ms: u64) -> Option<u64> {
        match &self.phase {
            Phase::Visible { since_ms, .. } => {
                let deadline = since_ms.saturating_add(timeout_ms);
                Some(deadline.saturating_sub(now_ms))
            }
            Phase::Idle | Phase::Pending { .. } | Phase::Dismissed { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::WidgetId;

    #[test]
    fn hover_alone_does_not_show_the_tooltip() {
        let mut t: HoverTooltipTracker<WidgetId> = HoverTooltipTracker::new();
        let changed = t.on_hover(Some(WidgetId::new("a")), 0);
        assert!(!changed);
        assert_eq!(t.visible_item(), None);
        assert!(t.is_pending());
    }

    #[test]
    fn tick_before_delay_elapses_stays_pending() {
        let mut t: HoverTooltipTracker<WidgetId> = HoverTooltipTracker::new();
        t.on_hover(Some(WidgetId::new("a")), 0);
        let changed = t.tick(500, 400);
        assert!(!changed);
        assert_eq!(t.visible_item(), None);
    }

    #[test]
    fn tick_at_or_after_delay_shows_the_tooltip() {
        let mut t: HoverTooltipTracker<WidgetId> = HoverTooltipTracker::new();
        t.on_hover(Some(WidgetId::new("a")), 0);
        let changed = t.tick(500, 500);
        assert!(changed);
        assert_eq!(t.visible_item(), Some(&WidgetId::new("a")));
        assert!(!t.is_pending());
    }

    #[test]
    fn hovering_the_same_item_while_visible_keeps_it_shown() {
        let mut t: HoverTooltipTracker<WidgetId> = HoverTooltipTracker::new();
        t.on_hover(Some(WidgetId::new("a")), 0);
        t.tick(500, 500);
        let changed = t.on_hover(Some(WidgetId::new("a")), 600);
        assert!(!changed);
        assert_eq!(t.visible_item(), Some(&WidgetId::new("a")));
    }

    #[test]
    fn pointer_leave_hides_a_visible_tooltip() {
        let mut t: HoverTooltipTracker<WidgetId> = HoverTooltipTracker::new();
        t.on_hover(Some(WidgetId::new("a")), 0);
        t.tick(500, 500);
        let changed = t.on_hover(None, 600);
        assert!(changed);
        assert_eq!(t.visible_item(), None);
    }

    #[test]
    fn pointer_leave_while_still_pending_cancels_with_no_redraw() {
        let mut t: HoverTooltipTracker<WidgetId> = HoverTooltipTracker::new();
        t.on_hover(Some(WidgetId::new("a")), 0);
        // Leaves before the 500ms delay elapses -- nothing was ever
        // shown, so there is nothing to redraw away.
        let changed = t.on_hover(None, 100);
        assert!(!changed);
        assert_eq!(t.visible_item(), None);
        assert!(!t.is_pending());
    }

    #[test]
    fn moving_to_a_different_item_while_visible_restarts_the_delay() {
        let mut t: HoverTooltipTracker<WidgetId> = HoverTooltipTracker::new();
        t.on_hover(Some(WidgetId::new("a")), 0);
        t.tick(500, 500);
        assert_eq!(t.visible_item(), Some(&WidgetId::new("a")));

        let changed = t.on_hover(Some(WidgetId::new("b")), 600);
        assert!(changed, "the old tooltip must hide immediately");
        assert_eq!(t.visible_item(), None);
        assert!(t.is_pending());

        // The new item doesn't show until its own delay elapses too.
        assert!(!t.tick(500, 900));
        assert!(t.tick(500, 1100));
        assert_eq!(t.visible_item(), Some(&WidgetId::new("b")));
    }

    #[test]
    fn explicit_dismiss_hides_a_visible_tooltip() {
        let mut t: HoverTooltipTracker<WidgetId> = HoverTooltipTracker::new();
        t.on_hover(Some(WidgetId::new("a")), 0);
        t.tick(500, 500);
        assert!(t.dismiss());
        assert_eq!(t.visible_item(), None);
        // Dismissing again is a no-op.
        assert!(!t.dismiss());
    }

    #[test]
    fn explicit_dismiss_also_cancels_a_pending_tooltip() {
        let mut t: HoverTooltipTracker<WidgetId> = HoverTooltipTracker::new();
        t.on_hover(Some(WidgetId::new("a")), 0);
        assert!(t.dismiss());
        assert!(!t.is_pending());
        // Even if the pointer never moved, the delay no longer matters.
        assert!(!t.tick(500, 10_000));
        assert_eq!(t.visible_item(), None);
    }

    #[test]
    fn auto_dismiss_hides_after_the_timeout_even_without_pointer_movement() {
        let mut t: HoverTooltipTracker<WidgetId> = HoverTooltipTracker::new();
        t.on_hover(Some(WidgetId::new("a")), 0);
        t.tick(500, 500);

        assert!(!t.tick_auto_dismiss(5_000, 3_000));
        assert_eq!(t.visible_item(), Some(&WidgetId::new("a")));

        assert!(t.tick_auto_dismiss(5_000, 5_500));
        assert_eq!(t.visible_item(), None);
    }

    #[test]
    fn auto_dismiss_latches_shut_while_the_same_item_stays_hovered() {
        let mut t: HoverTooltipTracker<WidgetId> = HoverTooltipTracker::new();
        t.on_hover(Some(WidgetId::new("a")), 0);
        t.tick(500, 500);
        assert!(t.tick_auto_dismiss(5_000, 5_500));
        assert_eq!(t.visible_item(), None);

        // The pointer never moved -- re-feeding the same item must not
        // re-arm Pending, or the tooltip would show (and auto-dismiss)
        // again every delay+timeout cycle forever.
        for n in 0..5u64 {
            let now_ms = 5_500 + n * 1_000;
            let changed = t.on_hover(Some(WidgetId::new("a")), now_ms);
            assert!(
                !changed,
                "iteration {n}: re-hovering the same item must not redraw"
            );
            assert_eq!(t.visible_item(), None, "iteration {n}: must stay hidden");
            assert!(
                !t.is_pending(),
                "iteration {n}: must not re-arm the pending delay"
            );
            assert!(
                !t.tick(500, now_ms + 10_000),
                "iteration {n}: tick must not promote a latched dismissal \
                 to Visible even much later"
            );
        }
    }

    #[test]
    fn auto_dismiss_latch_clears_on_a_different_item() {
        let mut t: HoverTooltipTracker<WidgetId> = HoverTooltipTracker::new();
        t.on_hover(Some(WidgetId::new("a")), 0);
        t.tick(500, 500);
        assert!(t.tick_auto_dismiss(5_000, 5_500));

        let changed = t.on_hover(Some(WidgetId::new("b")), 5_600);
        assert!(
            !changed,
            "nothing was visible to hide -- moving off a latched dismissal \
             is not itself a redraw"
        );
        assert!(t.is_pending());
        assert!(t.tick(500, 6_100));
        assert_eq!(t.visible_item(), Some(&WidgetId::new("b")));
    }

    #[test]
    fn auto_dismiss_latch_clears_on_pointer_leave() {
        let mut t: HoverTooltipTracker<WidgetId> = HoverTooltipTracker::new();
        t.on_hover(Some(WidgetId::new("a")), 0);
        t.tick(500, 500);
        assert!(t.tick_auto_dismiss(5_000, 5_500));

        assert!(!t.on_hover(None, 5_600));
        assert!(!t.is_pending());
        assert_eq!(t.visible_item(), None);

        // Hovering the same item again afterwards starts a fresh delay.
        t.on_hover(Some(WidgetId::new("a")), 5_700);
        assert!(t.is_pending());
        assert!(t.tick(500, 6_200));
        assert_eq!(t.visible_item(), Some(&WidgetId::new("a")));
    }

    #[test]
    fn auto_dismiss_for_duration_matches_the_millisecond_form() {
        let mut t: HoverTooltipTracker<WidgetId> = HoverTooltipTracker::new();
        t.on_hover(Some(WidgetId::new("a")), 0);
        t.tick(500, 500);
        assert!(t.tick_auto_dismiss_for(Duration::from_secs(5), 5_600));
        assert_eq!(t.visible_item(), None);
    }

    #[test]
    fn next_wake_ms_is_none_while_idle() {
        let t: HoverTooltipTracker<WidgetId> = HoverTooltipTracker::new();
        assert_eq!(t.next_wake_ms(0, 500), None);
    }

    #[test]
    fn next_wake_ms_counts_down_to_the_delay_while_pending() {
        let mut t: HoverTooltipTracker<WidgetId> = HoverTooltipTracker::new();
        t.on_hover(Some(WidgetId::new("a")), 100);
        assert_eq!(t.next_wake_ms(100, 500), Some(500));
        assert_eq!(t.next_wake_ms(300, 500), Some(300));
        assert_eq!(t.next_wake_ms(600, 500), Some(0));
    }

    #[test]
    fn next_wake_ms_is_none_once_visible() {
        let mut t: HoverTooltipTracker<WidgetId> = HoverTooltipTracker::new();
        t.on_hover(Some(WidgetId::new("a")), 0);
        t.tick(500, 500);
        assert_eq!(t.next_wake_ms(500, 500), None);
    }

    #[test]
    fn next_auto_dismiss_wake_ms_is_none_while_idle_or_pending() {
        let mut t: HoverTooltipTracker<WidgetId> = HoverTooltipTracker::new();
        assert_eq!(t.next_auto_dismiss_wake_ms(0, 5_000), None);
        t.on_hover(Some(WidgetId::new("a")), 0);
        assert_eq!(t.next_auto_dismiss_wake_ms(0, 5_000), None);
    }

    #[test]
    fn next_auto_dismiss_wake_ms_counts_down_to_the_timeout_while_visible() {
        let mut t: HoverTooltipTracker<WidgetId> = HoverTooltipTracker::new();
        t.on_hover(Some(WidgetId::new("a")), 0);
        t.tick(500, 500);
        assert_eq!(t.next_auto_dismiss_wake_ms(500, 5_000), Some(5_000));
        assert_eq!(t.next_auto_dismiss_wake_ms(3_000, 5_000), Some(2_500));
        assert_eq!(t.next_auto_dismiss_wake_ms(5_500, 5_000), Some(0));
    }

    #[test]
    fn a_plain_string_item_works_too() {
        // T is generic -- a single-widget caller with no separate id to
        // look up can track the tooltip text itself.
        let mut t: HoverTooltipTracker<String> = HoverTooltipTracker::new();
        t.on_hover(Some("Save file".to_string()), 0);
        t.tick(500, 500);
        assert_eq!(t.visible_item(), Some(&"Save file".to_string()));
    }
}
