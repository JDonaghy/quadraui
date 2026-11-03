//! napi-rs bindings over `quadraui`'s headless TUI driver.
//!
//! The language-neutral hit testing, semantic event synthesis, text-input/
//! focus/drag controllers, retained descriptor model, and render -> frame
//! assembly this binding drives all live in `quadraui_binding_core` (see
//! that crate's module doc) -- this crate is the napi-rs leaf around it,
//! the same role `quadraui-py` plays for Python.
//!
//! [`QuiApp`] wraps a `quadraui::tui::testing::TuiDriver` running an
//! in-crate [`NodeAppLogic`] that holds one [`quadraui_binding_core::BindingHost`]
//! (the six fixed primitive slots, any additive slots, and the
//! backend-settings replay store) plus a [`quadraui_binding_core::InteractionEngine`]
//! and a pending-error slot -- the napi-rs analog of `quadraui-py`'s
//! `PyAppLogic`. Today's surface is the minimal headless one: `new`,
//! `step`, `run`, `screen`, `screen_contains`, `resize`, `push_key_event`,
//! `slot_clear`. No JS event handler is registered anywhere yet -- every
//! dispatched event is accepted with no side effect -- and no `insert_*`
//! descriptor method exists yet either; both are expected future additions
//! to this crate's surface, mirroring `quadraui-py`'s own.
//!
//! A Rust panic reaching a driver call is caught at the [`QuiApp::guarded`]
//! boundary, raised as a JS `Error` instead of unwinding into Node, and
//! poisons the handle so every later call raises rather than touching a
//! driver that may have panicked mid-mutation -- mirrors `quadraui-py`'s
//! `QuiApp::guarded`. napi-rs itself already wraps every exported method in
//! `catch_unwind` to stop a panic aborting the whole process, but that alone
//! does not poison the handle for later calls; this boundary adds exactly
//! that.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::panic::{self, AssertUnwindSafe};
use std::rc::Rc;

use napi::{Error, Result};
use napi_derive::napi;

use quadraui::tui::testing::TuiDriver;
use quadraui::{AppLogic, Backend, Reaction, Rect, UiEvent, WidgetId};
use quadraui_binding_core::{
    draw_bands, key_pressed_event, panic_message, render_overlays, step_driver, BindingHost,
    InteractionEngine,
};

/// The [`AppLogic`] a [`QuiApp`] drives: a thin napi-rs leaf around
/// [`BindingHost`] (the six fixed primitive slots, any additive slots, and
/// the backend-settings replay store -- see that type's own doc) plus the
/// [`InteractionEngine`] every binding's `AppLogic` holds alongside it, and
/// a pending-error slot a registered JS handler/render callback would
/// populate the same way `quadraui-py`'s `PyAppLogic::pending_error` does --
/// see [`take_pending_error`]. Nothing writes it today: this struct's own
/// `handle` has no handler to call.
#[derive(Default)]
struct NodeAppLogic {
    /// The host-side slot/replay state this binding is a thin napi-rs leaf
    /// over -- see [`BindingHost`]'s own doc.
    host: BindingHost,
    /// The language-neutral `event -> hit -> semantic` orchestration engine
    /// this binding is a thin napi-rs leaf over -- see
    /// `quadraui_binding_core::engine`'s module doc. [`AppLogic::render`]
    /// feeds it this frame's painted hits via [`InteractionEngine::set_hits`];
    /// a registered JS handler would forward dispatch to it the same way
    /// `quadraui-py`'s `PyAppLogic::handle` does.
    engine: InteractionEngine,
    /// Would be set by a registered JS callback when it raises. Present on
    /// this struct even with no handler to call yet, so its shape already
    /// matches `quadraui-py`'s `PyAppLogic`; every render-triggering
    /// `QuiApp` method already drains it via [`take_pending_error`], so
    /// adding a handler only means writing to this slot, not touching any
    /// call site that reads it.
    pending_error: Rc<RefCell<Option<Error>>>,
}

