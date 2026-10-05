//! `Canvas` primitive `AppLogic` ([`tui_canvas`] / [`gtk_canvas`], issue
//! #1102).
//!
//! A custom gauge the 40 other shipped primitives don't cover — exactly the
//! scenario `Canvas`'s own module doc names as the escape hatch's
//! reason to exist: a track ([`DrawOp::Rect`]), a proportional fill
//! ([`DrawOp::Rect`]), a 50% tick mark ([`DrawOp::Line`]), and a
//! percentage label painted on top ([`DrawOp::TextRun`]) — all through
//! one `Backend::draw_canvas` call, on every backend.
//!
//! Controls:
//! - `+` / Up            raise the value by 10
//! - `-` / Down          lower the value by 10
//! - click inside the gauge   jump to that position's value
//! - q / Esc              quit

use quadraui::{
    AppLogic, Backend, Canvas, CanvasHit, Color, DrawOp, InteractionState, Key, MouseButton,
    NamedKey, Point, Reaction, Rect, StatusBar, StatusBarSegment, UiEvent, WidgetId,
};

pub struct CanvasApp {
    value: u8,
}

impl CanvasApp {
    pub fn new() -> Self {
        Self { value: 30 }
    }

    /// The gauge's own rect, in the active backend's native units — a
    /// margin of one line-height on every side, same "size off
    /// `line_height`, not a literal constant" convention as
    /// `examples/common/image_app.rs`'s `bar_rects`.
    fn gauge_rect(&self, backend: &dyn Backend) -> Rect {
        let viewport = backend.viewport();
        let lh = backend.line_height();
        let margin = lh;
        Rect::new(
            margin,
            lh * 2.0,
            (viewport.width - margin * 2.0).max(0.0),
            lh,
        )
    }

    /// Build the gauge's `DrawOp`s, in LOCAL coordinates (`(0, 0)` is
    /// the gauge rect's own top-left corner — `Canvas`'s module doc's
    /// "Coordinate frame" section).
    fn gauge_canvas(&self, rect: Rect) -> Canvas {
        let w = rect.width;
        let h = rect.height;
        let fill_w = (w * self.value as f32 / 100.0).clamp(0.0, w);
        let track_color = Color::rgb(60, 60, 60);
        let fill_color = Color::rgb(40, 160, 90);
        let tick_color = Color::rgb(220, 220, 220);
        let text_color = Color::rgb(255, 255, 255);
        Canvas {
            id: WidgetId::new("gauge"),
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
                    text: format!("{}%", self.value),
                    color: text_color,
                },
            ],
        }
    }

    fn status_bar(&self) -> StatusBar {
        StatusBar {
            id: WidgetId::new("status"),
            left_segments: vec![StatusBarSegment {
                text: format!(
                    " gauge: {}% — click it or +/-/arrows to adjust, q to quit ",
                    self.value
                ),
                fg: Color::rgb(255, 255, 255),
                bg: Color::rgb(40, 80, 120),
                bold: false,
                action_id: None,
            }],
            right_segments: vec![],
        }
    }
}

impl Default for CanvasApp {
    fn default() -> Self {
        Self::new()
    }
}

impl AppLogic for CanvasApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let viewport = backend.viewport();
        let lh = backend.line_height();
        let rect = self.gauge_rect(backend);
        let _ = backend.draw_canvas(rect, &self.gauge_canvas(rect));

        let status_rect = Rect::new(0.0, viewport.height - lh, viewport.width, lh);
        let _ = backend.draw_status_bar_interactive(
            status_rect,
            &self.status_bar(),
            &InteractionState::new(),
        );
    }

    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
        match event {
            UiEvent::KeyPressed {
                key: Key::Char('q'),
                ..
            }
            | UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Escape),
                ..
            } => Reaction::Exit,
            UiEvent::KeyPressed {
                key: Key::Char('+'),
                ..
            }
            | UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Up),
                ..
            } => {
                self.value = self.value.saturating_add(10).min(100);
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Char('-'),
                ..
            }
            | UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Down),
                ..
            } => {
                self.value = self.value.saturating_sub(10);
                Reaction::Redraw
            }
            UiEvent::MouseDown {
                button: MouseButton::Left,
                position,
                ..
            } => {
                let rect = self.gauge_rect(backend);
                // Same rect both `render` and this handler build the
                // canvas from — `Backend::canvas_layout` (the no-paint
                // twin) so paint and click-routing can never disagree,
                // the same contract every other primitive's `*_layout`
                // method carries.
                let layout = backend.canvas_layout(rect, &self.gauge_canvas(rect));
                if layout.hit_test(position.x, position.y) == CanvasHit::Inside {
                    let frac = ((position.x - layout.bounds.x) / layout.bounds.width.max(1.0))
                        .clamp(0.0, 1.0);
                    self.value = (frac * 100.0).round() as u8;
                    return Reaction::Redraw;
                }
                Reaction::Continue
            }
            UiEvent::WindowResized { .. } => Reaction::Redraw,
            _ => Reaction::Continue,
        }
    }
}
