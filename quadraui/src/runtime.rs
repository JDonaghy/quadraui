//! Shared plumbing for the per-backend runners (`tui::run`, `gtk::run`,
//! `macos::run`, `win::run`) — quadraui#496, finished by #813.
//!
//! Each backend's `run.rs` drives a live event source (crossterm poll
//! loop, GTK signal closures, AppKit responder methods, a Win32
//! `wndproc`) through [`crate::AppLogic`]. Four pieces of that plumbing
//! were duplicated — or, worse, present in some runners and silently
//! missing from others — across the four runners before this module
//! existed:
//!
//! - [`EventOutcome`] — what the loop should do after one event (or the
//!   periodic `tick`) has been handled. Declared four times, byte-for-
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
//!   store, so it only picks up the shared constant/doc). macOS and
//!   Windows dispatched `WindowResized` undebounced until quadraui#780
//!   — that issue's audit found the shared machinery already existed
//!   here but only two of the four backends had adopted it. Both now
//!   adopt [`ResizeDebouncer`] too: macOS's `NSTimer`
//!   cancel-and-reschedule and Windows' `SetTimer`
//!   replace-if-already-armed both give "fire once, `RESIZE_SETTLE`
//!   after the last resize" for free, same as GTK's `glib::SourceId`,
//!   but — unlike GTK — neither can cheaply re-read a "live" size at
//!   fire time from outside the event that carried it, so they still
//!   need somewhere to stash the pending viewport between "resize
//!   arrived" and "burst settled". [`ResizeDebouncer`] is exactly that
//!   store, decoupled from any particular platform timer so its
//!   coalescing behavior stays unit-testable here even though the
//!   `NSTimer`/`SetTimer` plumbing that drives it isn't.
//! - [`preprocess_event`] (quadraui#813, finishing what #496 scoped but
//!   never wrote) — the copy/paste/selection/double-click/accelerator
//!   pipeline every `dispatch_event` ran ahead of `AppLogic::handle`.
//!   #496 shipped only the three pieces above and left four independent
//!   `dispatch_event` ladders in place; #813's audit measured their
//!   pairwise similarity at 0.21–0.77 — each a different *subset* of one
//!   ladder, not a stylistic rewrite of the same one. Concretely, before
//!   this function existed: Windows folded double-clicks inline while
//!   GTK did it from native `GdkEventType` press-count in two different
//!   places; only GTK read the PRIMARY selection on middle-click; TUI
//!   never intercepted Ctrl-V at all (it has bracketed paste instead,
//!   which most terminals use to deliver `ClipboardPaste` directly — see
//!   `TuiBackend`'s `PreprocessBackend` impl); and TUI forced
//!   `EventOutcome::Redraw` after Ctrl-C regardless of the app's own
//!   `Reaction`, where GTK/macOS/Windows folded it through unchanged.
//!   All of that is now one function every backend's `dispatch_event`
//!   routes through — see [`preprocess_event`]'s own doc for the unified
//!   priority order.
//!
//!   Two of the divergences #496's original audit found really were
//!   load-bearing, not accidental, and stay preserved as documented
//!   per-backend overrides on [`PreprocessBackend`] rather than being
//!   normalised away:
//!
//!   - [`PreprocessBackend::is_copy_keypress`] — TUI's Ctrl-C guard
//!     tolerates a stray Shift and matches `'C'` (CapsLock) as well as
//!     `'c'`; real terminals attach modifier noise to Ctrl-C that a
//!     strict guard would silently drop. GTK/macOS/Windows keep the
//!     strict default (exactly Ctrl, lowercase `'c'`).
//!   - [`PreprocessBackend::paste_modifier`] — macOS pastes on
//!     Cmd-V/Cmd-Shift-V, the platform convention; every other backend
//!     uses the default, Ctrl-V/Ctrl-Shift-V.
//!
//!   Everything else the original audit flagged — the outcome-fold after
//!   Ctrl-C, the missing middle-click/Ctrl-V/double-click coverage named
//!   above — was accidental, not a platform requirement, and is fixed by
//!   routing every backend through this one function instead of
//!   preserved as a quirk. That is a deliberate, reviewable behavior
//!   change on TUI (Ctrl-C's outcome-fold) and on TUI/macOS/Windows
//!   (middle-click paste, and Ctrl-V on TUI) — see #813's PR body for
//!   the full list and the driver-tier tests that pin each one down.

