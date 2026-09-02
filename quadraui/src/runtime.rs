//! Shared plumbing for the per-backend runners (`tui::run`, `gtk::run`,
//! `macos::run`) — quadraui#496.
//!
//! Each backend's `run.rs` drives a live event source (crossterm poll
//! loop, GTK signal closures, AppKit responder methods) through
//! [`crate::AppLogic`]. Three small pieces of that plumbing were
//! duplicated verbatim (or near-verbatim) across all three runners
//! before this module existed:
//!
//! - [`EventOutcome`] — what the loop should do after one event (or the
//!   periodic `tick`) has been handled. Declared three times, byte-for-
//!   byte identical modulo doc comments.
//! - "Apply an outcome to the live window" — GTK had two near-duplicate
//!   functions (`apply_reaction` for `Reaction`, `apply_event_outcome`
//!   for `EventOutcome`); macOS had a third, inherent to `QuadraView`.
//!   [`ReactionSink`] + [`apply_outcome`] replace all three with one
//!   definition, generic over anything convertible to [`EventOutcome`].
//! - The 120ms trailing-edge resize-settle debounce (quadraui#437): TUI
//!   and GTK each reinvented it with a twin ~20-line rationale comment.
//!   [`RESIZE_SETTLE`] centralises the constant + the doc; TUI also
//!   adopts [`ResizeDebouncer`] for the pending-viewport coalescing
//!   itself (GTK's mechanism is GLib-timer-native — cancel + reschedule
//!   a `glib::SourceId` — and doesn't need a separate pending-value
//!   store, so it only picks up the shared constant/doc).
//!
//! ## What this module deliberately does *not* unify yet
//!
//! The issue that motivated this module (#496) also scoped in a shared,
//! capability-aware `preprocess_event` covering the Ctrl-C-copy /
//! Ctrl-A-select-all / click-clears-selection / `TextSelectionChanged`
//! pipeline that both `tui::run::dispatch_event` and
//! `gtk::run::dispatch_event` implement. Comparing the two bodies
//! line-by-line (rather than assuming they match because they look
//! similar) turned up real, load-bearing divergences that a shared
//! function would either have to paper over — changing observable
//! behavior on one backend — or thread through as extra parameters:
//!
//! - TUI's Ctrl-C guard accepts Shift as a stray modifier
//!   (`modifiers.ctrl && !modifiers.alt && !modifiers.cmd`, no `shift`
//!   check) and matches both `'c'` and `'C'`. GTK's requires
//!   `Modifiers { ctrl: true, shift: false, alt: false, cmd: false }`
//!   exactly and matches only lowercase `'c'`.
//! - TUI's Ctrl-C handler *forces* `EventOutcome::Redraw` after
//!   `app.handle(TextCopied, …)` regardless of the app's own
//!   `Reaction` (`Reaction::Exit => Exit, _ => Redraw`). GTK instead
//!   folds the app's `Reaction` through unchanged
//!   (`Continue => Continue, Redraw => Redraw, Exit => Exit`) — a
//!   `Reaction::Continue` app stays `EventOutcome::Continue` on GTK but
//!   would become `EventOutcome::Redraw` on TUI if it ran through TUI's
//!   fold.
//! - GTK's pipeline is also interleaved with GTK-only steps that have
//!   no TUI equivalent at all (ActivityBar keyboard-focus redirect,
//!   global-accelerator rewrite, Ctrl-V/Ctrl-Shift-V paste, middle-click
//!   PRIMARY-selection paste) in a specific priority order that a
//!   shared call has to preserve exactly.
//!
//! Silently normalising either divergence would violate this issue's
//! own acceptance bar ("No behavior change on TUI/GTK — driver suites
//! green"), and the 84+4 driver tests back that guarantee: an
//! unnoticed change here fails as a test regression, not a review
//! comment. Unifying the pipeline *bodies* safely needs each divergence
//! either confirmed intentional-and-preserved (parameterised) or
//! confirmed accidental-and-fixed (a behavior change of its own, needing
//! its own review) — that's follow-up work, not a mechanical extraction,
//! and is being left for a separate pass rather than guessed at here.
//!
//! Separately: `macos::run::dispatch_event` has *no* selection pipeline
//! to extract in the first place — `MacBackend`'s `BackendCaps` declares
//! `text_selection: false` (it doesn't track `TextRegion`s or drag-based
//! selection state at all yet), so there is nothing to make
//! capability-aware here beyond "don't call selection-pipeline code
//! against a backend that has none." When `MacBackend` gains
//! `text_selection` support, the trio above is the first thing to lift
//! into a shared, capability-gated helper in this module (e.g. a
//! `SelectionBackend` trait implemented only by backends whose
//! `BackendCaps::text_selection` is `true`), following the same
//! `ReactionSink`-style pattern already established here.
//!
//! macOS *does* adopt [`EventOutcome`] and [`ReactionSink`] in this
//! pass — those two have no such divergence (see
//! `macos::run::QuadraView`'s `ReactionSink` impl).

