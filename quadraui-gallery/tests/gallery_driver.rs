//! End-to-end `TuiDriver` tests for the gallery shell. Drives the real
//! [`GalleryApp`] — the same
//! [`quadraui::ShellApp`] `src/main.rs`'s `tui` arm runs — through the
//! `event → handle → render` path against a headless `TestBackend`, via
//! [`quadraui::tui::testing::driver_with_shell`] (the `ShellApp`
//! integration driver, mirroring `quadraui::tui::shell_runner::run_with_shell`).
//!
//! Assertions use `find()` / `screen_contains()` only — no hardcoded
//! coordinates, per `CLAUDE.md`'s "Every TUI example also ships an
//! automated black-box test" rule.
#![cfg(feature = "tui")]

use quadraui::tui::testing::{driver_with_shell, TuiDriver};
use quadraui::{testing::ConformanceDriver, AppLogic};

use quadraui_gallery::app::{group_icon, group_panel_id, GROUPS};
use quadraui_gallery::registry::registry;
use quadraui_gallery::GalleryApp;

/// Index of `"Overlays"` in [`GROUPS`] — the only group with a
/// registered demo today. Resolved from the list rather than hardcoded
/// as `3`, so a future reordering of `GROUPS` doesn't silently break
/// this test's activity-bar click.
fn overlays_group_index() -> usize {
    GROUPS
        .iter()
        .position(|g| *g == "Overlays")
        .expect("GROUPS must include \"Overlays\" while ToastDemo is registered there")
}

/// Click `GROUPS[index]`'s activity-bar item at its real painted bounds,
/// resolved from `ConformanceDriver::inventory()`'s registered zone for
/// [`group_panel_id`] — not a scraped icon glyph. Used by the
/// registry-driven test below, which (unlike the other tests in this
/// file) must visit every group, including ones no demo renders into
/// today, so it can't rely on `driver.find(group_icon(i))` the way those
/// other tests do (a demo registered into `GROUPS[0]` could paint a `"1"`
/// of its own and make that scrape ambiguous).
fn click_group_zone<A: AppLogic>(driver: &mut TuiDriver<A>, index: usize) {
    let id = group_panel_id(index);
    let zone = driver
        .inventory()
        .zones()
        .iter()
        .find(|z| z.id == id)
        .unwrap_or_else(|| panic!("no registered zone for group {index}'s activity-bar item"))
        .bounds;
    driver.click(zone.x + zone.width / 2.0, zone.y + zone.height / 2.0);
}

/// Click the Overlays activity-bar icon, click the Toast sidebar row,
/// then the Code tab — asserting the demo shows, and the Code tab shows
/// text from `ToastDemo`'s own `// gallery:begin` / `// gallery:end`
/// region.
#[test]
fn navigating_group_then_demo_shows_it_and_its_code_region() {
    let config = GalleryApp::config();
    let mut driver = driver_with_shell(GalleryApp::new(), config, 100, 32);

    // Nothing selected yet in the default (Content) group.
    assert!(
        driver.screen_contains("Select a demo"),
        "no demo should be selected in the default group:\n{}",
        driver.screen()
    );

    // Navigate to the Overlays group.
    let icon = group_icon(overlays_group_index());
    let (x, y) = driver.find(icon).unwrap_or_else(|| {
        panic!(
            "Overlays activity-bar icon ({icon}) should paint:\n{}",
            driver.screen()
        )
    });
    driver.click(x, y);
    assert!(
        driver.screen_contains("Toast"),
        "sidebar should list the Toast demo once Overlays is active:\n{}",
        driver.screen()
    );

    // Select the Toast demo from the sidebar.
    let (x, y) = driver
        .find("Toast")
        .unwrap_or_else(|| panic!("Toast sidebar row should paint:\n{}", driver.screen()));
    driver.click(x, y);
    assert!(
        driver.screen_contains("add severity"),
        "Demo tab should render ToastDemo's own content once selected:\n{}",
        driver.screen()
    );

    // Switch to the Code tab.
    let (x, y) = driver
        .find("Code")
        .unwrap_or_else(|| panic!("Code tab label should paint:\n{}", driver.screen()));
    driver.click(x, y);
    assert!(
        driver.screen_contains("pub struct ToastDemo"),
        "Code tab should show text from ToastDemo's gallery:begin/end region:\n{}",
        driver.screen()
    );
}

/// Same navigation as above, but to the Data tab — asserting it shows
/// the active variant's primitive(s) as pretty JSON.
#[test]
fn data_tab_shows_the_primitive_as_json() {
    let config = GalleryApp::config();
    let mut driver = driver_with_shell(GalleryApp::new(), config, 100, 32);

    let icon = group_icon(overlays_group_index());
    let (x, y) = driver.find(icon).expect("Overlays icon should paint");
    driver.click(x, y);
    let (x, y) = driver
        .find("Toast")
        .expect("Toast sidebar row should paint");
    driver.click(x, y);

    let (x, y) = driver.find("Data").expect("Data tab label should paint");
    driver.click(x, y);

    let screen = driver.screen();
    // `ToastDemo::data` serialises its `ToastOverlay`, whose `id` field
    // is the literal string below — present in the JSON regardless of
    // how serde_json wraps whitespace/quoting across cells.
    assert!(
        screen.contains("gallery-toast-stack") || screen.contains("toast-stack"),
        "Data tab should show the ToastOverlay's JSON (looking for its id):\n{screen}"
    );
    assert!(
        screen.contains("severity") || screen.contains("Info"),
        "Data tab's JSON should include the seeded toast's fields:\n{screen}"
    );
}

