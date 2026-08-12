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
use crate::shell::{ShellApp, ShellConfig};
use crate::testing::{Anchor, ConformanceDriver, FrameInventory, LogicalViewport, TextRun};
use crate::{ButtonMask, Key, Modifiers, MouseButton, NamedKey, Point, ScrollDelta, UiEvent};

use super::backend::GtkBackend;
use super::run::{dispatch_event, render_frame, EventOutcome};

/// Build a [`GtkDriver`] that wraps `app` in the full
/// [`crate::shell_adapter::ShellAdapter`] stack, mirroring exactly what
/// [`crate::gtk::shell_runner::run_with_shell`] does at runtime — but
/// returning a testable driver instead of entering the live event loop.
/// The GTK twin of [`crate::tui::testing::driver_with_shell`]; the two share
/// the same [`crate::shell::ShellApp`] + [`ShellConfig`] input, differing
/// only in the native units their respective drivers take (TUI cells vs
/// GTK pixels, per [`GtkDriver::new`]).
///
/// Use this constructor in tests that need to verify the full
/// `ShellApp → ShellAdapter → dispatch_event` integration path on the GTK
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
/// # use quadraui::gtk::testing::driver_with_shell;
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
    width: i32,
    height: i32,
) -> GtkDriver<impl AppLogic> {
    let adapter = super::shell_runner::build_shell_adapter(app, config);
    GtkDriver::new(adapter, width, height)
}

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
        // Record every painted text run into `GtkBackend::painted_text`
        // so `find`/`find_bounds`/`screen_contains` can locate text from
        // *any* primitive, not just the three whose trait methods
        // hand-roll a `record_painted_text` call (quadraui#489). Off in
        // production runners — a live app never reads the map.
        backend.set_painted_text_recording(true);
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

    /// Mutable access to the app state for tests that need to poke state
    /// directly rather than through a scripted [`UiEvent`] — mirrors
    /// [`crate::tui::testing::TuiDriver::app_mut`] (quadraui#488).
    pub fn app_mut(&mut self) -> &mut A {
        &mut self.app
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

    /// Raw `ARgb32` pixel buffer of the last [`Self::render`] — the
    /// `screen()`-equivalent GD-2 raw-buffer accessor. Four bytes per
    /// pixel, `[B, G, R, A]` (see [`Self::pixel`]); row stride is
    /// [`Self::stride`], not necessarily `width * 4`. Copies out of the
    /// surface (Cairo's borrow guard can't outlive this call) — prefer
    /// [`Self::pixel`] / [`Self::find`] / [`Self::screen_contains`] for
    /// assertions; this is the escape hatch for tests that need the whole
    /// buffer (e.g. diffing two frames).
    pub fn screen(&mut self) -> Vec<u8> {
        self.surface.flush();
        self.surface.data().expect("surface data").to_vec()
    }

    /// Row stride (bytes per row) of the surface [`Self::screen`] reads
    /// back from. Cairo pads rows for alignment, so this can exceed
    /// `width * 4`.
    pub fn stride(&self) -> i32 {
        self.surface.stride()
    }

    /// All text painted during the last [`Self::render`], as recorded by
    /// [`super::backend::GtkBackend::record_painted_text`] — the
    /// `(text, bounds)` map [`Self::find`] / [`Self::find_bounds`] /
    /// [`Self::screen_contains`] query.
    ///
    /// Every text-bearing primitive reports in (quadraui#489), recorded at
    /// the `show_layout` choke point every GTK rasteriser paints through —
    /// so a run here is one Pango run, which for multi-span content (a
    /// styled tree row, a syntax-highlighted editor line) means one entry
    /// *per span* rather than one per logical row. Match on a substring
    /// within a span, the way [`Self::screen_contains`] does, rather than
    /// on a phrase that straddles a style change. The exceptions are
    /// `draw_terminal` (a per-cell grid — deliberately not recorded) and
    /// the primitives that paint no text at all (`draw_split`,
    /// `draw_split_tree`, `draw_scrollbar`, `draw_drop_overlay`).
    pub fn painted_texts(&self) -> Vec<&str> {
        self.backend
            .painted_text
            .iter()
            .map(|p| p.text.as_str())
            .collect()
    }

    /// True if any painted label contains `needle` — the GTK analogue of
    /// [`crate::tui::testing::TuiDriver::screen_contains`].
    pub fn screen_contains(&self, needle: &str) -> bool {
        self.backend
            .painted_text
            .iter()
            .any(|p| p.text.contains(needle))
    }

    /// Pixel bounds (backend coordinates) of the first painted label
    /// containing `needle`, via the `(text, bounds)` map recorded at
    /// paint time (quadraui#447, GD-2) — mirrors the `TuiDriver::find`
    /// rule (*locate targets with `find`, never hardcode coords*), but
    /// resolved from Pango-measured layout geometry rather than a
    /// character grid.
    pub fn find_bounds(&self, needle: &str) -> Option<crate::Rect> {
        self.backend
            .painted_text
            .iter()
            .find(|p| p.text.contains(needle))
            .map(|p| p.bounds)
    }

    /// Center coordinates (pixels) of the first painted label containing
    /// `needle` — pass straight to [`Self::click`]. `None` if nothing
    /// painted this frame matched.
    pub fn find(&self, needle: &str) -> Option<(f32, f32)> {
        self.find_bounds(needle)
            .map(|b| (b.x + b.width / 2.0, b.y + b.height / 2.0))
    }
}

