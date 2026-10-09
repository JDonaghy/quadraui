//! `Canvas` demo — adapted from `quadraui/examples/common/canvas_app.rs`.
//!
//! Two variants of the same gauge shape — a track, a proportional
//! fill, a 50% tick, and a percentage label — in different accent
//! colours, each with its own independently adjustable value
//! (`+`/`-`/arrows, or click inside the gauge to jump to a position).

use quadraui::{
    Backend, BackendCaps, Canvas, CanvasHit, Color, DrawOp, InteractionState, Key, MouseButton,
    Point, Reaction, Rect, StatusBar, StatusBarSegment, UiEvent, WidgetId,
};

use crate::demo::{extract_region, Demo};

const SOURCE: &str = include_str!("canvas.rs");

// gallery:begin
pub struct CanvasDemo {
    values: Vec<u8>,
}

impl CanvasDemo {
    pub fn new() -> Self {
        Self {
            values: vec![30, 72],
        }
    }

    fn value(&self, variant: usize) -> u8 {
        self.values[variant.min(self.values.len() - 1)]
    }

    fn gauge_rect(area: Rect, backend: &dyn Backend) -> Rect {
        let lh = backend.line_height();
        let margin = lh;
        Rect::new(
            area.x + margin,
            area.y + lh * 2.0,
            (area.width - margin * 2.0).max(0.0),
            lh,
        )
    }

    fn gauge_canvas(&self, variant: usize, rect: Rect) -> Canvas {
        let w = rect.width;
        let h = rect.height;
        let value = self.value(variant);
        let fill_w = (w * value as f32 / 100.0).clamp(0.0, w);
        let track_color = Color::rgb(60, 60, 60);
        let fill_color = if variant == 1 {
            Color::rgb(220, 140, 40)
        } else {
            Color::rgb(40, 160, 90)
        };
        let tick_color = Color::rgb(220, 220, 220);
        let text_color = Color::rgb(255, 255, 255);
        Canvas {
            id: WidgetId::new(format!("gallery:canvas:gauge:{variant}")),
            ops: vec![
                DrawOp::Rect {
                    rect: Rect::new(0.0, 0.0, w, h),
                    color: track_color,
                },
                DrawOp::Rect {
                    rect: Rect::new(0.0, 0.0, fill_w, h),
                    color: fill_color,
                },
                DrawOp::Line {
                    from: Point::new(w / 2.0, 0.0),
                    to: Point::new(w / 2.0, h),
                    color: tick_color,
                    stroke_width: 1.0,
                },
                DrawOp::TextRun {
                    rect: Rect::new(2.0, 0.0, w.max(1.0), h),
                    text: format!("{value}%"),
                    color: text_color,
                },
            ],
        }
    }

    fn status(&self, variant: usize) -> StatusBar {
        StatusBar {
            id: WidgetId::new("gallery:canvas:status"),
            left_segments: vec![StatusBarSegment {
                text: format!(
                    " gauge: {}% — click it or +/-/arrows to adjust ",
                    self.value(variant)
                ),
                fg: Color::rgb(255, 255, 255),
                bg: Color::rgb(40, 80, 120),
                bold: false,
                action_id: None,
            }],
            right_segments: vec![],
        }
    }

    fn status_rect(area: Rect, backend: &dyn Backend) -> Rect {
        let lh = backend.line_height();
        Rect::new(area.x, area.y + area.height - lh, area.width, lh)
    }
}

impl Default for CanvasDemo {
    fn default() -> Self {
        Self::new()
    }
}

impl Demo for CanvasDemo {
    fn name(&self) -> &'static str {
        "Canvas"
    }

    fn group(&self) -> &'static str {
        "Data"
    }

    fn variants(&self) -> &'static [&'static str] {
        &["Health gauge (green)", "Load gauge (amber)"]
    }

    fn render(&self, variant: usize, backend: &mut dyn Backend, area: Rect) {
        let rect = Self::gauge_rect(area, backend);
        let _ = backend.draw_canvas(rect, &self.gauge_canvas(variant, rect));

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
        let idx = variant.min(self.values.len() - 1);
        match event {
            UiEvent::KeyPressed {
                key: Key::Char('+') | Key::Named(quadraui::NamedKey::Up),
                ..
            } => {
                self.values[idx] = self.values[idx].saturating_add(10).min(100);
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Char('-') | Key::Named(quadraui::NamedKey::Down),
                ..
            } => {
                self.values[idx] = self.values[idx].saturating_sub(10);
                Reaction::Redraw
            }
            UiEvent::MouseDown {
                button: MouseButton::Left,
                position,
                ..
            } => {
                let rect = Self::gauge_rect(area, backend);
                let layout = backend.canvas_layout(rect, &self.gauge_canvas(variant, rect));
                if layout.hit_test(position.x, position.y) == CanvasHit::Inside {
                    let frac = ((position.x - layout.bounds.x) / layout.bounds.width.max(1.0))
                        .clamp(0.0, 1.0);
                    self.values[idx] = (frac * 100.0).round() as u8;
                    return Reaction::Redraw;
                }
                Reaction::Continue
            }
            _ => Reaction::Continue,
        }
    }

    fn source(&self) -> &'static str {
        extract_region(SOURCE)
    }

    fn data(&self, variant: usize) -> serde_json::Value {
        let rect = Rect::new(0.0, 0.0, 40.0, 1.0);
        serde_json::to_value(self.gauge_canvas(variant, rect)).unwrap_or(serde_json::Value::Null)
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
    fn each_variant_has_its_own_value() {
        let demo = CanvasDemo::new();
        assert_eq!(demo.value(0), 30);
        assert_eq!(demo.value(1), 72);
    }

    #[test]
    fn plus_key_raises_the_value_by_ten() {
        let mut demo = CanvasDemo::new();
        let mut backend = quadraui::testing::RecordingBackend::new();
        let area = Rect::new(0.0, 0.0, 40.0, 10.0);
        let event = UiEvent::KeyPressed {
            key: Key::Char('+'),
            modifiers: Default::default(),
            repeat: false,
        };
        let reaction = demo.handle(0, &event, &mut backend, area);
        assert!(matches!(reaction, Reaction::Redraw));
        assert_eq!(demo.value(0), 40);
    }
}
