//! `WorkspaceController` demo — adapted from
//! `quadraui/examples/common/workspace_demo.rs`.
//!
//! An open-N-view-one document set living **inside a panel**, not in
//! shell chrome (quadraui#596): the controller renders its own tab strip
//! into whatever rect the host hands it, and the host paints the active
//! document's body wherever it likes. `p` demonstrates the preview tier
//! (quadraui#597): each press replaces the previous preview tab in place
//! rather than accumulating tabs.

use std::cell::RefCell;

use quadraui::{
    Backend, BackendCaps, Color, InteractionState, Key, Reaction, Rect, StatusBar,
    StatusBarSegment, UiEvent, WidgetId, WorkspaceController, WorkspaceDoc, WorkspaceEvent,
};

use crate::demo::{extract_region, Demo};

const SOURCE: &str = include_str!("workspace.rs");

// gallery:begin
const INITIAL: [(&str, &str); 3] = [
    ("gallery:doc:alpha", "alpha"),
    ("gallery:doc:beta", "beta"),
    ("gallery:doc:gamma", "gamma"),
];

const BACKLOG: [(&str, &str); 2] = [
    ("gallery:doc:delta", "delta-doc"),
    ("gallery:doc:epsilon", "epsilon-doc"),
];

const BAR_ID: &str = "gallery:workspace:tabs";

pub struct WorkspaceDemo {
    // `RefCell` so `render(&self, …)` can call the controller's `&mut
    // self` render — the same pattern `bottom_panel.rs` uses.
    workspace: RefCell<WorkspaceController>,
    next_backlog: usize,
    next_preview_backlog: usize,
    last_event: String,
}

impl WorkspaceDemo {
    pub fn new() -> Self {
        let mut workspace = WorkspaceController::new(BAR_ID);
        for (id, label) in INITIAL {
            workspace.open(WorkspaceDoc::new(id, label));
        }
        workspace.activate(INITIAL[0].0);
        Self {
            workspace: RefCell::new(workspace),
            next_backlog: 0,
            next_preview_backlog: 0,
            last_event: "ready".into(),
        }
    }

    fn describe(event: &WorkspaceEvent) -> String {
        match event {
            WorkspaceEvent::Opened { id, .. } => format!("opened {id}"),
            WorkspaceEvent::Activated { id, .. } => format!("activated {id}"),
            WorkspaceEvent::Closed { id, .. } => format!("closed {id}"),
            WorkspaceEvent::Reordered { id, from, to } => format!("reordered {id} {from}->{to}"),
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
            self.last_event = "backlog exhausted".into();
            return Reaction::Redraw;
        };
        self.next_backlog += 1;
        let events = self
            .workspace
            .borrow_mut()
            .open(WorkspaceDoc::new(id, label));
        self.record(&events)
    }

    fn open_next_preview_doc(&mut self) -> Reaction {
        let Some((id, label)) = BACKLOG.get(self.next_preview_backlog).copied() else {
            self.last_event = "preview backlog exhausted".into();
            return Reaction::Redraw;
        };
        self.next_preview_backlog += 1;
        let events = self
            .workspace
            .borrow_mut()
            .open_preview(WorkspaceDoc::new(id, label));
        self.record(&events)
    }

    fn promote_active(&mut self) -> Reaction {
        let mut ws = self.workspace.borrow_mut();
        let Some(id) = ws.active_id().map(str::to_string) else {
            return Reaction::Continue;
        };
        match ws.promote(&id) {
            Some(ev) => {
                drop(ws);
                self.record(&[ev])
            }
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

impl Demo for WorkspaceDemo {
    fn name(&self) -> &'static str {
        "Workspace"
    }

    fn group(&self) -> &'static str {
        "Chrome"
    }

    fn render(&self, _variant: usize, backend: &mut dyn Backend, area: Rect) {
        let lh = backend.line_height();
        let strip_h = lh;
        let strip = Rect::new(area.x, area.y, area.width, strip_h);
        self.workspace.borrow_mut().render(backend, strip);

        let body_rect = Rect::new(area.x, area.y + strip_h, area.width, lh);
        let ws = self.workspace.borrow();
        let body = match ws.active_id() {
            Some(id) if ws.is_preview(id) => format!(" viewing: {id} (preview) "),
            Some(id) => format!(" viewing: {id} "),
            None => " viewing: (no documents open) ".to_string(),
        };
        backend.draw_status_bar_interactive(
            body_rect,
            &Self::label_bar("gallery:workspace:body", body, Color::rgb(220, 220, 220)),
            &InteractionState::new(),
        );
        let hint_rect = Rect::new(area.x, area.y + strip_h + lh, area.width, lh);
        backend.draw_status_bar_interactive(
            hint_rect,
            &Self::label_bar(
                "gallery:workspace:hint",
                format!(" last: {} ", self.last_event),
                Color::rgb(150, 200, 150),
            ),
            &InteractionState::new(),
        );
    }

    fn handle(
        &mut self,
        _variant: usize,
        event: &UiEvent,
        _backend: &mut dyn Backend,
        _area: Rect,
    ) -> Reaction {
        match event {
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
            UiEvent::KeyPressed { key, modifiers, .. } => {
                let handled = self.workspace.borrow_mut().handle_key(key, *modifiers);
                match handled {
                    Some(ev) => self.record(&[ev]),
                    None => Reaction::Continue,
                }
            }
            UiEvent::MouseDown {
                position, button, ..
            } => {
                let events = self
                    .workspace
                    .borrow_mut()
                    .handle_click(position.x, position.y, *button);
                self.record(&events)
            }
            _ => Reaction::Continue,
        }
    }

    fn source(&self) -> &'static str {
        extract_region(SOURCE)
    }

    fn data(&self, _variant: usize) -> serde_json::Value {
        let ws = self.workspace.borrow();
        serde_json::json!({
            "docs": ws.docs().iter().map(|d| &d.label).collect::<Vec<_>>(),
            "active": ws.active_id(),
            "last_event": self.last_event,
        })
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
    fn open_next_backlog_doc_grows_the_workspace() {
        let mut demo = WorkspaceDemo::new();
        let before = demo.workspace.borrow().len();
        demo.open_next_backlog_doc();
        assert_eq!(demo.workspace.borrow().len(), before + 1);
    }

    #[test]
    fn preview_doc_replaces_itself_on_repeat() {
        let mut demo = WorkspaceDemo::new();
        let before = demo.workspace.borrow().len();
        demo.open_next_preview_doc();
        let after_first = demo.workspace.borrow().len();
        assert_eq!(after_first, before + 1);
    }
}
