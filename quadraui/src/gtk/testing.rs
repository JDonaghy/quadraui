//! In-process headless test driver for [`AppLogic`] GTK apps.
//!
//! [`GtkDriver`] mirrors [`crate::tui::testing::TuiDriver`]'s surface —
//! `new`, `render`, `dispatch`/`press`/`type_char`/`click` — so the same
//! scripted [`UiEvent`]s that drive `TuiDriver` also drive the GTK
//! backend (quadraui#446, GD-1).
//!
//! ## Display-free (verified in-repo)
//!
//! Renders into a headless `cairo::ImageSurface` (`Format::ARgb32`) — no
//! `gtk::init`, no `Application`, no window, no `GdkDisplay`. This is the
//! exact display-free path `gtk/pipeline_view.rs` and `gtk/tab_bar.rs`
//! already prove works for per-primitive paint tests; `GtkDriver` lifts
//! it to the whole-`AppLogic` level.
//!
//! ## No drift from production
//!
//! [`Self::render`] and [`Self::dispatch`] call the same
//! [`super::run::render_frame`] / [`super::run::dispatch_event`] the live
//! `quadraui::gtk::run` runner uses (mirroring quadraui#300's TUI split),
//! so the test path paints and pre-processes events (ActivityBar focus
//! intercept, accelerators, Ctrl-C/V/A, text selection) identically to
//! production. [`Self::click`] / [`Self::drag`] route through the same
//! [`crate::dispatch::dispatch_click`] / `dispatch_mouse_drag` /
//! `dispatch_mouse_up` the live click/motion/release handlers use, so
//! text-region and scrollbar drags behave the same under test.
//!
//! ## Limitations
//!
//! It renders into an in-memory `ImageSurface`, so it does **not**
//! exercise real GDK signal delivery — raw keycode translation
//! (`gdk_key_to_uievent`), IME, or actual `EventController` wiring are
//! out of scope and need a live-display smoke test instead. There is
//! also no character grid to string-match the way `TuiDriver::screen`
//! does; use [`Self::pixel`] to assert on rendered pixel colour instead.
//!
//! ```no_run
//! # use quadraui::gtk::testing::GtkDriver;
//! # use quadraui::{AppLogic, Backend, Reaction, UiEvent};
//! # fn demo<A: AppLogic + Default>() {
//! let mut driver = GtkDriver::new(A::default(), 100, 30);
//! driver.type_char('x');
//! let _ = driver.pixel(0, 0);
//! # }
//! ```

use pangocairo::cairo::{Context, Format, ImageSurface};

use crate::backend::Backend;
use crate::dispatch::{dispatch_click, dispatch_mouse_drag, dispatch_mouse_up};
use crate::runner::{AppLogic, Reaction};
use crate::{ButtonMask, Key, Modifiers, MouseButton, NamedKey, Point, UiEvent};

use super::backend::GtkBackend;
use super::run::{dispatch_event, render_frame, EventOutcome};

/// Drives an [`AppLogic`] impl headlessly against the GTK backend for
/// tests. Construct with [`Self::new`] (runs `setup` + paints the first
/// frame), poke it with [`Self::press`] / [`Self::type_char`] /
/// [`Self::click`] / [`Self::drag`], and read painted pixels back with
/// [`Self::pixel`].
pub struct GtkDriver<A: AppLogic> {
    app: A,
    backend: GtkBackend,
    surface: ImageSurface,
    width: i32,
    height: i32,
    exited: bool,
}

impl<A: AppLogic> GtkDriver<A> {
    /// Build a driver for `app` on a `width`×`height` pixel surface, run
    /// the app's `setup` hook, and paint the first frame.
    pub fn new(app: A, width: i32, height: i32) -> Self {
        let surface =
            ImageSurface::create(Format::ARgb32, width, height).expect("create ImageSurface");
        let mut backend = GtkBackend::new();
        // Seed the viewport from the driver's surface size BEFORE setup,
        // exactly as the live `gtk::run::activate` does (quadraui#437):
        // without this, `app.setup()` would read `GtkBackend::new()`'s
        // zeroed default viewport instead of the requested
        // `width`×`height`.
        backend.begin_frame(crate::Viewport::new(width as f32, height as f32, 1.0));
        let mut app = app;
        app.setup(&mut backend);
        let mut driver = Self {
            app,
            backend,
            surface,
            width,
            height,
            exited: false,
        };
        driver.render();
        driver
    }

    /// Repaint one frame through the shared production render path.
    pub fn render(&mut self) {
        let cr = Context::new(&self.surface).expect("Context::new on headless ImageSurface");
        render_frame(&mut self.backend, &self.app, &cr, self.width, self.height);
    }

