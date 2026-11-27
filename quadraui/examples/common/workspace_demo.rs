//! [`Workspace`] demo — an open-N-view-one document set living **inside
//! a panel**, not in the shell's chrome.
//!
//! The workspace paints its tab strip into whatever rect the host hands
//! it, so a consumer can put one inside a single `ShellView` sidebar panel
//! and paint the active document's body somewhere else entirely. No
//! shell-level tab-strip slot is involved.
//!
//! The workspace is mounted into [`ShellLayout::sidebar_content_bounds`]
//! (the panel's rect) and the active document's "body" is painted into
//! [`ShellLayout::main_content_bounds`]. `WorkspaceDoc` is an opaque id
//! plus a label, and this app paints the body itself from its own state.
//!
//! The strip is registered in the app's [`ShellApp::frame_tree`], so the
//! runtime routes clicks on it through `Workspace`'s own `Interactive`
//! and delivers what it reports as `UiEvent::Workspace`, which this app
//! applies with [`Workspace::apply`].
//!
//! Controls:
//! - click a tab            activate that document (and promote it, if preview)
//! - middle-click a tab     close that document
//! - click a tab's `×`      close that document
//! - `Ctrl+Tab`             next document (wraps, never promotes)
//! - `Ctrl+Shift+Tab`       previous document (wraps)
//! - `Ctrl+PageDown/PageUp` same, VS Code's other spelling
//! - `o`                    open the next document from the backlog, permanent
//! - `p`                    open the next document from the backlog, as **preview**
//! - `m`                    promote ("mark permanent"/pin) the active document
//! - `q` / `Esc`            quit
//!
//! `p` demonstrates the preview tier: each press replaces the previous
//! preview tab in place rather than accumulating tabs, and the preview tab
//! paints italic (`TabItem::is_preview`). `m` and clicking the preview tab
//! both promote it — the six-trigger table lives on
//! [`quadraui::compose::workspace`]'s module doc.

use quadraui::DrawsPrimitives;
use std::cell::Cell;

use quadraui::compose::app_shell::{PanelDefinition, ShellLayout};
use quadraui::{
    Backend, Color, FrameTree, Key, NamedKey, Reaction, Rect, ShellApp, ShellConfig, ShellContext,
    StatusBar, StatusBarSegment, UiEvent, WidgetId, Workspace, WorkspaceDoc, WorkspaceEvent,
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
/// [`quadraui::TabBar::fit_active_scroll_offset`] through the workspace.
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
    workspace: Workspace,
    /// The panel rect the last `render_content` painted the strip into,
    /// which [`ShellApp::frame_tree`] routes clicks against.
    panel: Cell<Option<Rect>>,
    /// Index of the next [`BACKLOG`] entry `o` will open.
    next_backlog: usize,
    /// Index of the next [`BACKLOG`] entry `p` will open as preview —
    /// tracked separately from `next_backlog` so the two keys don't
    /// fight over the same cursor.
    next_preview_backlog: usize,
    last_event: String,
}

impl WorkspaceDemo {
    pub fn new() -> Self {
        let mut workspace = Workspace::new(WidgetId::new(BAR_ID));
        for (id, label) in INITIAL {
            workspace.open(WorkspaceDoc::new(id, label));
        }
        // Start on the first document rather than the last one opened, so
        // the demo's initial screen reads left-to-right.
        workspace.activate(INITIAL[0].0);
        Self {
            workspace,
            panel: Cell::new(None),
            next_backlog: 0,
            next_preview_backlog: 0,
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
            WorkspaceEvent::Promoted { id } => format!("promoted {id}"),
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
        let events = self.workspace.open(WorkspaceDoc::new(id, label));
        self.record(&events)
    }

    /// `p`: open the next backlog document as the workspace's single
    /// **preview** — each press replaces whatever was previewed before,
    /// so the tab count only grows the first time.
    fn open_next_preview_doc(&mut self) -> Reaction {
        let Some((id, label)) = BACKLOG.get(self.next_preview_backlog).copied() else {
            self.last_event = "preview backlog exhausted".to_string();
            return Reaction::Redraw;
        };
        self.next_preview_backlog += 1;
        let events = self.workspace.open_preview(WorkspaceDoc::new(id, label));
        self.record(&events)
    }

    /// `m`: promote ("mark permanent") the active document — the
    /// explicit-pin trigger from the module doc's promotion table. A
    /// no-op when the active document isn't the preview.
    fn promote_active(&mut self) -> Reaction {
        let Some(id) = self.workspace.active_id().map(str::to_string) else {
            return Reaction::Continue;
        };
        match self.workspace.promote(&id) {
            Some(ev) => self.record(&[ev]),
            None => Reaction::Continue,
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
    fn render_content(&self, backend: &mut dyn Backend, layout: &ShellLayout) {
        let lh = backend.line_height();

        // The workspace lives *inside the panel* — its tab strip is
        // painted into the sidebar's content rect, not into shell chrome.
        self.panel.set(layout.sidebar_content_bounds);
        if let Some(panel) = layout.sidebar_content_bounds {
            backend.draw_dyn(panel, &self.workspace, &());
        }

        // The host paints the active document's body itself, from its own
        // state.
        let main = layout.main_content_bounds;
        let ws = &self.workspace;
        let body = match ws.active_id() {
            Some(id) if ws.is_preview(id) => format!(" viewing: {id} (preview) "),
            Some(id) => format!(" viewing: {id} "),
            None => " viewing: (no documents open) ".to_string(),
        };
        backend.draw_dyn(
            Rect::new(main.x, main.y, main.width, lh),
            &Self::label_bar("workspace-demo:body", body, Color::rgb(220, 220, 220)),
            &(),
        );
        backend.draw_dyn(
            Rect::new(main.x, main.y + lh, main.width, lh),
            &Self::label_bar(
                "workspace-demo:hint",
                format!(" last: {} ", self.last_event),
                Color::rgb(150, 200, 150),
            ),
            &(),
        );
    }

    fn frame_tree(&self) -> FrameTree {
        let mut tree = FrameTree::new();
        if let Some(panel) = self.panel.get() {
            tree.push(self.workspace.id, panel, Box::new(self.workspace.clone()));
        }
        tree
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
            UiEvent::KeyPressed {
                key: Key::Char('p'),
                ..
            } => self.open_next_preview_doc(),
            UiEvent::KeyPressed {
                key: Key::Char('m'),
                ..
            } => self.promote_active(),
            // The strip's tab clicks, routed through its own
            // `Interactive` and written back here.
            UiEvent::Workspace(_, intent) => {
                let events = self.workspace.apply(intent);
                self.record(&events)
            }
            // Ctrl+Tab / Ctrl+PageUp/PageDown cycle documents app-wide,
            // whether or not the strip holds keyboard focus.
            UiEvent::KeyPressed { key, modifiers, .. } => {
                match Workspace::cycle_delta(&key, modifiers) {
                    Some(delta) => match self.workspace.cycle(delta) {
                        Some(ev) => self.record(&[ev]),
                        None => Reaction::Continue,
                    },
                    None => Reaction::Continue,
                }
            }
            _ => Reaction::Continue,
        }
    }
}
