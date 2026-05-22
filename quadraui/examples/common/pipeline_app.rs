//! Backend-agnostic app code for the pipeline view example
//! ([`tui_pipeline`] / [`gtk_pipeline`]).
//!
//! [`PipelineApp`] demonstrates a five-stage CI/CD pipeline with various
//! statuses and clickable action buttons. It uses [`PipelineView`] to
//! render the stages and [`StatusBar`] to show a message strip.
//!
//! Controls:
//! - Left / Right arrows    move keyboard focus between stages
//! - Enter                  fire the focused stage's action
//! - r                      reset pipeline to initial state
//! - q / Esc                quit

use quadraui::{
    AppLogic, Backend, Color, Key, NamedKey, PipelineEvent, PipelineHit, PipelineStage,
    PipelineView, Reaction, Rect, StageStatus, StatusBar, StatusBarSegment, UiEvent, WidgetId,
};

pub struct PipelineApp {
    stages: Vec<PipelineStage>,
    focused_stage: Option<usize>,
    last_message: String,
}

impl PipelineApp {
    pub fn new() -> Self {
        Self {
            stages: initial_stages(),
            focused_stage: Some(1), // focus on the Active stage
            last_message: "←/→=focus  Enter=action  r=reset  q=quit".into(),
        }
    }

    fn pipeline(&self) -> PipelineView {
        PipelineView {
            id: WidgetId::new("pipeline"),
            stages: self.stages.clone(),
            focused_stage: self.focused_stage,
        }
    }

    fn status_bar(&self) -> StatusBar {
        StatusBar {
            id: WidgetId::new("status"),
            left_segments: vec![StatusBarSegment {
                text: format!("  {} ", self.last_message),
                fg: Color::rgb(255, 255, 255),
                bg: Color::rgb(40, 80, 120),
                bold: false,
                action_id: None,
            }],
            right_segments: vec![StatusBarSegment {
                text: " q=quit ".into(),
                fg: Color::rgb(200, 200, 200),
                bg: Color::rgb(40, 80, 120),
                bold: false,
                action_id: None,
            }],
        }
    }
}

fn initial_stages() -> Vec<PipelineStage> {
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

impl Default for PipelineApp {
    fn default() -> Self {
        Self::new()
    }
}

impl AppLogic for PipelineApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let viewport = backend.viewport();
        let lh = backend.line_height();

        // Status bar at bottom.
        let status_rect = Rect::new(0.0, viewport.height - lh, viewport.width, lh);
        let _ = backend.draw_status_bar(status_rect, &self.status_bar(), None, None);

        // Pipeline view occupying most of the screen, centred vertically.
        let pv_h = lh * 5.0; // roughly 5 rows tall
        let pv_y = (viewport.height - lh - pv_h) / 2.0;
        let margin = lh * 2.0;
        let pv_rect = Rect::new(margin, pv_y, viewport.width - margin * 2.0, pv_h);
        let _ = backend.draw_pipeline_view(pv_rect, &self.pipeline());
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
                key: Key::Char('r'),
                ..
            } => {
                self.stages = initial_stages();
                self.focused_stage = Some(1);
                self.last_message = "Reset".into();
                Reaction::Redraw
            }

            UiEvent::KeyPressed { key, modifiers, .. } => {
                let key_str = match &key {
                    Key::Named(NamedKey::Left) => "Left",
                    Key::Named(NamedKey::Right) => "Right",
                    Key::Named(NamedKey::Enter) => "Enter",
                    Key::Char(c) => {
                        // Pass unknown chars through as KeyPressed.
                        let s = c.to_string();
                        let mut view = self.pipeline();
                        if let Some(ev) = view.handle_key(&s, modifiers) {
                            self.handle_pipeline_event(ev);
                            self.focused_stage = view.focused_stage;
                            return Reaction::Redraw;
                        }
                        return Reaction::Continue;
                    }
                    _ => return Reaction::Continue,
                };
                let mut view = self.pipeline();
                if let Some(ev) = view.handle_key(key_str, modifiers) {
                    self.handle_pipeline_event(ev);
                }
                self.focused_stage = view.focused_stage;
                Reaction::Redraw
            }

            UiEvent::MouseDown { position, .. } => {
                let viewport = backend.viewport();
                let lh = backend.line_height();
                let pv_h = lh * 5.0;
                let pv_y = (viewport.height - lh - pv_h) / 2.0;
                let margin = lh * 2.0;
                let pv_rect = Rect::new(margin, pv_y, viewport.width - margin * 2.0, pv_h);
                let view = self.pipeline();
                let layout = backend.pipeline_view_layout(pv_rect, &view);
                match layout.hit_test(position.x, position.y) {
                    PipelineHit::Action(idx) => {
                        self.last_message =
                            format!("Action on stage {}: {}", idx, self.stages[idx].label);
                        self.focused_stage = Some(idx);
                    }
                    PipelineHit::Body(idx) => {
                        self.last_message =
                            format!("Selected stage {}: {}", idx, self.stages[idx].label);
                        self.focused_stage = Some(idx);
                    }
                    PipelineHit::Empty => return Reaction::Continue,
                }
                Reaction::Redraw
            }

            UiEvent::WindowResized { .. } => Reaction::Redraw,
            _ => Reaction::Continue,
        }
    }
}

impl PipelineApp {
    fn handle_pipeline_event(&mut self, event: PipelineEvent) {
        match event {
            PipelineEvent::StageAction { index } => {
                let label = self.stages[index].label.clone();
                let action = self.stages[index].action.clone().unwrap_or_default();
                self.last_message = format!("{} on '{}'", action, label);
                // Demo: firing Retry on Active stage marks it Done.
                if self.stages[index].status == StageStatus::Active {
                    self.stages[index].status = StageStatus::Done;
                }
            }
            PipelineEvent::StageSelected { index } => {
                self.last_message = format!("Stage {}: {}", index, self.stages[index].label);
            }
            PipelineEvent::KeyPressed { key, .. } => {
                self.last_message = format!("Key: {}", key);
            }
        }
    }
}
