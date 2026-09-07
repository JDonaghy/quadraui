//! Runner-crate API for `AppLogic`-style apps that delegate event +
//! frame loops to a per-backend runner (`quadraui::tui::run`,
//! `quadraui::gtk::run`, future `quadraui::win::run` /
//! `quadraui::macos::run`).
//!
//! See `docs/BACKEND_SETUP_AUDIT.md` (B.5d / #260) for the design
//! notes that drove the trait shape, and `examples/tui_app.rs` for a
//! minimal end-to-end usage.
//!
//! # Trait shape
//!
//! Apps implement [`AppLogic`] with three methods:
//! - [`AppLogic::setup`] — one-time init: register accelerators,
//!   warm caches, etc.
//! - [`AppLogic::render`] — per-frame paint. The runner enters its
//!   `enter_frame_scope` first; `render` calls `backend.draw_*(...)`
//!   to paint primitives.
//! - [`AppLogic::handle`] — per-event dispatch. Returns a
//!   [`Reaction`] telling the runner what to do next (continue,
//!   redraw, exit).
//!
//! # Why direct `&mut dyn Backend`
//!
//! Apps already need to know the `Backend` trait surface to call
//! `draw_*` methods. Wrapping it in a `RenderCtx` would add a parallel
//! API to maintain without hiding anything meaningful. Direct backend
//! access also gives apps `services()` (clipboard, dialogs) and
//! `modal_stack_handle()` for free in the event handler.
//!
//! # AreaId associated type
//!
//! The trait carries an [`AppLogic::AreaId`] associated type. In
//! practice all runners (GTK single-DA, TUI) use `type AreaId = ()`
//! and pass `Default::default()`. The single-DA model was locked in
//! by #217: zone routing is handled by `AppShell::compute_layout` +
//! `FrameHitMap` hit-testing, not by multiple GTK DrawingAreas. The
//! associated type is retained as a compatibility seam.

use std::time::Duration;

use crate::backend::Backend;
use crate::event::{Rect, UiEvent};
use crate::types::WidgetId;

