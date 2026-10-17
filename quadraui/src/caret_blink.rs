//! [`CaretBlinkState`] — backend-neutral software caret-blink state machine.
//!
//! A blinking text-input caret is "toggle a visibility flag on a timer,
//! except hold it solid while the user is actively typing" — logic that
//! has nothing to do with any particular platform's timer API. This
//! module is that logic alone: a pure `visible`/`paused_until` state
//! machine driven by caller-supplied [`Instant`]s, with no `Rc<Cell<_>>`,
//! no native timer, and no backend import.
//!
//! `macos::caret_blink` (cfg-gated to `target_os = "macos"`, so not
//! linked from here) is the first consumer of this shape — an `NSTimer`
//! ticking against an Obj-C target — and predates this module; it is not
//! required to adopt it. A GTK or Win backend that wants the same
//! blinking convention for
//! `MultiSectionView`'s `SectionAux::Input`/`Search` caret owns the
//! timer mechanism itself:
//!
//! - GTK: a `glib::timeout_add_local` (or [`glib::source`]) firing every
//!   [`CaretBlinkState::period`], calling [`CaretBlinkState::tick`] and
//!   queuing a redraw when it returns `true`.
//! - Windows: a dedicated, always-repeating `SetTimer` (armed once at
//!   startup, never killed — the same posture as `win::run`'s idle-poll
//!   timer) firing every [`CaretBlinkState::period`], calling
//!   [`CaretBlinkState::tick`] on each `WM_TIMER` and invalidating the
//!   window only when it returns `true`. This bypasses the shared
//!   `AppLogic::tick` wake path entirely, so it can never double-tick
//!   against that path's own frame/idle timers.
//!
//! Either way, a `KeyPressed`/`CharTyped` event calls
//! [`CaretBlinkState::on_activity`] to hold the caret solid while typing,
//! matching the AppKit/GTK/Win convention that a blinking caret only
//! blinks while idle.

use std::time::{Duration, Instant};

/// Default "on" + "off" blink period. Matches both AppKit's
/// `NSTextInsertionPointBlinkPeriodOn`/`Off` combined default (~530ms,
/// see `macos::caret_blink`'s `BLINK_PERIOD_SECS`) and the Windows
/// caret-blink rate (`SPI_GETCARETBLINKTIME`'s own ~530ms default) — a
/// reasonable
/// cross-platform default, not a hard requirement; a backend reading its
/// own platform's user setting should pass that value to
/// [`CaretBlinkState::with_config`] instead.
pub const DEFAULT_BLINK_PERIOD: Duration = Duration::from_millis(530);

/// Default duration a [`CaretBlinkState`] holds the caret solid-visible
/// after [`CaretBlinkState::on_activity`], before blinking resumes.
pub const DEFAULT_TYPING_PAUSE: Duration = Duration::from_millis(500);

/// Backend-neutral caret-blink state machine.
///
/// Holds a `visible` flag plus a `paused_until` deadline: calling
/// [`Self::tick`] with the current time toggles `visible` unless `now`
/// is still inside the typing-pause window, in which case the tick is a
/// no-op and the caret stays solid. [`Self::on_activity`] starts (or
/// extends) that window and forces `visible` back to `true`, so the
/// caret never disappears mid-keystroke.
///
/// All time is caller-supplied — nothing here calls [`Instant::now`]
/// except [`Default::default`] — so a test can drive the whole cycle
/// with synthetic instants and no real sleeping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaretBlinkState {
    visible: bool,
    paused_until: Instant,
    period: Duration,
    pause: Duration,
}

impl CaretBlinkState {
    /// A fresh state at `now`: caret visible, no typing pause active, and
    /// [`DEFAULT_BLINK_PERIOD`]/[`DEFAULT_TYPING_PAUSE`] for the two
    /// durations.
    pub fn new(now: Instant) -> Self {
        Self::with_config(now, DEFAULT_BLINK_PERIOD, DEFAULT_TYPING_PAUSE)
    }

    /// Like [`Self::new`], but with a caller-chosen blink period (e.g. a
    /// GTK backend honouring `gtk-cursor-blink-time`) and
    /// [`DEFAULT_TYPING_PAUSE`] for the typing pause.
    pub fn with_period(now: Instant, period: Duration) -> Self {
        Self::with_config(now, period, DEFAULT_TYPING_PAUSE)
    }

    /// Like [`Self::new`], with both durations chosen by the caller.
    pub fn with_config(now: Instant, period: Duration, pause: Duration) -> Self {
        Self {
            visible: true,
            paused_until: now,
            period,
            pause,
        }
    }

    /// Whether the caret should currently paint. The "on" half of the
    /// blink cycle, or any time inside a typing-pause window.
    pub fn visible(&self) -> bool {
        self.visible
    }

