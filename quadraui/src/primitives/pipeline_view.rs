//! `PipelineView` primitive: a horizontal row of stage boxes connected by
//! arrows, each with a label, status indicator, and optional action button.
//!
//! Useful for CI/CD pipelines, multi-step wizards, deployment workflows, and
//! any sequential process where stages have discrete pass/fail status.
//!
//! ## Layout model
//!
//! ```text
//! ┌──────────┐      ┌──────────┐      ┌──────────┐
//! │  ✓ Build │ ───▶ │  ● Test  │ ───▶ │  Deploy  │
//! │          │      │  [Retry] │      │  [Go]    │
//! └──────────┘      └──────────┘      └──────────┘
//! ```
//!
//! Stage boxes have equal width (computed from the widest label + padding).
//! Arrow connectors sit between adjacent boxes. Click routing distinguishes
//! action-button vs. stage-body clicks. Keyboard focus moves with Left/Right;
//! Enter fires the focused stage's action if present.
//!
//! ## Event routing
//!
//! `PipelineView` owns layout and hit-testing; backends own painting.
//! After each paint the backend returns a [`PipelineViewLayout`] which the
//! host holds. On mouse events the host calls
//! [`PipelineViewLayout::hit_test`] to translate `(x, y)` into an
//! `Option<PipelineEvent>`. Keyboard events are handled by the host via
//! [`PipelineView::handle_key`].

use crate::event::Rect;
use crate::types::{Modifiers, WidgetId};
use serde::{Deserialize, Serialize};

// ── Data model ───────────────────────────────────────────────────────────────

/// Status of a single pipeline stage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum StageStatus {
    /// Waiting to run — rendered dim.
    Pending,
    /// Currently executing — rendered with accent colour (optional spinner).
    Active,
    /// Completed successfully — rendered with green checkmark (✓).
    Done,
    /// Execution failed — rendered with red X (✗).
    Failed,
    /// Intentionally bypassed — rendered with strikethrough / grey dash (─).
    Skipped,
}

/// A single stage in a [`PipelineView`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PipelineStage {
    /// Display label for this stage (e.g. "Build", "Test", "Deploy").
    pub label: String,
    /// Execution status controlling colour and icon.
    pub status: StageStatus,
    /// Optional action button text shown at the bottom of the box
    /// (e.g. "Go", "Retry", "Skip"). `None` = no button rendered.
    #[serde(default)]
    pub action: Option<String>,
}

/// Declarative description of a horizontal pipeline widget.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PipelineView {
    pub id: WidgetId,
    /// Ordered list of stages rendered left-to-right.
    pub stages: Vec<PipelineStage>,
    /// Index of the keyboard-focused stage, if any.
    #[serde(default)]
    pub focused_stage: Option<usize>,
}

/// Events emitted by a [`PipelineView`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PipelineEvent {
    /// User clicked (or pressed Enter on) the action button of a stage.
    StageAction { index: usize },
    /// User clicked the body of a stage box (not the action button).
    StageSelected { index: usize },
    /// Key pressed while the widget has focus but not consumed by nav.
    KeyPressed { key: String, modifiers: Modifiers },
}

// ── Layout + hit-testing ─────────────────────────────────────────────────────

/// Resolved layout for a single stage (coordinates in backend-native units).
#[derive(Debug, Clone, PartialEq)]
pub struct StageBounds {
    /// Index into [`PipelineView::stages`].
    pub index: usize,
    /// Full stage-box bounds.
    pub box_bounds: Rect,
    /// Bounds of the status icon area (top half of box).
    pub icon_bounds: Rect,
    /// Bounds of the label area.
    pub label_bounds: Rect,
    /// Bounds of the action button, if this stage has one.
    pub action_bounds: Option<Rect>,
    /// Bounds of the arrow connector leading **to** the *next* stage.
    /// `None` for the last stage.
    pub arrow_bounds: Option<Rect>,
}

