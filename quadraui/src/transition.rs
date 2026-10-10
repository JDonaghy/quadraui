//! `Transition` — a small, generic value interpolator for chrome motion,
//! part of the GUI-look epic.
//!
//! Owner decision: **no general animation framework.** This is
//! deliberately not a timeline, a keyframe system, or a spring simulator
//! — it is one `f32` value moving from `from` to `to` over `duration`,
//! sampled on demand. That is enough for every chrome transition in
//! scope today: a hover/press fade on a primitive that already tracks
//! [`crate::InteractionState`] ([`crate::interaction`]'s
//! `hover_fade_alpha`/`press_fade_alpha`), and a toast's slide-in/
//! slide-out ([`crate::primitives::toast::ToastMotion`]).
//!
//! # Why sampled, not ticked
//!
//! A [`Transition`] does not advance itself and has no "step forward by
//! one frame" method. It records *when* it started and *what* it is
//! moving between, and [`Self::value_at`] answers "what is the value at
//! this instant" — pure, cheap, and callable any number of times for
//! the same instant without side effects. The thing that changes frame
//! to frame is the caller's clock, not this type's internal state.
//!
//! This shape is what makes a [`Transition`] trivially testable without
//! a real clock or a sleep: construct one with [`Self::start_at`],
//! sample [`Self::value_at`] at a handful of synthetic `Instant`s, and
//! assert the curve. It is also exactly what [`Self::is_done_at`] needs
//! to answer "should the caller keep asking to be woken" — see that
//! method's doc for the [`crate::runner::Reaction::RedrawAfter`]
//! chained-rearm pattern this is meant to drive.
//!
//! # TUI: snap, don't animate
//!
//! The TUI painter has no frame budget to spend on cosmetic motion and
//! must render the same state a screen-reader or scripted test would
//! see — so TUI code paths use [`Self::snap`] (an already-finished
//! transition sitting on `to`) instead of a real [`Self::start`]/
//! [`Self::start_at`] call. Every [`Self::value_at`]/[`Self::is_done_at`]
//! call against a snapped transition behaves exactly as if `duration`
//! had already fully elapsed, immediately, regardless of `now` — see
//! `tui_snaps_to_the_end_state_immediately` below.
use std::time::{Duration, Instant};

/// One of a couple of easing curves a [`Transition`] can apply to its
/// `0.0..=1.0` progress before interpolating `from`/`to`. Deliberately a
/// short, closed list rather than an extensible curve-authoring system.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum Easing {
    /// Constant speed. `apply(t) == t`.
    #[default]
    Linear,
    /// Starts fast, slows into the destination — the usual choice for a
    /// hover-in fade or a toast sliding into view (VS Code's own chrome
    /// motion uses this shape).
    EaseOut,
    /// Slow start, fast middle, slow finish — symmetric, good for a
    /// press flash or anything that should feel equally weighted on
    /// both ends.
    EaseInOut,
}

impl Easing {
    /// Apply the curve to linear progress `t`, clamped to `0.0..=1.0`
    /// first so a caller never has to pre-clamp.
    ///
    /// - [`Easing::Linear`][]: `t`.
    /// - [`Easing::EaseOut`][]: `1.0 - (1.0 - t).powi(3)` (cubic ease-out).
    /// - [`Easing::EaseInOut`][]: the standard smoothstep, `3t² - 2t³`.
    ///
    /// All three satisfy `apply(0.0) == 0.0`, `apply(1.0) == 1.0`, and
    /// stay within `0.0..=1.0` for `t` in that same range — covered by
    /// `every_curve_is_anchored_at_its_endpoints` and
    /// `every_curve_stays_within_unit_range`.
    pub fn apply(self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Easing::Linear => t,
            Easing::EaseOut => 1.0 - (1.0 - t).powi(3),
            Easing::EaseInOut => t * t * (3.0 - 2.0 * t),
        }
    }
}

/// Interpolates a single `f32` from `from` to `to` over `duration`,
/// sampled by [`Self::value_at`] — see the module doc for the full
/// design rationale.
///
/// `Clone`/`Copy`/`PartialEq` so a caller can cheaply stash one per
/// [`crate::WidgetId`] (as [`crate::InteractionState`] does) without
/// wrapping it in anything.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transition {
    from: f32,
    to: f32,
    start: Instant,
    duration: Duration,
    easing: Easing,
}

impl Transition {
    /// Start a transition from `from` to `to` right now
    /// (`Instant::now()`). See [`Self::start_at`] for the deterministic,
    /// test-friendly twin.
    pub fn start(from: f32, to: f32, duration: Duration, easing: Easing) -> Self {
        Self::start_at(Instant::now(), from, to, duration, easing)
    }

