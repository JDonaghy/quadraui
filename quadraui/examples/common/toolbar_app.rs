//! Backend-agnostic app code for the toolbar example
//! ([`tui_toolbar`] / `gtk_toolbar`).
//!
//! [`ToolbarApp`] demonstrates the `Toolbar` primitive's full surface:
//! - Action buttons with icon + label + key hint
//! - A toggle action (`is_active` flips on click)
//! - A permanently disabled action (renders dim, swallows clicks)
//! - A `Separator` between button groups
//! - A non-clickable `Label` showing live state ("paused" / "running")
//! - Hover state (highlight tracks the mouse cursor)
//!
//! Controls:
//! - Click an action button       fire it
//! - 1 / 2 / 3 / 4               keyboard shortcuts for the four enabled actions
//! - q / Esc                     quit

use quadraui::{
    AppLogic, Backend, Color, Key, NamedKey, Reaction, Rect, StatusBar, StatusBarSegment, Toolbar,
    ToolbarButton, ToolbarHit, UiEvent, WidgetId,
};

pub struct ToolbarApp {
    /// Set when the user toggles the "Filter" action.
    filter_active: bool,
    /// Running vs paused (Label item flips with this).
    running: bool,
    /// Last status line message (echoes the most recent click / keypress).
    last_message: String,
    /// `WidgetId` of the toolbar action currently under the mouse, or
    /// `None` when the cursor is outside. Drives the hover highlight —
    /// rasterisers paint `theme.hover_bg` on the matching button.
    hovered_id: Option<WidgetId>,
    /// `WidgetId` of the action currently held down by the user (between
    /// `MouseDown` and `MouseUp`). Drives the pressed highlight.
    pressed_id: Option<WidgetId>,
}

impl ToolbarApp {
    pub fn new() -> Self {
        Self {
            filter_active: false,
            running: true,
            last_message: "Click a button or press 1/2/3/4. q=quit".into(),
            hovered_id: None,
            pressed_id: None,
        }
    }

