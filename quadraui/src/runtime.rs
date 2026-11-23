//! Shared plumbing for the per-backend runners (`tui::run`, `gtk::run`,
//! `macos::run`, `win::run`).
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
//! - The 120ms trailing-edge resize-settle debounce: TUI
//!   and GTK each reinvented it with a twin ~20-line rationale comment.
//!   [`RESIZE_SETTLE`] centralises the constant + the doc; TUI also
//!   adopts [`ResizeDebouncer`] for the pending-viewport coalescing
//!   itself (GTK's mechanism is GLib-timer-native — cancel + reschedule
//!   a `glib::SourceId` — and doesn't need a separate pending-value
//!   store, so it only picks up the shared constant/doc). macOS and
//!   Windows dispatched `WindowResized` undebounced until they adopted
//!   the shared machinery. Both now adopt [`ResizeDebouncer`] too: macOS's
//!   `NSTimer` cancel-and-reschedule and Windows' `SetTimer`
//!   replace-if-already-armed both give "fire once, `RESIZE_SETTLE`
//!   after the last resize" for free, same as GTK's `glib::SourceId`,
//!   but — unlike GTK — neither can cheaply re-read a "live" size at
//!   fire time from outside the event that carried it, so they still
//!   need somewhere to stash the pending viewport between "resize
//!   arrived" and "burst settled". [`ResizeDebouncer`] is exactly that
//!   store, decoupled from any particular platform timer so its
//!   coalescing behavior stays unit-testable here even though the
//!   `NSTimer`/`SetTimer` plumbing that drives it isn't.
//! - [`preprocess_event`] — the copy/paste/selection/double-click/accelerator
//!   pipeline every `dispatch_event` ran ahead of `AppLogic::handle`.
//!   This unified the four independent `dispatch_event` ladders that each
//!   implemented a different subset of this pipeline. Concretely, before
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
//!   Two of the divergences discovered really were load-bearing, not
//!   accidental, and stay preserved as documented per-backend overrides
//!   on [`PreprocessBackend`] rather than being normalised away:
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
//!   (middle-click paste, and Ctrl-V on TUI) — unified behavior brought
//!   by routing all backends through this one function.

use crate::backend::PointerShape;
use crate::desktop::{is_paste_keypress, PasteModifier};
use crate::focus::FocusTree;
use crate::frame_tree::{FrameTree, FrameTreeNode};
use crate::interactive::{EventCx, EventHost, FocusSpec, KeyEvent, PointerKind, RoutedEvent};
use crate::primitive::Units;
use crate::runner::AppLogic;
use crate::text_selection::ActiveTextSelection;
use crate::{
    AcceleratorId, ActivityBarEvent, Handled, Key, Modifiers, MouseButton, NamedKey, Point,
    UiEvent, WidgetId,
};

// `EventOutcome::RedrawAfter` needs `Duration` unconditionally
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
/// A plain alias for [`Reaction`]: the two are the same four outcomes
/// under two names, so [`Reaction::merge`] is the one merge
/// implementation both the public `AppLogic::handle` contract and this
/// crate-internal plumbing share, and no conversion is needed at the
/// boundary between them.
pub(crate) type EventOutcome = Reaction;

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
    /// Arm a scheduled wake — see [`Backend::request_frame_in`].
    /// `&self`, matching the other two methods and
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

/// Apply an [`EventOutcome`] to a [`ReactionSink`]: the single definition
/// every GTK and macOS call site shares instead of its own near-identical
/// `match`.
#[cfg(any(feature = "gtk", all(feature = "macos", target_os = "macos")))]
pub(crate) fn apply_outcome(outcome: EventOutcome, sink: &impl ReactionSink) {
    match outcome {
        EventOutcome::Continue => {}
        EventOutcome::Redraw => sink.request_redraw(),
        EventOutcome::RedrawAfter(d) => sink.request_frame_in(d),
        EventOutcome::Exit => sink.request_exit(),
    }
}

/// The capability surface [`preprocess_event`] needs from a concrete
/// backend to run the shared copy/paste/selection/double-click/
/// accelerator pipeline. One `impl` per concrete backend (`TuiBackend`,
/// `GtkBackend`, `MacBackend`, `WinBackend`), each defined next to the
/// type in its own `backend.rs` so the method bodies can delegate to
/// that backend's existing (often private) helpers without new
/// crate-visible surface.
///
/// Every method has a concrete counterpart that already existed on at
/// least one backend. The two methods with default bodies
/// ([`Self::paste_modifier`], [`Self::is_copy_keypress`]) are the two
/// spots where a *platform* reason makes one backend answer differently
/// — see this module's doc for which backend overrides which and why.
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

    /// Mutable access to this backend's widget-contract routing state
    /// (hover/capture bookkeeping + per-widget
    /// [`crate::widget_state::WidgetStateStore`] — see
    /// [`crate::widget_state::WidgetRoutingState`]), or `None` for a
    /// backend that hasn't embedded one in its
    /// [`crate::backend_core::BackendCore`] yet.
    ///
    /// Default: `None` — exactly zero behavior change for a backend that
    /// doesn't override this. [`preprocess_event`]'s widget-routing step
    /// (step 2 below) checks this *and* [`AppLogic::frame_tree`] before
    /// doing anything, so a backend that returns `None` here never enters
    /// that step at all, identically to how an app that never overrides
    /// [`AppLogic::extra_tab_stops`] keeps step 4
    /// (Tab/Shift+Tab cycling) inert. `TuiBackend` and `MacBackend` override
    /// it today — see their own `PreprocessBackend` impls.
    fn widget_routing_mut(&mut self) -> Option<&mut crate::widget_state::WidgetRoutingState> {
        None
    }

    /// Mutable access to this backend's [`FocusTree`] — the counterpart to
    /// the sealed [`crate::Backend::focus_tree`]'s read-only borrow, used
    /// internally by the widget-contract routing engine
    /// ([`route_widget_key`], [`WidgetEventHost`]) and by
    /// [`preprocess_event`]'s own Tab/Shift+Tab step (step 4 in its own
    /// doc) to keep the tree's stops synced with each dispatched event's
    /// [`crate::frame_tree::FrameTree`] ([`FocusTree::sync`]) and, once a
    /// widget resolves which of its own children a gesture or a focus
    /// request names, to set that sub-target's focus precisely —
    /// `(WidgetId, Some(SubTarget))`, which
    /// [`crate::Backend::set_focus`]'s flat `WidgetId` signature cannot
    /// express.
    fn focus_tree_mut(&mut self) -> &mut FocusTree;
}

