//! Backend-agnostic Debug-sidebar consumer pattern using
//! [`SidebarSystem`] — the compose helper that handles MSV + TreeView
//! interaction (scroll, selection, keyboard nav, scrollbar drag).
//!
//! The app defines 4 sections, sets row data, and matches on
//! [`SidebarEvent`] for semantic actions. All interaction logic
//! (Tab cycling, arrow scrolling, click dispatch, scrollbar drag)
//! is handled internally by `SidebarSystem`.
//!
//! Controls:
//! - mouse click on header        activate that section
//! - mouse click on body row      activate + select
//! - mouse click on scrollbar     drag thumb / page via track
//! - `Tab` / `Shift+Tab`          cycle active section
//! - `↑` / `↓`                    scroll active section
//! - `Enter`                      select first row of active section
//! - `q` / `Esc`                  quit

use quadraui::{
    AppLogic, Backend, Color, Decoration, Key, NamedKey, NavigationMode, Reaction, Rect,
    SectionSize, SidebarEvent, SidebarSectionDef, SidebarSystem, StatusBar, StatusBarAction,
    StatusBarInteraction, StatusBarSegment, StyledText, TreeRow, UiEvent, WidgetId,
};

const STATUS_BAR_LINES: f32 = 1.5;

pub struct DebugSidebar {
    sidebar: SidebarSystem,
    status_interaction: StatusBarInteraction,
    last_action: String,
}

impl DebugSidebar {
    pub fn new() -> Self {
        let mut sidebar = SidebarSystem::new(vec![
            SidebarSectionDef::new("variables", "VARIABLES"),
            SidebarSectionDef::new("watch", "WATCH"),
            SidebarSectionDef::new("call-stack", "CALL STACK"),
            SidebarSectionDef::new("breakpoints", "BREAKPOINTS"),
        ]);
        sidebar.set_rows(0, fake_rows("v", 12));
        sidebar.set_rows(1, fake_rows("w", 8));
        sidebar.set_rows(2, fake_rows("frame", 5));
        sidebar.set_rows(3, fake_rows("bp", 0));
        sidebar.set_navigation_mode(NavigationMode::Selection);
        sidebar.set_active_section(Some(0));
        Self {
            sidebar,
            status_interaction: StatusBarInteraction::new(),
            last_action: "—".into(),
        }
    }

    fn sidebar_rect(backend: &dyn Backend) -> Rect {
        let viewport = backend.viewport();
        let status_h = backend.line_height() * STATUS_BAR_LINES;
        Rect::new(
            0.0,
            0.0,
            viewport.width,
            (viewport.height - status_h).max(0.0),
        )
    }

    fn status_rect(backend: &dyn Backend) -> Rect {
        let viewport = backend.viewport();
        let status_h = backend.line_height() * STATUS_BAR_LINES;
        Rect::new(
            0.0,
            (viewport.height - status_h).max(0.0),
            viewport.width,
            status_h,
        )
    }

    fn build_status_bar(&self) -> StatusBar {
        let active = match self.sidebar.active_section() {
            Some(i) => format!("section {i}"),
            None => "<none>".into(),
        };
        let fg = Color::rgb(220, 220, 220);
        let bg = Color::rgb(40, 40, 60);
        let btn_bg = Color::rgb(60, 60, 90);
        StatusBar {
            id: WidgetId::new("multi-tree-status"),
            left_segments: vec![
                StatusBarSegment {
                    text: format!(" active: {active}  last: {} ", self.last_action),
                    fg,
                    bg,
                    bold: false,
                    action_id: None,
                },
                StatusBarSegment {
                    text: " ▶ Run ".into(),
                    fg,
                    bg: btn_bg,
                    bold: true,
                    action_id: Some(WidgetId::new("run")),
                },
                StatusBarSegment {
                    text: " ■ Stop ".into(),
                    fg,
                    bg: btn_bg,
                    bold: true,
                    action_id: Some(WidgetId::new("stop")),
                },
            ],
            right_segments: vec![StatusBarSegment {
                text: " mouse / Tab / ↑↓ / Enter / r=edit / q ".into(),
                fg,
                bg,
                bold: false,
                action_id: None,
            }],
        }
    }
}

impl Default for DebugSidebar {
    fn default() -> Self {
        Self::new()
    }
}

impl AppLogic for DebugSidebar {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let sidebar = Self::sidebar_rect(backend);
        let status = Self::status_rect(backend);
        self.sidebar.render(backend, sidebar);
        let regions = backend.draw_status_bar(
            status,
            &self.build_status_bar(),
            self.status_interaction.hovered_id(),
            self.status_interaction.pressed_id(),
        );
        self.status_interaction.set_hit_regions(regions);
    }

    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
        let status_rect = Self::status_rect(backend);
        match self.status_interaction.handle(&event, status_rect) {
            StatusBarAction::Clicked(id) => {
                self.last_action = format!("btn:{}", id.as_str());
                return Reaction::Redraw;
            }
            StatusBarAction::Redraw => return Reaction::Redraw,
            StatusBarAction::Ignored => {}
        }

        let rect = Self::sidebar_rect(backend);
        match self.sidebar.handle(&event, backend, rect) {
            SidebarEvent::HeaderActivated { section } => {
                self.last_action = format!("header→{section}");
                Reaction::Redraw
            }
            SidebarEvent::RowSelected { section, path }
            | SidebarEvent::RowActivated { section, path } => {
                self.last_action = format!("row→{section} {path:?}");
                Reaction::Redraw
            }
            SidebarEvent::ContextMenuRequested {
                section,
                path,
                position,
            } => {
                self.last_action = format!(
                    "ctx→{section} {path:?} @({:.0},{:.0})",
                    position.x, position.y
                );
                Reaction::Redraw
            }
            SidebarEvent::EditConfirmed { section, path, .. } => {
                self.last_action = format!("edit-ok→{section} {path:?}");
                Reaction::Redraw
            }
            SidebarEvent::EditCancelled { section, path } => {
                self.last_action = format!("edit-cancel→{section} {path:?}");
                Reaction::Redraw
            }
            SidebarEvent::ScrollChanged { .. }
            | SidebarEvent::EditChanged { .. }
            | SidebarEvent::StateChanged
            | SidebarEvent::Consumed => Reaction::Redraw,
            SidebarEvent::Ignored => match event {
                UiEvent::KeyPressed {
                    key: Key::Char('r'),
                    ..
                } => {
                    if let Some(section) = self.sidebar.active_section() {
                        if let Some(path) = self.sidebar.selected_path(section).cloned() {
                            let label = format!("item{}", path.last().unwrap_or(&0));
                            let len = label.len();
                            self.sidebar
                                .start_editing(section, path, label, len, Some(0), None);
                        }
                    }
                    Reaction::Redraw
                }
                UiEvent::KeyPressed {
                    key: Key::Char('q'),
                    ..
                }
                | UiEvent::KeyPressed {
                    key: Key::Named(NamedKey::Escape),
                    ..
                } => Reaction::Exit,
                UiEvent::WindowResized { .. } => Reaction::Redraw,
                _ => Reaction::Continue,
            },
        }
    }
}

fn fake_rows(prefix: &str, n: usize) -> Vec<TreeRow> {
    (0..n)
        .map(|i| TreeRow {
            path: vec![i as u16],
            indent: 0,
            icon: None,
            text: StyledText::plain(format!("{prefix}{i}")),
            badge: None,
            is_expanded: None,
            decoration: Decoration::Normal,
            edit: None,
        })
        .collect()
}