use crate::desktop::{is_paste_keypress, PasteModifier};
use crate::focus::FocusManager;
use crate::runner::AppLogic;
use crate::text_selection::ActiveTextSelection;
use crate::{
    AcceleratorId, ActivityBarEvent, Key, Modifiers, MouseButton, NamedKey, Point, UiEvent,
    WidgetId,
};

#[cfg(any(
    feature = "tui",
    feature = "gtk",
    all(feature = "win", target_os = "windows"),
    all(feature = "macos", target_os = "macos")
))]
use std::time::Duration;

#[cfg(any(
    feature = "tui",
    all(feature = "win", target_os = "windows"),
    all(feature = "macos", target_os = "macos")
))]
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

// ── preprocess_event (quadraui#813, finishing #496) ─────────────────────────

/// The capability surface [`preprocess_event`] needs from a concrete
/// backend to run the shared copy/paste/selection/double-click/
/// accelerator pipeline. One `impl` per concrete backend (`TuiBackend`,
/// `GtkBackend`, `MacBackend`, `WinBackend`), each defined next to the
/// type in its own `backend.rs` so the method bodies can delegate to
/// that backend's existing (often private) helpers without new
/// crate-visible surface.
///
/// Every method has a concrete counterpart that already existed on at
/// least one backend before #813 — this trait doesn't invent new
/// behavior, it names the shared shape four different `dispatch_event`s
/// converged on independently. The two methods with default bodies
/// ([`Self::paste_modifier`], [`Self::is_copy_keypress`]) are the two
/// spots #496's original audit found a *platform* reason for one backend
/// to answer differently — see this module's doc for which backend
/// overrides which and why.
#[cfg(any(
    feature = "tui",
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
pub(crate) trait PreprocessBackend {
    /// The currently active text selection, if any.
    fn active_text_selection(&self) -> Option<&ActiveTextSelection>;

    /// Update (or start) the active text selection — called on
    /// `UiEvent::TextSelectionChanged`.
    fn set_active_text_selection(&mut self, region: WidgetId, anchor: Point, focus: Point);

    /// Clear the active selection and end any in-progress drag — called
    /// after Ctrl-C copies it.
    fn clear_text_selection(&mut self);

    /// Clear the *displayed* selection highlight only, without ending an
    /// in-progress drag — called on a fresh `MouseDown`/`DoubleClick` so
    /// the old highlight doesn't linger over whatever the new press
    /// starts.
    fn clear_selection_display(&mut self);

    /// Select the entire content of the most-recently focused
    /// `TextRegion` (the Ctrl-A target). Returns whether one resolved.
    fn select_all_text_region(&mut self) -> bool;

    /// The text to copy for the active selection. Already backend-
    /// extracted: TUI serves its `Buffer`-cached copy
    /// (`TuiBackend::cached_selection_text`), GTK/macOS/Win-GUI compute
    /// it pixel-wise from `TextRegion::lines`
    /// (`{Gtk,Mac,Win}Backend::extract_selection_text`).
    fn selection_text_for_copy(&self) -> String;

    /// `WidgetId` of the `ActivityBar` that declared
    /// `is_keyboard_focused = true` during the most recent
    /// `Backend::draw_activity_bar` call, if any.
    fn focused_activity_bar_id(&self) -> Option<&WidgetId>;

    /// Mutable access to this backend's [`FocusManager`] (issue #830) —
    /// the counterpart to the read-only `Backend::focus_manager`. Only
    /// [`preprocess_event`]'s Tab/Shift+Tab intercept calls this; see
    /// `crate::focus`'s module doc for why that's the single writer.
    fn focus_manager_mut(&mut self) -> &mut FocusManager;