/// Dispatch one already-translated [`UiEvent`] through the app, applying
/// the shared runner pre-processing pipeline first. This is the single
/// function every backend's `dispatch_event` now funnels through for the
/// steps below — see this module's doc for the full history and the two
/// documented per-backend exceptions.
///
/// Pre-processing handled here, in priority order (anything not matched
/// falls through to `app.handle` unchanged):
///
/// 1. [`UiEvent::MouseDown`] → [`PreprocessBackend::fold_double_click`]:
///    fold into [`UiEvent::DoubleClick`] if it lands in the backend's
///    double-click window.
/// 2. Widget-contract routing, only when
///    [`PreprocessBackend::widget_routing_mut`] returns `Some` *and*
///    [`AppLogic::frame_tree`] returns a non-empty
///    [`crate::frame_tree::FrameTree`] for the current frame — see
///    [`route_widget_event`]'s own doc for the full
///    hit-test/capture/Enter-Leave/focus/bubbling algorithm. Runs right
///    after double-click folding (so a folded `DoubleClick` is what a
///    widget sees, not the raw second `MouseDown`) and before every
///    later step, so a widget's own keys (a toy counter's
///    `Up`/`Down`/`Enter`) reach [`crate::interactive::Interactive::handle`]
///    before the shared Tab-cycling/accelerator steps below get a
///    chance to reinterpret them. A widget that doesn't want an event
///    for itself answers [`Handled::Ignored`] and it bubbles down to
///    step 3 unchanged, exactly as if this step didn't run at all —
///    except that any `Reaction` this step *did* produce on the way
///    (a hover repaint, an [`crate::interactive::EventHost::emit`]ted
///    event's own `Reaction`) is still merged into whatever the step
///    that finally answers the event returns.
/// 3. `KeyPressed` while an `ActivityBar` declared
///    `is_keyboard_focused = true`: redirect to
///    `UiEvent::ActivityBar(id, KeyPressed { .. })` instead of the app's
///    normal `handle` — `ShellAdapter`'s built-in activity-bar keyboard
///    cursor depends on this.
/// 4. Tab / Shift+Tab focus cycling, only when [`all_tab_stops`] (today
///    just [`AppLogic::extra_tab_stops`], run through a defensive
///    by-id dedup) returns non-empty for the current frame *and* the
///    currently-focused id (if any) is actually named by it: sync
///    [`FocusTree`]'s tab order to this flat list and cycle it
///    ([`FocusTree::focus_next`]/[`FocusTree::focus_prev`] —
///    `NamedKey::BackTab`, or `NamedKey::Tab` with Shift held, retreats;
///    plain `NamedKey::Tab` advances), then deliver
///    `UiEvent::FocusChanged` instead of the raw key press. Ctrl/Alt/Cmd
///    held is left alone (reserved for app-level chords like
///    `TabGroupController`'s Ctrl-Tab pane switching). While it stays
///    empty — the default — this step never fires and Tab/Shift+Tab pass
///    through unchanged, exactly as before. When the currently-focused id
///    is focused but deliberately absent from the list (e.g. a
///    main-tree `"form"` extra holding engine focus — see
///    `InteractionEngine::tab_stops`'s own doc), this step also stays out
///    of the way instead of reading the omission as "this id went away"
///    and clearing focus: the raw key press falls through to
///    `app.handle` unchanged, so whatever excluded it can claim it with
///    its own more specific handling.
/// 5. `KeyPressed` matching a registered `Global`-scope accelerator:
///    rewrite to `UiEvent::Accelerator`.
/// 6. Ctrl-C ([`PreprocessBackend::is_copy_keypress`]) with an active
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
///    misconfigured tmux swallows the copy entirely. Apps should
///    word their copy confirmation accordingly; see
///    `quadraui/docs/CLIPBOARD.md`.
/// 7. Ctrl-V / Ctrl-Shift-V ([`is_paste_keypress`], keyed off
///    [`PreprocessBackend::paste_modifier`]): read the clipboard and
///    deliver `UiEvent::ClipboardPaste` instead of forwarding the raw key
///    press.
/// 8. Middle-click (`MouseDown` with `MouseButton::Middle`): read the
///    PRIMARY selection and deliver `UiEvent::ClipboardPaste` — the
///    X11/Wayland "paste what was last selected" convention, distinct
///    from the CLIPBOARD selection Ctrl-V reads. Every backend but GTK's
///    `Clipboard` impl answers `None` here (see
///    [`crate::backend::Clipboard::read_primary_selection`]'s default),
///    so this step is a safe no-op everywhere else.
/// 9. Ctrl-A: select the entire content of the most-recently focused
///    `TextRegion`, if one is registered.
/// 10. `MouseDown` or `DoubleClick`: clear the displayed selection
///     highlight — a fresh drag (or a folded double-click landing on the
///     same spot) may be starting/finishing and shouldn't show a stale
///     highlight from the previous interaction. Never cancels an
///     in-progress drag — `clear_selection_display` only clears the
///     rendered overlay (see its doc).
/// 11. `TextSelectionChanged`: update the backend's active selection and
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

    // ── 2. Widget-contract routing ─────────────────────────────────────
    // A `Bubble` payload is a `Reaction` step 2 produced without claiming
    // the event, so every `return` below merges it in rather than
    // answering with its own `Reaction` alone — it is `Reaction::Continue`
    // (`merge`'s identity) in all but the few cases named on
    // `WidgetRouteOutcome::Bubble`.
    let widget_side_reaction = match route_widget_event(&event, backend, app) {
        WidgetRouteOutcome::Claimed(outcome) => return outcome,
        WidgetRouteOutcome::Bubble(side_effect) => side_effect,
    };

    // ── 3. ActivityBar keyboard focus intercept ─────────────────────
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
            return widget_side_reaction.merge(app.handle(bar_ev, backend));
        }
    }

    // ── 4. Tab / Shift+Tab focus cycling ────────────────────────────────
    if let UiEvent::KeyPressed {
        key: Key::Named(named @ (NamedKey::Tab | NamedKey::BackTab)),
        modifiers,
        ..
    } = &event
    {
        if !modifiers.ctrl && !modifiers.alt && !modifiers.cmd {
            let stops = all_tab_stops(app, A::AreaId::default());
            let currently_focused = backend.focus_tree().focused().map(|(id, _)| *id);
            // A focused id this frame's `stops` doesn't name is never "an
            // id this cycle lost track of" — `AppLogic::extra_tab_stops`
            // only ever omits one deliberately, to hand the key to its own
            // more specific handling instead (the main-tree `"form"` extra
            // case: `InteractionEngine::tab_stops` excludes its own stop
            // for as long as it holds engine focus, so the raw key below
            // reaches `InteractionEngine::handle_form_tab`). Running
            // `sync_tab_order` anyway would read that omission as "this id
            // went away" and clear focus, landing the next
            // `focus_next()`/`focus_prev()` on the first/last stop instead
            // of leaving the key alone for whoever excluded it on purpose.
            let focus_excluded_from_stops = currently_focused
                .as_ref()
                .is_some_and(|id| !stops.iter().any(|(stop_id, _)| stop_id == id));
            if !stops.is_empty() && !focus_excluded_from_stops {
                backend.focus_tree_mut().sync_tab_order(&stops);
                let retreat = matches!(named, NamedKey::BackTab) || modifiers.shift;
                let tree = backend.focus_tree_mut();
                let changed = if retreat {
                    tree.focus_prev()
                } else {
                    tree.focus_next()
                };
                if changed {
                    let focused = backend.focus_tree().focused().map(|(id, _)| *id);
                    return widget_side_reaction
                        .merge(app.handle(UiEvent::FocusChanged(focused), backend));
                }
                // Claimed but no-op (e.g. a single-widget tab order) —
                // swallow it rather than letting it fall through to
                // `app.handle` as a raw KeyPressed once an app has
                // opted in at all.
                return widget_side_reaction;
            }
        }
    }

    // ── 5. Global accelerator dispatch ─────────────────────────────────
    let event = if let UiEvent::KeyPressed { key, modifiers, .. } = &event {
        match backend.match_keypress(key, *modifiers) {
            Some(id) => UiEvent::Accelerator(id, *modifiers),
            None => event,
        }
    } else {
        event
    };

    // ── 6. Ctrl-C interception (copy active text selection) ────────────
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
            return widget_side_reaction.merge(app.handle(UiEvent::TextCopied(text), backend));
        }
    }

    // ── 7. Ctrl-V / Ctrl-Shift-V interception (paste) ───────────────────
    if let UiEvent::KeyPressed {
        ref key,
        ref modifiers,
        ..
    } = event
    {
        if is_paste_keypress(key, modifiers, backend.paste_modifier()) {
            return widget_side_reaction.merge(match backend.services().clipboard().read_text() {
                Some(text) => app.handle(UiEvent::ClipboardPaste(text), backend),
                None => EventOutcome::Continue,
            });
        }
    }

    // ── 8. Middle-click interception (PRIMARY-selection paste) ──────────
    if let UiEvent::MouseDown {
        button: MouseButton::Middle,
        ..
    } = &event
    {
        if let Some(text) = backend.services().clipboard().read_primary_selection() {
            return widget_side_reaction.merge(app.handle(UiEvent::ClipboardPaste(text), backend));
        }
    }

    // ── 9. Ctrl-A interception (select-all for text regions) ────────────
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
            return widget_side_reaction.merge(EventOutcome::Redraw);
        }
    }

    // ── 10. MouseDown / DoubleClick: clear the displayed selection ───────
    if matches!(
        event,
        UiEvent::MouseDown { .. } | UiEvent::DoubleClick { .. }
    ) {
        backend.clear_selection_display();
    }

    // ── 11. TextSelectionChanged: update active selection while dragging ─
    let mut force_redraw = false;
    if let UiEvent::TextSelectionChanged {
        ref region,
        anchor,
        focus,
    } = event
    {
        backend.set_active_text_selection(*region, anchor, focus);
        force_redraw = true;
    }

    // ── 12. DpiChanged: always repaint, regardless of the app's own
    // Reaction. A DPI change means every pixel-based
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
    // Step 2's widget-routing side effect (a hover Enter/Leave repaint, a
    // captured drag's `Consumed`) never claimed the event, but still has
    // to land in the final `Reaction` rather than be discarded — merged
    // in here, ahead of `force_redraw`, exactly like any other `Reaction`
    // produced within this one event's handling.
    let outcome: EventOutcome = widget_side_reaction.merge(app.handle(event, backend));
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

// ─── Widget-contract routing engine ─────────────────────────────────────────
//
// Hit-test/capture/Enter-Leave/focus/bubbling for `Interactive` widgets —
// step 2 of `preprocess_event`'s pipeline above. See `route_widget_event`'s
// own doc for the algorithm and `crate::frame_tree`/`crate::widget_state`'s
// module docs for the two types it's built on.

/// Step 2 of [`preprocess_event`]: route `event` to whichever
/// [`crate::frame_tree::FrameTree`] widget it targets, if any.
///
/// Returns [`WidgetRouteOutcome::Claimed`] when the widget contract
/// claimed `event` entirely — either consuming it (a redraw) or producing
/// a semantic [`UiEvent`] already routed through `app.handle`. Returns
/// [`WidgetRouteOutcome::Bubble`] to mean **bubble**: nothing claimed it,
/// so `preprocess_event`'s later steps (and eventually `app.handle` with
/// the original `event`) run exactly as if this step didn't exist — but
/// its payload `Reaction` (`Continue` when there's nothing to report) is
/// still merged into whatever `preprocess_event` ultimately returns, so a
/// side effect like a hover repaint isn't lost just because the move
/// itself bubbles. `Bubble(Reaction::Continue)` covers every case where
/// the opt-in itself isn't live (`widget_routing_mut` returns `None`, or
/// `AppLogic::frame_tree` is empty), so this function is always safe to
/// call unconditionally — it reads as "try widget routing first" rather
/// than a conditional the caller has to gate itself.
///
/// # Algorithm
///
/// - **Modal scope.** If [`crate::Backend::modal_stack_handle`]'s stack is
///   non-empty, this step does nothing at all (`Bubble(Continue)`) —
///   `Interactive` widgets are base-layer content only (there is no
///   modal-interior `Surface::Widget` story yet), so any open modal
///   blocks the whole engine rather than trying to arbitrate which side
///   of the stack a hit belongs to. This is what makes a modal "block" a
///   widget: the widget never sees the event, and — because this
///   function bubbles — neither does anything else below it that would
///   otherwise reach the base layer unprotected.
/// - **Hit-test** (`MouseDown`): [`FrameTree::hit_test`] against the
///   event's position; a hit whose [`crate::interactive::FocusSpec`]
///   isn't `None` also moves keyboard focus to it first (click-to-focus),
///   before the gesture itself is delivered.
/// - **Focus routing** (`KeyPressed`): the currently-focused widget (via
///   [`crate::Backend::focus_tree`]), if it's named in this frame's
///   tree.
/// - **Capture** (`MouseMoved`, [`EventHost::capture_pointer`]/
///   `release_pointer`): while
///   [`crate::widget_state::WidgetRoutingState::captured`] names a
///   widget, every move routes to it directly ([`PointerKind::Move`]),
///   bypassing hit-testing/hover entirely — see [`route_widget_move`].
/// - **Release** (`MouseUp`): routes [`PointerKind::Up`] to whichever
///   widget currently holds the capture, if any — see
///   [`route_widget_pointer_up`]. This is what lets a widget's own
///   `MouseUp` arm actually run [`EventHost::release_pointer`] in the real
///   runtime, not just in a test that hands `handle` an `Up` by hand;
///   without it a capture taken on `MouseDown` is never released, so
///   every later `MouseMoved` keeps routing to the captured widget
///   (`route_widget_move`) and every other widget on screen permanently
///   loses hover (`route_widget_hover` is bypassed while captured).
///   Bubbles (`Bubble(Continue)`) when nothing is captured — a release
///   with no active capture has nothing to deliver.
/// - **Enter/Leave** (`MouseMoved`, no active capture): compares this
///   move's hit-test result against
///   [`crate::widget_state::WidgetRoutingState::hovered`] and delivers
///   [`PointerKind::Leave`]/[`PointerKind::Enter`] to whichever widgets
///   changed — see [`route_widget_hover`]. Never claims the move itself
///   (`MouseMoved` always bubbles, as [`WidgetRouteOutcome::Bubble`]) —
///   hover tracking is a side effect, not a consumption; the app's own
///   `MouseMoved` handling (cursors, tooltips) still runs. The resulting
///   `Reaction` (e.g. a hover highlight's redraw) is kept, though: it
///   travels back as `Bubble`'s payload and [`preprocess_event`] merges
///   it into whatever it ultimately returns for this event.
/// - **Bubbling** is [`crate::primitive::DynWidget::handle`]'s own job via
///   [`crate::compose::Handled::Ignored`] — see
///   [`dispatch_widget_routed_event`].
///
/// Every other [`UiEvent`] variant falls straight through
/// (`Bubble(Continue)`) — scroll/double-click widget routing is future
/// work, not needed by anything that opts in today. `MouseUp` is handled
/// above, not here, since it only ever needs the capture lookup, never a
/// fresh hit-test.
///
/// Returns [`WidgetRouteOutcome`] rather than a bare `Option<EventOutcome>`
/// because `MouseMoved`'s hover/capture handling never claims the event
/// (so [`preprocess_event`]'s later steps and `app.handle` still run) but
/// can still produce a `Reaction` — a hover highlight's redraw request, a
/// captured drag's `Consumed` — that would otherwise be silently dropped;
/// see [`WidgetRouteOutcome::Bubble`].
fn route_widget_event<B, A>(event: &UiEvent, backend: &mut B, app: &mut A) -> WidgetRouteOutcome
where
    B: PreprocessBackend + crate::Backend,
    A: AppLogic,
{
    if backend.widget_routing_mut().is_none() {
        return WidgetRouteOutcome::Bubble(Reaction::Continue);
    }
    if !backend.modal_stack_handle().borrow().is_empty() {
        return WidgetRouteOutcome::Bubble(Reaction::Continue);
    }
    let tree = app.frame_tree(A::AreaId::default());
    if tree.is_empty() {
        return WidgetRouteOutcome::Bubble(Reaction::Continue);
    }
    // Keep the sub-target-aware `FocusTree` seam fresh for this event —
    // `route_widget_key`/`WidgetEventHost` read it below. Cheap to
    // rebuild every routed event, same tradeoff `app.frame_tree` above
    // already makes (see this module's doc on why a `FrameTree` node
    // can't be cached across the frame).
    backend.focus_tree_mut().sync(&tree);

    match event {
        UiEvent::MouseDown {
            position,
            button,
            modifiers,
            ..
        } => route_widget_pointer_down(*position, *button, *modifiers, backend, app, &tree),
        UiEvent::MouseMoved { position, .. } => {
            let side_effect = route_widget_move(*position, backend, app, &tree);
            WidgetRouteOutcome::Bubble(side_effect)
        }
        UiEvent::MouseUp {
            position, button, ..
        } => route_widget_pointer_up(*position, *button, backend, app, &tree),
        UiEvent::KeyPressed {
            key,
            modifiers,
            repeat,
        } => route_widget_key(key.clone(), *modifiers, *repeat, backend, app, &tree),
        UiEvent::DoubleClick { position, .. } => {
            route_widget_double_click(*position, backend, app, &tree)
        }
        _ => WidgetRouteOutcome::Bubble(Reaction::Continue),
    }
}

