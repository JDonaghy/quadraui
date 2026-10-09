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
use quadraui::{testing::ConformanceDriver, AppLogic, Backend};

use quadraui_gallery::app::{group_icon, group_panel_id, GROUPS};
use quadraui_gallery::registry::registry;
use quadraui_gallery::GalleryApp;

/// Index of `"Overlays"` in [`GROUPS`] — where `ToastDemo` lives.
/// Resolved from the list rather than hardcoded as `3`, so a future
/// reordering of `GROUPS` doesn't silently break this test's
/// activity-bar click.
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

/// Click the sidebar row at `row_in_group` (0-based, in registry order,
/// counting only demos in the currently-active group) by computing its
/// bounds from the registered `app-shell:sidebar-content` zone — not a
/// `driver.find(name)` scrape, which a name that's a substring of
/// another demo's name (e.g. "Panel" inside "Bottom Panel") can resolve
/// to the wrong row. Mirrors `GalleryApp::render_content`'s own row
/// layout: one `line_height()`-tall row per demo, top to bottom.
///
/// Panics if the row falls outside the sidebar's painted area.
/// `render_content` stops drawing rows once they would overflow
/// (`if y + lh > sb.y + sb.height { break }`), so a group with more
/// demos than the sidebar has lines would otherwise make this helper a
/// silent no-op: the click would land on blank space, the selection
/// would never change, and a caller whose only check is "didn't panic"
/// would keep passing while exercising the empty-selection screen.
fn click_sidebar_row<A: AppLogic>(driver: &mut TuiDriver<A>, row_in_group: usize) {
    let id = quadraui::WidgetId::new("app-shell:sidebar-content");
    let sb = driver
        .inventory()
        .zones()
        .iter()
        .find(|z| z.id == id)
        .expect("no registered zone for the sidebar content area")
        .bounds;
    let lh = driver.backend().line_height();
    let y = sb.y + row_in_group as f32 * lh;
    assert!(
        y + lh <= sb.y + sb.height,
        "sidebar row {row_in_group} is past the painted sidebar area \
         (rows are {lh} tall, content area is {} tall, fits {} rows) — \
         grow the driver's terminal height or paginate the sidebar",
        sb.height,
        (sb.height / lh) as usize
    );
    driver.click(sb.x + sb.width / 2.0, y + lh / 2.0);
}

