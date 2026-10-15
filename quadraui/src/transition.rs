//! Time-based progress helper for chrome motion (fades, slides, resizes).
//!
//! `Transition` is deliberately backend-agnostic and has no `Backend`
//! dependency at all: it is a pure function of a start [`Instant`] and a
//! [`Duration`], sampled by whoever is about to paint a frame. A caller
//! typically holds one `Transition` per animating widget, calls
//! [`Transition::progress`] (or the deterministic [`Transition::progress_at`]
//! in tests) once per frame, feeds the resulting `0.0..=1.0` value into
//! [`lerp`]/[`lerp_rect`] to interpolate the widget's geometry or style, and
//! keeps requesting frames (via `Backend::request_frame_in`, see
//! `backend.rs`) until [`Transition::is_finished`] is `true`.
//!
//! This module intentionally does not decide *when* to repaint, nor does it
//! touch [`crate::backend::InvalidationLog`] itself — it only answers "how
//! far along am I." Driving a redraw loop from that answer is the caller's
//! job (an `AppLogic`/`ShellApp` impl, or future chrome-motion plumbing built
//! on top of this seam).

use std::time::{Duration, Instant};

/// A single time-bounded progress ramp from `0.0` to `1.0`.
///
/// Construct with [`Transition::start`] (anchored at [`Instant::now`]) or
/// [`Transition::starting_at`] (anchored at a caller-supplied `Instant`, for
/// deterministic tests that don't want to depend on wall-clock timing).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transition {
    start: Instant,
    duration: Duration,
}

impl Transition {
    /// Start a transition of `duration`, anchored at `Instant::now()`.
    ///
    /// A zero `duration` is valid and immediately finished: every
    /// `progress_at` call at or after `start` returns `1.0`.
    pub fn start(duration: Duration) -> Self {
        Self::starting_at(Instant::now(), duration)
    }

    /// Start a transition of `duration`, anchored at a caller-supplied
    /// `start` instant. Use this in tests to avoid depending on real wall
    /// clock timing — construct a fixed `Instant` (e.g. via `Instant::now()`
    /// captured once at test setup) and advance `now` in `progress_at` calls
    /// by adding a `Duration` to it.
    pub fn starting_at(start: Instant, duration: Duration) -> Self {
        Self { start, duration }
    }

    /// The instant this transition began.
    pub fn start_time(&self) -> Instant {
        self.start
    }

    /// The total duration of this transition.
    pub fn duration(&self) -> Duration {
        self.duration
    }

    /// Progress at a caller-supplied instant, clamped to `0.0..=1.0`.
    ///
    /// `now` earlier than `start` clamps to `0.0` rather than going negative
    /// (a backend that samples `Instant::now()` just before latching `start`
    /// could otherwise observe `now < start` by a few nanoseconds).
    pub fn progress_at(&self, now: Instant) -> f32 {
        if self.duration.is_zero() {
            return 1.0;
        }
        if now <= self.start {
            return 0.0;
        }
        let elapsed = now.duration_since(self.start);
        if elapsed >= self.duration {
            return 1.0;
        }
        elapsed.as_secs_f32() / self.duration.as_secs_f32()
    }

    /// Progress at `Instant::now()`, clamped to `0.0..=1.0`.
    pub fn progress(&self) -> f32 {
        self.progress_at(Instant::now())
    }

    /// Whether the transition has reached `1.0` progress at `now`.
    pub fn is_finished_at(&self, now: Instant) -> bool {
        self.progress_at(now) >= 1.0
    }

    /// Whether the transition has reached `1.0` progress at `Instant::now()`.
    pub fn is_finished(&self) -> bool {
        self.is_finished_at(Instant::now())
    }
}

/// Linearly interpolate between `a` and `b` at `t`.
///
/// `t` is not clamped here — callers that want clamped output should clamp
/// `t` (e.g. via [`Transition::progress_at`], which already clamps) before
/// calling this. Un-clamped `t` outside `0.0..=1.0` extrapolates, which is
/// occasionally useful for overshoot easing.
pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Linearly interpolate each field of two [`crate::event::Rect`]s at `t`.
///
/// See [`lerp`] for `t`'s clamping contract.
pub fn lerp_rect(a: crate::event::Rect, b: crate::event::Rect, t: f32) -> crate::event::Rect {
    crate::event::Rect::new(
        lerp(a.x, b.x, t),
        lerp(a.y, b.y, t),
        lerp(a.width, b.width, t),
        lerp(a.height, b.height, t),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_duration_is_immediately_finished() {
        let t = Transition::starting_at(Instant::now(), Duration::ZERO);
        assert_eq!(t.progress_at(Instant::now()), 1.0);
        assert!(t.is_finished_at(Instant::now()));
    }

    #[test]
    fn progress_clamps_to_0_at_start() {
        let start = Instant::now();
        let t = Transition::starting_at(start, Duration::from_secs(1));
        assert_eq!(t.progress_at(start), 0.0);
        assert!(!t.is_finished_at(start));
    }

    #[test]
    fn progress_is_linear_mid_transition() {
        let start = Instant::now();
        let t = Transition::starting_at(start, Duration::from_secs(4));
        let mid = start + Duration::from_secs(1);
        assert!((t.progress_at(mid) - 0.25).abs() < 1e-6);
    }

    #[test]
    fn progress_clamps_to_1_after_duration() {
        let start = Instant::now();
        let t = Transition::starting_at(start, Duration::from_secs(1));
        let after = start + Duration::from_secs(5);
        assert_eq!(t.progress_at(after), 1.0);
        assert!(t.is_finished_at(after));
    }

    #[test]
    fn progress_clamps_to_0_before_start() {
        let start = Instant::now() + Duration::from_secs(1);
        let t = Transition::starting_at(start, Duration::from_secs(1));
        assert_eq!(t.progress_at(Instant::now()), 0.0);
    }

    #[test]
    fn lerp_interpolates() {
        assert_eq!(lerp(0.0, 10.0, 0.0), 0.0);
        assert_eq!(lerp(0.0, 10.0, 1.0), 10.0);
        assert_eq!(lerp(0.0, 10.0, 0.5), 5.0);
    }

    #[test]
    fn lerp_rect_interpolates_each_field() {
        let a = crate::event::Rect::new(0.0, 0.0, 10.0, 10.0);
        let b = crate::event::Rect::new(10.0, 20.0, 30.0, 40.0);
        let mid = lerp_rect(a, b, 0.5);
        assert_eq!(mid, crate::event::Rect::new(5.0, 10.0, 20.0, 25.0));
    }

    #[test]
    fn accessors_report_start_and_duration() {
        let start = Instant::now();
        let dur = Duration::from_millis(250);
        let t = Transition::starting_at(start, dur);
        assert_eq!(t.start_time(), start);
        assert_eq!(t.duration(), dur);
    }
}
