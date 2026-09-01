//! In-process headless test driver for [`AppLogic`] macOS apps
//! (quadraui#493).
//!
//! [`MacDriver`] mirrors [`crate::tui::testing::TuiDriver`] /
//! [`crate::gtk::testing::GtkDriver`]'s surface — `new`, `render`,
//! `dispatch`/`press`/`type_char`/`click` — so the same scripted
//! [`UiEvent`]s that drive those two also drive the macOS backend.
//!
//! ## Display-free (verified in-repo)
//!
//! Renders into a headless [`super::headless::BitmapSurface`]
//! (`CGBitmapContext`) via [`super::backend::MacBackend::enter_frame_scope`]
//! — no `NSApplication`, no `NSWindow`, no display server. This is the
//! exact display-free path `macos/headless.rs`'s own
//! `integrates_with_mac_backend_frame_scope` test already proves works;
//! `MacDriver` lifts it to the whole-`AppLogic` level, the same way
//! `GtkDriver` lifted GTK's headless `cairo::ImageSurface` path.
//!
//! ## No drift from production
//!
//! [`Self::render`] and [`Self::dispatch`] call the same
//! [`super::run::render_frame`] / [`super::run::dispatch_event`] the live
//! `quadraui::macos::run` runner uses (mirroring quadraui#446's GTK split
//! and quadraui#300's TUI split), so the test path paints and
//! pre-processes events (caret-blink bump, double-click folding, global
//! accelerator dispatch, Cmd-V paste) identically to production.
//!
//! ## Limitations
//!
//! It renders into an in-memory `CGBitmapContext`, so it does **not**
//! exercise real `NSEvent` delivery — raw `NSEvent`/keycode translation
//! (`macos::events`), IME, or actual `NSResponder` wiring are out of
//! scope and need a live-window smoke test instead. `MacBackend` also
//! doesn't yet support text selection or scrollbar drag (`backend_caps`
//! declares neither), so [`Self::drag`]-based scenarios that depend on
//! either are `Anchor`/`ConformanceDriver::backend_caps`-gated the same
//! way they are on every other backend.
//!
//! ```no_run
//! # use quadraui::macos::testing::MacDriver;
//! # use quadraui::{AppLogic, Backend, Reaction, UiEvent};
//! # fn demo<A: AppLogic + Default>() {
//! let mut driver = MacDriver::new(A::default(), 100, 30);
//! driver.type_char('x');
//! let _ = driver.pixel(0, 0);
//! # }
//! ```

use crate::backend::Backend;
use crate::runner::{AppLogic, Reaction};
use crate::shell::{ShellApp, ShellConfig};
use crate::testing::{Anchor, ConformanceDriver, FrameInventory, LogicalViewport};
use crate::{ButtonMask, Key, Modifiers, MouseButton, NamedKey, Point, ScrollDelta, UiEvent};

use super::backend::MacBackend;
use super::headless::BitmapSurface;
use super::run::{dispatch_event, render_frame, EventOutcome};
use super::text::make_font;

/// Build a [`MacDriver`] that wraps `app` in the full
/// [`crate::shell_adapter::ShellAdapter`] stack, mirroring exactly what
/// [`crate::macos::shell_runner::run_with_shell`] does at runtime — but
/// returning a testable driver instead of entering the live `NSApplication`
/// run loop. The macOS twin of [`crate::tui::testing::driver_with_shell`] /
/// [`crate::gtk::testing::driver_with_shell`]; all three share the same
/// [`ShellApp`] + [`ShellConfig`] input, differing only in the native units
/// their respective drivers take (TUI cells vs GTK/macOS points/pixels).
///
/// Use this constructor in tests that need to verify the full
/// `ShellApp → ShellAdapter → dispatch_event` integration path on the macOS
/// backend — e.g. confirming that shell chrome (activity bar, sidebar
/// panel) renders and that panel switches reach the real [`AppShell`]
/// instance [`crate::shell_adapter::ShellAdapter`] paints, not a shadow
/// copy.
///
/// [`AppShell`]: crate::compose::app_shell::AppShell
///
/// # Example
///
/// ```no_run
/// # use quadraui::macos::testing::driver_with_shell;
/// # use quadraui::{ShellApp, ShellConfig, Backend, ShellContext, Reaction, UiEvent};
/// # struct MyApp;
/// # impl ShellApp for MyApp {
/// #     fn render_content(&self, _: &mut dyn Backend, _: &quadraui::compose::app_shell::AppShellLayout) {}
/// #     fn handle(&mut self, _: UiEvent, _: &mut dyn Backend, _: &ShellContext) -> Reaction { Reaction::Continue }
/// # }
/// let config = ShellConfig::new("Demo", vec![]);
/// let mut driver = driver_with_shell(MyApp, config, 800, 480);
/// let _ = driver.pixel(0, 0);
/// ```
pub fn driver_with_shell<A: ShellApp + 'static>(
    app: A,
    config: ShellConfig,
    width: u32,
    height: u32,
) -> MacDriver<impl AppLogic> {
    let adapter = crate::shell_adapter::build_shell_adapter(app, config);
    MacDriver::new(adapter, width, height)
}