/// Tells the runner what to do after `handle` returns.
///
/// `#[non_exhaustive]` (quadraui#832): this PR adds [`Reaction::RedrawAfter`]
/// — verified non-breaking for both downstream consumers today (neither
/// `coord-tui` nor `vimcode` exhaustively matches a `Reaction` value; see
/// this PR's "Downstream impact" note for the grep), but the *next*
/// variant might not be so lucky. Marking this `#[non_exhaustive]` now
/// costs neither consumer anything — they only ever construct
/// `Reaction::Redraw`/`Continue`/`Exit`/`RedrawAfter` values or compare
/// them (`assert_eq!`/`matches!`), never exhaustively match one — and
/// forecloses this exact "is a new variant breaking" question for future
/// additions, per `CLAUDE.md`'s public-API rule 8.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Reaction {
    /// Continue the loop. Next event poll. The runner will redraw
    /// only if some other event in the same batch returned
    /// [`Reaction::Redraw`], or if the runner's own internal
    /// invalidation triggers (resize, etc.).
    Continue,
    /// Force a redraw on the next loop iteration. Use after engine
    /// state mutations that need visible feedback.
    Redraw,
    /// Don't redraw now, but wake the runner and call [`AppLogic::tick`]
    /// again after `Duration` — sooner if some other event wakes it
    /// first (quadraui#832).
    ///
    /// This is the replacement for the pre-#832 pattern of relying on a
    /// backend's fixed-cadence idle poll to eventually notice
    /// time-driven state (a spinner frame, a caret blink, a countdown)
    /// and re-check it. Return this from [`AppLogic::tick`] (or
    /// [`AppLogic::handle`]) instead of [`Reaction::Continue`] whenever
    /// there's nothing to redraw *now* but something to redraw *later*:
    ///
    /// ```ignore
    /// fn tick(&mut self, _backend: &mut dyn Backend) -> Reaction {
    ///     if self.job.is_running() {
    ///         // Nothing has changed on screen yet — the job is still
    ///         // going — but re-check in ~100ms rather than waiting for
    ///         // whatever idle cadence (if any) the backend has.
    ///         return Reaction::RedrawAfter(Duration::from_millis(100));
    ///     }
    ///     Reaction::Continue
    /// }
    /// ```
    ///
    /// If the same tick *also* has something to paint right now — the
    /// usual animation case, where each wake advances a visible frame —
    /// this variant on its own is the wrong tool: it schedules the wake
    /// but skips the repaint. Paint *and* schedule by calling
    /// [`Backend::request_frame_in`] directly and returning
    /// [`Reaction::Redraw`], as `examples/common/chat_demo.rs`'s
    /// thinking-spinner countdown does:
    ///
    /// ```ignore
    /// fn tick(&mut self, backend: &mut dyn Backend) -> Reaction {
    ///     if self.busy {
    ///         self.spinner_frame = self.spinner_frame.wrapping_add(1);
    ///         backend.request_frame_in(Duration::from_millis(100));
    ///         return Reaction::Redraw; // paints the new frame now
    ///     }
    ///     Reaction::Continue
    /// }
    /// ```
    ///
    /// Returning this every tick while busy — the chained-rearm pattern
    /// — is the intended usage, not redundant. Backends may wake earlier
    /// than requested for unrelated reasons (a native event, a
    /// background [`Backend::waker`] call, another still-pending
    /// request) but never later; they are not required to cancel or
    /// dedupe overlapping requests, so an occasional extra wake before
    /// its `Duration` elapses is expected and harmless — a missed one
    /// is not. The runner implements the wake via
    /// [`Backend::request_frame_in`]; see that method's doc for the
    /// per-backend mechanism (native one-shot timer on GTK/macOS/
    /// Windows, a poll-timeout bound on TUI).
    ///
    /// A plain `Reaction::Redraw` still redraws *this* frame — this
    /// variant is for scheduling a *future* one without forcing the
    /// current frame to repaint.
    RedrawAfter(Duration),
    /// Tear down and exit the runner. The runner returns control to
    /// the caller (typically `main`).
    Exit,
}

impl Reaction {
    /// Combine two `Reaction`s produced within the same synthesized-event
    /// batch (e.g. a driver's `dispatch_all`/`pump_user_events` folding
    /// several `UiEvent`s from one gesture into the single `Reaction` it
    /// returns) into the one the caller should act on.
    ///
    /// `Exit` always wins, `Redraw` beats `RedrawAfter` (paint *now* is at
    /// least as good as a future wake), and two `RedrawAfter`s keep the
    /// *earlier* deadline — mirroring `crate::runtime::FrameScheduler`'s
    /// own coalescing. That last case is the one worth spelling out:
    /// "first non-`Continue` wins" (this method's predecessor at every
    /// call site) silently drops a shorter, more urgent deadline that
    /// arrives after a longer one in the same batch, producing a wake
    /// later than the app explicitly asked for — narrower than this
    /// variant's own doc promise that the backend "never" wakes later.
    #[cfg_attr(not(any(feature = "tui", feature = "gtk")), allow(dead_code))]
    pub(crate) fn merge(self, next: Reaction) -> Reaction {
        match (self, next) {
            (Reaction::Exit, _) | (_, Reaction::Exit) => Reaction::Exit,
            (Reaction::Redraw, _) | (_, Reaction::Redraw) => Reaction::Redraw,
            (Reaction::RedrawAfter(a), Reaction::RedrawAfter(b)) => Reaction::RedrawAfter(a.min(b)),
            (Reaction::RedrawAfter(d), Reaction::Continue)
            | (Reaction::Continue, Reaction::RedrawAfter(d)) => Reaction::RedrawAfter(d),
            (Reaction::Continue, Reaction::Continue) => Reaction::Continue,
        }
    }
}

/// Trait an app implements to plug into [`crate::tui::run`] /
/// [`crate::gtk::run`].
///
/// The runner owns the event loop, frame loop, terminal/widget setup,
/// and tear-down. The app owns its state (`&mut self`), per-frame
/// rendering, and event dispatch.
pub trait AppLogic {
    /// Identifier for distinct render targets. All current runners
    /// use `type AreaId = ()` — zone routing is handled by
    /// `AppShell::compute_layout` + `FrameHitMap`, not multiple DAs
    /// (#217). Retained as a compatibility seam.
    type AreaId: Copy + Eq + std::fmt::Debug + Default;

