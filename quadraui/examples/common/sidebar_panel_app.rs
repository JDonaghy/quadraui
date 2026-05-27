//! Backend-agnostic app code for the `SidebarPanel` example
//! ([`tui_sidebar_panel`] / [`gtk_sidebar_panel`]).
//!
//! Exercises every #259 gap end-to-end:
//!
//! - **Gap 1** (multi-row toolbar): the header toolbar is reserved at
//!   2 rows so you can verify the bg fills both rows and the button
//!   row is vertically centred.
//! - **Gap 2** (`SidebarPanel`): the host hands a single rect to
//!   `draw_sidebar_panel`, then paints its content list into
//!   `layout.content_bounds`. No manual carving.
//! - **Gap 3** (`ToolbarHoverTracker`): hover state for the toolbar
//!   buttons is owned by the helper — mouse-move just calls
//!   `tracker.update(...)`.
//! - **Gap 4** (icon-only buttons + wide-glyph icons): the toolbar
//!   includes an icon-only `[ + ]` button alongside icon+label buttons
//!   and a CJK-test glyph item.
//!
//! Controls:
//! - Click `+` button       add a task
//! - Click `↻`              clear last action message
//! - Click `Filter`         toggle the filter (live label flips)
//! - Click `Clear`          empty the task list
//! - Click a task row       select it (highlighted in content area)
//! - q / Esc                quit

use quadraui::{
    AppLogic, Backend, Color, Key, NamedKey, Reaction, Rect, SidebarPanel, SidebarPanelHit,
    StatusBar, StatusBarSegment, Toolbar, ToolbarButton, ToolbarHoverTracker, UiEvent, WidgetId,
};

const TOOLBAR_ROWS: f32 = 2.0;

pub struct SidebarPanelApp {
    tasks: Vec<String>,
    selected: Option<usize>,
    filter_on: bool,
    last_message: String,
    hover: ToolbarHoverTracker,
    pressed: Option<WidgetId>,
    next_id: usize,
}

impl SidebarPanelApp {
    pub fn new() -> Self {
        Self {
            tasks: vec![
                "Review PR #257".into(),
                "Write toolbar tests".into(),
                "Land sidebar panel".into(),
            ],
            selected: Some(0),
            filter_on: false,
            last_message: "Click toolbar buttons, then list rows. q=quit".into(),
            hover: ToolbarHoverTracker::new(),
            pressed: None,
            next_id: 4,
        }
    }

    fn toolbar(&self) -> Toolbar {
        Toolbar {
            id: WidgetId::new("sb:toolbar"),
            buttons: vec![
                // Icon-only `[ + ]` — exercises Gap 4 icon-only width.
                ToolbarButton::Action {
                    id: WidgetId::new("sb:add"),
                    label: "".into(),
                    icon: Some("+".into()),
                    key_hint: None,
                    enabled: true,
                    is_active: false,
                    tooltip: "Add task".into(),
                },
                // Icon-only refresh.
                ToolbarButton::Action {
                    id: WidgetId::new("sb:refresh"),
                    label: "".into(),
                    icon: Some("↻".into()),
                    key_hint: None,
                    enabled: true,
                    is_active: false,
                    tooltip: "Clear last message".into(),
                },
                ToolbarButton::Separator,
                // Icon + label + key hint — toggle.
                ToolbarButton::Action {
                    id: WidgetId::new("sb:filter"),
                    label: "Filter".into(),
                    icon: Some("⚙".into()),
                    key_hint: Some("f".into()),
                    enabled: true,
                    is_active: self.filter_on,
                    tooltip: "Toggle filter".into(),
                },
                ToolbarButton::Action {
                    id: WidgetId::new("sb:clear"),
                    label: "Clear".into(),
                    icon: None,
                    key_hint: Some("c".into()),
                    // Disabled when there's nothing to clear — shows the dim paint.
                    enabled: !self.tasks.is_empty(),
                    is_active: false,
                    tooltip: "Remove all tasks".into(),
                },
                ToolbarButton::Separator,
                ToolbarButton::Label {
                    text: format!(
                        " {} task{} ",
                        self.tasks.len(),
                        if self.tasks.len() == 1 { "" } else { "s" }
                    ),
                    fg: Some(Color::rgb(160, 200, 160)),
                },
            ],
            bg: None,
        }
    }