/// [`route_widget_event`]'s result: either it claimed `event` entirely, or
/// it didn't but still wants a `Reaction` folded into whatever
/// [`preprocess_event`] ultimately returns for this event.
enum WidgetRouteOutcome {
    /// The widget contract claimed the event — use this `Reaction`
    /// directly, exactly as the old `Option<EventOutcome>::Some` did.
    Claimed(EventOutcome),
    /// Nothing claimed the event ("bubble" — `preprocess_event`'s later
    /// steps and eventually `app.handle` with the original event still
    /// run), but widget routing itself produced this `Reaction` as a side
    /// effect (e.g. a hover Enter/Leave repaint, a captured drag's
    /// `Consumed`, or an [`EventHost::emit`]ted event's own `Reaction` from
    /// a widget that answered [`Handled::Ignored`]) that must still be
    /// merged ([`Reaction::merge`]) into the final result rather than
    /// discarded — `Reaction::Continue` when there was no side effect to
    /// report.
    Bubble(Reaction),
}

impl WidgetRouteOutcome {
    /// The `Reaction` this outcome carries, discarding the claimed/bubbled
    /// distinction — for the callers that never claim the event in the
    /// first place and only need the side effect to fold into their own
    /// `Reaction` ([`route_widget_move`], [`route_widget_hover`]).
    fn reaction(self) -> Reaction {
        match self {
            Self::Claimed(outcome) => outcome,
            Self::Bubble(reaction) => reaction,
        }
    }
}

/// `MouseDown` arm of [`route_widget_event`]: hit-test, click-to-focus,
/// then deliver [`PointerKind::Down`]. `Bubble(Continue)` when the press
/// landed on no widget at all.
fn route_widget_pointer_down<B, A>(
    position: Point,
    button: MouseButton,
    modifiers: Modifiers,
    backend: &mut B,
    app: &mut A,
    tree: &FrameTree,
) -> WidgetRouteOutcome
where
    B: PreprocessBackend + crate::Backend,
    A: AppLogic,
{
    let Some(node) = tree.hit_test(position) else {
        return WidgetRouteOutcome::Bubble(Reaction::Continue);
    };
    let id = node.id;
    if !matches!(node.widget.focus(), FocusSpec::None) {
        backend.set_focus(Some(id));
    }
    let routed = RoutedEvent::Pointer {
        kind: PointerKind::Down(button),
        sub: None,
        local: local_point(position, node.rect),
        mods: modifiers,
    };
    dispatch_widget_routed_event(id, node, &routed, backend, app)
}

/// `DoubleClick` arm of [`route_widget_event`]: hit-test, then deliver
/// [`PointerKind::DoubleClick`] — the same hit-test-and-deliver shape as
/// [`route_widget_pointer_down`], except [`UiEvent::DoubleClick`] carries
/// no button or modifiers of its own, so this always resolves
/// [`MouseButton::Left`] with [`Modifiers::default`]. `Bubble(Continue)`
/// when the click landed on no widget.
fn route_widget_double_click<B, A>(
    position: Point,
    backend: &mut B,
    app: &mut A,
    tree: &FrameTree,
) -> WidgetRouteOutcome
where
    B: PreprocessBackend + crate::Backend,
    A: AppLogic,
{
    let Some(node) = tree.hit_test(position) else {
        return WidgetRouteOutcome::Bubble(Reaction::Continue);
    };
    let id = node.id;
    let routed = RoutedEvent::Pointer {
        kind: PointerKind::DoubleClick(MouseButton::Left),
        sub: None,
        local: local_point(position, node.rect),
        mods: Modifiers::default(),
    };
    dispatch_widget_routed_event(id, node, &routed, backend, app)
}

/// `MouseUp` arm of [`route_widget_event`]: deliver [`PointerKind::Up`] to
/// whichever widget currently holds the pointer capture (see
/// [`EventHost::capture_pointer`]), the same capture-bypasses-hit-test rule
/// [`route_widget_move`] follows — a release has to reach the widget that
/// is still dragging even if the pointer has since left its bounds.
/// `Bubble(Continue)` when nothing is captured, including a capture whose
/// widget id this frame's tree doesn't have — there is nothing to
/// release either way.
fn route_widget_pointer_up<B, A>(
    position: Point,
    button: MouseButton,
    backend: &mut B,
    app: &mut A,
    tree: &FrameTree,
) -> WidgetRouteOutcome
where
    B: PreprocessBackend + crate::Backend,
    A: AppLogic,
{
    let Some(captured_id) = backend.widget_routing_mut().and_then(|r| r.captured) else {
        return WidgetRouteOutcome::Bubble(Reaction::Continue);
    };
    let Some(node) = tree.find(&captured_id) else {
        return WidgetRouteOutcome::Bubble(Reaction::Continue);
    };
    let routed = RoutedEvent::Pointer {
        kind: PointerKind::Up(button),
        sub: None,
        local: local_point(position, node.rect),
        mods: Modifiers::default(),
    };
    dispatch_widget_routed_event(captured_id, node, &routed, backend, app)
}

/// `KeyPressed` arm of [`route_widget_event`]: route to whichever widget
/// currently holds keyboard focus, if any, and if this frame's
/// [`FrameTree`] still names it — `Bubble(Continue)` when either is
/// missing.
///
/// Reads [`crate::Backend::focus_tree`], which already carries a
/// composite widget's focused sub-target rather than only the whole
/// widget's id — the seam a future hit-test can use to route a key to
/// the right address instead of always `None`.
fn route_widget_key<B, A>(
    key: Key,
    modifiers: Modifiers,
    repeat: bool,
    backend: &mut B,
    app: &mut A,
    tree: &FrameTree,
) -> WidgetRouteOutcome
where
    B: PreprocessBackend + crate::Backend,
    A: AppLogic,
{
    let Some(&(focused, _)) = backend.focus_tree().focused() else {
        return WidgetRouteOutcome::Bubble(Reaction::Continue);
    };
    let Some(node) = tree.find(&focused) else {
        return WidgetRouteOutcome::Bubble(Reaction::Continue);
    };
    let routed = RoutedEvent::Key(KeyEvent {
        key,
        modifiers,
        repeat,
    });
    dispatch_widget_routed_event(focused, node, &routed, backend, app)
}

/// `MouseMoved` arm of [`route_widget_event`]: while a widget holds an
/// active pointer capture (see [`EventHost::capture_pointer`]), every
/// move routes to it directly — [`PointerKind::Move`], bypassing
/// hit-testing and hover tracking entirely, the standard "everything
/// goes to the captured widget regardless of where the pointer is" rule
/// a drag/resize/scrollbar-track gesture depends on. Otherwise falls
/// through to [`route_widget_hover`] for plain Enter/Leave tracking.
///
/// Returns the `Reaction` [`dispatch_widget_routed_event`] produced
/// (`Reaction::Continue` when nothing was dispatched, e.g. the captured
/// id doesn't name a live node anymore) — the move is never *claimed*
/// (see [`route_widget_event`]'s doc), but this side effect still needs
/// to reach [`preprocess_event`]'s final result via
/// [`WidgetRouteOutcome::Bubble`].
fn route_widget_move<B, A>(
    position: Point,
    backend: &mut B,
    app: &mut A,
    tree: &FrameTree,
) -> Reaction
where
    B: PreprocessBackend + crate::Backend,
    A: AppLogic,
{
    if let Some(captured_id) = backend.widget_routing_mut().and_then(|r| r.captured) {
        if let Some(node) = tree.find(&captured_id) {
            let routed = RoutedEvent::Pointer {
                kind: PointerKind::Move,
                sub: None,
                local: local_point(position, node.rect),
                mods: Modifiers::default(),
            };
            return dispatch_widget_routed_event(captured_id, node, &routed, backend, app)
                .reaction();
        }
    }
    route_widget_hover(position, backend, app, tree)
}