/// Click the Overlays activity-bar icon, click the Toast sidebar row,
/// then the Code tab — asserting the demo shows, and the Code tab shows
/// text from `ToastDemo`'s own `// gallery:begin` / `// gallery:end`
/// region.
#[test]
fn navigating_group_then_demo_shows_it_and_its_code_region() {
    let config = GalleryApp::config();
    let mut driver = driver_with_shell(GalleryApp::new(), config, 100, 32);

    // "Containers" has no demos registered yet (Input & forms and
    // Layout & chrome both do now) — navigate there first to exercise
    // the empty-group placeholder this test used to get for free from
    // the default (Content) group.
    let containers_idx = GROUPS
        .iter()
        .position(|g| *g == "Containers")
        .expect("GROUPS must include \"Containers\"");
    click_group_zone(&mut driver, containers_idx);
    assert!(
        driver.screen_contains("Select a demo") || driver.screen_contains("no demos ported yet"),
        "no demo should be selected in an empty group:\n{}",
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
    // Rows within a group paint in registry order (`GalleryApp::
    // demos_in_active_group` is a plain filter, order-preserving), so
    // this running per-group counter reproduces the exact 0-based row
    // index `click_sidebar_row` needs without a second `registry()` call.
    let mut row_in_group: std::collections::HashMap<&'static str, usize> =
        std::collections::HashMap::new();
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
        // digit-scraping click land on the wrong element. The sidebar
        // row is selected the same way — by its position in the sidebar
        // content area, not a `find(name)` scrape, since one demo's name
        // can be a substring of another's (e.g. "Panel" inside "Bottom
        // Panel").
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
        // `GalleryApp::new()` already starts on `GROUPS[0]` — clicking
        // that *same* activity-bar icon again would hit `AppShell`'s
        // "click the already-active icon to hide the sidebar" toggle
        // (quadraui's `app_shell.rs`), collapsing the very sidebar the
        // next line needs to click into. Only click the icon when this
        // demo's group isn't already the default active one.
        if group_idx != 0 {
            click_group_zone(&mut driver, group_idx);
        }
        let row = row_in_group.entry(demo.group()).or_insert(0);
        click_sidebar_row(&mut driver, *row);
        *row += 1;
        // Render already happened inside `click` (it redraws on every
        // dispatched event), but "didn't panic" alone would also pass if
        // the click missed its row and left nothing selected. Assert the
        // selection actually took: with `selected == None`,
        // `render_content` paints the "Select a demo from the sidebar"
        // placeholder instead of ever calling `Demo::render`, so its
        // absence is what proves this demo was driven through the shell.
        let demo_screen = driver.screen();
        assert!(
            !demo_screen.contains("Select a demo from the sidebar"),
            "clicking {}'s sidebar row should select it, but the main pane \
             still shows the empty-selection placeholder:\n{demo_screen}",
            demo.name()
        );

        // The Code tab must show this demo's own gallery region — a
        // second, demo-specific check that the right row got selected
        // (not merely *some* row). Compares against the region's first
        // non-blank line, trimmed, since the pane wraps and indents.
        if let Some((x, y)) = driver.find("Code") {
            driver.click(x, y);
            let first_line = source
                .lines()
                .map(str::trim)
                .find(|l| !l.is_empty())
                .expect("a non-empty Code region must have a non-blank line");
            let code_screen = driver.screen();
            assert!(
                code_screen.contains(first_line),
                "Code tab for {} should show the first line of its own \
                 gallery region ({first_line:?}):\n{code_screen}",
                demo.name()
            );
        }

        // The Data tab must also render without panicking.
        if let Some((x, y)) = driver.find("Data") {
            driver.click(x, y);
        }
        let _ = driver.screen();
    }
}

// ── Layout & chrome demos — targeted interaction tests ────────────────────

/// Index of `"Chrome"` in [`GROUPS`] — where every Layout & chrome demo
/// lives.
fn chrome_group_index() -> usize {
    GROUPS
        .iter()
        .position(|g| *g == "Chrome")
        .expect("GROUPS must include \"Chrome\"")
}

/// Navigate to the Chrome group and select the demo named `name` from
/// the sidebar.
fn select_chrome_demo<A: AppLogic>(driver: &mut TuiDriver<A>, name: &str) {
    click_group_zone(driver, chrome_group_index());
    let (x, y) = driver
        .find(name)
        .unwrap_or_else(|| panic!("{name} sidebar row should paint:\n{}", driver.screen()));
    driver.click(x, y);
}

/// Clicking a Toolbar action button (Filter) toggles it and the hint
/// line reflects the new state.
#[test]
fn toolbar_filter_click_updates_the_hint_line() {
    let mut driver = driver_with_shell(GalleryApp::new(), GalleryApp::config(), 100, 32);
    select_chrome_demo(&mut driver, "Toolbar");
    assert!(driver.screen_contains("Tab to focus"));

    let (x, y) = driver
        .find("Filter")
        .unwrap_or_else(|| panic!("Filter button should paint:\n{}", driver.screen()));
    // A full press+release cycle, not a bare `click()` (`MouseDown`
    // only) — `ToolbarDemo::dispatch` fires on `MouseUp`, matching the
    // release-on-the-same-button click contract every quadraui toolbar
    // follows (see `quadraui/tests/tui_example_driver.rs`'s
    // `toolbar_press_release_on_filter_toggles_is_active_state`).
    driver.mouse_down(x, y);
    driver.mouse_up(x, y);

    assert!(
        driver.screen_contains("Filter on"),
        "clicking Filter should flip it on and update the hint line:\n{}",
        driver.screen()
    );
}

