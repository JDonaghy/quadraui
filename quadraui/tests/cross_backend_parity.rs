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
use quadraui::{AppLogic, DataTableLayout, NamedKey};

#[path = "../examples/common/pipeline_app.rs"]
mod pipeline_app;
use pipeline_app::PipelineApp;

#[path = "../examples/common/appshell_demo.rs"]
mod appshell_demo;
use appshell_demo::{ActivityProbe, AppShellDemo};

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

    /// Click at an explicit point in this backend's native units.
    ///
    /// Shared bodies must derive the point from geometry the *shell*
    /// reported (see `ActivityProbe`), never from a literal — a literal
    /// would be cells on one backend and pixels on the other.
    fn click_at(&mut self, x: f32, y: f32);
    /// Move the pointer, buttons up, to update hover state.
    fn hover_at(&mut self, x: f32, y: f32);
    /// Height of one activity-bar row in this backend's native units.
    /// This is the one genuinely per-backend number the #552 bodies need:
    /// the TUI bar is one text row per icon, GTK a fixed 48px button.
    fn activity_row_height(&self) -> f32;
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
fn activity_row_center<D: ExampleDriver>(d: &D, probe: &ActivityProbe, idx: usize) -> (f32, f32) {
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
fn run_activity_click_after_title_bar_reveal<D: ExampleDriver>(
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
fn hover_row<D: ExampleDriver>(d: &mut D, probe: &ActivityProbe, idx: usize) -> Option<usize> {
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
fn hover_before_and_after_title_bar_reveal<D: ExampleDriver>(
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
