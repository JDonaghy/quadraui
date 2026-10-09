//! `Chart` demo — adapted from `quadraui/examples/common/chart_app.rs`.
//!
//! Four variants exercise all four [`ChartKind`]s: a sparkline, a
//! multi-series line chart, a stacked bar chart and a grouped bar
//! chart — the stacked/grouped pair share one dataset so switching
//! between them shows the same numbers composed vs. compared.

use quadraui::{
    Backend, BackendCaps, Chart, ChartKind, Color, InteractionState, Reaction, Rect, Series,
    StatusBar, StatusBarSegment, UiEvent, WidgetId,
};

use crate::demo::{extract_region, Demo};

const SOURCE: &str = include_str!("chart.rs");

// gallery:begin
pub struct ChartDemo {
    sparkline_data: Vec<f64>,
    hovered_point: Option<(usize, usize)>,
    crosshair_x: Option<f64>,
}

impl ChartDemo {
    pub fn new() -> Self {
        Self {
            sparkline_data: vec![30.0, 45.0, 25.0, 60.0, 50.0, 70.0, 55.0, 80.0, 40.0, 65.0],
            hovered_point: None,
            crosshair_x: None,
        }
    }

    fn sparkline(&self) -> Chart {
        Chart {
            id: WidgetId::new("gallery:chart:spark"),
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
            y_ticks: None,
            x_ticks: None,
            show_grid: false,
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
            id: WidgetId::new("gallery:chart:line"),
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
            y_ticks: Some(5),
            x_ticks: None,
            show_grid: true,
        }
    }

    fn bar_chart(&self, kind: ChartKind) -> Chart {
        Chart {
            id: WidgetId::new("gallery:chart:bar"),
            kind,
            series: vec![
                Series {
                    label: "OK".into(),
                    data: vec![120.0, 230.0, 180.0, 310.0, 95.0],
                    color: Some(Color::rgb(80, 220, 120)),
                    fill: false,
                },
                Series {
                    label: "Slow".into(),
                    data: vec![40.0, 60.0, 25.0, 70.0, 15.0],
                    color: Some(Color::rgb(220, 180, 60)),
                    fill: false,
                },
                Series {
                    label: "Failed".into(),
                    data: vec![10.0, 0.0, 35.0, 20.0, 5.0],
                    color: Some(Color::rgb(255, 100, 100)),
                    fill: false,
                },
            ],
            x_label: Some("Endpoint".into()),
            y_label: None,
            y_range: None,
            x_range: None,
            show_legend: true,
            y_ticks: None,
            x_ticks: None,
            show_grid: false,
        }
    }

    fn chart(&self, variant: usize) -> Chart {
        match variant {
            0 => self.sparkline(),
            1 => self.line_chart(),
            2 => self.bar_chart(ChartKind::Bar),
            _ => self.bar_chart(ChartKind::BarGrouped),
        }
    }

    fn chart_rect(variant: usize, area: Rect, backend: &dyn Backend) -> Rect {
        let lh = backend.line_height();
        if variant == 0 {
            Rect::new(area.x, area.y, area.width, lh)
        } else {
            Rect::new(area.x, area.y, area.width, (area.height - lh).max(0.0))
        }
    }

    fn status_rect(area: Rect, backend: &dyn Backend) -> Rect {
        let lh = backend.line_height();
        Rect::new(area.x, area.y + area.height - lh, area.width, lh)
    }

    fn status(&self, variant: usize) -> StatusBar {
        let label = match variant {
            0 => "Sparkline",
            1 => "Line",
            2 => "Bar (stacked)",
            _ => "Bar (grouped)",
        };
        StatusBar {
            id: WidgetId::new("gallery:chart:status"),
            left_segments: vec![StatusBarSegment {
                text: format!(" {label} — hover to inspect a point "),
                fg: Color::rgb(220, 220, 220),
                bg: Color::rgb(40, 40, 60),
                bold: false,
                action_id: None,
            }],
            right_segments: vec![],
        }
    }
}

impl Default for ChartDemo {
    fn default() -> Self {
        Self::new()
    }
}

impl Demo for ChartDemo {
    fn name(&self) -> &'static str {
        "Chart"
    }

    fn group(&self) -> &'static str {
        "Data"
    }

    fn variants(&self) -> &'static [&'static str] {
        &["Sparkline", "Line", "Bar (stacked)", "Bar (grouped)"]
    }

    fn render(&self, variant: usize, backend: &mut dyn Backend, area: Rect) {
        let chart_rect = Self::chart_rect(variant, area, backend);
        let hp = if variant == 0 {
            None
        } else {
            self.hovered_point
        };
        let cx = if variant == 0 { None } else { self.crosshair_x };
        let _ = backend.draw_chart(chart_rect, &self.chart(variant), hp, cx);

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
                key: quadraui::Key::Char(' '),
                ..
            } if variant == 0 => {
                let last = self.sparkline_data.last().copied().unwrap_or(50.0);
                let tick = self.sparkline_data.len() as f64;
                let delta = ((tick * 1.7).sin() * 15.0) + 5.0;
                self.sparkline_data.push((last + delta).clamp(0.0, 100.0));
                if self.sparkline_data.len() > 60 {
                    self.sparkline_data.remove(0);
                }
                Reaction::Redraw
            }
            UiEvent::MouseMoved { position, .. } if variant != 0 => {
                let chart_rect = Self::chart_rect(variant, area, backend);
                let chart = self.chart(variant);
                let layout = backend.chart_layout(chart_rect, &chart);
                let snap = backend.line_height() * 2.0;
                self.hovered_point = layout.nearest_point(position.x, position.y, snap);
                let data_len = chart.max_data_len();
                if position.x >= layout.plot_area.x
                    && position.x < layout.plot_area.x + layout.plot_area.width
                    && position.y >= layout.plot_area.y
                    && position.y < layout.plot_area.y + layout.plot_area.height
                {
                    self.crosshair_x = Some(layout.screen_to_data_x(position.x, data_len));
                } else {
                    self.crosshair_x = None;
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
        serde_json::to_value(self.chart(variant)).unwrap_or(serde_json::Value::Null)
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
    fn variants_cover_all_four_chart_kinds() {
        let demo = ChartDemo::new();
        assert_eq!(demo.chart(0).kind, ChartKind::Sparkline);
        assert_eq!(demo.chart(1).kind, ChartKind::Line);
        assert_eq!(demo.chart(2).kind, ChartKind::Bar);
        assert_eq!(demo.chart(3).kind, ChartKind::BarGrouped);
    }

    #[test]
    fn stacked_and_grouped_bar_variants_share_one_dataset() {
        let demo = ChartDemo::new();
        assert_eq!(demo.chart(2).series, demo.chart(3).series);
    }
}

#[cfg(all(test, feature = "tui"))]
mod tui_tests {
    use super::*;

    #[test]
    fn space_on_the_sparkline_variant_appends_a_data_point() {
        let mut demo = ChartDemo::new();
        let before = demo.sparkline_data.len();
        let mut backend = quadraui::tui::TuiBackend::new();
        let area = Rect::new(0.0, 0.0, 60.0, 20.0);
        let event = UiEvent::KeyPressed {
            key: quadraui::Key::Char(' '),
            modifiers: Default::default(),
            repeat: false,
        };
        let reaction = demo.handle(0, &event, &mut backend, area);
        assert!(matches!(reaction, Reaction::Redraw));
        assert_eq!(demo.sparkline_data.len(), before + 1);
    }
}
