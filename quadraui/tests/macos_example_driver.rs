//! First core smoke set for the macOS example-driver harness
//! — the macOS twin of `tests/gtk_example_driver.rs` / `tests/tui_example_driver.rs`.
//! Each test instantiates the *same* backend-agnostic `AppLogic` impl the
//! corresponding `macos_*` example runs, scripts real [`quadraui::UiEvent`]s
//! through it via [`MacDriver`], and asserts on the painted text — no
//! window, no `NSApplication`.
//!
//! Deliberately thin per the pattern the GTK/TUI suites already set — coverage grows
//! incrementally. `PipelineApp` and `DataTableApp` are the two `examples/common`
//! shapes: the same fixtures already covered on TUI/GTK in
//! `tests/tui_example_driver.rs` / `tests/gtk_example_driver.rs` and in
//! `tests/cross_backend_parity.rs`'s
//! `pipeline_parity_macos_agrees_with_tui_and_gtk_on_logical_state`.
#![cfg(all(feature = "macos", target_os = "macos"))]

use quadraui::macos::testing::{driver_with_shell, MacDriver};
use quadraui::testing::ConformanceDriver;
use quadraui::{Backend, NamedKey, Reaction};

#[path = "../examples/common/pipeline_app.rs"]
mod pipeline_app;
use pipeline_app::PipelineApp;

#[path = "../examples/common/appshell_demo.rs"]
mod appshell_demo;
use appshell_demo::AppShellDemo;

#[path = "../examples/common/data_table_app.rs"]
mod data_table_app;
use data_table_app::DataTableApp;

#[path = "../examples/common/minimap_app.rs"]
mod minimap_app;
use minimap_app::MinimapApp;

#[path = "../examples/common/image_app.rs"]
mod image_app;
use image_app::ImageApp;

#[path = "../examples/common/tab_icons_demo.rs"]
#[allow(dead_code)] // `LABELS`/`TOGGLE_KEY` are read by some tests, not all
mod tab_icons_demo;
use tab_icons_demo::TabIconsDemo;

#[path = "../examples/common/toolbar_app.rs"]
mod toolbar_app;
use toolbar_app::ToolbarApp;

#[path = "../examples/common/form_groups.rs"]
mod form_groups;
use form_groups::FormGroupsApp;

#[path = "../examples/common/canvas_app.rs"]
mod canvas_app;
use canvas_app::CanvasApp;

#[path = "../examples/common/canvas_shapes_app.rs"]
mod canvas_shapes_app;
use canvas_shapes_app::CanvasShapesApp;

// Point canvas — big enough for five pipeline stage boxes + arrow
// connectors + the bottom status bar at macOS's native (point, not cell)
// scale. Same size `tests/cross_backend_parity.rs` uses for its `MacDriver`
// row.
const W: u32 = 800;
const H: u32 = 480;

/// Antialiasing at a shape's edge (or a shared boundary between two
/// independently-rasterized fills) can land a blend fraction a shade of a
/// channel apart from the mathematically-exact expectation. `MacDriver::pixel`
/// returns RGBA; the alpha channel is irrelevant to every colour assertion
/// below, so this compares only the RGB components against a tolerance.
fn colors_close(a: (u8, u8, u8, u8), b: (u8, u8, u8), tol: i32) -> bool {
    (a.0 as i32 - b.0 as i32).abs() <= tol
        && (a.1 as i32 - b.1 as i32).abs() <= tol
        && (a.2 as i32 - b.2 as i32).abs() <= tol
}

// ─── PipelineApp: initial paint, keyboard, mouse-routed click ──────────────

#[test]
fn pipeline_initial_screen_paints_stages_and_hint() {
    let driver = MacDriver::new(PipelineApp::new(), W, H);
    assert!(
        driver.screen_contains("Checkout"),
        "stage label should be painted: {:?}",
        driver.painted_texts()
    );
    assert!(
        driver.screen_contains("Deploy"),
        "stage label should be painted: {:?}",
        driver.painted_texts()
    );
    assert!(
        driver.screen_contains("Enter"),
        "status bar hint should be painted: {:?}",
        driver.painted_texts()
    );
}

#[test]
fn pipeline_pressing_q_exits() {
    let mut driver = MacDriver::new(PipelineApp::new(), W, H);
    assert!(!driver.exited());
    driver.type_char('q');
    assert!(driver.exited(), "'q' should make the app exit");
}

#[test]
fn pipeline_pressing_r_resets_status_message() {
    let mut driver = MacDriver::new(PipelineApp::new(), W, H);
    driver.press_named(quadraui::NamedKey::Right);
    driver.type_char('r');
    assert!(
        driver.screen_contains("Reset"),
        "after 'r' the status bar should read Reset: {:?}",
        driver.painted_texts()
    );
}

/// A click round-trips paint -> hit_test -> handle -> state -> re-render,
/// with NO hardcoded coordinates: `find` locates the painted "Go"
/// (Deploy/stage-3) action button from the `text_runs` `MacBackend`
/// records at the `draw_text` choke point.
#[test]
fn pipeline_clicking_a_stage_action_routes_the_click() {
    let mut driver = MacDriver::new(PipelineApp::new(), W, H);
    assert!(
        !driver.screen_contains("stage 3"),
        "status bar should not yet mention stage 3"
    );

    let (x, y) = driver
        .find("Go")
        .expect("Go action button should be painted with locatable bounds");
    let reaction = driver.click(x, y);

    assert_eq!(reaction, Reaction::Redraw, "click should trigger a redraw");
    // PipelineApp writes "Action on stage 3: Deploy" (action) or
    // "Selected stage 3: Deploy" (body) -- both name stage 3.
    assert!(
        driver.screen_contains("stage 3"),
        "clicking the Deploy action should update the status to mention stage 3: {:?}",
        driver.painted_texts()
    );
}

// ─── AppShellDemo: `driver_with_shell` (ShellApp) coverage ─────────────────
//
// `AppShellDemo` implements `ShellApp` (not `AppLogic` directly) and is
// driven by `macos::shell_runner::run_with_shell` in production.
// `driver_with_shell` builds the identical `ShellAdapter` stack — through
// the same `build_shell_adapter` factory `run_with_shell` calls — then
// scripts events through it headlessly, with no `NSApplication` / window.
// These are the macOS twins of `tests/tui_example_driver.rs` /
// `tests/gtk_example_driver.rs`'s `appshell_demo_*` tests.

// Point canvas sized for the shell chrome (activity bar + sidebar + main
// content), distinct from the pipeline canvas above.
const SHELL_W: u32 = 800;
const SHELL_H: u32 = 480;

/// The initial frame paints real shell chrome — the sidebar header for the
/// default-active panel and the app's own content — asserted via
/// `find_bounds`/`painted_texts` (real painted geometry), not just "it did
/// not panic".
#[test]
fn appshell_demo_renders_shell_chrome_via_driver_with_shell() {
    let config = AppShellDemo::config();
    let driver = driver_with_shell(AppShellDemo::new(), config, SHELL_W, SHELL_H);

    let bounds = driver
        .find_bounds("EXPLORER")
        .expect("sidebar header for the default-active panel should be painted");
    assert!(
        bounds.width > 0.0 && bounds.height > 0.0,
        "sidebar header bounds should be non-empty: {bounds:?}"
    );
    assert!(
        driver
            .painted_texts()
            .iter()
            .any(|t| t.contains("Tab=focus bar")),
        "main content hint should be painted: {:?}",
        driver.painted_texts()
    );
}

