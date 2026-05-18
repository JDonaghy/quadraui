//! Backend-agnostic app code for the AppShell example
//! ([`tui_shell`] / [`gtk_shell`]).
//!
//! Demonstrates a VS Code-style app shell with:
//! - 3 sidebar panels (Explorer, Search, Source Control) each backed
//!   by a [`SidebarSystem`] with real tree rows
//! - 1 bottom activity bar item (Settings)
//! - Toggle/switch/resize behaviour driven by [`AppShell`]
//! - Status bar showing last action
//!
//! Controls:
//! - click activity bar icons   toggle / switch panels
//! - drag divider               resize sidebar
//! - click sidebar rows         select row
//! - `Tab` / `Shift+Tab`        cycle sidebar sections
//! - `↑` / `↓`                  scroll / select in sidebar
//! - `q` / `Esc`                quit

use quadraui::compose::app_shell::{AppShell, AppShellEvent, PanelDefinition};
use quadraui::{
    AppLogic, Backend, Color, Decoration, Key, NamedKey, NavigationMode, Reaction, Rect,
    SectionSize, SidebarEvent, SidebarSectionDef, SidebarSystem, StatusBar, StatusBarSegment,
    StyledText, TreePath, TreeRow, UiEvent, WidgetId,
};

pub struct ShellApp {
    shell: AppShell,
    explorer: SidebarSystem,
    search: SidebarSystem,
    git: SidebarSystem,
    last_message: String,
}

impl ShellApp {
    pub fn new() -> Self {
        let panels = vec![
            PanelDefinition {
                id: WidgetId::new("panel:explorer"),
                icon: "E".into(),
                tooltip: "Explorer".into(),
                title: "EXPLORER".into(),
            },
            PanelDefinition {
                id: WidgetId::new("panel:search"),
                icon: "S".into(),
                tooltip: "Search".into(),
                title: "SEARCH".into(),
            },
            PanelDefinition {
                id: WidgetId::new("panel:git"),
                icon: "G".into(),
                tooltip: "Source Control".into(),
                title: "SOURCE CONTROL".into(),
            },
        ];
        let bottom = vec![PanelDefinition {
            id: WidgetId::new("panel:settings"),
            icon: "*".into(),
            tooltip: "Settings".into(),
            title: "Settings".into(),
        }];

        // All widths are in line_height multiples — portable across
        // TUI (1 lh = 1 cell) and GTK (1 lh ≈ 16px).
        let shell = AppShell::new(panels, 20.0)
            .with_bottom_items(bottom)
            .with_min_width(8.0)
            .with_max_width(50.0)
            .with_activity_bar_width(3.0);

        let explorer = build_explorer_sidebar();
        let search = build_search_sidebar();
        let git = build_git_sidebar();

        Self {
            shell,
            explorer,
            search,
            git,
            last_message: "Click activity bar icons | drag divider | q to quit".into(),
        }
    }

    fn active_sidebar_mut(&mut self) -> Option<&mut SidebarSystem> {
        let id = self.shell.active_panel_id()?;
        match id.as_str() {
            "panel:explorer" => Some(&mut self.explorer),
            "panel:search" => Some(&mut self.search),
            "panel:git" => Some(&mut self.git),
            _ => None,
        }
    }

    fn active_sidebar(&self) -> Option<&SidebarSystem> {
        let id = self.shell.active_panel_id()?;
        match id.as_str() {
            "panel:explorer" => Some(&self.explorer),
            "panel:search" => Some(&self.search),
            "panel:git" => Some(&self.git),
            _ => None,
        }
    }

    fn status_bar(&self) -> StatusBar {
        let fg = Color::rgb(220, 220, 220);
        let bg = Color::rgb(0, 122, 204);
        let dim_bg = Color::rgb(30, 30, 40);
        StatusBar {
            id: WidgetId::new("shell-status"),
            left_segments: vec![StatusBarSegment {
                text: format!(" {} ", self.last_message),
                fg,
                bg,
                bold: false,
                action_id: None,
            }],
            right_segments: vec![StatusBarSegment {
                text: " q=quit | click icons | drag divider ".into(),
                fg: Color::rgb(180, 180, 180),
                bg: dim_bg,
                bold: false,
                action_id: None,
            }],
        }
    }
}

