//! Runs the parity app (`src/main.rs`) headless on the TUI backend through
//! the input script given on the command line, and prints one JSON trace:
//! after the first paint and after every step, the rendered cell grid
//! (glyph, fg and bg of every cell) and the app updates that step fired.
//! The Python and Node tracers print the same shape;
//! `scripts/parity_check.py` diffs them.
//!
//!     cargo run -p quadraui-parity --features tui --bin parity-trace -- examples/parity/script.json

#[path = "../main.rs"]
mod todo;

use quadraui::prelude::*;
use quadraui::tui::testing::TuiDriver;
use serde_json::{json, Value};

const WIDTH: u16 = 40;
const HEIGHT: u16 = 12;

/// The colour as hex, read the way the bindings' `cell_colors`/`cellColors`
/// read it: a terminal-default (non-RGB) colour comes back as black there,
/// so it does here too.
fn hex(color: quadraui::tui::testing::Color) -> String {
    let (r, g, b) = match color {
        quadraui::tui::testing::Color::Rgb(r, g, b) => (r, g, b),
        _ => (0, 0, 0),
    };
    format!("{r:02x}{g:02x}{b:02x}")
}

fn frame(driver: &mut TuiDriver<todo::Todo>, input: Value) -> Value {
    driver.render();
    let screen = driver.screen();
    let rows: Vec<&str> = screen.trim_end_matches('\n').split('\n').collect();
    let styles: Vec<Vec<String>> = (0..HEIGHT)
        .map(|y| {
            driver
                .styled_row(y)
                .into_iter()
                .map(|(_, s)| format!("{}/{}", hex(s.fg), hex(s.bg)))
                .collect()
        })
        .collect();
    let events = std::mem::take(&mut driver.app_mut().log);
    json!({ "input": input, "rows": rows, "styles": styles, "events": events })
}

fn key(name: &str) -> Key {
    match name {
        "Enter" => Key::Named(NamedKey::Enter),
        "Escape" => Key::Named(NamedKey::Escape),
        "Tab" => Key::Named(NamedKey::Tab),
        "Up" => Key::Named(NamedKey::Up),
        "Down" => Key::Named(NamedKey::Down),
        "Left" => Key::Named(NamedKey::Left),
        "Right" => Key::Named(NamedKey::Right),
        "Backspace" => Key::Named(NamedKey::Backspace),
        c if c.chars().count() == 1 => Key::Char(c.chars().next().unwrap()),
        other => panic!("unknown key {other:?}"),
    }
}

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: parity-trace SCRIPT.json");
    let script: Vec<Value> = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let mut driver = TuiDriver::new(todo::Todo::default(), WIDTH, HEIGHT);
    let mut trace = vec![frame(&mut driver, Value::Null)];
    for step in script {
        if let Some(at) = step.get("click") {
            driver.click(
                at[0].as_f64().unwrap() as f32,
                at[1].as_f64().unwrap() as f32,
            );
        } else if let Some(text) = step.get("type") {
            for c in text.as_str().unwrap().chars() {
                driver.press(Key::Char(c));
            }
        } else if let Some(name) = step.get("key") {
            driver.press(key(name.as_str().unwrap()));
        } else {
            panic!("unknown step {step}");
        }
        trace.push(frame(&mut driver, step));
    }
    println!("{}", Value::Array(trace));
}
