//! The binding **host** layer: everything a native binding (`quadraui-py`
//! today, a napi leaf next, #1126) needs around [`crate::engine::InteractionEngine`]
//! to drive a [`quadraui::AppLogic`], but that still carries zero
//! host-language FFI type. A binding's own `AppLogic` impl holds one
//! [`BindingHost`] (its per-kind widget map and the backend-settings
//! replay store) plus its own callback
//! fields (a Python callable, a JS function value, ...) and an
//! [`crate::engine::InteractionEngine`]; [`render_overlays`] is the one
//! render-time call every binding's `AppLogic::render` makes after painting
//! its own widgets, so the anchored-overlay z-order — context menu, float,
//! rich-text popup, completions, dialog, tooltip, toast topmost of all, in
//! the exact order [`render_overlays`]'s own body paints them — is written
//! down once instead of copy-pasted per binding.
//!
//! [`NativeExitPolicy`] wraps a binding's own `AppLogic` for the duration of
//! a `run_native`-style call into a real `GtkRunner`/`MacRunner`/`WinRunner`:
//! it forces a handler-less `WindowClose` to actually exit, and — on the
//! `macos` feature without `gtk` also enabled — downgrades any *other*
//! `Reaction::Exit` to `Continue` plus a sticky `exit_requested` flag, so
//! `[NSApp terminate:]`
//! never fires from underneath a binding's own `run_native` call (that
//! downgrade would otherwise end the whole process before `run_native`
//! could return control — and any caught exception with it — to its caller).
//! A napi leaf under `MacRunner` hits the identical hazard, which is why
//! this lives here rather than back in `quadraui-py`.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{Duration, Instant};

use serde::de::DeserializeOwned;
use serde::Serialize;

use quadraui::compose::HoverTooltipTracker;
use quadraui::tui::testing::TuiDriver;
use quadraui::Rect;
use quadraui::{
    mouse_down, mouse_up, Accelerator, AcceleratorId, AppLogic, Backend, Button, Decoration, Key,
    Modifiers, MouseButton, Reaction, Split, SplitMeasure, Style, StyledSpan, TextDisplay,
    TextDisplayLine, Theme, Tooltip, UiEvent, WidgetId,
};

use crate::engine::InteractionEngine;
use crate::hit::PaintedHit;
use crate::layout::{LayoutNode, LayoutTabs, LeafDescriptor, StyleSets};
use crate::render::{
    layout_container_rects, natural_main_length, scroll_content_slots, scroll_viewport_and_track,
    ScrollMetrics, ScrollSlots,
};
use quadraui::layout::stack::{
    apply_split_gap, quantise_horizontal, quantise_vertical, shrink_main_axis,
};
use quadraui::WidgetStateStore;

/// The host-side half of a binding's `AppLogic`: a bag of optional
/// widgets, one per kind, plus the backend-settings replay
/// store a `run_tui`/`run_native`-style call needs to carry onto a freshly
/// constructed backend. A binding's own `AppLogic` impl holds one of these
/// alongside its own callback fields and [`InteractionEngine`] — nothing
/// here is host-language-specific.
#[derive(Default)]
pub struct BindingHost {
    /// The widgets [`crate::render::draw_bands`] paints when a binding
    /// has no render callback, keyed by kind name.
    pub widgets: HashMap<String, std::rc::Rc<dyn quadraui::primitive::DynWidget>>,
    /// Set via a binding's own `register_accelerator`/`unregister_accelerator`
    /// surface. Kept here (not just applied directly to the live backend)
    /// so this whole host — moved by value into a real
    /// `TuiRunner`/`GtkRunner`/`MacRunner`/`WinRunner` once a binding's own
    /// `run_tui`/`run_native` is called — carries its accelerators with it;
    /// [`Self::setup`] replays every entry onto whichever backend that
    /// runner just constructed. Keyed by [`AcceleratorId`] so re-registering
    /// the same id replaces its previous binding/scope/label wholesale.
    pub accelerators: HashMap<AcceleratorId, Accelerator>,
    /// Set via a binding's own `set_theme` surface, replayed onto a new
    /// backend by [`Self::setup`] for the same reason [`Self::accelerators`]
    /// is — a `run_tui`/`run_native` call moves this whole host by value
    /// into a brand-new backend, which starts back at `Theme::default()`
    /// unless told otherwise.
    pub theme: Option<Theme>,
    /// Set via a binding's own `set_style` surface; replayed the same way
    /// as [`Self::theme`].
    pub style: Option<Style>,
    /// The named style sets a binding's own `define_style` surface fills,
    /// and a layout node's `"class"` key refers to — see [`StyleSets`].
    /// Unlike [`Self::theme`]/[`Self::style`] there is nothing to replay
    /// onto a freshly constructed backend: a set is never pushed onto a
    /// `Backend` at all, it is resolved during each frame's own render
    /// callback decode (`crate::wire::decode_render_output_with_styles`),
    /// so moving this host into a `run_tui`/`run_native` runner carries
    /// every registered set with it for free.
    pub style_sets: StyleSets,
    /// Set via a binding's own `set_nerd_fonts` surface; replayed the same
    /// way as [`Self::theme`].
    pub nerd_fonts: Option<bool>,
    /// Set via a binding's own `set_editor_font` surface as
    /// `(family, size_pt)`; replayed the same way as [`Self::theme`].
    pub editor_font: Option<(String, f32)>,
    /// Set via a binding's own `set_ui_font` surface; replayed the same way
    /// as [`Self::theme`].
    pub ui_font: Option<String>,
    /// Set via a binding's own `set_nerd_font_fallback` surface; replayed
    /// the same way as [`Self::theme`].
    pub nerd_font_fallback: Option<String>,
    /// Raw font bytes handed to a binding's own
    /// `register_font_from_memory` surface, kept here so each one can be
    /// re-registered onto a new backend in registration order, the same
    /// replay reasoning as [`Self::accelerators`].
    pub registered_fonts: Vec<Vec<u8>>,
    /// Delay + auto-dismiss state machine for whichever hovered widget's
    /// tooltip text [`Self::hover_tooltip_text`] resolves, driven by
    /// [`Self::drive_hover_tooltip`] — the "host decides when to show it"
    /// half of `Button::tooltip`'s own contract, shared by every binding
    /// instead of each reimplementing [`HoverTooltipTracker`] wiring like
    /// a native backend's own hover-driven test does directly against
    /// `Backend`.
    pub tooltip_tracker: HoverTooltipTracker<WidgetId>,
    /// Tree-address id -> tooltip text for every layout-tree `Button` leaf
    /// a binding's own `render` painted through `crate::render::paint_layout`
    /// this frame, via `crate::render::collect_leaf_tooltips`. Replaced
    /// wholesale by [`Self::set_leaf_tooltips`] each render; consulted by
    /// [`Self::hover_tooltip_text`] as the layout-tree counterpart of
    /// [`Self::widgets`]'s `"button"` entry read. A binding whose render
    /// never paints a layout tree at all leaves this empty forever, so [`Self::hover_tooltip_text`] falls through to
    /// [`Self::widgets`] directly. A [`RefCell`] (not a plain field)
    /// because `AppLogic::render` takes `&self`, not `&mut self` --
    /// the same reason [`Self::tooltip_clock`]/[`Self::test_clock_ms`]
    /// below are `Cell`s rather than plain fields.
    pub leaf_tooltips: RefCell<HashMap<WidgetId, String>>,
    /// Wall-clock epoch [`Self::now_ms`] measures from, lazily set on its
    /// first call rather than at construction — a host is typically built
    /// well before its first frame (a binding constructs its `App` before
    /// `run_tui`/`run_native`/`step` ever runs it), so starting the clock
    /// at construction would count that dead time against the hover
    /// delay. `None` means "not started yet". `pub` like every other
    /// field here only because `BindingHost { .., ..BindingHost::default() }`
    /// struct-update literals outside this crate require every field
    /// visible — a binding should still read this only via [`Self::now_ms`].
    pub tooltip_clock: Cell<Option<Instant>>,
    /// Test-oriented override for [`Self::now_ms`]: when set, `now_ms()`
    /// returns this value directly instead of reading [`Self::tooltip_clock`]'s
    /// real wall clock. `None` (the default) means "use the real clock" —
    /// setting this never changes behaviour for a binding that never calls
    /// [`Self::set_test_clock_ms`]. A binding exposes a thin wrapper around
    /// [`Self::set_test_clock_ms`] on its own app object (e.g. Python's
    /// `QuiApp.set_test_clock_ms`, Node's `QuiApp.setTestClockMs`) so a test
    /// can drive [`Self::drive_hover_tooltip`]'s delay/auto-dismiss deadlines
    /// deterministically — advancing past `tooltip_delay_ms`/
    /// `tooltip_auto_dismiss_ms` by setting an absolute millisecond value,
    /// the same headless-clock contract [`HoverTooltipTracker`]'s own module
    /// doc and this crate's `#[cfg(test)]` module already use directly —
    /// rather than sleeping past them in real wall-clock time.
    pub test_clock_ms: Cell<Option<u64>>,
    /// The session this host's own render-callback/`run_shell`-config
    /// decode calls report an unknown optional wire key through — warn
    /// (the default; once per key for this host's whole lifetime) unless a
    /// binding's own `strict` constructor argument/setter has called
    /// [`crate::wire::DecodeDiagnostics::set_strict`]`(true)` on it. A
    /// binding registers its own `on_unknown_key`-style callable via
    /// [`crate::wire::DecodeDiagnostics::set_hook`].
    pub wire_diagnostics: crate::wire::DecodeDiagnostics,
}

impl BindingHost {
    /// Replays every accelerator and backend-settings field this host has
    /// recorded onto `backend` — call from a binding's own
    /// [`AppLogic::setup`] so a chord/theme/style/font set before
    /// `run_tui`/`run_native` moved this host into a freshly constructed
    /// `TuiBackend`/`GtkBackend`/`MacBackend`/`WinBackend` still applies to
    /// that live backend, not just whichever one was dormant and headless
    /// before. Every real runner calls `app.setup(&mut backend)` right
    /// after building its backend and before the first frame, so this hook
    /// reaches all four.
    pub fn setup(&self, backend: &mut dyn Backend) {
        for acc in self.accelerators.values() {
            backend.register_accelerator(acc);
        }
        if let Some(theme) = self.theme {
            backend.set_theme(theme);
        }
        if let Some(style) = self.style {
            backend.set_style(style);
        }
        if let Some(enabled) = self.nerd_fonts {
            backend.set_nerd_fonts(enabled);
        }
        for bytes in &self.registered_fonts {
            backend.register_font_from_memory(bytes);
        }
        if let Some((family, size_pt)) = &self.editor_font {
            backend.set_editor_font(family, *size_pt);
        }
        if let Some(font_desc) = &self.ui_font {
            backend.set_ui_font(font_desc);
        }
        if let Some(family) = &self.nerd_font_fallback {
            backend.set_nerd_font_fallback(family);
        }
    }

    /// Milliseconds elapsed since this host's own first call to this
    /// method — a monotonic "headless clock" for [`Self::drive_hover_tooltip`]
    /// to feed [`HoverTooltipTracker`] with, since there is no
    /// `Backend`-level clock method to read one from instead (see
    /// [`Self::tooltip_clock`]'s own doc for why the epoch starts here
    /// rather than at construction). Returns [`Self::test_clock_ms`]
    /// directly instead, once a binding has set one via
    /// [`Self::set_test_clock_ms`] — the real wall clock above is never
    /// consulted again after that, so a test can drive deterministic
    /// delay/auto-dismiss deadlines with no real sleep at all. A caller
    /// that wants a fully synthetic, no-real-time-elapses clock without
    /// going through [`Self::test_clock_ms`] at all — the way this crate's
    /// own tests and `quadraui::compose::HoverTooltipTracker`'s module doc
    /// both demonstrate — can still call [`Self::drive_hover_tooltip`]
    /// directly with its own hand-picked `now_ms` rather than this
    /// method's value.
    pub fn now_ms(&self) -> u64 {
        if let Some(ms) = self.test_clock_ms.get() {
            return ms;
        }
        let epoch = self.tooltip_clock.get().unwrap_or_else(|| {
            let started = Instant::now();
            self.tooltip_clock.set(Some(started));
            started
        });
        epoch.elapsed().as_millis() as u64
    }

    /// Sets or clears [`Self::test_clock_ms`], the override
    /// [`Self::now_ms`] returns once set. `Some(ms)` pins `now_ms()` to
    /// `ms` until the next call to this method; `None` reverts to the real
    /// wall clock (as if this had never been called). A binding's own app
    /// object exposes a thin wrapper around this — e.g. Python's
    /// `QuiApp.set_test_clock_ms`, Node's `QuiApp.setTestClockMs` — so a
    /// test can advance the clock deterministically (set an increasing
    /// absolute value) past `tooltip_delay_ms`/`tooltip_auto_dismiss_ms`
    /// without a real sleep.
    pub fn set_test_clock_ms(&self, ms: Option<u64>) {
        self.test_clock_ms.set(ms);
    }

    /// The hover tooltip text for `id`: first [`Self::widgets`]'s own
    /// `"button"` entry's `Button::tooltip` (matched by that entry's own
    /// `Button::id`), then [`Self::leaf_tooltips`] (matched by tree-address
    /// id) -- the layout-tree leaf a render callback painted through
    /// `crate::render::paint_layout`. [`Self::drive_hover_tooltip`] uses
    /// this to decide whether a hovered widget has anything to show at
    /// all before arming [`Self::tooltip_tracker`] for it; a widget with
    /// no tooltip text in either source never starts the delay.
    pub fn hover_tooltip_text(&self, id: WidgetId) -> Option<String> {
        self.widgets
            .get("button")
            .and_then(|widget| widget.as_any().downcast_ref::<Button>())
            .filter(|button| button.id == id)
            .and_then(|button| button.tooltip.clone())
            .or_else(|| self.leaf_tooltips.borrow().get(&id).cloned())
    }

    /// Replaces [`Self::leaf_tooltips`] wholesale with `tooltips` --
    /// typically `crate::render::collect_leaf_tooltips`'s own return
    /// value, walked fresh from the layout tree a render callback just
    /// returned. A binding calls this once per render, right after
    /// painting via `crate::render::paint_layout`, so
    /// [`Self::hover_tooltip_text`] resolves against this frame's tree,
    /// not a stale one from before the tree's shape changed. Takes
    /// `&self` (not `&mut self`), like [`Self::set_test_clock_ms`] --
    /// `AppLogic::render` only ever gets `&self`.
    pub fn set_leaf_tooltips(&self, tooltips: HashMap<WidgetId, String>) {
        *self.leaf_tooltips.borrow_mut() = tooltips;
    }

    /// This host's most recently painted bounds for `id`, read from
    /// `engine`'s own [`InteractionEngine::hit_map`] — the anchor rect
    /// [`Self::drive_hover_tooltip`] shows a tooltip against. Only the
    /// `"button"`-kind [`PaintedHit::Widget`] carries a tooltip source
    /// today (see [`Self::hover_tooltip_text`]), so this only matches a
    /// `Button`-downcastable one; extending [`Self::hover_tooltip_text`]
    /// to a further primitive extends this the same way.
    fn hover_anchor(engine: &InteractionEngine, id: WidgetId) -> Option<Rect> {
        engine.hit_map().iter().find_map(|hit| match hit {
            PaintedHit::Widget {
                id: hit_id,
                area,
                widget,
            } if *hit_id == id && widget.as_any().is::<Button>() => Some(*area),
            _ => None,
        })
    }