/// `Tab` focuses the activity bar, `j` `j` moves the keyboard cursor down
/// two items (explorer → search → git), and `Enter` activates the
/// selection — switching the *real* `AppShell` panel, visible as the
/// sidebar header flipping from "EXPLORER" to "SOURCE CONTROL".
///
/// This is the acceptance criterion that the macOS test path and the live
/// `macos::run` path build the adapter through the same function: the
/// ActivityBar keyboard-focus intercept lives in `macos::run::dispatch_event`
/// (shared by `MacDriver::dispatch`) and reads `ShellAdapter` state built by
/// `build_shell_adapter` — if `driver_with_shell` constructed that adapter
/// differently than `run_with_shell` does, this round trip would desync
/// exactly the way vimcode's Ctrl+B bug did for the sidebar toggle.
#[test]
fn appshell_demo_tab_focus_then_jj_enter_switches_panel() {
    let config = AppShellDemo::config();
    let mut driver = driver_with_shell(AppShellDemo::new(), config, SHELL_W, SHELL_H);

    assert!(
        driver.find_bounds("EXPLORER").is_some(),
        "starts on the default (index 0) Explorer panel"
    );

    let reaction = driver.press_named(NamedKey::Tab);
    assert_eq!(
        reaction,
        Reaction::Redraw,
        "Tab should request activity-bar keyboard focus and redraw"
    );
    assert!(
        driver.screen_contains("Activity bar focused"),
        "focusing the bar should update the status hint: {:?}",
        driver.painted_texts()
    );

    // Cursor starts at index 0 (explorer). Two `j` presses move it to
    // index 2 (git) — 3 top panels, cursor saturates instead of wrapping.
    for _ in 0..2 {
        let reaction = driver.type_char('j');
        assert_eq!(
            reaction,
            Reaction::Redraw,
            "'j' while focused must be intercepted as ActivityBar nav, not fall through"
        );
    }

    let reaction = driver.press_named(NamedKey::Enter);
    assert_eq!(
        reaction,
        Reaction::Redraw,
        "Enter should activate the selected item and redraw"
    );

    let bounds = driver
        .find_bounds("SOURCE CONTROL")
        .expect("sidebar header must switch to the git panel's real title");
    assert!(bounds.width > 0.0 && bounds.height > 0.0);
    assert!(
        driver.find_bounds("EXPLORER").is_none(),
        "the stale Explorer header must not still be painted"
    );
    assert!(
        driver.screen_contains("Panel: panel:git"),
        "on_shell_event_ctx(PanelChanged) must fire for a keyboard-driven switch, \
         mirroring the TUI/GTK paths: {:?}",
        driver.painted_texts()
    );
}

/// `ctx.shell_mut()` reaches the real `AppShell` instance `ShellAdapter`
/// renders, so `Ctrl+B` hides/shows the sidebar `driver_with_shell` actually
/// painted — not a shadow copy. Mirrors `tests/tui_example_driver.rs` /
/// `tests/gtk_example_driver.rs`'s `appshell_demo_ctrl_b_toggles_the_real_rendered_sidebar`.
#[test]
fn appshell_demo_ctrl_b_toggles_the_real_rendered_sidebar() {
    let config = AppShellDemo::config();
    let mut driver = driver_with_shell(AppShellDemo::new(), config, SHELL_W, SHELL_H);

    assert!(
        driver.find_bounds("(sidebar content here)").is_some(),
        "sidebar should be visible on the initial screen"
    );

    let reaction = driver.ctrl_char('b');
    assert_eq!(
        reaction,
        Reaction::Redraw,
        "Ctrl+B toggling the real AppShell must redraw"
    );
    assert!(
        driver.find_bounds("(sidebar content here)").is_none(),
        "Ctrl+B must hide the sidebar that ShellAdapter actually renders, not a shadow copy"
    );
    assert!(
        driver.screen_contains("Sidebar hidden (Ctrl+B via ctx.shell_mut())"),
        "status line should confirm the toggle went through ctx.shell_mut()"
    );

    let reaction = driver.ctrl_char('b');
    assert_eq!(reaction, Reaction::Redraw);
    assert!(
        driver.find_bounds("(sidebar content here)").is_some(),
        "a second Ctrl+B should show the sidebar again"
    );
    assert!(
        driver.screen_contains("Sidebar shown (Ctrl+B via ctx.shell_mut())"),
        "status line should confirm the second toggle"
    );
}

// ─── DataTableApp: initial paint, sort cycling, row selection ──────────────
//
// Pixel canvas big enough for all 20 pod rows + header + the 2-row footer
// band + status bar at once, at the nominal 8px char / 16px line metrics
// `MacDriver::new_fixture`/`MacBackend::new()` start with
// (`min_total_width = 80 * char_width` = 640px), with no scrolling.

const DT_W: u32 = 900;
const DT_H: u32 = 600;

#[test]
fn data_table_initial_screen_paints_headers_and_rows() {
    let driver = MacDriver::new(DataTableApp::new(), DT_W, DT_H);
    assert!(
        driver.screen_contains("Name"),
        "column header should be painted: {:?}",
        driver.painted_texts()
    );
    assert!(
        driver.screen_contains("nginx-7d9b8c66b-x2j4k"),
        "a pod row should be painted: {:?}",
        driver.painted_texts()
    );
    assert!(
        driver.screen_contains("sort: Name asc"),
        "status bar should report the default sort: {:?}",
        driver.painted_texts()
    );
}

/// `s` cycles the sort column (Name → Status → …); the status bar's
/// "sort: <column> asc/desc" segment is the observable proof, same
/// substring `DataTableApp::status_bar` always paints.
#[test]
fn data_table_pressing_s_cycles_sort_column() {
    let mut driver = MacDriver::new(DataTableApp::new(), DT_W, DT_H);
    assert!(driver.screen_contains("sort: Name asc"));
    driver.type_char('s');
    assert!(
        driver.screen_contains("sort: Status asc"),
        "after one 's' the status bar should read Status: {:?}",
        driver.painted_texts()
    );
}

/// `j` moves the row selection down one; the status bar's "row N / 20"
/// segment is the observable proof.
#[test]
fn data_table_pressing_j_moves_selection() {
    let mut driver = MacDriver::new(DataTableApp::new(), DT_W, DT_H);
    assert!(driver.screen_contains("row 1 / 20"));
    driver.type_char('j');
    assert!(
        driver.screen_contains("row 2 / 20"),
        "after one 'j' the status bar should read row 2 / 20: {:?}",
        driver.painted_texts()
    );
}

/// The same divider-drag script the TUI and GTK driver tests run
/// (`tests/tui_example_driver.rs`'s
/// `data_table_divider_before_last_column_resizes_in_drag_direction`,
/// `tests/gtk_example_driver.rs`'s
/// `data_table_divider_before_last_column_widens_on_right_drag`), run
/// against macOS — dragging the divider immediately before the last
/// column (Age | Restarts) must move Age's width in the *drag's*
/// direction, never inverted.
///
/// The fix lives in the shared `primitives::data_table::resolve_columns`,
/// so every rasteriser must agree; `DataTableApp::resolved_column_widths`
/// reads the width back through the very same `DataTable::layout` the macOS
/// backend paints through, so nothing per-backend is hardcoded.
#[test]
fn data_table_divider_before_last_column_resizes_in_drag_direction() {
    // Widen: drag the Age|Restarts divider right. `Restarts` is
    // `Fixed(10.0)` and `DataTableApp` pair-resizes with a 4.0 floor, so
    // the achievable widening is small — the assertion is on direction,
    // not magnitude, exactly as on TUI/GTK.
    let mut driver = MacDriver::new(DataTableApp::new(), DT_W, DT_H);
    let before = driver.app().resolved_column_widths(driver.backend())[2];

    let layout = driver.app().table_layout(driver.backend());
    let age = layout.columns[2];
    let divider_x = age.x + age.width;
    let divider_y = layout.header_height / 2.0;
    driver.drag(divider_x, divider_y, divider_x + 80.0, divider_y);

    let widened = driver.app().resolved_column_widths(driver.backend())[2];
    assert!(
        widened > before,
        "dragging the divider before the last column right should widen it: \
         before={before}, after={widened}"
    );

    // Narrow: a *fresh* driver rather than a second drag on this one —
    // a second `mouse_down` near the same point would fold into a
    // synthetic `DoubleClick` (`DoubleClickDetector`, radius 1.5) in
    // `macos::run::dispatch_event` instead of starting a fresh resize
    // drag. Age's natural width here is ~1/10 of the 900pt viewport, so
    // a 40pt leftward drag lands well clear of the 4.0 floor and the
    // direction assertion is a real one rather than a clamp artifact.
    let mut driver = MacDriver::new(DataTableApp::new(), DT_W, DT_H);
    let natural = driver.app().resolved_column_widths(driver.backend())[2];
    assert!(
        natural > 44.0,
        "test precondition: Age's natural width ({natural}) must leave room \
         to narrow by 40pt without hitting the 4.0 pair-resize floor"
    );

    let layout = driver.app().table_layout(driver.backend());
    let age = layout.columns[2];
    let divider_x = age.x + age.width;
    let divider_y = layout.header_height / 2.0;
    driver.drag(divider_x, divider_y, divider_x - 40.0, divider_y);

    let narrowed = driver.app().resolved_column_widths(driver.backend())[2];
    assert!(
        narrowed < natural,
        "dragging the divider before the last column left should narrow it: \
         before={natural}, after={narrowed}"
    );
}