    /// Feed one synthetic event through the shared production
    /// [`dispatch_event`] path. Repaints on redraw and latches `exited`.
    pub fn dispatch(&mut self, event: UiEvent) -> Reaction {
        if self.exited {
            return Reaction::Exit;
        }
        match dispatch_event(event, &mut self.backend, &mut self.app) {
            EventOutcome::Continue => Reaction::Continue,
            EventOutcome::Redraw => {
                self.render();
                Reaction::Redraw
            }
            EventOutcome::Exit => {
                self.exited = true;
                Reaction::Exit
            }
        }
    }

    /// Press a key (no modifiers).
    pub fn press(&mut self, key: Key) -> Reaction {
        self.dispatch(UiEvent::KeyPressed {
            key,
            modifiers: Modifiers::default(),
            repeat: false,
        })
    }

    /// Type a single character key (no modifiers).
    pub fn type_char(&mut self, c: char) -> Reaction {
        self.press(Key::Char(c))
    }

    /// Press a named (non-printable) key, e.g. [`NamedKey::Enter`].
    pub fn press_named(&mut self, key: NamedKey) -> Reaction {
        self.press(Key::Named(key))
    }

    /// Press a character key with Ctrl held (e.g. `ctrl_char('c')` to
    /// trigger the runner's copy-on-selection path).
    pub fn ctrl_char(&mut self, c: char) -> Reaction {
        self.dispatch(UiEvent::KeyPressed {
            key: Key::Char(c),
            modifiers: Modifiers {
                ctrl: true,
                ..Modifiers::default()
            },
            repeat: false,
        })
    }

    /// Left-click at surface coordinates `(x, y)` (pixels), routed
    /// through the same [`dispatch_click`] the live click handler uses —
    /// so a click on a registered text region or scrollbar begins a drag
    /// exactly as it would live.
    pub fn click(&mut self, x: f32, y: f32) -> Reaction {
        self.mouse_down(x, y)
    }

    /// Press the left mouse button down at `(x, y)`. Begins a drag if it
    /// lands on a draggable target (text region, scrollbar thumb).
    pub fn mouse_down(&mut self, x: f32, y: f32) -> Reaction {
        let position = Point::new(x, y);
        let events = {
            let stack_rc = self.backend.modal_stack_handle();
            let drag_rc = self.backend.drag_state_handle();
            let stack = stack_rc.borrow();
            let mut drag = drag_rc.borrow_mut();
            let evs = dispatch_click(
                &stack,
                &[], // scroll surfaces not tracked by the driver — mirrors gtk::run
                &self.backend.text_regions,
                &mut drag,
                position,
                MouseButton::Left,
                Modifiers::default(),
            );
            if let Some(crate::dispatch::DragTarget::TextSelection { region, .. }) = drag.target() {
                self.backend.track_focused_text_region(region.clone());
            }
            evs
        };
        self.dispatch_all(events)
    }

    /// Move the cursor to `(x, y)` with the left button held. During an
    /// active drag this is translated to the drag's high-level event
    /// (e.g. [`UiEvent::TextSelectionChanged`] /
    /// [`UiEvent::ScrollOffsetChanged`]).
    pub fn mouse_move(&mut self, x: f32, y: f32) -> Reaction {
        let position = Point::new(x, y);
        let events = {
            let drag_rc = self.backend.drag_state_handle();
            let drag = drag_rc.borrow();
            dispatch_mouse_drag(
                &drag,
                position,
                ButtonMask {
                    left: true,
                    ..ButtonMask::default()
                },
            )
        };
        self.dispatch_all(events)
    }

    /// Release the left mouse button at `(x, y)`, ending any active drag.
    pub fn mouse_up(&mut self, x: f32, y: f32) -> Reaction {
        let position = Point::new(x, y);
        let events = {
            let stack_rc = self.backend.modal_stack_handle();
            let drag_rc = self.backend.drag_state_handle();
            let stack = stack_rc.borrow();
            let mut drag = drag_rc.borrow_mut();
            dispatch_mouse_up(&stack, &mut drag, position, MouseButton::Left)
        };
        self.dispatch_all(events)
    }

    /// Left-button drag from `(x0, y0)` to `(x1, y1)`: down → move → up.
    pub fn drag(&mut self, x0: f32, y0: f32, x1: f32, y1: f32) -> Reaction {
        self.mouse_down(x0, y0);
        self.mouse_move(x1, y1);
        self.mouse_up(x1, y1)
    }