    /// Drives [`Self::tooltip_tracker`] from `engine`'s current hit-tested
    /// hover ([`InteractionEngine::hovered`]) and the given headless
    /// clock, opening/closing `engine`'s own tooltip the same way a
    /// native backend's hover-driven test wires [`HoverTooltipTracker`]
    /// directly against `Backend::draw_tooltip` — except here the hovered
    /// widget's tooltip text comes from this host's own descriptor (via
    /// [`Self::hover_tooltip_text`]) rather than a bespoke test
    /// `AppLogic`, since a binding's `render` call is what populated that
    /// descriptor in the first place.
    ///
    /// Call this once per `handle`/`tick` dispatch, after `engine.handle`/
    /// [`InteractionEngine::set_hits`] has already run for the frame, with
    /// the same `now_ms` headless clock [`crate::engine::InteractionEngine`]'s
    /// own module doc and [`HoverTooltipTracker`]'s "headless-clock
    /// contract" describe — [`Self::now_ms`] for a real wall clock, or a
    /// hand-picked counter in a test. `delay_ms` is normally
    /// [`quadraui::Style::tooltip_delay_ms`], `auto_dismiss_ms` normally
    /// [`quadraui::Style::tooltip_auto_dismiss_ms`] (`0` disables it).
    ///
    /// Resyncs first: if [`Self::tooltip_tracker`] still thinks a tooltip
    /// is visible but `engine`'s own tooltip has already closed (e.g.
    /// [`InteractionEngine::handle`]'s own click/key auto-dismiss ran
    /// since the last call), the tracker is explicitly dismissed too, so
    /// a held-still hover over the same widget restarts the delay instead
    /// of believing a tooltip is still showing that the screen no longer
    /// has.
    ///
    /// The pending-delay tick only promotes [`HoverTooltipTracker`] to
    /// visible once [`Self::hover_anchor`] actually finds a painted rect
    /// for the hovered id. Without that guard, a hovered id with no
    /// anchor (not yet painted this frame, or no [`PaintedHit::Widget`]
    /// for it at all) would flip the tracker's own phase to visible with
    /// nothing shown on screen; the next call's resync above would then
    /// see `engine`'s tooltip still closed, dismiss the tracker, and
    /// `on_hover` would re-arm it from scratch — pending, visible-but-
    /// unpainted, dismissed, forever, for a pointer that never moved. The
    /// wake schedule below has the same guard, for the same reason:
    /// without an anchor, [`HoverTooltipTracker::tick`] never ran, so
    /// [`HoverTooltipTracker::next_wake_ms`] would otherwise keep
    /// returning an already-elapsed `Some(0)` and arm a zero-delay wake
    /// every single dispatch for as long as the hover stays stale.
    ///
    /// A tooltip that [`HoverTooltipTracker::tick_auto_dismiss`] hides
    /// stays hidden while the pointer keeps hovering the same anchor —
    /// the tracker latches into its own `Dismissed` phase rather than
    /// resetting to idle, so the delay doesn't simply re-arm and show the
    /// same tooltip again `delay_ms` later (which would auto-dismiss
    /// again after that, forever, with no user input at all). Hovering a
    /// different widget, or leaving, clears the latch.
    ///
    /// Schedules every wake this call still needs — the pending-delay
    /// deadline while an anchor exists, and, while a tooltip is visible
    /// with `auto_dismiss_ms > 0`, its auto-dismiss deadline — directly
    /// on `backend` via [`Backend::request_frame_in`], not only through
    /// the returned [`Reaction`]. A `Reaction::RedrawAfter` folded into a
    /// caller's own `Reaction::Redraw` via [`Reaction::merge`] loses the
    /// deadline (`Redraw` always wins); `request_frame_in` doesn't go
    /// through that merge at all, so the wake survives even on the frame
    /// a hover change *also* triggers an unrelated `Redraw`. The wake is
    /// still deliberately *also* folded into the returned `Reaction`, so
    /// a steady-state pending/visible hover arms two wakes per call
    /// (direct + the caller's own `request_frame_in` off the returned
    /// `Reaction`) rather than one; [`Reaction::RedrawAfter`]'s own doc
    /// calls an early extra wake harmless, and this guarantees the exact
    /// deadline reaches `backend` even when the caller's runner doesn't
    /// re-derive it from the merged reaction at all.
    ///
    /// Returns the [`Reaction`] the caller should merge into its own:
    /// [`Reaction::Redraw`] when a tooltip opened, closed, or
    /// auto-dismissed this call, [`Reaction::RedrawAfter`] while one is
    /// still pending (with an anchor) or counting down to auto-dismiss,
    /// [`Reaction::Continue`] otherwise.
    pub fn drive_hover_tooltip(
        &mut self,
        engine: &mut InteractionEngine,
        backend: &mut dyn Backend,
        now_ms: u64,
        delay_ms: u64,
        auto_dismiss_ms: u64,
    ) -> Reaction {
        if self.tooltip_tracker.visible_item().is_some() && !engine.tooltip_open() {
            self.tooltip_tracker.dismiss();
        }

        let hovered = engine
            .hovered()
            .and_then(|(id, _)| self.hover_tooltip_text(id).map(|text| (id, text)));
        let hovered_id = hovered.as_ref().map(|(id, _)| *id);

        let mut reaction = if self.tooltip_tracker.on_hover(hovered_id, now_ms) {
            engine.hide_tooltip();
            Reaction::Redraw
        } else {
            Reaction::Continue
        };

        let anchor = hovered
            .as_ref()
            .and_then(|(id, _)| Self::hover_anchor(engine, *id));
        if anchor.is_some() && self.tooltip_tracker.tick(delay_ms, now_ms) {
            if let (Some((id, text)), Some(anchor)) = (hovered, anchor) {
                engine.show_tooltip(Tooltip::new(id, text), anchor);
                reaction = Reaction::Redraw;
            }
        }

        if auto_dismiss_ms > 0
            && self
                .tooltip_tracker
                .tick_auto_dismiss(auto_dismiss_ms, now_ms)
        {
            engine.hide_tooltip();
            reaction = Reaction::Redraw;
        }

        // Gated on `anchor.is_some()`: without a painted anchor, `tick`
        // above never ran, so `Phase::Pending` never advances and
        // `next_wake_ms` would return `Some(0)` on every call once the
        // delay has already elapsed -- a zero-delay wake re-armed every
        // dispatch for as long as the hover stays stale. Nothing can
        // make the tracker progress without a new anchor anyway, so
        // skip arming a wake for it at all.
        if anchor.is_some() {
            if let Some(remaining_ms) = self.tooltip_tracker.next_wake_ms(now_ms, delay_ms) {
                let wake = Duration::from_millis(remaining_ms);
                backend.request_frame_in(wake);
                reaction = reaction.merge(Reaction::RedrawAfter(wake));
            }
        }

        if auto_dismiss_ms > 0 {
            if let Some(remaining_ms) = self
                .tooltip_tracker
                .next_auto_dismiss_wake_ms(now_ms, auto_dismiss_ms)
            {
                let wake = Duration::from_millis(remaining_ms);
                backend.request_frame_in(wake);
                reaction = reaction.merge(Reaction::RedrawAfter(wake));
            }
        }

        reaction
    }
}

// ─── Split divider ratio, uncontrolled by default ──────────────────────────
//
// A two-pane `Split`'s divider ratio is uncontrolled by default: an app
// that never echoes a divider drag keeps the dragged ratio, and one that
// sends a new ratio pins it. `Split::ratio` (a bare `f32`, never
// `Option`) has no structural "unset" sentinel at all — every ratio from
// `0.0` to `1.0` is equally plausible as a deliberately pinned value — so
// `SplitRatioState` remembers the last ratio
// this module actually SAW the descriptor send (`baseline`), not the last
// ratio it handed back (`resolved`): a frame counts as controlled only when
// its `Split::ratio` differs from that baseline, i.e. the caller itself
// changed what it's sending, never merely because it differs from whatever
// a previous divider drag persisted. An uncontrolled caller that always
// resends the same literal ratio therefore keeps seeing its own dragged
// value, because that literal ratio never stops matching `baseline`. This
// is the policy [`quadraui::behaviour::row_window::HostLatch`] gives every
// widget's own state.
#[derive(Debug, Clone, Copy)]
struct SplitRatioState {
    baseline: f32,
    resolved: f32,
}

/// The id [`SplitRatioState`] is actually stored under for `split_index`.
/// `split_index` is always `0` for a two-pane [`Split`] — `resolve_split_ratio`
/// looks it up by the split's own `id` with no further addressing, so index
/// `0` keys straight off `tree` unchanged. A `SplitTree`'s N-way divider set
/// can report any index (including `0`, for its own root divider) for a
/// [`quadraui::UiEvent::SplitDividerDragged`] sharing the same event shape,
/// so every OTHER index gets a distinct synthetic id — otherwise dragging
/// divider 3 then divider 1 of the same tree would silently overwrite one
/// shared entry keyed only by `tree`, each drag clobbering the last.
const SPLIT_RATIO_KEY_PREFIX: &str = "quadraui-binding-core::split-ratio::";

fn split_ratio_state_id(tree: WidgetId, split_index: usize) -> WidgetId {
    if split_index == 0 {
        tree
    } else {
        WidgetId::new(format!(
            "{SPLIT_RATIO_KEY_PREFIX}{}::{split_index}",
            tree.as_str()
        ))
    }
}

/// Persists `new_ratio` for `tree`'s `split_index`'th divider — call when a
/// [`quadraui::UiEvent::SplitDividerDragged`] fires, for a two-pane [`Split`]
/// (`split_index` always `0`) or one divider of a multi-divider `SplitTree`
/// alike; see [`split_ratio_state_id`] for how the two share
/// [`quadraui::WidgetStateStore`] without colliding. Leaves `baseline` untouched: it
/// tracks what the descriptor last sent, which a drag never changes by
/// itself — only [`resolve_split_ratio`]'s own next call, once the
/// caller's descriptor catches up (or doesn't), decides whether this
/// drag's value keeps winning. Nothing resolves a `SplitTree`'s divider
/// ratios back out today (there is no `SplitTree` counterpart of
/// `resolve_split_ratio` yet), so persisting one only prevents the
/// cross-divider corruption described above until that counterpart exists.
pub fn apply_split_divider_drag(
    widget_state: &RefCell<WidgetStateStore>,
    tree: WidgetId,
    split_index: usize,
    new_ratio: f32,
) {
    let id = split_ratio_state_id(tree, split_index);
    let mut state = widget_state
        .borrow()
        .value::<SplitRatioState>(&id)
        .unwrap_or(SplitRatioState {
            baseline: new_ratio,
            resolved: new_ratio,
        });
    state.resolved = new_ratio;
    widget_state.borrow_mut().set(id, state);
}

/// Resolves `split`'s ratio for painting — see this section's own doc for
/// why the controlled/uncontrolled decision compares `split.ratio` against
/// the last-seen `baseline` rather than a fixed sentinel. A caller that
/// pins `split.ratio` to the same literal value every frame — including
/// after rejecting a drag outright — reads back as controlled only once
/// that literal value itself changes, never merely because it differs from
/// whatever a previous drag persisted; a caller that never touches `ratio`
/// at all keeps whatever [`apply_split_divider_drag`] persisted
/// indefinitely. Call once per frame, before painting `split`; the
/// returned [`Split`] is what a binding's render should hand to
/// `draw_primitive`, not `split` itself.
pub fn resolve_split_ratio(widget_state: &RefCell<WidgetStateStore>, split: &Split) -> Split {
    let id = split_ratio_state_id(split.id, 0);
    let previous = widget_state.borrow().value::<SplitRatioState>(&id);
    let (baseline, resolved) = match previous {
        None => (split.ratio, split.ratio),
        Some(state) if split.ratio != state.baseline => (split.ratio, split.ratio),
        Some(state) => (state.baseline, state.resolved),
    };
    widget_state
        .borrow_mut()
        .set(id, SplitRatioState { baseline, resolved });
    Split {
        ratio: resolved,
        ..split.clone()
    }
}

// ─── Scroll offset, uncontrolled by default ────────────────────────────────
//
// Unlike `Split::ratio` above (a bare `f32` with no "unset" sentinel),
// `LayoutScroll::offset` is already `Option<f32>` -- see that field's own
// doc for why -- so [`quadraui::WidgetStateStore::resolve`]'s own controlled/uncontrolled
// policy (its module doc's "Controlled / uncontrolled resolution" section)
// applies directly here, with no baseline-latch struct of this module's
// own to maintain.

/// Resolves a [`crate::layout::LayoutNode::Scroll`]'s offset for painting
/// — the `ScrollView` counterpart of [`resolve_split_ratio`]. `offset` is
/// `scroll_node.offset` as decoded this frame (`Some` pins the controlled
/// position, `None` leaves it to whatever this id's own [`quadraui::WidgetStateStore`]
/// entry already holds, seeded at `0.0` the first time `id` is seen).
/// The resolved value is clamped into `[0, max_offset]` on every call
/// (not persisted pre-clamped) so a later frame whose content grows back
/// out doesn't stay stuck at a smaller clamp than the one in effect when
/// a previous, shorter-content frame stored it.
pub fn resolve_scroll_offset(
    widget_state: &RefCell<WidgetStateStore>,
    id: WidgetId,
    offset: Option<f32>,
    max_offset: f32,
) -> f32 {
    let raw = widget_state.borrow_mut().resolve(id, offset, || 0.0_f32);
    raw.clamp(0.0, max_offset.max(0.0))
}

/// Persists `new_offset` as `id`'s own stored scroll offset — call when the
/// engine's wheel/keyboard/scrollbar-drag handling resolves a `Scroll`
/// container to a new position. Always writes, mirroring
/// [`apply_split_divider_drag`]'s own "the next frame's resolve decides
/// whether this sticks" shape: [`resolve_scroll_offset`]'s own
/// [`quadraui::WidgetStateStore::resolve`] call overwrites whatever this stores the
/// moment the matching `LayoutScroll::offset` is `Some` again, so a
/// controlled `Scroll` moving here never actually changes what's
/// painted — only an uncontrolled one does.
pub fn apply_scroll_offset(
    widget_state: &RefCell<WidgetStateStore>,
    id: WidgetId,
    new_offset: f32,
) {
    widget_state.borrow_mut().set(id, new_offset);
}

// ─── Tab group active tab ──────────────────────────────────────────────────

/// Resolves a [`LayoutTabs`] tab group's content pane to the tab its strip
/// shows active: the strip is a [`TabBar`] widget whose own
/// [`quadraui::primitives::tab_bar::TabBarState`] (in `engine`'s widget states, keyed by the
/// bar's id) keeps the tab a click, key or close last activated, and this
/// sets `tabs.selected` to [`TabBar::resolved_active`] so
/// `crate::render::paint_layout`'s `LayoutNode::Tabs` arm paints that
/// tab's content. A `Split` whose `first`/`second` are each a `Tabs` IS
/// this crate's tab group — see [`LayoutTabs`]'s own doc — so an app gets
/// a working tab group with no app state at all.
pub fn resolve_layout_tabs_active(engine: &InteractionEngine, tabs: &mut LayoutTabs) {
    let id = tabs.tab_bar.id;
    let resolved = engine.with_widget_states(|states| {
        let state = states
            .get(&id)
            .and_then(|state| state.downcast_ref::<quadraui::primitives::tab_bar::TabBarState>());
        tabs.tab_bar.resolved_active(state)
    });
    if let Some(idx) = resolved.filter(|&idx| idx < tabs.entries.len()) {
        tabs.selected = idx;
    }
}

/// Resolves a render callback's decoded [`LayoutNode`] tree's container
/// state against `engine`, mutating `node` in place: every `Split`'s
/// ratio and every `Scroll`'s offset (in [`InteractionEngine::widget_state`]),
/// and every `Tabs` group's selected pane (from its strip's own state, see
/// [`resolve_layout_tabs_active`]). A leaf's widget resolves its own state
/// (selection, scroll, expansion, active tab) when it lays out. Walks
/// `Row`/`Column`'s own [`crate::render::layout_container_rects`]
/// subdivision and `Split`'s own [`Split::layout`] exactly as
/// [`crate::render::paint_layout`] does — including quantising
/// `first_min`/`second_min` into native units and carving the quantised
/// gap out before layout — so a nested `Scroll` gets the same viewport it
/// is actually painted into. `SidebarPanel`/`SplitTree` hold no state of
/// their own here, so their content recurses with `area` passed straight
/// through. A `SplitTree`'s own divider ratios are a different matter —
/// see [`apply_split_divider_drag`]'s own doc.
///
/// Call once per frame, immediately before [`crate::render::paint_layout`]
/// — `node` is mutated in place, so the exact same tree handed to
/// `paint_layout` right after is the resolved one.
pub fn resolve_layout_node(
    engine: &InteractionEngine,
    backend: &mut dyn Backend,
    area: Rect,
    node: &mut LayoutNode,
) {
    let widget_state = engine.widget_state();
    match node {
        LayoutNode::Row(container) => {
            let char_width = backend.char_width();
            let line_height = backend.line_height();
            let inner = container.spacing.inset(area, char_width, line_height);
            let rects = layout_container_rects(backend, inner, container, true);
            for (child, rect) in container.children.iter_mut().zip(rects) {
                resolve_layout_node(engine, backend, rect, &mut child.node);
            }
        }
        LayoutNode::Column(container) => {
            let char_width = backend.char_width();
            let line_height = backend.line_height();
            let inner = container.spacing.inset(area, char_width, line_height);
            let rects = layout_container_rects(backend, inner, container, false);
            for (child, rect) in container.children.iter_mut().zip(rects) {
                resolve_layout_node(engine, backend, rect, &mut child.node);
            }
        }
        LayoutNode::Grid(grid) => {
            let rects = grid.cell_rects(area, backend.char_width(), backend.line_height());
            for (child, rect) in grid.children.iter_mut().zip(rects) {
                resolve_layout_node(engine, backend, rect, child);
            }
        }
        LayoutNode::Leaf(_) => {}
        LayoutNode::Split(split) => {
            split.split = resolve_split_ratio(widget_state, &split.split);
            let char_width = backend.char_width();
            let line_height = backend.line_height();
            let inner = split.spacing.inset(area, char_width, line_height);
            // Mirrors `crate::render::paint_layout`'s own `LayoutNode::Split`
            // arm exactly: `first_min`/`second_min` are decoded as portable
            // line counts and must be scaled into native units before
            // `Split::layout` clamps against them, and the quantised gap is
            // carved out of `inner` before layout (`Split::layout`'s min-size
            // clamp evaluates against whatever rect it's handed) and
            // reinserted into `second_bounds` after, via the identical
            // `shrink_main_axis`/`apply_split_gap` pair -- otherwise a
            // non-zero `first_min`/`second_min`/`gap` would hand a nested
            // `Tree`/`List`/`DataTable` leaf a first-frame rect that
            // diverges from the one it's actually painted into.
            let native_split = Split {
                first_min: split.split.first_min * line_height,
                second_min: split.split.second_min * line_height,
                ..split.split.clone()
            };
            let horizontal = matches!(native_split.direction, quadraui::SplitDirection::Horizontal);
            let gap = if horizontal {
                quantise_horizontal(split.spacing.gap, char_width)
            } else {
                quantise_vertical(split.spacing.gap, line_height)
            };
            let draw_area = shrink_main_axis(inner, horizontal, gap);
            let mut layout = native_split.layout(draw_area, SplitMeasure::new(line_height));
            apply_split_gap(&mut layout, horizontal, gap);
            resolve_layout_node(engine, backend, layout.first_bounds, &mut split.first);
            resolve_layout_node(engine, backend, layout.second_bounds, &mut split.second);
        }
        LayoutNode::Tabs(tabs) => {
            resolve_layout_tabs_active(engine, tabs);
            let char_width = backend.char_width();
            let line_height = backend.line_height();
            let inner = tabs.spacing.inset(area, char_width, line_height);
            let strip_height = (3.0 * line_height).min(inner.height);
            let content_rect = Rect::new(
                inner.x,
                inner.y + strip_height,
                inner.width,
                (inner.height - strip_height).max(0.0),
            );
            let selected = tabs.selected;
            // Only the selected entry's content has geometry this frame and
            // resolves; every other entry's stored state is kept alive
            // untouched, so switching away and back restores its cursor,
            // scroll, ratios and active tabs.
            for (index, entry) in tabs.entries.iter_mut().enumerate() {
                if index == selected {
                    resolve_layout_node(engine, backend, content_rect, &mut entry.content);
                } else {
                    keep_subtree_state_alive(engine, &entry.content);
                }
            }
        }
        LayoutNode::SidebarPanel(sidebar_node) => {
            resolve_layout_node(engine, backend, area, &mut sidebar_node.content);
        }
        LayoutNode::SplitTree(split_tree) => {
            for leaf in &mut split_tree.leaves {
                resolve_layout_node(engine, backend, area, leaf);
            }
        }
        LayoutNode::Scroll(scroll_node) => {
            let metrics = ScrollMetrics {
                char_width: backend.char_width(),
                line_height: backend.line_height(),
            };
            let inner = scroll_node
                .spacing
                .inset(area, metrics.char_width, metrics.line_height);
            let (viewport, _track) =
                scroll_viewport_and_track(inner, scroll_node.axis, metrics.line_height);
            let content_len =
                natural_main_length(backend, &scroll_node.content, scroll_node.axis, metrics);
            let visible_len = match scroll_node.axis {
                quadraui::ScrollAxis::Vertical => viewport.height,
                quadraui::ScrollAxis::Horizontal => viewport.width,
            };
            let max_offset = (content_len - visible_len).max(0.0);
            let id = WidgetId::new(scroll_node.id.as_str());
            let offset = resolve_scroll_offset(widget_state, id, scroll_node.offset, max_offset);
            scroll_node.offset = Some(offset);

            let slots = scroll_content_slots(
                backend,
                viewport,
                scroll_node.axis,
                offset,
                &scroll_node.content,
                content_len,
                metrics,
            );
            match slots {
                ScrollSlots::Children(slots) => {
                    // `scroll_node.content` may be a `LayoutNode::Styled`
                    // wrapper around the `Row`/`Column` -- a `"style"` key
                    // is purely cosmetic, so match against
                    // `LayoutNode::unstyled_mut` here rather than the raw
                    // node, the same structural match
                    // `crate::render::scroll_content_slots`'s own
                    // `matching_container` makes.
                    if let LayoutNode::Row(container) | LayoutNode::Column(container) =
                        scroll_node.content.unstyled_mut()
                    {
                        for (child, slot) in container.children.iter_mut().zip(slots) {
                            if slot.visible {
                                resolve_layout_node(engine, backend, slot.rect, &mut child.node);
                            } else {
                                keep_subtree_state_alive(engine, &child.node);
                            }
                        }
                    }
                }
                ScrollSlots::Atomic(rect, visible) => {
                    if visible {
                        resolve_layout_node(engine, backend, rect, &mut scroll_node.content);
                    } else {
                        keep_subtree_state_alive(engine, &scroll_node.content);
                    }
                }
            }
        }
        // A `"style"` wrapper carries no widget state of its own -- it's
        // purely a `crate::render::paint_layout`-time overlay -- so this
        // resolve pass is a straight passthrough to whatever it wraps.
        LayoutNode::Styled(styled) => {
            resolve_layout_node(engine, backend, area, &mut styled.node);
        }
    }
}

