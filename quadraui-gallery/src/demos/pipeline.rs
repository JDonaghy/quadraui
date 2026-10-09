//! `PipelineView` demo — adapted from
//! `quadraui/examples/common/pipeline_app.rs`.
//!
//! Two variants: a pipeline mid-run (Checkout/Build done, Test active
//! with a Retry action) and one where every stage already failed —
//! `Enter` fires the focused stage's action, `←`/`→` move focus.

use quadraui::{
    Backend, BackendCaps, Color, InteractionState, Key, PipelineEvent, PipelineHit, PipelineStage,
    PipelineView, Reaction, Rect, StageStatus, StatusBar, StatusBarSegment, UiEvent, WidgetId,
};

use crate::demo::{extract_region, Demo};

const SOURCE: &str = include_str!("pipeline.rs");

// gallery:begin
pub struct PipelineDemo {
    stages: Vec<Vec<PipelineStage>>,
    focused_stage: Vec<Option<usize>>,
    last_message: String,
}

fn in_progress_stages() -> Vec<PipelineStage> {
    vec![
        PipelineStage {
            label: "Checkout".into(),
            status: StageStatus::Done,
            action: None,
        },
        PipelineStage {
            label: "Build".into(),
            status: StageStatus::Done,
            action: None,
        },
        PipelineStage {
            label: "Test".into(),
            status: StageStatus::Active,
            action: Some("Retry".into()),
        },
        PipelineStage {
            label: "Deploy".into(),
            status: StageStatus::Pending,
            action: Some("Go".into()),
        },
        PipelineStage {
            label: "Notify".into(),
            status: StageStatus::Pending,
            action: Some("Skip".into()),
        },
    ]
}

fn failed_stages() -> Vec<PipelineStage> {
    vec![
        PipelineStage {
            label: "Checkout".into(),
            status: StageStatus::Done,
            action: None,
        },
        PipelineStage {
            label: "Build".into(),
            status: StageStatus::Failed,
            action: Some("Retry".into()),
        },
        PipelineStage {
            label: "Test".into(),
            status: StageStatus::Failed,
            action: Some("Retry".into()),
        },
        PipelineStage {
            label: "Deploy".into(),
            status: StageStatus::Pending,
            action: Some("Go".into()),
        },
        PipelineStage {
            label: "Notify".into(),
            status: StageStatus::Pending,
            action: Some("Skip".into()),
        },
    ]
}

impl PipelineDemo {
    pub fn new() -> Self {
        Self {
            stages: vec![in_progress_stages(), failed_stages()],
            // Index 2 is the stage with something to act on in both
            // variants ("Test" is Active in the in-progress variant,
            // Failed in the failed one) — focusing it by default means
            // `Enter` has an immediate, visible effect without the user
            // first pressing `→` twice.
            focused_stage: vec![Some(2), Some(2)],
            last_message: "←/→=focus  Enter=action  r=reset".into(),
        }
    }

    fn view(&self, variant: usize) -> PipelineView {
        PipelineView {
            id: WidgetId::new("gallery:pipeline"),
            stages: self.stages[variant].clone(),
            focused_stage: self.focused_stage[variant],
        }
    }

    fn pipeline_rect(area: Rect, backend: &dyn Backend) -> Rect {
        let lh = backend.line_height();
        let pv_h = (lh * 5.0).min((area.height - lh).max(0.0));
        let pv_y = area.y + ((area.height - lh - pv_h) / 2.0).max(0.0);
        let margin = (lh * 2.0).min(area.width / 2.0);
        Rect::new(
            area.x + margin,
            pv_y,
            (area.width - margin * 2.0).max(0.0),
            pv_h,
        )
    }

    fn status_rect(area: Rect, backend: &dyn Backend) -> Rect {
        let lh = backend.line_height();
        Rect::new(area.x, area.y + area.height - lh, area.width, lh)
    }

    fn status(&self) -> StatusBar {
        StatusBar {
            id: WidgetId::new("gallery:pipeline:status"),
            left_segments: vec![StatusBarSegment {
                text: format!("  {} ", self.last_message),
                fg: Color::rgb(255, 255, 255),
                bg: Color::rgb(40, 80, 120),
                bold: false,
                action_id: None,
            }],
            right_segments: vec![],
        }
    }