impl Default for ShellApp {
    fn default() -> Self {
        Self::new()
    }
}

impl AppLogic for ShellApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let viewport = backend.viewport();
        let lh = backend.line_height();
        let status_h = lh;
        let shell_area = Rect::new(0.0, 0.0, viewport.width, viewport.height - status_h);
        let status_area = Rect::new(0.0, viewport.height - status_h, viewport.width, status_h);

        let layout = self.shell.render(backend, shell_area);

        if let Some(sidebar_bounds) = layout.sidebar_content_bounds {
            if self.shell.sidebar_visible() {
                if let Some(sidebar) = self.active_sidebar() {
                    sidebar.render(backend, sidebar_bounds);
                }
            }
        }

        // Main content: show a label in the content area.
        let selected_info = self
            .active_sidebar()
            .and_then(|s| {
                let section = s.active_section()?;
                let path = s.selected_path(section)?;
                Some(format!("section {section}, row {:?}", path))
            })
            .unwrap_or_else(|| "nothing selected".into());
        let main_label = StatusBar {
            id: WidgetId::new("main-label"),
            left_segments: vec![StatusBarSegment {
                text: format!(" Selected: {} ", selected_info),
                fg: Color::rgb(160, 160, 160),
                bg: Color::rgb(30, 30, 30),
                bold: false,
                action_id: None,
            }],
            right_segments: vec![],
        };
        let label_rect = Rect::new(
            layout.main_content_bounds.x,
            layout.main_content_bounds.y,
            layout.main_content_bounds.width,
            lh,
        );
        let _ = backend.draw_status_bar(label_rect, &main_label, None, None);

        let _ = backend.draw_status_bar(status_area, &self.status_bar(), None, None);
    }

    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
        let viewport = backend.viewport();
        let lh = backend.line_height();
        let status_h = lh;
        let shell_area = Rect::new(0.0, 0.0, viewport.width, viewport.height - status_h);

        match self.shell.handle(&event, backend, shell_area) {
            AppShellEvent::PanelChanged { panel_id } => {
                self.last_message = format!("Panel: {}", panel_id.as_str());
                return Reaction::Redraw;
            }
            AppShellEvent::SidebarHidden => {
                self.last_message = "Sidebar hidden".into();
                return Reaction::Redraw;
            }
            AppShellEvent::SidebarResized { new_width } => {
                self.last_message = format!("Sidebar width: {:.0}", new_width);
                return Reaction::Redraw;
            }
            AppShellEvent::BottomPanelResized { new_height } => {
                self.last_message = format!("Bottom panel: {new_height:.0}px");
                return Reaction::Redraw;
            }
            AppShellEvent::BottomPanelHidden => {
                self.last_message = "Bottom panel hidden".into();
                return Reaction::Redraw;
            }
            AppShellEvent::BottomItemClicked { id } => {
                self.last_message = format!("Settings clicked ({})", id.as_str());
                return Reaction::Redraw;
            }
            AppShellEvent::Consumed => return Reaction::Redraw,
            AppShellEvent::Ignored => {}
        }

        // Forward events to the active sidebar panel.
        if self.shell.sidebar_visible() {
            let layout = self.shell.layout(shell_area, lh);
            if let Some(sidebar_bounds) = layout.sidebar_content_bounds {
                if let Some(sidebar) = self.active_sidebar_mut() {
                    match sidebar.handle(&event, backend, sidebar_bounds) {
                        SidebarEvent::RowSelected { section, path } => {
                            self.last_message =
                                format!("Selected: section {section}, row {path:?}");
                            return Reaction::Redraw;
                        }
                        SidebarEvent::HeaderActivated { section } => {
                            self.last_message = format!("Header: section {section}");
                            return Reaction::Redraw;
                        }
                        SidebarEvent::StateChanged | SidebarEvent::Consumed => {
                            return Reaction::Redraw;
                        }
                        SidebarEvent::ScrollChanged { .. } => return Reaction::Redraw,
                        _ => {}
                    }
                }
            }
        }

        match event {
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
        }
    }
}

// ── Sidebar builders ─────────────────────────────────────────────────