/// [`route_widget_move`]'s no-capture path: synthesize
/// [`PointerKind::Leave`]/[`PointerKind::Enter`] for whichever widgets'
/// hover state changed, then record the new hover target. The
/// [`Handled::Event`]/[`Handled::Consumed`] outcome from either synthetic
/// delivery is never routed further as a gesture — hover is a side effect
/// a real mouse move causes, not something the app itself handles — but
/// it is folded ([`Reaction::merge`]) into the `Reaction` this function
/// returns, so a hover highlight still repaints even though the move
/// itself is never claimed.
fn route_widget_hover<B, A>(
    position: Point,
    backend: &mut B,
    app: &mut A,
    tree: &FrameTree,
) -> Reaction
where
    B: PreprocessBackend + crate::Backend,
    A: AppLogic,
{
    let hit_id = tree.hit_test(position).map(|node| node.id);
    let Some(routing) = backend.widget_routing_mut() else {
        return Reaction::Continue;
    };
    let previous = routing.hovered;
    if previous == hit_id {
        return Reaction::Continue;
    }

    let mut reaction = Reaction::Continue;
    if let Some(prev_id) = previous {
        if let Some(prev_node) = tree.find(&prev_id) {
            let routed = RoutedEvent::Pointer {
                kind: PointerKind::Leave,
                sub: None,
                local: local_point(position, prev_node.rect),
                mods: Modifiers::default(),
            };
            reaction = reaction.merge(
                dispatch_widget_routed_event(prev_id, prev_node, &routed, backend, app).reaction(),
            );
        }
    }
    if let Some(new_id) = hit_id {
        if let Some(new_node) = tree.find(&new_id) {
            let routed = RoutedEvent::Pointer {
                kind: PointerKind::Enter,
                sub: None,
                local: local_point(position, new_node.rect),
                mods: Modifiers::default(),
            };
            reaction = reaction.merge(
                dispatch_widget_routed_event(new_id, new_node, &routed, backend, app).reaction(),
            );
        }
    }
    if let Some(routing) = backend.widget_routing_mut() {
        routing.hovered = hit_id;
    }
    reaction
}

/// `local` coordinates for a point relative to `rect`'s origin — what
/// [`RoutedEvent::Pointer`]'s `local` field carries, per that variant's
/// own doc.
fn local_point(position: Point, rect: crate::event::Rect) -> Point {
    Point::new(position.x - rect.x, position.y - rect.y)
}

/// Deferred side effects an [`EventHost`] impl records during one
/// [`crate::primitive::DynWidget::handle`] call, applied to `backend` only after that call
/// returns — see [`dispatch_widget_routed_event`]'s doc for why: both
/// `state` (borrowed from [`crate::widget_state::WidgetRoutingState`])
/// and an [`EventHost`] that could call back into `backend` directly
/// would need two live mutable borrows of the same backend at once,
/// which Rust's borrow checker (correctly) rejects. Recording intents
/// here instead and applying them afterward, sequentially, needs only one
/// borrow at a time.
#[derive(Default)]
struct WidgetHostIntents {
    focus_request: Option<crate::focus::FocusAddress>,
    capture_request: Option<Option<WidgetId>>,
    cursor_request: Option<PointerShape>,
    redraw: bool,
    emitted: Vec<UiEvent>,
}

/// [`EventHost`] impl backing one [`crate::primitive::DynWidget::handle`] call — see
/// [`WidgetHostIntents`] for why this holds no backend reference at all,
/// only a snapshot of what [`EventHost::is_focused`]/[`EventHost::units`]
/// need to answer and an outbox for everything else.
///
/// `currently_focused` is a [`crate::focus::FocusAddress`] — sourced from
/// [`crate::Backend::focus_tree`], which carries a composite widget's
/// own sub-target — so [`EventHost::is_focused`] can tell its
/// individually-addressed children apart instead of answering every
/// `sub` identically for one focused id.
struct WidgetEventHost<'a> {
    currently_focused: Option<crate::focus::FocusAddress>,
    units: Units,
    intents: &'a mut WidgetHostIntents,
}

impl EventHost for WidgetEventHost<'_> {
    fn is_focused(&self, id: &WidgetId, sub: Option<crate::interactive::SubTarget>) -> bool {
        self.currently_focused == Some((*id, sub))
    }

    fn request_focus(&mut self, id: &WidgetId, sub: Option<crate::interactive::SubTarget>) {
        self.intents.focus_request = Some((*id, sub));
    }

    fn capture_pointer(&mut self, id: &WidgetId) {
        self.intents.capture_request = Some(Some(*id));
    }

    fn release_pointer(&mut self) {
        self.intents.capture_request = Some(None);
    }

    fn set_cursor(&mut self, cursor: PointerShape) {
        self.intents.cursor_request = Some(cursor);
    }

    fn request_redraw(&mut self) {
        self.intents.redraw = true;
    }

    fn emit(&mut self, event: UiEvent) {
        self.intents.emitted.push(event);
    }

    fn units(&self) -> Units {
        self.units
    }
}

/// Call `node.widget`'s [`crate::primitive::DynWidget::handle`] against `routed`, applying
/// every [`EventHost`] side effect ([`EventHost::request_focus`],
/// [`EventHost::capture_pointer`]/`release_pointer`,
/// [`EventHost::set_cursor`], [`EventHost::emit`]) to `backend`/`app`
/// afterward, then translates the outcome:
///
/// - [`Handled::Event`] — the widget's action, already turned into a
///   [`UiEvent`] by [`crate::interactive::WidgetAction::into_ui_event`] —
///   is routed through `app.handle`, same as every other synthetic
///   `UiEvent` [`preprocess_event`]'s other steps dispatch. This is the
///   "bubbling" half of the contract: a widget that *does* produce a
///   semantic event hands it up to the app exactly the way a DOM event
///   bubbles from a target to its listeners.
/// - [`Handled::Consumed`] — the widget changed its own state with
///   nothing further to route; `Some(`[`EventOutcome::Redraw`]`)`.
/// - [`Handled::Ignored`] — "not mine";
///   [`WidgetRouteOutcome::Bubble`], so the event bubbles down to
///   `preprocess_event`'s remaining steps (and eventually `app.handle` with
///   the *original*, un-routed event) exactly as if this widget didn't
///   exist. Bubbling is *not* the same as producing no `Reaction`, though:
///   a widget can answer `Ignored` having already called
///   [`EventHost::emit`], and the merged `Reaction` from those emitted
///   events rides back as `Bubble`'s payload rather than being dropped —
///   otherwise an `Exit` or a short `RedrawAfter` deadline one of them
///   asked for would be silently swallowed by the very arm that means
///   "carry on as if I weren't here".
///
/// `state`/`layout` are resolved fresh for this one call:
/// [`crate::widget_state::WidgetStateStore::state_for`] for the former
/// (created via [`crate::primitive::DynWidget::new_state`] on first use, then persisted —
/// see that store's own doc), [`crate::primitive::DynWidget::layout`] against `node.rect`
/// for the latter (cheap to recompute; `Interactive`'s whole contract is
/// that `Self::Layout` is already the value [`crate::primitive::DynWidget::paint`] would
/// have used this frame).
fn dispatch_widget_routed_event<B, A>(
    id: WidgetId,
    node: &FrameTreeNode,
    routed: &RoutedEvent,
    backend: &mut B,
    app: &mut A,
) -> WidgetRouteOutcome
where
    B: PreprocessBackend + crate::Backend,
    A: AppLogic,
{
    let ctx = crate::primitive::LayoutCtx::new(
        backend.line_height(),
        backend.char_width(),
        backend.nerd_fonts_enabled(),
    );
    let layout = {
        // The same measurer
        // [`crate::backend::DrawsPrimitives::draw_primitive`] attaches, so
        // the layout this hit-test runs against is the one that was
        // painted — a widget that sizes text through
        // [`crate::primitive::LayoutCtx::text_width`] would otherwise
        // resolve different geometry here than on screen. Scoped so the
        // closure's shared borrow ends before the `&mut backend` calls
        // below.
        let measurer = |text: &str, role: crate::FontRole| backend.measure_text(text, role).0;
        node.widget.layout(&ctx.with_measurer(&measurer), node.rect)
    };
    let currently_focused = backend.focus_tree().focused().copied();
    let units = ctx.units();

    let mut intents = WidgetHostIntents::default();
    let outcome = {
        let Some(routing) = backend.widget_routing_mut() else {
            return WidgetRouteOutcome::Bubble(Reaction::Continue);
        };
        let state = routing.states.state_for(id, node.widget.as_ref());
        let mut host = WidgetEventHost {
            currently_focused,
            units,
            intents: &mut intents,
        };
        let mut cx = EventCx::new(id, &mut host);
        node.widget.handle(state, routed, &*layout, &mut cx)
    };

    if let Some((focus_id, sub)) = intents.focus_request {
        // Set the tree directly with the widget's own `sub` — letting a
        // composite widget request focus for one of its own children
        // precisely, which `Backend::set_focus`'s flat `WidgetId`
        // signature can't express.
        backend.focus_tree_mut().set_focus(Some((focus_id, sub)));
    }
    if let Some(capture) = intents.capture_request {
        if let Some(routing) = backend.widget_routing_mut() {
            routing.captured = capture;
        }
    }
    if let Some(cursor) = intents.cursor_request {
        backend.set_cursor(cursor);
    }
    // Every `Reaction` produced while applying this one routed event — one
    // per `EventHost::emit`ed extra `UiEvent`, plus whichever of
    // `Handled::Event`'s own dispatch or `Handled::Consumed`'s `Redraw`
    // baseline applies — is folded with `Reaction::merge`, never
    // last-write-wins: an `Exit` from an earlier emitted event must not be
    // overwritten by a later `Continue`, and `Handled::Consumed`'s `Redraw`
    // must survive a `Continue` from an emitted event the same way
    // `control/mod.rs`'s batch dispatch already folds multiple `Reaction`s
    // from one gesture.
    let mut extra_reaction = EventOutcome::Continue;
    for extra in intents.emitted {
        extra_reaction = extra_reaction.merge(app.handle(extra, backend));
    }

    // `Handled::Ignored` bubbles, but it carries `extra_reaction` with it
    // instead of discarding it: "this widget didn't claim the event" and
    // "nothing happened while it decided that" are different statements,
    // and only the first one is what `Ignored` means. Whatever the emitted
    // events produced is merged into `preprocess_event`'s final `Reaction`
    // via `WidgetRouteOutcome::Bubble` — the same channel a hover
    // Enter/Leave repaint already travels back on — so the original,
    // un-routed event still reaches every later step and `app.handle`.
    let result = match outcome {
        Handled::Event(ui_event) => {
            WidgetRouteOutcome::Claimed(extra_reaction.merge(app.handle(ui_event, backend)))
        }
        Handled::Consumed => {
            WidgetRouteOutcome::Claimed(extra_reaction.merge(EventOutcome::Redraw))
        }
        Handled::Ignored => WidgetRouteOutcome::Bubble(extra_reaction),
    };

    // `EventHost::request_redraw` is a request made *during* `handle`,
    // independent of which of the three outcomes above it ended up
    // returning — force at least a `Redraw` the same way `preprocess_event`'s
    // own `force_redraw` does for `TextSelectionChanged`/`DpiChanged`,
    // but only when this step actually claimed the event: a bubbled
    // `Ignored` must still mean "carry on as if this step didn't run", and
    // upgrading its payload would repaint on a widget's behalf for an
    // event it explicitly declined.
    match result {
        WidgetRouteOutcome::Claimed(outcome) if intents.redraw => {
            WidgetRouteOutcome::Claimed(match outcome {
                EventOutcome::Continue | EventOutcome::RedrawAfter(_) => EventOutcome::Redraw,
                other => other,
            })
        }
        other => other,
    }
}