    /// Dispatch each of `events` in order, short-circuiting on `Exit` and
    /// returning the strongest [`Reaction`] observed (`Exit` beats
    /// `Redraw` beats `Continue`) — mirrors the live click/motion/release
    /// handlers' per-event loop.
    fn dispatch_all(&mut self, events: Vec<UiEvent>) -> Reaction {
        let mut result = Reaction::Continue;
        for ev in events {
            match self.dispatch(ev) {
                Reaction::Exit => return Reaction::Exit,
                Reaction::Redraw => result = Reaction::Redraw,
                Reaction::Continue => {}
            }
        }
        result
    }

    /// Whether the app has returned [`Reaction::Exit`].
    pub fn exited(&self) -> bool {
        self.exited
    }

    /// Access the app state for test assertions.
    pub fn app(&self) -> &A {
        &self.app
    }

    /// Access the backend for test assertions (e.g. active selection
    /// state, drag state).
    pub fn backend(&self) -> &GtkBackend {
        &self.backend
    }

    /// Read an RGB triple from the rendered surface at pixel `(x, y)`.
    ///
    /// Cairo's `ARgb32` stores each pixel as four bytes in native
    /// (little-endian) byte order: `[B, G, R, A]` — the same decode
    /// `gtk/tab_bar.rs`'s and `gtk/pipeline_view.rs`'s paint tests use.
    /// Flushes pending Cairo operations first so the readback reflects
    /// the last [`Self::render`].
    pub fn pixel(&mut self, x: i32, y: i32) -> (u8, u8, u8) {
        self.surface.flush();
        let stride = self.surface.stride() as usize;
        let data = self.surface.data().expect("surface data");
        let off = y as usize * stride + x as usize * 4;
        (data[off + 2], data[off + 1], data[off])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::status_bar::{StatusBar, StatusBarSegment};
    use crate::types::{Color, WidgetId};
    use crate::Rect;

    const W: i32 = 120;
    const H: i32 = 24;
    // Distinct from the default theme background so a match can only
    // come from the segment actually being painted.
    const KNOWN_BG: Color = Color::rgb(0, 255, 0);

    /// Minimal app that draws one `StatusBar` segment filling the whole
    /// surface with [`KNOWN_BG`].
    struct StatusBarApp;

    impl AppLogic for StatusBarApp {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            backend.draw_status_bar(
                Rect::new(0.0, 0.0, W as f32, H as f32),
                &StatusBar {
                    id: WidgetId::new("status"),
                    left_segments: vec![StatusBarSegment {
                        text: "known-pixel".to_string(),
                        fg: Color::rgb(255, 255, 255),
                        bg: KNOWN_BG,
                        bold: false,
                        action_id: None,
                    }],
                    right_segments: vec![],
                },
                None,
                None,
            );
        }

        fn handle(&mut self, _event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
            Reaction::Continue
        }
    }

    /// GD-1 acceptance test: build an `AppLogic`, render a frame
    /// offscreen (no display, no `gtk::init`), and read back a known
    /// pixel. `draw_status_bar` fills the bar with the first segment's
    /// `bg` when there's no gap to the right segments, so every pixel in
    /// the bar's rect should be [`KNOWN_BG`].
    #[test]
    fn renders_offscreen_and_reads_back_known_pixel() {
        let mut driver = GtkDriver::new(StatusBarApp, W, H);

        let (r, g, b) = driver.pixel(4, H / 2);
        assert_eq!(
            (r, g, b),
            (KNOWN_BG.r, KNOWN_BG.g, KNOWN_BG.b),
            "expected the status bar segment's known background colour, got ({r}, {g}, {b})"
        );
    }

    /// `setup()` must observe the driver's real surface dimensions, not
    /// a zeroed default — regression guard for the #437-style
    /// initial-layout bug, mirrored from `TuiDriver`'s equivalent test.
    #[test]
    fn setup_sees_real_viewport_not_default() {
        use std::cell::Cell;
        use std::rc::Rc;

        struct ViewportRecorder {
            seen: Rc<Cell<Option<crate::Viewport>>>,
        }

        impl AppLogic for ViewportRecorder {
            type AreaId = ();

            fn setup(&mut self, backend: &mut dyn Backend) {
                self.seen.set(Some(backend.viewport()));
            }

            fn render(&self, _backend: &mut dyn Backend, _area: ()) {}

            fn handle(&mut self, _event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
                Reaction::Continue
            }
        }

        let seen = Rc::new(Cell::new(None));
        let app = ViewportRecorder { seen: seen.clone() };
        let _driver = GtkDriver::new(app, 200, 60);

        let vp = seen.get().expect("setup() should have run");
        assert_eq!(
            (vp.width, vp.height),
            (200.0, 60.0),
            "setup() must see the driver's real size, not a zeroed default"
        );
    }
}