    /// Look up a registered `Global`-scope accelerator for `key`+`modifiers`.
    fn match_keypress(&self, key: &Key, modifiers: Modifiers) -> Option<AcceleratorId>;

    /// Fold a `MouseDown` into `DoubleClick` when it lands within this
    /// backend's double-click detector's time/position window of the
    /// previous click at the same button. Every other event passes
    /// through unchanged.
    ///
    /// TUI's implementation is a documented no-op passthrough — TUI
    /// folds double-clicks earlier, in `TuiBackend::apply_dispatch`
    /// (called once per batch of translated crossterm events, before any
    /// individual event reaches `dispatch_event`/`preprocess_event`), so
    /// calling its detector a second time here would double-consume the
    /// same state. See `TuiBackend`'s `PreprocessBackend` impl.
    fn fold_double_click(&mut self, ev: UiEvent) -> UiEvent;

    /// Which platform modifier this backend's paste chord treats as
    /// native — Ctrl everywhere except macOS (Cmd). See
    /// [`PasteModifier`].
    fn paste_modifier(&self) -> PasteModifier {
        PasteModifier::Ctrl
    }

    /// Does `key`+`modifiers` count as "Ctrl-C, copy the active
    /// selection" for this backend? Default: exactly `ctrl` held with no
    /// `shift`/`alt`/`cmd`, and lowercase `'c'` — what GTK, macOS, and
    /// Windows all required before #813. TUI overrides this (see
    /// `TuiBackend`'s impl) to also tolerate a stray Shift and accept
    /// `'C'` (CapsLock) — real terminals attach modifier noise to Ctrl-C
    /// that this stricter default would silently drop.
    fn is_copy_keypress(&self, key: &Key, modifiers: &Modifiers) -> bool {
        matches!(key, Key::Char('c'))
            && modifiers.ctrl
            && !modifiers.shift
            && !modifiers.alt
            && !modifiers.cmd
    }
}

