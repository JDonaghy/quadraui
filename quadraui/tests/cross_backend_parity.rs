//! Cross-backend parity test (quadraui#448, GD-3) — the headline payoff of
//! the driver-test epic (#301): one `AppLogic`, one scripted event
//! sequence, run against *both* [`TuiDriver`] and [`GtkDriver`], asserting
//! the two backends agree on logical state. A pty-only tool can never do
//! this — it only ever sees one backend.
//!
//! Runs each shared test body generic over the promoted
//! [`quadraui::testing::ConformanceDriver`] trait (quadraui#488 — formerly
//! a test-local `ExampleDriver` copy here that dropped `drag_text` from
//! `docs/TESTING.md`'s canonical sketch). The two backend-specific rows
//! (reading "the screen", coordinate units) are hidden behind the trait's
//! small surface. Locate targets with `find`/`click_text`, never a
//! hardcoded coordinate — TUI cells and GTK pixels are different units, so
//! a literal `click(x, y)` in a shared body would silently be wrong on one
//! side.
//!
//! Needs both drivers compiled in, so this file only runs under
//! `--features tui,gtk` (the CI `gtk` job builds/tests with both).
#![cfg(all(feature = "tui", feature = "gtk"))]

use quadraui::gtk::testing::{driver_with_shell as gtk_driver_with_shell, GtkDriver};
use quadraui::testing::{ConformanceDriver, FrameInventory, LogicalViewport};
use quadraui::tui::testing::{driver_with_shell as tui_driver_with_shell, TuiDriver};
use quadraui::{
    AppLogic, Backend, DataTableLayout, NamedKey, Reaction, Rect, Tooltip, TooltipBorder,
    TooltipChrome, TooltipMeasure, TooltipPlacement, UiEvent, WidgetId,
};

#[path = "../examples/common/pipeline_app.rs"]
mod pipeline_app;
use pipeline_app::PipelineApp;

#[path = "../examples/common/appshell_demo.rs"]
mod appshell_demo;
use appshell_demo::{ActivityProbe, AppShellDemo};

#[path = "../examples/common/data_table_app.rs"]
mod data_table_app;
use data_table_app::DataTableApp;

#[path = "../examples/common/panel_app.rs"]
mod panel_app;
use panel_app::PanelApp;

/// Extra backend-specific surface the #552 activity-bar section below
/// needs on top of [`ConformanceDriver`]: raw-coordinate click/hover
/// (always derived from shell-reported geometry — see `activity_row_center`
/// — never a literal) and the one genuinely per-backend number, activity
/// row height. Kept separate from `ConformanceDriver` itself, which by
/// design never exposes raw coordinates to a shared body.
trait ActivityBarProbeDriver: ConformanceDriver {
    /// Click at an explicit point in this backend's native units.
    fn click_at(&mut self, x: f32, y: f32);
    /// Move the pointer, buttons up, to update hover state.
    fn hover_at(&mut self, x: f32, y: f32);
    /// Height of one activity-bar row in this backend's native units.
    /// The TUI bar is one text row per icon, GTK a fixed 48px button.
    fn activity_row_height(&self) -> f32;
}

impl<A: AppLogic> ActivityBarProbeDriver for TuiDriver<A> {
    fn click_at(&mut self, x: f32, y: f32) {
        TuiDriver::click(self, x, y);
    }

    fn hover_at(&mut self, x: f32, y: f32) {
        TuiDriver::mouse_move(self, x, y);
    }

    fn activity_row_height(&self) -> f32 {
        // `tui::activity_bar` lays out with a row height of 1.0 — one
        // terminal text row per icon.
        1.0
    }
}

impl<A: AppLogic> ActivityBarProbeDriver for GtkDriver<A> {
    fn click_at(&mut self, x: f32, y: f32) {
        GtkDriver::click(self, x, y);
    }

    fn hover_at(&mut self, x: f32, y: f32) {
        GtkDriver::mouse_move(self, x, y);
    }

    fn activity_row_height(&self) -> f32 {
        quadraui::gtk::ACTIVITY_ROW_PX as f32
    }
}

