//! Backend-agnostic app code for the shell + `MenuSystem` regression demo
//! ([`tui_shell_menu`] / [`gtk_shell_menu`]).
//!
//! Reproduces issue #411: under `run_with_shell`, an app-level modal that
//! visually overlaps `AppShell` chrome (activity bar, sidebar, dividers,
//! bottom-panel grip) must still receive its `MouseDown` — the chrome must
//! not swallow it via pure position-based hit-testing just because the
//! click's on-screen coordinates happen to land inside a chrome region.
//!
//! The menu bar lives in the shell's title-bar band (`ShellConfig::
//! with_title_bar`) — a plain click on "File" reaches the app normally,
//! since `AppShell::handle` doesn't intercept title-bar clicks. But the
//! "File" menu's *dropdown* opens directly below the bar, at the same x
//! as the leftmost bar item — which puts it squarely over the activity
//! bar strip underneath, exactly like vimcode's `MenuSystem` dropdown
//! overlapping its activity bar (vimcode#552, the bug that motivated this
//! issue). Before the #411 fix, clicking a dropdown item whose position
//! fell inside the activity bar's bounds silently did nothing —
//! `AppShell::handle` consumed the `MouseDown` as an activity-bar click
//! before the app ever saw it.
//!
//! Controls:
//! - click "File" (or Alt+F)    opens the dropdown over the activity bar
//! - click a dropdown item      activates it, even items drawn over the
//!                               activity bar strip — this is the #411
//!                               regression check
//! - click an activity bar icon  switches panels (when no dropdown is open)
//! - q / Esc                     quit (when no dropdown is open)

use quadraui::compose::app_shell::{AppShellEvent, AppShellLayout, PanelDefinition};
use quadraui::{
    Backend, Color, ContextMenuItem, Key, MenuDef, MenuEvent, MenuSystem, NamedKey, Reaction, Rect,
    ShellApp as ShellAppTrait, ShellConfig, ShellContext, StatusBar, StatusBarSegment, StyledText,
    UiEvent, WidgetId,
};

pub struct ShellMenuDemo {
    menu_system: MenuSystem,
    last_action: String,
}

impl ShellMenuDemo {
    pub fn new() -> Self {
        Self {
            menu_system: MenuSystem::new(vec![
                MenuDef {
                    id: WidgetId::new("file"),
                    label: "&File".into(),
                    disabled: false,
                    items: vec![
                        action("new", "New File"),
                        action("open", "Open File"),
                        separator(),
                        action("quit", "Quit"),
                    ],
                },
                MenuDef {
                    id: WidgetId::new("edit"),
                    label: "&Edit".into(),
                    disabled: false,
                    items: vec![action("undo", "Undo"), action("redo", "Redo")],
                },
            ]),
            last_action: "click File, then click an item over the activity bar".into(),
        }
    }

    pub fn config() -> ShellConfig {
        ShellConfig::new(
            "Shell Menu Demo",
            vec![
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
            ],
        )
        // A title-bar band to host the menu bar. `AppShell::handle` doesn't
        // intercept title-bar clicks, so opening the menu always reaches
        // the app — it's the dropdown *below* it, anchored at the same x
        // as the "File" bar item, that overlaps the activity bar strip
        // (still 3 line-heights wide, `ShellConfig`'s default) beneath it.
        .with_title_bar(1.0)
    }
}

impl Default for ShellMenuDemo {
    fn default() -> Self {
        Self::new()
    }
}

impl ShellAppTrait for ShellMenuDemo {
    fn render_content(&self, backend: &mut dyn Backend, layout: &AppShellLayout) {
        if let Some(bar_rect) = layout.title_bar_bounds {
            self.menu_system.render(backend, bar_rect);
        }

        let lh = backend.line_height();
        let status = StatusBar {
            id: WidgetId::new("shell-menu-status"),
            left_segments: vec![StatusBarSegment {
                text: format!(" {} ", self.last_action),
                fg: Color::rgb(200, 200, 200),
                bg: Color::rgb(30, 30, 30),
                bold: false,
                action_id: None,
            }],
            right_segments: vec![],
        };
        let status_rect = Rect::new(
            layout.main_content_bounds.x,
            layout.main_content_bounds.y,
            layout.main_content_bounds.width,
            lh.min(layout.main_content_bounds.height),
        );
        let _ = backend.draw_status_bar(status_rect, &status, None, None);
    }

    fn handle(
        &mut self,
        event: UiEvent,
        backend: &mut dyn Backend,
        ctx: &ShellContext,
    ) -> Reaction {
        let Some(bar_rect) = ctx.title_bar_bounds() else {
            return Reaction::Continue;
        };

        match self.menu_system.handle(&event, backend, bar_rect) {
            MenuEvent::Activated(id) => {
                self.last_action = format!("activated: {}", id.as_str());
                if id.as_str() == "quit" {
                    return Reaction::Exit;
                }
                Reaction::Redraw
            }
            MenuEvent::StateChanged | MenuEvent::Consumed => Reaction::Redraw,
            MenuEvent::Ignored => match event {
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

    fn on_shell_event(&mut self, event: &AppShellEvent) {
        match event {
            AppShellEvent::PanelChanged { panel_id } => {
                self.last_action = format!("Panel: {}", panel_id.as_str());
            }
            AppShellEvent::SidebarHidden => {
                self.last_action = "Sidebar hidden".into();
            }
            _ => {}
        }
    }
}

fn action(id: &str, label: &str) -> ContextMenuItem {
    ContextMenuItem {
        id: Some(WidgetId::new(id)),
        label: StyledText::plain(label),
        ..Default::default()
    }
}

fn separator() -> ContextMenuItem {
    ContextMenuItem::default()
}