    /// One-time setup hook. Called by the runner after backend
    /// construction but before the first frame. Use this to register
    /// accelerators, warm caches, set up file watchers, etc.
    ///
    /// Default impl is a no-op so apps that don't need setup don't
    /// have to write boilerplate.
    fn setup(&mut self, _backend: &mut dyn Backend) {}

    /// Per-frame paint. Called inside the runner's
    /// `enter_frame_scope`. The app calls `backend.draw_*(...)` for
    /// each primitive it wants drawn. The app is also responsible
    /// for setting `theme` / `line_height` / `char_width` on the
    /// backend if the app's theme system varies (typically once per
    /// frame at the start of `render`).
    ///
    /// `area` identifies which target is being painted. Single-area
    /// apps ignore the value (always `Default::default()`).
    /// Multi-area apps `match area` to dispatch to the right paint
    /// path.
    fn render(&self, backend: &mut dyn Backend, area: Self::AreaId);

    /// Per-event dispatch. The runner calls this for every
    /// [`UiEvent`] returned from `backend.wait_events`. Returns a
    /// [`Reaction`] telling the runner what to do next.
    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction;

    /// Periodic tick. Apps implement this for timer logic, auto-refresh,
    /// background-task polling, etc., without needing synthetic event
    /// injection.
    ///
    /// **Since quadraui#832, this is no longer called on a fixed
    /// cadence.** Before #832, every backend polled unconditionally
    /// (TUI every 16ms, GTK every 33ms) and called `tick` on every
    /// timeout regardless of whether anything was scheduled — cheap to
    /// rely on, but it burned CPU on a fully idle app. `tick` now runs:
    /// - after every batch of native events (as before), and
    /// - after a backend's bounded idle-poll ceiling elapses (TUI/GTK
    ///   keep a coarse fallback so time-based work with no explicit
    ///   opt-in — e.g. an embedded terminal's PTY-output poll — still
    ///   makes progress; see each backend's `run.rs` for the exact
    ///   value), and
    /// - promptly after a [`Reaction::RedrawAfter`] deadline it
    ///   previously returned, via [`crate::Backend::request_frame_in`].
    ///
    /// An app with time-driven state (spinner frame, caret blink,
    /// countdown) should return [`Reaction::RedrawAfter`] with the exact
    /// interval it needs instead of assuming `tick` will be called again
    /// soon on its own — that assumption no longer holds on GTK/macOS/
    /// Windows once nothing else is scheduled, and even where a fallback
    /// ceiling exists it's deliberately coarser than before.
    ///
    /// Default impl is a no-op so apps that don't need periodic
    /// callbacks don't have to write boilerplate.
    fn tick(&mut self, _backend: &mut dyn Backend) -> Reaction {
        Reaction::Continue
    }

    /// Whether wheel/trackpad scroll events should use "natural scroll"
    /// direction (content follows the gesture, like a touchscreen) instead
    /// of the traditional wheel convention (wheel-down reveals content
    /// further down the document, i.e. `UiEvent::Scroll`'s `delta.y`
    /// becomes negative on wheel-down).
    ///
    /// Defaults to `false` (traditional direction), so apps that don't
    /// override this method keep quadraui's existing behavior unchanged.
    ///
    /// Only [`crate::gtk::run`] consults this today (quadraui#418) — TUI
    /// and Win32 wheel events already arrive in a single fixed direction
    /// with no natural-scroll concept, and macOS's translator
    /// (`crate::macos::events::ns_scroll`) doesn't yet expose the option.
    /// Consumers wiring their own `DrawingArea` outside `crate::gtk::run`
    /// use `crate::gtk::events::wire_da_events_with_scroll_direction`
    /// directly instead of this trait hook. On Linux, libinput already applies
    /// the OS-level natural-scroll preference
    /// (`org.gnome.desktop.peripherals.touchpad natural-scroll`) before
    /// GTK ever sees the event, so most apps should leave this `false`
    /// and let the OS handle it — override it only if the app wants an
    /// in-app preference independent of (or layered on top of) the OS
    /// setting.
    fn natural_scroll(&self) -> bool {
        false
    }