/// Dispatch one already-translated [`UiEvent`] through the app, applying
/// the shared runner pre-processing pipeline first (quadraui#813,
/// finishing #496). This is the single function every backend's
/// `dispatch_event` now funnels through for the steps below — see this
/// module's doc for the full history and the two documented per-backend
/// exceptions.
///
/// Pre-processing handled here, in priority order (anything not matched
/// falls through to `app.handle` unchanged):
///
/// 1. [`UiEvent::MouseDown`] → [`PreprocessBackend::fold_double_click`]:
///    fold into [`UiEvent::DoubleClick`] if it lands in the backend's
///    double-click window.
/// 2. `KeyPressed` while an `ActivityBar` declared
///    `is_keyboard_focused = true`: redirect to
///    `UiEvent::ActivityBar(id, KeyPressed { .. })` instead of the app's
///    normal `handle` — `ShellAdapter`'s built-in activity-bar keyboard
///    cursor depends on this.
/// 3. Tab / Shift+Tab focus cycling (issue #830), only when
///    [`AppLogic::tab_stops`] returns non-empty for the current frame:
///    sync [`FocusManager`]'s tab order, cycle it
///    ([`FocusManager::focus_next`]/[`FocusManager::focus_prev`] —
///    `NamedKey::BackTab`, or `NamedKey::Tab` with Shift held, retreats;
///    plain `NamedKey::Tab` advances), and deliver
///    `UiEvent::FocusChanged` instead of the raw key press. Ctrl/Alt/Cmd
///    held is left alone (reserved for app-level chords like
///    `TabGroupController`'s Ctrl-Tab pane switching). While an app's
///    `tab_stops` stays empty — the default — this step never fires and
///    Tab/Shift+Tab pass through unchanged, exactly as before #830.
/// 4. `KeyPressed` matching a registered `Global`-scope accelerator:
///    rewrite to `UiEvent::Accelerator`.
/// 5. Ctrl-C ([`PreprocessBackend::is_copy_keypress`]) with an active
///    text selection: copy it to the clipboard, clear it, and deliver
///    `UiEvent::TextCopied` instead of forwarding the raw key press —
///    forwarding it could trigger quit/copy-all handlers, and
///    `ClipboardPaste` would wrongly insert text. The app's own
///    `Reaction` to `TextCopied` is folded through unchanged.
///
///    `TextCopied` is emitted **unconditionally**, right after
///    `write_text` returns `()`. It means "the copy was attempted", not
///    "the system clipboard now holds this text" — on the TUI backend
///    every clipboard leg is best-effort and silent on failure, and a
///    misconfigured tmux swallows the copy entirely (#331). Apps should
///    word their copy confirmation accordingly; see
///    `quadraui/docs/CLIPBOARD.md`.
/// 6. Ctrl-V / Ctrl-Shift-V ([`is_paste_keypress`], keyed off
///    [`PreprocessBackend::paste_modifier`]): read the clipboard and
///    deliver `UiEvent::ClipboardPaste` instead of forwarding the raw key
///    press.
/// 7. Middle-click (`MouseDown` with `MouseButton::Middle`): read the
///    PRIMARY selection and deliver `UiEvent::ClipboardPaste` — the
///    X11/Wayland "paste what was last selected" convention, distinct
///    from the CLIPBOARD selection Ctrl-V reads. Every backend but GTK's
///    `Clipboard` impl answers `None` here (see
///    [`crate::backend::Clipboard::read_primary_selection`]'s default),
///    so this step is a safe no-op everywhere else.
/// 8. Ctrl-A: select the entire content of the most-recently focused
///    `TextRegion`, if one is registered.
/// 9. `MouseDown` or `DoubleClick`: clear the displayed selection
///    highlight — a fresh drag (or a folded double-click landing on the
///    same spot) may be starting/finishing and shouldn't show a stale
///    highlight from the previous interaction. Never cancels an
///    in-progress drag — `clear_selection_display` only clears the
///    rendered overlay (see its doc).
/// 10. `TextSelectionChanged`: update the backend's active selection and
///     force a redraw.
#[cfg(any(
    feature = "tui",
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
pub(crate) fn preprocess_event<B, A>(event: UiEvent, backend: &mut B, app: &mut A) -> EventOutcome
where
    B: PreprocessBackend + crate::Backend,
    A: AppLogic,
{
    // ── 1. Double-click folding ────────────────────────────────────────
    let event = backend.fold_double_click(event);

    // ── 2. ActivityBar keyboard focus intercept ─────────────────────
    if let UiEvent::KeyPressed {
        ref key, modifiers, ..
    } = event
    {
        if let Some(bar_id) = backend.focused_activity_bar_id().cloned() {
            let key_str = crate::primitives::activity_bar::key_to_activity_bar_string(key);
            let bar_ev = UiEvent::ActivityBar(
                bar_id,
                ActivityBarEvent::KeyPressed {
                    key: key_str,
                    modifiers,
                },
            );
            return app.handle(bar_ev, backend).into();
        }
    }

    // ── 3. Tab / Shift+Tab focus cycling (issue #830) ───────────────────
    if let UiEvent::KeyPressed {
        key: Key::Named(named @ (NamedKey::Tab | NamedKey::BackTab)),
        modifiers,
        ..
    } = &event
    {
        if !modifiers.ctrl && !modifiers.alt && !modifiers.cmd {
            let stops = app.tab_stops(A::AreaId::default());
            if !stops.is_empty() {
                backend.focus_manager_mut().sync_tab_order(&stops);
                let retreat = matches!(named, NamedKey::BackTab) || modifiers.shift;
                let fm = backend.focus_manager_mut();
                let changed = if retreat {
                    fm.focus_prev()
                } else {
                    fm.focus_next()
                };
                if changed {
                    let focused = backend.focus_manager().focused().cloned();
                    return app.handle(UiEvent::FocusChanged(focused), backend).into();
                }
                // Claimed but no-op (e.g. a single-widget tab order) —
                // swallow it rather than letting it fall through to
                // `app.handle` as a raw KeyPressed once an app has
                // opted in at all.
                return EventOutcome::Continue;
            }
        }
    }

    // ── 4. Global accelerator dispatch ─────────────────────────────────
    let event = if let UiEvent::KeyPressed { key, modifiers, .. } = &event {
        match backend.match_keypress(key, *modifiers) {
            Some(id) => UiEvent::Accelerator(id, *modifiers),
            None => event,
        }
    } else {
        event
    };

    // ── 5. Ctrl-C interception (copy active text selection) ────────────
    if let UiEvent::KeyPressed {
        ref key,
        ref modifiers,
        ..
    } = event
    {
        if backend.is_copy_keypress(key, modifiers) && backend.active_text_selection().is_some() {
            let text = backend.selection_text_for_copy();
            backend.services().clipboard().write_text(&text);
            backend.clear_text_selection();
            return app.handle(UiEvent::TextCopied(text), backend).into();
        }
    }

    // ── 6. Ctrl-V / Ctrl-Shift-V interception (paste) ───────────────────
    if let UiEvent::KeyPressed {
        ref key,
        ref modifiers,
        ..
    } = event
    {
        if is_paste_keypress(key, modifiers, backend.paste_modifier()) {
            return match backend.services().clipboard().read_text() {
                Some(text) => app.handle(UiEvent::ClipboardPaste(text), backend).into(),
                None => EventOutcome::Continue,
            };
        }
    }

    // ── 7. Middle-click interception (PRIMARY-selection paste) ──────────
    if let UiEvent::MouseDown {
        button: MouseButton::Middle,
        ..
    } = &event
    {
        if let Some(text) = backend.services().clipboard().read_primary_selection() {
            return app.handle(UiEvent::ClipboardPaste(text), backend).into();
        }
    }

    // ── 8. Ctrl-A interception (select-all for text regions) ────────────
    if let UiEvent::KeyPressed {
        key: Key::Char('a') | Key::Char('A'),
        modifiers,
        ..
    } = &event
    {
        if modifiers.ctrl
            && !modifiers.shift
            && !modifiers.alt
            && !modifiers.cmd
            && backend.select_all_text_region()
        {
            return EventOutcome::Redraw;
        }
    }

    // ── 9. MouseDown / DoubleClick: clear the displayed selection ───────
    if matches!(
        event,
        UiEvent::MouseDown { .. } | UiEvent::DoubleClick { .. }
    ) {
        backend.clear_selection_display();
    }

    // ── 10. TextSelectionChanged: update active selection while dragging ─
    let mut force_redraw = false;
    if let UiEvent::TextSelectionChanged {
        ref region,
        anchor,
        focus,
    } = event
    {
        backend.set_active_text_selection(region.clone(), anchor, focus);
        force_redraw = true;
    }

    // ── Normal app dispatch ──────────────────────────────────────────────
    let outcome: EventOutcome = app.handle(event, backend).into();
    if force_redraw {
        match outcome {
            EventOutcome::Continue => EventOutcome::Redraw,
            other => other,
        }
    } else {
        outcome
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
/// All four runners coalesce the burst and dispatch a single
/// `WindowResized` at the final settled size once no new resize has
/// arrived for this interval. Painting stays live throughout in all —
/// each runner's frame/draw callback re-reads the real surface size
/// every frame regardless of whether the debounced event has fired yet.
/// macOS and Windows adopted this alongside TUI/GTK in quadraui#780;
/// before that they dispatched `WindowResized` on every intermediate
/// size, undebounced.
#[cfg(any(
    feature = "tui",
    feature = "gtk",
    all(feature = "win", target_os = "windows"),
    all(feature = "macos", target_os = "macos")
))]
pub(crate) const RESIZE_SETTLE: Duration = Duration::from_millis(120);