/// [`AppLogic::extra_tab_stops`], defensively deduped by id — the one tab
/// order every caller in this module cycles/scans.
#[cfg(any(
    feature = "tui",
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
fn all_tab_stops<A: AppLogic>(app: &A, area: A::AreaId) -> Vec<(WidgetId, crate::event::Rect)> {
    let mut stops = app.extra_tab_stops(area);
    // A duplicate id would otherwise land twice in the cycled order,
    // shortening the effective cycle by one and stranding a later stop
    // unreachable — `FocusTree::step` resolves the *current* position by
    // its first match. Keep the first occurrence and drop any later
    // duplicate.
    let mut seen = std::collections::HashSet::new();
    stops.retain(|(id, _)| seen.insert(*id));
    stops
}

/// The screen [`Rect`] of whichever tab stop currently has focus, or
/// `None` — the per-frame lookup every runner (`gtk::run`, `macos::run`,
/// `win::run`, `tui::run`) performs just before painting the focus-ring
/// convention: `backend.focus_tree().focused()` for the focused id, then
/// a linear scan of [`all_tab_stops`] for the matching rect.
///
/// Returns `None` both when nothing is focused and when the focused id
/// isn't among this frame's tab stops (e.g. it scrolled out of view) —
/// callers treat both the same way: skip `Backend::draw_focus_ring` for
/// this frame.
#[cfg(any(
    feature = "tui",
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
pub(crate) fn focused_stop_rect<B, A>(
    backend: &B,
    app: &A,
    area: A::AreaId,
) -> Option<crate::event::Rect>
where
    B: crate::Backend,
    A: AppLogic,
{
    backend
        .focus_tree()
        .focused()
        .map(|(id, _)| *id)
        .and_then(|id| {
            all_tab_stops(app, area)
                .into_iter()
                .find(|(stop_id, _)| *stop_id == id)
                .map(|(_, rect)| rect)
        })
}

/// Trailing-edge debounce settle window for `WindowResized` dispatch.
///
/// A live terminal/window edge-drag delivers a burst of resize
/// notifications (tens per second). Apps with PTY-backed side effects
/// (`TerminalApp::handle` → `TerminalSession::resize` → SIGWINCH) were
/// resizing the child shell on *every* intermediate size. A shell's
/// line-editor (readline/zle) redraws its prompt for the width it was
/// SIGWINCH'd with; if the grid is reflowed to a *different* width
/// before that redraw is parsed, the cursor-relative bytes land in the
/// wrong columns and scatter duplicated prompt fragments that stick
/// until the next resize.
///
/// All four runners coalesce the burst and dispatch a single
/// `WindowResized` at the final settled size once no new resize has
/// arrived for this interval. Painting stays live throughout in all —
/// each runner's frame/draw callback re-reads the real surface size
/// every frame regardless of whether the debounced event has fired yet.
#[cfg(any(
    feature = "tui",
    feature = "gtk",
    all(feature = "win", target_os = "windows"),
    all(feature = "macos", target_os = "macos")
))]
pub(crate) const RESIZE_SETTLE: Duration = Duration::from_millis(120);

/// Fallback idle-poll bound for backends that keep a coarse "call `tick`
/// even with nothing scheduled" cadence — TUI, GTK, macOS, and Windows.
///
/// TUI and GTK used to poll unconditionally (TUI every 16ms and GTK every
/// 33ms) — burning CPU waking a fully idle app 60–30 times a second.
/// [`crate::runner::Reaction::RedrawAfter`] +
/// [`crate::backend::Backend::request_frame_in`] replace that for any app
/// that opts in with a *precise* scheduled wake, but this constant stays
/// as the ceiling for apps that don't: an idle-poll fallback is still
/// needed because at least one existing behavior relies on `tick` being
/// called periodically with no explicit request — `examples/common/
/// terminal_app.rs`'s embedded terminal notices new PTY output by
/// polling from `tick`, and that background reader thread has no
/// `Backend::waker` wired to it. 250ms is a deliberate, order-of-magnitude
/// reduction from both old constants (6x fewer wakes than TUI's 16ms, 7x
/// fewer than GTK's 33ms) while keeping that fallback path — and
/// `Backend::waker`'s own latency on TUI specifically, which has no
/// native way to interrupt a blocked `crossterm::event::poll` early, see
/// that method's doc — bounded to a quarter second instead of unbounded.
///
/// **macOS and Windows** have no other cadence of their own: on both,
/// `AppLogic::tick` otherwise only ever runs off a
/// [`crate::backend::Backend::request_frame_in`] deadline or a native
/// event, so a host that queues deferred work (a file dialog request, a
/// dirty-frame flag) with no further input would have it stranded
/// forever without this fallback. Both platforms install a repeating
/// timer at this same ceiling to ensure periodic `tick` calls.
#[cfg(any(
    feature = "tui",
    feature = "gtk",
    all(feature = "macos", target_os = "macos"),
    all(feature = "win", target_os = "windows")
))]
pub(crate) const IDLE_POLL_CEILING: Duration = Duration::from_millis(250);

/// Trailing-edge resize-event coalescing: stores the most recent
/// viewport from a burst of resize events, superseding any earlier one,
/// until the caller decides the burst has settled and takes it.
///
/// Mechanism-agnostic by design — this struct only coalesces the
/// *value*, not the *timing*. Each backend owns how it decides "has this
/// settled": TUI polls an `Instant` deadline once per loop iteration;
/// macOS cancels and reschedules a one-shot `NSTimer` targeting the view
/// itself per resize; Windows relies on `SetTimer` replacing an
/// already-armed timer of the same id rather than stacking a new one, so
/// a live drag keeps rescheduling the same `WM_TIMER`. GTK cancels and
/// reschedules a `glib::SourceId` timer per resize and doesn't need this
/// struct at all, since GLib's timer already gives it exactly-once-after-
/// settle semantics and it re-reads the DA's live size at fire time
/// rather than storing a pending value.
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

/// TUI's [`Backend::request_frame_in`] bookkeeping.
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
///
/// Also used by [`crate::win::backend::WinBackend`] —
/// not because Windows lacks a native timer (it has one, `SetTimer`/
/// `WM_TIMER`, armed directly from `WinBackend::request_frame_in`, same
/// as GTK/macOS), but because that real timer is a no-op before
/// `attach_surface` has ever run (no `HWND` to arm it against — see
/// that method's doc), which makes it unobservable from a headless
/// `WinDriver` test that never attaches a live window. `WinBackend`
/// records into this scheduler unconditionally, `hwnd` or not, purely
/// as a test-facing call-count/deadline accessor
/// (`WinBackend::frame_requests`/`pending_frame_delay`) mirroring
/// `TuiBackend`'s — the live Win32 timer arm is untouched and still
/// gated on `hwnd` being `Some`.
#[cfg(any(feature = "tui", feature = "win"))]
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

#[cfg(any(feature = "tui", feature = "win"))]
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
}

// TUI is the only backend that needs these two: it has no native run
// loop to arm a timer against (see this type's module doc), so its live
// runner has to fold the pending deadline into its own `wait_events`
// timeout (`poll_timeout`) and clear it itself once due
// (`clear_if_due`). GTK/macOS/Windows arm a real native timer directly
// from `request_frame_in` instead and never call either — under a
// `win`-only build (no `tui`) these would be unused and trip `dead_code`
// under this crate's workflow-wide `-D warnings`.
#[cfg(feature = "tui")]
impl FrameScheduler {
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

/// Thread-safe inbox for [`UiEvent::User`] payloads.
///
/// Every backend's own event queue — `GtkBackend::events` /
/// `MacBackend::events` / `WinBackend::events`, all
/// `Rc<RefCell<VecDeque<UiEvent>>>` — is deliberately `!Send`: nothing
/// outside the owning thread can touch it. `UserEventQueue` is the
/// `Send + Sync` staging area a background thread *can* touch. Each
/// backend owns one `Arc<Self>`, clones it into the closure
/// [`crate::Backend::waker`] hands out, and drains it back into
/// `UiEvent::User` on the owning thread — the same place it already
/// drains its native event source (TUI's `poll_events`/`wait_events`;
/// GTK/macOS/Windows' live dispatch path).
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
/// An earlier approach did have `MainThreadBound` here — a bare
/// `unsafe impl Send/Sync` over an `Rc`. It was removed rather than
/// repaired: with the derived `Drop` it raced `Rc`'s non-atomic refcount
/// whenever the last handle died on a background thread, and forwarding
/// that drop back through `MainContext::invoke` traded the race for a
/// hang any time the GTK main loop wasn't running to dispatch the
/// forwarded drop. Don't reintroduce it; keep `!Send` state on the thread
/// that owns it.
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

    /// A payload pushed from a background thread must be observable on
    /// another thread via `drain_into` — this is the plumbing every
    /// backend's `waker()` closure relies on.
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

    /// `RedrawAfter` must reach `request_frame_in`, not
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

    /// A test that drives a burst of resize events and asserts the
    /// dispatched count collapses. Simulates exactly what every native-timer-
    /// driven caller (macOS's `viewFrameDidChange:`, Windows' `WM_SIZE`) does
    /// per event — `note()`, never `take()` — for a burst of intermediate
    /// sizes a live drag would deliver, then fires the settle timer once, the
    /// way `resizeDebounceFired:`/`WM_TIMER` do. However large the burst,
    /// exactly one `WindowResized` worth of dispatch data (the final size)
    /// survives to be taken — the rest never reach `AppLogic::handle` at all,
    /// which is the collapse this debouncer exists to guarantee.
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

    /// With nothing scheduled, the number of wake-ups over a fixed interval
    /// is bounded by the (much coarser) idle ceiling, not the unconditional
    /// 16ms poll: this verifies the tighter wake bound against the idle
    /// polling ceiling.
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

/// [`EventOutcome::merge`] — the fix for a review finding: every
/// batch-dispatch loop across all backends used to keep the *first*
/// non-`Continue` outcome seen in a batch rather than the *earliest*
/// `RedrawAfter` deadline, which could silently drop a shorter, more
/// urgent wake requested by a later event in the same batch. These tests
/// pin the corrected, order-independent semantics directly, since the
/// batches that would exercise this in a live runner (multiple synthetic
/// events from one drag gesture, each separately requesting a different
/// `RedrawAfter`) are awkward to synthesize end-to-end.
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

/// End-to-end proof of the widget-contract routing engine (step 2 of
/// [`preprocess_event`]) against a real [`crate::tui::TuiBackend`] — the
/// "TUI-proven" half of this module's widget routing. Exercises the exact
/// scenarios the engine exists for: click→focus, key→state-change→
/// semantic event, hover→Enter/Leave, a modal blocking all of the above,
/// and [`crate::a11y::route_widget_action`] producing the identical
/// semantic event a direct keystroke would.
#[cfg(all(test, feature = "tui"))]
mod widget_routing_tests {
    use super::*;
    use crate::a11y::{A11yInfo, A11yPrimitive};
    use crate::event::Rect;
    use crate::interactive::{Interactive, WidgetAction};
    use crate::primitive::{LayoutCtx, Primitive};
    use crate::runner::Reaction;
    use crate::tui::TuiBackend;
    use crate::Backend;