    /// Opt into the shared runner-owned [`crate::focus::FocusManager`]
    /// (issue #830): the current frame's Tab/Shift+Tab order, as
    /// `(WidgetId, Rect)` pairs in cycle order — typically
    /// [`crate::frame::ScreenLayout::tab_stops`]'s output, for an app
    /// that builds its screen that way.
    ///
    /// Defaults to empty, meaning **Tab/Shift+Tab pass through to
    /// [`Self::handle`] completely unclaimed**, exactly as before #830 —
    /// zero behavior change for every app that doesn't override this.
    ///
    /// Once this returns a non-empty list, the runner claims
    /// Tab/Shift+Tab globally: the shared pipeline
    /// ([`crate::runtime::preprocess_event`]) intercepts them, cycles
    /// [`crate::Backend::focus_manager`], and delivers
    /// [`crate::UiEvent::FocusChanged`] to [`Self::handle`] instead of
    /// the raw key press — see that event's doc for the exact contract,
    /// including why a widget that treats literal Tab as input (a code
    /// editor's indent command, say) should stay out of this list, or
    /// this method should return `[]` while that widget holds focus.
    ///
    /// Called on `&self` (no interior mutability required) both when a
    /// Tab/Shift+Tab keystroke arrives and once per frame after
    /// `render`, to resolve the focused widget's rect for
    /// [`crate::Backend::draw_focus_ring`] — cheap for an app that
    /// already has this data on hand from building its `ScreenLayout`.
    fn tab_stops(&self, _area: Self::AreaId) -> Vec<(WidgetId, Rect)> {
        Vec::new()
    }
}

/// [`Reaction::merge`] — the `Reaction`-side twin of the fix covered by
/// `runtime::event_outcome_merge_tests`; see that module's doc for the
/// review finding this closes (quadraui#832: batch-dispatch loops used to
/// keep the *first* `RedrawAfter` seen rather than the *earliest*
/// deadline).
#[cfg(test)]
mod reaction_merge_tests {
    use super::*;

    #[test]
    fn exit_beats_everything_either_order() {
        assert_eq!(Reaction::Continue.merge(Reaction::Exit), Reaction::Exit);
        assert_eq!(Reaction::Exit.merge(Reaction::Redraw), Reaction::Exit);
        assert_eq!(
            Reaction::RedrawAfter(Duration::from_millis(5)).merge(Reaction::Exit),
            Reaction::Exit
        );
    }

    #[test]
    fn redraw_beats_redraw_after_either_order() {
        assert_eq!(
            Reaction::RedrawAfter(Duration::from_millis(5)).merge(Reaction::Redraw),
            Reaction::Redraw
        );
        assert_eq!(
            Reaction::Redraw.merge(Reaction::RedrawAfter(Duration::from_millis(5))),
            Reaction::Redraw
        );
    }

    #[test]
    fn continue_is_the_identity() {
        assert_eq!(
            Reaction::Continue.merge(Reaction::Continue),
            Reaction::Continue
        );
        assert_eq!(
            Reaction::Continue.merge(Reaction::RedrawAfter(Duration::from_millis(5))),
            Reaction::RedrawAfter(Duration::from_millis(5))
        );
    }

    /// The actual regression: a shorter deadline arriving *after* a longer
    /// one in the same batch must still win — "first wins" would have kept
    /// whichever arrived first, dropping the shorter one about half the
    /// time.
    #[test]
    fn two_redraw_afters_keep_the_earlier_deadline_regardless_of_arrival_order() {
        let long = Reaction::RedrawAfter(Duration::from_millis(100));
        let short = Reaction::RedrawAfter(Duration::from_millis(5));

        assert_eq!(long.merge(short), short);
        assert_eq!(short.merge(long), short);
    }
}