// ─── MinimapApp / ImageApp: macOS must degrade, not panic ──────────────────
//
// `MacBackend::draw_minimap`/`draw_image` now have real implementations
// on macOS. `MacDriver::new` paints the first frame immediately (same as every
// other constructor in this file), so constructing either driver below
// demonstrates these backends no longer panic on macOS — the exact inverse of
// the four-backend promise these fixtures already prove on TUI/GTK
// (`tests/tui_example_driver.rs`, `tests/gtk_example_driver.rs`) and in
// `tests/cross_backend_parity.rs`.

const MINIMAP_W: u32 = 400;
const MINIMAP_H: u32 = 300;

/// `draw_minimap` is called every frame `MinimapApp::render` runs; this
/// proves that keeps working (and keeps scrolling) on macOS. The minimap
/// track has a real Core Graphics/Core Text rasteriser.
#[test]
fn minimap_app_renders_every_frame_without_panicking_and_scrolls() {
    let mut driver = MacDriver::new(MinimapApp::new(), MINIMAP_W, MINIMAP_H);
    assert!(
        driver.screen_contains("Minimap demo — line 0"),
        "initial status bar should paint fine alongside the minimap track: {:?}",
        driver.painted_texts()
    );

    driver.press_named(NamedKey::Down);
    assert!(
        driver.screen_contains("Minimap demo — line 1"),
        "scrolling must keep re-rendering every frame without panicking: {:?}",
        driver.painted_texts()
    );
}

const IMAGE_W: u32 = 400;
const IMAGE_H: u32 = 200;

/// `draw_image` decodes the demo's real `quadra_logo.png` asset and
/// paints it via Core Graphics/ImageIO on macOS — no longer the only
/// backend that could paint nothing for `ImageApp`'s logo. The menu bar
/// beside the icon still routes clicks correctly, proving the real paint
/// doesn't disturb the icon-narrowed rect math both `render` and `handle` share.
#[test]
fn image_app_logo_paints_and_menu_click_routing_still_works() {
    let mut driver = MacDriver::new(ImageApp::new(), IMAGE_W, IMAGE_H);

    // Unlike TUI (which paints `Image::fallback_text` for its own
    // categorical Unsupported case), macOS never paints fallback text —
    // it now paints the real decoded asset instead.
    assert!(
        !driver.screen_contains("[Q]"),
        "a decodable source must not fall back to fallback_text: {:?}",
        driver.painted_texts()
    );

    // The icon column is `4 * line_height` wide, anchored at the bar's
    // top-left corner (`ImageApp::bar_rects`) — scan inside it for a
    // pixel that isn't plain background, proving the logo actually
    // rasterised rather than leaving the column blank.
    let lh = driver.backend().line_height();
    let icon_w = (lh * 4.0).round() as u32;
    let icon_h = lh.round() as u32;
    let painted_any = (0..icon_w).any(|x| {
        (0..icon_h.max(1)).any(|y| {
            let (_, _, _, a) = driver.pixel(x, y);
            a > 0
        })
    });
    assert!(
        painted_any,
        "expected the real Core Graphics/ImageIO decoder (#962) to paint \
         non-transparent pixels somewhere in the icon column"
    );

    // The icon's reserved width still narrows the menu bar's hit-test
    // rect (`ImageApp::bar_rects`, shared by every backend), so a real
    // click on "File" must still route correctly alongside the now-real
    // icon paint.
    driver.click_text("File");
    assert!(
        driver.screen_contains("activated: &File"),
        "clicking the File menu item must still route through the \
         icon-narrowed rect and update the status bar: {:?}",
        driver.painted_texts()
    );
}

// ─── TabIconsDemo: the CoreText icon-width pass ───────────────────────────
//
// The macOS half of icon-width handling. `MacBackend::draw_tab_bar_icons`
// processes the sidecar for decorated tabs, so glyphs paint and labels shift
// correctly. The pixel-level paint↔hit round trip lives in
// `src/macos/tab_bar.rs`'s `#[cfg(test)]` block; this file covers the same
// behaviour through the example an operator actually runs.

const TAB_ICONS_W: u32 = 640;
const TAB_ICONS_H: u32 = 120;

/// Mirrors the private `macos::tab_bar::TAB_ICON_GAP` (and GTK's
/// `TAB_ICON_GAP`, deliberately the same value) — duplicated here on
/// purpose so a backend that quietly changes its gap has to come and
/// change this cross-backend parity number too.
const TAB_ICON_GAP_PT: f32 = 6.0;

#[test]
fn tab_icons_demo_paints_the_icon_glyph_as_its_own_run() {
    let mut driver = MacDriver::new(TabIconsDemo::new(), TAB_ICONS_W, TAB_ICONS_H);
    // One `draw_text` run per glyph, painted in `TabIcon::color`.
    for glyph in ["R", "T", "M"] {
        assert!(
            driver.painted_texts().contains(&glyph),
            "icon glyph {glyph:?} should paint as its own text run: {:?}",
            driver.painted_texts()
        );
    }

    // …and it must stop painting when the sidecar is emptied, which is
    // what proves the glyphs come from the sidecar rather than a label.
    driver.type_char('i');
    assert!(
        driver.screen_contains("icons: off"),
        "the toggle should be reflected in the hint bar: {:?}",
        driver.painted_texts()
    );
    for glyph in ["R", "T", "M"] {
        assert!(
            !driver.painted_texts().contains(&glyph),
            "with an empty sidecar no icon run should paint: {:?}",
            driver.painted_texts()
        );
    }
}

#[test]
fn tab_icons_demo_toggling_the_sidecar_shifts_labels_by_the_reservation() {
    let mut driver = MacDriver::new(TabIconsDemo::new(), TAB_ICONS_W, TAB_ICONS_H);
    let with_icons = driver
        .find_bounds("main.rs")
        .expect("label should paint with icons on");
    let glyph = driver
        .find_bounds("R")
        .expect("tab 0's icon glyph should paint ahead of its label");
    assert!(
        glyph.x < with_icons.x,
        "the glyph paints at the tab's leading edge, before the label \
         (glyph x {} vs label x {})",
        glyph.x,
        with_icons.x,
    );

    driver.type_char('i');
    let without_icons = driver
        .find_bounds("main.rs")
        .expect("label should still paint with icons off");

    // The reservation is exactly the CoreText-measured glyph width plus
    // the gap — not a guessed constant, and not zero.
    let shift = with_icons.x - without_icons.x;
    assert!(
        (shift - (glyph.width + TAB_ICON_GAP_PT)).abs() < 0.01,
        "an empty sidecar must give back exactly the glyph width \
         ({}) plus the {TAB_ICON_GAP_PT}pt gap, but the label moved {shift}",
        glyph.width,
    );
}

