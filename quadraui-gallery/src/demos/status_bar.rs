//! `StatusBar` demo — adapted from
//! `quadraui/examples/common/status_bar_priority_demo.rs`.
//!
//! Shows the documented segment-priority convention: `right_segments`
//! are least-important first, with the cursor-position segment *last*
//! so it survives priority-drop truncation. Toggling "dirty" grows the
//! left segment enough, at the demo's width, to force a low-priority
//! right segment to drop while the cursor-position segment keeps
//! painting.

use quadraui::{
    Backend, BackendCaps, Color, InteractionState, Reaction, Rect, StatusBar, StatusBarSegment,
    Style, UiEvent, WidgetId,
};

use crate::demo::{extract_region, Demo};

const SOURCE: &str = include_str!("status_bar.rs");

/// `Style`'s chrome-metric `token` (`tab_height`, `status_bar_height`,
/// `toolbar_height`, …), floored at the chrome line height on a pixel
/// backend — never shrunk below it, so a chrome font taller than the
/// design token isn't clipped — or the plain chrome line height on TUI's
/// fixed cell grid, where every `Style` chrome metric is a documented
/// no-op. Mirrors `AppShell`'s own token-vs-line-height carve
/// (`compose::shell_layout::compute_layout`'s `header_h`/`status_h`).
fn chrome_strip_height(backend: &dyn Backend, token: impl Fn(&Style) -> f32) -> f32 {
    let lh = backend.chrome_line_height();
    if lh <= 1.0 {
        backend.snap_height(lh)
    } else {
        backend.snap_height(lh.max(token(&backend.style())))
    }
}

// gallery:begin
/// Build the status bar for either state: `dirty == true` pads the mode
/// segment wide enough, at this demo's width, to force a low-priority
/// right segment to drop under priority-drop truncation.
fn status_bar(dirty: bool) -> StatusBar {
    let mode_text = if dirty {
        format!("NORMAL{}", " [+]".repeat(60))
    } else {
        "NORMAL".to_string()
    };
    let plain = |text: &str| StatusBarSegment {
        text: text.to_string(),
        fg: Color::rgb(220, 220, 220),
        bg: Color::rgb(40, 80, 120),
        bold: false,
        action_id: None,
    };
    StatusBar {
        id: WidgetId::new("gallery:status-bar"),
        left_segments: vec![StatusBarSegment {
            text: mode_text,
            fg: Color::rgb(255, 255, 255),
            bg: Color::rgb(40, 80, 120),
            bold: true,
            action_id: None,
        }],
        // Least-important first, cursor-position segment last — the
        // documented convention that keeps it visible under
        // priority-drop.
        right_segments: vec![
            plain("Spaces: 4"),
            plain("UTF-8"),
            plain("LF"),
            plain("Ln 1, Col 1"),
        ],
    }
}

pub struct StatusBarDemo;

impl StatusBarDemo {
    pub fn new() -> Self {
        Self
    }

    /// `area`'s top strip, sized off `Style::status_bar_height` on a pixel
    /// backend (floored at the chrome line height so a chrome font taller
    /// than the token isn't clipped) and off the plain chrome line height
    /// on TUI's fixed cell grid — mirrors `AppShell`'s own status-bar
    /// sizing (`compose/shell_layout.rs`'s `status_h`).
    fn rect(area: Rect, backend: &dyn Backend) -> Rect {
        let h = chrome_strip_height(backend, |s| s.status_bar_height);
        Rect::new(area.x, area.y, area.width, h)
    }
}

impl Default for StatusBarDemo {
    fn default() -> Self {
        Self::new()
    }
}

impl Demo for StatusBarDemo {
    fn name(&self) -> &'static str {
        "Status Bar"
    }

    fn group(&self) -> &'static str {
        "Chrome"
    }

    fn variants(&self) -> &'static [&'static str] {
        &["Clean", "Dirty (priority drop)"]
    }

    fn render(&self, variant: usize, backend: &mut dyn Backend, area: Rect) {
        let rect = Self::rect(area, backend);
        let bar = status_bar(variant == 1);
        let _ = backend.draw_status_bar_interactive(rect, &bar, &InteractionState::new());
    }

    fn handle(
        &mut self,
        _variant: usize,
        _event: &UiEvent,
        _backend: &mut dyn Backend,
        _area: Rect,
    ) -> Reaction {
        Reaction::Continue
    }

    fn source(&self) -> &'static str {
        extract_region(SOURCE)
    }

    fn data(&self, variant: usize) -> serde_json::Value {
        serde_json::to_value(status_bar(variant == 1)).unwrap_or(serde_json::Value::Null)
    }

    fn caps_note(&self, _variant: usize, _caps: &BackendCaps) -> Option<String> {
        None
    }
}
// gallery:end

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dirty_variant_grows_the_left_segment() {
        let clean = status_bar(false);
        let dirty = status_bar(true);
        assert!(dirty.left_segments[0].text.len() > clean.left_segments[0].text.len());
    }

    /// On a pixel backend (a GTK-shaped chrome line height, simulated via
    /// `RecordingBackend`), the painted status-bar rect is exactly
    /// `Style::status_bar_height` tall — not the raw chrome line height,
    /// the bug this test guards against regressing.
    #[test]
    fn rect_height_matches_status_bar_height_token_on_a_pixel_backend() {
        let backend = quadraui::testing::RecordingBackend {
            chrome_line_height: Some(20.0),
            ..quadraui::testing::RecordingBackend::new()
        };
        let area = Rect::new(0.0, 0.0, 200.0, 100.0);
        let rect = StatusBarDemo::rect(area, &backend);
        assert_eq!(rect.height, Style::default().status_bar_height);
    }
}

#[cfg(all(test, feature = "tui"))]
mod tui_tests {
    use super::*;

    /// TUI's fixed cell grid keeps its one-row degrade —
    /// `Style::status_bar_height` is a documented no-op there.
    #[test]
    fn rect_height_stays_one_chrome_line_on_tui() {
        let backend = quadraui::tui::TuiBackend::new();
        let area = Rect::new(0.0, 0.0, 80.0, 24.0);
        let rect = StatusBarDemo::rect(area, &backend);
        assert_eq!(rect.height, backend.chrome_line_height());
    }
}