    const COUNTER_RECT: Rect = Rect::new(0.0, 0.0, 10.0, 3.0);

    /// Mirrors `crate::interactive`'s own `Counter` test fixture, extended
    /// with a `hovered` flag so [`PointerKind::Enter`]/[`PointerKind::Leave`]
    /// are independently observable from `count`.
    #[derive(Debug)]
    struct Counter {
        id: WidgetId,
    }

    #[derive(Debug, Default, Clone, PartialEq)]
    struct CounterState {
        count: i32,
        hovered: bool,
    }

    #[derive(Debug, Clone, Copy, PartialEq)]
    struct Activated;

    impl WidgetAction for Activated {
        fn into_ui_event(self, id: WidgetId) -> UiEvent {
            UiEvent::DoubleClick {
                widget: Some(id),
                position: Point::default(),
            }
        }
    }

    impl A11yPrimitive for Counter {
        fn a11y_info(&self) -> A11yInfo {
            A11yInfo::default()
        }
    }

    impl Primitive for Counter {
        type Layout = Rect;
        type Event = ();

        fn id(&self) -> Option<&WidgetId> {
            Some(&self.id)
        }

        fn layout(&self, _ctx: &LayoutCtx, rect: Rect) -> Rect {
            rect
        }

        fn paint(
            &self,
            _layout: &Rect,
            _surface: &mut dyn crate::paint_surface::PaintSurface,
            _theme: &crate::theme::Theme,
            _style: &crate::style::Style,
        ) {
        }
    }

    impl Interactive for Counter {
        type State = CounterState;
        type Action = Activated;

        fn focus(&self) -> FocusSpec {
            FocusSpec::Stop
        }

        fn handle(
            &self,
            state: &mut CounterState,
            event: &RoutedEvent,
            _layout: &Rect,
            cx: &mut EventCx,
        ) -> Handled<Activated> {
            use crate::event::NamedKey;
            match event {
                // Shift-click: emits an extra `UiEvent` the test app maps
                // to `Reaction::Continue`, then answers `Consumed` — the
                // `dispatch_widget_routed_event` merge-not-last-write-wins
                // regression fixture (see
                // `consumed_merges_with_an_emitted_events_reaction_not_last_write_wins`).
                RoutedEvent::Pointer {
                    kind: PointerKind::Down(_),
                    mods,
                    ..
                } if mods.shift => {
                    cx.emit(UiEvent::MouseMoved {
                        position: Point::default(),
                        buttons: Default::default(),
                    });
                    Handled::Consumed
                }
                RoutedEvent::Pointer {
                    kind: PointerKind::Down(_),
                    ..
                } => {
                    state.count += 1;
                    cx.request_redraw();
                    Handled::Consumed
                }
                RoutedEvent::Pointer {
                    kind: PointerKind::DoubleClick(_),
                    ..
                } => Handled::Event(Activated),
                RoutedEvent::Pointer {
                    kind: PointerKind::Enter,
                    ..
                } => {
                    state.hovered = true;
                    Handled::Consumed
                }
                RoutedEvent::Pointer {
                    kind: PointerKind::Leave,
                    ..
                } => {
                    state.hovered = false;
                    Handled::Consumed
                }
                RoutedEvent::Key(KeyEvent {
                    key: Key::Named(NamedKey::Up),
                    ..
                }) => {
                    state.count += 1;
                    Handled::Consumed
                }
                RoutedEvent::Key(KeyEvent {
                    key: Key::Named(NamedKey::Down),
                    ..
                }) => {
                    state.count -= 1;
                    Handled::Consumed
                }
                RoutedEvent::Key(KeyEvent {
                    key: Key::Named(NamedKey::Enter),
                    ..
                }) => Handled::Event(Activated),
                RoutedEvent::Action(crate::a11y::A11yAction::Click) => Handled::Event(Activated),
                // The three arms below drive `EventHost::emit`'s reaction
                // merging through the `Handled::Ignored` and
                // `Handled::Event` arms: each emits one or two marker
                // `UiEvent::TextCopied`s, which `CounterApp::handle`
                // decodes into a specific `Reaction`, so the merge is
                // observable from what widget routing hands back rather
                // than only from a `Reaction::merge` unit test.
                RoutedEvent::Key(KeyEvent {
                    key: Key::Named(NamedKey::Escape),
                    ..
                }) => {
                    cx.emit(UiEvent::TextCopied("exit-marker".into()));
                    Handled::Ignored
                }
                RoutedEvent::Key(KeyEvent {
                    key: Key::Char('e'),
                    ..
                }) => {
                    cx.emit(UiEvent::TextCopied("exit-marker".into()));
                    Handled::Event(Activated)
                }
                RoutedEvent::Key(KeyEvent {
                    key: Key::Char('b'),
                    ..
                }) => {
                    cx.emit(UiEvent::TextCopied("redraw-after:100".into()));
                    cx.emit(UiEvent::TextCopied("redraw-after:5".into()));
                    Handled::Ignored
                }
                _ => Handled::Ignored,
            }
        }
    }

    fn counter_tree(id: WidgetId) -> FrameTree {
        let mut tree = FrameTree::new();
        tree.push(id, COUNTER_RECT, Box::new(Counter { id }));
        tree
    }

    /// An [`AppLogic`] whose only widget is one [`Counter`], and which
    /// records every [`UiEvent`] `handle` receives — the "did the semantic
    /// event actually bubble to the app" half of each assertion below.
    struct CounterApp {
        id: WidgetId,
        received: Vec<UiEvent>,
    }

    impl AppLogic for CounterApp {
        type AreaId = ();

        fn render(&self, _backend: &mut dyn crate::Backend, _area: ()) {}

        fn handle(&mut self, event: UiEvent, _backend: &mut dyn crate::Backend) -> Reaction {
            // `MouseMoved` answers `Continue` on purpose — the widget
            // routing engine's hover-redraw merge test below relies on
            // this being the *only* source of `Reaction::Continue` in the
            // batch, so a `Redraw` observed for that event can only have
            // come from the hover Enter/Leave dispatch's own outcome.
            //
            // `TextCopied` carries the markers `Counter::handle`'s
            // `cx.emit` arms produce, decoded here into the specific
            // `Reaction` each test wants that emitted event to contribute
            // to the merge.
            let reaction = match &event {
                UiEvent::MouseMoved { .. } => Reaction::Continue,
                UiEvent::TextCopied(text) if text == "exit-marker" => Reaction::Exit,
                UiEvent::TextCopied(text) => text
                    .strip_prefix("redraw-after:")
                    .and_then(|ms| ms.parse::<u64>().ok())
                    .map_or(Reaction::Redraw, |ms| {
                        Reaction::RedrawAfter(Duration::from_millis(ms))
                    }),
                _ => Reaction::Redraw,
            };
            self.received.push(event);
            reaction
        }

        fn frame_tree(&self, _area: ()) -> FrameTree {
            counter_tree(self.id)
        }
    }

    fn harness() -> (TuiBackend, CounterApp) {
        let id = WidgetId::new("counter");
        (
            TuiBackend::new(),
            CounterApp {
                id,
                received: Vec::new(),
            },
        )
    }

    fn mouse_down_at(x: f32, y: f32) -> UiEvent {
        UiEvent::MouseDown {
            widget: None,
            button: MouseButton::Left,
            position: Point::new(x, y),
            modifiers: Modifiers::default(),
        }
    }

    fn double_click_at(x: f32, y: f32) -> UiEvent {
        UiEvent::DoubleClick {
            widget: None,
            position: Point::new(x, y),
        }
    }

    fn key(named: crate::event::NamedKey) -> UiEvent {
        UiEvent::KeyPressed {
            key: Key::Named(named),
            modifiers: Modifiers::default(),
            repeat: false,
        }
    }

    fn char_key(c: char) -> UiEvent {
        UiEvent::KeyPressed {
            key: Key::Char(c),
            modifiers: Modifiers::default(),
            repeat: false,
        }
    }

    /// Click → focus: a `MouseDown` landing inside the counter's rect
    /// moves keyboard focus to it, with no app code at all.
    #[test]
    fn click_moves_focus_to_the_hit_widget() {
        let (mut backend, mut app) = harness();
        assert_eq!(backend.focus_tree().focused(), None);

        preprocess_event(mouse_down_at(1.0, 1.0), &mut backend, &mut app);

        assert_eq!(backend.focus_tree().focused(), Some(&(app.id, None)));
    }

    /// `UiEvent::DoubleClick` reaches a hit widget as
    /// [`PointerKind::DoubleClick`] — the gap the widget-contract adversarial
    /// review flagged: before `route_widget_event` grew this arm, a
    /// double-click never reached `Interactive::handle` at all, so
    /// `PointerKind::DoubleClick(_)` was dead on every `Surface::Widget`.
    #[test]
    fn double_click_reaches_the_hit_widget_as_pointer_double_click() {
        let (mut backend, mut app) = harness();

        let outcome = preprocess_event(double_click_at(1.0, 1.0), &mut backend, &mut app);

        assert_eq!(outcome, Reaction::Redraw);
        assert_eq!(
            app.received,
            vec![UiEvent::DoubleClick {
                widget: Some(app.id),
                position: Point::default(),
            }]
        );
    }

    /// A double-click landing outside every widget's bounds never reaches
    /// `Counter::handle` — widget routing bubbles it (`WidgetRouteOutcome::Bubble`)
    /// rather than claiming it, so the original, unclaimed `UiEvent` still
    /// reaches `app.handle` through `preprocess_event`'s later steps, the
    /// same as any other event no widget wanted.
    #[test]
    fn double_click_outside_any_widget_bubbles() {
        let (mut backend, mut app) = harness();

        preprocess_event(double_click_at(100.0, 100.0), &mut backend, &mut app);

        assert_eq!(app.received, vec![double_click_at(100.0, 100.0)]);
    }