/// Drives an [`AppLogic`] impl headlessly against the macOS backend for
/// tests. Construct with [`Self::new`] (runs `setup` + paints the first
/// frame), poke it with [`Self::press`] / [`Self::type_char`] /
/// [`Self::click`] / [`Self::drag`], and read painted pixels back with
/// [`Self::pixel`] or locate painted text with [`Self::find`].
pub struct MacDriver<A: AppLogic> {
    app: A,
    backend: MacBackend,
    surface: BitmapSurface,
    width: u32,
    height: u32,
    exited: bool,
}

impl<A: AppLogic> MacDriver<A> {
    /// Build a driver for `app` on a `width`×`height` pixel surface, run
    /// the app's `setup` hook, and paint the first frame.
    ///
    /// Installs Menlo 14pt as the default font before `setup` runs — the
    /// same default [`super::run::run`] installs before opening a real
    /// window — so backend-trait calls inside `setup` or the first
    /// render find a font to measure against without every test needing
    /// to call `MacBackend::set_current_font` itself. `None` from
    /// [`make_font`] (a font family missing from the host) is a no-op
    /// here too, matching production.
    pub fn new(app: A, width: u32, height: u32) -> Self {
        let surface = BitmapSurface::new(width, height);
        let mut backend = MacBackend::new();
        // Record every painted text run into `MacBackend::text_runs` so
        // `find`/`find_bounds`/`screen_contains` can locate text from
        // *any* primitive — off in production, a live app never reads
        // it (mirrors `GtkBackend::set_painted_text_recording`).
        backend.set_painted_text_recording(true);
        if let Some(font) = make_font("Menlo", 14.0) {
            backend.set_current_font(font);
        }
        // Seed the viewport from the driver's surface size BEFORE
        // setup, exactly as the live `macos::run` does before its first
        // `drawRect:` — without this, `app.setup()` would read
        // `MacBackend::new()`'s zeroed default viewport instead of the
        // requested `width`×`height`.
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
        let viewport = crate::Viewport::new(self.width as f32, self.height as f32, 1.0);
        render_frame(
            &mut self.backend,
            &self.app,
            viewport,
            self.surface.context_ptr(),
        );
    }

