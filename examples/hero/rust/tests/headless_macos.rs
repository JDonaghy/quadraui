//! The `tests/headless.rs` script, on the macOS backend: the same `Counter`
//! painted by Core Graphics + Core Text into an off-screen bitmap, clicked
//! through the same routing, in pixels instead of terminal cells.
#![cfg(target_os = "macos")]

#[allow(dead_code)]
#[path = "../src/main.rs"]
mod hero;

use quadraui::macos::testing::MacDriver;

#[test]
fn plus_minus_plus_counts_to_one_in_a_native_surface() {
    let mut driver = MacDriver::new(hero::Counter::default(), 400, 200);
    driver.render();
    assert!(driver.screen_contains("Count: 0"));
    for label in ["+", "-", "+"] {
        let (x, y) = driver
            .find(label)
            .unwrap_or_else(|| panic!("{label:?} not painted"));
        driver.click(x, y);
    }
    assert!(driver.screen_contains("Count: 1"));
    assert_eq!(driver.app().count, 1);
}
