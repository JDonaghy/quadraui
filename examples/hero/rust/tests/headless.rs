//! Drives the hero counter headless on the TUI backend: click `+`, `-`, `+` by
//! locating each button's label on screen, then assert the
//! rendered frame. The Python and Node runners
//! (`examples/hero/{python,node}/run_headless.*`) drive the identical script.

#[allow(dead_code)]
#[path = "../src/main.rs"]
mod hero;

use quadraui::tui::testing::TuiDriver;

fn click_label(driver: &mut TuiDriver<hero::Counter>, label: &str) {
    let (x, y) = driver
        .find(label)
        .unwrap_or_else(|| panic!("{label:?} not on screen:\n{}", driver.screen()));
    driver.click(x, y);
}

#[test]
fn plus_minus_plus_counts_to_one() {
    let mut driver = TuiDriver::new(hero::Counter::default(), 30, 5);
    driver.render();
    assert!(driver.screen_contains("Count: 0"), "{}", driver.screen());
    // `+ - +`, not `+ + -`: two clicks on one spot this fast fold into a
    // DoubleClick, which a Button does not treat as a second activation
    // (see examples/hero/README.md, "Gaps").
    for label in ["+", "-", "+"] {
        click_label(&mut driver, label);
    }
    let screen = driver.screen();
    println!("{screen}");
    assert!(screen.contains("Count: 1"), "{screen}");
    assert_eq!(driver.app().count, 1);
}