/// Classification of a hit-test result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PipelineHit {
    /// Click landed on a stage's action button.
    Action(usize),
    /// Click landed on a stage body (not the action button).
    Body(usize),
    /// Click missed all interactive regions.
    Empty,
}

/// Fully-resolved pipeline layout.
#[derive(Debug, Clone, PartialEq)]
pub struct PipelineViewLayout {
    /// Overall bounding rect of the entire widget.
    pub bounds: Rect,
    /// Per-stage resolved positions.
    pub stages: Vec<StageBounds>,
    /// Uniform stage-box width (all stages share this).
    pub stage_width: f32,
    /// Uniform stage-box height.
    pub stage_height: f32,
    /// Width of each arrow connector between boxes.
    pub arrow_width: f32,
}

impl PipelineViewLayout {
    /// Hit-test a click at `(x, y)`. Returns [`PipelineHit::Action`] if the
    /// click falls inside a stage's action-button bounds, [`PipelineHit::Body`]
    /// for the rest of a stage box, or [`PipelineHit::Empty`] otherwise.
    ///
    /// Action bounds are checked first so they win over the stage body on any
    /// overlap.
    pub fn hit_test(&self, x: f32, y: f32) -> PipelineHit {
        for sb in &self.stages {
            // Action button takes priority.
            if let Some(ab) = sb.action_bounds {
                if contains(ab, x, y) {
                    return PipelineHit::Action(sb.index);
                }
            }
            if contains(sb.box_bounds, x, y) {
                return PipelineHit::Body(sb.index);
            }
        }
        PipelineHit::Empty
    }
}

fn contains(r: Rect, x: f32, y: f32) -> bool {
    x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height
}

// ── Measurement ──────────────────────────────────────────────────────────────

/// Caller-supplied measurements for computing a [`PipelineViewLayout`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PipelineViewMeasure {
    /// Width of the whole widget in backend-native units.
    pub width: f32,
    /// Height of the whole widget.
    pub height: f32,
    /// Width of each arrow connector between boxes.
    pub arrow_width: f32,
    /// Height reserved for the action button at the bottom of each box.
    /// `0` if no stages have actions.
    pub action_height: f32,
}

impl PipelineViewMeasure {
    pub fn new(width: f32, height: f32, arrow_width: f32, action_height: f32) -> Self {
        Self {
            width,
            height,
            arrow_width,
            action_height,
        }
    }
}

impl PipelineView {
    /// Compute the layout for this pipeline.
    ///
    /// Stage width is allocated uniformly: each box gets an equal share of
    /// `measure.width` after subtracting the `(n-1) * arrow_width` connectors.
    /// Callers supply `measure.width` from their known widget rect.
    ///
    /// `origin_x`, `origin_y` are the top-left of the widget in backend-native
    /// coordinates.
    pub fn layout(
        &self,
        origin_x: f32,
        origin_y: f32,
        measure: PipelineViewMeasure,
    ) -> PipelineViewLayout {
        let n = self.stages.len();
        let bounds = Rect::new(origin_x, origin_y, measure.width, measure.height);

        if n == 0 {
            return PipelineViewLayout {
                bounds,
                stages: vec![],
                stage_width: 0.0,
                stage_height: measure.height,
                arrow_width: measure.arrow_width,
            };
        }

        let arrow_total = measure.arrow_width * (n as f32 - 1.0).max(0.0);
        let stage_width = ((measure.width - arrow_total) / n as f32).max(1.0);
        let stage_height = measure.height;

        let mut stages = Vec::with_capacity(n);
        let mut x = origin_x;

        for (i, _stage) in self.stages.iter().enumerate() {
            let box_bounds = Rect::new(x, origin_y, stage_width, stage_height);

            // Icon occupies the top 40% of the box (min 1 unit).
            let icon_h = (stage_height * 0.4).max(1.0);
            let icon_bounds = Rect::new(x, origin_y, stage_width, icon_h);

            // Label sits below the icon.
            let label_y = origin_y + icon_h;
            let label_h = if measure.action_height > 0.0 {
                (stage_height - icon_h - measure.action_height).max(0.0)
            } else {
                (stage_height - icon_h).max(0.0)
            };
            let label_bounds = Rect::new(x, label_y, stage_width, label_h);

            // Action button at the bottom of the box.
            let action_bounds = if self.stages[i].action.is_some() && measure.action_height > 0.0 {
                let ay = origin_y + stage_height - measure.action_height;
                Some(Rect::new(x, ay, stage_width, measure.action_height))
            } else {
                None
            };

            // Arrow connector to the right (not on the last stage).
            let arrow_bounds = if i + 1 < n {
                Some(Rect::new(
                    x + stage_width,
                    origin_y,
                    measure.arrow_width,
                    stage_height,
                ))
            } else {
                None
            };

            stages.push(StageBounds {
                index: i,
                box_bounds,
                icon_bounds,
                label_bounds,
                action_bounds,
                arrow_bounds,
            });

            x += stage_width + measure.arrow_width;
        }

        PipelineViewLayout {
            bounds,
            stages,
            stage_width,
            stage_height,
            arrow_width: measure.arrow_width,
        }
    }

