//! Pins a [`TuiDriver`] screen — every cell's glyph plus its style, run-length
//! encoded per row — against a checked-in fixture, so a test can prove a
//! refactor leaves the painted output byte-identical.
//!
//! Each pinned app keeps one fixture file holding every step it records, in
//! order. `SCREEN_PIN_WRITE=1` rewrites the fixtures from the current output
//! instead of comparing.

use std::fmt::Write as _;
use std::path::PathBuf;

use quadraui::tui::testing::TuiDriver;
use quadraui::AppLogic;

/// The steps one app records, written to or compared against
/// `<fixture_dir>/<name>.txt` by [`Self::finish`].
pub struct ScreenPins {
    path: PathBuf,
    recorded: String,
}

impl ScreenPins {
    /// A recorder for the fixture `<fixture_dir>/<name>.txt`.
    pub fn new(fixture_dir: &str, name: &str) -> Self {
        Self {
            path: PathBuf::from(fixture_dir).join(format!("{name}.txt")),
            recorded: String::new(),
        }
    }

    /// Record the driver's current screen under `step`.
    pub fn step<A: AppLogic>(&mut self, step: &str, driver: &TuiDriver<A>) {
        let _ = writeln!(self.recorded, "=== {step}");
        self.recorded.push_str(&dump(driver));
    }

    /// Compare everything recorded against the fixture, or rewrite it when
    /// `SCREEN_PIN_WRITE=1`.
    pub fn finish(self) {
        if std::env::var("SCREEN_PIN_WRITE").as_deref() == Ok("1") {
            if let Some(dir) = self.path.parent() {
                std::fs::create_dir_all(dir).expect("create the fixture directory");
            }
            std::fs::write(&self.path, &self.recorded).expect("write the fixture");
            return;
        }
        let expected = std::fs::read_to_string(&self.path).unwrap_or_else(|e| {
            panic!(
                "missing screen pin {} ({e}); run with SCREEN_PIN_WRITE=1 to record it",
                self.path.display()
            )
        });
        if expected != self.recorded {
            let first = expected
                .lines()
                .zip(self.recorded.lines())
                .position(|(a, b)| a != b)
                .unwrap_or_else(|| expected.lines().count().min(self.recorded.lines().count()));
            let context = |s: &str| {
                s.lines()
                    .skip(first.saturating_sub(3))
                    .take(8)
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            panic!(
                "painted output differs from {} at line {}:\n--- expected\n{}\n--- actual\n{}",
                self.path.display(),
                first + 1,
                context(&expected),
                context(&self.recorded)
            );
        }
    }
}

/// The driver's screen: the glyph rows, then each row's style runs.
fn dump<A: AppLogic>(driver: &TuiDriver<A>) -> String {
    let screen = driver.screen();
    let mut out = String::new();
    for line in screen.lines() {
        let _ = writeln!(out, "|{line}|");
    }
    for (y, _) in screen.lines().enumerate() {
        let row = driver.styled_row(y as u16);
        let mut runs: Vec<(usize, usize, String)> = Vec::new();
        for (x, (_, style)) in row.iter().enumerate() {
            let key = format!("{style:?}");
            match runs.last_mut() {
                Some((_, end, last)) if *last == key => *end = x,
                _ => runs.push((x, x, key)),
            }
        }
        let _ = write!(out, "{y}:");
        for (start, end, key) in runs {
            let _ = write!(out, " {start}-{end} {key};");
        }
        out.push('\n');
    }
    out
}