/// A click on the *trailing* end of a decorated tab — past where an
/// icon-blind layout would have put that tab's slot — still routes to the
/// tab the user saw. Three upstream icon reservations have pushed tab 2's
/// slot right, so a rasteriser that painted glyphs without widening the
/// slots would leave tab 0 active.
#[test]
fn tab_icons_demo_click_past_a_decorated_tab_label_activates_that_tab() {
    let mut driver = MacDriver::new(TabIconsDemo::new(), TAB_ICONS_W, TAB_ICONS_H);
    let main = driver
        .find_bounds("main.rs")
        .expect("tab 0's label should paint");
    let readme = driver
        .find_bounds("README.md")
        .expect("tab 2's label should paint");

    // Sample each tab's background in the bare gap between its icon glyph
    // and its label — `TAB_ICON_GAP_PT` wide, so no glyph ink lands there
    // and the pixel is the tab's fill colour.
    let bg_at = |d: &MacDriver<TabIconsDemo>, label: &quadraui::Rect| {
        d.pixel(
            (label.x - TAB_ICON_GAP_PT / 2.0) as u32,
            (label.y + label.height / 2.0) as u32,
        )
    };
    let main_before = bg_at(&driver, &main);
    let readme_before = bg_at(&driver, &readme);
    assert_ne!(
        main_before, readme_before,
        "tab 0 starts active and tab 2 inactive, so their fills differ",
    );

    // Just past the label's trailing edge — the close-glyph end of tab 2's
    // slot, derived from painted geometry rather than hardcoded.
    driver.click(
        readme.x + readme.width + 4.0,
        readme.y + readme.height / 2.0,
    );

    let main_after = bg_at(&driver, &main);
    let readme_after = bg_at(&driver, &readme);
    assert_eq!(
        readme_after, main_before,
        "clicking inside tab 2's painted slot should make it active",
    );
    assert_eq!(main_after, readme_before, "and tab 0 should go inactive");
}

#[test]
fn tab_icons_demo_pressing_q_exits() {
    let mut driver = MacDriver::new(TabIconsDemo::new(), TAB_ICONS_W, TAB_ICONS_H);
    assert!(!driver.exited());
    driver.type_char('q');
    assert!(driver.exited(), "'q' should make the demo exit");
}

/// Close-glyph center for tab `tab_idx` in `TabIconsDemo`'s initial
/// layout (all three `tab_icons_demo::LABELS` open, tab 0 active).
/// `MacDriver` has no `tab_close_center` helper of its own (unlike
/// `TuiDriver`/`GtkDriver`, which cache a `TabBarLayout` per paint), so
/// this resolves the same geometry `TabIconsDemo::handle` does: build the
/// identical `TabBar` + icon sidecar and run them through
/// `Backend::tab_bar_layout_icons`, the no-paint twin of the call
/// `TabIconsDemo::render` paints with. `rect.x` is `0.0`, so the returned
/// bounds are already in the surface coordinates `MacDriver::click` takes.
#[allow(deprecated)] // `Backend::tab_bar_layout_icons` returns `TabBarHits`
fn tab_icons_initial_close_center(driver: &MacDriver<TabIconsDemo>, tab_idx: usize) -> (f32, f32) {
    let viewport = driver.backend().viewport();
    let lh = driver.backend().line_height();
    let bar = quadraui::TabBar {
        id: quadraui::WidgetId::new("tab-icons-demo:tabs"),
        tabs: tab_icons_demo::LABELS
            .iter()
            .enumerate()
            .map(|(i, label)| quadraui::TabItem {
                label: label.to_string(),
                is_active: i == 0,
                ..Default::default()
            })
            .collect(),
        scroll_offset: 0,
        right_segments: vec![],
        active_accent: None,
        show_tab_close: true,
        compact: false,
    };
    let icons = TabIconsDemo::new().icons();
    let hits = driver.backend().tab_bar_layout_icons(
        quadraui::Rect::new(0.0, 0.0, viewport.width, lh),
        &bar,
        &icons,
    );
    let (start, end) = hits.close_bounds[tab_idx].expect("every tab in this demo is closable");
    (((start + end) / 2.0) as f32, lh / 2.0)
}

#[test]
fn tab_icons_demo_clicking_a_tab_close_glyph_closes_that_tab() {
    let mut driver = MacDriver::new(TabIconsDemo::new(), TAB_ICONS_W, TAB_ICONS_H);
    assert!(
        driver.find_bounds("README.md").is_some(),
        "tab 2's label should be painted before it is closed"
    );
    let (x, y) = tab_icons_initial_close_center(&driver, 2);
    driver.click(x, y);
    assert!(
        driver.find_bounds("README.md").is_none(),
        "clicking tab 2's close glyph should remove it from the bar: {:?}",
        driver.painted_texts()
    );
    assert!(
        driver.find_bounds("main.rs").is_some(),
        "the other tabs should stay painted: {:?}",
        driver.painted_texts()
    );
}

#[test]
fn tab_icons_demo_closing_the_active_tab_activates_the_next_one() {
    let mut driver = MacDriver::new(TabIconsDemo::new(), TAB_ICONS_W, TAB_ICONS_H);
    let main_before = driver
        .find_bounds("main.rs")
        .expect("tab 0's label should be painted");
    let active_bg_before = driver.pixel(
        (main_before.x - TAB_ICON_GAP_PT / 2.0) as u32,
        (main_before.y + main_before.height / 2.0) as u32,
    );

    let (x, y) = tab_icons_initial_close_center(&driver, 0);
    driver.click(x, y);
    assert!(
        driver.find_bounds("main.rs").is_none(),
        "closing the active tab should remove it: {:?}",
        driver.painted_texts()
    );

    let cargo = driver
        .find_bounds("Cargo.toml")
        .expect("tab 1 should still be painted and should now be active");
    let active_bg_after = driver.pixel(
        (cargo.x - TAB_ICON_GAP_PT / 2.0) as u32,
        (cargo.y + cargo.height / 2.0) as u32,
    );
    assert_eq!(
        active_bg_after,
        active_bg_before,
        "the next remaining tab should become active, inheriting the \
         active background: {:?}",
        driver.painted_texts()
    );
}

#[test]
fn tab_icons_demo_closing_a_middle_active_tab_activates_its_right_neighbour() {
    // Regression for the "always promotes the leftmost tab" bug: activate
    // the *middle* tab, close it, and assert its right neighbour — not
    // tab 0 — becomes active. `tab_icons_demo_closing_the_active_tab_activates_the_next_one`
    // only exercises closing tab 0, which is the one case where "leftmost"
    // and "right neighbour" happen to coincide.
    let mut driver = MacDriver::new(TabIconsDemo::new(), TAB_ICONS_W, TAB_ICONS_H);

    // Tab 0 ("main.rs") starts active — capture the shared "active tab"
    // background once, while it's still the one wearing it, so the later
    // assertion isn't fooled by comparing two *currently* inactive tabs
    // to each other.
    let main_before = driver
        .find_bounds("main.rs")
        .expect("tab 0's label should be painted");
    let active_bg = driver.pixel(
        (main_before.x - TAB_ICON_GAP_PT / 2.0) as u32,
        (main_before.y + main_before.height / 2.0) as u32,
    );

    let cargo = driver
        .find_bounds("Cargo.toml")
        .expect("tab 1's label should be painted");
    driver.click(cargo.x + cargo.width / 2.0, cargo.y + cargo.height / 2.0);
    assert!(
        driver.find_bounds("Cargo.toml").is_some(),
        "tab 1 should still be painted and now be active: {:?}",
        driver.painted_texts()
    );

    // Close-glyph geometry doesn't depend on which tab is active, only on
    // which tabs are open, so the initial-layout helper still resolves
    // tab 1's close-glyph center correctly here.
    let (x, y) = tab_icons_initial_close_center(&driver, 1);
    driver.click(x, y);
    assert!(
        driver.find_bounds("Cargo.toml").is_none(),
        "closing tab 1 should remove it: {:?}",
        driver.painted_texts()
    );

    let main = driver
        .find_bounds("main.rs")
        .expect("tab 0 should still be painted");
    let main_bg = driver.pixel(
        (main.x - TAB_ICON_GAP_PT / 2.0) as u32,
        (main.y + main.height / 2.0) as u32,
    );
    let readme = driver
        .find_bounds("README.md")
        .expect("tab 2 should still be painted");
    let readme_bg = driver.pixel(
        (readme.x - TAB_ICON_GAP_PT / 2.0) as u32,
        (readme.y + readme.height / 2.0) as u32,
    );
    assert_eq!(
        readme_bg,
        active_bg,
        "closing the active middle tab should activate its right \
         neighbour (README.md): {:?}",
        driver.painted_texts()
    );
    assert_ne!(
        main_bg,
        active_bg,
        "the leftmost tab (main.rs) should stay inactive, not be \
         wrongly promoted: {:?}",
        driver.painted_texts()
    );
}