    /// Key → state-change → semantic event: `Up` mutates the counter's
    /// persisted state (observable across calls, proving
    /// `WidgetStateStore` — not the rebuilt `Counter` descriptor — is what
    /// survives), and a focused `Enter` bubbles `Activated`'s `UiEvent` to
    /// `app.handle`.
    #[test]
    fn key_changes_state_and_enter_bubbles_a_semantic_event() {
        let (mut backend, mut app) = harness();
        backend.set_focus(Some(app.id));

        let outcome = preprocess_event(key(crate::event::NamedKey::Up), &mut backend, &mut app);
        assert_eq!(outcome, Reaction::Redraw);

        let count = {
            let routing = backend.widget_routing_mut().unwrap();
            let state = routing
                .states
                .state_for(app.id, &Counter { id: app.id })
                .downcast_mut::<CounterState>()
                .unwrap();
            state.count
        };
        assert_eq!(count, 1);

        preprocess_event(key(crate::event::NamedKey::Enter), &mut backend, &mut app);
        assert_eq!(
            app.received,
            vec![UiEvent::DoubleClick {
                widget: Some(app.id),
                position: Point::default(),
            }]
        );
    }

    /// Hover → Enter/Leave: moving the pointer into, then out of, the
    /// counter's rect flips `CounterState::hovered` both ways, and the
    /// `MouseMoved` event itself still bubbles to `app.handle` afterward
    /// (hover tracking is a side effect, not a consumption).
    #[test]
    fn hover_synthesizes_enter_then_leave() {
        let (mut backend, mut app) = harness();

        preprocess_event(
            UiEvent::MouseMoved {
                position: Point::new(1.0, 1.0),
                buttons: Default::default(),
            },
            &mut backend,
            &mut app,
        );
        let hovered = |backend: &mut TuiBackend, app: &CounterApp| {
            backend
                .widget_routing_mut()
                .unwrap()
                .states
                .state_for(app.id, &Counter { id: app.id })
                .downcast_mut::<CounterState>()
                .unwrap()
                .hovered
        };
        assert!(hovered(&mut backend, &app));
        assert!(app
            .received
            .iter()
            .any(|e| matches!(e, UiEvent::MouseMoved { .. })));

        preprocess_event(
            UiEvent::MouseMoved {
                position: Point::new(100.0, 100.0),
                buttons: Default::default(),
            },
            &mut backend,
            &mut app,
        );
        assert!(!hovered(&mut backend, &app));
    }

    /// The hover Enter's `Handled::Consumed` (→ `Redraw`) is folded into
    /// `preprocess_event`'s final `Reaction` via `Reaction::merge`, even
    /// though step 2 never claims `MouseMoved` itself (it always bubbles)
    /// and `CounterApp::handle` answers `Reaction::Continue` for this
    /// event — so the only way the result can be `Redraw` is the merge,
    /// not last-write-wins.
    #[test]
    fn hover_redraw_merges_into_the_bubbled_mousemoved_outcome() {
        let (mut backend, mut app) = harness();

        let outcome = preprocess_event(
            UiEvent::MouseMoved {
                position: Point::new(1.0, 1.0),
                buttons: Default::default(),
            },
            &mut backend,
            &mut app,
        );

        assert_eq!(outcome, Reaction::Redraw);
    }

    /// `dispatch_widget_routed_event` folds `Handled::Consumed`'s `Redraw`
    /// baseline with the `Reaction` an emitted `UiEvent` produced via
    /// `Reaction::merge`, not last-write-wins: a shift-click emits one
    /// extra event that `CounterApp::handle` answers with `Continue`, and
    /// the widget itself answers `Consumed` — the merged result must stay
    /// `Redraw`, never be overwritten down to `Continue`.
    #[test]
    fn consumed_merges_with_an_emitted_events_reaction_not_last_write_wins() {
        let (mut backend, mut app) = harness();

        let outcome = preprocess_event(
            UiEvent::MouseDown {
                widget: None,
                button: MouseButton::Left,
                position: Point::new(1.0, 1.0),
                modifiers: Modifiers {
                    shift: true,
                    ..Default::default()
                },
            },
            &mut backend,
            &mut app,
        );

        assert_eq!(outcome, Reaction::Redraw);
    }

    /// `Handled::Ignored` means "bubble", not "nothing happened": the
    /// widget declines the `Escape` — so the raw key still reaches
    /// `app.handle` after the marker event it emitted, exactly as if this
    /// step hadn't run — while the `Reaction::Exit` that marker's handler
    /// returned still reaches `preprocess_event`'s result instead of being
    /// dropped by the declining arm. `CounterApp::handle` answers the
    /// bubbled key with `Reaction::Redraw`, so only the merge can make the
    /// final outcome `Exit`.
    #[test]
    fn ignored_bubbles_the_event_and_still_surfaces_an_emitted_exit() {
        let (mut backend, mut app) = harness();
        backend.set_focus(Some(app.id));

        let outcome = preprocess_event(key(crate::event::NamedKey::Escape), &mut backend, &mut app);

        assert_eq!(outcome, Reaction::Exit);
        assert_eq!(
            app.received,
            vec![
                UiEvent::TextCopied("exit-marker".into()),
                key(crate::event::NamedKey::Escape),
            ]
        );
    }

    /// `Handled::Event` merges the emitted reaction into the widget's own
    /// bubbled action rather than replacing it: the emitted marker asks to
    /// exit while the `Activated` action (bubbled as `UiEvent::DoubleClick`)
    /// would only have produced `Reaction::Redraw` — `Exit` must win.
    #[test]
    fn event_arm_merges_with_an_emitted_exit_instead_of_dropping_it() {
        let (mut backend, mut app) = harness();
        backend.set_focus(Some(app.id));

        let outcome = preprocess_event(char_key('e'), &mut backend, &mut app);

        assert_eq!(outcome, Reaction::Exit);
        assert_eq!(
            app.received,
            vec![
                UiEvent::TextCopied("exit-marker".into()),
                UiEvent::DoubleClick {
                    widget: Some(app.id),
                    position: Point::default(),
                },
            ]
        );
    }

    /// A batch of emitted events folds with [`Reaction::merge`], not "last
    /// one wins": the widget emits a 100ms `RedrawAfter` request and then a
    /// 5ms one before answering `Handled::Ignored`, and the *earlier*
    /// deadline has to survive both the fold and the declining arm.
    ///
    /// Asserted on `route_widget_event`'s own `WidgetRouteOutcome` rather
    /// than `preprocess_event`'s final `Reaction` because the two halves of
    /// the contract are only separable here: the outcome must be `Bubble`
    /// (so the key goes on to reach `app.handle`, whose `Redraw` for it
    /// would otherwise mask any `RedrawAfter` in the end result) and its
    /// payload must be the merged 5ms deadline.
    #[test]
    fn a_batch_of_emitted_events_bubbles_with_the_earliest_deadline() {
        let (mut backend, mut app) = harness();
        backend.set_focus(Some(app.id));

        match route_widget_event(&char_key('b'), &mut backend, &mut app) {
            WidgetRouteOutcome::Bubble(reaction) => {
                assert_eq!(reaction, Reaction::RedrawAfter(Duration::from_millis(5)));
            }
            WidgetRouteOutcome::Claimed(outcome) => {
                panic!("Handled::Ignored must bubble, not claim; got Claimed({outcome:?})")
            }
        }
        assert_eq!(
            app.received,
            vec![
                UiEvent::TextCopied("redraw-after:100".into()),
                UiEvent::TextCopied("redraw-after:5".into()),
            ]
        );
    }

    /// A minimal widget that captures the pointer on `Down` and releases
    /// it on `Up` — isolated from `Counter`'s unrelated arms so
    /// [`route_widget_pointer_up`]'s own effect (capture release) is the
    /// only thing under test.
    #[derive(Debug)]
    struct CaptureWidget {
        id: WidgetId,
    }

    #[derive(Debug, Default, Clone, Copy, PartialEq)]
    struct CaptureState {
        /// How many `PointerKind::Move`s this widget saw while it held
        /// the capture — zero once a `MouseUp` has released it, proving
        /// a later move fell through to plain hover tracking instead.
        moves_while_captured: u32,
    }

    impl A11yPrimitive for CaptureWidget {
        fn a11y_info(&self) -> A11yInfo {
            A11yInfo::default()
        }
    }

    impl Primitive for CaptureWidget {
        type Layout = Rect;
        type Event = ();

        fn id(&self) -> Option<&WidgetId> {
            Some(&self.id)
        }

        fn layout(&self, _ctx: &LayoutCtx, rect: Rect) -> Rect {
            rect
        }

        fn paint(
            &self,
            _layout: &Rect,
            _surface: &mut dyn crate::paint_surface::PaintSurface,
            _theme: &crate::theme::Theme,
            _style: &crate::style::Style,
        ) {
        }
    }

    impl Interactive for CaptureWidget {
        type State = CaptureState;
        type Action = Activated;

        fn focus(&self) -> FocusSpec {
            FocusSpec::Stop
        }

        fn handle(
            &self,
            state: &mut CaptureState,
            event: &RoutedEvent,
            _layout: &Rect,
            cx: &mut EventCx,
        ) -> Handled<Activated> {
            match event {
                RoutedEvent::Pointer {
                    kind: PointerKind::Down(_),
                    ..
                } => {
                    cx.capture_pointer();
                    Handled::Consumed
                }
                RoutedEvent::Pointer {
                    kind: PointerKind::Move,
                    ..
                } => {
                    state.moves_while_captured += 1;
                    Handled::Consumed
                }
                RoutedEvent::Pointer {
                    kind: PointerKind::Up(_),
                    ..
                } => {
                    cx.release_pointer();
                    Handled::Consumed
                }
                _ => Handled::Ignored,
            }
        }
    }

    fn capture_tree(id: WidgetId) -> FrameTree {
        let mut tree = FrameTree::new();
        tree.push(id, COUNTER_RECT, Box::new(CaptureWidget { id }));
        tree
    }

    struct CaptureApp {
        id: WidgetId,
    }

    impl AppLogic for CaptureApp {
        type AreaId = ();

        fn render(&self, _backend: &mut dyn crate::Backend, _area: ()) {}

        fn handle(&mut self, _event: UiEvent, _backend: &mut dyn crate::Backend) -> Reaction {
            Reaction::Redraw
        }

        fn frame_tree(&self, _area: ()) -> FrameTree {
            capture_tree(self.id)
        }
    }

