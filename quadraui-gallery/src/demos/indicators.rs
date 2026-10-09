//! `ProgressBar` + `Spinner` demo — adapted from
//! `quadraui/examples/common/indicators_app.rs`.
//!
//! Two variants: a determinate progress bar (space = +10%, `c` toggles
//! a cancel button) and an indeterminate one with the spinner's frame
//! auto-advancing on every [`Demo::tick`].

use quadraui::{
    Backend, BackendCaps, Color, InteractionState, Key, ProgressBar, ProgressBarHit, Reaction,
    Rect, Spinner, StatusBar, StatusBarSegment, UiEvent, WidgetId,
};

use crate::demo::{extract_region, Demo};

const SOURCE: &str = include_str!("indicators.rs");

// gallery:begin
pub struct IndicatorsDemo {
    progress: f32,
    cancellable: bool,
    spinner_frame: usize,
    last_message: String,
}

impl IndicatorsDemo {
    pub fn new() -> Self {
        Self {
            progress: 0.3,
            cancellable: false,
            spinner_frame: 0,
            last_message: "space=+10% r=reset c=cancel".into(),
        }
    }

    fn progress_bar(&self, variant: usize) -> ProgressBar {
        ProgressBar {
            id: WidgetId::new("gallery:indicators:progress"),
            label: if variant == 1 {
                "Working...".into()
            } else {
                format!("{:.0}%", self.progress * 100.0)
            },
            value: if variant == 1 {
                None
            } else {
                Some(self.progress)
            },
            frame_idx: self.spinner_frame,
            cancellable: self.cancellable,
            accent: None,
        }
    }

    fn spinner(&self) -> Spinner {
        Spinner {
            id: WidgetId::new("gallery:indicators:spinner"),
            label: "Loading...".into(),
            frame_idx: self.spinner_frame,
            accent: None,
        }
    }

    fn status(&self) -> StatusBar {
        StatusBar {
            id: WidgetId::new("gallery:indicators:status"),
            left_segments: vec![StatusBarSegment {
                text: format!(" {} ", self.last_message),
                fg: Color::rgb(255, 255, 255),
                bg: Color::rgb(40, 80, 120),
                bold: false,
                action_id: None,
            }],
            right_segments: vec![],
        }
    }

    fn progress_rect(area: Rect, backend: &dyn Backend) -> Rect {
        let lh = backend.line_height();
        Rect::new(area.x + 1.0, area.y + lh, (area.width - 2.0).max(0.0), lh)
    }

    fn spinner_rect(area: Rect, backend: &dyn Backend) -> Rect {
        let lh = backend.line_height();
        Rect::new(
            area.x + 1.0,
            area.y + lh * 3.0,
            (area.width - 2.0).max(0.0),
            lh,
        )
    }

    fn status_rect(area: Rect, backend: &dyn Backend) -> Rect {
        let lh = backend.line_height();
        Rect::new(area.x, area.y + area.height - lh, area.width, lh)
    }
}

impl Default for IndicatorsDemo {
    fn default() -> Self {
        Self::new()
    }
}

impl Demo for IndicatorsDemo {
    fn name(&self) -> &'static str {
        "Indicators"
    }

    fn group(&self) -> &'static str {
        "Data"
    }

    fn variants(&self) -> &'static [&'static str] {
        &["Progress", "Indeterminate"]
    }

    fn render(&self, variant: usize, backend: &mut dyn Backend, area: Rect) {
        let progress_rect = Self::progress_rect(area, backend);
        let _ = backend.draw_progress(progress_rect, &self.progress_bar(variant));

        let spinner_rect = Self::spinner_rect(area, backend);
        let _ = backend.draw_spinner(spinner_rect, &self.spinner());

        let status_rect = Self::status_rect(area, backend);
        let _ = backend.draw_status_bar_interactive(
            status_rect,
            &self.status(),
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
                key: Key::Char(' '),
                ..
            } => {
                self.progress = (self.progress + 0.1).min(1.0);
                self.spinner_frame += 1;
                self.last_message = format!("Progress: {:.0}%", self.progress * 100.0);
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Char('r'),
                ..
            } => {
                self.progress = 0.0;
                self.last_message = "Reset".into();
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Char('c'),
                ..
            } => {
                self.cancellable = !self.cancellable;
                self.last_message = if self.cancellable {
                    "Cancel enabled".into()
                } else {
                    "Cancel disabled".into()
                };
                Reaction::Redraw
            }
            UiEvent::MouseDown { position, .. } => {
                let progress_rect = Self::progress_rect(area, backend);
                let bar = self.progress_bar(variant);
                let layout = backend.progress_layout(progress_rect, &bar);
                match layout.hit_test(position.x, position.y) {
                    ProgressBarHit::Cancel(_) => {
                        self.last_message = "Cancelled!".into();
                        self.progress = 0.0;
                    }
                    ProgressBarHit::Body(_) => {
                        self.last_message = "Bar clicked".into();
                    }
                    ProgressBarHit::Empty => return Reaction::Continue,
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
            "progress_bar": self.progress_bar(variant),
            "spinner": self.spinner(),
        })
    }

    fn caps_note(&self, _variant: usize, _caps: &BackendCaps) -> Option<String> {
        None
    }

    fn tick(&mut self, _variant: usize, _backend: &mut dyn Backend) -> Reaction {
        self.spinner_frame = self.spinner_frame.wrapping_add(1);
        Reaction::Redraw
    }
}
// gallery:end

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indeterminate_variant_has_no_progress_value() {
        let demo = IndicatorsDemo::new();
        assert_eq!(demo.progress_bar(1).value, None);
        assert_eq!(demo.progress_bar(0).value, Some(demo.progress));
    }

    #[test]
    fn c_key_toggles_cancellable() {
        let mut demo = IndicatorsDemo::new();
        let mut backend = quadraui::testing::RecordingBackend::new();
        let area = Rect::new(0.0, 0.0, 60.0, 10.0);
        let event = UiEvent::KeyPressed {
            key: Key::Char('c'),
            modifiers: Default::default(),
            repeat: false,
        };
        let reaction = demo.handle(0, &event, &mut backend, area);
        assert!(matches!(reaction, Reaction::Redraw));
        assert!(demo.cancellable);
    }

    #[test]
    fn tick_advances_the_spinner_frame() {
        let mut demo = IndicatorsDemo::new();
        let mut backend = quadraui::testing::RecordingBackend::new();
        let before = demo.spinner_frame;
        demo.tick(0, &mut backend);
        assert_eq!(demo.spinner_frame, before + 1);
    }
}