/// One scripted event sequence, written once: move focus right, fire the
/// Deploy stage's "Go" action by text (no per-backend coordinates), then
/// quit. Returns the observations a test wants to compare across backends.
fn run_pipeline_script<D: ConformanceDriver>(d: &mut D) -> Vec<bool> {
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

/// macOS twin of the test above (quadraui#493): `MacDriver` now
/// implements `ConformanceDriver`, so `run_pipeline_script` — written
/// once, against the trait, with no per-backend branch — runs unmodified
/// a third time. Kept as its own test (rather than folded into
/// `pipeline_parity_tui_and_gtk_agree_on_logical_state` above) so a build
/// without `macos` — every build except a Mac's — never has to skip
/// anything here; this test simply doesn't exist in that build.
#[cfg(all(feature = "macos", target_os = "macos"))]
#[test]
fn pipeline_parity_macos_agrees_with_tui_and_gtk_on_logical_state() {
    use quadraui::macos::testing::MacDriver;

    let mut tui = TuiDriver::new(PipelineApp::new(), 100, 30);
    let mut mac = MacDriver::new(PipelineApp::new(), 800, 480);

    let tui_observations = run_pipeline_script(&mut tui);
    let mac_observations = run_pipeline_script(&mut mac);

    assert_eq!(
        tui_observations, mac_observations,
        "macOS should reach the same logical state as TUI/GTK \
         (mentions-stage-3 before click, after click, exited-on-q) \
         for the identical PipelineApp event script"
    );
    assert_eq!(
        mac_observations,
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
// entry points, same logical outcome. `ConformanceDriver` is reused
// unchanged — both `driver_with_shell` functions return a driver generic
// over `impl AppLogic` (the `ShellAdapter` wrapping the `ShellApp`), which
// the blanket `impl<A: AppLogic> ConformanceDriver for {Tui,Gtk}Driver<A>`
// in `quadraui::{tui,gtk}::testing` already covers.

/// Script: focus the activity bar, move the keyboard cursor down two items
/// (explorer → search → git), activate the selection, then quit. Returns
/// the observations a test wants to compare across backends: whether the
/// sidebar-header text for the *destination* panel appears before the
/// switch, after it, and whether 'q' exits.
fn run_appshell_script<D: ConformanceDriver>(d: &mut D) -> Vec<bool> {
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

// ─── quadraui#488: promoted `ConformanceDriver` — a shared `drag_text` step ─
//
// `docs/TESTING.md`'s canonical `ExampleDriver` sketch included a
// `drag_text(from, to)` method; the test-local trait this file used to
// define dropped it, so no drag scenario could be written once and shared
// across backends. Promoting it to `quadraui::testing::ConformanceDriver`
// (see the top of this file) restores it — this is the acceptance case:
// one generic test body, one `drag_text` step, constructed via the
// trait's `LogicalViewport`-aligned `new_fixture` and run against both
// drivers unmodified.

/// Drag-select from "brown" (content line 0) to "wizards" (content line 3)
/// across `PanelApp`'s selectable content block — the same fixture and
/// substrings `tests/tui_example_driver.rs::panel_drag_selects_text_and_ctrl_c_copies_it`
/// already proves work — and report whether the shell reacted with a
/// "Selecting row…" status message, i.e. the drag actually started a
/// `TextSelectionChanged` sequence rather than a no-op click.
fn run_panel_drag_select_script<D: ConformanceDriver<App = PanelApp>>(d: &mut D) -> bool {
    let before = d.screen_has("Selecting row");
    d.drag_text("brown", "wizards");
    let after = d.screen_has("Selecting row");
    !before && after
}

#[test]
fn panel_drag_text_selects_across_lines_parity_tui_and_gtk_agree() {
    // `new_fixture` is `ConformanceDriver`'s LogicalViewport-aligned
    // constructor (quadraui#488 TEST-07) — the same 80×24 logical size on
    // both backends, each interpreting it in its own native units.
    let mut tui = TuiDriver::new_fixture(PanelApp::new(), LogicalViewport::new(80, 24));
    let mut gtk = GtkDriver::new_fixture(PanelApp::new(), LogicalViewport::new(80, 24));

    let tui_selected = run_panel_drag_select_script(&mut tui);
    let gtk_selected = run_panel_drag_select_script(&mut gtk);

    assert!(
        tui_selected,
        "TUI: dragging from \"brown\" to \"wizards\" should start a text \
         selection spanning both content lines:\n{}",
        tui.screen()
    );
    assert!(
        gtk_selected,
        "GTK: dragging from \"brown\" to \"wizards\" should start a text \
         selection spanning both content lines"
    );
    assert_eq!(
        tui_selected, gtk_selected,
        "both backends should agree that drag_text produced a selection"
    );
}

// ─── DataTableApp resize-direction parity (#516 defect 3) ──────────────────
//
// The acceptance bar for defect 3 is specifically that the fix landed in
// the *shared* `primitives::data_table::resolve_columns`, not in one
// rasteriser — so the same divider-drag script must produce the same
// *direction* of change on both backends. `ConformanceDriver` above is
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
    /// Drag the divider to the right of column `col` by `dx`, in this
    /// backend's native coordinate space — generalized `drag_age_divider`
    /// for #521's pair-resize and viewport-fill coverage, which needs to
    /// drag dividers other than just the one before the last column.
    fn drag_divider_at(&mut self, col: usize, dx: f32);
    /// All resolved column widths, in this backend's native units.
    fn column_widths(&self) -> Vec<f32>;
    /// The current table layout — resolved column bounds, viewport and
    /// content width, scrollbar reservation.
    fn table_layout(&self) -> DataTableLayout;
}

impl DataTableResizeDriver for TuiDriver<DataTableApp> {
    fn age_width(&self) -> f32 {
        self.app().resolved_column_widths(self.backend())[2]
    }

    fn drag_age_divider(&mut self, dx: f32) {
        self.drag_divider_at(2, dx);
    }

    fn drag_divider_at(&mut self, col: usize, dx: f32) {
        let layout = self.app().table_layout(self.backend());
        let target = layout.columns[col];
        let divider_x = target.x + target.width;
        let y = 0.5;
        self.drag(divider_x, y, divider_x + dx, y);
    }

    fn column_widths(&self) -> Vec<f32> {
        self.app().resolved_column_widths(self.backend())
    }

    fn table_layout(&self) -> DataTableLayout {
        self.app().table_layout(self.backend())
    }
}

impl DataTableResizeDriver for GtkDriver<DataTableApp> {
    fn age_width(&self) -> f32 {
        self.app().resolved_column_widths(self.backend())[2]
    }

    fn drag_age_divider(&mut self, dx: f32) {
        self.drag_divider_at(2, dx);
    }

    fn drag_divider_at(&mut self, col: usize, dx: f32) {
        let layout = self.app().table_layout(self.backend());
        let target = layout.columns[col];
        let divider_x = target.x + target.width;
        let y = layout.header_height / 2.0;
        self.drag(divider_x, y, divider_x + dx, y);
    }

    fn column_widths(&self) -> Vec<f32> {
        self.app().resolved_column_widths(self.backend())
    }

    fn table_layout(&self) -> DataTableLayout {
        self.app().table_layout(self.backend())
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

// ─── #521 defect 1: divider drag isolation parity ───────────────────────────

/// Drags the Age|Restarts divider (col 2) by `dx` and returns the resolved
/// column widths before and after — used to check that columns 0 (Name)
/// and 1 (Status), which the divider doesn't border, are untouched.
fn run_pair_isolation_script<D: DataTableResizeDriver + DataTableDriverCtor>(
    dx: f32,
) -> (Vec<f32>, Vec<f32>) {
    let mut d = D::new_default();
    let before = d.column_widths();
    d.drag_divider_at(2, dx);
    let after = d.column_widths();
    (before, after)
}

#[test]
fn data_table_divider_drag_leaves_unrelated_columns_untouched_parity_tui_and_gtk_agree() {
    // Grabbing and shrinking the Age|Restarts divider must leave Name and
    // Status exactly as they were — the literal #521 defect 1 symptom was
    // that shrinking one divider moved a column it didn't border. Shared
    // `resolve_columns` logic, so this must hold on both backends.
    let (tui_before, tui_after) = run_pair_isolation_script::<TuiDriver<DataTableApp>>(-20.0);
    let (gtk_before, gtk_after) = run_pair_isolation_script::<GtkDriver<DataTableApp>>(-40.0);

    for (backend, before, after) in [
        ("TUI", &tui_before, &tui_after),
        ("GTK", &gtk_before, &gtk_after),
    ] {
        assert!(
            (before[0] - after[0]).abs() < 0.05,
            "{backend}: column 0 (Name) must be byte-identical before/after a divider drag \
             it doesn't border: before={before:?}, after={after:?}"
        );
        assert!(
            (before[1] - after[1]).abs() < 0.05,
            "{backend}: column 1 (Status) must be byte-identical before/after a divider drag \
             it doesn't border: before={before:?}, after={after:?}"
        );
        assert!(
            after[2] < before[2],
            "{backend}: dragging the divider left should narrow the column to its left: \
             before={before:?}, after={after:?}"
        );
    }
}

// ─── #521 defect 2: viewport-fill parity after resize ───────────────────────

/// Drags several dividers in sequence (Name|Status, then Status|Age, then
/// Age|Restarts) and returns whether the table still fills its viewport
/// (no horizontal scrollbar triggered, and the resolved content flush
/// with the visible column area) after all three.
fn run_multi_drag_fill_script<D: DataTableResizeDriver + DataTableDriverCtor>(dx: f32) -> bool {
    let mut d = D::new_default();
    d.drag_divider_at(0, dx);
    d.drag_divider_at(1, -dx);
    d.drag_divider_at(2, dx);
    let layout = d.table_layout();
    let visible_col_area = layout.viewport_width - layout.scrollbar_width;
    layout.h_scrollbar_height == 0.0 && (layout.content_width - visible_col_area).abs() < 0.5
}

#[test]
fn data_table_rightmost_column_stays_flush_after_multiple_drags_parity_tui_and_gtk_agree() {
    // After a sequence of divider drags, the table must still fill its
    // viewport exactly — #521 defect 2's literal symptom was the
    // rightmost column's right edge detaching from the frame's right
    // edge once every `Flex` column had been overridden by a drag.
    let tui_flush = run_multi_drag_fill_script::<TuiDriver<DataTableApp>>(10.0);
    let gtk_flush = run_multi_drag_fill_script::<GtkDriver<DataTableApp>>(20.0);

    assert!(
        tui_flush,
        "TUI: table must still fill its viewport after multiple divider drags"
    );
    assert!(
        gtk_flush,
        "GTK: table must still fill its viewport after multiple divider drags"
    );
}

// ─── Issue #552: activity-bar hit regions vs. a revealed title bar ───────
//
// `Backend::draw_activity_bar` returns `ActivityBarRowHit`s whose
// `y_start`/`y_end` are **bar-relative** — measured from the `rect` the bar
// was drawn into. `AppShell` adds `activity_bar_bounds.y` itself in both its
// click reader (`cached_activity_hit`) and its hover reader
// (`update_hover`). The TUI rasteriser used to fold that origin in a second
// time, so every TUI hit region sat `activity_bar_bounds.y` too low.
//
// That offset is **zero while the title bar is hidden**, which is why the
// bug was invisible to every existing test: they all construct in a fixed
// state, and `AppShell`'s own layout assertions only ever check
// `activity_bar_bounds.y` against the title bar, never a hit region against
// a click. Only the *transition* — hidden bar, then `set_title_bar_visible(
// true)` — makes the origin nonzero and the double-count observable. Same
// trap #547 documented ("static construction tests cannot catch it … only a
// toggle exercises the defect"); this is its coordinate-space half.
//
// Symptom downstream (JDonaghy/vimcode#634, three consecutive failed smoke
// rounds): reveal the menu bar, click the Search icon, and the *adjacent*
// panel opens — icons painted correctly, click-to-action mapping off by one
// row. Hover drifted identically, because both readers share the bug.
//
// GTK was always correct here, so the GTK half of each test below is the
// parity guard `GtkDriver` (#301/#446) exists for: it pins the behaviour
// TUI now has to match, and would catch a "fix" that merely moved the error
// to the other backend.

/// Center of activity-bar row `idx` in the driver's native units, derived
/// from the bounds the *shell* reported this frame — never a literal.
fn activity_row_center<D: ActivityBarProbeDriver>(
    d: &D,
    probe: &ActivityProbe,
    idx: usize,
) -> (f32, f32) {
    let ab = probe
        .bounds()
        .expect("render_content should have published activity_bar_bounds");
    let row_h = d.activity_row_height();
    (ab.x + ab.width / 2.0, ab.y + row_h * (idx as f32 + 0.5))
}

/// Reveal the title bar (`t`), then click the center of activity-bar row
/// `idx`. Returns whether the *expected* panel's sidebar header is showing
/// afterwards, plus the `last_event` line the shell reported.
///
/// Row 1 is Search and row 2 is Source Control. Both are deliberately
/// **not** the initially-active panel (Explorer, row 0): clicking the
/// already-active icon is a sidebar *toggle*, not a panel switch, so it
/// would mask an off-by-one instead of exposing it.
fn run_activity_click_after_title_bar_reveal<D: ActivityBarProbeDriver>(
    d: &mut D,
    probe: &ActivityProbe,
    idx: usize,
    expected_header: &str,
) -> bool {
    // Title bar starts hidden: `AppShellDemo::config()` never calls
    // `with_title_bar`, so `activity_bar_bounds.y == 0` and the old
    // double-count added nothing.
    let ab_before = probe.bounds().expect("bounds published on first frame");

    d.type_char('t');

    let ab_after = probe.bounds().expect("bounds republished after reveal");
    assert!(
        ab_after.y > ab_before.y,
        "revealing the title bar must push the activity bar down — otherwise \
         this test proves nothing (before={}, after={})",
        ab_before.y,
        ab_after.y
    );

    let (x, y) = activity_row_center(d, probe, idx);
    d.click_at(x, y);
    d.screen_has(expected_header)
}

#[test]
fn activity_click_hits_the_clicked_row_after_title_bar_reveal_parity() {
    // Row 1 = Search. Pre-fix the TUI resolved this click into a different
    // row's region (or none at all), so "SEARCH" never appeared.
    let tui_app = AppShellDemo::new();
    let tui_probe = tui_app.probe();
    let mut tui = tui_driver_with_shell(tui_app, AppShellDemo::config(), 100, 30);

    let gtk_app = AppShellDemo::new();
    let gtk_probe = gtk_app.probe();
    let mut gtk = gtk_driver_with_shell(gtk_app, AppShellDemo::config(), 800, 480);

    let tui_hit = run_activity_click_after_title_bar_reveal(&mut tui, &tui_probe, 1, "SEARCH");
    let gtk_hit = run_activity_click_after_title_bar_reveal(&mut gtk, &gtk_probe, 1, "SEARCH");

    assert!(
        tui_hit,
        "TUI: clicking activity row 1 with the title bar revealed must open \
         the Search panel, not a neighbour (issue #552):\n{}",
        tui.screen()
    );
    assert!(
        gtk_hit,
        "GTK: clicking activity row 1 with the title bar revealed must open \
         the Search panel (this backend was always correct — it is the \
         parity guard that the fix did not just move the error here)"
    );
    assert_eq!(
        tui_hit, gtk_hit,
        "both backends must agree on which panel a row-1 click opens"
    );
}

#[test]
fn activity_click_row_two_hits_row_two_after_title_bar_reveal_parity() {
    // A second, further-down row: an off-by-one shows up at every index, so
    // pinning two of them rules out a fix that merely re-centred row 1.
    let tui_app = AppShellDemo::new();
    let tui_probe = tui_app.probe();
    let mut tui = tui_driver_with_shell(tui_app, AppShellDemo::config(), 100, 30);

    let gtk_app = AppShellDemo::new();
    let gtk_probe = gtk_app.probe();
    let mut gtk = gtk_driver_with_shell(gtk_app, AppShellDemo::config(), 800, 480);

    let tui_hit =
        run_activity_click_after_title_bar_reveal(&mut tui, &tui_probe, 2, "SOURCE CONTROL");
    let gtk_hit =
        run_activity_click_after_title_bar_reveal(&mut gtk, &gtk_probe, 2, "SOURCE CONTROL");

    assert!(
        tui_hit,
        "TUI: clicking activity row 2 must open Source Control:\n{}",
        tui.screen()
    );
    assert!(
        gtk_hit,
        "GTK: clicking activity row 2 must open Source Control"
    );
}

/// Hover the center of visual activity row `idx` and report what the shell
/// decided is hovered.
///
/// Note the returned index is a position in the shell's hit-region list,
/// which is **bottom-pinned-first** (see `ActivityBarLayout::visible_items`)
/// — so it is deliberately not asserted against `idx` directly. What
/// matters for #552 is that the answer does not *change* when the bar
/// moves.
fn hover_row<D: ActivityBarProbeDriver>(
    d: &mut D,
    probe: &ActivityProbe,
    idx: usize,
) -> Option<usize> {
    let (x, y) = activity_row_center(d, probe, idx);
    d.hover_at(x, y);
    probe.hovered_idx()
}

/// For visual row `idx`: hover it with the title bar hidden (the state in
/// which the double-count added zero, i.e. the known-good baseline), then
/// reveal the title bar and hover the same visual row again. Returns both
/// answers.
///
/// Hover shared the click path's double-counted comparison, which is why
/// the reported symptom was "hovering icon N highlights icon N+1" — the
/// highlight followed the wrong icon exactly as activation did. Comparing
/// hidden vs revealed states the invariant without hard-coding the shell's
/// internal ordering: revealing chrome above the bar must not change which
/// icon a given on-screen row belongs to.
fn hover_before_and_after_title_bar_reveal<D: ActivityBarProbeDriver>(
    d: &mut D,
    probe: &ActivityProbe,
    idx: usize,
) -> (Option<usize>, Option<usize>) {
    let hidden = hover_row(d, probe, idx);
    d.type_char('t');
    let revealed = hover_row(d, probe, idx);
    (hidden, revealed)
}

#[test]
fn activity_hover_highlights_the_hovered_row_after_title_bar_reveal_parity() {
    for idx in [0usize, 1, 2] {
        let tui_app = AppShellDemo::new();
        let tui_probe = tui_app.probe();
        let mut tui = tui_driver_with_shell(tui_app, AppShellDemo::config(), 100, 30);

        let gtk_app = AppShellDemo::new();
        let gtk_probe = gtk_app.probe();
        let mut gtk = gtk_driver_with_shell(gtk_app, AppShellDemo::config(), 800, 480);

        let (tui_hidden, tui_revealed) =
            hover_before_and_after_title_bar_reveal(&mut tui, &tui_probe, idx);
        let (gtk_hidden, gtk_revealed) =
            hover_before_and_after_title_bar_reveal(&mut gtk, &gtk_probe, idx);

        assert!(
            tui_hidden.is_some(),
            "sanity: hovering row {idx} with the title bar hidden should \
             highlight *something* on TUI"
        );
        assert_eq!(
            tui_revealed, tui_hidden,
            "TUI: revealing the title bar must not change which activity \
             icon visual row {idx} maps to — hover drifted by \
             activity_bar_bounds.y (issue #552)"
        );
        assert_eq!(
            gtk_revealed, gtk_hidden,
            "GTK: hover must be stable across the title-bar reveal (this \
             backend was already correct — parity guard)"
        );
        assert_eq!(
            tui_revealed, gtk_revealed,
            "both backends must agree on the hovered activity row for \
             visual row {idx}"
        );
    }
}

/// Guard for the assertion above: `hover_row` must actually discriminate
/// between rows, otherwise "stable across the reveal" would be satisfied by
/// a hover that always returned the same thing (or always `None`).
#[test]
fn activity_hover_distinguishes_adjacent_rows_after_title_bar_reveal() {
    let app = AppShellDemo::new();
    let probe = app.probe();
    let mut d = tui_driver_with_shell(app, AppShellDemo::config(), 100, 30);

    d.type_char('t');
    let row0 = hover_row(&mut d, &probe, 0);
    let row1 = hover_row(&mut d, &probe, 1);
    let row2 = hover_row(&mut d, &probe, 2);

    assert!(
        row0.is_some() && row1.is_some() && row2.is_some(),
        "every activity row should highlight something once hovered: \
         {row0:?} {row1:?} {row2:?}"
    );
    assert!(
        row0 != row1 && row1 != row2 && row0 != row2,
        "three distinct rows must map to three distinct icons — got \
         {row0:?} {row1:?} {row2:?}"
    );
}

// ─── quadraui#490: FrameInventory relational vocabulary ────────────────────
//
// `left_of`/`above`/`inside` are the whole point of `FrameInventory` — a
// shared test body asks how two painted things relate, in whichever units
// the backend that produced them uses, and never writes a coordinate
// itself. Proves the three hold identically for `AppShellDemo` (the
// `shell_app` fixture) on both `TuiDriver` and `GtkDriver`, per #490's
// acceptance bar.

/// Switch to the Source Control panel (`p` is `AppShellDemo`'s
/// jump-to-Source-Control binding — see its `handle` impl) and return the
/// [`quadraui::testing::FrameInventory`] for the resulting frame.
fn source_control_inventory<D: ConformanceDriver>(d: &mut D) -> quadraui::testing::FrameInventory {
    d.type_char('p');
    d.inventory()
}

#[test]
fn frame_inventory_relations_agree_tui_and_gtk() {
    let mut tui = tui_driver_with_shell(AppShellDemo::new(), AppShellDemo::config(), 100, 30);
    let mut gtk = gtk_driver_with_shell(AppShellDemo::new(), AppShellDemo::config(), 800, 480);

    let tui_inv = source_control_inventory(&mut tui);
    let gtk_inv = source_control_inventory(&mut gtk);

    let sidebar_content_zone = WidgetId::new("app-shell:sidebar-content");
    let main_content_zone = WidgetId::new("app-shell:main-content");

    for (name, inv) in [("TUI", &tui_inv), ("GTK", &gtk_inv)] {
        assert!(
            inv.screen_has("CONTROL"),
            "{name}: sidebar header should read SOURCE CONTROL after 'p'"
        );
        assert!(
            inv.left_of("G", "CONTROL"),
            "{name}: the activity bar's Source Control icon ('G') must sit \
             left of the sidebar header it activates"
        );
        assert!(
            inv.above("CONTROL", "content"),
            "{name}: the sidebar header must sit above the sidebar content \
             row (' (sidebar content here) ')"
        );
        assert!(
            inv.inside("content", &sidebar_content_zone),
            "{name}: the sidebar content text must fall inside the \
             registered app-shell:sidebar-content zone"
        );
        assert!(
            !inv.inside("content", &main_content_zone),
            "{name}: the sidebar content text must NOT fall inside the \
             main-content zone — the two zones must not overlap"
        );
    }
}

// ─── Issue #541: Tooltip border vocabulary agrees across backends ──────────
//
// #542 gave the tooltip a *structural-parity* tier (sealed in
// `tests/acceptance/ms-11/structural_parity.rs`) pinning the *default*
// border (`TooltipBorder::Full`) so both backends enclose their text on
// every side. #541 adds the vocabulary itself — `Sides` / `Full` / `None`
// plus an optional title — and this section is its parity coverage: for
// every setting a consumer can choose, not just the default, TUI and GTK
// must agree on the resulting chrome shape.
//
// Known, documented exception (not covered below): TUI's `Full` falls
// back to `Sides`-style chrome when the measured box is too short to fit
// both border rows (`height < 3`, see `tui::tooltip`'s module doc and its
// `full_border_drops_title_when_too_short_to_close` /
// `sides_border_never_closes_even_when_height_allows_it` unit tests,
// which pin that fallback in isolation). GTK and macOS always stroke a
// full rectangle regardless of height — they have no equivalent
// short-height degrade. Every `Full` fixture below therefore measures
// `rows: 3.0` (room enough to close on every backend) specifically so
// this divergence never fires here; it is TUI's own rendering detail
// rather than something a consumer selects, and asserting parity at
// `height < 3` would be asserting two backends *disagree* by design, not
// a parity bug. Left as a documented exception per the #541 review
// rather than force-added coverage here.
//
// Scope note — the tooltip-border harness below is still *two*-backend
// only, not three. `ConformanceDriver` now has a `MacDriver` impl
// (quadraui#493 — see `pipeline_parity_macos_agrees_with_tui_and_gtk_on_
// logical_state` above for where it *does* participate), but
// `TooltipBorderFixture` below asserts on a registered zone
// (`border_tooltip_zone`), and `macos::backend::MacBackend::
// draw_tooltip_with_chrome` doesn't call `Backend::register_zone` the way
// `TuiBackend`'s and `GtkBackend`'s do — so macOS still can't participate
// in *this specific* harness (a real gap, not something #541 introduced
// or could close on its own). macOS's side of the #541 vocabulary is
// covered instead by direct pixel-probe unit tests in
// `src/macos/tooltip.rs`
// (`sides_border_only_strokes_left_and_right`, `none_border_strokes_nothing`,
// `full_border_title_punches_a_gap_...`, `title_is_ignored_when_border_is_
// sides_or_none`), which mirror the GTK rasteriser's assertions one for one.
// Anyone treating this file as the single source of truth across all three
// backends should read those too.

const BORDER_TOOLTIP_ID: &str = "cbp:541:tooltip";
const BORDER_TOOLTIP_TEXT: &str = "BorderProbe";
const BORDER_TOOLTIP_TITLE: &str = "FrameTitle";

/// Draws a single `Tooltip` with the given `border`/`title`. `rows` picks
/// the measured box height in `line_height` multiples — callers pass
/// exactly what the variant under test needs to close (1 row for `Sides`/
/// `None`, which never reserve a border row; 3 for `Full`, matching
/// `structural_parity.rs`'s fixture) so any gap between the registered
/// zone and the painted text is attributable to border chrome, not
/// leftover slack from a box taller than its content.
struct TooltipBorderFixture {
    border: TooltipBorder,
    title: Option<String>,
    rows: f32,
}

impl AppLogic for TooltipBorderFixture {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let vp = backend.viewport();
        let cw = backend.char_width();
        let lh = backend.line_height();
        let viewport = Rect::new(0.0, 0.0, vp.width, vp.height);
        let anchor = Rect::new(0.0, 0.0, vp.width, lh);

        // `border`/`title` (#541) travel in a `TooltipChrome` sidecar
        // passed alongside the tooltip and its layout — not as fields on
        // either. See `primitives::tooltip`'s module doc for why: it keeps
        // every exhaustive `Tooltip { .. }` *and* `TooltipLayout { .. }`
        // literal (in-tree and downstream) compiling untouched.
        let tooltip = Tooltip::new(
            WidgetId::new(BORDER_TOOLTIP_ID),
            BORDER_TOOLTIP_TEXT.to_string(),
        )
        .with_placement(TooltipPlacement::Bottom);
        // Horizontal slack (+4 columns, same margin `structural_parity.rs`
        // uses) so the tier-A control below can't fail for an unrelated
        // reason (the last glyph clipped for lack of padding).
        let measure = TooltipMeasure::new(
            cw * (BORDER_TOOLTIP_TEXT.chars().count() as f32 + 4.0),
            lh * self.rows,
        );
        let layout = tooltip.layout(anchor, viewport, measure, lh);
        let mut chrome = TooltipChrome::new(self.border);
        if let Some(title) = self.title.clone() {
            chrome = chrome.with_title(title);
        }
        backend.draw_tooltip_with_chrome(&tooltip, &layout, &chrome);
    }

    fn handle(&mut self, _event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        Reaction::Continue
    }
}

fn border_frame<D, A>(app: A) -> FrameInventory
where
    A: AppLogic,
    D: ConformanceDriver<App = A>,
{
    D::new_fixture(app, LogicalViewport::new(60, 12)).inventory()
}

/// Both backends' inventories for one `(border, title, rows)` combination,
/// in a fixed order so every assertion below can name them the same way.
/// `make` is called once per backend so each gets its own `AppLogic`
/// instance.
fn border_frames(make: impl Fn() -> TooltipBorderFixture) -> [(&'static str, FrameInventory); 2] {
    [
        ("TuiDriver", border_frame::<TuiDriver<_>, _>(make())),
        ("GtkDriver", border_frame::<GtkDriver<_>, _>(make())),
    ]
}

fn border_tooltip_zone(name: &str, inv: &FrameInventory) -> Rect {
    inv.zones()
        .iter()
        .find(|z| z.id.as_str() == BORDER_TOOLTIP_ID)
        .unwrap_or_else(|| {
            panic!(
                "{name}: no {BORDER_TOOLTIP_ID} zone registered — surfaces: {:?}",
                inv.zones()
                    .iter()
                    .map(|z| z.id.as_str())
                    .collect::<Vec<_>>()
            )
        })
        .bounds
}

fn border_tooltip_text(name: &str, inv: &FrameInventory) -> Rect {
    inv.text_runs()
        .iter()
        .find(|r| r.text.contains(BORDER_TOOLTIP_TEXT))
        .unwrap_or_else(|| {
            panic!(
                "{name}: no {BORDER_TOOLTIP_TEXT:?} text run painted — \
             painted runs: {:?}",
                inv.text_runs()
                    .iter()
                    .map(|r| r.text.as_str())
                    .collect::<Vec<_>>()
            )
        })
        .bounds
}

/// `(has_top, has_bottom, has_left, has_right)` chrome — purely a
/// comparison between the registered zone and the painted text's own
/// bounds, so it's unit-free by construction (each backend's own cells or
/// pixels) and comparable across backends without a conversion factor,
/// the same technique
/// `structural_parity_tooltip_surface_encloses_its_text_on_every_side`
/// uses for the default (`Full`) setting.
fn chrome_sides(name: &str, inv: &FrameInventory) -> (bool, bool, bool, bool) {
    let zone = border_tooltip_zone(name, inv);
    let text = border_tooltip_text(name, inv);
    (
        zone.y < text.y,
        zone.y + zone.height > text.y + text.height,
        zone.x < text.x,
        zone.x + zone.width > text.x + text.width,
    )
}

/// Total vertical gap (top + bottom) between the registered zone and the
/// painted text, as a fraction of the text's own height — unit-free by
/// construction, like `chrome_sides`, but a ratio rather than a boolean:
/// GTK reserves a fixed ~2px top pad for content *regardless* of border
/// (`text_top = by + 2.0` in `gtk::tooltip::draw_tooltip`), so a `Sides`/
/// `None` tooltip's gap is never *exactly* zero there the way it is on
/// TUI (whole-cell rows, no sub-cell padding) — an inherent unit
/// difference, not a chrome difference. Comparing this ratio against the
/// same backend's `Full` ratio (see the two tests below) routes around
/// that: what should agree across backends isn't the raw gap, it's
/// *how much smaller* a borderless variant's gap is than a boxed one's.
fn vertical_gap_ratio(name: &str, inv: &FrameInventory) -> f32 {
    let zone = border_tooltip_zone(name, inv);
    let text = border_tooltip_text(name, inv);
    let top_gap = (text.y - zone.y).max(0.0);
    let bottom_gap = ((zone.y + zone.height) - (text.y + text.height)).max(0.0);
    (top_gap + bottom_gap) / text.height.max(f32::EPSILON)
}

/// Both backends' `vertical_gap_ratio` for the default (`Full`) setting —
/// the reference "this is what real border chrome costs" measurement the
/// borderless-variant tests below compare against.
fn full_border_vertical_gap_ratios() -> [(&'static str, f32); 2] {
    let frames = border_frames(|| TooltipBorderFixture {
        border: TooltipBorder::default(),
        title: None,
        rows: 3.0,
    });
    [
        (frames[0].0, vertical_gap_ratio(frames[0].0, &frames[0].1)),
        (frames[1].0, vertical_gap_ratio(frames[1].0, &frames[1].1)),
    ]
}

#[test]
fn sides_border_has_horizontal_chrome_and_far_less_vertical_gap_than_full_on_both_backends() {
    let full_ratios = full_border_vertical_gap_ratios();
    let frames = border_frames(|| TooltipBorderFixture {
        border: TooltipBorder::Sides,
        title: None,
        rows: 1.4,
    });

    for (i, (name, inv)) in frames.iter().enumerate() {
        let (_top, _bottom, left, right) = chrome_sides(name, inv);
        assert!(
            left && right,
            "{name}: TooltipBorder::Sides must still enclose its text horizontally — the \
             side bars are the whole point of this variant (left={left}, right={right})"
        );

        let sides_ratio = vertical_gap_ratio(name, inv);
        let full_ratio = full_ratios[i].1;
        assert!(
            sides_ratio < full_ratio * 0.5,
            "{name}: Sides' vertical gap ({sides_ratio}× its text height) should be far \
             smaller than Full's ({full_ratio}× — real top/bottom border rows) — Sides never \
             draws a top/bottom rule, regardless of box height"
        );
    }
}

#[test]
fn none_border_has_far_less_vertical_gap_than_full_on_both_backends() {
    let full_ratios = full_border_vertical_gap_ratios();
    let frames = border_frames(|| TooltipBorderFixture {
        border: TooltipBorder::None,
        title: None,
        rows: 1.4,
    });

    for (i, (name, inv)) in frames.iter().enumerate() {
        let none_ratio = vertical_gap_ratio(name, inv);
        let full_ratio = full_ratios[i].1;
        assert!(
            none_ratio < full_ratio * 0.5,
            "{name}: None's vertical gap ({none_ratio}× its text height) should be far \
             smaller than Full's ({full_ratio}× — real top/bottom border rows) — None draws \
             no chrome at all"
        );
    }
}

#[test]
fn full_border_title_reaches_every_backend_and_sits_above_the_content() {
    let frames = border_frames(|| TooltipBorderFixture {
        border: TooltipBorder::default(),
        title: Some(BORDER_TOOLTIP_TITLE.to_string()),
        rows: 3.0,
    });

    for (name, inv) in &frames {
        assert!(
            inv.screen_has(BORDER_TOOLTIP_TITLE),
            "{name}: a Full-bordered tooltip's title must reach the screen — \
             painted runs: {:?}",
            inv.text_runs()
                .iter()
                .map(|r| r.text.as_str())
                .collect::<Vec<_>>()
        );
        assert!(
            inv.screen_has(BORDER_TOOLTIP_TEXT),
            "{name}: the body text must still reach the screen alongside the title"
        );
        assert!(
            inv.above(BORDER_TOOLTIP_TITLE, BORDER_TOOLTIP_TEXT),
            "{name}: the title (embedded in the top border row) must sit above the body \
             text (the first content row) — title and content must not collide"
        );
    }
}

#[test]
fn title_does_not_reach_the_screen_when_border_is_not_full() {
    for border in [TooltipBorder::Sides, TooltipBorder::None] {
        let frames = border_frames(|| TooltipBorderFixture {
            border,
            title: Some(BORDER_TOOLTIP_TITLE.to_string()),
            rows: 1.4,
        });

        for (name, inv) in &frames {
            assert!(
                inv.absent(BORDER_TOOLTIP_TITLE),
                "{name}: {border:?} has no top rule to embed a title in — it must not \
                 leak into the content area either. painted runs: {:?}",
                inv.text_runs()
                    .iter()
                    .map(|r| r.text.as_str())
                    .collect::<Vec<_>>()
            );
        }
    }
}