impl NodeAppLogic {
    /// Whether `slot` names one of this app's six fixed primitive slots.
    fn is_known_slot(slot: i64) -> bool {
        BindingHost::is_known_slot(slot)
    }

    /// Empties one slot, returning whether it held anything.
    fn clear_slot(&mut self, slot: i64) -> bool {
        self.host.clear_slot(slot)
    }
}

impl AppLogic for NodeAppLogic {
    type AreaId = ();

    /// Replays [`Self::host`]'s accelerators and backend-settings fields
    /// onto `backend` -- [`BindingHost::setup`] -- mirrors `quadraui-py`'s
    /// `PyAppLogic::setup`.
    fn setup(&mut self, backend: &mut dyn Backend) {
        self.host.setup(backend);
    }

    /// Paints this app's six fixed slots (nothing, since no `insert_*`
    /// method exists yet to populate [`Self::host`]) plus the anchored
    /// overlay pass every binding's `render` runs -- see
    /// [`render_overlays`]'s own doc.
    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let mut hits = Vec::new();
        let chrome_interaction = |id: WidgetId| self.engine.chrome_interaction(id);
        draw_bands(
            backend,
            self.host.render_refs(),
            &mut hits,
            &chrome_interaction,
        );
        render_overlays(&self.engine, backend, hits);
    }

    /// Delegates to [`InteractionEngine::tab_stops`], which derives this
    /// frame's tab stops directly from the hit map [`Self::render`] just
    /// fed it via [`InteractionEngine::set_hits`].
    fn tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
        self.engine.tab_stops()
    }

    /// No JS handler exists yet, so every event is accepted with no side
    /// effect -- the same `Reaction::Continue` default `quadraui-py`'s
    /// `PyAppLogic::handle` falls back to when no handler is registered.
    fn handle(&mut self, _event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        Reaction::Continue
    }
}

/// Takes and re-raises any error a registered callback recorded during the
/// `render()` call that just ran -- mirrors `quadraui-py`'s
/// `take_pending_error`. Always `Ok(())` today, since [`NodeAppLogic`] has
/// no handler that could ever write to `pending_error`; kept as the one
/// drain point every render-triggering [`QuiApp`] method already calls, so
/// a future registered handler/render callback only needs to write the
/// error, not add a new drain call site.
fn take_pending_error(app: &mut NodeAppLogic) -> Result<()> {
    if let Some(err) = app.pending_error.borrow_mut().take() {
        return Err(err);
    }
    Ok(())
}

/// Rejects a zero `width`/`height`, otherwise narrows both to the `u16`
/// [`TuiDriver`]/[`quadraui::tui::testing::TuiBackend`] take -- the napi-rs
/// analog of `quadraui-py`'s own zero-dimension check in `QuiApp::new`/
/// `resize`.
fn checked_dimensions(width: u16, height: u16) -> Result<(u16, u16)> {
    if width == 0 || height == 0 {
        return Err(Error::from_reason("width and height must both be non-zero"));
    }
    Ok((width, height))
}

/// A headless quadraui app, driven entirely from JavaScript/TypeScript.
///
/// Construct it with a `width`/`height` cell grid and inspect the rendered
/// frame with `screen()`/`screenContains()`. No event handler is
/// registered and no `insert*` descriptor method exists yet, so today's
/// `QuiApp` only exercises the headless driver's own step/render/resize
/// loop.
#[napi]
pub struct QuiApp {
    driver: TuiDriver<NodeAppLogic>,
    /// Events queued by `pushKeyEvent`, drained one per `step`/`run` call
    /// (oldest first); a step ticks the app instead when this is empty.
    pending_events: VecDeque<UiEvent>,
    /// Set once a call into `driver` has panicked and been caught by
    /// [`QuiApp::guarded`]. Every method on a poisoned app raises instead of
    /// touching the driver again.
    poisoned: bool,
}