    fn panel(&self) -> SidebarPanel {
        SidebarPanel {
            id: WidgetId::new("sb:panel"),
            toolbar: Some(self.toolbar()),
            // Gap 1: explicit 2-row tall toolbar slot. The TUI
            // rasteriser must fill both rows and centre the button
            // text — pre-#259 it would only paint row 0.
            toolbar_height: Some(TOOLBAR_ROWS),
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

    fn panel_rect(backend: &dyn Backend) -> Rect {
        let viewport = backend.viewport();
        let lh = backend.line_height();
        Rect::new(
            0.0,
            lh, // below the title row
            viewport.width,
            viewport.height - 2.0 * lh, // above the status row
        )
    }

    fn dispatch_toolbar(&mut self, id: &WidgetId) {
        match id.as_str() {
            "sb:add" => {
                self.tasks.push(format!("Task #{}", self.next_id));
                self.next_id += 1;
                self.last_message = "Added a task".into();
            }
            "sb:refresh" => {
                self.last_message = "Ready".into();
            }
            "sb:filter" => {
                self.filter_on = !self.filter_on;
                self.last_message = format!("Filter {}", if self.filter_on { "on" } else { "off" });
            }
            "sb:clear" => {
                self.tasks.clear();
                self.selected = None;
                self.last_message = "Cleared all tasks".into();
            }
            other => {
                self.last_message = format!("Unknown action: {}", other);
            }
        }
    }
}

impl Default for SidebarPanelApp {
    fn default() -> Self {
        Self::new()
    }
}

impl AppLogic for SidebarPanelApp {
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
                    text: "  SidebarPanel demo — exercises #259 gaps ".into(),
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

        // The panel itself.
        let panel_rect = Self::panel_rect(backend);
        let layout = backend.draw_sidebar_panel(
            panel_rect,
            &self.panel(),
            self.hover.hovered_id(),
            self.pressed.as_ref(),
        );

        // Host paints into the returned content rect. Here: a list of
        // tasks with the selected row highlighted via reverse colours.
        // No manual rect math — `content_bounds` is the authoritative
        // box (proves Gap 2).
        let content = layout.content_bounds;
        let header_text = if self.filter_on {
            "  Tasks (filtered)  "
        } else {
            "  Tasks  "
        };
        let _ = backend.draw_status_bar(
            Rect::new(content.x, content.y, content.width, lh),
            &StatusBar {
                id: WidgetId::new("content:header"),
                left_segments: vec![StatusBarSegment {
                    text: header_text.into(),
                    fg: Color::rgb(200, 200, 200),
                    bg: Color::rgb(20, 20, 20),
                    bold: true,
                    action_id: None,
                }],
                right_segments: vec![],
            },
            None,
            None,
        );

        // Row strip: each task as a single-row status bar with hover-
        // free selection highlight. (Status bar is fine for read-only
        // rows — that's exactly what it's for.)
        for (i, task) in self.tasks.iter().enumerate() {
            let row_y = content.y + lh * (1.0 + i as f32);
            if row_y + lh > content.y + content.height {
                break;
            }
            let row_rect = Rect::new(content.x, row_y, content.width, lh);
            let (fg, bg) = if Some(i) == self.selected {
                (Color::rgb(255, 255, 255), Color::rgb(60, 100, 160))
            } else {
                (Color::rgb(220, 220, 220), Color::rgb(20, 20, 20))
            };
            let _ = backend.draw_status_bar(
                row_rect,
                &StatusBar {
                    id: WidgetId::new(format!("row:{i}")),
                    left_segments: vec![StatusBarSegment {
                        text: format!("  {}  ", task),
                        fg,
                        bg,
                        bold: false,
                        action_id: None,
                    }],
                    right_segments: vec![],
                },
                None,
                None,
            );
        }

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
                    'f' => Some("sb:filter"),
                    'c' if !self.tasks.is_empty() => Some("sb:clear"),
                    _ => None,
                };
                if let Some(id) = id {
                    self.dispatch_toolbar(&WidgetId::new(id));
                    return Reaction::Redraw;
                }
                Reaction::Continue
            }

            UiEvent::MouseMoved { position, .. } => {
                let panel_rect = Self::panel_rect(backend);
                let layout = backend.sidebar_panel_layout(panel_rect, &self.panel());
                // Hover applies only to the toolbar — the helper's
                // hit_test inspects the nested ToolbarLayout when the
                // cursor sits inside the toolbar slot, otherwise it
                // clears.
                let new = if let Some(tlayout) = &layout.toolbar_layout {
                    let mut tmp = self.hover.clone();
                    tmp.update(tlayout, position.x, position.y);
                    tmp
                } else {
                    let mut tmp = self.hover.clone();
                    tmp.clear();
                    tmp
                };
                if new.current() != self.hover.current() {
                    self.hover = new;
                    return Reaction::Redraw;
                }
                Reaction::Continue
            }

            UiEvent::MouseDown { position, .. } => {
                let panel_rect = Self::panel_rect(backend);
                let layout = backend.sidebar_panel_layout(panel_rect, &self.panel());
                match layout.hit_test(position.x, position.y) {
                    SidebarPanelHit::ToolbarButton(id) => {
                        self.pressed = Some(id);
                        Reaction::Redraw
                    }
                    SidebarPanelHit::Content { y, .. } => {
                        // Content rows: header row (lh) then one row
                        // per task. Header row is non-clickable.
                        let lh = backend.line_height();
                        if y >= lh {
                            let idx = ((y - lh) / lh).floor() as usize;
                            if idx < self.tasks.len() {
                                self.selected = Some(idx);
                                self.last_message = format!("Selected: {}", self.tasks[idx]);
                                return Reaction::Redraw;
                            }
                        }
                        Reaction::Continue
                    }
                    _ => Reaction::Continue,
                }
            }

            UiEvent::MouseUp { position, .. } => {
                let panel_rect = Self::panel_rect(backend);
                let layout = backend.sidebar_panel_layout(panel_rect, &self.panel());
                let pressed = self.pressed.take();
                if let (Some(pressed_id), SidebarPanelHit::ToolbarButton(release_id)) =
                    (pressed, layout.hit_test(position.x, position.y))
                {
                    if pressed_id == release_id {
                        self.dispatch_toolbar(&release_id);
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
