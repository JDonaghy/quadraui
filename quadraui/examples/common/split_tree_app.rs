//! Backend-agnostic app code for the split-tree example
//! ([`tui_split_tree`] / [`gtk_split_tree`]).
//!
//! [`SplitTreeApp`] demonstrates [`quadraui::SplitTree`] (issue #435):
//! a 3-way nested split — `Split(Horizontal, Split(Vertical, A, B), C)`
//! — with all dividers draggable through the shared
//! [`quadraui::DragTarget::SplitDivider`] dispatch path (the same
//! `dispatch_mouse_drag` machinery scrollbars already use), instead of
//! hand-rolled per-divider ratio math.
//!
//! Controls:
//! - drag either divider      resize the adjacent panes
//! - r                        reset both ratios to 0.5
//! - q / Esc                  quit

use quadraui::{
    AppLogic, Backend, Color, DragState, DragTarget, Key, NamedKey, Reaction, Rect, StatusBar,
    StatusBarSegment, UiEvent, WidgetId,
};

const TREE_ID: &str = "demo-tree";
/// Symmetric pixel/cell tolerance for divider hit-testing — see
/// `SplitTreeDivider::hit_tolerant`. 1.0 covers both TUI's single-cell
/// divider and GTK's ~4px one comfortably.
const DIVIDER_TOLERANCE: f32 = 2.0;

pub struct SplitTreeApp {
    tree: quadraui::SplitTree,
    drag: DragState,
}

impl SplitTreeApp {
    pub fn new() -> Self {
        Self {
            tree: Self::default_tree(),
            drag: DragState::new(),
        }
    }

    /// Current tree state, for test assertions (`driver.app().tree()`).
    pub fn tree(&self) -> &quadraui::SplitTree {
        &self.tree
    }

    fn default_tree() -> quadraui::SplitTree {
        quadraui::SplitTree::split(
            quadraui::SplitDirection::Horizontal,
            0.5,
            quadraui::SplitTree::split(
                quadraui::SplitDirection::Vertical,
                0.5,
                quadraui::SplitTree::leaf(WidgetId::new("a")),
                quadraui::SplitTree::leaf(WidgetId::new("b")),
            ),
            quadraui::SplitTree::leaf(WidgetId::new("c")),
        )
    }

    fn tree_rect(&self, backend: &mut dyn Backend) -> Rect {
        let viewport = backend.viewport();
        let lh = backend.line_height();
        Rect::new(0.0, 0.0, viewport.width, viewport.height - lh)
    }

    fn status_bar(&self) -> StatusBar {
        StatusBar {
            id: WidgetId::new("status"),
            left_segments: vec![StatusBarSegment {
                text: format!(
                    " SplitTree — {} leaves, {} dividers ",
                    self.tree.leaf_count(),
                    self.tree.split_count()
                ),
                fg: Color::rgb(255, 255, 255),
                bg: Color::rgb(40, 80, 120),
                bold: false,
                action_id: None,
            }],
            right_segments: vec![StatusBarSegment {
                text: " drag dividers | r=reset | q=quit ".into(),
                fg: Color::rgb(220, 220, 220),
                bg: Color::rgb(40, 80, 120),
                bold: false,
                action_id: None,
            }],
        }
    }

    fn fill_pane(
        &self,
        backend: &mut dyn Backend,
        bounds: Rect,
        label: &str,
        fg: Color,
        bg: Color,
    ) {
        let lh = backend.line_height();
        let label_rect = Rect::new(bounds.x, bounds.y, bounds.width, lh.min(bounds.height));
        let bar = StatusBar {
            id: WidgetId::new("pane-label"),
            left_segments: vec![StatusBarSegment {
                text: format!(" {label} "),
                fg,
                bg,
                bold: true,
                action_id: None,
            }],
            right_segments: vec![],
        };
        let _ = backend.draw_status_bar(label_rect, &bar, None, None);
    }
}

impl Default for SplitTreeApp {
    fn default() -> Self {
        Self::new()
    }
}

impl AppLogic for SplitTreeApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let rect = self.tree_rect(backend);
        let layout = backend.draw_split_tree(rect, &self.tree);

        let colors = [
            (Color::rgb(255, 255, 255), Color::rgb(60, 60, 100)),
            (Color::rgb(255, 255, 255), Color::rgb(60, 100, 60)),
            (Color::rgb(255, 255, 255), Color::rgb(100, 60, 60)),
        ];
        for (i, (id, leaf_rect)) in layout.leaves.iter().enumerate() {
            let (fg, bg) = colors[i % colors.len()];
            self.fill_pane(backend, *leaf_rect, id.as_str(), fg, bg);
        }

        let viewport = backend.viewport();
        let lh = backend.line_height();
        let status_rect = Rect::new(0.0, viewport.height - lh, viewport.width, lh);
        let _ = backend.draw_status_bar(status_rect, &self.status_bar(), None, None);
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
                self.tree.set_all_ratios(0.5);
                Reaction::Redraw
            }
            UiEvent::MouseDown { position, .. } => {
                let rect = self.tree_rect(backend);
                let layout = backend.split_tree_layout(rect, &self.tree);
                if let Some(split_index) = layout.hit_test_divider(position, DIVIDER_TOLERANCE) {
                    if let Some(div) = layout
                        .dividers
                        .iter()
                        .find(|d| d.split_index == split_index)
                    {
                        self.drag.begin(DragTarget::SplitDivider {
                            tree: WidgetId::new(TREE_ID),
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
                if !self.drag.is_active() {
                    return Reaction::Continue;
                }
                // The exact machinery a backend's mouse-move handler
                // calls — DragTarget::SplitDivider carries enough state
                // (axis_start/axis_size) to resume the ratio math
                // without re-walking the tree.
                let events = quadraui::dispatch_mouse_drag(&self.drag, position, buttons);
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
                self.drag.end();
                Reaction::Continue
            }
            UiEvent::WindowResized { .. } => Reaction::Redraw,
            _ => Reaction::Continue,
        }
    }
}
