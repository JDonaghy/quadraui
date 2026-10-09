//! `Minimap` demo — adapted from
//! `quadraui/examples/common/minimap_app.rs`.
//!
//! Two variants toggle [`MinimapScale`] (`One` vs. `Two`) over the
//! same ~120-line synthetic buffer. `Up`/`Down` scroll the viewport
//! band; clicking the strip seeks to that fraction of the file.

use quadraui::{
    aggregate_spans, sample_blocks, Backend, BackendCaps, Color, InteractionState, Key, Minimap,
    MinimapGrid, MinimapHit, MinimapScale, MinimapSpan, MouseButton, Reaction, Rect, StatusBar,
    StatusBarSegment, UiEvent, WidgetId,
};

use crate::demo::{extract_region, Demo};

const SOURCE: &str = include_str!("minimap.rs");

// gallery:begin
const VIEWPORT_ROWS: usize = 10;
const LINES_PER_ROW: usize = 4;

pub struct MinimapDemo {
    buffer: Vec<String>,
    scroll_offset: usize,
}

impl MinimapDemo {
    pub fn new() -> Self {
        let buffer = (0..120)
            .map(|i| match i % 6 {
                0 => format!("fn function_{i}() {{"),
                1 => "    let value = compute();".to_string(),
                2 => "    // a short comment explaining it".to_string(),
                3 => String::new(),
                4 => "    value.process()".to_string(),
                _ => "}".to_string(),
            })
            .collect();
        Self {
            buffer,
            scroll_offset: 0,
        }
    }

    fn scale(variant: usize) -> MinimapScale {
        if variant == 1 {
            MinimapScale::Two
        } else {
            MinimapScale::One
        }
    }

    fn minimap(&self, area: Rect, backend: &dyn Backend) -> Minimap {
        let buffer = &self.buffer;
        let lines = sample_blocks(buffer.len(), buffer.len(), |i| buffer[i].clone());

        let visible_row_start = lines
            .iter()
            .position(|l| l.line_idx >= self.scroll_offset)
            .unwrap_or(0);
        let visible_row_end = lines
            .iter()
            .position(|l| l.line_idx >= self.scroll_offset + VIEWPORT_ROWS)
            .unwrap_or(lines.len());

        let mut minimap = Minimap {
            id: WidgetId::new("gallery:minimap"),
            lines,
            syntax_spans: Vec::new(),
            visible_row_start,
            visible_row_count: visible_row_end.saturating_sub(visible_row_start).max(1),
            total_buffer_lines: self.buffer.len(),
        };

        let minimap_rect = Self::minimap_rect(area, backend);
        let cols_per_cell = backend.minimap_layout(minimap_rect, &minimap).cols_per_cell;

        let raw_spans: Vec<MinimapSpan> = minimap
            .lines
            .iter()
            .enumerate()
            .filter_map(|(idx, l)| {
                let trimmed = l.text.trim_start();
                if trimmed.starts_with("fn") {
                    Some(MinimapSpan {
                        line_idx: idx,
                        start_col: 0,
                        end_col: 2,
                        color: Color::rgb(80, 160, 255),
                    })
                } else if trimmed.starts_with("//") {
                    Some(MinimapSpan {
                        line_idx: idx,
                        start_col: 4,
                        end_col: l.text.len(),
                        color: Color::rgb(100, 180, 100),
                    })
                } else {
                    None
                }
            })
            .collect();
        let grid = MinimapGrid {
            rows: minimap.lines.len().div_ceil(LINES_PER_ROW).max(1),
            cols: 200,
            lines_per_row: LINES_PER_ROW,
            cols_per_cell,
        };
        minimap.syntax_spans = aggregate_spans(&raw_spans, grid);
        minimap
    }

    fn minimap_rect(area: Rect, backend: &dyn Backend) -> Rect {
        let lh = backend.line_height();
        let minimap_w = (backend.char_width() * 14.0).min(area.width);
        Rect::new(
            area.x + area.width - minimap_w,
            area.y,
            minimap_w,
            (area.height - lh).max(0.0),
        )
    }

    fn status_rect(area: Rect, backend: &dyn Backend) -> Rect {
        let lh = backend.line_height();
        Rect::new(area.x, area.y + area.height - lh, area.width, lh)
    }