/// Keeps every state entry owned by `node`'s subtree alive through this
/// frame's sweep without resolving it: the container state in
/// [`InteractionEngine::widget_state`] and each leaf widget's own
/// `Interactive::State`. A subtree with no geometry this frame (an
/// unselected tab entry, a scroll child outside the viewport) still owns
/// its selection, scroll offset, split ratio, expansion and active tab;
/// they come back unchanged when it is shown again.
fn keep_subtree_state_alive(engine: &InteractionEngine, node: &LayoutNode) {
    let mut owners = HashSet::new();
    let mut widgets = Vec::new();
    collect_state_owner_ids(node, &mut owners, &mut widgets);
    engine.keep_widget_states_alive(widgets);
    if owners.is_empty() {
        return;
    }
    engine
        .widget_state()
        .borrow_mut()
        .keep_alive(|key| owners.contains(state_owner_id(key)));
}

/// Every container id under `node` that [`InteractionEngine::widget_state`]
/// keys state on, directly or through a derived key [`state_owner_id`]
/// maps back to it, into `owners`; every leaf widget's state key into
/// `widgets`.
fn collect_state_owner_ids(
    node: &LayoutNode,
    owners: &mut HashSet<String>,
    widgets: &mut Vec<WidgetId>,
) {
    match node {
        LayoutNode::Row(container) | LayoutNode::Column(container) => {
            for child in &container.children {
                collect_state_owner_ids(&child.node, owners, widgets);
            }
        }
        LayoutNode::Grid(grid) => {
            for child in &grid.children {
                collect_state_owner_ids(child, owners, widgets);
            }
        }
        LayoutNode::Leaf(leaf) => {
            let LeafDescriptor::Widget { widget, .. } = &leaf.descriptor;
            widgets.push(crate::engine::dyn_state_key(
                WidgetId::new(leaf.id.as_str()),
                widget.as_ref(),
            ));
        }
        LayoutNode::Split(split) => {
            owners.insert(split.split.id.as_str().to_string());
            collect_state_owner_ids(&split.first, owners, widgets);
            collect_state_owner_ids(&split.second, owners, widgets);
        }
        LayoutNode::Tabs(tabs) => {
            widgets.push(tabs.tab_bar.id);
            for entry in &tabs.entries {
                collect_state_owner_ids(&entry.content, owners, widgets);
            }
        }
        LayoutNode::SidebarPanel(sidebar_node) => {
            collect_state_owner_ids(&sidebar_node.content, owners, widgets);
        }
        LayoutNode::SplitTree(split_tree) => {
            owners.insert(split_tree.id.clone());
            for leaf in &split_tree.leaves {
                collect_state_owner_ids(leaf, owners, widgets);
            }
        }
        LayoutNode::Scroll(scroll_node) => {
            owners.insert(scroll_node.id.clone());
            collect_state_owner_ids(&scroll_node.content, owners, widgets);
        }
        LayoutNode::Styled(styled) => collect_state_owner_ids(&styled.node, owners, widgets),
    }
}

/// The owning container id of an [`InteractionEngine::widget_state`] key:
/// the key itself, or the id a derived [`split_ratio_state_id`] key was
/// built from.
fn state_owner_id(key: &str) -> &str {
    match key.strip_prefix(SPLIT_RATIO_KEY_PREFIX) {
        Some(rest) => rest.rsplit_once("::").map_or(rest, |(owner, _)| owner),
        None => key,
    }
}

/// What a binding's `widget_state(id)` reports for one id: the
/// [`quadraui::interactive::StateSummary`] the widget painted under `id`
/// keeps in its own state (selection, scroll, active tab, expansion),
/// plus a `Split`'s divider ratio, which the host keeps.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WidgetStateSnapshot {
    /// The widget's own state summary.
    #[serde(flatten)]
    pub summary: quadraui::interactive::StateSummary,
    /// A `Split`'s stored divider ratio, if `id` is one being tracked by
    /// [`resolve_split_ratio`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ratio: Option<f32>,
}

/// Reads [`WidgetStateSnapshot`] back for `id`, or `None` if neither a
/// widget painted this frame keeps state under `id` nor a split ratio is
/// stored for it. Never mutates any state — a snapshot is a
/// side-effect-free read.
pub fn widget_state_snapshot(
    engine: &InteractionEngine,
    id: WidgetId,
) -> Option<WidgetStateSnapshot> {
    let summary = engine.widget_state_summary(id);
    let ratio = engine
        .widget_state()
        .borrow()
        .value::<SplitRatioState>(&id)
        .map(|stored| stored.resolved);
    if summary.is_none() && ratio.is_none() {
        return None;
    }
    Some(WidgetStateSnapshot {
        summary: summary.unwrap_or_default(),
        ratio,
    })
}

/// The anchored-overlay paint pass every binding's `AppLogic::render` runs
/// once, after painting its own fixed/additive slots (via [`crate::render::draw_bands`]
/// or [`crate::render::paint_layout`]) and collecting `hits`: makes the one [`InteractionEngine::render_overlays`] call that
/// paints every open overlay in z-order (dialog on top of everything but
/// tooltip/toast, then float/rich-text-popup/completions below it, toast
/// topmost of all) — a correctness contract worth writing down in exactly
/// one place rather than copy-pasted at each of a binding's render paths
/// (the static-slots path and the render-callback path both reach this).
/// Takes `hits` by value and hands it to [`InteractionEngine::set_hits`] at
/// the end, after `render_overlays` has painted.
pub fn render_overlays(
    engine: &InteractionEngine,
    backend: &mut dyn Backend,
    hits: Vec<PaintedHit>,
) {
    engine.render_overlays(backend);
    engine.set_hits(hits);
}

/// Maps a registered handler/tick callback's return value to a [`Reaction`]:
/// `Some(false)` requests the same exit a raised exception would, with no
/// exception needed; anything else (including the common case of no
/// explicit `return` and a non-bool return) continues. `value` is the
/// callback's return value, already narrowed to `Option<bool>` by the
/// binding's own "was this literally a bool" check (an int `0`, an empty
/// string, or any other "falsy" host-language value must keep running
/// rather than exiting, so only an explicit `false` opts in — the binding
/// decides what "literally a bool" means for its own host language, this
/// function only decides what to do once it knows).
pub fn callback_reaction(value: Option<bool>) -> Reaction {
    match value {
        Some(false) => Reaction::Exit,
        _ => Reaction::Continue,
    }
}

/// Wraps [`callback_reaction`], promoting an ordinary [`Reaction::Continue`]
/// to [`Reaction::Redraw`] whenever `render_callback_present` — i.e. a
/// render callback is registered on this app.
///
/// A render callback only runs when the runner decides a frame needs
/// repainting (`AppLogic::render` is reached only after an
/// `EventOutcome::Redraw`/`RedrawAfter`-driven repaint). A binding's own
/// `handle`/`tick` have no way to tell whether a callback's mutation of its
/// own host-language state changed anything visible, so once a render
/// callback is registered every non-exit dispatch is treated as one that
/// might have — the same "redraw every frame, let the render callback
/// decide what that looks like" contract a React/Electron-style
/// `render(state)` callback expects. Without this, a handler returning the
/// common "no explicit return" case maps to `Reaction::Continue`, the
/// runner never calls `AppLogic::render` again, and the render callback
/// never re-runs — the screen stays frozen at whatever the last repaint
/// drew.
pub fn callback_reaction_for_render(
    value: Option<bool>,
    render_callback_present: bool,
) -> Reaction {
    match callback_reaction(value) {
        Reaction::Continue if render_callback_present => Reaction::Redraw,
        other => other,
    }
}

/// Maps a registered handler's return value to a [`Reaction`] for a
/// `WindowClose` dispatch specifically — the opposite default from
/// [`callback_reaction`]. A `WindowClose` handler's job is to decide
/// whether the window may close, so the no-opinion case (`None` — by far
/// the common case for a handler that just logs or ignores the event) must
/// let the close proceed, exactly like having no handler registered at
/// all; only an explicit `Some(true)` vetoes it and leaves the window open.
/// A non-bool return (an int, a string, any other "truthy" host-language
/// value, narrowed to `None` by the binding's own check) does not
/// accidentally veto a close the author never meant to block.
pub fn window_close_reaction(value: Option<bool>, render_callback_present: bool) -> Reaction {
    match value {
        Some(true) if render_callback_present => Reaction::Redraw,
        Some(true) => Reaction::Continue,
        _ => Reaction::Exit,
    }
}

/// Whether a binding's own `AppLogic` has a registered handler for
/// `WindowClose` events — the one piece of host-specific knowledge
/// [`NativeExitPolicy`] needs that it cannot recover from a [`Reaction`]
/// alone (a no-handler-at-all dispatch and an explicit "stay open" veto
/// both resolve to [`Reaction::Continue`], but only the former should force
/// an exit). A binding implements this once, trivially (`self.handler.is_some()`
/// or equivalent), alongside its own [`AppLogic`] impl.
pub trait WindowCloseHandling {
    fn has_window_close_handler(&self) -> bool;
}

/// Wraps a binding's own `AppLogic` for the duration of a `run_native`-style
/// call into a real `GtkRunner`/`MacRunner`/`WinRunner` — see this module's
/// own doc for the two things it corrects that `apply_reaction`'s generic
/// `Reaction` handling cannot: a handler-less `WindowClose` must actually
/// exit (the inner `AppLogic`'s own "no handler" default is
/// [`Reaction::Continue`], which would otherwise leave `run_native()` with
/// no way to close its own window), and — on the `macos` feature without
/// `gtk` also enabled — every *other* `Reaction::Exit` must downgrade to
/// `Continue` plus `exit_requested` instead of reaching
/// `apply_reaction`'s own `Reaction::Exit` branch, which calls
/// `[NSApp terminate:]` and ends the whole OS process before `run_native`
/// could return.
///
/// `win` needs no `exit_requested` field, for the same reason `gtk`
/// doesn't: both already call their own window-destroy path for *any*
/// `Reaction::Exit`, not just one reached through `WindowClose` — there is
/// no AppKit-style `[NSApp terminate:]` for this wrapper to head off there.
#[cfg(any(feature = "gtk", feature = "macos", feature = "win"))]
#[cfg_attr(
    any(
        all(feature = "macos", not(feature = "gtk"), not(target_os = "macos")),
        all(
            feature = "win",
            not(feature = "gtk"),
            not(feature = "macos"),
            not(target_os = "windows")
        )
    ),
    allow(dead_code)
)]
pub struct NativeExitPolicy<A> {
    pub inner: A,
    /// Set by `handle`/`tick` when a non-`WindowClose` `Reaction::Exit` is
    /// intercepted; the caller's own pump loop breaks once this is set.
    /// Only present on the `macos` feature without `gtk` also enabled —
    /// GTK's own `apply_reaction` and `win`'s dispatch/tick already handle
    /// `Reaction::Exit` from any event without this wrapper's help.
    #[cfg(all(feature = "macos", not(feature = "gtk")))]
    pub exit_requested: std::rc::Rc<Cell<bool>>,
}

#[cfg(any(feature = "gtk", feature = "macos", feature = "win"))]
impl<A: AppLogic + WindowCloseHandling> AppLogic for NativeExitPolicy<A> {
    type AreaId = A::AreaId;

    fn setup(&mut self, backend: &mut dyn Backend) {
        self.inner.setup(backend);
    }

    fn render(&self, backend: &mut dyn Backend, area: Self::AreaId) {
        self.inner.render(backend, area);
    }

    // An inner app that overrides `extra_tab_stops` must keep doing so
    // through this wrapper — the default on `AppLogic` returns an empty
    // list, which would silently drop the inner app's tab order.
    fn extra_tab_stops(&self, area: Self::AreaId) -> Vec<(WidgetId, Rect)> {
        self.inner.extra_tab_stops(area)
    }

    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
        let is_window_close = matches!(event, UiEvent::WindowClose);
        // A registered handler's own opinion on `WindowClose` — including a
        // veto (anything but `Reaction::Exit`) — must reach the caller
        // untouched; only the no-handler-at-all case gets a forced default.
        let has_window_close_handler = is_window_close && self.inner.has_window_close_handler();
        let reaction = self.inner.handle(event, backend);
        if is_window_close {
            return if has_window_close_handler {
                reaction
            } else {
                Reaction::Exit
            };
        }
        #[cfg(all(feature = "macos", not(feature = "gtk")))]
        if matches!(reaction, Reaction::Exit) {
            self.exit_requested.set(true);
            return Reaction::Continue;
        }
        reaction
    }

    fn tick(&mut self, backend: &mut dyn Backend) -> Reaction {
        let reaction = self.inner.tick(backend);
        #[cfg(all(feature = "macos", not(feature = "gtk")))]
        if matches!(reaction, Reaction::Exit) {
            self.exit_requested.set(true);
            return Reaction::Continue;
        }
        reaction
    }
}

/// Narrow, dyn-safe surface of [`TuiDriver`] a binding's own headless-shell
/// app (one built around `quadraui::tui::shell_adapter`'s `ShellApp`, whose
/// concrete `TuiDriver<impl AppLogic>` type is opaque per call site) can
/// store behind `Box<dyn ErasedShellDriver>` instead.
pub trait ErasedShellDriver {
    fn dispatch(&mut self, event: UiEvent) -> Reaction;
    fn pump_user_events(&mut self) -> Reaction;
    fn tick(&mut self) -> Reaction;
    fn render(&mut self);
    fn resize(&mut self, width: u16, height: u16);
    fn exited(&self) -> bool;
    fn screen(&self) -> String;
    fn screen_contains(&self, needle: &str) -> bool;
    fn find(&self, needle: &str) -> Option<(f32, f32)>;
    fn tab_center(&self, bar: &WidgetId, tab_idx: usize) -> Option<(f32, f32)>;
}

impl<A: AppLogic> ErasedShellDriver for TuiDriver<A> {
    fn dispatch(&mut self, event: UiEvent) -> Reaction {
        self.dispatch(event)
    }

    fn pump_user_events(&mut self) -> Reaction {
        self.pump_user_events()
    }

    fn tick(&mut self) -> Reaction {
        self.tick()
    }

    fn render(&mut self) {
        self.render()
    }

    fn resize(&mut self, width: u16, height: u16) {
        self.resize(width, height)
    }

    fn exited(&self) -> bool {
        self.exited()
    }

    fn screen(&self) -> String {
        self.screen()
    }

    fn screen_contains(&self, needle: &str) -> bool {
        self.screen_contains(needle)
    }

    fn find(&self, needle: &str) -> Option<(f32, f32)> {
        self.find(needle)
    }

    fn tab_center(&self, bar: &WidgetId, tab_idx: usize) -> Option<(f32, f32)> {
        self.tab_center(bar, tab_idx)
    }
}