    /// The real-runtime regression this module's `MouseUp` arm fixes: a
    /// left press captures the pointer, and only `MouseUp` routing (not
    /// the `PointerKind::Up` a hand-built `Interactive::handle` call could
    /// fabricate) can run the widget's own `EventHost::release_pointer`.
    /// Without it, a later `MouseMoved` keeps routing to the captured
    /// widget forever instead of falling through to plain hover tracking.
    #[test]
    fn mouse_up_releases_the_capture_so_a_later_move_is_not_routed_to_it() {
        let id = WidgetId::new("capture-widget");
        let mut backend = TuiBackend::new();
        let mut app = CaptureApp { id };

        preprocess_event(mouse_down_at(1.0, 1.0), &mut backend, &mut app);
        assert_eq!(
            backend.widget_routing_mut().unwrap().captured,
            Some(id),
            "a left press must capture the pointer"
        );

        preprocess_event(
            UiEvent::MouseUp {
                widget: None,
                button: MouseButton::Left,
                position: Point::new(1.0, 1.0),
            },
            &mut backend,
            &mut app,
        );
        assert_eq!(
            backend.widget_routing_mut().unwrap().captured,
            None,
            "MouseUp must route to the captured widget so its own release_pointer call takes effect"
        );

        preprocess_event(
            UiEvent::MouseMoved {
                position: Point::new(1.0, 1.0),
                buttons: Default::default(),
            },
            &mut backend,
            &mut app,
        );
        let moves = backend
            .widget_routing_mut()
            .unwrap()
            .states
            .state_for(id, &CaptureWidget { id })
            .downcast_mut::<CaptureState>()
            .unwrap()
            .moves_while_captured;
        assert_eq!(
            moves, 0,
            "a move after release must never reach the widget as PointerKind::Move again"
        );
    }

    /// Modal scope blocks it: with a modal pushed, a click that would
    /// otherwise focus the counter does nothing — the widget never sees
    /// the event.
    #[test]
    fn open_modal_blocks_widget_routing_entirely() {
        let (mut backend, mut app) = harness();
        backend.modal_stack_handle().borrow_mut().push(
            WidgetId::new("some-modal"),
            Rect::new(50.0, 50.0, 10.0, 10.0),
        );

        preprocess_event(mouse_down_at(1.0, 1.0), &mut backend, &mut app);

        assert_eq!(backend.focus_tree().focused(), None);
    }

    /// `route_widget_action` produces the identical semantic event a
    /// direct keystroke would: an `A11yAction::Click` through the a11y
    /// path and a direct `Enter` keypress through `preprocess_event` both
    /// end up calling the same `Counter::handle` arm, producing the same
    /// `UiEvent`.
    #[test]
    fn a11y_route_widget_action_matches_direct_input() {
        let id = WidgetId::new("counter:a11y");
        let tree = counter_tree(id);
        let mut states = crate::widget_state::WidgetStateStore::new();
        let ctx = LayoutCtx::new(1.0, 1.0, false);

        let via_a11y = crate::a11y::route_widget_action(
            &tree,
            &mut states,
            &ctx,
            id,
            crate::a11y::A11yAction::Click,
        );

        let (mut backend, mut app) = harness();
        app.id = id;
        backend.set_focus(Some(id));
        preprocess_event(key(crate::event::NamedKey::Enter), &mut backend, &mut app);

        assert_eq!(via_a11y, app.received);
    }

    /// An [`AppLogic`] with independently configurable
    /// [`AppLogic::extra_tab_stops`]/[`AppLogic::frame_tree`] — the
    /// step-4 Tab/Shift+Tab-cycling counterpart to [`CounterApp`], which
    /// never overrides either.
    struct TabCycleApp {
        id: WidgetId,
        received: Vec<UiEvent>,
        extra_tab_stops: Vec<(WidgetId, Rect)>,
        frame_tree: bool,
    }

    impl AppLogic for TabCycleApp {
        type AreaId = ();

        fn render(&self, _backend: &mut dyn crate::Backend, _area: ()) {}

        fn handle(&mut self, event: UiEvent, _backend: &mut dyn crate::Backend) -> Reaction {
            self.received.push(event);
            Reaction::Redraw
        }

        fn extra_tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
            self.extra_tab_stops.clone()
        }

        fn frame_tree(&self, _area: ()) -> FrameTree {
            if self.frame_tree {
                counter_tree(self.id)
            } else {
                FrameTree::new()
            }
        }
    }

    /// `AppLogic::extra_tab_stops` drives step 4's Tab cycling.
    #[test]
    fn extra_tab_stops_drives_tab_cycling() {
        let id = WidgetId::new("extra-only");
        let mut backend = TuiBackend::new();
        let mut app = TabCycleApp {
            id,
            received: Vec::new(),
            extra_tab_stops: vec![(id, COUNTER_RECT)],
            frame_tree: false,
        };

        let outcome = preprocess_event(key(crate::event::NamedKey::Tab), &mut backend, &mut app);

        assert_eq!(outcome, Reaction::Redraw);
        assert_eq!(backend.focus_tree().focused(), Some(&(id, None)));
        assert_eq!(app.received, vec![UiEvent::FocusChanged(Some(id))]);
    }

    /// Two stops in [`AppLogic::extra_tab_stops`] cycle in order and wrap.
    #[test]
    fn extra_tab_stops_cycles_every_stop_in_order() {
        let a = WidgetId::new("a");
        let b = WidgetId::new("b");
        let mut backend = TuiBackend::new();
        let mut app = TabCycleApp {
            id: a,
            received: Vec::new(),
            extra_tab_stops: vec![(a, COUNTER_RECT), (b, Rect::new(0.0, 3.0, 10.0, 3.0))],
            frame_tree: false,
        };

        preprocess_event(key(crate::event::NamedKey::Tab), &mut backend, &mut app);
        assert_eq!(backend.focus_tree().focused(), Some(&(a, None)));

        preprocess_event(key(crate::event::NamedKey::Tab), &mut backend, &mut app);
        assert_eq!(backend.focus_tree().focused(), Some(&(b, None)));
    }

    /// The regression this guards against: an app that overrides both
    /// [`AppLogic::frame_tree`] (so widget-contract routing and
    /// `FocusTree` apply) and [`AppLogic::extra_tab_stops`] (so step 4's
    /// Tab cycling also applies) must have a Tab-cycled focus actually
    /// reachable by the very next key.
    #[test]
    fn tab_cycled_focus_reaches_the_widget_on_the_next_key() {
        let id = WidgetId::new("counter");
        let mut backend = TuiBackend::new();
        let mut app = TabCycleApp {
            id,
            received: Vec::new(),
            extra_tab_stops: vec![(id, COUNTER_RECT)],
            frame_tree: true,
        };

        preprocess_event(key(crate::event::NamedKey::Tab), &mut backend, &mut app);
        assert_eq!(backend.focus_tree().focused(), Some(&(id, None)));

        preprocess_event(key(crate::event::NamedKey::Up), &mut backend, &mut app);

        let count = {
            let routing = backend.widget_routing_mut().unwrap();
            let state = routing
                .states
                .state_for(id, &Counter { id })
                .downcast_mut::<CounterState>()
                .unwrap();
            state.count
        };
        assert_eq!(
            count, 1,
            "Up must reach the Tab-cycled widget through FocusTree"
        );
    }

    /// A composite widget's children, declared via [`FocusSpec::Children`].
    /// `handle` records which [`SubTarget`](crate::interactive::SubTarget)
    /// it was asked about via [`EventCx::is_focused`] — the direct probe
    /// for whether [`WidgetEventHost::is_focused`] actually discriminates
    /// by sub-target instead of answering every child identically for one
    /// focused id.
    #[derive(Debug)]
    struct TwoCellTable {
        id: WidgetId,
    }

    #[derive(Debug, Default, Clone, PartialEq)]
    struct TwoCellState {
        zero_hits: i32,
        one_hits: i32,
    }

    impl A11yPrimitive for TwoCellTable {
        fn a11y_info(&self) -> A11yInfo {
            A11yInfo::default()
        }
    }

    impl Primitive for TwoCellTable {
        type Layout = Rect;
        type Event = ();

        fn id(&self) -> Option<&WidgetId> {
            Some(&self.id)
        }

        fn layout(&self, _ctx: &LayoutCtx, rect: Rect) -> Rect {
            rect
        }

        fn paint(
            &self,
            _layout: &Rect,
            _surface: &mut dyn crate::paint_surface::PaintSurface,
            _theme: &crate::theme::Theme,
            _style: &crate::style::Style,
        ) {
        }
    }

    impl Interactive for TwoCellTable {
        type State = TwoCellState;
        type Action = crate::interactive::Never;

        fn focus(&self) -> FocusSpec {
            FocusSpec::Children(vec![
                crate::interactive::FocusStop {
                    sub: crate::interactive::SubTarget(0),
                },
                crate::interactive::FocusStop {
                    sub: crate::interactive::SubTarget(1),
                },
            ])
        }

        fn handle(
            &self,
            state: &mut TwoCellState,
            event: &RoutedEvent,
            _layout: &Rect,
            cx: &mut EventCx,
        ) -> Handled<crate::interactive::Never> {
            if !matches!(event, RoutedEvent::Key(_)) {
                return Handled::Ignored;
            }
            if cx.is_focused(Some(crate::interactive::SubTarget(0))) {
                state.zero_hits += 1;
            }
            if cx.is_focused(Some(crate::interactive::SubTarget(1))) {
                state.one_hits += 1;
            }
            Handled::Consumed
        }
    }

    struct TwoCellApp {
        id: WidgetId,
    }

    impl AppLogic for TwoCellApp {
        type AreaId = ();

        fn render(&self, _backend: &mut dyn crate::Backend, _area: ()) {}

        fn handle(&mut self, _event: UiEvent, _backend: &mut dyn crate::Backend) -> Reaction {
            Reaction::Redraw
        }

        fn frame_tree(&self, _area: ()) -> FrameTree {
            let mut tree = FrameTree::new();
            tree.push(
                self.id,
                COUNTER_RECT,
                Box::new(TwoCellTable { id: self.id }),
            );
            tree
        }
    }

    /// `WidgetEventHost::is_focused` (backing `EventCx::is_focused`) tells
    /// a composite widget's own children apart by
    /// [`SubTarget`](crate::interactive::SubTarget) instead of answering
    /// every child identically for one focused id — the stated point of
    /// `dispatch_widget_routed_event` reading `FocusTree`, which carries
    /// the sub-target.
    #[test]
    fn composite_widget_children_are_told_apart_by_sub_target() {
        let id = WidgetId::new("table");
        let mut backend = TuiBackend::new();
        let mut app = TwoCellApp { id };

        backend
            .focus_tree_mut()
            .set_focus(Some((id, Some(crate::interactive::SubTarget(0)))));
        preprocess_event(key(crate::event::NamedKey::Up), &mut backend, &mut app);

        let hits = |backend: &mut TuiBackend| {
            let routing = backend.widget_routing_mut().unwrap();
            let state = routing
                .states
                .state_for(id, &TwoCellTable { id })
                .downcast_mut::<TwoCellState>()
                .unwrap();
            (state.zero_hits, state.one_hits)
        };
        assert_eq!(
            hits(&mut backend),
            (1, 0),
            "sub-target 0 is focused; 1 must not see itself as focused too"
        );

        backend
            .focus_tree_mut()
            .set_focus(Some((id, Some(crate::interactive::SubTarget(1)))));
        preprocess_event(key(crate::event::NamedKey::Up), &mut backend, &mut app);

        assert_eq!(
            hits(&mut backend),
            (1, 1),
            "sub-target 1's own key must not also register for 0"
        );
    }
}