/// Trailing-edge resize-event coalescing (quadraui#437, extracted for
/// #496; adopted by macOS/Windows in #780): stores the most recent
/// viewport from a burst of resize events, superseding any earlier one,
/// until the caller decides the burst has settled and takes it.
///
/// Mechanism-agnostic by design — this struct only coalesces the
/// *value*, not the *timing*. Each backend owns how it decides "has this
/// settled": TUI polls an `Instant` deadline once per loop iteration
/// (see `tui::run::run_inner`); macOS cancels and reschedules a one-shot
/// `NSTimer` targeting the view itself per `viewFrameDidChange:` (see
/// `macos::run::QuadraView::view_frame_did_change`); Windows relies on
/// `SetTimer` replacing an already-armed timer of the same id rather
/// than stacking a new one, so a live drag keeps rescheduling the same
/// `WM_TIMER` (see `win::run`'s `WM_SIZE`/`WM_TIMER` arms). GTK cancels
/// and reschedules a `glib::SourceId` timer per resize (see
/// `gtk::run::run_with`) and doesn't need this struct at all, since
/// GLib's timer already gives it exactly-once-after-settle semantics
/// and it re-reads the DA's live size at fire time rather than storing
/// a pending value.
#[cfg(any(
    feature = "tui",
    all(feature = "win", target_os = "windows"),
    all(feature = "macos", target_os = "macos")
))]
pub(crate) struct ResizeDebouncer {
    pending: Option<Viewport>,
}