impl<A: AppLogic> ConformanceDriver for GtkDriver<A> {
    type App = A;

    fn new_fixture(app: Self::App, viewport: LogicalViewport) -> Self {
        // GTK's native unit is the pixel. Scale the logical cols/rows by
        // `GtkBackend::new()`'s nominal char_width/line_height (8px/16px)
        // — the driver's first frame (and therefore the app's real font
        // metrics) doesn't exist yet to measure from, so this is the same
        // nominal default the backend itself starts with.
        const NOMINAL_CHAR_WIDTH: i32 = 8;
        const NOMINAL_LINE_HEIGHT: i32 = 16;
        GtkDriver::new(
            app,
            viewport.cols as i32 * NOMINAL_CHAR_WIDTH,
            viewport.rows as i32 * NOMINAL_LINE_HEIGHT,
        )
    }

    fn press_named(&mut self, key: NamedKey) {
        GtkDriver::press_named(self, key);
    }

    fn type_char(&mut self, c: char) {
        GtkDriver::type_char(self, c);
    }

    fn ctrl_char(&mut self, c: char) {
        GtkDriver::ctrl_char(self, c);
    }

    fn click_text_at(&mut self, needle: &str, at: Anchor) {
        let bounds = self
            .find_bounds(needle)
            .unwrap_or_else(|| panic!("GtkDriver: {needle:?} not painted"));
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
            .unwrap_or_else(|| panic!("GtkDriver: {from:?} not painted"));
        let (x1, y1) = self
            .find(to)
            .unwrap_or_else(|| panic!("GtkDriver: {to:?} not painted"));
        self.drag(x0, y0, x1, y1);
    }

    fn scroll_at(&mut self, needle: &str, lines: i32) {
        let (x, y) = self
            .find(needle)
            .unwrap_or_else(|| panic!("GtkDriver: {needle:?} not painted"));
        let line_height = self.backend.line_height();
        self.dispatch(UiEvent::Scroll {
            widget: None,
            delta: ScrollDelta::new(0.0, lines as f32 * line_height),
            position: Point::new(x, y),
        });
    }

    fn inventory(&self) -> FrameInventory {
        FrameInventory {
            text_runs: self
                .backend
                .painted_text
                .iter()
                .map(|p| TextRun {
                    text: p.text.clone(),
                    bounds: p.bounds,
                })
                .collect(),
            // Zones registered this frame via `Backend::register_zone` —
            // currently the shell-chrome zones `AppShell::render` records
            // (activity-bar items, sidebar header/content, status bar,
            // ...). Primitives that don't yet call `register_zone`
            // contribute no zone (quadraui#490).
            zones: self.backend.zones.clone(),
        }
    }

    fn screen_has(&self, needle: &str) -> bool {
        self.screen_contains(needle)
    }

