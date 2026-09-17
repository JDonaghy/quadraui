//! Demo: AppShell with a bottom panel tab strip plus an independently-gated
//! bottom band (issue #997).
//!
//! Demonstrates `ShellConfig.with_bottom_panel_config()`:
//! - Two tabs ("TERMINAL" and "PROBLEMS") with a `BackendWidget` each.
//! - Click a tab label to switch.
//! - Click `×` on "PROBLEMS" to close it.
//! - Click `^` to maximise/restore.
//! - Drag the resize grip (top edge of the panel) to change its height.
//!
//! Also demonstrates `ShellConfig.with_bottom_bands()` (#997): a one-line
//! "STATUS" band docked directly below the tabbed panel, independent of it
//! — closing/maximising the tabbed panel above does not affect the band,
//! and the band's own presence is gated separately:
//! - Press `s` to toggle the status band's visibility.
//!
//! `q` / Esc quits.

use quadraui::compose::app_shell::{AppShellEvent, AppShellLayout, BottomBand, PanelDefinition};
use quadraui::compose::bottom_panel::{
    BackendWidget, BottomPanelConfig, BottomPanelEvent, BottomPanelTab,
};
use quadraui::{
    Backend, Color, InteractionState, Key, NamedKey, Reaction, Rect, ShellApp, ShellConfig,
    ShellContext, StatusBar, StatusBarSegment, UiEvent, WidgetId,
};

/// Id of the demo's one `BottomBand` (issue #997).
pub fn status_band_id() -> WidgetId {
    WidgetId::new("band:status")
}

// ── Content widgets ───────────────────────────────────────────────────────────

/// Terminal-tab content: a fake "terminal" showing a prompt line.
pub struct TerminalContent {
    pub lines: Vec<String>,
}

impl BackendWidget for TerminalContent {
    fn render(&self, backend: &mut dyn Backend, rect: Rect) {
        if rect.height < 1.0 {
            return;
        }
        let lh = backend.line_height();
        let prompt = self
            .lines
            .last()
            .map(|l| format!("$ {l}"))
            .unwrap_or_else(|| "$ _".to_string());
        let bar = StatusBar {
            id: WidgetId::new("bp:terminal-content"),
            left_segments: vec![StatusBarSegment {
                text: format!(" {prompt} "),
                fg: Color::rgb(100, 200, 100),
                bg: Color::rgb(20, 20, 20),
                bold: false,
                action_id: None,
            }],
            right_segments: vec![],
        };
        backend.draw_status_bar_interactive(
            Rect::new(rect.x, rect.y, rect.width, lh),
            &bar,
            &InteractionState::new(),
        );
    }
}

/// Problems-tab content: a list of diagnostic messages.
pub struct ProblemsContent {
    pub problems: Vec<String>,
}

impl BackendWidget for ProblemsContent {
    fn render(&self, backend: &mut dyn Backend, rect: Rect) {
        if rect.height < 1.0 {
            return;
        }
        let lh = backend.line_height();
        for (i, problem) in self.problems.iter().enumerate() {
            let y = rect.y + i as f32 * lh;
            if y + lh > rect.y + rect.height {
                break;
            }
            let bar = StatusBar {
                id: WidgetId::new("bp:problems-row"),
                left_segments: vec![StatusBarSegment {
                    text: format!("  ⚠ {problem} "),
                    fg: Color::rgb(255, 180, 50),
                    bg: Color::rgb(25, 25, 25),
                    bold: false,
                    action_id: None,
                }],
                right_segments: vec![],
            };
            backend.draw_status_bar_interactive(
                Rect::new(rect.x, y, rect.width, lh),
                &bar,
                &InteractionState::new(),
            );
        }
    }
}

// ── App ───────────────────────────────────────────────────────────────────────

pub struct BottomPanelDemo {
    last_event: String,
    problems_visible: bool,
}

impl BottomPanelDemo {
    pub fn new() -> Self {
        Self {
            last_event: "click tabs | drag resize | ^ to maximise | q=quit".into(),
            problems_visible: true,
        }
    }

    pub fn config() -> ShellConfig {
        let tabs = vec![
            BottomPanelTab {
                id: "bp:terminal".into(),
                label: "TERMINAL".into(),
                closable: true,
                badge: None,
                content: Box::new(TerminalContent {
                    lines: vec!["echo hello".into()],
                }),
            },
            BottomPanelTab {
                id: "bp:problems".into(),
                label: "PROBLEMS".into(),
                closable: true,
                badge: Some("2".into()),
                content: Box::new(ProblemsContent {
                    problems: vec![
                        "src/main.rs:12 — unused variable `x`".into(),
                        "src/lib.rs:4 — missing semicolon".into(),
                    ],
                }),
            },
        ];
        // Ensure first tab is active.
        let active_tab_id = tabs[0].id.clone();

        ShellConfig::new(
            "BottomPanel Demo",
            vec![PanelDefinition {
                id: WidgetId::new("panel:explorer"),
                icon: "E".into(),
                tooltip: "Explorer".into(),
                title: "EXPLORER".into(),
            }],
        )
        .with_bottom_panel_config(BottomPanelConfig {
            tabs,
            active_tab_id,
            maximised: false,
            height_fraction: 0.3,
        })
        // #997: an independently-gated band, separate from the tabbed
        // panel above — closing/maximising the panel doesn't touch this,
        // and its own visibility toggles independently (see `handle`'s
        // `s` binding below).
        .with_bottom_bands(vec![BottomBand::new(status_band_id(), 1.0)])
    }
}

