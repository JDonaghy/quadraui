//! `Toolbar` demo — adapted from `quadraui/examples/common/toolbar_app.rs`.
//!
//! Exercises action buttons, a toggle action, a permanently-disabled
//! action, a separator, a non-clickable label, hover/press state and
//! keyboard focus (Tab / Shift-Tab / Enter) — the `Toolbar` primitive's
//! full surface.

use quadraui::{
    Backend, BackendCaps, Color, InteractionState, Key, NamedKey, Reaction, Rect, StatusBar,
    StatusBarSegment, Toolbar, ToolbarButton, ToolbarHit, UiEvent, WidgetId,
};

use crate::demo::{extract_region, Demo};

const SOURCE: &str = include_str!("toolbar.rs");

// gallery:begin
pub struct ToolbarDemo {
    filter_active: bool,
    running: bool,
    last_message: String,
    interaction: InteractionState,
    focused_index: Option<usize>,
}

impl ToolbarDemo {
    pub fn new() -> Self {
        Self {
            filter_active: false,
            running: true,
            last_message: "Click, Tab to focus, Enter to activate".into(),
            interaction: InteractionState::new(),
            focused_index: None,
        }
    }

    fn toolbar(&self) -> Toolbar {
        Toolbar::new(WidgetId::new("gallery:toolbar"))
            .with_buttons(vec![
                ToolbarButton::Action {
                    id: WidgetId::new("gallery:toolbar:continue"),
                    label: "Continue".into(),
                    icon: Some("▶".into()),
                    key_hint: Some("1".into()),
                    enabled: !self.running,
                    is_active: false,
                    tooltip: "Resume execution".into(),
                },
                ToolbarButton::Action {
                    id: WidgetId::new("gallery:toolbar:pause"),
                    label: "Pause".into(),
                    icon: Some("⏸".into()),
                    key_hint: Some("2".into()),
                    enabled: self.running,
                    is_active: false,
                    tooltip: "Pause execution".into(),
                },
                ToolbarButton::Separator,
                ToolbarButton::Action {
                    id: WidgetId::new("gallery:toolbar:filter"),
                    label: "Filter".into(),
                    icon: Some("⚙".into()),
                    key_hint: Some("3".into()),
                    enabled: true,
                    is_active: self.filter_active,
                    tooltip: "Toggle filter".into(),
                },
                ToolbarButton::Action {
                    id: WidgetId::new("gallery:toolbar:debug"),
                    label: "Debug".into(),
                    icon: None,
                    key_hint: None,
                    enabled: false,
                    is_active: false,
                    tooltip: "Disabled in this build".into(),
                },
                ToolbarButton::Label {
                    text: if self.running {
                        " running".into()
                    } else {
                        " paused".into()
                    },
                    fg: Some(if self.running {
                        Color::rgb(120, 200, 120)
                    } else {
                        Color::rgb(220, 180, 80)
                    }),
                },
            ])
            .with_focused_index(self.focused_index)
    }

    fn toolbar_rect(area: Rect) -> Rect {
        Rect::new(area.x, area.y, area.width, area.height)
    }

    fn hint_bar(&self, backend: &dyn Backend) -> StatusBar {
        let _ = backend;
        StatusBar {
            id: WidgetId::new("gallery:toolbar:hint"),
            left_segments: vec![StatusBarSegment {
                text: format!("  {} ", self.last_message),
                fg: Color::rgb(255, 255, 255),
                bg: Color::rgb(30, 30, 30),
                bold: false,
                action_id: None,
            }],
            right_segments: vec![],
        }
    }

    fn dispatch(&mut self, id: &WidgetId) {
        match id.as_str() {
            "gallery:toolbar:continue" => {
                self.running = true;
                self.last_message = "Continue".into();
            }
            "gallery:toolbar:pause" => {
                self.running = false;
                self.last_message = "Paused".into();
            }
            "gallery:toolbar:filter" => {
                self.filter_active = !self.filter_active;
                self.last_message =
                    format!("Filter {}", if self.filter_active { "on" } else { "off" });
            }
            other => {
                self.last_message = format!("Unknown action: {other}");
            }
        }
    }

    fn focusable_indices(&self) -> Vec<usize> {
        self.toolbar()
            .buttons
            .into_iter()
            .enumerate()
            .filter_map(|(i, btn)| match btn {
                ToolbarButton::Action { enabled, .. } if enabled => Some(i),
                _ => None,
            })
            .collect()
    }

