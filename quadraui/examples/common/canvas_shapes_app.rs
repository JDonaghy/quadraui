//! `Canvas` v2 op demo ([`tui_canvas_shapes`] / [`gtk_canvas_shapes`]).
//!
//! A chart-style gauge drawn entirely from the `Canvas` v2 op set, one
//! `Backend::draw_canvas` call, on every backend:
//! - [`DrawOp::StrokeRect`] — the plot border.
//! - [`DrawOp::DashedLine`] — horizontal gridlines.
//! - [`DrawOp::Polygon`] — tapered bars (a trapezoid per bar, not a
//!   plain rect, so this demo actually exercises the fill rasteriser).
//! - [`DrawOp::Ellipse`] — a peak marker on top of each bar.
//! - [`DrawOp::Bezier`] — a smoothed trend curve through the bar tops.
//! - [`DrawOp::Arc`] — a small corner dial showing the average value.
//! - [`DrawOp::StyledTextRun`] — per-bar value labels.
//!
//! Every op here degrades to the braille grid on TUI (see
//! `crate::primitives::canvas`'s module doc for the degrade table) and
//! paints natively on GTK/macOS/Win — this file has no backend-specific
//! branch at all.
//!
//! Controls:
//! - r         randomize the bar values
//! - q / Esc   quit

use quadraui::primitives::canvas::CanvasFontRole;
use quadraui::{
    AppLogic, Backend, Canvas, Color, DrawOp, InteractionState, Key, NamedKey, Point, Reaction,
    Rect, StatusBar, StatusBarSegment, UiEvent, WidgetId,
};

const LABELS: [&str; 5] = ["Mon", "Tue", "Wed", "Thu", "Fri"];

pub struct CanvasShapesApp {
    values: [f32; 5],
    seed: u32,
}

impl CanvasShapesApp {
    pub fn new() -> Self {
        Self {
            values: [35.0, 60.0, 45.0, 80.0, 55.0],
            seed: 1,
        }
    }

    /// The chart's own rect — a one-line-height margin on every side,
    /// the same "size off `line_height`" convention as
    /// `examples/common/canvas_app.rs`'s `gauge_rect`.
    fn chart_rect(&self, backend: &dyn Backend) -> Rect {
        let viewport = backend.viewport();
        let lh = backend.line_height();
        Rect::new(
            lh,
            lh * 2.0,
            (viewport.width - lh * 2.0).max(1.0),
            (viewport.height - lh * 5.0).max(1.0),
        )
    }