/// Clicking a demo widget (here: the Toast demo's own dismiss glyph)
/// appends to the bottom-panel event log.
#[test]
fn clicking_a_demo_widget_appends_to_the_event_log() {
    let config = GalleryApp::config();
    let mut driver = driver_with_shell(GalleryApp::new(), config, 100, 32);

    let icon = group_icon(overlays_group_index());
    let (x, y) = driver.find(icon).expect("Overlays icon should paint");
    driver.click(x, y);
    let (x, y) = driver
        .find("Toast")
        .expect("Toast sidebar row should paint");
    driver.click(x, y);

    let before = driver.screen();
    let before_log_lines = before.matches("MouseDown").count();
    assert!(
        before.contains("Welcome"),
        "ToastDemo seeds a Welcome toast:\n{before}"
    );

    // The seeded "Welcome" toast's title row paints as `"Welcome × "` —
    // `×` alone would also match the demo's own decorative hint text
    // ("click ×=dismiss") higher up the screen, so anchor on the whole
    // span and click its *last* cell (the real glyph), not `find("×")`'s
    // first match.
    let span = driver
        .find_bounds("Welcome \u{d7}")
        .unwrap_or_else(|| panic!("toast title + dismiss glyph should paint together:\n{before}"));
    let dismiss_x = span.x + span.width - 0.5;
    let dismiss_y = span.y + span.height / 2.0;
    driver.click(dismiss_x, dismiss_y);

    let after = driver.screen();
    let after_log_lines = after.matches("MouseDown").count();
    assert!(
        after_log_lines > before_log_lines,
        "clicking the dismiss glyph should append a MouseDown entry to the event log:\n{after}"
    );
    assert!(
        !after.contains("Welcome"),
        "clicking the real dismiss glyph should also remove the toast, proving the click \
         reached ToastDemo (not just the hint text above it):\n{after}"
    );
}

/// Keying a demo widget (pressing `1` to add an Info toast) also appends
/// to the event log, and the new toast actually renders.
#[test]
fn keying_a_demo_widget_appends_to_the_event_log() {
    let config = GalleryApp::config();
    let mut driver = driver_with_shell(GalleryApp::new(), config, 100, 32);

    let icon = group_icon(overlays_group_index());
    let (x, y) = driver.find(icon).expect("Overlays icon should paint");
    driver.click(x, y);
    let (x, y) = driver
        .find("Toast")
        .expect("Toast sidebar row should paint");
    driver.click(x, y);

    let before = driver.screen();
    let before_key_lines = before.matches("KeyPressed").count();

    driver.type_char('1');

    let after = driver.screen();
    let after_key_lines = after.matches("KeyPressed").count();
    assert!(
        after_key_lines > before_key_lines,
        "typing '1' should append a KeyPressed entry to the event log:\n{after}"
    );
    assert!(
        after.contains("Info notification"),
        "typing '1' should forward to ToastDemo and add an Info toast:\n{after}"
    );
}

/// Table-driven acceptance test over the whole registry: every registered
/// demo renders without panicking, and its Code region
/// (the text between its `// gallery:begin` / `// gallery:end` markers)
/// is non-empty. Port issues get this coverage for free by appending to
/// `registry::registry`.
#[test]
fn every_registered_demo_renders_and_has_a_non_empty_code_region() {
    for demo in registry() {
        let source = demo.source();
        assert!(
            !source.trim().is_empty(),
            "{}'s Code region (gallery:begin/gallery:end) must not be empty",
            demo.name()
        );

        // Driving each demo through the real gallery shell (rather than
        // calling `Demo::render` directly) exercises the exact code path
        // a user reaches: group switch → sidebar select → render. Group
        // navigation clicks the activity-bar item's real zone bounds
        // (via `ConformanceDriver::inventory()`), not a scraped icon
        // glyph — `group_icon`'s plain digits are only collision-free
        // with a group's own rendered content by accident of timing (see
        // that constant's doc), and this test is exactly the one that
        // will one day register a demo into `GROUPS[0]` and make a
        // digit-scraping click land on the wrong element.
        let config = GalleryApp::config();
        let mut driver = driver_with_shell(GalleryApp::new(), config, 100, 32);
        let group_idx = GROUPS
            .iter()
            .position(|g| *g == demo.group())
            .unwrap_or_else(|| {
                panic!(
                    "{}'s group {:?} is not one of GROUPS",
                    demo.name(),
                    demo.group()
                )
            });
        click_group_zone(&mut driver, group_idx);
        if let Some((x, y)) = driver.find(demo.name()) {
            driver.click(x, y);
        }
        // Render already happened inside `click` (it redraws on every
        // dispatched event) — reaching here without a panic is the
        // assertion. `screen()` forces one more render for good measure.
        let _ = driver.screen();

        // The Data tab must also render without panicking.
        if let Some((x, y)) = driver.find("Data") {
            driver.click(x, y);
        }
        let _ = driver.screen();
    }
}
