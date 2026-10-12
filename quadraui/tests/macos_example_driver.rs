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

// Point canvas — big enough for five pipeline stage boxes + arrow
// connectors + the bottom status bar at macOS's native (point, not cell)
// scale. Same size `tests/cross_backend_parity.rs` uses for its `MacDriver`
// row.
const W: u32 = 800;
const H: u32 = 480;

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