#[napi]
impl QuiApp {
    /// Creates a new headless app on a `width`x`height` cell grid. Raises if
    /// either is zero.
    #[napi(constructor)]
    pub fn new(width: u16, height: u16) -> Result<Self> {
        let (width, height) = checked_dimensions(width, height)?;
        let driver = TuiDriver::new(NodeAppLogic::default(), width, height);
        Ok(QuiApp {
            driver,
            pending_events: VecDeque::new(),
            poisoned: false,
        })
    }

    /// Resizes the underlying cell grid to `width`x`height`. Raises if
    /// either is zero.
    #[napi]
    pub fn resize(&mut self, width: u16, height: u16) -> Result<()> {
        let (width, height) = checked_dimensions(width, height)?;
        self.guarded(|app| {
            app.driver.resize(width, height);
            Ok(())
        })
    }

    /// Queues a synthetic `KeyPressed` event for the character with Unicode
    /// code point `ch`, no modifiers held, not a repeat. Raises if `ch`
    /// isn't a valid Unicode scalar value.
    #[napi(js_name = "pushKeyEvent")]
    pub fn push_key_event(&mut self, ch: u32) -> Result<()> {
        let ch = char::from_u32(ch)
            .ok_or_else(|| Error::from_reason(format!("invalid Unicode code point: {ch}")))?;
        self.pending_events.push_back(key_pressed_event(ch));
        Ok(())
    }

    /// Advances the runner by one step: dispatches the oldest event queued
    /// by `pushKeyEvent`, or ticks if none is queued. Returns `true` if the
    /// app is still running afterwards, `false` once it has exited.
    #[napi]
    pub fn step(&mut self) -> Result<bool> {
        self.guarded(|app| app.step_once())
    }

    /// Runs up to `max_steps` steps (default 1). Stops early once the app
    /// exits. Returns `true` if still running afterwards, `false` once
    /// exited.
    #[napi]
    pub fn run(&mut self, max_steps: Option<u32>) -> Result<bool> {
        let steps = max_steps.unwrap_or(1).max(1);
        let mut running = true;
        for _ in 0..steps {
            running = self.guarded(|app| app.step_once())?;
            if !running {
                break;
            }
        }
        Ok(running)
    }

    /// Renders the current frame and returns it as a plain-text screen
    /// dump, one row per line.
    #[napi]
    pub fn screen(&mut self) -> Result<String> {
        self.guarded(|app| {
            app.driver.render();
            take_pending_error(app.driver.app_mut())?;
            Ok(app.driver.screen())
        })
    }

    /// Whether `needle` appears anywhere in the current rendered frame.
    #[napi(js_name = "screenContains")]
    pub fn screen_contains(&mut self, needle: String) -> Result<bool> {
        self.guarded(|app| {
            app.driver.render();
            take_pending_error(app.driver.app_mut())?;
            Ok(app.driver.screen_contains(&needle))
        })
    }

    /// Empties a slot (one of the six fixed primitive slot ids
    /// `quadraui_binding_core` defines -- `SLOT_BUTTON` etc.), re-rendering
    /// so `screen()` no longer shows it. Returns whether the slot held
    /// anything. Raises if `slot` doesn't name one of the six.
    #[napi(js_name = "slotClear")]
    pub fn slot_clear(&mut self, slot: i64) -> Result<bool> {
        if !NodeAppLogic::is_known_slot(slot) {
            return Err(Error::from_reason(format!("unknown slot id: {slot}")));
        }
        self.guarded(|app| {
            let cleared = app.driver.app_mut().clear_slot(slot);
            if cleared {
                app.driver.render();
                take_pending_error(app.driver.app_mut())?;
            }
            Ok(cleared)
        })
    }
}

impl QuiApp {
    /// Runs `body` on `self`, catching a Rust panic instead of letting it
    /// propagate as napi-rs's own generic panic-to-`Error` conversion (which
    /// stops the process aborting but does not poison this handle). On
    /// success returns `body`'s result unchanged; on a panic, poisons this
    /// app and raises an `Error` carrying [`panic_message`]'s text. If this
    /// app is already poisoned (an earlier call panicked), raises without
    /// running `body` at all -- mirrors `quadraui-py`'s `QuiApp::guarded`.
    fn guarded<T>(&mut self, body: impl FnOnce(&mut Self) -> Result<T>) -> Result<T> {
        if self.poisoned {
            return Err(Error::from_reason(
                "this QuiApp is poisoned by an earlier caught panic and can no longer be used",
            ));
        }
        match panic::catch_unwind(AssertUnwindSafe(|| body(self))) {
            Ok(result) => result,
            Err(payload) => {
                self.poisoned = true;
                Err(Error::from_reason(panic_message(&*payload)))
            }
        }
    }

