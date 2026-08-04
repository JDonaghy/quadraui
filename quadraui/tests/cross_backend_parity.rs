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

use quadraui::gtk::testing::{driver_with_shell as gtk_driver_with_shell, GtkDriver};
use quadraui::tui::testing::{driver_with_shell as tui_driver_with_shell, TuiDriver};
use quadraui::{AppLogic, NamedKey};

#[path = "../examples/common/pipeline_app.rs"]
mod pipeline_app;
use pipeline_app::PipelineApp;

#[path = "../examples/common/appshell_demo.rs"]
mod appshell_demo;
use appshell_demo::AppShellDemo;

#[path = "../examples/common/data_table_app.rs"]
mod data_table_app;
use data_table_app::DataTableApp;

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

// ─── Shell-level parity (quadraui#518): a ShellApp, not just an AppLogic ───
//
// `pipeline_parity_*` above proves parity for the `AppLogic`-level harness
// (#448/GD-3). But every *real* quadraui consumer (coord-tui included)
// implements `ShellApp` and is driven by `{tui,gtk}::testing::driver_with_shell`,
// not `AppLogic` directly — so this lifts the same claim to the shell level:
// one `ShellApp` (`AppShellDemo`), one script, both `driver_with_shell`
// entry points, same logical outcome. `ExampleDriver` is reused unchanged —
// both `driver_with_shell` functions return a driver generic over
// `impl AppLogic` (the `ShellAdapter` wrapping the `ShellApp`), which the
// existing blanket `impl<A: AppLogic> ExampleDriver for {Tui,Gtk}Driver<A>`
// already covers.

/// Script: focus the activity bar, move the keyboard cursor down two items
/// (explorer → search → git), activate the selection, then quit. Returns
/// the observations a test wants to compare across backends: whether the
/// sidebar-header text for the *destination* panel appears before the
/// switch, after it, and whether 'q' exits.
fn run_appshell_script<D: ExampleDriver>(d: &mut D) -> Vec<bool> {
    let before = d.screen_has("SOURCE CONTROL");
    d.press_named(NamedKey::Tab);
    d.type_char('j');
    d.type_char('j');
    d.press_named(NamedKey::Enter);
    let after_activate = d.screen_has("SOURCE CONTROL");
    d.type_char('q');
    vec![before, after_activate, d.exited()]
}

#[test]
fn appshell_demo_parity_tui_and_gtk_agree_on_logical_state() {
    // Same native-unit split as the `AppLogic`-level test: TUI cells vs
    // GTK pixels for the identical logical shell chrome.
    let mut tui = tui_driver_with_shell(AppShellDemo::new(), AppShellDemo::config(), 100, 30);
    let mut gtk = gtk_driver_with_shell(AppShellDemo::new(), AppShellDemo::config(), 800, 480);

    let tui_observations = run_appshell_script(&mut tui);
    let gtk_observations = run_appshell_script(&mut gtk);

    assert_eq!(
        tui_observations, gtk_observations,
        "TUI and GTK should reach the same logical state \
         (mentions-SOURCE-CONTROL before activation, after activation, exited-on-q) \
         for the identical AppShellDemo (ShellApp) event script"
    );
    assert_eq!(
        tui_observations,
        vec![false, true, true],
        "expected: no Source Control mention before activating the cursor item, \
         a mention after Tab+j+j+Enter switches to panel:git, and exited after 'q'"
    );
}

// ─── DataTableApp resize-direction parity (#516 defect 3) ──────────────────
//
// The acceptance bar for defect 3 is specifically that the fix landed in
// the *shared* `primitives::data_table::resolve_columns`, not in one
// rasteriser — so the same divider-drag script must produce the same
// *direction* of change on both backends. `ExampleDriver` above is
// generic over any `AppLogic` and can't reach `DataTableApp`-specific
// methods (`table_layout`, `resolved_column_widths`), so this uses a
// narrower trait implemented directly for the two concrete driver types
// instead — same "one script body, two backends" shape, just scoped to
// what this script needs.
trait DataTableResizeDriver {
    /// Resolved width of the "Age" column (index 2) — the one directly
    /// before the last column ("Restarts"), the divider this script drags.
    fn age_width(&self) -> f32;
    /// Drag the Age|Restarts divider horizontally by `dx` (positive =
    /// right = widen) in this backend's native coordinate space.
    fn drag_age_divider(&mut self, dx: f32);
}

impl DataTableResizeDriver for TuiDriver<DataTableApp> {
    fn age_width(&self) -> f32 {
        self.app().resolved_column_widths(self.backend())[2]
    }

    fn drag_age_divider(&mut self, dx: f32) {
        let layout = self.app().table_layout(self.backend());
        let age = layout.columns[2];
        let divider_x = age.x + age.width;
        let y = 0.5;
        self.drag(divider_x, y, divider_x + dx, y);
    }
}

impl DataTableResizeDriver for GtkDriver<DataTableApp> {
    fn age_width(&self) -> f32 {
        self.app().resolved_column_widths(self.backend())[2]
    }

    fn drag_age_divider(&mut self, dx: f32) {
        let layout = self.app().table_layout(self.backend());
        let age = layout.columns[2];
        let divider_x = age.x + age.width;
        let y = layout.header_height / 2.0;
        self.drag(divider_x, y, divider_x + dx, y);
    }
}

/// One script, run against both concrete driver types: widen by dragging
/// right, then widen further by a larger amount from scratch on a fresh
/// driver (avoiding a second same-position `mouse_down`, which the TUI
/// backend's double-click detector would fold into a `DoubleClick`
/// instead of a fresh resize drag — see the sibling test in
/// `tests/tui_example_driver.rs` for the full explanation). Returns
/// whether each drag widened the column, for both backends to agree on.
fn run_datatable_resize_script<D: DataTableResizeDriver + DataTableDriverCtor>(dx: f32) -> bool {
    let mut d = D::new_default();
    let before = d.age_width();
    d.drag_age_divider(dx);
    let after = d.age_width();
    after > before
}

/// Constructs a fresh driver wrapping a fresh `DataTableApp` — kept
/// separate from `DataTableResizeDriver` so the resize trait stays
/// focused on the drag script itself.
trait DataTableDriverCtor {
    fn new_default() -> Self;
}

impl DataTableDriverCtor for TuiDriver<DataTableApp> {
    fn new_default() -> Self {
        TuiDriver::new(DataTableApp::new(), 100, 26)
    }
}

impl DataTableDriverCtor for GtkDriver<DataTableApp> {
    fn new_default() -> Self {
        GtkDriver::new(DataTableApp::new(), 900, 600)
    }
}

#[test]
fn data_table_divider_before_last_column_parity_tui_and_gtk_agree_on_direction() {
    // Dragging the divider immediately before the last column (Age |
    // Restarts) right must widen Age on *both* backends — the literal
    // reported symptom was that this direction inverted. Age is
    // Flex-declared and Restarts (last) is Fixed, the exact shape that
    // reproduced the bug (see `resolve_columns`'s pass-2 doc comment in
    // `src/primitives/data_table.rs` for the root cause).
    let tui_widened = run_datatable_resize_script::<TuiDriver<DataTableApp>>(60.0);
    let gtk_widened = run_datatable_resize_script::<GtkDriver<DataTableApp>>(120.0);

    assert_eq!(
        tui_widened, gtk_widened,
        "TUI and GTK should agree on the resize direction for the identical drag script"
    );
    assert!(
        tui_widened && gtk_widened,
        "dragging the divider before the last column right should widen it on both backends"
    );
}