    /// Build the chart's `DrawOp`s, in LOCAL coordinates (`(0, 0)` is
    /// `rect`'s own top-left corner — see `Canvas`'s module doc).
    fn chart_canvas(&self, rect: Rect) -> Canvas {
        let w = rect.width;
        let h = rect.height;
        let label_h = (h * 0.15).max(1.0);
        let plot_h = (h - label_h).max(1.0);

        let border_color = Color::rgb(90, 90, 90);
        let grid_color = Color::rgb(60, 60, 60);
        let bar_color = Color::rgb(80, 160, 255);
        let marker_color = Color::rgb(255, 210, 90);
        let trend_color = Color::rgb(255, 120, 80);
        let dial_color = Color::rgb(80, 220, 120);
        let text_color = Color::rgb(230, 230, 230);

        let mut ops = vec![DrawOp::StrokeRect {
            rect: Rect::new(0.0, 0.0, w, plot_h),
            color: border_color,
            stroke_width: 1.0,
        }];

        for frac in [0.25_f32, 0.5, 0.75] {
            let y = plot_h * (1.0 - frac);
            ops.push(DrawOp::DashedLine {
                from: Point::new(0.0, y),
                to: Point::new(w, y),
                color: grid_color,
                stroke_width: 1.0,
                dash_len: 4.0,
                gap_len: 3.0,
            });
        }

        let n = self.values.len();
        let slot_w = w / n as f32;
        let bar_w = slot_w * 0.6;
        let bar_gap = (slot_w - bar_w) / 2.0;
        let mut tops = Vec::with_capacity(n);
        for (i, &value) in self.values.iter().enumerate() {
            let x = slot_w * i as f32 + bar_gap;
            let top_y = plot_h * (1.0 - value.clamp(0.0, 100.0) / 100.0);
            tops.push(Point::new(x + bar_w / 2.0, top_y));

            // A trapezoid, not a plain rect, so this exercises
            // `DrawOp::Polygon`'s even-odd fill rather than the plain
            // `DrawOp::Rect` the older `canvas_app` demo already covers.
            let taper = bar_w * 0.15;
            ops.push(DrawOp::Polygon {
                points: vec![
                    Point::new(x, plot_h),
                    Point::new(x + bar_w, plot_h),
                    Point::new(x + bar_w - taper, top_y),
                    Point::new(x + taper, top_y),
                ],
                color: bar_color,
            });

            ops.push(DrawOp::Ellipse {
                center: Point::new(x + bar_w / 2.0, top_y),
                radius_x: bar_w * 0.18,
                radius_y: bar_w * 0.18,
                color: marker_color,
            });

            ops.push(DrawOp::StyledTextRun {
                rect: Rect::new(x - bar_gap, plot_h, slot_w, label_h),
                text: format!("{} {:.0}", LABELS[i % LABELS.len()], value),
                color: text_color,
                role: CanvasFontRole::Chrome,
                italic: false,
                size: None,
            });
        }

        // Smooth trend curve through the bar tops, one cubic Bézier
        // segment per pair of consecutive tops with control points
        // placed a third of the way along the horizontal span.
        for i in 0..tops.len().saturating_sub(1) {
            let from = tops[i];
            let to = tops[i + 1];
            let dx = (to.x - from.x) / 3.0;
            ops.push(DrawOp::Bezier {
                from,
                control1: Point::new(from.x + dx, from.y),
                control2: Point::new(to.x - dx, to.y),
                to,
                color: trend_color,
                stroke_width: 2.0,
            });
        }

        // A small corner dial reporting the average value as a stroked
        // arc sweep, `-90°` (straight up) plus up to a half turn.
        let avg = self.values.iter().sum::<f32>() / n.max(1) as f32;
        let dial_center = Point::new(w - bar_w, bar_w);
        let dial_radius = bar_w * 0.7;
        let start_angle = -std::f32::consts::FRAC_PI_2;
        let end_angle = start_angle + std::f32::consts::PI * (avg.clamp(0.0, 100.0) / 100.0);
        ops.push(DrawOp::Arc {
            center: dial_center,
            radius: dial_radius,
            start_angle,
            end_angle,
            color: dial_color,
            stroke_width: 2.0,
        });

        Canvas {
            id: WidgetId::new("canvas_shapes"),
            ops,
        }
    }

    fn status_bar(&self) -> StatusBar {
        StatusBar {
            id: WidgetId::new("status"),
            left_segments: vec![StatusBarSegment {
                text: " Canvas v2 shapes demo — r=randomize q=quit ".into(),
                fg: Color::rgb(255, 255, 255),
                bg: Color::rgb(40, 80, 120),
                bold: false,
                action_id: None,
            }],
            right_segments: vec![],
        }
    }

    /// Deterministic pseudo-random walk (no `rand` dependency) — same
    /// `sin`-of-a-counter trick `examples/common/chart_app.rs`'s
    /// `next_value` uses.
    fn randomize(&mut self) {
        for (i, value) in self.values.iter_mut().enumerate() {
            self.seed = self.seed.wrapping_add(1);
            let t = (self.seed as f32) * 0.7 + (i as f32) * 1.3;
            *value = (50.0 + 45.0 * t.sin()).clamp(5.0, 100.0);
        }
    }
}

impl Default for CanvasShapesApp {
    fn default() -> Self {
        Self::new()
    }
}

impl AppLogic for CanvasShapesApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let viewport = backend.viewport();
        let lh = backend.line_height();
        let rect = self.chart_rect(backend);
        let _ = backend.draw_canvas(rect, &self.chart_canvas(rect));

        let status_rect = Rect::new(0.0, viewport.height - lh, viewport.width, lh);
        let _ = backend.draw_status_bar_interactive(
            status_rect,
            &self.status_bar(),
            &InteractionState::new(),
        );
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
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
                key: Key::Char('r'),
                ..
            } => {
                self.randomize();
                Reaction::Redraw
            }
            UiEvent::WindowResized { .. } => Reaction::Redraw,
            _ => Reaction::Continue,
        }
    }
}