#[test]
fn tab_icons_demo_closing_the_last_active_tab_activates_its_left_neighbour() {
    // Closing the *rightmost* open tab while it's active has no right
    // neighbour to promote, so it falls back to the new last tab.
    let mut driver = MacDriver::new(TabIconsDemo::new(), TAB_ICONS_W, TAB_ICONS_H);

    // Tab 0 ("main.rs") starts active — capture the shared "active tab"
    // background once, while it's still the one wearing it.
    let main_before = driver
        .find_bounds("main.rs")
        .expect("tab 0's label should be painted");
    let active_bg = driver.pixel(
        (main_before.x - TAB_ICON_GAP_PT / 2.0) as u32,
        (main_before.y + main_before.height / 2.0) as u32,
    );

    let readme = driver
        .find_bounds("README.md")
        .expect("tab 2's label should be painted");
    driver.click(
        readme.x + readme.width / 2.0,
        readme.y + readme.height / 2.0,
    );
    assert!(
        driver.find_bounds("README.md").is_some(),
        "tab 2 should still be painted and now be active: {:?}",
        driver.painted_texts()
    );

    let (x, y) = tab_icons_initial_close_center(&driver, 2);
    driver.click(x, y);
    assert!(
        driver.find_bounds("README.md").is_none(),
        "closing tab 2 should remove it: {:?}",
        driver.painted_texts()
    );

    let main = driver
        .find_bounds("main.rs")
        .expect("tab 0 should still be painted");
    let main_bg = driver.pixel(
        (main.x - TAB_ICON_GAP_PT / 2.0) as u32,
        (main.y + main.height / 2.0) as u32,
    );
    let cargo = driver
        .find_bounds("Cargo.toml")
        .expect("tab 1 should still be painted and should now be active");
    let cargo_bg = driver.pixel(
        (cargo.x - TAB_ICON_GAP_PT / 2.0) as u32,
        (cargo.y + cargo.height / 2.0) as u32,
    );
    assert_eq!(
        cargo_bg,
        active_bg,
        "closing the active last tab should fall back to its left \
         neighbour (Cargo.toml): {:?}",
        driver.painted_texts()
    );
    assert_ne!(
        main_bg,
        active_bg,
        "the leftmost tab (main.rs) should stay inactive, not be \
         wrongly promoted: {:?}",
        driver.painted_texts()
    );
}

// ─── flex-layout packing stays identical on macOS ──────────────────────────
//
// `AppShell`, `Form` and `Toolbar` each route their 1D position arithmetic
// through `crate::flex::pack_1d` when the `layout` feature is enabled, and
// through the original hand-rolled arithmetic when it is not. The three
// tests below are the macOS twins of `tests/tui_example_driver.rs`'s
// `toolbar_items_pack_left_to_right_in_one_row` /
// `appshell_chrome_bands_carve_left_to_right` /
// `form_fields_stack_and_group_items_pack` — same fixtures, same packing
// claims, asserted in points via `find_bounds` rather than cells via a
// character-grid `screen()` string, since `MacDriver` has no grid to scan:
// there is no `row_containing`/divider-column-scan equivalent here, and
// activity-bar icons paint as Nerd-Font glyphs (not the ASCII `E`/`S`/`G`
// fallback TUI's test keys off), so the AppShell band check below is
// `find_bounds` on painted *text* chrome only (sidebar header, main-area
// hint), not the activity bar itself.

/// `(x, y, width)` of each needle's painted bounds, in the order given.
///
/// Panics naming the first needle that isn't on screen.
fn mac_bounds_of<A: quadraui::AppLogic>(
    driver: &MacDriver<A>,
    needles: &[&str],
) -> Vec<(f32, f32, f32)> {
    needles
        .iter()
        .map(|needle| {
            let b = driver
                .find_bounds(needle)
                .unwrap_or_else(|| panic!("{needle:?} not painted: {:?}", driver.painted_texts()));
            (b.x, b.y, b.width)
        })
        .collect()
}

/// Assert `items` sit on one row, in the given order, without overlapping.
fn mac_assert_packed_in_one_row(items: &[(f32, f32, f32)], labels: &[&str]) {
    for (pair, names) in items.windows(2).zip(labels.windows(2)) {
        let (lx, ly, lw) = pair[0];
        let (rx, ry, _) = pair[1];
        assert_eq!(
            ly, ry,
            "{:?} and {:?} must pack onto the same row",
            names[0], names[1]
        );
        assert!(
            lx + lw <= rx,
            "{:?} (x={lx}, w={lw}) must end before {:?} starts (x={rx})",
            names[0],
            names[1]
        );
    }
}

#[test]
fn toolbar_items_pack_left_to_right_in_one_row() {
    let driver = MacDriver::new(ToolbarApp::new(), 900, 120);

    // `Toolbar::layout` decides each item's left edge, so any packing
    // drift pulls one of these out of row-order or overlapping with its
    // left neighbour.
    let labels = ["Continue", "Pause", "Filter", "Reset", "Debug"];
    mac_assert_packed_in_one_row(&mac_bounds_of(&driver, &labels), &labels);
}

#[test]
fn appshell_chrome_bands_carve_left_to_right() {
    let config = AppShellDemo::config();
    let driver = driver_with_shell(AppShellDemo::new(), config, SHELL_W, SHELL_H);

    // Bands run left to right: activity bar, sidebar, main area. The
    // sidebar header and the main-area hint text are both chrome this
    // demo paints every frame regardless of which panel is active, so
    // their relative x-order is the band carve's signature.
    let [(explorer_x, _, _), (main_x, _, _)] =
        mac_bounds_of(&driver, &["EXPLORER", "Tab=focus bar"])[..]
    else {
        unreachable!("mac_bounds_of returns one entry per needle")
    };
    assert!(
        main_x > explorer_x,
        "main area (x={main_x}) sits right of the sidebar header (x={explorer_x})"
    );

    // The sidebar band's content starts inside the sidebar, left of the
    // main area — the same "band clips its content" claim TUI's test
    // makes by comparing against the divider column.
    let content = driver
        .find_bounds("(sidebar content here)")
        .expect("sidebar content placeholder should be painted");
    assert!(
        content.x < main_x,
        "sidebar content (x={}) starts left of the main area (x={main_x})",
        content.x
    );
}

#[test]
fn form_fields_stack_and_group_items_pack() {
    let driver = MacDriver::new(FormGroupsApp::new(), 700, 400);
    let line_height = driver.backend().line_height();
    // `Form` rows are `(line_height * 1.4).round()` tall
    // (`layout_metrics::form_row_height`), not `line_height` itself —
    // every `FormField` (including `ToggleGroup`/`ButtonRow`) measures
    // exactly one row of this height
    // (`form_field_measure_height_is_one_row_for_every_field_kind`).
    let row_h = quadraui::primitives::layout_metrics::form_row_height(line_height as f64);

    // Each field's top edge is the running sum of the row heights above
    // it. The fixture's five fields are: "search" (label "Find"),
    // "toggles" (`ToggleGroup`, no label), "replace" (label "Replace"),
    // "buttons" (`ButtonRow`, no label — one of its buttons is itself
    // labelled "Replace"), "scope" (label "Scope"). So the row carrying
    // the "Replace" label sits two rows below "Find" (skipping
    // "toggles"), and "Scope" sits two rows below that (skipping
    // "buttons").
    //
    // The "replace" field's `TextInput` is empty, so it paints its
    // placeholder `"Replace…"` (with the ellipsis glyph) in the same row
    // as the "Replace" label — unlike the bare label text, this needle
    // can't collide with the "Replace"/"Replace All" buttons painted
    // later in the "buttons" row, which don't share that glyph.
    let [(_, find_y, _), (_, replace_y, _), (_, scope_y, _)] =
        mac_bounds_of(&driver, &["Find", "Replace…", "Scope"])[..]
    else {
        unreachable!("mac_bounds_of returns one entry per needle")
    };
    // Compared with a tolerance rather than `assert_eq!`: every row
    // centres its text vertically by a fractional offset
    // `(row_h - text_h) / 2`, and adding that offset to a two-row
    // baseline before subtracting it again costs a few ULPs of `f32`.
    // The geometric claim is "exactly two rows", at point precision.
    let two_rows = 2.0 * row_h as f32;
    assert!(
        (replace_y - find_y - two_rows).abs() < 0.01,
        "Replace field stacks two rows below Find, skipping the toggles row \
         (find_y={find_y}, replace_y={replace_y}, row_h={row_h})"
    );
    assert!(
        (scope_y - replace_y - two_rows).abs() < 0.01,
        "Scope field stacks two rows below Replace, skipping the buttons row \
         (replace_y={replace_y}, scope_y={scope_y}, row_h={row_h})"
    );

    // The other half of `Form`'s packing: items *inside* one field pack
    // left-to-right with the field's own gap. The "scope" field is a
    // `FieldKind::Toolbar`, so its three buttons are laid out by the
    // embedded `Toolbar` rasteriser rather than by `Form` itself —
    // worth asserting separately from the `ButtonRow` below, which
    // `Form` packs directly. "Find Next" and "Replace All" bracket that
    // button row; the middle "Replace" button is skipped because its
    // label is a prefix of both its neighbour's and the field label
    // above it, and `find_bounds` matches painted runs by substring.
    let scope_buttons = ["Workspace", "File", "Selection"];
    mac_assert_packed_in_one_row(&mac_bounds_of(&driver, &scope_buttons), &scope_buttons);
    let buttons = ["Find Next", "Replace All"];
    mac_assert_packed_in_one_row(&mac_bounds_of(&driver, &buttons), &buttons);
}

