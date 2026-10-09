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
    UiEvent, WidgetId,
};

use crate::demo::{extract_region, Demo};

const SOURCE: &str = include_str!("status_bar.rs");

// gallery:begin
pub struct StatusBarDemo {
    dirty: bool,
}

impl StatusBarDemo {
    pub fn new() -> Self {
        Self { dirty: false }
    }

    fn status_bar(&self) -> StatusBar {
        let mode_text = if self.dirty {
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
        let lh = backend.line_height();
        let rect = Rect::new(area.x, area.y, area.width, lh);
        let dirty = variant == 1;
        let bar = StatusBarDemo { dirty }.status_bar();
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
        let dirty = variant == 1;
        serde_json::to_value(StatusBarDemo { dirty }.status_bar())
            .unwrap_or(serde_json::Value::Null)
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
        let clean = StatusBarDemo { dirty: false }.status_bar();
        let dirty = StatusBarDemo { dirty: true }.status_bar();
        assert!(dirty.left_segments[0].text.len() > clean.left_segments[0].text.len());
    }
}