    fn status(&self, variant: usize) -> StatusBar {
        let scale_label = if variant == 1 { "2 (2x4)" } else { "1 (1x2)" };
        StatusBar {
            id: WidgetId::new("gallery:minimap:status"),
            left_segments: vec![StatusBarSegment {
                text: format!(" line {} — scale {scale_label} ", self.scroll_offset),
                fg: Color::rgb(255, 255, 255),
                bg: Color::rgb(40, 80, 120),
                bold: false,
                action_id: None,
            }],
            right_segments: vec![StatusBarSegment {
                text: " up/down=scroll click=seek ".into(),
                fg: Color::rgb(220, 220, 220),
                bg: Color::rgb(40, 80, 120),
                bold: false,
                action_id: None,
            }],
        }
    }

    fn seek(&mut self, fraction: f32) {
        let max_start = self.buffer.len().saturating_sub(VIEWPORT_ROWS);
        let target = (fraction as f64 * max_start as f64).round() as usize;
        self.scroll_offset = target.min(max_start);
    }
}

impl Default for MinimapDemo {
    fn default() -> Self {
        Self::new()
    }
}

impl Demo for MinimapDemo {
    fn name(&self) -> &'static str {
        "Minimap"
    }

    fn group(&self) -> &'static str {
        "Data"
    }

    fn variants(&self) -> &'static [&'static str] {
        &["Scale 1 (1x2)", "Scale 2 (2x4)"]
    }

    fn render(&self, variant: usize, backend: &mut dyn Backend, area: Rect) {
        backend.set_minimap_scale(Self::scale(variant));
        let minimap_rect = Self::minimap_rect(area, backend);
        let minimap = self.minimap(area, backend);
        let _ = backend.draw_minimap(minimap_rect, &minimap);

        let status_rect = Self::status_rect(area, backend);
        let _ = backend.draw_status_bar_interactive(
            status_rect,
            &self.status(variant),
            &InteractionState::new(),
        );
    }

    fn handle(
        &mut self,
        variant: usize,
        event: &UiEvent,
        backend: &mut dyn Backend,
        area: Rect,
    ) -> Reaction {
        match event {
            UiEvent::KeyPressed {
                key: Key::Named(quadraui::NamedKey::Down),
                ..
            } => {
                let max_start = self.buffer.len().saturating_sub(VIEWPORT_ROWS);
                self.scroll_offset = (self.scroll_offset + 1).min(max_start);
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Named(quadraui::NamedKey::Up),
                ..
            } => {
                self.scroll_offset = self.scroll_offset.saturating_sub(1);
                Reaction::Redraw
            }
            UiEvent::MouseDown {
                button: MouseButton::Left,
                position,
                ..
            } => {
                backend.set_minimap_scale(Self::scale(variant));
                let minimap_rect = Self::minimap_rect(area, backend);
                let minimap = self.minimap(area, backend);
                let layout = backend.minimap_layout(minimap_rect, &minimap);
                if let MinimapHit::Seek { fraction } = layout.hit_test(position.x, position.y) {
                    self.seek(fraction);
                }
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
            "scale": format!("{:?}", Self::scale(variant)),
            "scroll_offset": self.scroll_offset,
            "total_lines": self.buffer.len(),
        })
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
    fn variant_one_selects_scale_two() {
        assert_eq!(MinimapDemo::scale(0), MinimapScale::One);
        assert_eq!(MinimapDemo::scale(1), MinimapScale::Two);
    }

    #[test]
    fn seek_clamps_to_the_last_valid_start() {
        let mut demo = MinimapDemo::new();
        demo.seek(2.0);
        assert_eq!(demo.scroll_offset, demo.buffer.len() - VIEWPORT_ROWS);
    }
}

#[cfg(all(test, feature = "tui"))]
mod tui_tests {
    use super::*;

    #[test]
    fn down_key_advances_scroll_offset() {
        let mut demo = MinimapDemo::new();
        let mut backend = quadraui::tui::TuiBackend::new();
        let area = Rect::new(0.0, 0.0, 60.0, 20.0);
        let event = UiEvent::KeyPressed {
            key: Key::Named(quadraui::NamedKey::Down),
            modifiers: Default::default(),
            repeat: false,
        };
        let reaction = demo.handle(0, &event, &mut backend, area);
        assert!(matches!(reaction, Reaction::Redraw));
        assert_eq!(demo.scroll_offset, 1);
    }
}
