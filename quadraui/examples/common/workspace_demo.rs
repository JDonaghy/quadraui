//! [`WorkspaceController`] demo — an open-N-view-one document set living
//! **inside a panel**, not in the shell's chrome (quadraui#596).
//!
//! This is the shape #596 split out of #469: the controller renders its
//! own tab strip into whatever rect the host hands it, so a consumer can
//! put one inside a single `AppShell` sidebar panel and paint the active
//! document's body somewhere else entirely. No shell-level tab-strip slot
//! is involved — that half stays in #469.
//!
//! The workspace is mounted into [`AppShellLayout::sidebar_content_bounds`]
//! (the panel's rect) and the active document's "body" is painted into
//! [`AppShellLayout::main_content_bounds`], which is exactly the split the
//! controller's no-content-ownership design buys: `WorkspaceDoc` is an
//! opaque id plus a label, and this app paints the body itself from its
//! own state — something
//! [`TabGroupController`](quadraui::TabGroupController)'s
//! `Box<dyn BackendWidget>` (`Send + 'static`) content could not do.
//!
//! Controls:
//! - click a tab            activate that document
//! - click a tab's `×`      close that document
//! - `Ctrl+Tab`             next document (wraps)
//! - `Ctrl+Shift+Tab`       previous document (wraps)
//! - `Ctrl+PageDown/PageUp` same, VS Code's other spelling
//! - `o`                    open the next document from the backlog
//! - `q` / `Esc`            quit

use std::cell::RefCell;

use quadraui::compose::app_shell::{AppShellLayout, PanelDefinition};
use quadraui::{
    Backend, Color, Key, NamedKey, Reaction, Rect, ShellApp, ShellConfig, ShellContext, StatusBar,
    StatusBarSegment, UiEvent, WidgetId, WorkspaceController, WorkspaceDoc, WorkspaceEvent,
};

/// The three documents the demo starts with, `(id, label)`. Labels are
/// deliberately short so all three fit the sidebar without overflow —
/// [`BACKLOG`] is what a test uses to *force* overflow.
pub const INITIAL: [(&str, &str); 3] = [
    ("doc:alpha", "alpha"),
    ("doc:beta", "beta"),
    ("doc:gamma", "gamma"),
];

/// Documents `o` opens, in order. Long enough that a few of them push the
/// strip past the sidebar's width and exercise
/// [`quadraui::TabBar::fit_active_scroll_offset`] through the controller.
pub const BACKLOG: [(&str, &str); 4] = [
    ("doc:delta", "delta-doc"),
    ("doc:epsilon", "epsilon-doc"),
    ("doc:zeta", "zeta-doc"),
    ("doc:eta", "eta-doc"),
];

/// [`WidgetId`] the workspace's tab strip paints under — what a driver
/// test passes to `TuiDriver::tab_center` / `GtkDriver::tab_center`.
pub const BAR_ID: &str = "workspace-demo:tabs";

pub struct WorkspaceDemo {
    /// `RefCell` so `render_content(&self, …)` can call the controller's
    /// `&mut self` render — the same pattern `TabGroupDemo` uses.
    workspace: RefCell<WorkspaceController>,
    /// Index of the next [`BACKLOG`] entry `o` will open.
    next_backlog: usize,
    last_event: String,
}

impl WorkspaceDemo {
    pub fn new() -> Self {
        let mut workspace = WorkspaceController::new(BAR_ID);
        for (id, label) in INITIAL {
            workspace.open(WorkspaceDoc::new(id, label));
        }
        // Start on the first document rather than the last one opened, so
        // the demo's initial screen reads left-to-right.
        workspace.activate(INITIAL[0].0);
        Self {
            workspace: RefCell::new(workspace),
            next_backlog: 0,
            last_event: "ready".to_string(),
        }
    }

    pub fn config() -> ShellConfig {
        ShellConfig::new(
            "Workspace Demo",
            vec![PanelDefinition {
                id: WidgetId::new("panel:docs"),
                icon: "D".into(),
                tooltip: "Documents".into(),
                title: "DOCUMENTS".into(),
            }],
        )
    }