impl Default for BottomPanelDemo {
    fn default() -> Self {
        Self::new()
    }
}

impl ShellApp for BottomPanelDemo {
    fn render_content(&self, backend: &mut dyn Backend, layout: &AppShellLayout) {
        let lh = backend.line_height();

        // Sidebar placeholder.
        if let Some(sb) = layout.sidebar_content_bounds {
            let bar = StatusBar {
                id: WidgetId::new("bp-demo:sidebar"),
                left_segments: vec![StatusBarSegment {
                    text: " (sidebar) ".into(),
                    fg: Color::rgb(140, 140, 140),
                    bg: Color::rgb(30, 30, 30),
                    bold: false,
                    action_id: None,
                }],
                right_segments: vec![],
            };
            backend.draw_status_bar_interactive(
                Rect::new(sb.x, sb.y, sb.width, lh),
                &bar,
                &InteractionState::new(),
            );
        }

        // Main content area: show the last event.
        let main = layout.main_content_bounds;
        if main.height > 0.0 {
            let bar = StatusBar {
                id: WidgetId::new("bp-demo:main"),
                left_segments: vec![StatusBarSegment {
                    text: format!(" {} ", self.last_event),
                    fg: Color::rgb(200, 200, 200),
                    bg: Color::rgb(30, 30, 30),
                    bold: false,
                    action_id: None,
                }],
                right_segments: vec![],
            };
            backend.draw_status_bar_interactive(
                Rect::new(main.x, main.y, main.width, lh),
                &bar,
                &InteractionState::new(),
            );
        }
    }

    // #997: the independently-gated status band, when visible. A separate
    // `ShellApp` hook (not read off `layout` in `render_content` above) —
    // see `render_bottom_band`'s doc for why bottom-band bounds aren't a
    // field on `AppShellLayout`.
    fn render_bottom_band(&self, backend: &mut dyn Backend, id: &WidgetId, bounds: Rect) {
        if *id != status_band_id() {
            return;
        }
        let bar = StatusBar {
            id: WidgetId::new("bp-demo:status-band"),
            left_segments: vec![StatusBarSegment {
                text: " STATUS: everything ok (press s to toggle) ".into(),
                fg: Color::rgb(230, 230, 230),
                bg: Color::rgb(40, 60, 40),
                bold: false,
                action_id: None,
            }],
            right_segments: vec![],
        };
        backend.draw_status_bar_interactive(bounds, &bar, &InteractionState::new());
    }

    fn handle(
        &mut self,
        event: UiEvent,
        _backend: &mut dyn Backend,
        ctx: &ShellContext,
    ) -> Reaction {
        match &event {
            UiEvent::KeyPressed {
                key: Key::Char('q') | Key::Named(NamedKey::Escape),
                ..
            } => Reaction::Exit,
            UiEvent::KeyPressed {
                key: Key::Char('s'),
                ..
            } => {
                let id = status_band_id();
                let currently_visible = ctx
                    .shell()
                    .bottom_band(&id)
                    .map(|b| b.visible)
                    .unwrap_or(false);
                ctx.shell_mut()
                    .set_bottom_band_visible(&id, !currently_visible);
                Reaction::Redraw
            }
            _ => Reaction::Continue,
        }
    }

    fn on_shell_event_ctx(&mut self, event: &AppShellEvent, _ctx: &ShellContext) {
        self.last_event = match event {
            AppShellEvent::PanelChanged { panel_id } => {
                format!("Panel: {}", panel_id.as_str())
            }
            AppShellEvent::SidebarResized { new_width } => {
                format!("Sidebar resized: {new_width:.0}")
            }
            AppShellEvent::BottomPanelResized { new_height } => {
                format!("Panel resized: {new_height:.0}")
            }
            _ => return,
        };
    }

    fn on_bottom_panel_event(&mut self, event: &BottomPanelEvent) {
        self.last_event = match event {
            BottomPanelEvent::TabActivated(id) => format!("Tab activated: {id}"),
            BottomPanelEvent::TabClosed(id) => {
                self.problems_visible = false;
                format!("Tab closed: {id}")
            }
            BottomPanelEvent::MaximiseToggled => "Maximise toggled".into(),
            BottomPanelEvent::Resized(h) => format!("Panel resized: {h:.0}"),
        };
    }
}