    fn exited(&self) -> bool {
        GtkDriver::exited(self)
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

    /// `find`/`find_bounds` locate the segment's label via the
    /// `(text, bounds)` map [`GtkBackend::draw_status_bar`] records — the
    /// coordinate-free counterpart to
    /// [`renders_offscreen_and_reads_back_known_pixel`]'s hardcoded
    /// `pixel(4, H / 2)` (quadraui#447, GD-2).
    #[test]
    fn find_locates_status_bar_segment_by_text() {
        let mut driver = GtkDriver::new(StatusBarApp, W, H);

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
        assert!(
            x >= bounds.x && x <= bounds.x + bounds.width,
            "find()'s x should fall within the segment's own bounds"
        );
        assert!(
            y >= bounds.y && y <= bounds.y + bounds.height,
            "find()'s y should fall within the segment's own bounds"
        );

        // Sample the whole segment bounds and take the most common
        // colour — the background fill, since it covers far more area
        // than the label's thin anti-aliased glyph strokes — rather than
        // guessing a coordinate known to dodge the text.
        let (r, g, b) = dominant_pixel(&mut driver, bounds);
        assert_eq!(
            (r, g, b),
            (KNOWN_BG.r, KNOWN_BG.g, KNOWN_BG.b),
            "dominant colour within find_bounds() should be the segment's bg, got ({r}, {g}, {b})"
        );

        assert!(driver.screen_contains("known-pixel"));
        assert!(!driver.screen_contains("no such label"));
    }

    /// Most common `(r, g, b)` pixel within `bounds` — the background
    /// colour, since it covers far more area than a label's thin
    /// anti-aliased glyph strokes. Used to assert on a text-tight
    /// segment's fill colour without hardcoding a coordinate known (by
    /// construction) to dodge the text.
    fn dominant_pixel<A: AppLogic>(driver: &mut GtkDriver<A>, bounds: Rect) -> (u8, u8, u8) {
        use std::collections::HashMap;

        let mut counts: HashMap<(u8, u8, u8), u32> = HashMap::new();
        for py in bounds.y as i32..(bounds.y + bounds.height) as i32 {
            for px in bounds.x as i32..(bounds.x + bounds.width) as i32 {
                *counts.entry(driver.pixel(px, py)).or_insert(0) += 1;
            }
        }
        counts
            .into_iter()
            .max_by_key(|(_, count)| *count)
            .map(|(color, _)| color)
            .expect("bounds should contain at least one pixel")
    }

    /// Fixture analogous to the issue's `make_test_app(BoardData)`: a
    /// full GTK view built purely from in-memory state — one clickable
    /// `StatusBar` segment that flips its background colour on click.
    /// No live app, no example wiring — just an `AppLogic` a test can
    /// hand straight to [`GtkDriver::new`].
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

        fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
            // Hit-test via the non-painting `status_bar_layout` query
            // (mirrors CLAUDE.md's "cached layout hit-test pattern" —
            // recompute the layout from state, don't repaint to hit-test).
            if let UiEvent::MouseDown { position, .. } = event {
                let layout =
                    backend.status_bar_layout(Rect::new(0.0, 0.0, W as f32, H as f32), &self.bar());
                if layout.hit_test(position.x, position.y)
                    == crate::primitives::status_bar::StatusBarHit::Segment(WidgetId::new("toggle"))
                {
                    self.on = !self.on;
                    return Reaction::Redraw;
                }
            }
            Reaction::Continue
        }
    }

    /// GD-2 acceptance test: `find` locates the control (no hardcoded
    /// coords), `click` activates it, and both a pixel-colour probe and a
    /// geometry (bounds) assertion observe the resulting change.
    #[test]
    fn find_click_asserts_pixel_and_geometry_change() {
        let mut driver = GtkDriver::new(ToggleStatusBarApp { on: false }, W, H);

        let (x, y) = driver
            .find("Toggle")
            .expect("find should locate the toggle segment before any click");
        let bounds_before = driver.find_bounds("Toggle").expect("bounds before click");
        // The segment's bounds are text-tight (clickable segments aren't
        // padded out to fill the bar, unlike the trailing non-clickable
        // segment `renders_offscreen_and_reads_back_known_pixel` reads
        // from), so no single fixed pixel is guaranteed to dodge the
        // label's anti-aliased glyphs. Sample the whole bounds rect and
        // take the most common colour — the background, since it covers
        // far more area than the thin glyph strokes.
        let pixel_before = dominant_pixel(&mut driver, bounds_before);
        assert_eq!(
            pixel_before,
            (
                ToggleStatusBarApp::OFF_BG.r,
                ToggleStatusBarApp::OFF_BG.g,
                ToggleStatusBarApp::OFF_BG.b
            ),
            "segment should start OFF-coloured"
        );

        let reaction = driver.click(x, y);
        assert_eq!(reaction, Reaction::Redraw, "click should trigger a redraw");
        assert!(
            driver.app().on,
            "click should have flipped the toggle state"
        );

        // Pixel assertion: same bounds (segment didn't move — the bar
        // layout is unchanged), different dominant colour.
        let pixel_after = dominant_pixel(&mut driver, bounds_before);
        assert_eq!(
            pixel_after,
            (
                ToggleStatusBarApp::ON_BG.r,
                ToggleStatusBarApp::ON_BG.g,
                ToggleStatusBarApp::ON_BG.b
            ),
            "segment should be ON-coloured after the click toggled state"
        );
        assert_ne!(
            pixel_before, pixel_after,
            "click should change the painted pixel"
        );

        // Geometry assertion: re-`find` after the click (the label text
        // is unchanged, so this proves the API works post-redraw too) and
        // confirm the bounds are stable across the repaint.
        let bounds_after = driver
            .find_bounds("Toggle")
            .expect("bounds after click should still resolve");
        assert_eq!(
            bounds_before, bounds_after,
            "segment geometry should be unchanged by a colour-only redraw"
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

    // ─── quadraui#488 ────────────────────────────────────────────────────

    const STACKED_ROW_H: f32 = 20.0;

    /// Two `StatusBar`s stacked vertically at different `rect.y` offsets —
    /// the shape `examples/common/panel_app.rs`'s per-line content
    /// rendering uses, and the one that exposed the `draw_status_bar` bug
    /// this regression test guards.
    struct StackedRowsApp;

    impl AppLogic for StackedRowsApp {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            for (i, label) in ["row zero", "row one"].into_iter().enumerate() {
                backend.draw_status_bar(
                    Rect::new(0.0, i as f32 * STACKED_ROW_H, 200.0, STACKED_ROW_H),
                    &StatusBar {
                        id: WidgetId::new(format!("row-{i}")),
                        left_segments: vec![StatusBarSegment {
                            text: label.to_string(),
                            fg: Color::rgb(255, 255, 255),
                            bg: Color::rgb(20, 20, 20),
                            bold: false,
                            action_id: None,
                        }],
                        right_segments: vec![],
                    },
                    None,
                    None,
                );
            }
        }

        fn handle(&mut self, _event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
            Reaction::Continue
        }
    }

    /// Regression guard for quadraui#488's `GtkBackend::draw_status_bar`
    /// fix: each row's painted text must record its *own* absolute `y`,
    /// not the bar-local `y = 0` every row shared before the fix (which
    /// meant a multi-row caller's `find`/`find_bounds` always reported the
    /// top row's `y` no matter which row's text actually matched).
    #[test]
    fn find_bounds_reports_each_stacked_rows_own_absolute_y() {
        let driver = GtkDriver::new(StackedRowsApp, 200, 60);

        let row0 = driver
            .find_bounds("row zero")
            .expect("row zero should be painted");
        let row1 = driver
            .find_bounds("row one")
            .expect("row one should be painted");

        assert_eq!(row0.y, 0.0, "row 0 should record at its own rect's y");
        assert_eq!(
            row1.y, STACKED_ROW_H,
            "row 1 should record at its own rect's y, not row 0's"
        );
    }

    /// `app_mut` mirrors `TuiDriver::app_mut` (quadraui#488): tests can
    /// poke app state directly rather than only through a scripted
    /// `UiEvent`.
    #[test]
    fn app_mut_allows_direct_state_mutation() {
        let mut driver = GtkDriver::new(ToggleStatusBarApp { on: false }, W, H);
        assert!(!driver.app().on);

        driver.app_mut().on = true;

        assert!(
            driver.app().on,
            "app_mut should mutate the wrapped app in place"
        );
    }

    /// `ConformanceDriver` smoke test for the GTK impl: `new_fixture`
    /// (the `LogicalViewport`-aligned constructor), `click_text_at` with
    /// each `Anchor`, and `screen_has`/`exited` all round-trip through
    /// the promoted trait, not just the inherent methods the other tests
    /// in this module exercise directly.
    #[test]
    fn conformance_driver_new_fixture_and_click_text_at_round_trip() {
        use crate::testing::{Anchor, ConformanceDriver, LogicalViewport};

        let mut driver: GtkDriver<ToggleStatusBarApp> = GtkDriver::new_fixture(
            ToggleStatusBarApp { on: false },
            LogicalViewport::new(30, 5),
        );
        assert!(!ConformanceDriver::exited(&driver));
        assert!(ConformanceDriver::screen_has(&driver, "Toggle"));

        driver.click_text_at("Toggle", Anchor::Center);

        assert!(
            driver.app().on,
            "click_text_at(Anchor::Center) should activate the toggle segment \
             exactly like the existing find()+click() path does"
        );
    }
}