/// Clicking the second Activity Bar icon activates it — visible on the
/// Data tab as the `last_action` field recording the newly-activated item.
/// Before the click `last_action` is `"ready"`, so this cannot pass without
/// the click actually driving `ActivityBarDemo::activate`.
#[test]
fn activity_bar_click_activates_the_item() {
    // Tall enough that the Data tab's full `ActivityBar` JSON (3 items,
    // ~30 lines) fits without the Demo-tab text clipping cutting off the
    // activated item before it ever paints.
    let mut driver = driver_with_shell(GalleryApp::new(), GalleryApp::config(), 100, 60);
    select_chrome_demo(&mut driver, "Activity Bar");

    let (dx, dy) = driver.find("Data").expect("Data tab label should paint");
    driver.click(dx, dy);
    // Pin the key as well as the value: a bare `"ready"` would match any
    // field in the payload that happens to hold that string.
    assert!(
        driver.screen_contains("\"last_action\": \"ready\""),
        "last_action should start as \"ready\" before any click:\n{}",
        driver.screen()
    );

    let (gx, gy) = driver.find("Demo").expect("Demo tab label should paint");
    driver.click(gx, gy);
    let (x, y) = driver.find("G").unwrap_or_else(|| {
        panic!(
            "'G' (Source Control icon) should paint:\n{}",
            driver.screen()
        )
    });
    driver.click(x, y);

    let (dx, dy) = driver.find("Data").expect("Data tab label should paint");
    driver.click(dx, dy);
    let screen = driver.screen();
    assert!(
        screen.contains("activated: Source Control"),
        "Data tab should show the activated item in last_action:\n{screen}"
    );
}

/// Clicking the "lib.rs" tab on the Tab Bar "Chrome frame" variant
/// activates it — visible on the Data tab as `chrome_action` recording the
/// activation. Before the click `chrome_action` is `"ready"`, so this
/// cannot pass without the click actually driving `TabBarDemo::activate_chrome_tab`.
#[test]
fn tab_bar_chrome_click_activates_the_clicked_tab() {
    let mut driver = driver_with_shell(GalleryApp::new(), GalleryApp::config(), 100, 32);
    select_chrome_demo(&mut driver, "Tab Bar");
    assert!(driver.screen_contains("main.rs"));

    let (dx, dy) = driver.find("Data").expect("Data tab label should paint");
    driver.click(dx, dy);
    // Pin the key as well as the value, as above.
    assert!(
        driver.screen_contains("\"chrome_action\": \"ready\""),
        "chrome_action should start as \"ready\" before any click:\n{}",
        driver.screen()
    );

    let (cx, cy) = driver.find("Demo").expect("Demo tab label should paint");
    driver.click(cx, cy);
    let (x, y) = driver
        .find("lib.rs")
        .unwrap_or_else(|| panic!("lib.rs tab should paint:\n{}", driver.screen()));
    driver.click(x, y);

    let (dx, dy) = driver.find("Data").expect("Data tab label should paint");
    driver.click(dx, dy);
    let screen = driver.screen();
    assert!(
        screen.contains("activated lib.rs"),
        "Data tab should show lib.rs as the activated tab in chrome_action:\n{screen}"
    );
}

/// Clicking a Window Control button updates the status line with the
/// `ServiceResult` outcome.
#[test]
fn window_control_click_shows_a_result_in_the_status_line() {
    let mut driver = driver_with_shell(GalleryApp::new(), GalleryApp::config(), 100, 32);
    select_chrome_demo(&mut driver, "Window Control");

    let (x, y) = driver
        .find("Set Title")
        .unwrap_or_else(|| panic!("Set Title button should paint:\n{}", driver.screen()));
    driver.click(x, y);

    assert!(
        driver.screen_contains("set_title"),
        "clicking Set Title should show the set_title ServiceResult:\n{}",
        driver.screen()
    );
}