    fn apply_event(&mut self, variant: usize, event: PipelineEvent) {
        match event {
            PipelineEvent::StageAction { index } => {
                let label = self.stages[variant][index].label.clone();
                let action = self.stages[variant][index]
                    .action
                    .clone()
                    .unwrap_or_default();
                self.last_message = format!("{action} on '{label}'");
                if self.stages[variant][index].status == StageStatus::Active
                    || self.stages[variant][index].status == StageStatus::Failed
                {
                    self.stages[variant][index].status = StageStatus::Done;
                }
            }
            PipelineEvent::StageSelected { index } => {
                self.last_message = format!("Stage {index}: {}", self.stages[variant][index].label);
            }
            PipelineEvent::KeyPressed { key, .. } => {
                self.last_message = format!("Key: {key}");
            }
        }
    }
}

impl Default for PipelineDemo {
    fn default() -> Self {
        Self::new()
    }
}

impl Demo for PipelineDemo {
    fn name(&self) -> &'static str {
        "Pipeline"
    }

    fn group(&self) -> &'static str {
        "Data"
    }

    fn variants(&self) -> &'static [&'static str] {
        &["In progress", "Failed"]
    }

    fn render(&self, variant: usize, backend: &mut dyn Backend, area: Rect) {
        let pv_rect = Self::pipeline_rect(area, backend);
        let _ = backend.draw_pipeline_view(pv_rect, &self.view(variant));

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
                key: Key::Char('r'),
                ..
            } => {
                self.stages[variant] = if variant == 0 {
                    in_progress_stages()
                } else {
                    failed_stages()
                };
                self.focused_stage[variant] = Some(2);
                self.last_message = "Reset".into();
                Reaction::Redraw
            }
            UiEvent::KeyPressed { key, modifiers, .. } => {
                let key_str = match key {
                    Key::Named(quadraui::NamedKey::Left) => "Left",
                    Key::Named(quadraui::NamedKey::Right) => "Right",
                    Key::Named(quadraui::NamedKey::Enter) => "Enter",
                    Key::Char(c) => {
                        let s = c.to_string();
                        let mut view = self.view(variant);
                        return match view.handle_key(&s, *modifiers) {
                            Some(ev) => {
                                self.apply_event(variant, ev);
                                self.focused_stage[variant] = view.focused_stage;
                                Reaction::Redraw
                            }
                            None => Reaction::Continue,
                        };
                    }
                    _ => return Reaction::Continue,
                };
                let mut view = self.view(variant);
                if let Some(ev) = view.handle_key(key_str, *modifiers) {
                    self.apply_event(variant, ev);
                }
                self.focused_stage[variant] = view.focused_stage;
                Reaction::Redraw
            }
            UiEvent::MouseDown { position, .. } => {
                let pv_rect = Self::pipeline_rect(area, backend);
                let view = self.view(variant);
                let layout = backend.pipeline_view_layout(pv_rect, &view);
                match layout.hit_test(position.x, position.y) {
                    PipelineHit::Action(idx) => {
                        self.last_message =
                            format!("Action on stage {idx}: {}", self.stages[variant][idx].label);
                        self.focused_stage[variant] = Some(idx);
                    }
                    PipelineHit::Body(idx) => {
                        self.last_message =
                            format!("Selected stage {idx}: {}", self.stages[variant][idx].label);
                        self.focused_stage[variant] = Some(idx);
                    }
                    PipelineHit::Empty => return Reaction::Continue,
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
        serde_json::to_value(self.view(variant)).unwrap_or(serde_json::Value::Null)
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
    fn failed_variant_starts_with_two_failed_stages() {
        let demo = PipelineDemo::new();
        let failed = demo.stages[1]
            .iter()
            .filter(|s| s.status == StageStatus::Failed)
            .count();
        assert_eq!(failed, 2);
    }

    #[test]
    fn enter_on_the_focused_active_stage_marks_it_done() {
        let mut demo = PipelineDemo::new();
        let mut backend = quadraui::testing::RecordingBackend::new();
        let area = Rect::new(0.0, 0.0, 80.0, 20.0);
        let event = UiEvent::KeyPressed {
            key: Key::Named(quadraui::NamedKey::Enter),
            modifiers: Default::default(),
            repeat: false,
        };
        let reaction = demo.handle(0, &event, &mut backend, area);
        assert!(matches!(reaction, Reaction::Redraw));
        assert_eq!(demo.stages[0][2].status, StageStatus::Done);
    }
}