#[cfg(any(
    feature = "tui",
    all(feature = "win", target_os = "windows"),
    all(feature = "macos", target_os = "macos")
))]
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

/// Thread-safe inbox for [`UiEvent::User`] payloads (issue #831).
///
/// Every backend's own event queue — `GtkBackend::events` /
/// `MacBackend::events` / `WinBackend::events`, all
/// `Rc<RefCell<VecDeque<UiEvent>>>` — is deliberately `!Send`: nothing
/// outside the owning thread can touch it, which is exactly the
/// architectural gap #831 tracks ("zero mpsc/Waker/channel in any
/// runner"). `UserEventQueue` is the `Send + Sync` staging area a
/// background thread *can* touch. Each backend owns one `Arc<Self>`,
/// clones it into the closure [`crate::Backend::waker`] hands out, and
/// drains it back into `UiEvent::User` on the owning thread — the same
/// place it already drains its native event source (TUI's
/// `poll_events`/`wait_events`; GTK/macOS/Windows' live dispatch path).
///
/// TUI is the only backend that can drain this purely by polling more
/// often — see `Backend::waker`'s doc for why GTK/macOS/Windows also need
/// a native "run this on the UI thread" nudge (`glib::MainContext::invoke`,
/// `dispatch2::DispatchQueue::main`, `PostMessageW`) alongside the queue
/// itself.
pub(crate) struct UserEventQueue {
    inbox: std::sync::Mutex<
        std::collections::VecDeque<std::sync::Arc<dyn std::any::Any + Send + Sync>>,
    >,
}

impl UserEventQueue {
    pub(crate) fn new() -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self {
            inbox: std::sync::Mutex::new(std::collections::VecDeque::new()),
        })
    }

    /// Push one payload. Called from the closure `Backend::waker` hands
    /// out — may run on any thread, including the owning one.
    pub(crate) fn push(&self, payload: std::sync::Arc<dyn std::any::Any + Send + Sync>) {
        // A poisoned lock means some other thread panicked while holding
        // it; recovering the inner guard is safe here since a
        // `VecDeque::push_back` can't leave the queue torn.
        let mut inbox = self.inbox.lock().unwrap_or_else(|e| e.into_inner());
        inbox.push_back(payload);
    }

    /// Drain every queued payload into `UiEvent::User`, appended to `out`
    /// in FIFO order. Called on the backend's owning thread only.
    pub(crate) fn drain_into(&self, out: &mut Vec<UiEvent>) {
        let mut inbox = self.inbox.lock().unwrap_or_else(|e| e.into_inner());
        out.extend(
            inbox
                .drain(..)
                .map(|payload| UiEvent::User(crate::event::UserPayload::from_arc(payload))),
        );
    }
}

