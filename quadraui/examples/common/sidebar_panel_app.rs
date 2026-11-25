//! Backend-agnostic app code for the `SidebarPanel` example
//! ([`tui_sidebar_panel`] / [`gtk_sidebar_panel`]).
//!
//! Demonstrates a sidebar panel layout with:
//!
//! - **Multi-row toolbar**: the header toolbar is reserved at
//!   2 rows so the background fills both rows and the button
//!   row is vertically centred.
//! - **SidebarPanel**: the host paints the panel through
//!   `DrawsPrimitives::draw_dyn`, then paints its content list into
//!   `layout.content_bounds`. No manual carving.
//! - **Widget contract**: the panel is registered in the app's
//!   `FrameTree`, so hover, press and click on its toolbar route
//!   through the panel's own `Interactive` impl and arrive as
//!   `UiEvent::Toolbar(_, ButtonClicked { .. })`.
//! - **Icon-only buttons with wide-glyph icons**: the toolbar
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

use std::cell::Cell;

use quadraui::{
    AppLogic, Backend, Color, DrawsPrimitives, FrameTree, InteractionState, Key, NamedKey,
    Reaction, Rect, SidebarPanel, SidebarPanelHit, SidebarPanelLayout, StatusBar, StatusBarSegment,
    Toolbar, ToolbarButton, ToolbarEvent, UiEvent, WidgetId,
};

const TOOLBAR_ROWS: f32 = 2.0;

pub struct SidebarPanelApp {
    tasks: Vec<String>,
    selected: Option<usize>,
    filter_on: bool,
    last_message: String,
    next_id: usize,
    /// Where the panel painted last frame, for `frame_tree` to register
    /// it at.
    panel_rect: Cell<Rect>,
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
            next_id: 4,
            panel_rect: Cell::new(Rect::default()),
        }
    }

    fn toolbar(&self) -> Toolbar {
        Toolbar::new(WidgetId::new("sb:toolbar")).with_buttons(vec![
            // Icon-only `[ + ]` — exercises Gap 4 icon-only width.
            ToolbarButton::Action {
                spec: None,
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
                spec: None,
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
                spec: None,
                id: WidgetId::new("sb:filter"),
                label: "Filter".into(),
                icon: Some("⚙".into()),
                key_hint: Some("f".into()),
                enabled: true,
                is_active: self.filter_on,
                tooltip: "Toggle filter".into(),
            },
            ToolbarButton::Action {
                spec: None,
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
        ])
    }

    fn panel_id() -> WidgetId {
        WidgetId::new("sb:panel")
    }

    fn panel(&self) -> SidebarPanel {
        SidebarPanel {
            id: Self::panel_id(),
            toolbar: Some(self.toolbar()),
            // The toolbar is 2 rows tall; background fills both rows
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
        let _ = backend.draw_status_bar_interactive(
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
            &InteractionState::new(),
        );

        // The panel itself, painted through the widget contract.
        let panel_rect = Self::panel_rect(backend);
        self.panel_rect.set(panel_rect);
        let layout = backend
            .draw_dyn(panel_rect, &self.panel(), &())
            .downcast_ref::<SidebarPanelLayout>()
            .cloned()
            .expect("a SidebarPanel resolves a SidebarPanelLayout");

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
        let _ = backend.draw_status_bar_interactive(
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
            &InteractionState::new(),
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
            let _ = backend.draw_status_bar_interactive(
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
                &InteractionState::new(),
            );
        }

        // Status bar at the bottom.
        let status_rect = Rect::new(0.0, viewport.height - lh, viewport.width, lh);
        let _ = backend.draw_status_bar_interactive(
            status_rect,
            &self.status_bar(),
            &InteractionState::new(),
        );
    }

    fn frame_tree(&self, _area: ()) -> FrameTree {
        let mut tree = FrameTree::new();
        tree.push(
            Self::panel_id(),
            self.panel_rect.get(),
            Box::new(self.panel()),
        );
        tree
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

            UiEvent::Toolbar(_, ToolbarEvent::ButtonClicked { id, .. }) => {
                self.dispatch_toolbar(&id);
                Reaction::Redraw
            }

            // A press the panel's toolbar didn't claim bubbles here; a
            // press in the content area selects a task row.
            UiEvent::MouseDown { position, .. } => {
                let panel_rect = Self::panel_rect(backend);
                let layout = backend.layout_primitive(panel_rect, &self.panel());
                match layout.hit_test(position.x, position.y) {
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

            UiEvent::WindowResized { .. } => Reaction::Redraw,
            _ => Reaction::Continue,
        }
    }
}