// ─── CanvasApp (gauge): parity with the TUI/GTK suites ─────────────────────
//
// `tests/tui_example_driver.rs` and `tests/gtk_example_driver.rs`'s
// `canvas_demo_*` tests already cover this behaviourally; these mirror them
// on macOS so the gauge has the same coverage on every backend before the
// Canvas v2 op tests below exercise the ops the macOS rasteriser paints
// through `impl PaintSurface for MacBackend`.

/// Mirrors `examples/common/canvas_app.rs::CanvasApp::gauge_rect` (private
/// to the example) from the backend's own metrics, never a hardcoded
/// pixel — the same contract the example's own click handler relies on.
fn canvas_app_gauge_rect(driver: &MacDriver<CanvasApp>) -> quadraui::Rect {
    let viewport = Backend::viewport(driver.backend());
    let lh = Backend::line_height(driver.backend());
    quadraui::Rect::new(lh, lh * 2.0, (viewport.width - lh * 2.0).max(0.0), lh)
}

#[test]
fn canvas_demo_paints_the_initial_gauge_label() {
    let driver = MacDriver::new(CanvasApp::new(), W, H);
    assert!(
        driver.screen_contains("30%"),
        "the gauge should start at 30%: {:?}",
        driver.painted_texts()
    );
}

#[test]
fn canvas_demo_plus_key_raises_the_value() {
    let mut driver = MacDriver::new(CanvasApp::new(), W, H);
    driver.type_char('+');
    assert!(
        driver.screen_contains("40%"),
        "'+' should raise the gauge from 30% to 40%: {:?}",
        driver.painted_texts()
    );
}

#[test]
fn canvas_demo_up_arrow_raises_the_value() {
    let mut driver = MacDriver::new(CanvasApp::new(), W, H);
    driver.press_named(NamedKey::Up);
    assert!(
        driver.screen_contains("40%"),
        "Up should raise the gauge from 30% to 40%, same as '+': {:?}",
        driver.painted_texts()
    );
}

#[test]
fn canvas_demo_minus_key_lowers_the_value() {
    let mut driver = MacDriver::new(CanvasApp::new(), W, H);
    driver.type_char('-');
    assert!(
        driver.screen_contains("20%"),
        "'-' should lower the gauge from 30% to 20%: {:?}",
        driver.painted_texts()
    );
}

#[test]
fn canvas_demo_value_never_exceeds_100_or_drops_below_0() {
    let mut driver = MacDriver::new(CanvasApp::new(), W, H);
    for _ in 0..20 {
        driver.type_char('+');
    }
    assert!(
        driver.screen_contains("100%"),
        "repeated '+' should clamp at 100%, not wrap or overflow: {:?}",
        driver.painted_texts()
    );
    for _ in 0..20 {
        driver.type_char('-');
    }
    assert!(
        driver.screen_contains("0%") && !driver.screen_contains("100%"),
        "repeated '-' should clamp at 0%, not underflow: {:?}",
        driver.painted_texts()
    );
}

/// Clicking inside the gauge jumps straight to that position's value. Unlike
/// the TUI twin (a fixed 100-wide cell grid), macOS's gauge rect comes from
/// real backend metrics, so the click positions are derived from
/// `canvas_app_gauge_rect` instead of a hardcoded coordinate.
#[test]
fn canvas_demo_clicking_the_gauge_jumps_to_that_positions_value() {
    let driver = MacDriver::new(CanvasApp::new(), W, H);
    let rect = canvas_app_gauge_rect(&driver);
    let mut driver = driver;

    let near_right = rect.x + rect.width - 1.0;
    driver.click(near_right, rect.y + rect.height / 2.0);
    let frac = ((near_right - rect.x) / rect.width).clamp(0.0, 1.0);
    let expected = format!("{}%", (frac * 100.0).round() as u8);
    assert!(
        driver.screen_contains(&expected),
        "clicking near the gauge's right edge should jump close to 100% \
         (expected {expected:?}): {:?}",
        driver.painted_texts()
    );

    let driver = MacDriver::new(CanvasApp::new(), W, H);
    let rect = canvas_app_gauge_rect(&driver);
    let mut driver = driver;
    let near_left = rect.x + 1.0;
    driver.click(near_left, rect.y + rect.height / 2.0);
    let frac = ((near_left - rect.x) / rect.width).clamp(0.0, 1.0);
    let expected = format!("{}%", (frac * 100.0).round() as u8);
    assert!(
        driver.screen_contains(&expected),
        "clicking near the gauge's left edge should jump close to 0% \
         (expected {expected:?}): {:?}",
        driver.painted_texts()
    );
}

/// A click outside the gauge's own row must not change the value —
/// `CanvasHit::Outside` is a no-op, not a clamped-in-range guess.
#[test]
fn canvas_demo_clicking_outside_the_gauge_does_not_change_the_value() {
    let driver = MacDriver::new(CanvasApp::new(), W, H);
    let rect = canvas_app_gauge_rect(&driver);
    let mut driver = driver;
    driver.click(rect.x + rect.width / 2.0, rect.y + rect.height + 40.0);
    assert!(
        driver.screen_contains("30%"),
        "a click outside the gauge's row should leave the value at 30%: {:?}",
        driver.painted_texts()
    );
}

#[test]
fn canvas_demo_q_exits() {
    let mut driver = MacDriver::new(CanvasApp::new(), W, H);
    assert!(!driver.exited());
    driver.type_char('q');
    assert!(driver.exited(), "'q' should exit the canvas demo");
}

#[test]
fn canvas_demo_escape_exits() {
    let mut driver = MacDriver::new(CanvasApp::new(), W, H);
    driver.press_named(NamedKey::Escape);
    assert!(driver.exited(), "Escape should exit the canvas demo");
}

// ─── CanvasShapesApp: the Canvas v2 op set — real macOS rendering ──────────
//
// `tests/tui_example_driver.rs`'s `canvas_shapes_demo_*` tests already prove
// the whole `Backend::draw_canvas` round trip reaches the screen for the new
// op set generically (every op degrades to TUI's braille grid), and
// `tests/gtk_example_driver.rs` proves it on GTK's real rasteriser. These
// check *real* pixel colour and glyph runs on macOS — `impl PaintSurface for
// MacBackend` actually painting each new op (`Polygon` / `Ellipse` / `Arc` /
// `Bezier` / `DashedLine` / `StrokeRect` / `StyledTextRun`), not just a
// backend-agnostic degrade path.

/// The demo's initial, deterministic bar values — `CanvasShapesApp::new()`.
const SHAPES_VALUES: [f32; 5] = [35.0, 60.0, 45.0, 80.0, 55.0];

/// Mirrors the private geometry `examples/common/canvas_shapes_app.rs`'s
/// `CanvasShapesApp::chart_rect` / `chart_canvas` build, from the backend's
/// own metrics — never a hardcoded pixel.
struct ChartGeometry {
    rect: quadraui::Rect,
    plot_h: f32,
    slot_w: f32,
    bar_w: f32,
    bar_gap: f32,
}