    fn toolbar(&self) -> Toolbar {
        Toolbar {
            id: WidgetId::new("demo:toolbar"),
            buttons: vec![
                ToolbarButton::Action {
                    id: WidgetId::new("demo:continue"),
                    label: "Continue".into(),
                    icon: Some("▶".into()),
                    key_hint: Some("1".into()),
                    enabled: !self.running,
                    is_active: false,
                    tooltip: "Resume execution".into(),
                },
                ToolbarButton::Action {
                    id: WidgetId::new("demo:pause"),
                    label: "Pause".into(),
                    icon: Some("⏸".into()),
                    key_hint: Some("2".into()),
                    enabled: self.running,
                    is_active: false,
                    tooltip: "Pause execution".into(),
                },
                ToolbarButton::Separator,
                ToolbarButton::Action {
                    id: WidgetId::new("demo:filter"),
                    label: "Filter".into(),
                    icon: Some("⚙".into()),
                    key_hint: Some("3".into()),
                    enabled: true,
                    is_active: self.filter_active,
                    tooltip: "Toggle filter".into(),
                },
                ToolbarButton::Action {
                    id: WidgetId::new("demo:reset"),
                    label: "Reset".into(),
                    icon: Some("↺".into()),
                    key_hint: Some("4".into()),
                    enabled: true,
                    is_active: false,
                    tooltip: "Reset to defaults".into(),
                },
                ToolbarButton::Separator,
                ToolbarButton::Action {
                    id: WidgetId::new("demo:debug"),
                    label: "Debug".into(),
                    icon: None,
                    key_hint: None,
                    // Permanently disabled — demonstrates dimmed paint
                    // and that clicks are dropped.
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
            ],
            // `None` lets the backend pick its theme default (header_bg).
            bg: None,
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

    /// Layout rect of the toolbar inside the viewport. Shared between
    /// `render` (paint) and `handle` (hit-test) so paint and click
    /// consume the same coordinates — the source-of-truth contract.
    fn toolbar_rect(backend: &dyn Backend) -> Rect {
        let viewport = backend.viewport();
        let lh = backend.line_height();
        Rect::new(0.0, lh, viewport.width, lh)
    }

    fn dispatch(&mut self, id: &WidgetId) {
        match id.as_str() {
            "demo:continue" => {
                self.running = true;
                self.last_message = "Continue".into();
            }
            "demo:pause" => {
                self.running = false;
                self.last_message = "Paused".into();
            }
            "demo:filter" => {
                self.filter_active = !self.filter_active;
                self.last_message =
                    format!("Filter {}", if self.filter_active { "on" } else { "off" });
            }
            "demo:reset" => {
                self.filter_active = false;
                self.running = true;
                self.last_message = "Reset".into();
            }
            other => {
                self.last_message = format!("Unknown action: {}", other);
            }
        }
    }
}

impl Default for ToolbarApp {
    fn default() -> Self {
        Self::new()
    }
}

impl AppLogic for ToolbarApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let viewport = backend.viewport();
        let lh = backend.line_height();

        // Title row.
        let title_rect = Rect::new(0.0, 0.0, viewport.width, lh);
        let _ = backend.draw_status_bar(
            title_rect,
            &StatusBar {
                id: WidgetId::new("title"),
                left_segments: vec![StatusBarSegment {
                    text: "  Toolbar primitive demo ".into(),
                    fg: Color::rgb(255, 255, 255),
                    bg: Color::rgb(30, 30, 30),
                    bold: true,
                    action_id: None,
                }],
                right_segments: vec![],
            },
            None,
            None,
        );

        // Toolbar in the second row.
        let _ = backend.draw_toolbar(
            Self::toolbar_rect(backend),
            &self.toolbar(),
            self.hovered_id.as_ref(),
            self.pressed_id.as_ref(),
        );

        // Status bar at the bottom.
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
                key: Key::Char(c), ..
            } => {
                let id = match c {
                    '1' => Some("demo:continue"),
                    '2' => Some("demo:pause"),
                    '3' => Some("demo:filter"),
                    '4' => Some("demo:reset"),
                    _ => None,
                };
                if let Some(id) = id {
                    // Honour the toolbar's enabled state — same gate the
                    // hit-test path applies to mouse clicks.
                    let bar = self.toolbar();
                    let allowed = bar.buttons.iter().any(|btn| {
                        matches!(
                            btn,
                            ToolbarButton::Action { id: bid, enabled, .. }
                                if bid.as_str() == id && *enabled
                        )
                    });
                    if allowed {
                        self.dispatch(&WidgetId::new(id));
                        return Reaction::Redraw;
                    }
                }
                Reaction::Continue
            }

            UiEvent::MouseMoved { position, .. } => {
                let rect = Self::toolbar_rect(backend);
                let bar = self.toolbar();
                let layout = backend.toolbar_layout(rect, &bar);
                let new_hover = match layout.hit_test(position.x, position.y) {
                    ToolbarHit::Button(id) => Some(id),
                    ToolbarHit::Empty => None,
                };
                if new_hover != self.hovered_id {
                    self.hovered_id = new_hover;
                    return Reaction::Redraw;
                }
                Reaction::Continue
            }

            UiEvent::MouseDown { position, .. } => {
                let rect = Self::toolbar_rect(backend);
                let bar = self.toolbar();
                let layout = backend.toolbar_layout(rect, &bar);
                match layout.hit_test(position.x, position.y) {
                    ToolbarHit::Button(id) => {
                        self.pressed_id = Some(id);
                        Reaction::Redraw
                    }
                    ToolbarHit::Empty => Reaction::Continue,
                }
            }

            UiEvent::MouseUp { position, .. } => {
                let rect = Self::toolbar_rect(backend);
                let bar = self.toolbar();
                let layout = backend.toolbar_layout(rect, &bar);
                let pressed = self.pressed_id.take();
                if let (Some(pressed_id), ToolbarHit::Button(release_id)) =
                    (pressed, layout.hit_test(position.x, position.y))
                {
                    // Fire only if release lands on the same button as
                    // press — the standard click-versus-drag contract.
                    if pressed_id == release_id {
                        self.dispatch(&release_id);
                    }
                    return Reaction::Redraw;
                }
                Reaction::Redraw
            }

            UiEvent::WindowResized { .. } => Reaction::Redraw,
            _ => Reaction::Continue,
        }
    }
}