/// Wraps a `!Send` value — a GTK widget handle, an `Rc<RefCell<_>>` app
/// pair — so it can be captured by the `Send + Sync` closure
/// [`crate::Backend::waker`] hands out (issue #831).
///
/// **GTK-only, deliberately.** The wrapper exists to make a native
/// "run this on the UI thread" primitive do something useful once it
/// wakes: the callback it schedules needs a handle to *something* owned
/// by the UI thread (a widget to redraw, a backend/app pair to dispatch
/// through), and every such handle (`Rc<RefCell<_>>`, a GTK/GObject
/// wrapper) is deliberately `!Send` — the same architectural fact that
/// motivates `UserEventQueue` existing as a separate `Send + Sync`
/// staging area in the first place. Only `glib::MainContext::invoke`
/// has that shape *and* no ready-made wrapper:
///
/// - **macOS** needs the identical shape but gets it from upstream —
///   `MacBackend::waker` uses `dispatch2::MainThreadBound`, which ships
///   with `dispatch2::DispatchQueue::main` and carries an
///   `MainThreadMarker` proof rather than a thread-id assertion. Don't
///   substitute this type there.
/// - **Windows** doesn't need the indirection at all: `PostMessageW`
///   wakes the loop by queueing a real Win32 message, and `wndproc`
///   already holds the app/backend state when that message is
///   dispatched, so there is nothing to carry across the thread boundary
///   (see `WinBackend::waker`'s doc).
///
/// Keep this `#[cfg(feature = "gtk")]`. Widening it to `win` makes the
/// type dead code on a Windows host — `cargo clippy --features win`
/// under `-D warnings` fails on `dead_code`, which is not reproducible
/// from a Linux `cargo check --features win` because the `cfg` arm
/// wouldn't have been active there in the first place.
///
/// The soundness argument is the same one `dispatch2::MainThreadBound`
/// documents for its own identical wrapper: the value only ever
/// originates from, and is only ever read back on, the UI thread — the
/// native primitive's whole contract is "this callback runs on the thread
/// that owns the loop". [`Self::get`] additionally asserts that in debug
/// builds (`debug_assert_eq!`) rather than trusting it silently, so a
/// future call site that violates the invariant panics loudly on the
/// thread that got it wrong instead of racing `Rc`'s refcount from two
/// threads at once.
#[cfg(feature = "gtk")]
pub(crate) struct MainThreadBound<T> {
    value: T,
    owner: std::thread::ThreadId,
}

// SAFETY: `value` is only ever read via `get`, which asserts (debug) that
// the calling thread matches `owner` — the thread `new` was called from.
// Every caller of `get` in this crate does so from inside a callback a
// native "run this on the main/UI thread" primitive scheduled, which by
// that primitive's own contract only ever runs on `owner`. The wrapper
// itself never dereferences `T` on any other thread.
#[cfg(feature = "gtk")]
unsafe impl<T> Send for MainThreadBound<T> {}
// SAFETY: shared access (`&self` in `get`) is read-only and carries the
// same thread-identity assertion as the `Send` impl above.
#[cfg(feature = "gtk")]
unsafe impl<T> Sync for MainThreadBound<T> {}

#[cfg(feature = "gtk")]
impl<T> MainThreadBound<T> {
    /// Wrap `value`, capturing the current thread as its only valid
    /// future accessor. Call this from the UI thread, before the value
    /// crosses into a `Send`-only context.
    pub(crate) fn new(value: T) -> Self {
        Self {
            value,
            owner: std::thread::current().id(),
        }
    }

    /// Recover the wrapped value. Panics (debug builds only) if called
    /// from any thread other than the one that constructed this.
    pub(crate) fn get(&self) -> &T {
        debug_assert_eq!(
            std::thread::current().id(),
            self.owner,
            "MainThreadBound accessed off its owning thread"
        );
        &self.value
    }
}