/// Advances `driver` by one step: dispatches the oldest queued event, or —
/// if none is queued — pumps any posted user events and then ticks. Returns
/// whether the app is still running (`false` once `driver.exited()`). The
/// "drain the queue, else pump-and-tick" order every binding's own
/// `step`/`step_once` surface needs identically; the one thing that differs
/// between a headless app and a shell app — where a pending callback error
/// is recorded, and in what shape it is re-raised — stays with the caller.
///
/// The idle ("nothing queued") branch always renders once at the end, even
/// when neither `pump_user_events` nor `tick` asked for a
/// [`Reaction::Redraw`] (each of those already repaints internally when
/// they do, via `TuiDriver::pump_user_events`/`TuiDriver::tick`, so this
/// skips a redundant second repaint in that case). Without this, a freshly
/// registered render callback that decodes badly only surfaces its error
/// the next time something explicitly forces a render — `screen`/
/// `screen_contains`, or an event whose own dispatch happens to request a
/// redraw — rather than on the very next idle `step`/`run` call, which is
/// surprising for a caller stepping an app with nothing queued specifically
/// to let it settle before inspecting it.
///
/// This is deliberately unconditional rather than gated on some "is the
/// tree actually dirty" flag: no such flag exists anywhere upstream of
/// this call (`Reaction::Continue` means "nothing *this event/tick* asked
/// to redraw", not "nothing has changed since the last repaint"), and
/// `TuiDriver::render`/`render_frame` has no cheaper early-out than
/// re-walking the tree and re-painting `ratatui`'s cell buffer either. The
/// cost this trades away is real: a caller `step`-ping or `run`-ning at a
/// steady cadence with nothing queued now re-invokes the registered
/// render callback (JS/Python decode included) on every idle poll, not
/// only on an explicit redraw request — for a host polling at, say, 60 Hz
/// while genuinely idle, that is 60 render-callback invocations/sec where
/// there were none before. A future dirty-tracking flag (set on any state
/// mutation, cleared after a render) would let this skip the repaint when
/// nothing changed and only pay that cost on the very first idle step
/// after a fresh registration — the one case this doc's "freshly
/// registered render callback" paragraph above actually needs.
pub fn step_driver(
    driver: &mut dyn ErasedShellDriver,
    pending_events: &mut VecDeque<UiEvent>,
) -> bool {
    if driver.exited() {
        return false;
    }
    match pending_events.pop_front() {
        Some(event) => {
            driver.dispatch(event);
        }
        None => {
            let reaction = driver.pump_user_events();
            let reaction = if driver.exited() {
                reaction
            } else {
                reaction.merge(driver.tick())
            };
            if !driver.exited() && reaction != Reaction::Redraw {
                driver.render();
            }
        }
    }
    !driver.exited()
}

/// A synthetic `KeyPressed` event for the character `ch`, no modifiers
/// held, not a repeat — the event shape behind a binding's own
/// `push_key_event` convenience.
pub fn key_pressed_event(ch: char) -> UiEvent {
    UiEvent::KeyPressed {
        key: Key::Char(ch),
        modifiers: Modifiers::default(),
        repeat: false,
    }
}

/// A synthetic `MouseDown` + `MouseUp` left-click at `(x, y)` — the event
/// pair behind a binding's own `push_click` convenience.
pub fn click_down_up_events(x: f32, y: f32) -> [UiEvent; 2] {
    [
        mouse_down(MouseButton::Left, x, y, Modifiers::default()),
        mouse_up(MouseButton::Left, x, y),
    ]
}

/// Message for a `QuiApp` method restricted to top-level script code
/// (one of the overlay/OS-call/window-control methods with a matching
/// `QuiAppActions` method — `show_message_dialog`, `set_title`, ...) that
/// a caller invoked re-entrantly, from inside that same app's own
/// registered event/tick/render callback. Used by `quadraui-py`'s own
/// `ReentrantCallError` guard (`guard_against_reentrant_call`) so the
/// wording there stays in one place rather than duplicated at every one
/// of its call sites. `quadraui-node` has no Rust-side caller of this
/// function at all — its own FFI layer never gets a chance to run custom
/// guard code before napi-rs's own borrow check rejects the re-entrant
/// call (see `quadraui-node/js/index.ts`'s `wrapNativeErrors` doc) — so
/// it hardcodes the identical wording in TypeScript instead; a reword
/// here needs a matching manual edit there, since nothing pins the two
/// strings together.
pub fn reentrant_call_message(method: &str) -> String {
    format!(
        "app.{method} can't be called from inside a handler; use the \
         actions() handle passed to / available in the handler"
    )
}

/// Downcasts a caught panic payload to a human-readable message — shared by
/// every binding's own panic-at-the-boundary guard (mirrors
/// `quadraui-ffi`'s `record_panic`).
pub fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(text) = payload.downcast_ref::<&'static str>() {
        (*text).to_owned()
    } else if let Some(text) = payload.downcast_ref::<String>() {
        text.clone()
    } else {
        "<non-string panic payload>".to_owned()
    }
}

/// Why [`guarded`] returned early with an error instead of `body`'s own
/// result — the one piece of information `to_err` needs to build that
/// binding's own exception type (`PoisonedError`/`PanicError` in
/// `quadraui-py`, their napi-rs equivalents in `quadraui-node`).
pub enum GuardedFailure {
    /// `state` was already poisoned by an earlier caught panic; `body`
    /// never ran.
    Poisoned,
    /// `body` panicked; [`panic_message`]'s text for that panic.
    Panic(String),
}

/// Runs `body` on `state`, catching a Rust panic instead of letting it
/// unwind across the binding's own FFI boundary (PyO3, napi-rs, ...) and
/// poisoning `state` on one — the "catch panic, poison on panic, refuse
/// every call once poisoned" shape every binding's own `QuiApp::guarded`/
/// `QuiShellApp::guarded` pair repeats (`quadraui-py`'s and
/// `quadraui-node`'s alike), parameterized over that binding's own
/// `state` type `S`, its own error type `E` (`PyErr`, napi-rs's `Error`,
/// ...), and the small pieces that differ per call site: how `S` itself
/// stores its poison flag (`is_poisoned`/`set_poisoned` — a plain `bool`
/// field for every call site today, but this takes closures rather than
/// assuming that shape so a future call site backed by a `Cell`/
/// `AtomicBool` still fits), and how to build `E` from a [`GuardedFailure`]
/// (a binding's own exception type, e.g.
/// `PoisonedError::new_err(..)`/`PanicError::new_err(..)`).
///
/// `before`/`after` run immediately before/after the guarded `body` call —
/// empty no-ops for a binding with nothing else to do there
/// (`quadraui-py`'s `QuiApp`/`QuiShellApp`), or napi-rs's own
/// "stash the live `Env`, flip a re-entrancy flag, run `body`, flip the
/// flag back" shape (`quadraui-node`'s `QuiApp`/`QuiShellApp`) — `after`
/// still runs even when `body` panics, since `before`/`after` bracket the
/// `catch_unwind` call itself rather than running inside it. A binding's
/// own re-entrancy check (`check_not_dispatching`-style) runs before this
/// function is even called, since it must reject *without* touching the
/// poison flag or running `before` at all.
///
/// On success, returns `body`'s result unchanged. If `state` is already
/// poisoned, returns `to_err(GuardedFailure::Poisoned)` without calling
/// `before`/`body`/`after` at all. On a caught panic, poisons `state` via
/// `set_poisoned` and returns `to_err(GuardedFailure::Panic(..))`.
pub fn guarded<S, T, E>(
    state: &mut S,
    is_poisoned: impl FnOnce(&S) -> bool,
    set_poisoned: impl FnOnce(&mut S),
    before: impl FnOnce(&mut S),
    after: impl FnOnce(&mut S),
    to_err: impl FnOnce(GuardedFailure) -> E,
    body: impl FnOnce(&mut S) -> Result<T, E>,
) -> Result<T, E> {
    if is_poisoned(state) {
        return Err(to_err(GuardedFailure::Poisoned));
    }
    before(state);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| body(state)));
    after(state);
    match result {
        Ok(result) => result,
        Err(payload) => {
            set_poisoned(state);
            Err(to_err(GuardedFailure::Panic(panic_message(&*payload))))
        }
    }
}

/// Records `err` into `slot`, but only if no earlier error already
/// claimed it — "first error wins", so that (for example) a handler raise
/// recorded first isn't silently clobbered by a render callback also
/// raising while painting the same frame. The same first-wins contract
/// every binding's own `pending_error`/`ShellState::record_error` already
/// implements by hand, generalized over that binding's own error type
/// `E` (`PyErr`, napi-rs's `Error`, ...) and wherever it stores the slot
/// (a plain field, or the `Option<E>` a `Rc<RefCell<_>>`/`RefMut`
/// dereferences to).
pub fn record_pending_error<E>(slot: &mut Option<E>, err: E) {
    if slot.is_none() {
        *slot = Some(err);
    }
}

/// Takes `slot`'s own pending error, if any, turning it into an `Err` the
/// caller re-raises out of its own `step`/`run` — the `Result`-shaped
/// counterpart of [`record_pending_error`]'s "first error wins" slot,
/// generalized the same way.
pub fn take_pending_error<E>(slot: &mut Option<E>) -> Result<(), E> {
    match slot.take() {
        Some(err) => Err(err),
        None => Ok(()),
    }
}

/// A single-line [`TextDisplay`] descriptor per entry of `lines`, under
/// `id` — the shape behind both a binding's own static bottom-panel-tab
/// content and its `render_content`/`set_render_content` callback result.
pub fn text_display_from_lines(id: WidgetId, lines: Vec<String>) -> TextDisplay {
    TextDisplay {
        id,
        lines: lines
            .into_iter()
            .map(|text| TextDisplayLine {
                spans: vec![StyledSpan::plain(text)],
                decoration: Decoration::Normal,
                timestamp: None,
            })
            .collect(),
        scroll_offset: 0,
        items_window: None,
        auto_scroll: false,
        max_lines: 0,
        has_focus: false,
        title: None,
        show_scrollbar: false,
    }
}

/// An error from [`merge_overrides`] — a binding maps each variant to its
/// own error type (an "invalid input" kind for [`OverrideError::UnknownField`]/
/// [`OverrideError::InvalidValue`]/[`OverrideError::Invalid`], an "internal
/// bug" kind for [`OverrideError::Internal`]).
#[derive(Debug)]
pub enum OverrideError {
    /// Serializing `current` to JSON failed, or its JSON form wasn't an
    /// object — a bug in the caller's own `T`, never the caller's input.
    Internal(String),
    /// An override key names a field `T` doesn't have.
    UnknownField(String),
    /// `value_for` rejected one override's value.
    InvalidValue(String),
    /// The merged value no longer deserializes into `T` — a type mismatch
    /// between an override's value and the field it names.
    Invalid(String),
}

impl std::fmt::Display for OverrideError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OverrideError::Internal(msg) => write!(f, "{msg}"),
            OverrideError::UnknownField(key) => {
                write!(f, "{key:?} is not a recognized field")
            }
            OverrideError::InvalidValue(msg) | OverrideError::Invalid(msg) => write!(f, "{msg}"),
        }
    }
}

/// Merges one override per `keys` entry onto `current`'s own JSON
/// representation, then deserializes the merged value back into `T` — a
/// binding's own `set_theme`/`set_style`-style "override only the fields
/// you name" mechanism, so a caller never has to restate every field just
/// to change one. `value_for` is called with each key only after this
/// function has confirmed `T`'s JSON form actually has that field (so a
/// binding's own per-key value decode never runs for an unrecognized key),
/// and converts that binding's own wire value for `key` into the
/// [`serde_json::Value`] to splice in — see [`theme_color_value`] for the
/// one binding-specific convenience ([`crate`]'s own `Theme` colour fields)
/// built on top of this.
pub fn merge_overrides<T, F>(
    current: T,
    keys: Vec<String>,
    mut value_for: F,
) -> Result<T, OverrideError>
where
    T: Serialize + DeserializeOwned,
    F: FnMut(&str) -> Result<serde_json::Value, String>,
{
    let mut value =
        serde_json::to_value(&current).map_err(|err| OverrideError::Internal(err.to_string()))?;
    let obj = value
        .as_object_mut()
        .ok_or_else(|| OverrideError::Internal("expected a JSON object".to_string()))?;
    for key in keys {
        if !obj.contains_key(&key) {
            return Err(OverrideError::UnknownField(key));
        }
        let json = value_for(&key).map_err(OverrideError::InvalidValue)?;
        obj.insert(key, json);
    }
    serde_json::from_value(value).map_err(|err| OverrideError::Invalid(err.to_string()))
}

/// [`merge_overrides`]'s one [`Theme`]-specific convenience: when `value` is
/// a JSON string, treats it as a `"#rrggbb"`/`"#rrggbbaa"` hex colour and
/// converts it to the JSON shape [`quadraui::Color`]'s own `Deserialize`
/// expects, rather than requiring every `Theme` colour override to spell
/// out the `{"r":.., "g":.., "b":.., "a":..}` dict (or `[r, g, b[, a]]`
/// list/tuple) shape [`quadraui::Color`]'s derived `Deserialize` already
/// accepts on its own. Any other JSON shape passes through unchanged.
pub fn theme_color_value(key: &str, value: serde_json::Value) -> Result<serde_json::Value, String> {
    if let serde_json::Value::String(hex) = &value {
        let color = quadraui::Color::from_hex(hex).ok_or_else(|| {
            format!("{key:?}: {hex:?} is not a valid \"#rrggbb\"/\"#rrggbbaa\" hex colour")
        })?;
        return serde_json::to_value(color).map_err(|err| err.to_string());
    }
    Ok(value)
}

/// Snake-cased name for a [`quadraui::ColorDepth`] variant — the vocabulary
/// a binding's own `backend_caps`-style surface reports under
/// `"color_depth"`.
pub fn color_depth_name(depth: quadraui::ColorDepth) -> &'static str {
    match depth {
        quadraui::ColorDepth::Ansi16 => "ansi16",
        quadraui::ColorDepth::Indexed256 => "indexed256",
        quadraui::ColorDepth::TrueColor => "true_color",
    }
}

/// Decodes a friendly lowercase word into a [`quadraui::DialogSeverity`] —
/// the vocabulary a binding's own `show_message_dialog`-style surface
/// accepts for its `severity` argument. `Err` carries the same message a
/// binding's own "unknown severity" error wraps.
pub fn dialog_severity(value: &str) -> Result<quadraui::DialogSeverity, String> {
    match value {
        "info" => Ok(quadraui::DialogSeverity::Info),
        "question" => Ok(quadraui::DialogSeverity::Question),
        "warning" => Ok(quadraui::DialogSeverity::Warning),
        "error" => Ok(quadraui::DialogSeverity::Error),
        other => Err(format!(
            "unknown severity {other:?}; expected one of \"info\", \"question\", \"warning\", \
             \"error\""
        )),
    }
}

/// Decodes a friendly lowercase word into a [`quadraui::ToastCorner`] — the
/// vocabulary a binding's own `show_toast`-style surface accepts for its
/// `corner` argument, the same friendly-lowercase-word shape
/// [`dialog_severity`] decodes `severity` with.
pub fn toast_corner(value: &str) -> Result<quadraui::ToastCorner, String> {
    match value {
        "bottom_right" => Ok(quadraui::ToastCorner::BottomRight),
        "bottom_left" => Ok(quadraui::ToastCorner::BottomLeft),
        "top_right" => Ok(quadraui::ToastCorner::TopRight),
        "top_left" => Ok(quadraui::ToastCorner::TopLeft),
        other => Err(format!(
            "unknown corner {other:?}; expected one of \"bottom_right\", \"bottom_left\", \
             \"top_right\", \"top_left\""
        )),
    }
}

/// Decodes a friendly lowercase word into a [`quadraui::FontRole`] — the
/// vocabulary a binding's own `measure_text`-style surface accepts for its
/// `role` argument.
pub fn font_role(value: &str) -> Result<quadraui::FontRole, String> {
    match value {
        "chrome" => Ok(quadraui::FontRole::Chrome),
        "editor" => Ok(quadraui::FontRole::Editor),
        other => Err(format!(
            "unknown role {other:?}; expected \"chrome\" or \"editor\""
        )),
    }
}

/// Decodes a friendly lowercase word into a [`quadraui::EditorCursorShape`]
/// — the vocabulary a binding's own `set_caret_shape`-style surface accepts.
pub fn caret_shape(value: &str) -> Result<quadraui::EditorCursorShape, String> {
    match value {
        "block" => Ok(quadraui::EditorCursorShape::Block),
        "bar" => Ok(quadraui::EditorCursorShape::Bar),
        "underline" => Ok(quadraui::EditorCursorShape::Underline),
        other => Err(format!(
            "unknown shape {other:?}; expected \"block\", \"bar\", or \"underline\""
        )),
    }
}

/// Decodes a friendly lowercase word into a [`quadraui::ResizeEdge`] — the
/// vocabulary [`pointer_shape`]'s `edge` argument accepts for the `"resize"`
/// shape.
pub fn resize_edge(value: &str) -> Result<quadraui::ResizeEdge, String> {
    match value {
        "north" => Ok(quadraui::ResizeEdge::North),
        "south" => Ok(quadraui::ResizeEdge::South),
        "east" => Ok(quadraui::ResizeEdge::East),
        "west" => Ok(quadraui::ResizeEdge::West),
        "north_east" => Ok(quadraui::ResizeEdge::NorthEast),
        "north_west" => Ok(quadraui::ResizeEdge::NorthWest),
        "south_east" => Ok(quadraui::ResizeEdge::SouthEast),
        "south_west" => Ok(quadraui::ResizeEdge::SouthWest),
        other => Err(format!(
            "unknown edge {other:?}; expected one of \"north\", \"south\", \"east\", \"west\", \
             \"north_east\", \"north_west\", \"south_east\", \"south_west\""
        )),
    }
}