    fn advance_focus(&mut self, forward: bool) {
        let candidates = self.focusable_indices();
        if candidates.is_empty() {
            return;
        }
        self.focused_index = Some(match self.focused_index {
            None => {
                if forward {
                    candidates[0]
                } else {
                    *candidates.last().unwrap()
                }
            }
            Some(current) => match candidates.iter().position(|&i| i == current) {
                None => candidates[0],
                Some(p) => {
                    let next = if forward {
                        (p + 1) % candidates.len()
                    } else {
                        (p + candidates.len() - 1) % candidates.len()
                    };
                    candidates[next]
                }
            },
        });
    }

    fn activate_focused(&mut self) -> bool {
        let Some(idx) = self.focused_index else {
            return false;
        };
        let bar = self.toolbar();
        if let Some(ToolbarButton::Action { id, enabled, .. }) = bar.buttons.get(idx) {
            if *enabled {
                let id = id.clone();
                self.dispatch(&id);
                return true;
            }
        }
        false
    }
}

impl Default for ToolbarDemo {
    fn default() -> Self {
        Self::new()
    }
}

impl Demo for ToolbarDemo {
    fn name(&self) -> &'static str {
        "Toolbar"
    }

    fn group(&self) -> &'static str {
        "Chrome"
    }

    fn render(&self, _variant: usize, backend: &mut dyn Backend, area: Rect) {
        let lh = backend.line_height();
        let rect = Self::toolbar_rect(area);
        let bar = self.toolbar();
        let _ = backend.draw_toolbar_interactive(rect, &bar, &self.interaction);

        let hint_rect = Rect::new(area.x, area.y + lh, area.width, lh);
        let _ = backend.draw_status_bar_interactive(
            hint_rect,
            &self.hint_bar(backend),
            &InteractionState::new(),
        );
    }

    fn handle(
        &mut self,
        _variant: usize,
        event: &UiEvent,
        backend: &mut dyn Backend,
        area: Rect,
    ) -> Reaction {
        match event {
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Tab),
                ..
            } => {
                self.advance_focus(true);
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::BackTab),
                ..
            } => {
                self.advance_focus(false);
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Enter) | Key::Char(' '),
                ..
            } => {
                if self.activate_focused() {
                    Reaction::Redraw
                } else {
                    Reaction::Continue
                }
            }
            UiEvent::KeyPressed {
                key: Key::Char(c @ '1'..='3'),
                ..
            } => {
                let id = match c {
                    '1' => "gallery:toolbar:continue",
                    '2' => "gallery:toolbar:pause",
                    _ => "gallery:toolbar:filter",
                };
                let bar = self.toolbar();
                let allowed = bar.buttons.iter().any(|btn| {
                    matches!(btn, ToolbarButton::Action { id: bid, enabled, .. }
                        if bid.as_str() == id && *enabled)
                });
                if allowed {
                    self.dispatch(&WidgetId::new(id));
                    return Reaction::Redraw;
                }
                Reaction::Continue
            }
            UiEvent::MouseMoved { .. } | UiEvent::MouseDown { .. } | UiEvent::MouseUp { .. } => {
                let rect = Self::toolbar_rect(area);
                let bar = self.toolbar();
                let layout = backend.toolbar_layout(rect, &bar);
                let hit_test = |x: f32, y: f32| match layout.hit_test(x, y) {
                    ToolbarHit::Button(id) => Some(id),
                    ToolbarHit::Empty => None,
                };
                let was_pressed = self.interaction.pressed().cloned();
                let changed = self.interaction.handle_mouse(event, hit_test);

                if let UiEvent::MouseUp { position, .. } = event {
                    if let (Some(pressed_id), ToolbarHit::Button(release_id)) =
                        (was_pressed, layout.hit_test(position.x, position.y))
                    {
                        if pressed_id == release_id {
                            self.dispatch(&release_id);
                        }
                    }
                    return Reaction::Redraw;
                }
                if changed {
                    Reaction::Redraw
                } else {
                    Reaction::Continue
                }
            }
            _ => Reaction::Continue,
        }
    }

    fn source(&self) -> &'static str {
        extract_region(SOURCE)
    }

    fn data(&self, _variant: usize) -> serde_json::Value {
        serde_json::to_value(self.toolbar()).unwrap_or(serde_json::Value::Null)
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
    fn dispatch_toggles_filter() {
        let mut demo = ToolbarDemo::new();
        assert!(!demo.filter_active);
        demo.dispatch(&WidgetId::new("gallery:toolbar:filter"));
        assert!(demo.filter_active);
    }

    #[test]
    fn advance_focus_skips_disabled_buttons() {
        let mut demo = ToolbarDemo::new();
        // `running` starts `true`, so "Continue" (index 0) is disabled —
        // the first focus stop must be "Pause" (index 1), not index 0.
        demo.advance_focus(true);
        assert_eq!(demo.focused_index, Some(1));
    }
}