    /// The configured blink period (see [`Self::with_period`]).
    pub fn period(&self) -> Duration {
        self.period
    }

    /// Record typing activity at `now`: forces [`Self::visible`] to
    /// `true` and holds it there until `now + pause` (the duration
    /// passed to [`Self::with_config`], [`DEFAULT_TYPING_PAUSE`] by
    /// default) — matching the "caret stays solid while typing"
    /// convention every blink-aware backend here follows.
    pub fn on_activity(&mut self, now: Instant) {
        self.visible = true;
        self.paused_until = now + self.pause;
    }

    /// Advance the blink cycle to `now`. A no-op (returns `false`,
    /// `visible` unchanged) while `now` is still inside the typing-pause
    /// window armed by [`Self::on_activity`]; otherwise toggles
    /// [`Self::visible`] and returns `true`.
    ///
    /// The return value tells the caller whether a redraw is actually
    /// needed — ticking during a typing pause should not force one.
    pub fn tick(&mut self, now: Instant) -> bool {
        if now < self.paused_until {
            return false;
        }
        self.visible = !self.visible;
        true
    }

    /// How long from `now` until the next call to [`Self::tick`] would
    /// do something — either the remainder of an active typing pause, or
    /// a full [`Self::period`] otherwise. A backend driving this state
    /// through [`crate::backend::Backend::request_frame_in`] (rather
    /// than a native repeating timer) passes this as the wake delay.
    pub fn next_wake_delay(&self, now: Instant) -> Duration {
        if now < self.paused_until {
            self.paused_until - now
        } else {
            self.period
        }
    }
}

impl Default for CaretBlinkState {
    /// A fresh state starting now, with the default period and pause.
    /// Prefer [`Self::new`] in tests, which takes the "now" explicitly.
    fn default() -> Self {
        Self::new(Instant::now())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn instants(n: usize) -> Vec<Instant> {
        // `Instant::now()` has no stable "zero" to construct from, so
        // tests anchor on one real sample and derive the rest by offset
        // — still fully deterministic relative to each other.
        let base = Instant::now();
        (0..n)
            .map(|i| base + Duration::from_secs(i as u64))
            .collect()
    }

    #[test]
    fn starts_visible_with_no_pause_active() {
        let t = instants(1);
        let state = CaretBlinkState::new(t[0]);
        assert!(state.visible());
    }

    #[test]
    fn tick_toggles_when_not_paused() {
        let t = instants(1);
        let mut state = CaretBlinkState::new(t[0]);
        let period = state.period();

        assert!(state.tick(t[0] + period));
        assert!(!state.visible());

        assert!(state.tick(t[0] + period * 2));
        assert!(state.visible());
    }

    #[test]
    fn tick_is_a_noop_inside_the_typing_pause() {
        let t = instants(1);
        let mut state = CaretBlinkState::with_config(
            t[0],
            Duration::from_millis(100),
            Duration::from_millis(500),
        );
        state.on_activity(t[0]);

        // Still inside the 500ms pause window.
        assert!(!state.tick(t[0] + Duration::from_millis(100)));
        assert!(state.visible(), "caret stays solid while typing");
    }

    #[test]
    fn on_activity_forces_visible_and_restarts_the_pause() {
        let t = instants(1);
        let mut state = CaretBlinkState::new(t[0]);
        let period = state.period();

        // Blink off once first.
        assert!(state.tick(t[0] + period));
        assert!(!state.visible());

        state.on_activity(t[0] + period);
        assert!(state.visible());

        // A tick right after activity, before the pause elapses, is
        // still a no-op.
        assert!(!state.tick(t[0] + period + Duration::from_millis(1)));
        assert!(state.visible());
    }

    #[test]
    fn tick_resumes_once_the_pause_elapses() {
        let t = instants(1);
        let mut state = CaretBlinkState::new(t[0]);
        let pause = Duration::from_millis(500);
        state.on_activity(t[0]);

        assert!(state.tick(t[0] + pause));
        assert!(!state.visible());
    }

    #[test]
    fn next_wake_delay_matches_the_pause_then_the_period() {
        let t = instants(1);
        let mut state = CaretBlinkState::with_config(
            t[0],
            Duration::from_millis(530),
            Duration::from_millis(500),
        );
        state.on_activity(t[0]);

        assert_eq!(
            state.next_wake_delay(t[0] + Duration::from_millis(200)),
            Duration::from_millis(300)
        );

        // Once unpaused, the delay is a full period.
        let after_pause = t[0] + Duration::from_millis(500);
        assert_eq!(
            state.next_wake_delay(after_pause),
            Duration::from_millis(530)
        );
    }

    #[test]
    fn default_starts_visible() {
        assert!(CaretBlinkState::default().visible());
    }
}