/// Pressing `o` on the Workspace demo opens the next backlog document,
/// growing the tab strip and updating the event log.
#[test]
fn workspace_open_key_adds_a_document() {
    let mut driver = driver_with_shell(GalleryApp::new(), GalleryApp::config(), 100, 32);
    select_chrome_demo(&mut driver, "Workspace");
    assert!(driver.screen_contains("alpha"));

    driver.type_char('o');

    assert!(
        driver.screen_contains("delta-doc") || driver.screen_contains("opened"),
        "pressing 'o' should open the next backlog document:\n{}",
        driver.screen()
    );
}

/// `click_sidebar_row` must refuse a row the sidebar never painted
/// rather than clicking blank space. Guards the guard: without the
/// bounds assertion this returns normally, the selection stays empty,
/// and every caller whose check is "didn't panic" keeps passing while
/// driving nothing.
#[test]
#[should_panic(expected = "past the painted sidebar area")]
fn click_sidebar_row_panics_on_a_row_the_sidebar_never_painted() {
    let mut driver = driver_with_shell(GalleryApp::new(), GalleryApp::config(), 100, 32);
    click_group_zone(&mut driver, chrome_group_index());
    click_sidebar_row(&mut driver, 9_999);
}

// ── Input & forms demos — targeted interaction tests ──────────────────────

/// Select the demo named `name` from the sidebar. Unlike
/// `select_chrome_demo`, this does **not** click the Content group's own
/// activity-bar icon first: `GalleryApp::new()` already starts on
/// `GROUPS[0]` ("Content"), and `AppShell` toggles (hides) the sidebar
/// when the *already-active* icon is clicked again — clicking it here
/// would collapse the sidebar these tests need to click into, rather
/// than re-selecting it.
fn select_content_demo<A: AppLogic>(driver: &mut TuiDriver<A>, name: &str) {
    let (x, y) = driver
        .find(name)
        .unwrap_or_else(|| panic!("{name} sidebar row should paint:\n{}", driver.screen()));
    driver.click(x, y);
}

/// Typing a character into the "Empty" `TextInput` variant inserts it —
/// proves the click→type→render round trip, not just that the primitive
/// paints a placeholder.
#[test]
fn text_input_typing_inserts_the_character() {
    let mut driver = driver_with_shell(GalleryApp::new(), GalleryApp::config(), 100, 32);
    select_content_demo(&mut driver, "Text Input");
    assert!(driver.screen_contains("Type something"));

    driver.type_char('~');

    assert!(
        driver.screen_contains("~"),
        "typing '~' should insert it into the empty buffer:\n{}",
        driver.screen()
    );
}

/// Pressing Tab moves the `Focus` demo's status-bar echo from the left
/// list to the right one.
#[test]
fn focus_tab_key_moves_the_focus_echo() {
    let mut driver = driver_with_shell(GalleryApp::new(), GalleryApp::config(), 100, 32);
    select_content_demo(&mut driver, "Focus");
    assert!(driver.screen_contains("focus: gallery:focus:left"));

    driver.press_named(quadraui::NamedKey::Tab);

    assert!(
        driver.screen_contains("focus: gallery:focus:right"),
        "Tab should move the focus echo from left to right:\n{}",
        driver.screen()
    );
}

/// Clicking the Find & Replace panel's chevron flips `show_replace`,
/// visible on the Data tab.
#[test]
fn find_replace_chevron_click_toggles_show_replace() {
    let mut driver = driver_with_shell(GalleryApp::new(), GalleryApp::config(), 100, 32);
    select_content_demo(&mut driver, "Find & Replace");

    // `FindReplaceDemo::new` starts with `show_replace: true`, so the
    // chevron paints collapsed (▼).
    let (x, y) = driver
        .find("▼")
        .unwrap_or_else(|| panic!("chevron glyph should paint:\n{}", driver.screen()));
    driver.click(x, y);

    let (dx, dy) = driver.find("Data").expect("Data tab label should paint");
    driver.click(dx, dy);
    assert!(
        driver.screen_contains("\"show_replace\": false"),
        "clicking the chevron should flip show_replace to false:\n{}",
        driver.screen()
    );
}