/// Decodes a friendly lowercase word (plus, for `"resize"` only, an `edge`
/// decoded via [`resize_edge`]) into a [`quadraui::PointerShape`] — the
/// vocabulary a binding's own `set_cursor`-style surface accepts. `edge` is
/// required when `value` is `"resize"` and ignored otherwise.
pub fn pointer_shape(value: &str, edge: Option<&str>) -> Result<quadraui::PointerShape, String> {
    match value {
        "default" => Ok(quadraui::PointerShape::Default),
        "resize" => {
            let edge = edge.ok_or_else(|| "shape \"resize\" requires an edge".to_string())?;
            resize_edge(edge).map(quadraui::PointerShape::Resize)
        }
        "text" => Ok(quadraui::PointerShape::Text),
        "pointer" => Ok(quadraui::PointerShape::Pointer),
        "grab" => Ok(quadraui::PointerShape::Grab),
        "grabbing" => Ok(quadraui::PointerShape::Grabbing),
        "crosshair" => Ok(quadraui::PointerShape::Crosshair),
        "wait" => Ok(quadraui::PointerShape::Wait),
        "not_allowed" => Ok(quadraui::PointerShape::NotAllowed),
        "col_resize" => Ok(quadraui::PointerShape::ColResize),
        "row_resize" => Ok(quadraui::PointerShape::RowResize),
        other => Err(format!(
            "unknown shape {other:?}; expected one of \"default\", \"resize\", \"text\", \
             \"pointer\", \"grab\", \"grabbing\", \"crosshair\", \"wait\", \"not_allowed\", \
             \"col_resize\", \"row_resize\""
        )),
    }
}

/// Decodes a friendly lowercase word into a [`quadraui::AppPathKind`] — the
/// vocabulary a binding's own `app_path`/`resolve_app_path`-style surface
/// accepts, the same friendly-lowercase-word shape [`dialog_severity`]
/// decodes `severity` with.
pub fn app_path_kind(value: &str) -> Result<quadraui::AppPathKind, String> {
    match value {
        "user_data" => Ok(quadraui::AppPathKind::UserData),
        "app_config" => Ok(quadraui::AppPathKind::AppConfig),
        "app_cache" => Ok(quadraui::AppPathKind::AppCache),
        "logs" => Ok(quadraui::AppPathKind::Logs),
        "temp" => Ok(quadraui::AppPathKind::Temp),
        "downloads" => Ok(quadraui::AppPathKind::Downloads),
        "documents" => Ok(quadraui::AppPathKind::Documents),
        "home" => Ok(quadraui::AppPathKind::Home),
        other => Err(format!(
            "unknown path kind {other:?}; expected one of \"user_data\", \"app_config\", \
             \"app_cache\", \"logs\", \"temp\", \"downloads\", \"documents\", \"home\""
        )),
    }
}

/// Decodes a friendly lowercase word into a [`quadraui::PowerSaveKind`] —
/// the vocabulary a binding's own `begin_power_save_block`-style surface
/// accepts.
pub fn power_save_kind(value: &str) -> Result<quadraui::PowerSaveKind, String> {
    match value {
        "prevent_display_sleep" => Ok(quadraui::PowerSaveKind::PreventDisplaySleep),
        "prevent_app_suspension" => Ok(quadraui::PowerSaveKind::PreventAppSuspension),
        other => Err(format!(
            "unknown power save kind {other:?}; expected one of \
             \"prevent_display_sleep\", \"prevent_app_suspension\""
        )),
    }
}

/// Applies `f` to `backend`'s [`quadraui::WindowControl`] surface, folding
/// [`Backend::window`] returning `None` (no window at all, or not
/// constructed yet) into the same [`quadraui::BackendError::Unsupported`] a
/// missing method on a real window already reports — the same fold
/// `quadraui/examples/common/window_control_demo.rs`'s `describe` helper
/// uses, so a caller sees one uniform error shape for either case. Shared
/// by every binding's `set_title`/`set_document_edited`/
/// `set_represented_filename` surface rather than copied per leaf.
pub fn with_window<T>(
    backend: &mut dyn Backend,
    f: impl FnOnce(&mut dyn quadraui::WindowControl) -> quadraui::ServiceResult<T>,
) -> quadraui::ServiceResult<T> {
    match backend.window() {
        Some(window) => f(window),
        None => Err(quadraui::BackendError::Unsupported),
    }
}

/// [`with_window`]'s [`quadraui::backend::TrayService`] counterpart — folds
/// [`Backend::tray`] returning `None` into
/// [`quadraui::BackendError::Unsupported`]. Shared by every binding's
/// `set_tray_icon`/`set_tray_tooltip`/`set_tray_menu` surface.
pub fn with_tray<T>(
    backend: &mut dyn Backend,
    f: impl FnOnce(&mut dyn quadraui::backend::TrayService) -> quadraui::ServiceResult<T>,
) -> quadraui::ServiceResult<T> {
    match backend.tray() {
        Some(tray) => f(tray),
        None => Err(quadraui::BackendError::Unsupported),
    }
}