#[cfg(test)]
mod user_event_queue_tests {
    use super::UserEventQueue;
    use crate::UiEvent;
    use std::sync::Arc;

    #[test]
    fn drain_into_is_empty_when_nothing_pushed() {
        let q = UserEventQueue::new();
        let mut out = Vec::new();
        q.drain_into(&mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn push_then_drain_round_trips_fifo() {
        let q = UserEventQueue::new();
        q.push(Arc::new(1_i32));
        q.push(Arc::new(2_i32));

        let mut out = Vec::new();
        q.drain_into(&mut out);
        assert_eq!(out.len(), 2);
        let values: Vec<i32> = out
            .iter()
            .map(|ev| match ev {
                UiEvent::User(payload) => *payload.downcast_ref::<i32>().unwrap(),
                other => panic!("expected UiEvent::User, got {other:?}"),
            })
            .collect();
        assert_eq!(values, vec![1, 2], "payloads must drain in push order");

        // Draining clears the queue.
        let mut out2 = Vec::new();
        q.drain_into(&mut out2);
        assert!(out2.is_empty());
    }

    /// The whole point of #831: a payload pushed from a background thread
    /// must be observable on another thread via `drain_into` — this is
    /// the plumbing every backend's `waker()` closure relies on.
    #[test]
    fn push_from_background_thread_is_observed_after_join() {
        let q = UserEventQueue::new();
        let q2 = Arc::clone(&q);
        let handle = std::thread::spawn(move || {
            q2.push(Arc::new("from background".to_string()));
        });
        handle.join().unwrap();

        let mut out = Vec::new();
        q.drain_into(&mut out);
        assert_eq!(out.len(), 1);
        match &out[0] {
            UiEvent::User(payload) => {
                assert_eq!(
                    payload.downcast_ref::<String>().map(String::as_str),
                    Some("from background")
                );
            }
            other => panic!("expected UiEvent::User, got {other:?}"),
        }
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

// `ResizeDebouncer` exists under `feature = "tui"`, `"win"` (on a real
// Windows host), or `"macos"` (on a real macOS host) — see its doc
// comment for why GTK doesn't need it.
#[cfg(all(
    test,
    any(
        feature = "tui",
        all(feature = "win", target_os = "windows"),
        all(feature = "macos", target_os = "macos")
    )
))]
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

    /// quadraui#780 acceptance: "a test that drives a burst of resize
    /// events and asserts the dispatched count collapses". Simulates
    /// exactly what every native-timer-driven caller (macOS's
    /// `viewFrameDidChange:`, Windows' `WM_SIZE`) does per event —
    /// `note()`, never `take()` — for a burst of intermediate sizes a
    /// live drag would deliver, then fires the settle timer once, the
    /// way `resizeDebounceFired:`/`WM_TIMER` do. However large the
    /// burst, exactly one `WindowResized` worth of dispatch data (the
    /// final size) survives to be taken — the rest never reach
    /// `AppLogic::handle` at all, which is the collapse this debouncer
    /// exists to guarantee.
    #[test]
    fn burst_of_resize_events_collapses_to_one_dispatch() {
        let mut d = ResizeDebouncer::new();
        let mut dispatched = 0u32;

        // A live edge-drag burst: many intermediate sizes, no settle
        // between them.
        for i in 1..=50u32 {
            d.note(Viewport::new(i as f32, i as f32, 1.0));
            // No timer fired yet during the burst — nothing to take.
        }

        // The drag settles: the native timer fires exactly once and
        // the caller takes whatever is pending.
        if let Some(viewport) = d.take() {
            dispatched += 1;
            assert_eq!(
                viewport,
                Viewport::new(50.0, 50.0, 1.0),
                "the settled dispatch must carry the final size, not an \
                 intermediate one"
            );
        }

        assert_eq!(
            dispatched, 1,
            "a 50-event resize burst must collapse to exactly one \
             dispatched WindowResized"
        );
        assert!(
            d.take().is_none(),
            "take() must not yield a second dispatch for the same burst"
        );
    }
}