    /// Handle a keyboard event on the focused pipeline.
    ///
    /// - `ArrowLeft` / `ArrowRight` move `focused_stage`.
    /// - `Enter` fires [`PipelineEvent::StageAction`] for the focused stage if
    ///   it has an action, or [`PipelineEvent::StageSelected`] otherwise.
    ///
    /// Returns `Some(event)` if the key was consumed, `None` if the caller
    /// should handle it.
    pub fn handle_key(&mut self, key: &str, modifiers: Modifiers) -> Option<PipelineEvent> {
        let n = self.stages.len();
        if n == 0 {
            return None;
        }
        match key {
            "ArrowLeft" | "Left" => {
                let cur = self.focused_stage.unwrap_or(0);
                self.focused_stage = Some(cur.saturating_sub(1));
                None
            }
            "ArrowRight" | "Right" => {
                let cur = self.focused_stage.unwrap_or(0);
                self.focused_stage = Some((cur + 1).min(n - 1));
                None
            }
            "Enter" => {
                if let Some(idx) = self.focused_stage {
                    if idx < n {
                        if self.stages[idx].action.is_some() {
                            return Some(PipelineEvent::StageAction { index: idx });
                        } else {
                            return Some(PipelineEvent::StageSelected { index: idx });
                        }
                    }
                }
                None
            }
            _ => Some(PipelineEvent::KeyPressed {
                key: key.to_string(),
                modifiers,
            }),
        }
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::WidgetId;

    fn make_pipeline() -> PipelineView {
        PipelineView {
            id: WidgetId::new("pipe"),
            stages: vec![
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
            ],
            focused_stage: None,
        }
    }

    fn measure() -> PipelineViewMeasure {
        // 300 wide, 4 arrow units each side, action area 10 units tall.
        PipelineViewMeasure::new(300.0, 60.0, 4.0, 10.0)
    }

    // ── Construction ────────────────────────────────────────────────────

    #[test]
    fn construction_and_stage_count() {
        let p = make_pipeline();
        assert_eq!(p.stages.len(), 3);
        assert_eq!(p.stages[0].label, "Build");
        assert_eq!(p.stages[1].status, StageStatus::Active);
        assert!(p.stages[2].action.is_some());
    }

    #[test]
    fn stage_mutation() {
        let mut p = make_pipeline();
        p.stages[0].status = StageStatus::Failed;
        assert_eq!(p.stages[0].status, StageStatus::Failed);
        p.stages.push(PipelineStage {
            label: "Notify".into(),
            status: StageStatus::Pending,
            action: None,
        });
        assert_eq!(p.stages.len(), 4);
    }

    // ── Layout ──────────────────────────────────────────────────────────

    #[test]
    fn layout_equal_width_boxes() {
        let p = make_pipeline();
        let layout = p.layout(0.0, 0.0, measure());
        // 3 stages, 2 arrows × 4.0 = 8.0; (300 - 8) / 3 ≈ 97.33
        let expected_w = (300.0 - 8.0) / 3.0;
        assert!(
            (layout.stage_width - expected_w).abs() < 0.01,
            "stage_width = {}, expected ~{}",
            layout.stage_width,
            expected_w
        );
        for sb in &layout.stages {
            assert!(
                (sb.box_bounds.width - expected_w).abs() < 0.01,
                "stage {} box width = {}, expected {}",
                sb.index,
                sb.box_bounds.width,
                expected_w
            );
        }
    }

    #[test]
    fn layout_origin_offset() {
        let p = make_pipeline();
        let layout = p.layout(10.0, 5.0, measure());
        assert_eq!(layout.bounds.x, 10.0);
        assert_eq!(layout.bounds.y, 5.0);
        assert_eq!(layout.stages[0].box_bounds.x, 10.0);
        assert_eq!(layout.stages[0].box_bounds.y, 5.0);
    }

    #[test]
    fn layout_arrow_between_stages() {
        let p = make_pipeline();
        let layout = p.layout(0.0, 0.0, measure());
        // Arrow after stage 0.
        let arrow = layout.stages[0].arrow_bounds.expect("arrow after stage 0");
        assert_eq!(arrow.x, layout.stages[0].box_bounds.x + layout.stage_width);
        assert_eq!(arrow.width, 4.0);
        // No arrow after last stage.
        assert!(layout.stages[2].arrow_bounds.is_none());
    }

    #[test]
    fn layout_action_bounds_present_when_action_set() {
        let p = make_pipeline();
        let layout = p.layout(0.0, 0.0, measure());
        // Stage 0 has no action.
        assert!(layout.stages[0].action_bounds.is_none());
        // Stage 1 has "Retry".
        let ab = layout.stages[1]
            .action_bounds
            .expect("action bounds for stage 1");
        assert_eq!(ab.height, 10.0);
        // Stage 2 has "Go".
        assert!(layout.stages[2].action_bounds.is_some());
    }

    #[test]
    fn layout_empty_pipeline_is_safe() {
        let p = PipelineView {
            id: WidgetId::new("empty"),
            stages: vec![],
            focused_stage: None,
        };
        let layout = p.layout(0.0, 0.0, PipelineViewMeasure::new(300.0, 60.0, 4.0, 10.0));
        assert_eq!(layout.stages.len(), 0);
        assert_eq!(layout.stage_width, 0.0);
    }

    // ── Hit-testing ─────────────────────────────────────────────────────

    #[test]
    fn hit_test_action_button_returns_action_event() {
        let p = make_pipeline();
        let layout = p.layout(0.0, 0.0, measure());
        // Stage 1 has action bounds.
        let ab = layout.stages[1].action_bounds.unwrap();
        let cx = ab.x + ab.width / 2.0;
        let cy = ab.y + ab.height / 2.0;
        assert_eq!(layout.hit_test(cx, cy), PipelineHit::Action(1));
    }

    #[test]
    fn hit_test_stage_body_returns_body_event() {
        let p = make_pipeline();
        let layout = p.layout(0.0, 0.0, measure());
        let bb = layout.stages[0].box_bounds;
        // Click in the top half of stage 0 (no action there).
        let cx = bb.x + bb.width / 2.0;
        let cy = bb.y + bb.height / 4.0;
        assert_eq!(layout.hit_test(cx, cy), PipelineHit::Body(0));
    }

    #[test]
    fn hit_test_miss_returns_empty() {
        let p = make_pipeline();
        let layout = p.layout(0.0, 0.0, measure());
        // Click past the right edge.
        assert_eq!(layout.hit_test(500.0, 30.0), PipelineHit::Empty);
        // Click above the widget.
        assert_eq!(layout.hit_test(50.0, -1.0), PipelineHit::Empty);
    }

    #[test]
    fn hit_test_arrow_region_returns_empty() {
        let p = make_pipeline();
        let layout = p.layout(0.0, 0.0, measure());
        let arrow = layout.stages[0].arrow_bounds.unwrap();
        // Arrow connector is non-interactive — no PipelineHit returned for it.
        let hit = layout.hit_test(arrow.x + arrow.width / 2.0, arrow.y + arrow.height / 2.0);
        assert_eq!(hit, PipelineHit::Empty);
    }

    // ── Keyboard navigation ─────────────────────────────────────────────

    #[test]
    fn keyboard_right_moves_focus() {
        let mut p = make_pipeline();
        p.focused_stage = Some(0);
        p.handle_key("Right", Modifiers::default());
        assert_eq!(p.focused_stage, Some(1));
        p.handle_key("ArrowRight", Modifiers::default());
        assert_eq!(p.focused_stage, Some(2));
    }

    #[test]
    fn keyboard_right_clamps_at_last() {
        let mut p = make_pipeline();
        p.focused_stage = Some(2); // last
        p.handle_key("Right", Modifiers::default());
        assert_eq!(p.focused_stage, Some(2));
    }

    #[test]
    fn keyboard_left_moves_focus() {
        let mut p = make_pipeline();
        p.focused_stage = Some(2);
        p.handle_key("Left", Modifiers::default());
        assert_eq!(p.focused_stage, Some(1));
        p.handle_key("ArrowLeft", Modifiers::default());
        assert_eq!(p.focused_stage, Some(0));
    }

    #[test]
    fn keyboard_left_clamps_at_zero() {
        let mut p = make_pipeline();
        p.focused_stage = Some(0);
        p.handle_key("Left", Modifiers::default());
        assert_eq!(p.focused_stage, Some(0));
    }

    #[test]
    fn keyboard_enter_fires_action_when_present() {
        let mut p = make_pipeline();
        p.focused_stage = Some(1); // has "Retry" action
        let event = p.handle_key("Enter", Modifiers::default());
        assert_eq!(event, Some(PipelineEvent::StageAction { index: 1 }));
    }

    #[test]
    fn keyboard_enter_fires_selected_when_no_action() {
        let mut p = make_pipeline();
        p.focused_stage = Some(0); // no action
        let event = p.handle_key("Enter", Modifiers::default());
        assert_eq!(event, Some(PipelineEvent::StageSelected { index: 0 }));
    }

    #[test]
    fn keyboard_enter_no_focus_is_noop() {
        let mut p = make_pipeline();
        p.focused_stage = None;
        let event = p.handle_key("Enter", Modifiers::default());
        assert_eq!(event, None);
    }

    #[test]
    fn keyboard_unknown_key_passes_through() {
        let mut p = make_pipeline();
        p.focused_stage = Some(0);
        let event = p.handle_key("Escape", Modifiers::default());
        assert!(matches!(event, Some(PipelineEvent::KeyPressed { .. })));
    }

    // ── Serde ────────────────────────────────────────────────────────────

    #[test]
    fn serde_roundtrip() {
        let p = make_pipeline();
        let json = serde_json::to_string(&p).unwrap();
        let back: PipelineView = serde_json::from_str(&json).unwrap();
        assert_eq!(p, back);
    }

    #[test]
    fn event_serde_roundtrip() {
        let events = vec![
            PipelineEvent::StageAction { index: 2 },
            PipelineEvent::StageSelected { index: 0 },
            PipelineEvent::KeyPressed {
                key: "Escape".into(),
                modifiers: Modifiers::default(),
            },
        ];
        for e in &events {
            let json = serde_json::to_string(e).unwrap();
            let back: PipelineEvent = serde_json::from_str(&json).unwrap();
            assert_eq!(e, &back);
        }
    }
}