/// Builds a [`quadraui::ContextMenu`] from a flat `items` list for
/// attaching to a non-primitive menu surface — a tray icon's menu or a
/// Dock icon's menu — neither of which has its own on-screen anchor
/// point/selection state the way a right-click [`quadraui::ContextMenu`]
/// does. `id` names the surface (`"qui-tray-menu"`, `"qui-dock-menu"`) so
/// a [`quadraui::UiEvent::ContextMenuItemActivated`] fired from it is
/// still traceable back to its source; `selected_idx` is seeded at the
/// first selectable item the same way a freshly opened right-click menu
/// starts, even though neither surface actually renders a selection
/// highlight today.
pub fn menu_for_surface(
    id: WidgetId,
    items: Vec<quadraui::ContextMenuItem>,
) -> quadraui::ContextMenu {
    let mut menu = quadraui::ContextMenu {
        id,
        items,
        selected_idx: 0,
        bg: None,
        placement: quadraui::ContextMenuPlacement::AnchorPoint,
        glyphs: Default::default(),
    };
    menu.selected_idx = menu.first_selectable();
    menu
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    use quadraui::backend::MessageDialogOptions;
    use quadraui::tui::testing::TuiDriver;
    use quadraui::tui::TuiBackend;
    use quadraui::DrawsPrimitives;
    use quadraui::{
        AppLogic, Backend, Button, ButtonMask, Point, Reaction, Rect, TextInput, Toast,
        ToastSeverity, UiEvent,
    };

    use super::*;
    use crate::engine::EventSink;
    use crate::layout::{
        LayoutContainer, LayoutLeaf, LayoutNode, LayoutSplit, LayoutTabEntry, LeafDescriptor,
    };
    use crate::render::{collect_leaf_tooltips, draw_bands, paint_layout};
    use quadraui::Spacing;

    /// A [`BindingHost::widgets`] map holding just `button` under its
    /// `"button"` key — the test fixtures' own shorthand for the single
    /// slot [`BindingHost`] carries today, now that `Button` is a generic
    /// [`crate::widget_kinds::WIDGET_KINDS`] entry rather than its own
    /// typed field.
    fn button_widgets(button: Button) -> HashMap<String, Rc<dyn quadraui::primitive::DynWidget>> {
        let mut widgets = HashMap::new();
        widgets.insert(
            "button".to_string(),
            Rc::new(button) as Rc<dyn quadraui::primitive::DynWidget>,
        );
        widgets
    }

    #[derive(Default)]
    struct RecordingSink;

    impl EventSink for RecordingSink {
        fn dispatch(
            &mut self,
            _event: &UiEvent,
            _hit_detail: Option<&crate::hit::HitDetail>,
        ) -> Reaction {
            Reaction::Continue
        }
    }

    /// A minimal [`AppLogic`] whose `render` goes through [`BindingHost::widgets`]
    /// and [`render_overlays`] exactly the way a real binding's own thin
    /// `AppLogic` leaf would.
    struct HostLogic {
        host: BindingHost,
        engine: InteractionEngine,
        sink: RefCell<RecordingSink>,
    }

    impl AppLogic for HostLogic {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            let mut hits = Vec::new();
            self.engine
                .paint_with_widget_state(|| draw_bands(backend, &self.host.widgets, &mut hits));
            render_overlays(&self.engine, backend, hits);
        }

        fn extra_tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
            self.engine.tab_stops()
        }

        fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
            self.engine
                .handle(event, backend, &mut *self.sink.borrow_mut())
        }
    }

    /// [`render_overlays`] writes the anchored-overlay paint order (every
    /// `sync_*`/`render_*` call, dialog before tooltip before toast) down
    /// in one place instead of copy-pasting it per binding render path.
    /// Opens a toast and a message dialog together — two independent,
    /// non-overlapping overlays — and renders through a single
    /// `render_overlays` call, asserting both still show up: the one call
    /// now covers every overlay kind the two former duplicate blocks did.
    #[test]
    fn render_overlays_paints_every_registered_overlay_in_one_call() {
        let logic = HostLogic {
            host: BindingHost {
                widgets: button_widgets(Button::new(WidgetId::new("go"), "Go")),
                ..BindingHost::default()
            },
            engine: InteractionEngine::default(),
            sink: RefCell::new(RecordingSink),
        };
        let mut driver = TuiDriver::new(logic, 40, 20);

        let (backend, app) = driver.parts_mut();
        app.engine.show_toast(
            Toast {
                id: WidgetId::new("toast"),
                title: "Saved".to_string(),
                body: String::new(),
                severity: ToastSeverity::Info,
                actions: Vec::new(),
                accent: None,
            },
            quadraui::ToastCorner::BottomRight,
        );
        app.engine.open_message_dialog(
            MessageDialogOptions {
                title: "Heads up".to_string(),
                body: "Something happened.".to_string(),
                buttons: Vec::new(),
                severity: None,
            },
            backend,
        );

        driver.render();

        assert!(
            driver.screen_contains("Heads up"),
            "the message dialog must still paint"
        );
        assert!(
            driver.screen_contains("Saved"),
            "the toast opened alongside the dialog must still paint through \
             the same render_overlays call"
        );
    }

    /// A layout dialog with no buttons renders -- its content subtree is
    /// the only thing a caller needs to resolve it -- and closing it by
    /// the same path every other overlay kind closes through reports it
    /// was open and removes it from the next frame. Decodes the
    /// descriptor through [`crate::wire::decode_layout_dialog`] (rather
    /// than building [`quadraui::Dialog`] by struct literal) so the test
    /// covers the whole chain a `show_overlay("layout_dialog", ...)`
    /// caller with no `"buttons"` key actually goes through.
    #[test]
    fn a_layout_dialog_with_no_buttons_renders_and_closes() {
        use serde_json::json;

        use crate::wire::{decode_layout_dialog, DecodeDiagnostics, StyleContext};

        let logic = HostLogic {
            host: BindingHost::default(),
            engine: InteractionEngine::default(),
            sink: RefCell::new(RecordingSink),
        };
        let mut driver = TuiDriver::new(logic, 40, 20);

        let descriptor = json!({
            "dialog": {
                "id": "dialog:prefs",
                "title": "Preferences",
            },
            "content": {"id": "go", "button": {"id": "go", "label": "Go"}},
        });
        let decoded = decode_layout_dialog(
            "test",
            &descriptor,
            StyleContext::new(&StyleSets::new(), &Theme::default()),
            &DecodeDiagnostics::warn(),
        )
        .expect("a layout dialog with no buttons decodes");

        {
            let (backend, app) = driver.parts_mut();
            app.engine.open_layout_dialog(
                decoded.chrome,
                decoded.content,
                crate::engine::DialogContentSize::default(),
                backend,
            );
        }
        driver.render();
        assert!(
            driver.screen_contains("Preferences"),
            "the dialog chrome must paint with no buttons"
        );
        assert!(
            driver.screen_contains("Go"),
            "the content subtree must still paint in the dialog's body slot"
        );

        let (backend, app) = driver.parts_mut();
        assert!(
            app.engine.close_layout_dialog(backend),
            "closing must report an overlay was actually open"
        );
        driver.render();
        assert!(
            !driver.screen_contains("Preferences"),
            "the dialog must no longer paint once closed"
        );
    }

    /// A `Split`'s ratio persists after a divider drag — the same
    /// `apply_split_divider_drag`/`resolve_split_ratio` hook
    /// `InteractionEngine::handle` already applies for every binding's own
    /// `UiEvent::SplitDividerDragged` — and [`widget_state_snapshot`]
    /// reports the dragged ratio back to app code, not the descriptor's
    /// original one.
    #[test]
    fn widget_state_snapshot_reports_a_split_ratio_persisted_after_a_drag() {
        let engine = InteractionEngine::new();
        let widget_state = engine.widget_state();
        let id = WidgetId::new("split:main");
        let split = Split {
            id,
            direction: quadraui::SplitDirection::Horizontal,
            ratio: 0.3,
            first_min: 0.0,
            second_min: 0.0,
        };
        // The first frame resolves the descriptor's own starting ratio.
        resolve_split_ratio(widget_state, &split);
        // A drag persists a new ratio.
        apply_split_divider_drag(widget_state, id, 0, 0.7);

        let snapshot = widget_state_snapshot(&engine, id).expect("a ratio must be stored for id");
        assert_eq!(snapshot.ratio, Some(0.7));

        // The next frame's descriptor still sends the OLD ratio (the same
        // literal the app always has) -- uncontrolled, so the dragged
        // value keeps winning.
        let resolved = resolve_split_ratio(widget_state, &split);
        assert_eq!(resolved.ratio, 0.7);
    }

    /// An `id` no widget painted under and no split ratio was stored for
    /// has no state of any kind — [`widget_state_snapshot`] must say so
    /// plainly (`None`) rather than returning a snapshot of all-`None`
    /// fields an app could mistake for "resolved, but empty".
    #[test]
    fn widget_state_snapshot_is_none_for_an_id_never_resolved() {
        let engine = InteractionEngine::new();
        assert!(widget_state_snapshot(&engine, WidgetId::new("never-rendered")).is_none());
    }

    /// A minimal [`AppLogic`] painting a single two-pane `Split` through a
    /// decoded [`LayoutNode`] tree, resolving the WHOLE tree through
    /// [`resolve_layout_node`] before every render — the generic,
    /// render-callback-path counterpart of `engine::tests::SplitLogic`
    /// (which calls `resolve_split_ratio` directly on a bare `Split`, not
    /// through a decoded tree). `split` is the raw descriptor an app would
    /// hand a binding, never mutated by a drag.
    struct ResolvedSplitLogic {
        split: Split,
        engine: InteractionEngine,
        sink: RefCell<RecordingSink>,
    }

    impl AppLogic for ResolvedSplitLogic {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            let viewport = backend.viewport();
            let area = Rect::new(0.0, 0.0, viewport.width, viewport.height);
            let mut node = LayoutNode::Split(LayoutSplit {
                split: self.split.clone(),
                first: Box::new(LayoutNode::Leaf(LayoutLeaf {
                    id: "left".to_string(),
                    descriptor: LeafDescriptor::Widget {
                        kind: "button",
                        widget: Rc::new(Button::new(WidgetId::new("left-btn"), "L")),
                    },
                })),
                second: Box::new(LayoutNode::Leaf(LayoutLeaf {
                    id: "right".to_string(),
                    descriptor: LeafDescriptor::Widget {
                        kind: "button",
                        widget: Rc::new(Button::new(WidgetId::new("right-btn"), "R")),
                    },
                })),
                spacing: Spacing::default(),
            });
            resolve_layout_node(&self.engine, backend, area, &mut node);
            let mut hits = Vec::new();
            paint_layout(backend, area, &node, &mut hits);
            self.engine.set_hits(hits);
        }

        fn extra_tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
            self.engine.tab_stops()
        }

        fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
            self.engine
                .handle(event, backend, &mut *self.sink.borrow_mut())
        }
    }

    /// Dragging a two-pane `Split`'s divider persists the resolved ratio
    /// through [`resolve_layout_node`]'s generic `LayoutNode::Split`
    /// recursion — not just [`resolve_split_ratio`] called directly, which
    /// `engine::tests::dragging_a_split_divider_persists_the_ratio_into_the_widget_state_store`
    /// already covers — and [`widget_state_snapshot`] reports the dragged
    /// ratio back to app code afterward.
    #[test]
    fn resolve_layout_node_persists_a_split_ratio_after_a_drag() {
        let split = Split {
            id: WidgetId::new("split"),
            direction: quadraui::SplitDirection::Horizontal,
            ratio: 0.5,
            first_min: 0.0,
            second_min: 0.0,
        };
        let logic = ResolvedSplitLogic {
            split,
            engine: InteractionEngine::new(),
            sink: RefCell::new(RecordingSink),
        };
        let mut driver = TuiDriver::new(logic, 40, 10);
        driver.render();

        let divider_bounds = {
            let hits = driver.app().engine.hit_map();
            hits.iter()
                .find_map(|hit| match hit {
                    PaintedHit::Split { layout } => Some(layout.divider_bounds),
                    _ => None,
                })
                .expect("the split should have painted a divider")
        };
        let divider_point = Point::new(
            divider_bounds.x,
            divider_bounds.y + divider_bounds.height / 2.0,
        );

        driver.dispatch(UiEvent::MouseDown {
            widget: None,
            button: quadraui::MouseButton::Left,
            position: divider_point,
            modifiers: Modifiers::default(),
        });
        driver.dispatch(UiEvent::MouseMoved {
            position: Point::new(divider_point.x + 15.0, divider_point.y),
            buttons: quadraui::ButtonMask {
                left: true,
                right: false,
                middle: false,
            },
        });

        // `apply_split_divider_drag` persists the drag inside
        // `InteractionEngine::handle` itself, before `sink.dispatch` even
        // runs (see that function's own doc) -- so the widget-state store
        // already reflects it immediately, with no need to inspect what
        // `RecordingSink` (a no-op here) did with the dispatched event.
        let dragged_ratio = widget_state_snapshot(&driver.app().engine, WidgetId::new("split"))
            .and_then(|snapshot| snapshot.ratio)
            .expect("the drag must have persisted a split_ratio for id");
        assert_ne!(
            dragged_ratio, 0.5,
            "the drag must have actually moved the ratio away from its starting 0.5"
        );

        // The app's own `Split` descriptor is never touched by the drag —
        // this render still hands `resolve_layout_node` the exact same
        // `ratio: 0.5` it always has.
        driver.render();
        let resolved_ratio = {
            let hits = driver.app().engine.hit_map();
            hits.iter()
                .find_map(|hit| match hit {
                    PaintedHit::Split { layout } => Some(layout.resolved_ratio),
                    _ => None,
                })
                .expect("the split should still have painted a divider")
        };
        assert!(
            (resolved_ratio - dragged_ratio).abs() < 0.001,
            "the dragged ratio must survive a render whose descriptor still says 0.5: got {resolved_ratio}, expected {dragged_ratio}"
        );

        let snapshot = widget_state_snapshot(&driver.app().engine, WidgetId::new("split"))
            .expect("the split's id must have resolved state");
        assert!(
            (snapshot.ratio.expect("ratio must be set") - dragged_ratio).abs() < 0.001,
            "the snapshot must report the same dragged ratio back to app code"
        );
    }

    /// An [`AppLogic`] whose `render` resolves then paints a
    /// `LayoutNode::Scroll` hosting a column of buttons -- drives
    /// [`resolve_layout_node`]'s own `LayoutNode::Scroll` arm through a
    /// real [`TuiDriver`] frame, the way a binding's own `render` would
    /// (resolve, then paint, never paint alone).
    struct ScrollLeafLogic {
        button_count: usize,
        offset: Option<f32>,
        engine: InteractionEngine,
    }

    impl AppLogic for ScrollLeafLogic {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            let viewport = backend.viewport();
            let area = Rect::new(0.0, 0.0, viewport.width, viewport.height);
            let content = LayoutNode::Column(LayoutContainer::new(
                (0..self.button_count)
                    .map(|i| {
                        let label = i.to_string();
                        LayoutNode::Leaf(LayoutLeaf {
                            id: label.clone(),
                            descriptor: LeafDescriptor::Widget {
                                kind: "button",
                                widget: Rc::new(Button::new(WidgetId::new(label.clone()), label)),
                            },
                        })
                    })
                    .collect(),
                Default::default(),
            ));
            let mut node = LayoutNode::Scroll(crate::layout::LayoutScroll {
                id: "scroll".to_string(),
                axis: quadraui::ScrollAxis::Vertical,
                offset: self.offset,
                content: Box::new(content),
                spacing: Default::default(),
            });
            resolve_layout_node(&self.engine, backend, area, &mut node);
            let mut hits = Vec::new();
            paint_layout(backend, area, &node, &mut hits);
            self.engine.set_hits(hits);
        }

        fn handle(&mut self, _event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
            Reaction::Continue
        }
    }

    fn painted_button_ids(hits: &[PaintedHit]) -> Vec<String> {
        hits.iter()
            .filter_map(|hit| match hit {
                PaintedHit::Widget { id, widget, .. } if widget.as_any().is::<Button>() => {
                    Some(id.as_str().to_string())
                }
                _ => None,
            })
            .collect()
    }

    /// A controlled offset resolved through [`resolve_layout_node`] (not
    /// just [`crate::render::paint_layout`] called alone, as
    /// `crate::render::tests` already covers) still paints exactly the
    /// ten buttons the issue's own acceptance scenario names.
    #[test]
    fn resolve_layout_node_scroll_paints_the_ten_buttons_at_a_controlled_offset() {
        let logic = ScrollLeafLogic {
            button_count: 40,
            offset: Some(20.0),
            engine: InteractionEngine::new(),
        };
        let mut driver = TuiDriver::new(logic, 20, 10);
        driver.render();
        let hits = driver.app().engine.hit_map();
        let want: Vec<String> = (20..30).map(|i| i.to_string()).collect();
        assert_eq!(painted_button_ids(&hits), want);
    }

    /// An uncontrolled (`None`) offset resolves to `0.0` the first frame,
    /// and — unlike a bare [`crate::render::paint_layout`] call with no
    /// `resolve_layout_node` pass first — persists across a further
    /// uncontrolled render rather than being swept by
    /// [`quadraui::WidgetStateStore::gc`], mirroring
    /// [`resolve_layout_node_keys_a_leaf_whose_descriptor_id_differs_from_its_own_on_the_outer_leaf_id`]'s
    /// own "survives a further render" assertion for a `Tree` leaf.
    #[test]
    fn resolve_layout_node_scroll_uncontrolled_offset_persists_across_renders() {
        let logic = ScrollLeafLogic {
            button_count: 40,
            offset: None,
            engine: InteractionEngine::new(),
        };
        let mut driver = TuiDriver::new(logic, 20, 10);
        driver.render();
        let want: Vec<String> = (0..10).map(|i| i.to_string()).collect();
        assert_eq!(painted_button_ids(&driver.app().engine.hit_map()), want);

        driver.render();
        assert_eq!(
            painted_button_ids(&driver.app().engine.hit_map()),
            want,
            "an uncontrolled scroll offset must survive a further render unchanged"
        );
    }

    /// [`ScrollLeafLogic`]'s own scenario, but with `content` wrapped in a
    /// `LayoutNode::Styled` (a `"style"` key on the `"scroll"` node's
    /// `"content"` dict) -- a purely cosmetic `"style"` sibling must not
    /// change the scroll's own structural behaviour: `resolve_layout_node`
    /// must still resolve each child's own offset against the wrapped
    /// `Column`'s children (not fall back to the atomic "whole subtree is
    /// one leaf" path), `scroll_content_slots` must still report one slot
    /// per child, and the painted button whose own `style` overrides `fg`
    /// must show that colour. Covers the review's own "a `Styled` wrapper
    /// is invisible to the `"scroll"` path's structural matches" finding.
    struct StyledScrollContentLogic {
        button_count: usize,
        offset: Option<f32>,
        engine: InteractionEngine,
    }

    impl AppLogic for StyledScrollContentLogic {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            let viewport = backend.viewport();
            let area = Rect::new(0.0, 0.0, viewport.width, viewport.height);
            let content = LayoutNode::Column(LayoutContainer::new(
                (0..self.button_count)
                    .map(|i| {
                        let label = i.to_string();
                        let leaf = LayoutNode::Leaf(LayoutLeaf {
                            id: label.clone(),
                            descriptor: LeafDescriptor::Widget {
                                kind: "button",
                                widget: Rc::new(Button::new(WidgetId::new(label.clone()), label)),
                            },
                        });
                        if i == 0 {
                            LayoutNode::Styled(crate::layout::LayoutStyled {
                                style: Box::new(crate::layout::StyleOverride {
                                    fg: Some(quadraui::Color::rgb(0, 255, 0)),
                                    ..Default::default()
                                }),
                                node: Box::new(leaf),
                            })
                        } else {
                            leaf
                        }
                    })
                    .collect(),
                Default::default(),
            ));
            // Wrap the whole content in its own `"style"` override too --
            // a cosmetic `bg` that must never flip `scroll_content_slots`
            // from `Children` to `Atomic`.
            let styled_content = LayoutNode::Styled(crate::layout::LayoutStyled {
                style: Box::new(crate::layout::StyleOverride {
                    bg: Some(quadraui::Color::rgb(255, 0, 0)),
                    ..Default::default()
                }),
                node: Box::new(content),
            });
            let mut node = LayoutNode::Scroll(crate::layout::LayoutScroll {
                id: "scroll".to_string(),
                axis: quadraui::ScrollAxis::Vertical,
                offset: self.offset,
                content: Box::new(styled_content),
                spacing: Default::default(),
            });
            resolve_layout_node(&self.engine, backend, area, &mut node);
            let mut hits = Vec::new();
            paint_layout(backend, area, &node, &mut hits);
            self.engine.set_hits(hits);
        }

        fn handle(&mut self, _event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
            Reaction::Continue
        }
    }

    /// A `"style"` key on a scroll's `content` node must not change which
    /// buttons paint at a given offset -- the same ten-of-forty window
    /// [`resolve_layout_node_scroll_paints_the_ten_buttons_at_a_controlled_offset`]
    /// asserts for an unstyled `Column`.
    #[test]
    fn resolve_layout_node_scroll_paints_the_ten_buttons_when_content_is_styled() {
        let logic = StyledScrollContentLogic {
            button_count: 40,
            offset: Some(20.0),
            engine: InteractionEngine::new(),
        };
        let mut driver = TuiDriver::new(logic, 20, 10);
        driver.render();
        let hits = driver.app().engine.hit_map();
        let want: Vec<String> = (20..30).map(|i| i.to_string()).collect();
        assert_eq!(
            painted_button_ids(&hits),
            want,
            "a styled scroll content must resolve/paint the same per-child \
             slots as an unstyled one"
        );
    }

    /// A styled scroll's own `PaintedHit::Scroll::children` must still be
    /// populated per-child -- a `LayoutNode::Styled` wrapper around the
    /// content `Column` must not drop `ScrollSlots::Children` down to
    /// `ScrollSlots::Atomic`, which would report an empty `children` list
    /// (`crate::render::paint_scroll`'s own `ScrollChildHit` match).
    #[test]
    fn resolve_layout_node_scroll_children_hits_survive_a_styled_content_wrapper() {
        let logic = StyledScrollContentLogic {
            button_count: 5,
            offset: Some(0.0),
            engine: InteractionEngine::new(),
        };
        let mut driver = TuiDriver::new(logic, 20, 10);
        driver.render();
        let hits = driver.app().engine.hit_map();
        let scroll_children = hits.iter().find_map(|hit| match hit {
            PaintedHit::Scroll { children, .. } => Some(children.clone()),
            _ => None,
        });
        assert_eq!(
            scroll_children.map(|c| c.len()),
            Some(5),
            "a styled scroll content column must still report one \
             ScrollChildHit per child"
        );
    }

    /// The styled first button's own `"fg"` override must still paint --
    /// proof [`crate::render::paint_scroll`]'s own `push_styled_scope`
    /// call actually brackets the per-child paint, not just the atomic
    /// fallback path.
    #[test]
    fn resolve_layout_node_scroll_content_child_style_override_still_paints() {
        let logic = StyledScrollContentLogic {
            button_count: 5,
            offset: Some(0.0),
            engine: InteractionEngine::new(),
        };
        let mut driver = TuiDriver::new(logic, 20, 10);
        driver.render();

        let (x, y) = driver.find("0").expect("button \"0\" should paint");
        let fg = driver
            .style_at(x as u16, y as u16)
            .expect("style at button 0")
            .fg;
        assert_eq!(
            fg,
            quadraui::tui::ratatui_color(quadraui::Color::rgb(0, 255, 0)),
            "button \"0\"'s own nested \"style\" override must still apply \
             when painted through a styled scroll content wrapper"
        );
    }

    /// An [`AppLogic`] whose `render` paints a focused, bordered
    /// [`TextInput`] across the whole viewport directly (not through a
    /// [`BindingHost`] slot — this test only needs its border painted,
    /// not its full leaf wiring) before calling [`render_overlays`], so a toast
    /// opened on top of it has to paint over real bordered chrome, not
    /// just empty space.
    struct BorderedEditorLogic {
        engine: InteractionEngine,
        sink: RefCell<RecordingSink>,
    }

    impl AppLogic for BorderedEditorLogic {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            let viewport = backend.viewport();
            let rect = Rect::new(0.0, 0.0, viewport.width, viewport.height);
            let mut editor = TextInput::new(WidgetId::new("editor"));
            editor.lines = vec!["fn main() {}".to_string()];
            editor.has_focus = true;
            backend.draw_dyn(rect, &editor, &());
            render_overlays(&self.engine, backend, Vec::new());
        }

        fn extra_tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
            Vec::new()
        }

        fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
            self.engine
                .handle(event, backend, &mut *self.sink.borrow_mut())
        }
    }

    /// The paint-order contract [`render_overlays`]'s own doc states —
    /// "dialog > float > menus > toast > tooltip" at the module level,
    /// toast last of all in this function's own body — must hold even
    /// when the base layer underneath is a *bordered, focused* editor,
    /// not empty space: a toast opened over it must still show its own
    /// text, not get painted over by the editor's own border/background.
    #[test]
    fn toast_paints_over_a_bordered_focused_editor() {
        let logic = BorderedEditorLogic {
            engine: InteractionEngine::default(),
            sink: RefCell::new(RecordingSink),
        };
        let mut driver = TuiDriver::new(logic, 40, 20);

        let (_backend, app) = driver.parts_mut();
        app.engine.show_toast(
            Toast {
                id: WidgetId::new("toast"),
                title: "Saved".to_string(),
                body: String::new(),
                severity: ToastSeverity::Info,
                actions: Vec::new(),
                accent: None,
            },
            quadraui::ToastCorner::BottomRight,
        );

        driver.render();

        assert!(
            driver.screen_contains("fn main"),
            "the editor's own content must still paint"
        );
        assert!(
            driver.screen_contains("Saved"),
            "the toast must paint over the bordered editor underneath it, \
             not get overwritten by its border/background"
        );
    }

    /// The paint-order contract [`render_overlays`]'s own doc states must
    /// hold for a tooltip too, not just a toast: shown over a bordered,
    /// focused editor, its own text must paint over that chrome — above
    /// the editor's focus ring and border, not underneath it.
    #[test]
    fn tooltip_paints_over_a_bordered_focused_editor() {
        let logic = BorderedEditorLogic {
            engine: InteractionEngine::default(),
            sink: RefCell::new(RecordingSink),
        };
        let mut driver = TuiDriver::new(logic, 40, 20);

        let (_backend, app) = driver.parts_mut();
        app.engine.show_tooltip(
            Tooltip::new(WidgetId::new("tip"), "Hint"),
            Rect::new(2.0, 2.0, 10.0, 1.0),
        );

        driver.render();

        assert!(
            driver.screen_contains("fn main"),
            "the editor's own content must still paint"
        );
        assert!(
            driver.screen_contains("Hint"),
            "the tooltip must paint over the bordered editor underneath it, \
             not get overwritten by its border/focus-ring chrome"
        );
    }

    /// An [`AppLogic`] wiring [`BindingHost::drive_hover_tooltip`] into
    /// real `MouseMoved` dispatch + `tick` — the shared-host equivalent of
    /// a native backend's own `hover_driven` test
    /// (`quadraui::macos::tooltip::tests::hover_driven`), proving
    /// `quadraui-binding-core` drives [`HoverTooltipTracker`] through a
    /// real [`BindingHost`]/[`InteractionEngine`] pair instead of each
    /// binding (and each binding's own test) reimplementing the wiring.
    /// `clock_ms` is a caller-driven millisecond counter, not a wall
    /// clock — the tracker's own "headless-clock contract" — so the test
    /// controls elapsed time exactly with no sleeping.
    ///
    /// Generic over the [`EventSink`] `S`: most tests use [`RecordingSink`]
    /// (always `Continue`), but
    /// [`drive_hover_tooltip_landing_frame_still_schedules_the_pending_wake_even_when_the_frame_also_redraws`]
    /// needs a sink that mimics a real binding's handler returning `Redraw`
    /// for the very `MouseEntered` that arms the pending tooltip — the
    /// exact same-frame collision [`BindingHost::drive_hover_tooltip`]'s
    /// own doc names.
    struct HoverHostLogic<S: EventSink> {
        host: BindingHost,
        engine: InteractionEngine,
        sink: RefCell<S>,
        clock_ms: Rc<Cell<u64>>,
        delay_ms: u64,
        auto_dismiss_ms: u64,
    }

    impl<S: EventSink> AppLogic for HoverHostLogic<S> {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            let mut hits = Vec::new();
            self.engine
                .paint_with_widget_state(|| draw_bands(backend, &self.host.widgets, &mut hits));
            render_overlays(&self.engine, backend, hits);
        }

        fn extra_tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
            self.engine.tab_stops()
        }

        fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
            let now = self.clock_ms.get();
            let reaction = self
                .engine
                .handle(event, backend, &mut *self.sink.borrow_mut());
            reaction.merge(self.host.drive_hover_tooltip(
                &mut self.engine,
                backend,
                now,
                self.delay_ms,
                self.auto_dismiss_ms,
            ))
        }

        fn tick(&mut self, backend: &mut dyn Backend) -> Reaction {
            let now = self.clock_ms.get();
            self.host.drive_hover_tooltip(
                &mut self.engine,
                backend,
                now,
                self.delay_ms,
                self.auto_dismiss_ms,
            )
        }
    }

    /// Drives [`BindingHost::drive_hover_tooltip`] through real
    /// `MouseMoved`/`tick` dispatch with a headless clock: the tooltip
    /// must not appear the instant the pointer lands, must appear once
    /// [`HoverHostLogic::delay_ms`] has elapsed with no further movement,
    /// and must hide again the moment the pointer leaves — the same
    /// three assertions a native backend's own `hover_driven` test makes,
    /// here proving `quadraui-binding-core` does the identical job for a
    /// `Button::tooltip` reached through [`BindingHost`].
    #[test]
    fn drive_hover_tooltip_shows_after_the_delay_and_hides_on_pointer_leave() {
        const DELAY_MS: u64 = 500;
        let clock = Rc::new(Cell::new(0u64));
        let mut button = Button::new(WidgetId::new("save"), "Save");
        button.tooltip = Some("Save the file".to_string());
        let logic = HoverHostLogic {
            host: BindingHost {
                widgets: button_widgets(button),
                ..BindingHost::default()
            },
            engine: InteractionEngine::default(),
            sink: RefCell::new(RecordingSink),
            clock_ms: clock.clone(),
            delay_ms: DELAY_MS,
            auto_dismiss_ms: 0,
        };
        let mut driver = TuiDriver::new(logic, 40, 20);
        driver.render();
        let (x, y) = driver
            .find("Save")
            .expect("the button's own label must be painted");

        // The pointer lands on the button -- still pending, no tooltip yet.
        driver.dispatch(UiEvent::MouseMoved {
            position: Point::new(x, y),
            buttons: ButtonMask::default(),
        });
        driver.render();
        assert!(
            !driver.screen_contains("Save the file"),
            "the tooltip must not show the instant the pointer lands"
        );

        // The delay elapses with no further pointer movement -- the
        // `tick()` a real runner's `RedrawAfter`-scheduled wake would have
        // triggered now shows the tooltip.
        clock.set(DELAY_MS);
        driver.tick();
        assert!(
            driver.screen_contains("Save the file"),
            "the tooltip must show once the hover delay has elapsed"
        );

        // The pointer leaves the button entirely -- the tooltip must
        // disappear immediately, no further delay.
        driver.dispatch(UiEvent::MouseMoved {
            position: Point::new(39.0, 19.0),
            buttons: ButtonMask::default(),
        });
        driver.render();
        assert!(
            !driver.screen_contains("Save the file"),
            "the tooltip must hide as soon as the pointer leaves its anchor"
        );
    }

    /// A click anywhere dismisses a visible tooltip through
    /// [`InteractionEngine::handle`]'s own generic auto-dismiss, which
    /// [`BindingHost::tooltip_tracker`] has no way to observe on its own
    /// (it only ever sees `on_hover`/`tick`, not engine-internal state).
    /// Without [`BindingHost::drive_hover_tooltip`]'s resync, the tracker
    /// would be stuck believing a tooltip is still visible for the rest
    /// of this continuous hover and never re-arm; with it, holding the
    /// pointer still over the same button past a further delay re-shows
    /// the tooltip exactly as if the hover had restarted.
    #[test]
    fn drive_hover_tooltip_resyncs_after_a_click_dismisses_the_tooltip() {
        const DELAY_MS: u64 = 500;
        let clock = Rc::new(Cell::new(0u64));
        let mut button = Button::new(WidgetId::new("save"), "Save");
        button.tooltip = Some("Save the file".to_string());
        let logic = HoverHostLogic {
            host: BindingHost {
                widgets: button_widgets(button),
                ..BindingHost::default()
            },
            engine: InteractionEngine::default(),
            sink: RefCell::new(RecordingSink),
            clock_ms: clock.clone(),
            delay_ms: DELAY_MS,
            auto_dismiss_ms: 0,
        };
        let mut driver = TuiDriver::new(logic, 40, 20);
        driver.render();
        let (x, y) = driver
            .find("Save")
            .expect("the button's own label must be painted");

        driver.dispatch(UiEvent::MouseMoved {
            position: Point::new(x, y),
            buttons: ButtonMask::default(),
        });
        clock.set(DELAY_MS);
        driver.tick();
        driver.render();
        assert!(
            driver.screen_contains("Save the file"),
            "the tooltip must be showing before the click"
        );

        // A click on the button itself (still hovering it) dismisses the
        // tooltip via `InteractionEngine::handle`'s own generic auto-dismiss.
        driver.dispatch(UiEvent::MouseDown {
            widget: None,
            button: quadraui::MouseButton::Left,
            position: Point::new(x, y),
            modifiers: quadraui::Modifiers::default(),
        });
        driver.render();
        assert!(
            !driver.screen_contains("Save the file"),
            "the click must dismiss the tooltip immediately"
        );

        // Still hovering the same button, unmoved, well past another full
        // delay -- without the resync, the tracker would still believe
        // the old (now-closed) tooltip is visible and never re-arm, so
        // this would stay hidden forever.
        clock.set(DELAY_MS * 10);
        driver.tick();
        driver.render();
        assert!(
            driver.screen_contains("Save the file"),
            "holding the pointer still over the same button after the \
             click dismissed its tooltip must eventually re-show it, not \
             stay stuck believing a tooltip is open that the screen no \
             longer has"
        );
    }

    /// A tooltip held visible past [`HoverHostLogic::auto_dismiss_ms`] with
    /// the pointer never moving off it must still hide itself —
    /// [`HoverTooltipTracker::tick_auto_dismiss`]'s own third dismissal
    /// path, wired through [`BindingHost::drive_hover_tooltip`] via
    /// [`quadraui::Style::tooltip_auto_dismiss_ms`] — and, with the
    /// pointer still held on the exact same anchor, must *stay* hidden
    /// rather than re-arming the delay and showing (then auto-dismissing)
    /// the same tooltip again every `delay_ms + auto_dismiss_ms`
    /// indefinitely.
    #[test]
    fn drive_hover_tooltip_auto_dismisses_after_the_timeout_with_the_pointer_held_still() {
        const DELAY_MS: u64 = 500;
        const AUTO_DISMISS_MS: u64 = 1_000;
        let clock = Rc::new(Cell::new(0u64));
        let mut button = Button::new(WidgetId::new("save"), "Save");
        button.tooltip = Some("Save the file".to_string());
        let logic = HoverHostLogic {
            host: BindingHost {
                widgets: button_widgets(button),
                ..BindingHost::default()
            },
            engine: InteractionEngine::default(),
            sink: RefCell::new(RecordingSink),
            clock_ms: clock.clone(),
            delay_ms: DELAY_MS,
            auto_dismiss_ms: AUTO_DISMISS_MS,
        };
        let mut driver = TuiDriver::new(logic, 40, 20);
        driver.render();
        let (x, y) = driver
            .find("Save")
            .expect("the button's own label must be painted");

        driver.dispatch(UiEvent::MouseMoved {
            position: Point::new(x, y),
            buttons: ButtonMask::default(),
        });
        clock.set(DELAY_MS);
        driver.tick();
        driver.render();
        assert!(
            driver.screen_contains("Save the file"),
            "the tooltip must be showing before the auto-dismiss timeout"
        );

        // Held still, well short of the auto-dismiss deadline -- stays shown.
        clock.set(DELAY_MS + AUTO_DISMISS_MS - 1);
        driver.tick();
        driver.render();
        assert!(
            driver.screen_contains("Save the file"),
            "the tooltip must not hide before its own timeout elapses"
        );

        // The pointer never moved, but the auto-dismiss deadline has now
        // passed -- the tooltip must hide itself with no further input.
        clock.set(DELAY_MS + AUTO_DISMISS_MS);
        driver.tick();
        driver.render();
        assert!(
            !driver.screen_contains("Save the file"),
            "a tooltip left open past tooltip_auto_dismiss_ms must hide \
             itself even though the pointer never left its anchor"
        );

        // Several more delay-plus-dismiss cycles pass with the pointer
        // still held on the very same button -- without a latch this
        // would re-show (then auto-dismiss) the tooltip every cycle
        // forever. It must stay hidden instead.
        for n in 1..=3u64 {
            clock.set(DELAY_MS + n * (DELAY_MS + AUTO_DISMISS_MS));
            driver.tick();
            driver.render();
            assert!(
                !driver.screen_contains("Save the file"),
                "cycle {n}: an auto-dismissed tooltip must not blink back \
                 on while the pointer keeps hovering the same anchor"
            );
        }

        // The pointer finally leaves and comes back -- the delay is free
        // to start over for a genuinely new hover.
        driver.dispatch(UiEvent::MouseMoved {
            position: Point::new(20.0, 10.0),
            buttons: ButtonMask::default(),
        });
        let resume_at = clock.get() + 1;
        clock.set(resume_at);
        driver.tick();
        driver.render();
        assert!(
            !driver.screen_contains("Save the file"),
            "leaving the anchor must not instantly show a new tooltip \
             either -- the delay still applies"
        );

        driver.dispatch(UiEvent::MouseMoved {
            position: Point::new(x, y),
            buttons: ButtonMask::default(),
        });
        clock.set(resume_at + DELAY_MS);
        driver.tick();
        driver.render();
        assert!(
            driver.screen_contains("Save the file"),
            "a fresh hover after leaving the anchor must show the tooltip \
             again once its own delay elapses"
        );
    }

    /// Mimics a real binding's registered handler: returns [`Reaction::Redraw`]
    /// for every `MouseEntered`/`MouseLeft` [`InteractionEngine::handle`]
    /// synthesizes, the same way a Python/JS callback that repaints its own
    /// hover-highlight state would — unlike [`RecordingSink`], which never
    /// does, and so never reproduces the same-frame collision this module's
    /// own test below exercises.
    #[derive(Default)]
    struct RedrawOnHoverChangeSink;

    impl EventSink for RedrawOnHoverChangeSink {
        fn dispatch(
            &mut self,
            event: &UiEvent,
            _hit_detail: Option<&crate::hit::HitDetail>,
        ) -> Reaction {
            match event {
                UiEvent::MouseEntered { .. } | UiEvent::MouseLeft { .. } => Reaction::Redraw,
                _ => Reaction::Continue,
            }
        }
    }

    /// Proves the fix for the exact collision [`BindingHost::drive_hover_tooltip`]'s
    /// own doc names: the pointer landing on a button both arms the
    /// pending-tooltip delay *and* triggers an unrelated `Redraw` (here,
    /// [`RedrawOnHoverChangeSink`]'s hover-highlight repaint) on the very
    /// same dispatch. Before the fix, [`Reaction::merge`] let that `Redraw`
    /// swallow the pending delay's own `RedrawAfter`, so the tooltip only
    /// ever showed once the backend's much coarser idle-poll ceiling
    /// happened to notice. Scheduling the wake directly on `backend` (not
    /// only through the returned [`Reaction`]) survives that merge.
    #[test]
    fn drive_hover_tooltip_landing_frame_still_schedules_the_pending_wake_even_when_the_frame_also_redraws(
    ) {
        const DELAY_MS: u64 = 500;
        let clock = Rc::new(Cell::new(0u64));
        let mut button = Button::new(WidgetId::new("save"), "Save");
        button.tooltip = Some("Save the file".to_string());
        let logic = HoverHostLogic {
            host: BindingHost {
                widgets: button_widgets(button),
                ..BindingHost::default()
            },
            engine: InteractionEngine::default(),
            sink: RefCell::new(RedrawOnHoverChangeSink),
            clock_ms: clock.clone(),
            delay_ms: DELAY_MS,
            auto_dismiss_ms: 0,
        };
        let mut driver = TuiDriver::new(logic, 40, 20);
        driver.render();
        let (x, y) = driver
            .find("Save")
            .expect("the button's own label must be painted");

        let reaction = driver.dispatch(UiEvent::MouseMoved {
            position: Point::new(x, y),
            buttons: ButtonMask::default(),
        });
        assert_eq!(
            reaction,
            Reaction::Redraw,
            "the landing frame's own hover-highlight repaint must still \
             return Redraw -- that's the collision being tested, not an \
             incidental setup detail"
        );
        assert!(
            driver.backend().frame_requests() > 0,
            "the pending tooltip's wake must be scheduled directly on the \
             backend even though this frame's merged Reaction is Redraw, \
             not Continue/RedrawAfter"
        );
        let armed = driver
            .backend()
            .pending_frame_delay()
            .expect("a wake must be armed for the still-pending tooltip");
        assert!(
            armed <= Duration::from_millis(DELAY_MS),
            "the armed wake must be no later than the remaining hover \
             delay, got {armed:?}"
        );
    }

    /// A hovered id with tooltip text but no [`PaintedHit::Widget`] anchor
    /// in this frame's hit map (a layout race, a mid-resize frame, or any
    /// other reason `render` hasn't repainted it yet) must never reach
    /// [`HoverTooltipTracker::visible_item`] — and, critically, repeated
    /// calls must not cycle through pending/visible-unpainted/dismissed
    /// forever for a pointer that never moved. Drives
    /// [`BindingHost::drive_hover_tooltip`] directly against a hand-built
    /// [`InteractionEngine`] rather than through [`HoverHostLogic`], so the
    /// anchor can be removed independently of the hover itself (which only
    /// [`UiEvent::MouseMoved`] re-dispatch would otherwise change).
    #[test]
    fn drive_hover_tooltip_never_reaches_visible_without_a_painted_anchor_and_does_not_oscillate() {
        const DELAY_MS: u64 = 500;
        let mut button = Button::new(WidgetId::new("save"), "Save");
        button.tooltip = Some("Save the file".to_string());
        let mut host = BindingHost {
            widgets: button_widgets(button),
            ..BindingHost::default()
        };
        let mut engine = InteractionEngine::default();
        let mut backend = TuiBackend::new();
        let mut sink = RecordingSink;

        let id = WidgetId::new("save");
        engine.set_hits(vec![PaintedHit::Widget {
            id,
            area: Rect::new(0.0, 0.0, 10.0, 1.0),
            widget: Rc::new(Button::new(id, "Save")),
        }]);
        engine.handle(
            UiEvent::MouseMoved {
                position: Point::new(1.0, 0.0),
                buttons: ButtonMask::default(),
            },
            &mut backend,
            &mut sink,
        );
        assert_eq!(
            engine.hovered().map(|(hovered_id, _)| hovered_id),
            Some(id),
            "the hover must land on the button while it's actually painted"
        );

        // The button's own paint vanishes from this frame's hit map while
        // `engine`'s cached hover still points at it.
        engine.set_hits(vec![]);

        for n in 0..5u64 {
            // Each call is well past the hover delay, so a painted anchor
            // would have let `tick` flip to `Visible` on the very first
            // one -- proving the guard, not just that not enough time
            // passed.
            let now_ms = DELAY_MS + n * DELAY_MS;
            let reaction = host.drive_hover_tooltip(&mut engine, &mut backend, now_ms, DELAY_MS, 0);
            assert_eq!(
                host.tooltip_tracker.visible_item(),
                None,
                "call {n}: a hovered id with no painted anchor must never \
                 show a tooltip"
            );
            assert!(
                host.tooltip_tracker.is_pending(),
                "call {n}: without an anchor the tracker must stay pending, \
                 not cycle through dismiss/re-arm"
            );
            assert_ne!(
                reaction,
                Reaction::Redraw,
                "call {n}: nothing actually changed on screen, so this must \
                 not force a redraw"
            );
            assert_eq!(
                backend.pending_frame_delay(),
                None,
                "call {n}: without an anchor, `next_wake_ms`'s Pending \
                 deadline has already elapsed, so arming a wake for it \
                 would be a zero-delay busy-spin re-armed every dispatch \
                 for as long as the hover stays stale -- the call must \
                 skip the wake entirely instead"
            );
        }
    }

    /// A minimal [`AppLogic`] whose `render` paints a layout tree through
    /// [`crate::render::paint_layout`] -- not [`BindingHost::widgets`]'
    /// flat bands -- and refreshes [`BindingHost::leaf_tooltips`] from
    /// it every frame via [`crate::render::collect_leaf_tooltips`], the
    /// same two calls `quadraui-py`'s own `PyAppLogic::render` makes for
    /// its `RenderOutput::Tree` path.
    struct TreeHoverHostLogic<S: EventSink> {
        host: BindingHost,
        engine: InteractionEngine,
        sink: RefCell<S>,
        clock_ms: Rc<Cell<u64>>,
        delay_ms: u64,
        tree: LayoutNode,
    }

    impl<S: EventSink> AppLogic for TreeHoverHostLogic<S> {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            let mut hits = Vec::new();
            let viewport = backend.viewport();
            let area = Rect::new(0.0, 0.0, viewport.width, viewport.height);
            paint_layout(backend, area, &self.tree, &mut hits);
            let mut leaf_tooltips = HashMap::new();
            collect_leaf_tooltips(&self.tree, &mut leaf_tooltips);
            self.host.set_leaf_tooltips(leaf_tooltips);
            render_overlays(&self.engine, backend, hits);
        }

        fn extra_tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
            self.engine.tab_stops()
        }

        fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
            let now = self.clock_ms.get();
            let reaction = self
                .engine
                .handle(event, backend, &mut *self.sink.borrow_mut());
            reaction.merge(self.host.drive_hover_tooltip(
                &mut self.engine,
                backend,
                now,
                self.delay_ms,
                0,
            ))
        }

        fn tick(&mut self, backend: &mut dyn Backend) -> Reaction {
            let now = self.clock_ms.get();
            self.host
                .drive_hover_tooltip(&mut self.engine, backend, now, self.delay_ms, 0)
        }
    }

    /// A `Button` leaf reached through a layout tree (not
    /// [`BindingHost::button`]'s own static slot) still shows its own
    /// `Button::tooltip` once hovered continuously for `delay_ms` --
    /// anchored to that exact leaf's own painted bounds, since the second
    /// leaf alongside it never arms a tooltip at all (its own `tooltip` is
    /// `None`) -- and hides the instant the pointer moves onto that second,
    /// tooltip-less leaf.
    #[test]
    fn drive_hover_tooltip_shows_a_layout_tree_leaf_tooltip_after_delay_anchored_to_its_bounds_and_hides_on_leave(
    ) {
        const DELAY_MS: u64 = 500;
        let clock = Rc::new(Cell::new(0u64));
        let mut save_button = Button::new(WidgetId::new("save"), "Save");
        save_button.tooltip = Some("Save the file".to_string());
        let other_button = Button::new(WidgetId::new("other"), "Other");
        let tree = LayoutNode::Row(LayoutContainer::new(
            vec![
                LayoutNode::Leaf(LayoutLeaf {
                    id: "leaf:save".to_string(),
                    descriptor: LeafDescriptor::Widget {
                        kind: "button",
                        widget: Rc::new(save_button),
                    },
                }),
                LayoutNode::Leaf(LayoutLeaf {
                    id: "leaf:other".to_string(),
                    descriptor: LeafDescriptor::Widget {
                        kind: "button",
                        widget: Rc::new(other_button),
                    },
                }),
            ],
            Spacing::default(),
        ));
        let logic = TreeHoverHostLogic {
            host: BindingHost::default(),
            engine: InteractionEngine::default(),
            sink: RefCell::new(RecordingSink),
            clock_ms: clock.clone(),
            delay_ms: DELAY_MS,
            tree,
        };
        let mut driver = TuiDriver::new(logic, 40, 20);
        driver.render();
        let (x, y) = driver
            .find("Save")
            .expect("the leaf's own label must be painted");

        // The pointer lands on the leaf -- still pending, no tooltip yet.
        driver.dispatch(UiEvent::MouseMoved {
            position: Point::new(x, y),
            buttons: ButtonMask::default(),
        });
        driver.render();
        assert!(
            !driver.screen_contains("Save the file"),
            "the tooltip must not show the instant the pointer lands"
        );

        // The delay elapses with no further pointer movement.
        clock.set(DELAY_MS);
        driver.tick();
        assert!(
            driver.screen_contains("Save the file"),
            "the tooltip must show once the hover delay has elapsed, \
             resolved by this leaf's own tree-address id even though it \
             was painted through `paint_layout`, not `BindingHost::button`"
        );

        // The pointer moves onto the second leaf, which carries no
        // tooltip at all -- the first leaf's tooltip must hide
        // immediately, proving the shown tooltip was anchored to the
        // first leaf's own painted bounds rather than some global state.
        let (ox, oy) = driver
            .find("Other")
            .expect("the second leaf's own label must be painted");
        driver.dispatch(UiEvent::MouseMoved {
            position: Point::new(ox, oy),
            buttons: ButtonMask::default(),
        });
        driver.render();
        assert!(
            !driver.screen_contains("Save the file"),
            "the tooltip must hide as soon as the pointer leaves its anchor"
        );
    }

    /// Two layout-tree leaves whose `Button` descriptors share the exact
    /// same `Button::id` ("shared") still resolve their own, independent
    /// tooltip text -- [`BindingHost::leaf_tooltips`] is keyed by each
    /// leaf's own tree-address id (`"leaf:alpha"`/`"leaf:bravo"`), not the
    /// descriptor's embedded (here, duplicated) `Button::id`.
    #[test]
    fn drive_hover_tooltip_resolves_each_leaf_independently_when_descriptors_share_an_id() {
        const DELAY_MS: u64 = 500;
        let clock = Rc::new(Cell::new(0u64));
        let shared_id = WidgetId::new("shared");
        let mut alpha = Button::new(shared_id, "Alpha");
        alpha.tooltip = Some("Tooltip for Alpha".to_string());
        let mut bravo = Button::new(shared_id, "Bravo");
        bravo.tooltip = Some("Tooltip for Bravo".to_string());
        let tree = LayoutNode::Row(LayoutContainer::new(
            vec![
                LayoutNode::Leaf(LayoutLeaf {
                    id: "leaf:alpha".to_string(),
                    descriptor: LeafDescriptor::Widget {
                        kind: "button",
                        widget: Rc::new(alpha),
                    },
                }),
                LayoutNode::Leaf(LayoutLeaf {
                    id: "leaf:bravo".to_string(),
                    descriptor: LeafDescriptor::Widget {
                        kind: "button",
                        widget: Rc::new(bravo),
                    },
                }),
            ],
            Spacing::default(),
        ));
        let logic = TreeHoverHostLogic {
            host: BindingHost::default(),
            engine: InteractionEngine::default(),
            sink: RefCell::new(RecordingSink),
            clock_ms: clock.clone(),
            delay_ms: DELAY_MS,
            tree,
        };
        let mut driver = TuiDriver::new(logic, 40, 20);
        driver.render();

        let (ax, ay) = driver
            .find("Alpha")
            .expect("the first leaf's own label must be painted");
        driver.dispatch(UiEvent::MouseMoved {
            position: Point::new(ax, ay),
            buttons: ButtonMask::default(),
        });
        clock.set(DELAY_MS);
        driver.tick();
        assert!(
            driver.screen_contains("Tooltip for Alpha"),
            "the first leaf's own tooltip must show"
        );
        assert!(
            !driver.screen_contains("Tooltip for Bravo"),
            "the second leaf's tooltip must not show while hovering the first"
        );

        let (bx, by) = driver
            .find("Bravo")
            .expect("the second leaf's own label must be painted");
        driver.dispatch(UiEvent::MouseMoved {
            position: Point::new(bx, by),
            buttons: ButtonMask::default(),
        });
        driver.render();
        assert!(
            !driver.screen_contains("Tooltip for Alpha"),
            "moving onto the second leaf must hide the first leaf's tooltip immediately"
        );

        clock.set(2 * DELAY_MS);
        driver.tick();
        assert!(
            driver.screen_contains("Tooltip for Bravo"),
            "the second leaf's own tooltip must show once its own delay has elapsed"
        );
        assert!(
            !driver.screen_contains("Tooltip for Alpha"),
            "the first leaf's tooltip must stay hidden"
        );
    }

    /// Minimal stand-in for a binding's own `QuiApp`/`QuiShellApp`: just a
    /// poison flag and a counter [`guarded`]'s `before`/`after` hooks can
    /// record calls into, proving both run around `body` (even when it
    /// panics) and neither runs once poisoned.
    #[derive(Default)]
    struct FakeApp {
        poisoned: bool,
        before_after_calls: u32,
    }

    /// [`GuardedFailure`] -> `String` for every test below: `"poisoned"`
    /// for [`GuardedFailure::Poisoned`], `"panic: <msg>"` for
    /// [`GuardedFailure::Panic`].
    fn fake_to_err(failure: GuardedFailure) -> String {
        match failure {
            GuardedFailure::Poisoned => "poisoned".to_string(),
            GuardedFailure::Panic(msg) => format!("panic: {msg}"),
        }
    }

    /// A successful `body` call returns its own result, with `state`
    /// untouched (not poisoned) and both hooks having run exactly once.
    #[test]
    fn guarded_runs_before_and_after_around_a_successful_body() {
        let mut app = FakeApp::default();
        let result: Result<i32, String> = guarded(
            &mut app,
            |s| s.poisoned,
            |s| s.poisoned = true,
            |s| s.before_after_calls += 1,
            |s| s.before_after_calls += 1,
            fake_to_err,
            |_s| Ok(42),
        );
        assert_eq!(result, Ok(42));
        assert!(!app.poisoned);
        assert_eq!(app.before_after_calls, 2);
    }

    /// A `body` that panics is caught, not propagated: [`guarded`] returns
    /// `to_err`'s own `Err` and poisons `state`. `after` still runs (it
    /// brackets the `catch_unwind` call, not the inside of it), but
    /// `before` only ran once, for this one `body` call.
    #[test]
    fn guarded_catches_a_panic_and_poisons() {
        let mut app = FakeApp::default();
        let result: Result<i32, String> = guarded(
            &mut app,
            |s| s.poisoned,
            |s| s.poisoned = true,
            |s| s.before_after_calls += 1,
            |s| s.before_after_calls += 1,
            fake_to_err,
            |_s| -> Result<i32, String> { panic!("boom") },
        );
        assert_eq!(result, Err("panic: boom".to_string()));
        assert!(app.poisoned);
        assert_eq!(app.before_after_calls, 2);
    }

    /// Once `state` is poisoned, a further [`guarded`] call rejects
    /// immediately via `to_err(GuardedFailure::Poisoned)`, running neither
    /// `before`/`after` nor `body` at all.
    #[test]
    fn guarded_rejects_without_running_body_once_poisoned() {
        let mut app = FakeApp {
            poisoned: true,
            before_after_calls: 0,
        };
        let result: Result<i32, String> = guarded(
            &mut app,
            |s| s.poisoned,
            |s| s.poisoned = true,
            |s| s.before_after_calls += 1,
            |s| s.before_after_calls += 1,
            fake_to_err,
            |_s| -> Result<i32, String> { panic!("must not run") },
        );
        assert_eq!(result, Err("poisoned".to_string()));
        assert_eq!(app.before_after_calls, 0);
    }

    /// [`record_pending_error`]'s "first error wins": a second recorded
    /// error never clobbers the first, the same contract every binding's
    /// own hand-written `record_error` implements.
    #[test]
    fn record_pending_error_keeps_the_first_error() {
        let mut slot: Option<String> = None;
        record_pending_error(&mut slot, "first".to_string());
        record_pending_error(&mut slot, "second".to_string());
        assert_eq!(slot, Some("first".to_string()));
    }

    /// [`take_pending_error`] turns a recorded error into an `Err` and
    /// empties the slot; with nothing recorded, it's a no-op `Ok(())`.
    #[test]
    fn take_pending_error_drains_the_slot_once() {
        let mut slot: Option<String> = None;
        assert_eq!(take_pending_error(&mut slot), Ok(()));

        record_pending_error(&mut slot, "boom".to_string());
        assert_eq!(take_pending_error(&mut slot), Err("boom".to_string()));
        assert_eq!(take_pending_error(&mut slot), Ok(()));
    }

    /// [`BindingHost::set_test_clock_ms`] pins [`BindingHost::now_ms`] to
    /// an exact value, repeatedly, with no real time elapsing between
    /// calls -- the seam a Python/Node test drives instead of a real
    /// sleep. Clearing the override with `None` falls back to the real
    /// wall clock, which is monotonic and non-zero once started.
    #[test]
    fn set_test_clock_ms_overrides_now_ms_until_cleared() {
        let host = BindingHost::default();

        host.set_test_clock_ms(Some(1_000));
        assert_eq!(host.now_ms(), 1_000);
        // Reading it again doesn't drift forward on its own, unlike the
        // real wall clock -- it stays pinned until explicitly re-set.
        assert_eq!(host.now_ms(), 1_000);

        host.set_test_clock_ms(Some(5_000));
        assert_eq!(host.now_ms(), 5_000);

        host.set_test_clock_ms(None);
        assert!(host.now_ms() < 5_000);
    }

    // ─── Container state ───────────────────────────────────────────────

    fn test_split(ratio: f32) -> Split {
        Split {
            id: WidgetId::new("host-test:split"),
            direction: quadraui::SplitDirection::Horizontal,
            ratio,
            first_min: 0.0,
            second_min: 0.0,
        }
    }

    /// Dragging a split divider with no app-supplied ratio on the next
    /// frame (the descriptor keeps resending its original literal `0.5`,
    /// as an app tracking no state of its own would) leaves the dragged
    /// ratio in place — this module's own acceptance contract for the
    /// split-ratio half of the widget-state store.
    #[test]
    fn apply_split_divider_drag_persists_across_an_uncontrolled_render() {
        let widget_state = RefCell::new(WidgetStateStore::new());
        let id = WidgetId::new("host-test:split");
        let split = test_split(0.5);

        // A render always happens before a drag can land on anything —
        // this seeds the baseline from the descriptor's own starting
        // ratio, the same order `InteractionEngine::handle` and a
        // binding's `render` run in for real.
        resolve_split_ratio(&widget_state, &split);
        apply_split_divider_drag(&widget_state, id, 0, 0.7);

        let resolved = resolve_split_ratio(&widget_state, &split);
        assert_eq!(
            resolved.ratio, 0.7,
            "a dragged ratio must survive a render with no ratio explicitly supplied"
        );
        // And it keeps surviving on a further uncontrolled render, not just
        // the first one right after the drag.
        let resolved_again = resolve_split_ratio(&widget_state, &split);
        assert_eq!(resolved_again.ratio, 0.7);
    }

    /// An app that explicitly changes `Split::ratio` to a new value (not
    /// merely resending the same literal it always has) wins outright over
    /// whatever a previous drag persisted.
    #[test]
    fn resolve_split_ratio_controlled_change_overrides_a_stored_drag() {
        let widget_state = RefCell::new(WidgetStateStore::new());
        let id = WidgetId::new("host-test:split");

        resolve_split_ratio(&widget_state, &test_split(0.5));
        apply_split_divider_drag(&widget_state, id, 0, 0.7);

        let split = test_split(0.3);
        let resolved = resolve_split_ratio(&widget_state, &split);
        assert_eq!(
            resolved.ratio, 0.3,
            "an app explicitly supplying a new ratio must override the stored drag"
        );
    }

    /// Uncontrolled (`None`) resolves to `0.0` the first time an id is
    /// seen, same "never touched = default" rule every other
    /// uncontrolled-by-default field in this module follows.
    #[test]
    fn resolve_scroll_offset_defaults_to_zero_when_never_set() {
        let widget_state = RefCell::new(WidgetStateStore::new());
        let id = WidgetId::new("host-test:scroll");
        assert_eq!(resolve_scroll_offset(&widget_state, id, None, 100.0), 0.0);
    }

    /// `Some(offset)` pins the controlled position outright, and is
    /// written back so a later uncontrolled frame resumes from it rather
    /// than re-seeding `0.0` -- mirroring
    /// [`apply_split_divider_drag_persists_across_an_uncontrolled_render`]'s
    /// own contract for `Split::ratio`.
    #[test]
    fn resolve_scroll_offset_controlled_value_is_written_back() {
        let widget_state = RefCell::new(WidgetStateStore::new());
        let id = WidgetId::new("host-test:scroll");
        assert_eq!(
            resolve_scroll_offset(&widget_state, id, Some(20.0), 100.0),
            20.0
        );
        assert_eq!(
            resolve_scroll_offset(&widget_state, id, None, 100.0),
            20.0,
            "an uncontrolled frame right after must resume from the pinned offset"
        );
    }

    /// The resolved offset is clamped into `[0, max_offset]` on every
    /// call, even for an already-stored (previously valid) value whose
    /// content has since shrunk -- `resolve_scroll_offset` re-clamps
    /// rather than trusting a stale stored clamp.
    #[test]
    fn resolve_scroll_offset_clamps_a_stored_value_whose_max_offset_shrank() {
        let widget_state = RefCell::new(WidgetStateStore::new());
        let id = WidgetId::new("host-test:scroll");
        resolve_scroll_offset(&widget_state, id, Some(90.0), 100.0);
        assert_eq!(resolve_scroll_offset(&widget_state, id, None, 30.0), 30.0);
    }

    fn test_tab_bar(tabs: Vec<bool>) -> quadraui::TabBar {
        quadraui::TabBar {
            id: WidgetId::new("host-test:tab-bar"),
            tabs: tabs
                .into_iter()
                .enumerate()
                .map(|(i, is_active)| quadraui::TabItem {
                    label: format!("tab {i}"),
                    is_active,
                    ..Default::default()
                })
                .collect(),
            scroll_offset: 0,
            right_segments: Vec::new(),
            active_accent: None,
            show_tab_close: true,
            compact: false,
            ..Default::default()
        }
    }

    fn test_layout_tabs(tabs: Vec<bool>) -> LayoutTabs {
        let tab_bar = test_tab_bar(tabs);
        let entries = (0..tab_bar.tabs.len())
            .map(|i| LayoutTabEntry {
                id: format!("content-{i}"),
                content: Box::new(LayoutNode::Leaf(LayoutLeaf {
                    id: format!("content-{i}-btn"),
                    descriptor: LeafDescriptor::Widget {
                        kind: "button",
                        widget: Rc::new(Button::new(
                            WidgetId::new(format!("content-{i}-btn")),
                            format!("tab {i} body"),
                        )),
                    },
                })),
            })
            .collect();
        LayoutTabs {
            tab_bar,
            entries,
            selected: 0,
            spacing: Spacing::default(),
            focused_index: None,
        }
    }

    /// Paints `tabs` once through `engine`, so its strip is on the hit map.
    fn paint_tabs(engine: &InteractionEngine, tabs: LayoutTabs) {
        let mut backend = TuiBackend::new();
        let mut hits = Vec::new();
        let area = Rect::new(0.0, 0.0, 40.0, 10.0);
        let node = LayoutNode::Tabs(tabs);
        engine.paint_with_widget_state(|| paint_layout(&mut backend, area, &node, &mut hits));
        engine.set_hits(hits);
    }

    /// A tab group's content pane follows the tab its strip's own state
    /// last activated — an app that never echoes the activation still sees
    /// the pane switch — and the strip reports that tab back through
    /// [`widget_state_snapshot`].
    #[test]
    fn resolve_layout_tabs_active_moves_the_content_pane_to_the_strips_active_tab() {
        let engine = InteractionEngine::new();
        paint_tabs(&engine, test_layout_tabs(vec![true, false, false]));
        let strip = WidgetId::new("host-test:tab-bar");
        engine.activate_tab(strip, 2);

        let mut resolved = test_layout_tabs(vec![true, false, false]);
        resolve_layout_tabs_active(&engine, &mut resolved);
        assert_eq!(resolved.selected, 2);
        let snapshot = widget_state_snapshot(&engine, strip).expect("the strip keeps state");
        assert_eq!(snapshot.summary.active_tab, Some(2));
    }

    /// A tab group whose app changes the active tab it sends wins over a
    /// tab its strip activated earlier.
    #[test]
    fn resolve_layout_tabs_active_controlled_change_overrides_an_earlier_activation() {
        let engine = InteractionEngine::new();
        paint_tabs(&engine, test_layout_tabs(vec![true, false, false]));
        engine.activate_tab(WidgetId::new("host-test:tab-bar"), 2);

        paint_tabs(&engine, test_layout_tabs(vec![false, true, false]));
        let mut resolved = test_layout_tabs(vec![false, true, false]);
        resolve_layout_tabs_active(&engine, &mut resolved);
        assert_eq!(resolved.selected, 1);
    }

    /// Nothing stored for an id under any shape — e.g. one that was never
    /// resolved/applied this process — snapshots to `None`.
    #[test]
    fn widget_state_snapshot_is_none_for_an_untouched_id() {
        let engine = InteractionEngine::new();
        assert_eq!(
            widget_state_snapshot(&engine, WidgetId::new("host-test:untouched")),
            None
        );
    }

    /// A split's ratio reads back exactly as `resolve_split_ratio` left it
    /// stored, once a drag has moved it off the descriptor's own literal.
    #[test]
    fn widget_state_snapshot_reads_back_a_split_ratio() {
        let engine = InteractionEngine::new();
        let id = WidgetId::new("host-test:split");
        resolve_split_ratio(engine.widget_state(), &test_split(0.5));
        apply_split_divider_drag(engine.widget_state(), id, 0, 0.7);

        let snapshot = widget_state_snapshot(&engine, id).expect("split ratio stored");
        assert_eq!(snapshot.ratio, Some(0.7));
        assert_eq!(snapshot.summary.selected_path, None);
    }
}