fn build_explorer_sidebar() -> SidebarSystem {
    let mut sidebar = SidebarSystem::new(vec![
        SidebarSectionDef::new("open-editors", "OPEN EDITORS"),
        SidebarSectionDef::new("project", "PROJECT"),
    ]);
    sidebar.set_rows(
        0,
        vec![
            file_row(&[0], 0, "main.rs"),
            file_row(&[1], 0, "lib.rs"),
            file_row(&[2], 0, "Cargo.toml"),
        ],
    );
    sidebar.set_rows(
        1,
        vec![
            dir_row(&[0], 0, "src/"),
            file_row(&[0, 0], 1, "main.rs"),
            file_row(&[0, 1], 1, "lib.rs"),
            file_row(&[0, 2], 1, "backend.rs"),
            file_row(&[0, 3], 1, "types.rs"),
            dir_row(&[1], 0, "tests/"),
            file_row(&[1, 0], 1, "integration.rs"),
            file_row(&[1, 1], 1, "smoke.rs"),
            dir_row(&[2], 0, "examples/"),
            file_row(&[2, 0], 1, "demo.rs"),
            file_row(&[2, 1], 1, "shell.rs"),
            file_row(&[3], 0, "Cargo.toml"),
            file_row(&[4], 0, "README.md"),
        ],
    );
    sidebar.set_navigation_mode(NavigationMode::Selection);
    sidebar.set_active_section(Some(1));
    sidebar
}

fn build_search_sidebar() -> SidebarSystem {
    let mut sidebar = SidebarSystem::new(vec![
        SidebarSectionDef {
            id: "results".into(),
            title: "RESULTS".into(),
            show_chevron: false,
            size: SectionSize::EqualShare,
            kind: quadraui::SectionKind::Tree,
        },
        SidebarSectionDef {
            id: "recent".into(),
            title: "RECENT SEARCHES".into(),
            show_chevron: false,
            size: SectionSize::EqualShare,
            kind: quadraui::SectionKind::Tree,
        },
    ]);
    sidebar.set_rows(
        0,
        vec![
            file_row(&[0], 0, "src/main.rs — 3 matches"),
            file_row(&[1], 0, "src/lib.rs — 1 match"),
            file_row(&[2], 0, "tests/smoke.rs — 2 matches"),
        ],
    );
    sidebar.set_rows(
        1,
        vec![
            file_row(&[0], 0, "\"AppShell\""),
            file_row(&[1], 0, "\"draw_activity_bar\""),
            file_row(&[2], 0, "\"SidebarSystem\""),
        ],
    );
    sidebar.set_navigation_mode(NavigationMode::Selection);
    sidebar.set_active_section(Some(0));
    sidebar
}

fn build_git_sidebar() -> SidebarSystem {
    let mut sidebar = SidebarSystem::new(vec![
        SidebarSectionDef::new("staged", "STAGED CHANGES"),
        SidebarSectionDef::new("changes", "CHANGES"),
    ]);
    sidebar.set_rows(
        0,
        vec![
            file_row(&[0], 0, "M compose/app_shell.rs"),
            file_row(&[1], 0, "A tui/activity_bar.rs"),
        ],
    );
    sidebar.set_rows(
        1,
        vec![
            file_row(&[0], 0, "M compose/mod.rs"),
            file_row(&[1], 0, "M lib.rs"),
            file_row(&[2], 0, "M tui/backend.rs"),
            file_row(&[3], 0, "M tui/mod.rs"),
            file_row(&[4], 0, "A examples/shell_app.rs"),
        ],
    );
    sidebar.set_navigation_mode(NavigationMode::Selection);
    sidebar.set_active_section(Some(1));
    sidebar
}

fn file_row(path: &[u16], indent: u16, text: &str) -> TreeRow {
    TreeRow {
        path: path.to_vec(),
        indent,
        icon: None,
        text: StyledText::plain(text),
        badge: None,
        is_expanded: None,
        decoration: Decoration::Normal,
        edit: None,
    }
}

fn dir_row(path: &[u16], indent: u16, text: &str) -> TreeRow {
    TreeRow {
        path: path.to_vec(),
        indent,
        icon: None,
        text: StyledText::plain(text),
        badge: None,
        is_expanded: Some(true),
        decoration: Decoration::Header,
        edit: None,
    }
}
