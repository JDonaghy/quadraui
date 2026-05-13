//! Backend-agnostic app code for the chart example
//! ([`tui_chart`] / [`gtk_chart`]).
//!
//! Demonstrates all three chart kinds: sparkline, line, and bar.
//!
//! Controls:
//! - space       add a data point to the sparkline
//! - 1 / 2 / 3  switch view to sparkline / line / bar
//! - q / Esc     quit

use quadraui::{
    AppLogic, Backend, Chart, ChartKind, Color, Key, NamedKey, Reaction, Rect, Series, StatusBar,
    StatusBarSegment, UiEvent, WidgetId,
};

pub struct ChartApp {
    sparkline_data: Vec<f64>,
    active_kind: ChartKind,
    tick: usize,
}

impl ChartApp {
    pub fn new() -> Self {
        Self {
            sparkline_data: vec![30.0, 45.0, 25.0, 60.0, 50.0, 70.0, 55.0, 80.0, 40.0, 65.0],
            active_kind: ChartKind::Sparkline,
            tick: 0,
        }
    }

    fn sparkline(&self) -> Chart {
        Chart {
            id: WidgetId::new("spark"),
            kind: ChartKind::Sparkline,
            series: vec![Series {
                label: "CPU".into(),
                data: self.sparkline_data.clone(),
                color: Some(Color::rgb(80, 200, 120)),
                fill: false,
            }],
            x_label: None,
            y_label: None,
            y_range: Some((0.0, 100.0)),
            x_range: None,
            show_legend: false,
        }
    }

    fn line_chart(&self) -> Chart {
        let n = 20;
        let cpu: Vec<f64> = (0..n)
            .map(|i| 30.0 + 20.0 * ((i as f64 * 0.3).sin()) + (i as f64 * 0.5))
            .collect();
        let mem: Vec<f64> = (0..n)
            .map(|i| 50.0 + 15.0 * ((i as f64 * 0.2 + 1.0).cos()))
            .collect();

        Chart {
            id: WidgetId::new("line"),
            kind: ChartKind::Line,
            series: vec![
                Series {
                    label: "CPU".into(),
                    data: cpu,
                    color: Some(Color::rgb(80, 160, 255)),
                    fill: false,
                },
                Series {
                    label: "Memory".into(),
                    data: mem,
                    color: Some(Color::rgb(255, 120, 80)),
                    fill: true,
                },
            ],
            x_label: Some("Time (s)".into()),
            y_label: Some("Usage".into()),
            y_range: Some((0.0, 100.0)),
            x_range: None,
            show_legend: true,
        }
    }

    fn bar_chart(&self) -> Chart {
        Chart {
            id: WidgetId::new("bar"),
            kind: ChartKind::Bar,
            series: vec![Series {
                label: "Requests".into(),
                data: vec![120.0, 230.0, 180.0, 310.0, 95.0],
                color: Some(Color::rgb(80, 220, 120)),
                fill: false,
            }],
            x_label: Some("Endpoint".into()),
            y_label: None,
            y_range: None,
            x_range: None,
            show_legend: false,
        }
    }

    fn status_bar(&self) -> StatusBar {
        let kind_label = match self.active_kind {
            ChartKind::Sparkline => "Sparkline",
            ChartKind::Line => "Line",
            ChartKind::Bar => "Bar",
        };
        StatusBar {
            id: WidgetId::new("status"),
            left_segments: vec![StatusBarSegment {
                text: format!(" Chart: {kind_label} "),
                fg: Color::rgb(255, 255, 255),
                bg: Color::rgb(40, 80, 120),
                bold: false,
                action_id: None,
            }],
            right_segments: vec![StatusBarSegment {
                text: " 1/2/3=kind space=add q=quit ".into(),
                fg: Color::rgb(220, 220, 220),
                bg: Color::rgb(40, 80, 120),
                bold: false,
                action_id: None,
            }],
        }
    }

    fn next_value(&self) -> f64 {
        let last = self.sparkline_data.last().copied().unwrap_or(50.0);
        let delta = ((self.tick as f64 * 1.7).sin() * 15.0) + 5.0;
        (last + delta).clamp(0.0, 100.0)
    }
}

impl Default for ChartApp {
    fn default() -> Self {
        Self::new()
    }
}

impl AppLogic for ChartApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let viewport = backend.viewport();
        let lh = backend.line_height();

        let chart_rect = Rect::new(1.0, lh, viewport.width - 2.0, viewport.height - lh * 3.0);
        match self.active_kind {
            ChartKind::Sparkline => {
                let spark_rect = Rect::new(1.0, lh, viewport.width - 2.0, lh);
                let _ = backend.draw_chart(spark_rect, &self.sparkline());
            }
            ChartKind::Line => {
                let _ = backend.draw_chart(chart_rect, &self.line_chart());
            }
            ChartKind::Bar => {
                let _ = backend.draw_chart(chart_rect, &self.bar_chart());
            }
        }

        let status_rect = Rect::new(0.0, viewport.height - lh, viewport.width, lh);
        let _ = backend.draw_status_bar(status_rect, &self.status_bar(), None, None);
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
                key: Key::Char(' '),
                ..
            } => {
                self.tick += 1;
                let val = self.next_value();
                self.sparkline_data.push(val);
                if self.sparkline_data.len() > 60 {
                    self.sparkline_data.remove(0);
                }
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Char('1'),
                ..
            } => {
                self.active_kind = ChartKind::Sparkline;
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Char('2'),
                ..
            } => {
                self.active_kind = ChartKind::Line;
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Char('3'),
                ..
            } => {
                self.active_kind = ChartKind::Bar;
                Reaction::Redraw
            }
            UiEvent::WindowResized { .. } => Reaction::Redraw,
            _ => Reaction::Continue,
        }
    }
}
