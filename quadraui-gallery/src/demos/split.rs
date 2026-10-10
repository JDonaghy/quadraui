//! `Split` demo — adapted from `quadraui/examples/common/split_app.rs` and
//! `split_tree_app.rs`.
//!
//! Variant 0 is a single draggable two-pane `Split`. Variant 1 is a 3-way
//! nested `SplitTree` — `Split(Horizontal, Split(Vertical, A, B), C)` —
//! with both dividers draggable through the shared
//! `DragTarget::SplitDivider` dispatch path.

use quadraui::{
    Backend, BackendCaps, Color, DragState, DragTarget, InteractionState, Reaction, Rect, Split,
    SplitDirection, SplitHit, SplitTree, StatusBar, StatusBarSegment, UiEvent, WidgetId,
};

use crate::demo::{extract_region, Demo};

const SOURCE: &str = include_str!("split.rs");

// gallery:begin
const DIVIDER_TOLERANCE: f32 = 2.0;

pub struct SplitDemo {
    ratio: f32,
    direction: SplitDirection,
    dragging: bool,
    tree: SplitTree,
    tree_drag: DragState,
}

impl SplitDemo {
    pub fn new() -> Self {
        Self {
            ratio: 0.5,
            direction: SplitDirection::Horizontal,
            dragging: false,
            tree: Self::default_tree(),
            tree_drag: DragState::new(),
        }
    }

    fn default_tree() -> SplitTree {
        SplitTree::split(
            SplitDirection::Horizontal,
            0.5,
            SplitTree::split(
                SplitDirection::Vertical,
                0.5,
                SplitTree::leaf(WidgetId::new("gallery:split-tree:a")),
                SplitTree::leaf(WidgetId::new("gallery:split-tree:b")),
            ),
            SplitTree::leaf(WidgetId::new("gallery:split-tree:c")),
        )
    }

    fn split(&self) -> Split {
        Split {
            id: WidgetId::new("gallery:split"),
            direction: self.direction,
            ratio: self.ratio,
            first_min: 0.0,
            second_min: 0.0,
        }
    }

    fn fill_pane(backend: &mut dyn Backend, bounds: Rect, label: &str, fg: Color, bg: Color) {
        let lh = backend.line_height();
        let label_rect = Rect::new(bounds.x, bounds.y, bounds.width, lh.min(bounds.height));
        let bar = StatusBar {
            id: WidgetId::new(format!("gallery:split:label:{label}")),
            left_segments: vec![StatusBarSegment {
                text: format!(" {label} "),
                fg,
                bg,
                bold: true,
                action_id: None,
            }],
            right_segments: vec![],
        };
        let _ = backend.draw_status_bar_interactive(label_rect, &bar, &InteractionState::new());
    }
}

impl Default for SplitDemo {
    fn default() -> Self {
        Self::new()
    }
}