#[cfg(any(feature = "tui", feature = "gtk"))]
use std::time::Duration;

#[cfg(feature = "tui")]
use crate::event::Viewport;
use crate::runner::Reaction;

/// What the frame/event loop should do after one event — or the
/// periodic `tick` — has been handled by the app.
///
/// One definition shared by every backend runner (quadraui#496); each
/// used to declare this verbatim.
pub(crate) enum EventOutcome {
    /// No redraw needed; keep looping.
    Continue,
    /// State changed; schedule a redraw before the next event drain.
    Redraw,
    /// The app requested exit.
    Exit,
}

impl From<Reaction> for EventOutcome {
    fn from(r: Reaction) -> Self {
        match r {
            Reaction::Continue => EventOutcome::Continue,
            Reaction::Redraw => EventOutcome::Redraw,
            Reaction::Exit => EventOutcome::Exit,
        }
    }
}

/// A live window/view that an [`EventOutcome`] can be applied to:
/// `Redraw` schedules a repaint, `Exit` tears the runner down. Each
/// backend implements this once for whatever handle its runner already
/// holds (GTK: the `DrawingArea` + `ApplicationWindow` pair; macOS: the
/// `QuadraView`) so [`apply_outcome`] is the single place the
/// Continue/Redraw/Exit → no-op/queue_draw/close mapping is written.
///
/// TUI doesn't implement this — its loop applies an outcome by directly
/// returning from `run_inner` (there's no separate "window" handle to
/// signal), so the match stays inline there. See `tui::run::run_inner`.
#[cfg(any(feature = "gtk", all(feature = "macos", target_os = "macos")))]
pub(crate) trait ReactionSink {
    /// Schedule a redraw.
    fn request_redraw(&self);
    /// Tear down / close.
    fn request_exit(&self);
}

/// Apply an outcome — an [`EventOutcome`] or anything that converts into
/// one, e.g. a raw [`Reaction`] — to a [`ReactionSink`]. Replaces what
/// used to be up to three near-identical `match` functions (GTK had two,
/// macOS had one) with a single definition.
#[cfg(any(feature = "gtk", all(feature = "macos", target_os = "macos")))]
pub(crate) fn apply_outcome(outcome: impl Into<EventOutcome>, sink: &impl ReactionSink) {
    match outcome.into() {
        EventOutcome::Continue => {}
        EventOutcome::Redraw => sink.request_redraw(),
        EventOutcome::Exit => sink.request_exit(),
    }
}

/// Trailing-edge debounce settle window for `WindowResized` dispatch
/// (quadraui#437).
///
/// A live terminal/window edge-drag delivers a burst of resize
/// notifications (tens per second). Apps with PTY-backed side effects
/// (`TerminalApp::handle` → `TerminalSession::resize` → SIGWINCH) were
/// resizing the child shell on *every* intermediate size. A shell's
/// line-editor (readline/zle) redraws its prompt for the width it was
/// SIGWINCH'd with; if the grid is reflowed to a *different* width
/// before that redraw is parsed, the cursor-relative bytes land in the
/// wrong columns and scatter duplicated prompt fragments that stick
/// until the next resize — the exact TUI corruption reported in round
/// #209, and the GTK counterpart the DrawingArea resize handler guards
/// against too.
///
/// Both runners coalesce the burst and dispatch a single
/// `WindowResized` at the final settled size once no new resize has
/// arrived for this interval. Painting stays live throughout in both —
/// each runner's frame/draw callback re-reads the real surface size
/// every frame regardless of whether the debounced event has fired yet.
#[cfg(any(feature = "tui", feature = "gtk"))]
pub(crate) const RESIZE_SETTLE: Duration = Duration::from_millis(120);