    /// Start a transition from `from` to `to`, anchored at `now` rather
    /// than the real clock — what every test in this module uses, and
    /// what a caller with its own notion of "now" (a replay harness, a
    /// deterministic driver) should use too.
    pub fn start_at(now: Instant, from: f32, to: f32, duration: Duration, easing: Easing) -> Self {
        Self {
            from,
            to,
            start: now,
            duration,
            easing,
        }
    }

    /// An already-finished transition sitting on `value` — what TUI
    /// code paths use instead of animating (see the module doc's "TUI:
    /// snap, don't animate" section). [`Self::value_at`] returns `value`
    /// for every `now`, and [`Self::is_done_at`] is always `true`.
    pub fn snap(value: f32) -> Self {
        Self {
            from: value,
            to: value,
            start: Instant::now(),
            duration: Duration::ZERO,
            easing: Easing::Linear,
        }
    }

    /// Redirect an in-flight transition toward a new `to`, keeping
    /// whatever progress has already been made as the new `from` — the
    /// move a hover-out needs when the pointer leaves a widget mid-fade:
    /// retarget from the *current* interpolated value back to `0.0`
    /// rather than snapping or restarting from `1.0`. `now` is also the
    /// new start instant, so the retargeted leg gets its own full
    /// `duration` to complete.
    ///
    /// Covered by `retarget_continues_from_the_current_value_not_from_to`.
    pub fn retarget_at(&mut self, now: Instant, to: f32) {
        let current = self.value_at(now);
        self.from = current;
        self.to = to;
        self.start = now;
    }

    /// Progress `0.0..=1.0` at `now`, before easing — `0.0` at or before
    /// `start`, `1.0` at or after `start + duration`.
    fn linear_progress_at(&self, now: Instant) -> f32 {
        if self.duration.is_zero() {
            return 1.0;
        }
        match now.checked_duration_since(self.start) {
            Some(elapsed) if elapsed >= self.duration => 1.0,
            Some(elapsed) => elapsed.as_secs_f32() / self.duration.as_secs_f32(),
            // `now` is before `start` (a caller sampling with a stale
            // clock, or a transition constructed with a future `start`)
            // — treat as "hasn't started yet" rather than panicking or
            // going negative.
            None => 0.0,
        }
    }

    /// The interpolated value at `now`.
    pub fn value_at(&self, now: Instant) -> f32 {
        let t = self.easing.apply(self.linear_progress_at(now));
        self.from + (self.to - self.from) * t
    }

    /// [`Self::value_at`] against the real clock (`Instant::now()`).
    pub fn value(&self) -> f32 {
        self.value_at(Instant::now())
    }

    /// Whether the transition has fully settled on `to` at `now`.
    pub fn is_done_at(&self, now: Instant) -> bool {
        self.duration.is_zero() || now.checked_duration_since(self.start) >= Some(self.duration)
    }

    /// [`Self::is_done_at`] against the real clock.
    pub fn is_done(&self) -> bool {
        self.is_done_at(Instant::now())
    }

    /// The transition's destination value, regardless of progress — the
    /// value a caller should paint with once it stops driving frames
    /// (after [`Self::is_done_at`] returns `true`), and what
    /// `InteractionState`'s binary `is_hovered`/`is_pressed` checks
    /// already compare against for widgets with no in-flight fade.
    pub fn target(&self) -> f32 {
        self.to
    }
}

/// The shared fade/slide duration for chrome transitions (hover/press
/// fade, toast slide) — VS Code's own chrome motion sits in the
/// 100-150ms range; 120ms split the difference. Not `pub(crate)`-only:
/// an app composing its own chrome transition with [`Transition`]
/// directly should use the same cadence other primitives do, rather
/// than inventing a different feel per widget.
pub const CHROME_TRANSITION_DURATION: Duration = Duration::from_millis(120);