impl Demo for SplitDemo {
    fn name(&self) -> &'static str {
        "Split"
    }

    fn group(&self) -> &'static str {
        "Chrome"
    }

    fn variants(&self) -> &'static [&'static str] {
        &["Two-pane", "Nested tree"]
    }

    fn render(&self, variant: usize, backend: &mut dyn Backend, area: Rect) {
        if variant == 0 {
            let split = self.split();
            let layout = backend.draw_split(area, &split);
            Self::fill_pane(
                backend,
                layout.first_bounds,
                "FIRST",
                Color::rgb(255, 255, 255),
                Color::rgb(60, 60, 100),
            );
            Self::fill_pane(
                backend,
                layout.second_bounds,
                "SECOND",
                Color::rgb(255, 255, 255),
                Color::rgb(100, 60, 60),
            );
        } else {
            let layout = backend.draw_split_tree(area, &self.tree);
            let colors = [
                (Color::rgb(255, 255, 255), Color::rgb(60, 60, 100)),
                (Color::rgb(255, 255, 255), Color::rgb(60, 100, 60)),
                (Color::rgb(255, 255, 255), Color::rgb(100, 60, 60)),
            ];
            for (i, (id, leaf_rect)) in layout.leaves.iter().enumerate() {
                let (fg, bg) = colors[i % colors.len()];
                Self::fill_pane(backend, *leaf_rect, id.as_str(), fg, bg);
            }
        }
    }

    fn handle(
        &mut self,
        variant: usize,
        event: &UiEvent,
        backend: &mut dyn Backend,
        area: Rect,
    ) -> Reaction {
        if variant == 0 {
            match event {
                UiEvent::MouseDown { position, .. } => {
                    let split = self.split();
                    let layout = backend.split_layout(area, &split);
                    if let SplitHit::Divider(_) = layout.hit_test(position.x, position.y) {
                        self.dragging = true;
                    }
                    Reaction::Continue
                }
                UiEvent::MouseMoved { position, .. } => {
                    if !self.dragging {
                        return Reaction::Continue;
                    }
                    let new_ratio = match self.direction {
                        SplitDirection::Horizontal => {
                            if area.width > 0.0 {
                                ((position.x - area.x) / area.width).clamp(0.05, 0.95)
                            } else {
                                0.5
                            }
                        }
                        SplitDirection::Vertical => {
                            if area.height > 0.0 {
                                ((position.y - area.y) / area.height).clamp(0.05, 0.95)
                            } else {
                                0.5
                            }
                        }
                    };
                    if (new_ratio - self.ratio).abs() > 0.001 {
                        self.ratio = new_ratio;
                        Reaction::Redraw
                    } else {
                        Reaction::Continue
                    }
                }
                UiEvent::MouseUp { .. } => {
                    self.dragging = false;
                    Reaction::Continue
                }
                _ => Reaction::Continue,
            }
        } else {
            match event {
                UiEvent::MouseDown { position, .. } => {
                    let layout = backend.split_tree_layout(area, &self.tree);
                    if let Some(split_index) = layout.hit_test_divider(*position, DIVIDER_TOLERANCE)
                    {
                        if let Some(div) = layout
                            .dividers
                            .iter()
                            .find(|d| d.split_index == split_index)
                        {
                            self.tree_drag.begin(DragTarget::SplitDivider {
                                tree: WidgetId::new("gallery:split-tree"),
                                split_index,
                                direction: div.direction,
                                axis_start: div.axis_start,
                                axis_size: div.axis_size,
                            });
                        }
                    }
                    Reaction::Continue
                }
                UiEvent::MouseMoved { position, buttons } => {
                    if !self.tree_drag.is_active() {
                        return Reaction::Continue;
                    }
                    let events =
                        quadraui::dispatch_mouse_drag(&self.tree_drag, *position, *buttons);
                    let mut redraw = false;
                    for ev in events {
                        if let UiEvent::SplitDividerDragged {
                            split_index,
                            new_ratio,
                            ..
                        } = ev
                        {
                            redraw |= self.tree.set_ratio_at_index(split_index, new_ratio);
                        }
                    }
                    if redraw {
                        Reaction::Redraw
                    } else {
                        Reaction::Continue
                    }
                }
                UiEvent::MouseUp { .. } => {
                    self.tree_drag.end();
                    Reaction::Continue
                }
                _ => Reaction::Continue,
            }
        }
    }

    fn source(&self) -> &'static str {
        extract_region(SOURCE)
    }

    fn data(&self, variant: usize) -> serde_json::Value {
        if variant == 0 {
            serde_json::to_value(self.split()).unwrap_or(serde_json::Value::Null)
        } else {
            serde_json::json!({
                "leaf_count": self.tree.leaf_count(),
                "split_count": self.tree.split_count(),
            })
        }
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
    fn default_tree_has_three_leaves_and_two_splits() {
        let demo = SplitDemo::new();
        assert_eq!(demo.tree.leaf_count(), 3);
        assert_eq!(demo.tree.split_count(), 2);
    }
}