impl ChartGeometry {
    fn new(driver: &MacDriver<CanvasShapesApp>) -> Self {
        let viewport = Backend::viewport(driver.backend());
        let lh = Backend::line_height(driver.backend());
        let rect = quadraui::Rect::new(
            lh,
            lh * 2.0,
            (viewport.width - lh * 2.0).max(1.0),
            (viewport.height - lh * 5.0).max(1.0),
        );
        let label_h = (rect.height * 0.15).max(1.0);
        let plot_h = (rect.height - label_h).max(1.0);
        let n = SHAPES_VALUES.len() as f32;
        let slot_w = rect.width / n;
        let bar_w = slot_w * 0.6;
        let bar_gap = (slot_w - bar_w) / 2.0;
        Self {
            rect,
            plot_h,
            slot_w,
            bar_w,
            bar_gap,
        }
    }

    /// Bar `i`'s top-y, in LOCAL (chart-rect-relative) coordinates, for a
    /// given value out of 100.
    fn bar_top_y(&self, value: f32) -> f32 {
        self.plot_h * (1.0 - value.clamp(0.0, 100.0) / 100.0)
    }

    /// Bar `i`'s left edge, in LOCAL coordinates.
    fn bar_x(&self, i: usize) -> f32 {
        self.slot_w * i as f32 + self.bar_gap
    }

    /// LOCAL chart coordinates to absolute screen pixel coordinates.
    /// `MacDriver::pixel` takes unsigned coordinates; a cast from a
    /// negative `f32` saturates to `0` rather than panicking or wrapping.
    fn to_screen(&self, local: quadraui::Point) -> (u32, u32) {
        (
            (self.rect.x + local.x).round() as u32,
            (self.rect.y + local.y).round() as u32,
        )
    }
}

#[test]
fn canvas_shapes_demo_paints_the_status_bar_and_initial_labels() {
    let driver = MacDriver::new(CanvasShapesApp::new(), W, H);
    assert!(
        driver.screen_contains("Canvas v2 shapes demo"),
        "the status bar should name the demo: {:?}",
        driver.painted_texts()
    );
    assert!(
        driver.screen_contains("Mon 35") && driver.screen_contains("Fri 55"),
        "the initial bar values should reach the screen through the \
         StyledTextRun labels: {:?}",
        driver.painted_texts()
    );
}

/// A point inside bar 3's `Polygon` trapezoid fill, a few pixels above the
/// plot's bottom edge — inside the trapezoid for any bar height, and far
/// from the StrokeRect border or any of the three DashedLine gridlines
/// (fixed at 25/50/75% of `plot_h`, never within a few pixels of `plot_h`
/// itself).
#[test]
fn canvas_shapes_demo_polygon_bar_fill_paints_the_bar_color() {
    let driver = MacDriver::new(CanvasShapesApp::new(), W, H);
    let geo = ChartGeometry::new(&driver);
    let bar_color = (80u8, 160u8, 255u8);

    let x = geo.bar_x(3) + geo.bar_w / 2.0;
    let y = geo.plot_h - 6.0;
    let (sx, sy) = geo.to_screen(quadraui::Point::new(x, y));

    assert!(
        colors_close(driver.pixel(sx, sy), bar_color, 10),
        "a point inside bar 3's Polygon fill at ({sx}, {sy}) should be the bar colour \
         {bar_color:?}, got {:?}",
        driver.pixel(sx, sy)
    );
}

/// The `Ellipse` peak marker should be the marker colour a few pixels above
/// or below its mathematical centre. The centre itself is also the Bezier
/// trend curve's exact endpoint at that bar (the curve's control points
/// share the endpoint's y, giving it a horizontal tangent there), and the
/// curve paints after the marker — so the dead centre pixel is legitimately
/// the trend colour, not the marker colour. Sampling just off-centre stays
/// inside the marker's radius (`bar_w*0.18`) while clearing the curve.
#[test]
fn canvas_shapes_demo_ellipse_marker_paints_the_marker_color() {
    let driver = MacDriver::new(CanvasShapesApp::new(), W, H);
    let geo = ChartGeometry::new(&driver);
    let marker_color = (255u8, 210u8, 90u8);

    let x = geo.bar_x(3) + geo.bar_w / 2.0;
    let center_y = geo.bar_top_y(SHAPES_VALUES[3]);
    let mut samples = Vec::new();
    for dy in [-5.0f32, 5.0] {
        let (sx, sy) = geo.to_screen(quadraui::Point::new(x, center_y + dy));
        samples.push(((sx, sy), driver.pixel(sx, sy)));
    }

    assert!(
        samples
            .iter()
            .any(|&(_, px)| colors_close(px, marker_color, 10)),
        "the Ellipse peak marker near its centre should be the marker colour \
         {marker_color:?}, got {samples:?}"
    );
}

/// The area outside the chart's own shapes is untouched canvas backing —
/// `MacBackend`'s bitmap context stores premultiplied alpha, so a partially
/// covered antialiased edge pixel blended against that fully transparent
/// backing looks like a faded, not pure, source colour (its RGB channels are
/// already scaled by its own coverage). Dividing them back out by the pixel's
/// own alpha recovers the exact source colour at any coverage fraction above
/// zero, so a half-covered edge pixel reads the same as a fully covered one.
fn unpremultiply(px: (u8, u8, u8, u8)) -> (u8, u8, u8, u8) {
    if px.3 == 0 {
        return (0, 0, 0, 0);
    }
    let a = px.3 as f32 / 255.0;
    (
        ((px.0 as f32 / a).round() as i32).clamp(0, 255) as u8,
        ((px.1 as f32 / a).round() as i32).clamp(0, 255) as u8,
        ((px.2 as f32 / a).round() as i32).clamp(0, 255) as u8,
        255,
    )
}

/// Scans the bottom edge of the plot's `StrokeRect` border across the first
/// inter-bar gap (never under a bar's own fill) and a small y band either
/// side of the mathematically-exact edge. The scanned area outside the
/// plotted shapes is untouched (fully transparent) canvas backing, so each
/// sample is unpremultiplied before comparison — see `unpremultiply`.
#[test]
fn canvas_shapes_demo_stroke_rect_paints_a_border() {
    let driver = MacDriver::new(CanvasShapesApp::new(), W, H);
    let geo = ChartGeometry::new(&driver);
    let border_color = (90u8, 90u8, 90u8);

    let gap_x = geo.bar_gap / 2.0;
    let mut samples = Vec::new();
    for dx in 0..(geo.bar_gap as i32).max(1) {
        for dy in -2..=1 {
            let (sx, sy) =
                geo.to_screen(quadraui::Point::new(gap_x + dx as f32, geo.plot_h + dy as f32));
            samples.push(unpremultiply(driver.pixel(sx, sy)));
        }
    }

    assert!(
        samples.iter().any(|&px| colors_close(px, border_color, 10)),
        "the chart's StrokeRect bottom border should be painted somewhere along the \
         scanned band: {samples:?}"
    );
}

/// The middle (50%) `DashedLine` gridline, scanned across its first ~30px —
/// long enough to span several `dash_len=4.0, gap_len=3.0` periods. A real
/// dash shows both the grid colour (dash) and a clearly different colour
/// (gap, plain canvas background) along the same scanline; a solid line
/// would only ever show the grid colour.
///
/// `plot_h * 0.5` is not pixel-aligned, so the nominal row only ever shows a
/// partial antialiased blend of grid colour and background on both "dash"
/// and "gap" pixels. Shifting onto the nearest pixel-aligned row gives dash
/// pixels near-full coverage (differing from `grid_color` by ~10) while gap
/// pixels stay at the plain background (differing by ~40) — cleanly
/// separable at a tolerance in between.
#[test]
fn canvas_shapes_demo_dashed_line_gridline_has_gaps() {
    let driver = MacDriver::new(CanvasShapesApp::new(), W, H);
    let geo = ChartGeometry::new(&driver);
    let grid_color = (60u8, 60u8, 60u8);

    let y = (geo.plot_h * 0.5).round() - 0.5;
    let mut hit = false;
    let mut miss = false;
    for dx in 0..30 {
        let (sx, sy) = geo.to_screen(quadraui::Point::new(dx as f32, y));
        let px = driver.pixel(sx, sy);
        if colors_close(px, grid_color, 15) {
            hit = true;
        } else if !colors_close(px, grid_color, 25) {
            miss = true;
        }
    }

    assert!(
        hit,
        "the gridline should paint the grid colour somewhere along its dashes"
    );
    assert!(
        miss,
        "the gridline should leave visible gaps (DashedLine, not a solid Line) \
         somewhere along the scanned span"
    );
}