    /// Dispatches the oldest queued event, or ticks if none is queued, then
    /// surfaces any pending error a callback recorded while handling it.
    /// Returns whether the app is still running.
    fn step_once(&mut self) -> Result<bool> {
        let running = step_driver(&mut self.driver, &mut self.pending_events);
        take_pending_error(self.driver.app_mut())?;
        Ok(running)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `QuiApp::new` rejects a zero `width`/`height` instead of constructing
    /// a driver that can never paint anything.
    #[test]
    fn new_rejects_zero_dimensions() {
        assert!(QuiApp::new(0, 10).is_err());
        assert!(QuiApp::new(10, 0).is_err());
        assert!(QuiApp::new(0, 0).is_err());
    }

    /// A fresh app renders a blank screen of the requested size: `height`
    /// lines, each `width` cells, every cell blank since nothing has been
    /// inserted into any slot yet.
    #[test]
    fn fresh_app_renders_blank_screen() {
        let mut app = QuiApp::new(8, 3).expect("valid dimensions");
        let screen = app.screen().expect("render must not fail");
        let lines: Vec<&str> = screen.lines().collect();
        assert_eq!(lines.len(), 3);
        for line in lines {
            assert!(
                line.trim().is_empty(),
                "expected a blank line, got {line:?}"
            );
        }
    }

    /// `step`/`run` advance the app and report it as still running, since
    /// nothing in this crate's current surface can request exit.
    #[test]
    fn step_and_run_keep_running() {
        let mut app = QuiApp::new(5, 5).expect("valid dimensions");
        assert!(app.step().expect("step must not fail"));
        assert!(app.run(Some(3)).expect("run must not fail"));
    }

    /// `pushKeyEvent` rejects a code point with no valid Unicode scalar
    /// value instead of queuing a malformed event.
    #[test]
    fn push_key_event_rejects_invalid_code_point() {
        let mut app = QuiApp::new(5, 5).expect("valid dimensions");
        assert!(app.push_key_event(0xD800).is_err());
    }

    /// `slotClear` raises on an id outside the six fixed primitive slots.
    #[test]
    fn slot_clear_rejects_unknown_slot() {
        let mut app = QuiApp::new(5, 5).expect("valid dimensions");
        assert!(app.slot_clear(999).is_err());
    }

    /// `slotClear` on a never-occupied slot is a no-op that reports
    /// `false`, not an error.
    #[test]
    fn slot_clear_on_empty_slot_reports_false() {
        let mut app = QuiApp::new(5, 5).expect("valid dimensions");
        assert!(!app
            .slot_clear(quadraui_binding_core::SLOT_BUTTON)
            .expect("a known slot id must not raise"));
    }

    /// A Rust panic raised from inside `guarded`'s body is caught and
    /// re-raised as an `Error`, and poisons the app so a later call raises
    /// without running its body -- mirrors `quadraui-py`'s
    /// `guarded_catches_panic_then_poisons`.
    #[test]
    fn guarded_catches_panic_then_poisons() {
        let mut app = QuiApp::new(10, 10).expect("valid dimensions");

        let panicked: Result<()> = app.guarded(|_| panic!("boom"));
        assert!(panicked.is_err(), "a caught panic must surface as an error");
        assert!(app.poisoned);

        let mut body_ran = false;
        let after_poison: Result<()> = app.guarded(|_| {
            body_ran = true;
            Ok(())
        });
        assert!(!body_ran, "a poisoned app must not run the guarded body");
        assert!(after_poison.is_err(), "a poisoned app must keep erroring");
    }
}
