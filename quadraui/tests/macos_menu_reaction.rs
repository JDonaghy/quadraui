#![cfg(all(feature = "macos", target_os = "macos"))]
//! Headless `MacDriver` coverage for what the macOS runner does with a
//! native menu item's [`Reaction`].
//!
//! A native `NSMenuItem` click does not arrive through the responder
//! chain: `menu_bar_install::quadra_menu_action` pushes
//! `UiEvent::MenuActivated(id)` onto the backend queue and calls
//! `MacBackend::redraw_kicker`, which is the wake callback
//! `macos::run::run` installed — and that callback drains the queue
//! through `QuadraView::dispatch_queued_events`, routing each event into
//! `AppLogic::handle` and applying the `Reaction` it answers. These tests
//! reproduce that drain on the headless `MacDriver` — [`drain`] below
//! polls the backend queue and feeds each event through
//! `MacDriver::dispatch`, the same shared `dispatch_event` →
//! `apply_outcome` path the live `QuadraView::dispatch` runs — so the
//! File > Quit case (`Reaction::Exit`) is asserted on any host rather
//! than only by clicking a real menu.

use quadraui::macos::testing::MacDriver;
use quadraui::{AppLogic, Backend, Button, DrawsPrimitives, Reaction, Rect, UiEvent, WidgetId};

#[path = "../examples/common/native_menu.rs"]
mod native_menu;
use native_menu::NativeMenuApp;

const CANVAS_W: u32 = 320;
const CANVAS_H: u32 = 120;

/// Minimal `AppLogic` with a native-menu-shaped `handle`: the quit item
/// answers `Reaction::Exit`, a formatting item answers `Reaction::Redraw`
/// after mutating state the next frame paints, and anything else answers
/// `Reaction::Continue`.
#[derive(Default)]
struct MenuApp {
    bold: bool,
    seen: Vec<WidgetId>,
}

impl AppLogic for MenuApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let label = if self.bold { "BOLD" } else { "plain" };
        backend.draw_primitive(
            Rect::new(10.0, 10.0, 160.0, 32.0),
            &Button::new(WidgetId::new("menu.state"), label),
        );
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        let UiEvent::MenuActivated(id) = event else {
            return Reaction::Continue;
        };
        self.seen.push(id);
        match id.as_str() {
            "file.quit" => Reaction::Exit,
            "format.bold" => {
                self.bold = !self.bold;
                Reaction::Redraw
            }
            _ => Reaction::Continue,
        }
    }
}

fn driver() -> MacDriver<MenuApp> {
    MacDriver::new(MenuApp::default(), CANVAS_W, CANVAS_H)
}

/// Drain the backend's queue through `MacDriver::dispatch`, folding the
/// reactions with [`Reaction::merge`] — the headless stand-in for
/// `QuadraView::dispatch_queued_events`, which the live runner drives
/// from the wake callback a menu action selector invokes.
fn drain<A: AppLogic<AreaId = ()>>(d: &mut MacDriver<A>) -> Reaction {
    let queued = d.parts_mut().0.poll_events();
    let mut merged = Reaction::Continue;
    for ev in queued {
        merged = merged.merge(d.dispatch(ev));
    }
    merged
}

/// The repro: a File > Quit menu item whose handler answers
/// `Reaction::Exit` must actually exit, not be swallowed by the drain.
#[test]
fn quit_menu_item_reaction_exit_exits_the_app() {
    let mut d = driver();
    assert!(!d.exited(), "fresh driver should not be exited");

    d.backend()
        .push_event(UiEvent::MenuActivated(WidgetId::new("file.quit")));
    let reaction = drain(&mut d);

    assert_eq!(
        reaction,
        Reaction::Exit,
        "a queued MenuActivated's Reaction::Exit must reach the caller"
    );
    assert!(
        d.exited(),
        "File > Quit must leave the runner in its exited state"
    );
    assert_eq!(
        d.app().seen,
        vec![WidgetId::new("file.quit")],
        "the queued activation must reach AppLogic::handle"
    );
}

/// A non-exiting menu item's `Reaction::Redraw` repaints, so the state
/// its handler mutated is on screen when the drain returns.
#[test]
fn menu_item_reaction_redraw_repaints_with_the_mutated_state() {
    let mut d = driver();
    assert!(d.screen_contains("plain"), "initial frame paints `plain`");

    d.backend()
        .push_event(UiEvent::MenuActivated(WidgetId::new("format.bold")));
    let reaction = drain(&mut d);

    assert_eq!(reaction, Reaction::Redraw);
    assert!(
        d.screen_contains("BOLD"),
        "the menu handler's mutation must be painted by the drain's repaint"
    );
    assert!(!d.exited());
}

/// Two activations queued before a single drain both reach `handle`, and
/// their reactions fold — `Exit` wins over the earlier `Redraw` — so a
/// quit queued behind another item is not lost.
#[test]
fn a_whole_batch_of_queued_activations_is_dispatched_and_folded() {
    let mut d = driver();
    d.backend()
        .push_event(UiEvent::MenuActivated(WidgetId::new("format.bold")));
    d.backend()
        .push_event(UiEvent::MenuActivated(WidgetId::new("file.quit")));

    let reaction = drain(&mut d);

    assert_eq!(reaction, Reaction::Exit);
    assert_eq!(
        d.app().seen,
        vec![WidgetId::new("format.bold"), WidgetId::new("file.quit")],
        "both queued activations must reach AppLogic::handle, in order"
    );
    assert!(d.exited());
}

/// The common case — nothing queued — must not touch `AppLogic::handle`
/// and must answer `Reaction::Continue`.
#[test]
fn an_empty_queue_drains_to_continue_without_calling_handle() {
    let mut d = driver();
    assert_eq!(drain(&mut d), Reaction::Continue);
    assert!(d.app().seen.is_empty());
    assert!(!d.exited());
}

/// The shipped demo for this path (`macos_native_menu`) must quit on its
/// own File > Quit Demo item, not just record it in the status bar.
#[test]
fn the_native_menu_example_quits_on_its_file_quit_item() {
    let mut d = MacDriver::new(NativeMenuApp::new(), 640, 240);
    d.backend()
        .push_event(UiEvent::MenuActivated(WidgetId::new("file.quit")));

    assert_eq!(drain(&mut d), Reaction::Exit);
    assert!(d.exited(), "File > Quit Demo must exit the example");
}

/// A non-quit item from the same example keeps the app running and shows
/// up in its status bar, so the `Exit` arm is specific to File > Quit.
#[test]
fn the_native_menu_example_keeps_running_on_a_toggle_item() {
    let mut d = MacDriver::new(NativeMenuApp::new(), 640, 240);
    d.backend()
        .push_event(UiEvent::MenuActivated(WidgetId::new("view.toggle_sidebar")));

    assert_eq!(drain(&mut d), Reaction::Redraw);
    assert!(!d.exited());
    assert!(d.screen_contains("view.toggle_sidebar"));
}
