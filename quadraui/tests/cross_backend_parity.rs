//! Cross-backend parity test (quadraui#448, GD-3) — the headline payoff of
//! the driver-test epic (#301): one `AppLogic`, one scripted event
//! sequence, run against *both* [`TuiDriver`] and [`GtkDriver`], asserting
//! the two backends agree on logical state. A pty-only tool can never do
//! this — it only ever sees one backend.
//!
//! Follows the `ExampleDriver` trait shape already sketched in
//! `docs/TESTING.md` ("Cross-backend example tests: shared bodies,
//! per-backend adapters"): the test body is written **once**, generic over
//! `ExampleDriver`, and the two backend-specific rows (reading "the
//! screen", coordinate units) are hidden behind the trait's small surface.
//! Locate targets with `find`/`click_text`, never a hardcoded coordinate —
//! TUI cells and GTK pixels are different units, so a literal `click(x, y)`
//! in a shared body would silently be wrong on one side.
//!
//! Needs both drivers compiled in, so this file only runs under
//! `--features tui,gtk` (the CI `gtk` job builds/tests with both).
#![cfg(all(feature = "tui", feature = "gtk"))]

use quadraui::gtk::testing::GtkDriver;
use quadraui::tui::testing::TuiDriver;
use quadraui::{AppLogic, NamedKey};

#[path = "../examples/common/pipeline_app.rs"]
mod pipeline_app;
use pipeline_app::PipelineApp;

/// Backend-agnostic driver surface a shared parity test body needs.
/// Implemented once per backend below — the two rows that genuinely
/// differ (screen representation, coordinate units) live entirely inside
/// these impls, never in the shared test bodies.
trait ExampleDriver {
    fn press_named(&mut self, key: NamedKey);
    fn type_char(&mut self, c: char);
    /// Locate `needle`'s painted bounds in this backend's native
    /// coordinate space and click its center.
    fn click_text(&mut self, needle: &str);
    fn screen_has(&self, needle: &str) -> bool;
    fn exited(&self) -> bool;
}

impl<A: AppLogic> ExampleDriver for TuiDriver<A> {
    fn press_named(&mut self, key: NamedKey) {
        TuiDriver::press_named(self, key);
    }

    fn type_char(&mut self, c: char) {
        TuiDriver::type_char(self, c);
    }

    fn click_text(&mut self, needle: &str) {
        let (x, y) = self
            .find(needle)
            .unwrap_or_else(|| panic!("TuiDriver: {needle:?} not painted:\n{}", self.screen()));
        self.click(x, y);
    }

    fn screen_has(&self, needle: &str) -> bool {
        self.screen_contains(needle)
    }

    fn exited(&self) -> bool {
        TuiDriver::exited(self)
    }
}

impl<A: AppLogic> ExampleDriver for GtkDriver<A> {
    fn press_named(&mut self, key: NamedKey) {
        GtkDriver::press_named(self, key);
    }

    fn type_char(&mut self, c: char) {
        GtkDriver::type_char(self, c);
    }

    fn click_text(&mut self, needle: &str) {
        let (x, y) = self
            .find(needle)
            .unwrap_or_else(|| panic!("GtkDriver: {needle:?} not painted"));
        self.click(x, y);
    }

    fn screen_has(&self, needle: &str) -> bool {
        self.screen_contains(needle)
    }

    fn exited(&self) -> bool {
        GtkDriver::exited(self)
    }
}

/// One scripted event sequence, written once: move focus right, fire the
/// Deploy stage's "Go" action by text (no per-backend coordinates), then
/// quit. Returns the observations a test wants to compare across backends.
fn run_pipeline_script<D: ExampleDriver>(d: &mut D) -> Vec<bool> {
    let before = d.screen_has("stage 3");
    d.press_named(NamedKey::Right);
    d.click_text("Go");
    let after_click = d.screen_has("stage 3");
    d.type_char('q');
    vec![before, after_click, d.exited()]
}

#[test]
fn pipeline_parity_tui_and_gtk_agree_on_logical_state() {
    // TUI uses a character-cell grid (~1 cell tall lines); GTK a pixel
    // surface (~16px lines) -- different units for the *same* logical
    // widget, per docs/TESTING.md's "Coordinate units" row. `click_text`
    // hides that: each driver resolves "Go" in its own native space.
    let mut tui = TuiDriver::new(PipelineApp::new(), 100, 30);
    let mut gtk = GtkDriver::new(PipelineApp::new(), 800, 480);

    let tui_observations = run_pipeline_script(&mut tui);
    let gtk_observations = run_pipeline_script(&mut gtk);

    assert_eq!(
        tui_observations, gtk_observations,
        "TUI and GTK should reach the same logical state \
         (mentions-stage-3 before click, after click, exited-on-q) \
         for the identical PipelineApp event script"
    );
    // Pin down what parity actually means here, not just "the two vecs
    // matched by coincidence" -- spell out the expected shape.
    assert_eq!(
        tui_observations,
        vec![false, true, true],
        "expected: no stage-3 mention before the click, a mention after \
         clicking Go, and exited after 'q'"
    );
}
