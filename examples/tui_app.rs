//! Minimal `AppLogic` + `quadraui::tui::run` example.
//!
//! Demonstrates the runner-crate API by building a tiny TUI app that
//! shows a status bar with a key counter and exits on `q`. The app
//! is ~70 lines — everything else (terminal init, frame loop, event
//! drain, tear-down) lives in `quadraui::tui::run`.
//!
//! Run with:
//!
//! ```sh
//! cargo run --example tui_app --features tui
//! ```

use quadraui::{
    AppLogic, Backend, Color, Key, NamedKey, Reaction, Rect, StatusBar, StatusBarSegment,
    StatusSegmentMeasure, Theme, UiEvent, WidgetId,
};

struct MiniApp {
    keys_pressed: u32,
    last_key: Option<Key>,
}

impl MiniApp {
    fn new() -> Self {
        Self {
            keys_pressed: 0,
            last_key: None,
        }
    }

    fn status_bar(&self) -> StatusBar {
        let count = format!(" keys: {} ", self.keys_pressed);
        let last = match &self.last_key {
            Some(Key::Char(c)) => format!(" last: {c} "),
            Some(Key::Named(n)) => format!(" last: {n:?} "),
            None => " press any key — q to quit ".to_string(),
        };
        StatusBar {
            id: WidgetId::new("status:bar"),
            left_segments: vec![StatusBarSegment {
                text: " quadraui::tui::run demo ".into(),
                fg: Color::rgb(255, 255, 255),
                bg: Color::rgb(40, 80, 120),
                bold: true,
                action_id: None,
            }],
            right_segments: vec![
                StatusBarSegment {
                    text: last,
                    fg: Color::rgb(220, 220, 220),
                    bg: Color::rgb(40, 80, 120),
                    bold: false,
                    action_id: None,
                },
                StatusBarSegment {
                    text: count,
                    fg: Color::rgb(255, 255, 255),
                    bg: Color::rgb(40, 80, 120),
                    bold: false,
                    action_id: None,
                },
            ],
        }
    }
}

impl AppLogic for MiniApp {
    fn render(&self, backend: &mut dyn Backend) {
        // Build the status bar's layout for measurement (1-cell-per-char
        // measurer for TUI). The trait method internally re-runs layout
        // — the bar struct alone is what it needs.
        let bar = self.status_bar();
        let viewport = backend.viewport();
        // Run a layout call so we can pass a precomputed layout to
        // the rasteriser shape that takes one. The trait's
        // `draw_status_bar` recomputes internally with `MIN_GAP_CELLS = 2`.
        let _ = bar.layout(viewport.width, 1.0, 2.0, |seg| {
            StatusSegmentMeasure::new(seg.text.chars().count() as f32)
        });
        // Clear the screen by painting the bar across the bottom row.
        let rect = Rect::new(0.0, viewport.height - 1.0, viewport.width, 1.0);
        let _ = backend.draw_status_bar(rect, &bar);
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        match event {
            UiEvent::KeyPressed { key, .. } => {
                if matches!(key, Key::Char('q')) || matches!(key, Key::Named(NamedKey::Escape)) {
                    return Reaction::Exit;
                }
                self.keys_pressed += 1;
                self.last_key = Some(key);
                Reaction::Redraw
            }
            UiEvent::WindowResized { .. } => Reaction::Redraw,
            _ => Reaction::Continue,
        }
    }
}

fn main() -> std::io::Result<()> {
    let app = MiniApp::new();
    quadraui::tui::run(app)?;
    // Hint: the runner sets a sensible default theme (`Theme::default`).
    // Apps that customise call `backend.set_current_theme(...)` inside
    // `render()` per frame. This example doesn't override.
    let _ = Theme::default();
    Ok(())
}