    /// Human-readable one-liner for the hint bar — the driver tests'
    /// window onto "activated" vs "closed", which the tab strip's glyphs
    /// alone cannot distinguish.
    fn describe(event: &WorkspaceEvent) -> String {
        match event {
            WorkspaceEvent::Opened { id, .. } => format!("opened {id}"),
            WorkspaceEvent::Activated { id, .. } => format!("activated {id}"),
            WorkspaceEvent::Closed { id, .. } => format!("closed {id}"),
            WorkspaceEvent::Reordered { id, from, to } => {
                format!("reordered {id} {from}->{to}")
            }
            _ => "unknown workspace event".to_string(),
        }
    }

    fn record(&mut self, events: &[WorkspaceEvent]) -> Reaction {
        if events.is_empty() {
            return Reaction::Continue;
        }
        self.last_event = events
            .iter()
            .map(Self::describe)
            .collect::<Vec<_>>()
            .join(", ");
        Reaction::Redraw
    }

    fn open_next_backlog_doc(&mut self) -> Reaction {
        let Some((id, label)) = BACKLOG.get(self.next_backlog).copied() else {
            self.last_event = "backlog exhausted".to_string();
            return Reaction::Redraw;
        };
        self.next_backlog += 1;
        let event = self
            .workspace
            .borrow_mut()
            .open(WorkspaceDoc::new(id, label));
        match event {
            Some(ev) => self.record(&[ev]),
            None => Reaction::Redraw,
        }
    }

    fn label_bar(id: &str, text: String, fg: Color) -> StatusBar {
        StatusBar {
            id: WidgetId::new(id),
            left_segments: vec![StatusBarSegment {
                text,
                fg,
                bg: Color::rgb(30, 30, 30),
                bold: false,
                action_id: None,
            }],
            right_segments: vec![],
        }
    }
}

impl Default for WorkspaceDemo {
    fn default() -> Self {
        Self::new()
    }
}

impl ShellApp for WorkspaceDemo {
    fn render_content(&self, backend: &mut dyn Backend, layout: &AppShellLayout) {
        let lh = backend.line_height();

        // The workspace lives *inside the panel* — its tab strip is
        // painted into the sidebar's content rect, not into shell chrome.
        if let Some(panel) = layout.sidebar_content_bounds {
            self.workspace.borrow_mut().render(backend, panel);
        }

        // The host paints the active document's body itself, from its own
        // state — the whole point of the controller owning no content.
        let main = layout.main_content_bounds;
        let ws = self.workspace.borrow();
        let body = match ws.active_id() {
            Some(id) => format!(" viewing: {id} "),
            None => " viewing: (no documents open) ".to_string(),
        };
        backend.draw_status_bar(
            Rect::new(main.x, main.y, main.width, lh),
            &Self::label_bar("workspace-demo:body", body, Color::rgb(220, 220, 220)),
            None,
            None,
        );
        backend.draw_status_bar(
            Rect::new(main.x, main.y + lh, main.width, lh),
            &Self::label_bar(
                "workspace-demo:hint",
                format!(" last: {} ", self.last_event),
                Color::rgb(150, 200, 150),
            ),
            None,
            None,
        );
    }

    fn handle(
        &mut self,
        event: UiEvent,
        _backend: &mut dyn Backend,
        _ctx: &ShellContext,
    ) -> Reaction {
        match event {
            UiEvent::KeyPressed {
                key: Key::Char('q') | Key::Named(NamedKey::Escape),
                ..
            } => Reaction::Exit,
            UiEvent::KeyPressed {
                key: Key::Char('o'),
                ..
            } => self.open_next_backlog_doc(),
            UiEvent::KeyPressed { key, modifiers, .. } => {
                // The controller owns its own key table (Ctrl+Tab,
                // Ctrl+PageUp/PageDown); anything it declines falls
                // through to the rest of the app unchanged.
                let handled = self.workspace.borrow_mut().handle_key(&key, modifiers);
                match handled {
                    Some(ev) => self.record(&[ev]),
                    None => Reaction::Continue,
                }
            }
            UiEvent::MouseDown { position, .. } => {
                let events = self
                    .workspace
                    .borrow_mut()
                    .handle_click(position.x, position.y);
                self.record(&events)
            }
            _ => Reaction::Continue,
        }
    }
}