/// Trailing-edge resize-event coalescing (quadraui#437, extracted for
/// #496): stores the most recent viewport from a burst of resize
/// events, superseding any earlier one, until the caller decides the
/// burst has settled and takes it.
///
/// Mechanism-agnostic by design — this struct only coalesces the
/// *value*, not the *timing*. Each backend owns how it decides "has this
/// settled": TUI polls an `Instant` deadline once per loop iteration
/// (see `tui::run::run_inner`); GTK cancels and reschedules a
/// `glib::SourceId` timer per resize (see `gtk::run::run_with`) and
/// doesn't need this struct at all, since GLib's timer already gives it
/// exactly-once-after-settle semantics and it re-reads the DA's live
/// size at fire time rather than storing a pending value.
#[cfg(feature = "tui")]
pub(crate) struct ResizeDebouncer {
    pending: Option<Viewport>,
}

#[cfg(feature = "tui")]
impl ResizeDebouncer {
    /// No resize pending.
    pub(crate) const fn new() -> Self {
        Self { pending: None }
    }

    /// Record a new resize, superseding any not-yet-settled one.
    pub(crate) fn note(&mut self, viewport: Viewport) {
        self.pending = Some(viewport);
    }

    /// Take the pending viewport, if any, clearing it. Call once the
    /// caller's own timer has determined the burst settled.
    pub(crate) fn take(&mut self) -> Option<Viewport> {
        self.pending.take()
    }
}

// `ReactionSink` / `apply_outcome` only exist under the same gate as their
// definitions above (`gtk`, or `macos` on a real macOS host) — see those
// items' doc comments for why. Kept as a separate `mod` (rather than
// `#[cfg(test)]` alone on each `#[test]` fn) so a `tui`-only `cargo test`
// doesn't try to compile a `RecordingSink` against a trait that isn't
// there.
#[cfg(all(
    test,
    any(feature = "gtk", all(feature = "macos", target_os = "macos"))
))]
mod reaction_sink_tests {
    use super::*;

    struct RecordingSink {
        redraws: std::cell::Cell<u32>,
        exits: std::cell::Cell<u32>,
    }

    impl RecordingSink {
        fn new() -> Self {
            Self {
                redraws: std::cell::Cell::new(0),
                exits: std::cell::Cell::new(0),
            }
        }
    }

    impl ReactionSink for RecordingSink {
        fn request_redraw(&self) {
            self.redraws.set(self.redraws.get() + 1);
        }
        fn request_exit(&self) {
            self.exits.set(self.exits.get() + 1);
        }
    }

    #[test]
    fn apply_outcome_continue_touches_nothing() {
        let sink = RecordingSink::new();
        apply_outcome(EventOutcome::Continue, &sink);
        assert_eq!(sink.redraws.get(), 0);
        assert_eq!(sink.exits.get(), 0);
    }

    #[test]
    fn apply_outcome_redraw_calls_request_redraw() {
        let sink = RecordingSink::new();
        apply_outcome(EventOutcome::Redraw, &sink);
        assert_eq!(sink.redraws.get(), 1);
        assert_eq!(sink.exits.get(), 0);
    }

    #[test]
    fn apply_outcome_exit_calls_request_exit() {
        let sink = RecordingSink::new();
        apply_outcome(EventOutcome::Exit, &sink);
        assert_eq!(sink.redraws.get(), 0);
        assert_eq!(sink.exits.get(), 1);
    }

    #[test]
    fn apply_outcome_accepts_a_raw_reaction() {
        let sink = RecordingSink::new();
        apply_outcome(Reaction::Redraw, &sink);
        assert_eq!(sink.redraws.get(), 1);
    }
}

// `ResizeDebouncer` only exists under `feature = "tui"` — see its doc
// comment for why GTK doesn't need it.
#[cfg(all(test, feature = "tui"))]
mod resize_debouncer_tests {
    use super::*;

    #[test]
    fn resize_debouncer_starts_empty() {
        let mut d = ResizeDebouncer::new();
        assert!(d.take().is_none());
    }

    #[test]
    fn resize_debouncer_take_clears_pending() {
        let mut d = ResizeDebouncer::new();
        d.note(Viewport::new(100.0, 50.0, 1.0));
        assert_eq!(d.take(), Some(Viewport::new(100.0, 50.0, 1.0)));
        assert!(d.take().is_none(), "take() must clear the pending value");
    }

    #[test]
    fn resize_debouncer_later_note_supersedes_earlier() {
        let mut d = ResizeDebouncer::new();
        d.note(Viewport::new(100.0, 50.0, 1.0));
        d.note(Viewport::new(200.0, 80.0, 1.0));
        assert_eq!(d.take(), Some(Viewport::new(200.0, 80.0, 1.0)));
    }
}
