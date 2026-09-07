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

// `EventOutcome::RedrawAfter` (quadraui#832) needs `Duration` unconditionally
// wherever `EventOutcome` itself compiles — matching `mod runtime`'s own gate
// in `lib.rs` (`any(tui, gtk, macos&&target_os=macos, win)`), *not* the
// narrower `all(win, target_os = "windows")` the resize-debounce items below
// use. `win`'s compile-check leg builds this module on Linux too (see
// `lib.rs`'s comment on `mod runtime`), so gating this import to
// `target_os = "windows"` would leave `EventOutcome` unable to resolve
// `Duration` under a plain `--features win` on a non-Windows host.
#[cfg(any(
    feature = "tui",
    feature = "gtk",
    feature = "win",
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
/// used to declare this verbatim. `Copy`/`Clone`/`Eq` (quadraui#832) so
/// [`EventOutcome::merge`]'s tests can reuse a value across multiple
/// assertions instead of constructing it fresh each time, mirroring
/// [`Reaction`]'s identical derive list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EventOutcome {
    /// No redraw needed; keep looping.
    Continue,
    /// State changed; schedule a redraw before the next event drain.
    Redraw,
    /// No redraw needed now; schedule a wake (and another `tick`/dispatch
    /// pass) after `Duration` — see [`Reaction::RedrawAfter`] (quadraui#832).
    RedrawAfter(Duration),
    /// The app requested exit.
    Exit,
}

impl EventOutcome {
    /// Combine two `EventOutcome`s produced within the same
    /// synthesized-event batch (e.g. a drag gesture's `MouseDown` +
    /// synthetic `TextSelectionChanged`, dispatched in one loop) into the
    /// single outcome the caller returns.
    ///
    /// `Exit` always wins, `Redraw` beats `RedrawAfter` (paint *now* is at
    /// least as good as a future wake), and two `RedrawAfter`s keep the
    /// *earlier* deadline — mirroring [`FrameScheduler::request`]'s own
    /// coalescing and [`Reaction::merge`], its `Reaction`-side twin. That
    /// last case is the one worth spelling out: "first non-`Continue`
    /// wins" (this method's predecessor at every call site) silently
    /// drops a shorter, more urgent deadline that arrives after a longer
    /// one in the same batch, producing a wake later than the app
    /// explicitly asked for — narrower than [`Reaction::RedrawAfter`]'s
    /// own doc promise that the backend "never" wakes later.
    #[cfg_attr(
        not(any(feature = "win", all(feature = "macos", target_os = "macos"))),
        allow(dead_code)
    )]
    pub(crate) fn merge(self, next: EventOutcome) -> EventOutcome {
        match (self, next) {
            (EventOutcome::Exit, _) | (_, EventOutcome::Exit) => EventOutcome::Exit,
            (EventOutcome::Redraw, _) | (_, EventOutcome::Redraw) => EventOutcome::Redraw,
            (EventOutcome::RedrawAfter(a), EventOutcome::RedrawAfter(b)) => {
                EventOutcome::RedrawAfter(a.min(b))
            }
            (EventOutcome::RedrawAfter(d), EventOutcome::Continue)
            | (EventOutcome::Continue, EventOutcome::RedrawAfter(d)) => {
                EventOutcome::RedrawAfter(d)
            }
            (EventOutcome::Continue, EventOutcome::Continue) => EventOutcome::Continue,
        }
    }
}

impl From<Reaction> for EventOutcome {
    fn from(r: Reaction) -> Self {
        match r {
            Reaction::Continue => EventOutcome::Continue,
            Reaction::Redraw => EventOutcome::Redraw,
            Reaction::RedrawAfter(d) => EventOutcome::RedrawAfter(d),
            Reaction::Exit => EventOutcome::Exit,
        }
    }
}