    /// Feed one synthetic event through the shared production
    /// [`dispatch_event`] path. Repaints on redraw and latches `exited`.
    pub fn dispatch(&mut self, event: UiEvent) -> Reaction {
        if self.exited {
            return Reaction::Exit;
        }
        let caret_visible = self.backend.caret_visible_handle();
        let caret_pause = self.backend.caret_blink_pause_handle();
        match dispatch_event(
            event,
            &mut self.backend,
            &mut self.app,
            &caret_visible,
            &caret_pause,
        ) {
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

    /// Press a character key with Ctrl held. `MacBackend` has no
    /// Ctrl-C/Ctrl-A text-selection interception of its own (Cmd, not
    /// Ctrl, is the Mac convention — see [`dispatch_event`]'s Cmd-V
    /// handling), so unlike `TuiDriver`/`GtkDriver`'s `ctrl_char` this is
    /// a plain `KeyPressed` with `ctrl: true` — still useful for
    /// `Ctrl`-bound accelerators, which `dispatch_event` does resolve.
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

    /// Left-click at surface coordinates `(x, y)` (points).
    pub fn click(&mut self, x: f32, y: f32) -> Reaction {
        self.mouse_down(x, y)
    }

    /// Press the left mouse button down at `(x, y)`.
    pub fn mouse_down(&mut self, x: f32, y: f32) -> Reaction {
        self.dispatch(UiEvent::MouseDown {
            widget: None,
            button: MouseButton::Left,
            position: Point::new(x, y),
            modifiers: Modifiers::default(),
        })
    }

    /// Move the cursor to `(x, y)` with the left button held.
    pub fn mouse_move(&mut self, x: f32, y: f32) -> Reaction {
        self.dispatch(UiEvent::MouseMoved {
            position: Point::new(x, y),
            buttons: ButtonMask {
                left: true,
                ..ButtonMask::default()
            },
        })
    }

    /// Release the left mouse button at `(x, y)`.
    pub fn mouse_up(&mut self, x: f32, y: f32) -> Reaction {
        self.dispatch(UiEvent::MouseUp {
            widget: None,
            button: MouseButton::Left,
            position: Point::new(x, y),
        })
    }

    /// Left-button drag from `(x0, y0)` to `(x1, y1)`: down → move → up.
    pub fn drag(&mut self, x0: f32, y0: f32, x1: f32, y1: f32) -> Reaction {
        self.mouse_down(x0, y0);
        self.mouse_move(x1, y1);
        self.mouse_up(x1, y1)
    }

    /// Whether the app has returned [`Reaction::Exit`].
    pub fn exited(&self) -> bool {
        self.exited
    }

    /// Access the app state for test assertions.
    pub fn app(&self) -> &A {
        &self.app
    }

    /// Mutable access to the app state for tests that need to poke state
    /// directly rather than through a scripted [`UiEvent`].
    pub fn app_mut(&mut self) -> &mut A {
        &mut self.app
    }

    /// Access the backend for test assertions.
    pub fn backend(&self) -> &MacBackend {
        &self.backend
    }

    /// Access the underlying offscreen surface, e.g. for
    /// [`BitmapSurface::write_ppm_and_open`]-based visual debugging.
    pub fn surface(&self) -> &BitmapSurface {
        &self.surface
    }

    /// Read an RGBA pixel from the rendered surface at `(x, y)` — see
    /// [`BitmapSurface::pixel`] for the byte-order contract.
    pub fn pixel(&self, x: u32, y: u32) -> (u8, u8, u8, u8) {
        self.surface.pixel(x, y)
    }

    /// All text painted during the last [`Self::render`], as recorded at
    /// the [`super::text::draw_text`] choke point (quadraui#493).
    pub fn painted_texts(&self) -> Vec<&str> {
        self.backend
            .text_runs()
            .iter()
            .map(|r| r.text.as_str())
            .collect()
    }

    /// True if any painted text contains `needle` — the macOS analogue of
    /// [`crate::tui::testing::TuiDriver::screen_contains`] /
    /// [`crate::gtk::testing::GtkDriver::screen_contains`].
    pub fn screen_contains(&self, needle: &str) -> bool {
        self.backend
            .text_runs()
            .iter()
            .any(|r| r.text.contains(needle))
    }

    /// Bounds (points) of the first painted text run containing `needle`.
    pub fn find_bounds(&self, needle: &str) -> Option<crate::Rect> {
        self.backend
            .text_runs()
            .iter()
            .find(|r| r.text.contains(needle))
            .map(|r| r.bounds)
    }

    /// Center coordinates (points) of the first painted text run
    /// containing `needle` — pass straight to [`Self::click`]. `None` if
    /// nothing painted this frame matched.
    pub fn find(&self, needle: &str) -> Option<(f32, f32)> {
        self.find_bounds(needle)
            .map(|b| (b.x + b.width / 2.0, b.y + b.height / 2.0))
    }
}

impl<A: AppLogic> ConformanceDriver for MacDriver<A> {
    type App = A;

    fn new_fixture(app: Self::App, viewport: LogicalViewport) -> Self {
        // macOS's native unit is the point (≈pixel at 1.0 backing
        // scale). Scale the logical cols/rows by the same nominal
        // char_width/line_height `MacBackend::new()` starts with — the
        // driver's first frame (and therefore the app's real font
        // metrics) doesn't exist yet to measure from, so this is the
        // same nominal default the backend itself starts with. Mirrors
        // `GtkDriver::new_fixture`.
        const NOMINAL_CHAR_WIDTH: u32 = 8;
        const NOMINAL_LINE_HEIGHT: u32 = 16;
        MacDriver::new(
            app,
            viewport.cols * NOMINAL_CHAR_WIDTH,
            viewport.rows * NOMINAL_LINE_HEIGHT,
        )
    }

    fn backend_caps(&self) -> crate::BackendCaps {
        // Straight off the real `MacBackend` this driver wraps — never a
        // re-statement (quadraui#492).
        crate::Backend::backend_caps(&self.backend)
    }

    fn press_named(&mut self, key: NamedKey) {
        MacDriver::press_named(self, key);
    }

    fn type_char(&mut self, c: char) {
        MacDriver::type_char(self, c);
    }

    fn ctrl_char(&mut self, c: char) {
        MacDriver::ctrl_char(self, c);
    }

    fn click_text_at(&mut self, needle: &str, at: Anchor) {
        let bounds = self
            .find_bounds(needle)
            .unwrap_or_else(|| panic!("MacDriver: {needle:?} not painted"));
        let y = bounds.y + bounds.height / 2.0;
        let x = match at {
            Anchor::Center => bounds.x + bounds.width / 2.0,
            Anchor::LeftEdge => bounds.x + 1.0,
            Anchor::RightEdge => bounds.x + bounds.width - 1.0,
        };
        self.click(x, y);
    }

    fn drag_text(&mut self, from: &str, to: &str) {
        let (x0, y0) = self
            .find(from)
            .unwrap_or_else(|| panic!("MacDriver: {from:?} not painted"));
        let (x1, y1) = self
            .find(to)
            .unwrap_or_else(|| panic!("MacDriver: {to:?} not painted"));
        self.drag(x0, y0, x1, y1);
    }

    fn scroll_at(&mut self, needle: &str, lines: i32) {
        let (x, y) = self
            .find(needle)
            .unwrap_or_else(|| panic!("MacDriver: {needle:?} not painted"));
        let line_height = crate::Backend::line_height(&self.backend);
        self.dispatch(UiEvent::Scroll {
            widget: None,
            delta: ScrollDelta::new(0.0, lines as f32 * line_height),
            position: Point::new(x, y),
        });
    }

    fn inventory(&self) -> FrameInventory {
        FrameInventory {
            text_runs: self.backend.text_runs().to_vec(),
            zones: self.backend.zones().to_vec(),
        }
    }

    fn screen_has(&self, needle: &str) -> bool {
        self.screen_contains(needle)
    }

    fn exited(&self) -> bool {
        MacDriver::exited(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::status_bar::{StatusBar, StatusBarSegment};
    use crate::types::{Color, WidgetId};
    use crate::Rect;

    const W: u32 = 120;
    const H: u32 = 24;
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

    /// Acceptance test: build an `AppLogic`, render a frame offscreen (no
    /// window, no `NSApplication`), and read back the segment's fill
    /// colour sampled across its whole bounds — the dominant colour, not
    /// a single hardcoded coordinate, since a lone pixel could land on an
    /// anti-aliased glyph stroke instead of the background fill. Mirrors
    /// `GtkDriver`'s `find_locates_status_bar_segment_by_text`.
    #[test]
    fn find_locates_status_bar_segment_by_text_and_reads_back_known_pixel() {
        let driver = MacDriver::new(StatusBarApp, W, H);

        let bounds = driver
            .find_bounds("known-pixel")
            .expect("find_bounds should locate the painted segment");
        assert_eq!(
            (bounds.x, bounds.y),
            (0.0, 0.0),
            "the only segment should start at the bar's origin"
        );

        let (x, y) = driver
            .find("known-pixel")
            .expect("find should locate the painted segment label");
        assert!(x >= bounds.x && x <= bounds.x + bounds.width);
        assert!(y >= bounds.y && y <= bounds.y + bounds.height);

        let (r, g, b, _a) = dominant_pixel(&driver, bounds);
        assert_eq!(
            (r, g, b),
            (KNOWN_BG.r, KNOWN_BG.g, KNOWN_BG.b),
            "dominant colour within find_bounds() should be the segment's bg, got ({r}, {g}, {b})"
        );

        assert!(driver.screen_contains("known-pixel"));
        assert!(!driver.screen_contains("no such label"));
    }

    /// Most common `(r, g, b, a)` pixel within `bounds` — the background
    /// colour, since it covers far more area than a label's thin
    /// anti-aliased glyph strokes.
    fn dominant_pixel<A: AppLogic>(driver: &MacDriver<A>, bounds: Rect) -> (u8, u8, u8, u8) {
        use std::collections::HashMap;

        let mut counts: HashMap<(u8, u8, u8, u8), u32> = HashMap::new();
        for py in bounds.y as u32..(bounds.y + bounds.height) as u32 {
            for px in bounds.x as u32..(bounds.x + bounds.width) as u32 {
                *counts.entry(driver.pixel(px, py)).or_insert(0) += 1;
            }
        }
        counts
            .into_iter()
            .max_by_key(|(_, count)| *count)
            .map(|(color, _)| color)
            .expect("bounds should contain at least one pixel")
    }

    /// Fixture analogous to `GtkDriver`'s `ToggleStatusBarApp`: one
    /// clickable `StatusBar` segment that flips its background colour on
    /// click. No live app, no example wiring — just an `AppLogic` a test
    /// can hand straight to [`MacDriver::new`].
    struct ToggleStatusBarApp {
        on: bool,
    }

    impl ToggleStatusBarApp {
        const OFF_BG: Color = Color::rgb(40, 40, 40);
        const ON_BG: Color = Color::rgb(0, 200, 0);

        fn bar(&self) -> StatusBar {
            StatusBar {
                id: WidgetId::new("status"),
                left_segments: vec![StatusBarSegment {
                    text: "Toggle".to_string(),
                    fg: Color::rgb(255, 255, 255),
                    bg: if self.on { Self::ON_BG } else { Self::OFF_BG },
                    bold: false,
                    action_id: Some(WidgetId::new("toggle")),
                }],
                right_segments: vec![],
            }
        }
    }

    impl AppLogic for ToggleStatusBarApp {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            backend.draw_status_bar(
                Rect::new(0.0, 0.0, W as f32, H as f32),
                &self.bar(),
                None,
                None,
            );
        }

        fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
            if let UiEvent::MouseDown { .. } = event {
                self.on = !self.on;
                return Reaction::Redraw;
            }
            Reaction::Continue
        }
    }

    /// Click round-trips paint → hit_test-by-text → `handle` → re-render,
    /// with no hardcoded coordinates — `find` locates the painted
    /// "Toggle" label and `click` lands on its center.
    #[test]
    fn click_toggles_segment_colour() {
        let mut driver = MacDriver::new(ToggleStatusBarApp { on: false }, W, H);
        let bounds = driver
            .find_bounds("Toggle")
            .expect("Toggle segment should be painted");
        let (r0, g0, b0, _) = dominant_pixel(&driver, bounds);
        assert_eq!(
            (r0, g0, b0),
            (
                ToggleStatusBarApp::OFF_BG.r,
                ToggleStatusBarApp::OFF_BG.g,
                ToggleStatusBarApp::OFF_BG.b
            )
        );

        let (x, y) = driver.find("Toggle").expect("Toggle label painted");
        let reaction = driver.click(x, y);
        assert_eq!(reaction, Reaction::Redraw, "click should trigger a redraw");

        let bounds = driver
            .find_bounds("Toggle")
            .expect("Toggle segment still painted after click");
        let (r1, g1, b1, _) = dominant_pixel(&driver, bounds);
        assert_eq!(
            (r1, g1, b1),
            (
                ToggleStatusBarApp::ON_BG.r,
                ToggleStatusBarApp::ON_BG.g,
                ToggleStatusBarApp::ON_BG.b
            ),
            "after the click the segment should read back the ON colour"
        );
    }

    /// `q` typed through [`MacDriver::type_char`] round-trips through the
    /// same [`super::run::dispatch_event`] preprocessing production uses.
    #[test]
    fn type_char_reaches_app_handle() {
        struct QuitsOnQ {
            quit: bool,
        }
        impl AppLogic for QuitsOnQ {
            type AreaId = ();
            fn render(&self, _backend: &mut dyn Backend, _area: ()) {}
            fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
                if let UiEvent::KeyPressed {
                    key: Key::Char('q'),
                    ..
                } = event
                {
                    self.quit = true;
                    return Reaction::Exit;
                }
                Reaction::Continue
            }
        }

        let mut driver = MacDriver::new(QuitsOnQ { quit: false }, W, H);
        assert!(!driver.exited());
        driver.type_char('q');
        assert!(driver.app().quit, "app.handle should have run");
        assert!(driver.exited(), "'q' should make the app exit");
    }

    // ── ActivityBar keyboard-focus redirect (#465) ────────────────────

    /// App that paints one [`crate::primitives::activity_bar::ActivityBar`]
    /// whose `is_keyboard_focused` flag is fixed at construction, and
    /// records every event `dispatch_event` hands it.
    struct ActivityBarApp {
        focused: bool,
        seen: Vec<UiEvent>,
    }

    impl AppLogic for ActivityBarApp {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            use crate::primitives::activity_bar::{ActivityBar, ActivityItem};
            backend.draw_activity_bar(
                Rect::new(0.0, 0.0, W as f32, H as f32),
                &ActivityBar {
                    id: WidgetId::new("bar"),
                    top_items: vec![ActivityItem {
                        id: WidgetId::new("panel:explorer"),
                        icon: "E".into(),
                        tooltip: "Explorer".into(),
                        is_active: true,
                        is_keyboard_selected: self.focused,
                    }],
                    bottom_items: Vec::new(),
                    active_accent: None,
                    active_bg: None,
                    selection_bg: None,
                    is_keyboard_focused: self.focused,
                },
                None,
            );
        }

        fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
            self.seen.push(event);
            Reaction::Continue
        }
    }

    /// Regression for #465: while a painted `ActivityBar` declares
    /// `is_keyboard_focused`, `dispatch_event` must hand the app
    /// `UiEvent::ActivityBar(bar_id, ActivityBarEvent::KeyPressed { … })`
    /// — the synthesized form `ShellAdapter`'s built-in activity-bar
    /// navigation (#409) matches on — rather than the raw `KeyPressed`.
    /// TUI (`TuiBackend::apply_dispatch`) and GTK (`gtk::run::dispatch_event`)
    /// already did this; macOS did not, so every `ShellApp` driven through
    /// `macos::shell_runner::run_with_shell` silently lost `j`/`k`/`Enter`
    /// panel navigation.
    #[test]
    fn keypress_redirects_to_focused_activity_bar() {
        let mut driver = MacDriver::new(
            ActivityBarApp {
                focused: true,
                seen: Vec::new(),
            },
            W,
            H,
        );

        driver.type_char('j');
        driver.press_named(crate::NamedKey::Enter);

        let keys: Vec<String> = driver
            .app()
            .seen
            .iter()
            .filter_map(|ev| match ev {
                UiEvent::ActivityBar(id, crate::ActivityBarEvent::KeyPressed { key, .. }) => {
                    Some(format!("{}:{key}", id.as_str()))
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            keys,
            vec!["bar:j".to_string(), "bar:Enter".to_string()],
            "focused bar must receive normalised ActivityBar key events, got {:?}",
            driver.app().seen
        );
        assert!(
            !driver
                .app()
                .seen
                .iter()
                .any(|ev| matches!(ev, UiEvent::KeyPressed { .. })),
            "no raw KeyPressed should leak through while the bar is focused"
        );
    }

    /// The complement: with no keyboard-focused bar painted, keys reach the
    /// app unchanged — the redirect must not swallow ordinary typing.
    #[test]
    fn keypress_untouched_when_no_activity_bar_is_focused() {
        let mut driver = MacDriver::new(
            ActivityBarApp {
                focused: false,
                seen: Vec::new(),
            },
            W,
            H,
        );

        driver.type_char('j');

        assert!(
            driver.app().seen.iter().any(|ev| matches!(
                ev,
                UiEvent::KeyPressed {
                    key: Key::Char('j'),
                    ..
                }
            )),
            "unfocused bar must not intercept: {:?}",
            driver.app().seen
        );
    }
}