/// The first `Bezier` trend-curve segment (bar 0's top to bar 1's top) with
/// control points a third of the way along the horizontal span, same as
/// `CanvasShapesApp::chart_canvas` builds them. Its exact midpoint
/// (`t = 0.5`) is the standard cubic Bézier basis at `t = 0.5`:
/// `0.125*from + 0.375*c1 + 0.375*c2 + 0.125*to`.
#[test]
fn canvas_shapes_demo_bezier_trend_curve_paints_the_trend_color() {
    let driver = MacDriver::new(CanvasShapesApp::new(), W, H);
    let geo = ChartGeometry::new(&driver);
    let trend_color = (255u8, 120u8, 80u8);

    let from = quadraui::Point::new(
        geo.bar_x(0) + geo.bar_w / 2.0,
        geo.bar_top_y(SHAPES_VALUES[0]),
    );
    let to = quadraui::Point::new(
        geo.bar_x(1) + geo.bar_w / 2.0,
        geo.bar_top_y(SHAPES_VALUES[1]),
    );
    let dx = (to.x - from.x) / 3.0;
    let c1 = quadraui::Point::new(from.x + dx, from.y);
    let c2 = quadraui::Point::new(to.x - dx, to.y);
    let mid = quadraui::Point::new(
        0.125 * from.x + 0.375 * c1.x + 0.375 * c2.x + 0.125 * to.x,
        0.125 * from.y + 0.375 * c1.y + 0.375 * c2.y + 0.125 * to.y,
    );

    // Scan a small neighbourhood around the exact midpoint: the curve's 2px
    // stroke centred exactly on the analytic path can land the pixel grid's
    // sample a row or column off depending on rounding.
    let mut samples = Vec::new();
    for ddx in -2..=2 {
        for ddy in -2..=2 {
            let (sx, sy) =
                geo.to_screen(quadraui::Point::new(mid.x + ddx as f32, mid.y + ddy as f32));
            samples.push(driver.pixel(sx, sy));
        }
    }

    assert!(
        samples.iter().any(|&px| colors_close(px, trend_color, 20)),
        "the Bezier trend curve should pass near its analytic midpoint \
         ({:.1}, {:.1}): {samples:?}",
        mid.x,
        mid.y
    );
}

/// The `Arc` dial's point at the midpoint of its sweep angle, same
/// `start_angle`/`end_angle`/`radius` formula as
/// `CanvasShapesApp::chart_canvas`.
#[test]
fn canvas_shapes_demo_arc_dial_paints_the_dial_color() {
    let driver = MacDriver::new(CanvasShapesApp::new(), W, H);
    let geo = ChartGeometry::new(&driver);
    let dial_color = (80u8, 220u8, 120u8);

    let avg = SHAPES_VALUES.iter().sum::<f32>() / SHAPES_VALUES.len() as f32;
    let center = quadraui::Point::new(geo.rect.width - geo.bar_w, geo.bar_w);
    let radius = geo.bar_w * 0.7;
    let start_angle = -std::f32::consts::FRAC_PI_2;
    let end_angle = start_angle + std::f32::consts::PI * (avg.clamp(0.0, 100.0) / 100.0);
    let mid_angle = (start_angle + end_angle) / 2.0;
    let mid = quadraui::Point::new(
        center.x + radius * mid_angle.cos(),
        center.y + radius * mid_angle.sin(),
    );

    let mut samples = Vec::new();
    for ddx in -2..=2 {
        for ddy in -2..=2 {
            let (sx, sy) =
                geo.to_screen(quadraui::Point::new(mid.x + ddx as f32, mid.y + ddy as f32));
            samples.push(driver.pixel(sx, sy));
        }
    }

    assert!(
        samples.iter().any(|&px| colors_close(px, dial_color, 20)),
        "the Arc dial should pass near its analytic midpoint-angle point \
         ({:.1}, {:.1}): {samples:?}",
        mid.x,
        mid.y
    );
}

#[test]
fn canvas_shapes_demo_r_key_randomizes_the_bar_values() {
    let mut driver = MacDriver::new(CanvasShapesApp::new(), W, H);
    assert!(driver.screen_contains("Mon 35") && driver.screen_contains("Fri 55"));
    driver.type_char('r');
    assert!(
        !driver.screen_contains("Mon 35") || !driver.screen_contains("Fri 55"),
        "'r' should redraw with new randomized bar values: {:?}",
        driver.painted_texts()
    );
}

#[test]
fn canvas_shapes_demo_q_exits() {
    let mut driver = MacDriver::new(CanvasShapesApp::new(), W, H);
    driver.type_char('q');
    assert!(driver.exited(), "'q' should exit the canvas shapes demo");
}

#[test]
fn canvas_shapes_demo_escape_exits() {
    let mut driver = MacDriver::new(CanvasShapesApp::new(), W, H);
    driver.press_named(NamedKey::Escape);
    assert!(driver.exited(), "Escape should exit the canvas shapes demo");
}

// ─── Canvas v2 per-op alpha ─────────────────────────────────────────────────
//
// Neither shipped Canvas example uses a translucent `Color` (every op uses
// `Color::rgb`, always `a: 255`), so asserting against them alone would
// never exercise alpha blending. This probe draws two overlapping
// opaque/translucent `DrawOp::Rect`s through one `Backend::draw_canvas` call
// and checks the overlap is a genuine blend of the two.

const ALPHA_PROBE_BASE: quadraui::Color = quadraui::Color::rgb(20, 20, 200);
const ALPHA_PROBE_TOP: quadraui::Color = quadraui::Color::rgba(200, 20, 20, 128);

struct CanvasAlphaProbe;

impl quadraui::AppLogic for CanvasAlphaProbe {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let rect = quadraui::Rect::new(0.0, 0.0, 60.0, 60.0);
        let canvas = quadraui::Canvas {
            id: quadraui::WidgetId::new("probe:canvas-alpha"),
            ops: vec![
                quadraui::DrawOp::Rect {
                    rect: quadraui::Rect::new(0.0, 0.0, 60.0, 60.0),
                    color: ALPHA_PROBE_BASE,
                },
                quadraui::DrawOp::Rect {
                    rect: quadraui::Rect::new(0.0, 0.0, 60.0, 60.0),
                    color: ALPHA_PROBE_TOP,
                },
            ],
        };
        let _ = backend.draw_canvas(rect, &canvas);
    }

    fn handle(&mut self, _event: quadraui::UiEvent, _backend: &mut dyn Backend) -> Reaction {
        Reaction::Continue
    }
}

/// A second `DrawOp::Rect` with `a < 255` layered over the first must blend
/// with it, not simply overwrite it (ignoring alpha) or leave it untouched
/// (ignoring the top op). Base, top's own RGB, and the true blend are far
/// enough apart in each channel that a 10-tolerance match against either
/// source colour alone is conclusive.
#[test]
fn canvas_v2_rect_op_honours_per_op_alpha_on_macos() {
    let driver = MacDriver::new(CanvasAlphaProbe, 80, 80);
    let center = driver.pixel(30, 30);

    assert!(
        !colors_close(
            center,
            (ALPHA_PROBE_BASE.r, ALPHA_PROBE_BASE.g, ALPHA_PROBE_BASE.b),
            10
        ),
        "the translucent top rect should visibly change the base colour, not leave it \
         untouched: got {center:?}"
    );
    assert!(
        !colors_close(
            center,
            (ALPHA_PROBE_TOP.r, ALPHA_PROBE_TOP.g, ALPHA_PROBE_TOP.b),
            10
        ),
        "the translucent top rect should blend with the base, not paint its own RGB \
         opaquely (ignoring `a`): got {center:?}"
    );
}