/// Switching the dual-mode palette to Input mode, typing a branch name
/// and pressing Enter creates and switches to it.
#[test]
fn palette_text_confirmed_creates_a_branch() {
    let mut driver = driver_with_shell(GalleryApp::new(), GalleryApp::config(), 100, 32);
    select_content_demo(&mut driver, "Command Palette");
    assert!(driver.screen_contains("main"));

    driver.press_named(quadraui::NamedKey::Tab);
    for c in "my-new-branch".chars() {
        driver.type_char(c);
    }
    driver.press_named(quadraui::NamedKey::Enter);

    assert!(
        driver.screen_contains("my-new-branch"),
        "confirming a typed branch name should switch to it:\n{}",
        driver.screen()
    );
}

/// Pressing Escape on the "Choose folder" variant cancels the picker —
/// visible on the Data tab's `confirmed` field.
#[test]
fn file_picker_escape_cancels_the_folder_picker() {
    let mut driver = driver_with_shell(GalleryApp::new(), GalleryApp::config(), 100, 32);
    select_content_demo(&mut driver, "File Picker");

    let (x, y) = driver
        .find("Choose folder")
        .expect("variant picker should list \"Choose folder\"");
    driver.click(x, y);

    driver.press_named(quadraui::NamedKey::Escape);

    let (dx, dy) = driver.find("Data").expect("Data tab label should paint");
    driver.click(dx, dy);
    assert!(
        driver.screen_contains("Dismissed"),
        "Escape should cancel the folder picker:\n{}",
        driver.screen()
    );
}

/// Switching the "Caret Shape" demo's variant calls
/// `Backend::set_caret_shape` exactly once per actual shape change — the
/// counter in the status line only advances on a real change, never on
/// every repaint.
#[test]
fn caret_shape_variant_switch_calls_set_caret_shape_once() {
    let mut driver = driver_with_shell(GalleryApp::new(), GalleryApp::config(), 100, 32);
    select_content_demo(&mut driver, "Caret Shape");
    assert!(driver.screen_contains("calls: 1"));

    // Re-rendering the same (default) variant must not bump the count —
    // force an unrelated redraw via a harmless key.
    driver.press_named(quadraui::NamedKey::Left);
    assert!(driver.screen_contains("calls: 1"));

    let (x, y) = driver
        .find("Insert (Bar)")
        .expect("variant picker should list \"Insert (Bar)\"");
    driver.click(x, y);
    assert!(
        driver.screen_contains("calls: 2"),
        "switching to a different shape should bump the call count:\n{}",
        driver.screen()
    );
}

/// Dragging across the "Text Selection" demo's content, then Ctrl-C,
/// copies the selected lines — the runner-owned selection pipeline
/// reached through nothing but `register_text_region`.
#[test]
fn text_selection_drag_then_ctrl_c_copies() {
    let mut driver = driver_with_shell(GalleryApp::new(), GalleryApp::config(), 100, 32);
    select_content_demo(&mut driver, "Text Selection");
    assert!(driver.screen_contains("quick brown fox"));

    let region = driver
        .find_bounds("quick brown fox")
        .expect("first content row should paint");
    let y = region.y + region.height / 2.0;
    driver.drag(region.x, y, region.x + region.width, y);
    driver.ctrl_char('c');

    assert!(
        driver.screen_contains("Copied:"),
        "Ctrl-C after a drag-select should copy and show a preview:\n{}",
        driver.screen()
    );
}