/// Target frame interval while a chrome transition is in flight — 60fps.
/// Paired with [`crate::runner::Reaction::RedrawAfter`]'s chained-rearm
/// pattern: a caller still animating a [`Transition`] re-arms with this
/// interval instead of guessing one. See
/// `crate::runner::chrome_transition_reaction` for the helper that wires
/// the two together.
pub const CHROME_FRAME_INTERVAL: Duration = Duration::from_millis(16);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linear_interpolates_evenly() {
        let t0 = Instant::now();
        let tr = Transition::start_at(t0, 0.0, 10.0, Duration::from_millis(100), Easing::Linear);
        assert_eq!(tr.value_at(t0), 0.0);
        assert_eq!(tr.value_at(t0 + Duration::from_millis(25)), 2.5);
        assert_eq!(tr.value_at(t0 + Duration::from_millis(50)), 5.0);
        assert_eq!(tr.value_at(t0 + Duration::from_millis(100)), 10.0);
        // Past the end, stays clamped at `to`.
        assert_eq!(tr.value_at(t0 + Duration::from_millis(500)), 10.0);
    }

    #[test]
    fn before_start_stays_at_from() {
        let t0 = Instant::now();
        let later_start = t0 + Duration::from_millis(50);
        let tr = Transition::start_at(
            later_start,
            1.0,
            0.0,
            Duration::from_millis(100),
            Easing::Linear,
        );
        assert_eq!(tr.value_at(t0), 1.0);
    }

    #[test]
    fn every_curve_is_anchored_at_its_endpoints() {
        let t0 = Instant::now();
        for easing in [Easing::Linear, Easing::EaseOut, Easing::EaseInOut] {
            let tr = Transition::start_at(t0, 0.0, 1.0, Duration::from_millis(100), easing);
            assert_eq!(tr.value_at(t0), 0.0, "{easing:?} must start at from");
            assert_eq!(
                tr.value_at(t0 + Duration::from_millis(100)),
                1.0,
                "{easing:?} must finish at to"
            );
        }
    }

    #[test]
    fn every_curve_stays_within_unit_range() {
        let t0 = Instant::now();
        for easing in [Easing::Linear, Easing::EaseOut, Easing::EaseInOut] {
            let tr = Transition::start_at(t0, 0.0, 1.0, Duration::from_millis(100), easing);
            for ms in 0..=100 {
                let v = tr.value_at(t0 + Duration::from_millis(ms));
                assert!(
                    (0.0..=1.0).contains(&v),
                    "{easing:?} produced out-of-range {v} at {ms}ms"
                );
            }
        }
    }

    #[test]
    fn ease_out_is_ahead_of_linear_at_the_midpoint() {
        let t0 = Instant::now();
        let linear = Transition::start_at(t0, 0.0, 1.0, Duration::from_millis(100), Easing::Linear);
        let ease_out =
            Transition::start_at(t0, 0.0, 1.0, Duration::from_millis(100), Easing::EaseOut);
        let mid = t0 + Duration::from_millis(50);
        assert!(ease_out.value_at(mid) > linear.value_at(mid));
    }

    #[test]
    fn is_done_follows_the_duration() {
        let t0 = Instant::now();
        let tr = Transition::start_at(t0, 0.0, 1.0, Duration::from_millis(100), Easing::Linear);
        assert!(!tr.is_done_at(t0));
        assert!(!tr.is_done_at(t0 + Duration::from_millis(99)));
        assert!(tr.is_done_at(t0 + Duration::from_millis(100)));
        assert!(tr.is_done_at(t0 + Duration::from_millis(200)));
    }

    #[test]
    fn retarget_continues_from_the_current_value_not_from_to() {
        let t0 = Instant::now();
        let mut tr = Transition::start_at(t0, 0.0, 1.0, Duration::from_millis(100), Easing::Linear);
        let mid = t0 + Duration::from_millis(50);
        let value_before_retarget = tr.value_at(mid);
        assert!((value_before_retarget - 0.5).abs() < 1e-6);

        // Retargeting to 0.0 must continue from ~0.5, not snap to 1.0
        // first.
        tr.retarget_at(mid, 0.0);
        assert!((tr.value_at(mid) - value_before_retarget).abs() < 1e-6);
        assert!(!tr.is_done_at(mid));
        assert_eq!(tr.value_at(mid + Duration::from_millis(100)), 0.0);
    }

    #[test]
    fn tui_snaps_to_the_end_state_immediately() {
        let snapped = Transition::snap(1.0);
        // Any `now`, including the instant the transition was built,
        // already reports the settled value — no frame needs to elapse.
        assert_eq!(snapped.value_at(Instant::now()), 1.0);
        assert!(snapped.is_done_at(Instant::now()));

        // A zero-duration `start_at` behaves identically — the general
        // case `snap` is sugar for.
        let t0 = Instant::now();
        let zero_duration = Transition::start_at(t0, 0.0, 1.0, Duration::ZERO, Easing::EaseOut);
        assert_eq!(zero_duration.value_at(t0), 1.0);
        assert!(zero_duration.is_done_at(t0));
    }

    #[test]
    fn target_returns_to_regardless_of_progress() {
        let t0 = Instant::now();
        let tr = Transition::start_at(t0, 0.0, 1.0, Duration::from_millis(100), Easing::Linear);
        assert_eq!(tr.target(), 1.0);
    }
}
