//! `Text Selection` demo — adapted from
//! `quadraui/examples/common/selection_app.rs`.
//!
//! Proves the runner-owned selection pipeline — drag to select, then
//! Ctrl-C to copy — works from a plain `register_text_region` call, no
//! app-side mouse-drag bookkeeping. Two variants swap the content being
//! selected.

use quadraui::{
    Backend, BackendCaps, Color, InteractionState, Reaction, Rect, StatusBar, StatusBarSegment,
    TextRegion, UiEvent, WidgetId,
};

use crate::demo::{extract_region, Demo};

const SOURCE: &str = include_str!("text_selection.rs");

// gallery:begin
const PANGRAM_LINES: &[&str] = &[
    "The quick brown fox jumps over the lazy dog.",
    "Pack my box with five dozen liquor jugs.",
    "How vexingly quick daft zebras jump!",
    "The five boxing wizards jump quickly.",
];

const HAIKU_LINES: &[&str] = &[
    "An old silent pond",
    "A frog jumps into the pond—",
    "Splash! Silence again.",
];

const CONTENT_ID: &str = "gallery:text-selection:content";

pub struct TextSelectionDemo {
    status: String,
}

impl TextSelectionDemo {
    pub fn new() -> Self {
        Self { status: hint() }
    }

    fn lines(variant: usize) -> &'static [&'static str] {
        if variant == 1 {
            HAIKU_LINES
        } else {
            PANGRAM_LINES
        }
    }

    fn content_rect(area: Rect, backend: &dyn Backend) -> Rect {
        let lh = backend.line_height();
        Rect::new(area.x, area.y, area.width, (area.height - lh).max(0.0))
    }

    fn status_rect(area: Rect, backend: &dyn Backend) -> Rect {
        let lh = backend.line_height();
        Rect::new(area.x, area.y + area.height - lh, area.width, lh)
    }
}

impl Default for TextSelectionDemo {
    fn default() -> Self {
        Self::new()
    }
}

impl Demo for TextSelectionDemo {
    fn name(&self) -> &'static str {
        "Text Selection"
    }

    fn group(&self) -> &'static str {
        "Content"
    }

    fn variants(&self) -> &'static [&'static str] {
        &["Pangrams", "Haiku"]
    }

    fn render(&self, variant: usize, backend: &mut dyn Backend, area: Rect) {
        let content = Self::content_rect(area, backend);
        let lh = backend.line_height();
        let lines = Self::lines(variant);
        let fg = Color::rgb(200, 200, 200);
        let bg = Color::rgb(20, 20, 35);
        let mut rendered_rows = 0usize;
        for (i, &line) in lines.iter().enumerate() {
            let row_y = content.y + i as f32 * lh;
            if row_y + lh > content.y + content.height {
                break;
            }
            let bar = StatusBar {
                id: WidgetId::new(format!("{CONTENT_ID}-row-{i}")),
                left_segments: vec![StatusBarSegment {
                    text: format!(" {line}"),
                    fg,
                    bg,
                    bold: false,
                    action_id: None,
                }],
                right_segments: vec![],
            };
            backend.draw_status_bar_interactive(
                Rect::new(content.x, row_y, content.width, lh),
                &bar,
                &InteractionState::new(),
            );
            rendered_rows = i + 1;
        }
        backend.register_text_region(TextRegion {
            id: WidgetId::new(CONTENT_ID),
            bounds: Rect::new(
                content.x,
                content.y,
                content.width,
                rendered_rows as f32 * lh,
            ),
            lines: lines.iter().map(|s| s.to_string()).collect(),
        });

        let status_bar = StatusBar {
            id: WidgetId::new("gallery:text-selection:status"),
            left_segments: vec![StatusBarSegment {
                text: format!(" {} ", self.status),
                fg: Color::rgb(255, 255, 255),
                bg: Color::rgb(40, 80, 120),
                bold: false,
                action_id: None,
            }],
            right_segments: vec![],
        };
        backend.draw_status_bar_interactive(
            Self::status_rect(area, backend),
            &status_bar,
            &InteractionState::new(),
        );
    }

    fn handle(
        &mut self,
        _variant: usize,
        event: &UiEvent,
        backend: &mut dyn Backend,
        _area: Rect,
    ) -> Reaction {
        match event {
            UiEvent::TextCopied(text) => {
                let preview: String = text.chars().take(40).collect();
                let ellipsis = if text.chars().count() > 40 { "…" } else { "" };
                self.status =
                    format!("Copied: \"{preview}{ellipsis}\" — drag or Ctrl-A to select again");
                Reaction::Redraw
            }
            UiEvent::TextSelectionChanged { anchor, focus, .. } => {
                let lh = backend.line_height();
                let a_row = (anchor.y / lh).floor() as usize + 1;
                let f_row = (focus.y / lh).floor() as usize + 1;
                let (start, end) = if a_row <= f_row {
                    (a_row, f_row)
                } else {
                    (f_row, a_row)
                };
                self.status = if start == end {
                    format!("Selecting row {start} — Ctrl-C to copy")
                } else {
                    format!("Selecting rows {start}–{end} — Ctrl-C to copy")
                };
                Reaction::Redraw
            }
            UiEvent::MouseDown { .. } => {
                self.status = hint();
                Reaction::Redraw
            }
            _ => Reaction::Continue,
        }
    }

    fn source(&self) -> &'static str {
        extract_region(SOURCE)
    }

    fn data(&self, variant: usize) -> serde_json::Value {
        serde_json::json!({
            "lines": Self::lines(variant),
            "status": self.status,
        })
    }

    fn caps_note(&self, _variant: usize, caps: &BackendCaps) -> Option<String> {
        if caps.text_selection {
            None
        } else {
            Some(
                "This backend doesn't implement drag-to-select — drag and Ctrl-C/Ctrl-A \
                 have no visible effect here."
                    .to_string(),
            )
        }
    }
}
// gallery:end

fn hint() -> String {
    "drag to select · Ctrl-A select all · Ctrl-C copy".into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variant_one_shows_the_haiku_lines() {
        assert_eq!(TextSelectionDemo::lines(1), HAIKU_LINES);
    }

    #[test]
    fn text_copied_event_updates_the_status_with_a_preview() {
        let mut demo = TextSelectionDemo::new();
        let mut backend = quadraui::testing::RecordingBackend::new();
        let area = Rect::new(0.0, 0.0, 40.0, 10.0);
        let reaction = demo.handle(
            0,
            &UiEvent::TextCopied("hello world".into()),
            &mut backend,
            area,
        );
        assert!(matches!(reaction, Reaction::Redraw));
        assert!(demo.status.contains("hello world"));
    }
}