/// The reverse of the conversion above — needed by drivers (`TuiDriver`,
/// `TuiVtDriver`) that dispatch a batch of `UiEvent`s through
/// [`preprocess_event`] (yielding an `EventOutcome` per event) but
/// accumulate their own result as a public-facing [`Reaction`], via
/// [`Reaction::merge`].
impl From<EventOutcome> for Reaction {
    fn from(o: EventOutcome) -> Self {
        match o {
            EventOutcome::Continue => Reaction::Continue,
            EventOutcome::Redraw => Reaction::Redraw,
            EventOutcome::RedrawAfter(d) => Reaction::RedrawAfter(d),
            EventOutcome::Exit => Reaction::Exit,
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
    /// Arm a scheduled wake — see [`Backend::request_frame_in`]
    /// (quadraui#832). `&self`, matching the other two methods and
    /// [`Backend::request_frame_in`]/[`Backend::waker`] themselves:
    /// macOS's implementor (`QuadraView`) is an `objc2` `NSObject`
    /// subclass, which is only ever handed out as `&self` (Objective-C
    /// objects use reference-counted, shared-ownership semantics —
    /// there is no exclusive `&mut` on one), so this trait can't require
    /// `&mut self` without losing that impl entirely. Every
    /// implementation reaches whatever backend state it needs to mutate
    /// through interior mutability instead (a `Cell`/`RefCell`/GTK's
    /// `WAKE_CALLBACKS` thread-local), the same way `request_redraw`/
    /// `request_exit` already do.
    ///
    /// [`Backend::request_frame_in`]: crate::backend::Backend::request_frame_in
    /// [`Backend::waker`]: crate::backend::Backend::waker
    fn request_frame_in(&self, delay: Duration);
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
        EventOutcome::RedrawAfter(d) => sink.request_frame_in(d),
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

    // ── 11. DpiChanged: always repaint, regardless of the app's own
    // Reaction (issue #834). A DPI change means every pixel-based
    // measurement quadraui/the app has cached is now stale — an app that
    // has no opinion on this event (the unhandled-catch-all default,
    // true of most existing examples) would otherwise leave the old,
    // now-mis-scaled frame on screen indefinitely. This is the one place
    // shared across GTK/macOS/Win that guarantees "re-measure on
    // receipt" without every backend's runner needing its own eager
    // invalidate — mirrors the `TextSelectionChanged` force above.
    if matches!(event, UiEvent::DpiChanged(_)) {
        force_redraw = true;
    }

    // ── Normal app dispatch ──────────────────────────────────────────────
    let outcome: EventOutcome = app.handle(event, backend).into();
    if force_redraw {
        match outcome {
            // The selection-highlight update this event caused needs to
            // land on screen now, regardless of what the app's own
            // `Reaction` says about *its* state — including a deferred
            // `RedrawAfter`, which only speaks to when the app wants to
            // be woken again, not whether this frame needs a repaint.
            EventOutcome::Continue | EventOutcome::RedrawAfter(_) => EventOutcome::Redraw,
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

/// Fallback idle-poll bound for backends that keep a coarse "call `tick`
/// even with nothing scheduled" cadence (quadraui#832) — TUI and GTK.
///
/// Before #832 both polled unconditionally, TUI every 16ms
/// (`tui::run::POLL_TIMEOUT`) and GTK every 33ms (`gtk::run::run_with`'s
/// idle timer) — burning CPU waking a fully idle app 60-30 times a
/// second. [`crate::runner::Reaction::RedrawAfter`] +
/// [`crate::backend::Backend::request_frame_in`] replace that for any app
/// that opts in with a *precise* scheduled wake, but this constant stays
/// as the ceiling for apps that don't: an idle-poll fallback is still
/// needed because at least one existing behavior relies on `tick` being
/// called periodically with no explicit request — `examples/common/
/// terminal_app.rs`'s embedded terminal notices new PTY output by
/// polling from `tick`, and that background reader thread predates #831
/// and has no `Backend::waker` wired to it (a natural follow-up, not
/// this issue's scope). 250ms is a deliberate, order-of-magnitude
/// reduction from both old constants (6x fewer wakes than TUI's 16ms, 7x
/// fewer than GTK's 33ms) while keeping that fallback path — and
/// `Backend::waker`'s own latency on TUI specifically, which has no
/// native way to interrupt a blocked `crossterm::event::poll` early, see
/// that method's doc — bounded to a quarter second instead of unbounded.
#[cfg(any(feature = "tui", feature = "gtk"))]
pub(crate) const IDLE_POLL_CEILING: Duration = Duration::from_millis(250);

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

/// TUI's [`Backend::request_frame_in`] bookkeeping (quadraui#832).
///
/// TUI has no native run loop of its own to arm a timer against — its
/// "event loop" is [`tui::run::run_inner`]'s own `loop {}` blocking on
/// `crossterm::event::poll(timeout)` — so unlike GTK/macOS/Windows
/// (each of which schedules a real one-shot native timer,
/// `glib::timeout_add_local_once`/`dispatch_after`/`SetTimer`, from
/// inside `Backend::request_frame_in` itself), TUI's implementation just
/// records the deadline here and folds it into the *next* `poll_timeout`
/// call — the run loop is what actually "waits" for it.
///
/// Coalesces to the earliest outstanding deadline: a later call never
/// pushes a sooner one back, matching
/// [`crate::runner::Reaction::RedrawAfter`]'s documented contract.
///
/// `Cell`-based (not a plain field) so every method takes `&self`, not
/// `&mut self` — matching [`Backend::request_frame_in`]'s own `&self`
/// (the same shape as [`Backend::waker`], and required so its GTK/macOS
/// implementations, which need only a *shared* backend reference, and
/// TUI's, which needs to mutate this deadline, can share one trait
/// signature).
///
/// [`Backend::request_frame_in`]: crate::backend::Backend::request_frame_in
/// [`Backend::waker`]: crate::backend::Backend::waker
/// [`tui::run::run_inner`]: crate::tui::run
#[cfg(feature = "tui")]
pub(crate) struct FrameScheduler {
    deadline: std::cell::Cell<Option<std::time::Instant>>,
    /// Total number of [`Self::request`] calls since construction — the
    /// observable behind [`crate::tui::TuiBackend::frame_requests`]. A
    /// monotonic counter rather than a "was a frame requested" flag
    /// because the behaviour worth testing is the *chained re-arm* (an
    /// app that asks again on every tick while its animation runs, and
    /// stops asking when it finishes); `deadline` alone can't show that,
    /// since coalescing means four requests at the same interval leave
    /// exactly the same deadline one request would.
    requests: std::cell::Cell<u64>,
}

#[cfg(feature = "tui")]
impl FrameScheduler {
    pub(crate) const fn new() -> Self {
        Self {
            deadline: std::cell::Cell::new(None),
            requests: std::cell::Cell::new(0),
        }
    }

    /// Arm (or tighten) the pending deadline to `delay` from now.
    pub(crate) fn request(&self, delay: Duration) {
        self.requests.set(self.requests.get().saturating_add(1));
        let candidate = std::time::Instant::now() + delay;
        self.deadline.set(Some(match self.deadline.get() {
            Some(d) if d <= candidate => d,
            _ => candidate,
        }));
    }

    /// How many times [`Self::request`] has been called since
    /// construction. Never reset — [`Self::clear_if_due`] clears the
    /// deadline, not the count.
    pub(crate) fn requests(&self) -> u64 {
        self.requests.get()
    }

    /// Time remaining until the pending deadline, or `None` when nothing
    /// is armed. Unlike [`Self::poll_timeout`] this doesn't clamp to a
    /// ceiling and doesn't conflate "nothing scheduled" with "scheduled
    /// far out" — the distinction a test needs.
    pub(crate) fn pending_delay(&self) -> Option<Duration> {
        self.deadline
            .get()
            .map(|d| d.saturating_duration_since(std::time::Instant::now()))
    }

    /// How long the run loop may safely block in `wait_events` before it
    /// needs to re-check state: the time remaining until the pending
    /// deadline, clamped to `ceiling` — or `ceiling` itself if nothing is
    /// pending.
    pub(crate) fn poll_timeout(&self, ceiling: Duration) -> Duration {
        match self.deadline.get() {
            Some(d) => d
                .saturating_duration_since(std::time::Instant::now())
                .min(ceiling),
            None => ceiling,
        }
    }

    /// Clear the deadline if it has passed. Call once per loop iteration
    /// right after `wait_events` returns, *before* dispatching that
    /// batch's events or calling `tick` — so a fresh `request` made from
    /// either can't be immediately clobbered by this clearing a deadline
    /// it already fired.
    pub(crate) fn clear_if_due(&self) {
        if self
            .deadline
            .get()
            .is_some_and(|d| std::time::Instant::now() >= d)
        {
            self.deadline.set(None);
        }
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
///
/// **Nothing `!Send` is stored here or anywhere else on the `waker()`
/// path in this module.** The nudge a backend schedules needs a handle to
/// something the UI thread owns, and every such handle (`Rc<RefCell<_>>`,
/// a GTK/GObject wrapper) is `!Send`. Each backend keeps that handle on
/// its own side of the boundary rather than smuggling it through a
/// hand-rolled `unsafe impl Send` wrapper here:
/// - **GTK** parks the `Rc<dyn Fn()>` in a thread-local keyed by an
///   integer id (`gtk::backend::WAKE_CALLBACKS`) and lets `waker()`'s
///   closure carry only the id.
/// - **macOS** uses upstream `dispatch2::MainThreadBound`, which carries
///   an `MainThreadMarker` proof and re-dispatches its own `Drop` back to
///   the main thread.
/// - **Windows** needs no indirection at all: `wndproc` already holds the
///   app/backend state when the posted `WM_QUADRAUI_USER_EVENT` is
///   dispatched.
///
/// An earlier revision of #831 did have a `MainThreadBound` here — a bare
/// `unsafe impl Send/Sync` over an `Rc`. It was removed rather than
/// repaired: with the derived `Drop` it raced `Rc`'s non-atomic refcount
/// whenever the last handle died on a background thread, and forwarding
/// that drop back through `MainContext::invoke` (as `dispatch2` does
/// through `run_on_main`) traded the race for a hang any time the GTK main
/// loop wasn't running to dispatch the forwarded drop. Don't reintroduce
/// it; keep `!Send` state on the thread that owns it.
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
        frame_requests: std::cell::RefCell<Vec<Duration>>,
    }

    impl RecordingSink {
        fn new() -> Self {
            Self {
                redraws: std::cell::Cell::new(0),
                exits: std::cell::Cell::new(0),
                frame_requests: std::cell::RefCell::new(Vec::new()),
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
        fn request_frame_in(&self, delay: Duration) {
            self.frame_requests.borrow_mut().push(delay);
        }
    }

    #[test]
    fn apply_outcome_continue_touches_nothing() {
        let sink = RecordingSink::new();
        apply_outcome(EventOutcome::Continue, &sink);
        assert_eq!(sink.redraws.get(), 0);
        assert_eq!(sink.exits.get(), 0);
        assert!(sink.frame_requests.borrow().is_empty());
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

    /// quadraui#832: `RedrawAfter` must reach `request_frame_in`, not
    /// `request_redraw` — the whole point is *not* forcing a repaint now.
    #[test]
    fn apply_outcome_redraw_after_calls_request_frame_in_not_redraw() {
        let sink = RecordingSink::new();
        apply_outcome(EventOutcome::RedrawAfter(Duration::from_millis(100)), &sink);
        assert_eq!(
            sink.redraws.get(),
            0,
            "RedrawAfter must not force an immediate redraw"
        );
        assert_eq!(
            *sink.frame_requests.borrow(),
            vec![Duration::from_millis(100)]
        );
    }

    #[test]
    fn apply_outcome_accepts_a_raw_redraw_after_reaction() {
        let sink = RecordingSink::new();
        apply_outcome(Reaction::RedrawAfter(Duration::from_millis(50)), &sink);
        assert_eq!(
            *sink.frame_requests.borrow(),
            vec![Duration::from_millis(50)]
        );
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

/// `FrameScheduler` exists only under `feature = "tui"` — see its doc for
/// why GTK/macOS/Windows use a real native one-shot timer instead.
#[cfg(all(test, feature = "tui"))]
mod frame_scheduler_tests {
    use super::*;

    #[test]
    fn no_request_polls_the_full_ceiling() {
        let s = FrameScheduler::new();
        assert_eq!(
            s.poll_timeout(Duration::from_millis(250)),
            Duration::from_millis(250)
        );
    }

    #[test]
    fn a_request_shorter_than_the_ceiling_shortens_the_poll_timeout() {
        let s = FrameScheduler::new();
        s.request(Duration::from_millis(10));
        // The exact remaining duration is timing-sensitive (a few
        // microseconds elapse between `request` and `poll_timeout`), but
        // it must be positive and well under the ceiling.
        let timeout = s.poll_timeout(Duration::from_millis(250));
        assert!(
            timeout <= Duration::from_millis(10) && timeout > Duration::ZERO,
            "expected a short bounded timeout, got {timeout:?}"
        );
    }

    #[test]
    fn a_request_longer_than_the_ceiling_is_clamped_to_it() {
        let s = FrameScheduler::new();
        s.request(Duration::from_secs(10));
        assert_eq!(
            s.poll_timeout(Duration::from_millis(250)),
            Duration::from_millis(250)
        );
    }

    /// A later, *sooner* request must win — the earliest deadline always
    /// governs, matching `Reaction::RedrawAfter`'s documented contract.
    #[test]
    fn coalesces_to_the_earliest_deadline() {
        let s = FrameScheduler::new();
        s.request(Duration::from_secs(10));
        s.request(Duration::from_millis(5));
        let timeout = s.poll_timeout(Duration::from_secs(60));
        assert!(
            timeout <= Duration::from_millis(5),
            "a sooner request must shorten the deadline, got {timeout:?}"
        );
    }

    /// A later, *later* request must not push a sooner one back.
    #[test]
    fn a_later_looser_request_does_not_override_a_sooner_one() {
        let s = FrameScheduler::new();
        s.request(Duration::from_millis(5));
        s.request(Duration::from_secs(10));
        let timeout = s.poll_timeout(Duration::from_secs(60));
        assert!(
            timeout <= Duration::from_millis(5),
            "the earlier, sooner deadline must still govern, got {timeout:?}"
        );
    }

    #[test]
    fn clear_if_due_is_a_noop_before_the_deadline() {
        let s = FrameScheduler::new();
        s.request(Duration::from_secs(60));
        s.clear_if_due();
        // Still armed — the ceiling-clamped timeout stays short of the
        // ceiling (i.e. governed by the still-pending deadline), not the
        // full ceiling a cleared scheduler would report.
        assert!(s.poll_timeout(Duration::from_millis(1)) <= Duration::from_millis(1));
        assert!(s.deadline.get().is_some());
    }

    #[test]
    fn clear_if_due_clears_a_past_deadline() {
        let s = FrameScheduler::new();
        // A zero-length request is due immediately.
        s.request(Duration::ZERO);
        std::thread::sleep(Duration::from_millis(1));
        s.clear_if_due();
        assert!(s.deadline.get().is_none());
        assert_eq!(
            s.poll_timeout(Duration::from_millis(250)),
            Duration::from_millis(250)
        );
    }

    /// This is quadraui#832's core acceptance criterion made concrete:
    /// with nothing scheduled, the number of wake-ups over a fixed
    /// interval is bounded by the (much coarser) idle ceiling, not the
    /// pre-#832 unconditional 16ms poll — RED against the old constant,
    /// GREEN against `IDLE_POLL_CEILING`.
    #[test]
    fn idle_wakeups_over_one_second_are_bounded_by_the_ceiling_not_the_old_16ms_poll() {
        let s = FrameScheduler::new();
        let ceiling = IDLE_POLL_CEILING;
        let interval = Duration::from_secs(1);

        let old_poll_timeout_ms = 16u128; // pre-#832 `tui::run::POLL_TIMEOUT`
        let old_wakeups = interval.as_millis() / old_poll_timeout_ms;
        let new_wakeups = interval.as_millis() / s.poll_timeout(ceiling).as_millis();

        assert_eq!(old_wakeups, 62, "sanity check on the pre-#832 baseline");
        assert!(
            new_wakeups < old_wakeups / 4,
            "expected the new idle ceiling ({ceiling:?}) to cut wakeups \
             over {interval:?} to well under a quarter of the old \
             unconditional-poll baseline ({old_wakeups}); got {new_wakeups}"
        );
    }
}

/// [`EventOutcome::merge`] — the fix for a review finding on quadraui#832:
/// every batch-dispatch loop across `macos::run`, `win::run`,
/// `gtk::testing`, `tui::testing` and `tui::vt_testing` used to keep the
/// *first* non-`Continue` outcome seen in a batch rather than the
/// *earliest* `RedrawAfter` deadline, which could silently drop a shorter,
/// more urgent wake requested by a later event in the same batch. These
/// tests pin the corrected, order-independent semantics directly, since
/// the batches that would exercise this in a live runner (multiple
/// synthetic events from one drag gesture, each separately requesting a
/// different `RedrawAfter`) are awkward to synthesize end-to-end.
#[cfg(test)]
mod event_outcome_merge_tests {
    use super::*;

    #[test]
    fn exit_beats_everything_either_order() {
        assert!(matches!(
            EventOutcome::Continue.merge(EventOutcome::Exit),
            EventOutcome::Exit
        ));
        assert!(matches!(
            EventOutcome::Exit.merge(EventOutcome::Redraw),
            EventOutcome::Exit
        ));
        assert!(matches!(
            EventOutcome::RedrawAfter(Duration::from_millis(5)).merge(EventOutcome::Exit),
            EventOutcome::Exit
        ));
    }

    #[test]
    fn redraw_beats_redraw_after_either_order() {
        assert!(matches!(
            EventOutcome::RedrawAfter(Duration::from_millis(5)).merge(EventOutcome::Redraw),
            EventOutcome::Redraw
        ));
        assert!(matches!(
            EventOutcome::Redraw.merge(EventOutcome::RedrawAfter(Duration::from_millis(5))),
            EventOutcome::Redraw
        ));
    }

    #[test]
    fn continue_is_the_identity() {
        assert!(matches!(
            EventOutcome::Continue.merge(EventOutcome::Continue),
            EventOutcome::Continue
        ));
        assert!(matches!(
            EventOutcome::Continue.merge(EventOutcome::RedrawAfter(Duration::from_millis(5))),
            EventOutcome::RedrawAfter(d) if d == Duration::from_millis(5)
        ));
    }

    /// The actual regression: a shorter deadline arriving *after* a longer
    /// one in the same batch must still win — "first wins" would have kept
    /// the 100ms request and dropped the 5ms one.
    #[test]
    fn two_redraw_afters_keep_the_earlier_deadline_regardless_of_arrival_order() {
        let long = EventOutcome::RedrawAfter(Duration::from_millis(100));
        let short = EventOutcome::RedrawAfter(Duration::from_millis(5));

        assert!(matches!(
            long.merge(short),
            EventOutcome::RedrawAfter(d) if d == Duration::from_millis(5)
        ));
        assert!(matches!(
            short.merge(long),
            EventOutcome::RedrawAfter(d) if d == Duration::from_millis(5)
        ));
    }
}
