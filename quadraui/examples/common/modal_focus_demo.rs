//! Backend-agnostic app code for the modal-trapping / fallback-on-
//! disappear demo ([`tui_modal_focus_demo`]).
//!
//! This app owns a [`FocusTree`] directly — no [`AppLogic::tab_stops`],
//! no [`AppLogic::frame_tree`] — the same "do your own thing, opt out
//! of the global system" pattern `crate::compose::toast_stack`'s own
//! internal cycling already follows (see `AppLogic::tab_stops`'s doc
//! for why that's a supported shape, not a workaround). The point is
//! `FocusTree` itself: a host with no [`quadraui::ModalStack`] in play
//! at all can still use it exactly like this for ordinary Tab cycling,
//! but this demo exists for the two capabilities that need one:
//!
//! - **Modal trapping**: `m` opens a two-button confirm dialog,
//!   registered on [`quadraui::ModalStack`] like any real modal. Both
//!   the dialog's buttons *and* the three base rows stay in this
//!   frame's [`FocusTree`] at once (the dialog doesn't replace the base
//!   layer, it sits on top of it) — [`FocusTree::restrict`] is what
//!   actually confines Tab/Shift+Tab to the two buttons, by containment
//!   against the dialog's own [`quadraui::ModalEntry::bounds`]. Closing
//!   the dialog (Enter/Escape) drops the restriction by resyncing from
//!   the rows alone.
//! - **Fallback-on-disappear**: `d` deletes whichever row currently
//!   holds focus. Its stop vanishes from the next
//!   [`FocusTree::sync_tab_order`] call along with it, so focus falls
//!   back to "nothing" rather than pointing at a row that no longer
//!   exists — [`FocusTree::sync_tab_order`]'s own documented contract,
//!   exercised here end to end through a real running app instead of
//!   only against a hand-built list.
//!
//! Controls:
//! - Tab / Shift+Tab: cycle focus (rows, or — while the dialog is
//!   open — just its two buttons)
//! - d: delete the focused row (only while the dialog is closed)
//! - m: open the confirm dialog
//! - Enter / Escape: close the dialog
//! - q: quit

use quadraui::{
    AppLogic, Backend, Color, FocusTree, InteractionState, Key, NamedKey, Point, Reaction, Rect,
    StatusBar, StatusBarSegment, UiEvent, WidgetId,
};

const DIALOG_ID: &str = "modal-focus:dialog";
const CANCEL_ID: &str = "modal-focus:cancel";
const OK_ID: &str = "modal-focus:ok";

/// Fixed row the dialog starts on — comfortably below the (at most
/// three) base rows, so the two never share a rect regardless of how
/// many rows `d` has deleted.
const DIALOG_ROW: f32 = 5.0;

struct Row {
    id: WidgetId,
    text: String,
}

pub struct ModalFocusDemo {
    rows: Vec<Row>,
    dialog_open: bool,
    tree: FocusTree,
    last_action: String,
}

impl ModalFocusDemo {
    pub fn new() -> Self {
        let rows = vec![
            Row {
                id: WidgetId::new("modal-focus:row-0"),
                text: "Alpha".to_string(),
            },
            Row {
                id: WidgetId::new("modal-focus:row-1"),
                text: "Bravo".to_string(),
            },
            Row {
                id: WidgetId::new("modal-focus:row-2"),
                text: "Charlie".to_string(),
            },
        ];
        Self {
            rows,
            dialog_open: false,
            tree: FocusTree::new(),
            last_action: String::new(),
        }
    }

    fn row_rect(&self, backend: &dyn Backend, idx: usize) -> Rect {
        let lh = backend.line_height();
        let width = backend.viewport().width;
        Rect::new(0.0, idx as f32 * lh, width, lh)
    }

    fn dialog_bounds(&self, backend: &dyn Backend) -> Rect {
        let lh = backend.line_height();
        let width = backend.viewport().width;
        Rect::new(0.0, DIALOG_ROW * lh, width, lh * 2.0)
    }

    fn cancel_rect(&self, backend: &dyn Backend) -> Rect {
        let lh = backend.line_height();
        let width = backend.viewport().width;
        Rect::new(0.0, DIALOG_ROW * lh, width, lh)
    }

    fn ok_rect(&self, backend: &dyn Backend) -> Rect {
        let lh = backend.line_height();
        let width = backend.viewport().width;
        Rect::new(0.0, (DIALOG_ROW + 1.0) * lh, width, lh)
    }

    /// Every stop this frame's screen has, whether or not the dialog is
    /// open — base rows plus, while it is, the dialog's own two
    /// buttons. The dialog never *replaces* the base layer: it sits on
    /// top of it, which is exactly what makes trapping a real problem
    /// for [`FocusTree::restrict`] to solve rather than something the
    /// caller could get for free by only ever listing one layer's stops.
    fn flat_stops(&self, backend: &dyn Backend) -> Vec<(WidgetId, Rect)> {
        let mut stops: Vec<(WidgetId, Rect)> = self
            .rows
            .iter()
            .enumerate()
            .map(|(i, row)| (row.id, self.row_rect(backend, i)))
            .collect();
        if self.dialog_open {
            stops.push((WidgetId::new(CANCEL_ID), self.cancel_rect(backend)));
            stops.push((WidgetId::new(OK_ID), self.ok_rect(backend)));
        }
        stops
    }

    /// Rebuild [`Self::tree`] from the current screen, then — while the
    /// dialog is open — trap it down to just the dialog's own stops by
    /// containment against its registered [`quadraui::ModalStack`]
    /// bounds. Called on every event that can change what's on screen
    /// (Tab/Shift+Tab, opening/closing the dialog, deleting a row) so
    /// the tree is always resolved against the *current* frame, the
    /// same freshness `crate::runtime`'s own widget-contract routing
    /// keeps for its own `FocusTree`.
    fn resync(&mut self, backend: &mut dyn Backend) {
        let stops = self.flat_stops(backend);
        self.tree.sync_tab_order(&stops);
        if self.dialog_open {
            if let Some(entry) = backend
                .modal_stack_handle()
                .borrow()
                .top()
                .filter(|e| e.id == WidgetId::new(DIALOG_ID))
            {
                let bounds = entry.bounds;
                self.tree
                    .restrict(|_addr, rect| bounds.contains(Point::new(rect.x, rect.y)));
            }
        }
    }

    fn open_dialog(&mut self, backend: &mut dyn Backend) {
        self.dialog_open = true;
        let bounds = self.dialog_bounds(backend);
        backend
            .modal_stack_handle()
            .borrow_mut()
            .push(WidgetId::new(DIALOG_ID), bounds);
        self.resync(backend);
        // A dialog opening conventionally takes focus itself rather than
        // leaving the base layer's old focus dangling behind it.
        self.tree.focus_next();
    }

    fn close_dialog(&mut self, backend: &mut dyn Backend) {
        self.dialog_open = false;
        backend
            .modal_stack_handle()
            .borrow_mut()
            .pop(&WidgetId::new(DIALOG_ID));
        self.resync(backend);
    }

    fn delete_focused_row(&mut self, backend: &mut dyn Backend) {
        let Some(&(focused, _)) = self.tree.focused() else {
            return;
        };
        if let Some(idx) = self.rows.iter().position(|r| r.id == focused) {
            self.last_action = format!("deleted {}", self.rows[idx].text);
            self.rows.remove(idx);
            self.resync(backend);
        }
    }

    fn focus_label(&self) -> String {
        match self.tree.focused() {
            Some((id, _)) if *id == WidgetId::new(CANCEL_ID) => "focus: Cancel".to_string(),
            Some((id, _)) if *id == WidgetId::new(OK_ID) => "focus: OK".to_string(),
            Some((id, _)) => format!(
                "focus: {}",
                self.rows
                    .iter()
                    .find(|r| r.id == *id)
                    .map(|r| r.text.as_str())
                    .unwrap_or("?")
            ),
            None => "focus: none".to_string(),
        }
    }

    fn line(&self, backend: &mut dyn Backend, rect: Rect, id: &str, text: String, focused: bool) {
        let bg = if focused {
            Color::rgb(60, 60, 90)
        } else {
            Color::rgb(24, 24, 32)
        };
        let bar = StatusBar {
            id: WidgetId::new(id),
            left_segments: vec![StatusBarSegment {
                text,
                fg: Color::rgb(220, 220, 220),
                bg,
                bold: focused,
                action_id: None,
            }],
            right_segments: vec![],
        };
        let _ = backend.draw_status_bar_interactive(rect, &bar, &InteractionState::new());
    }
}

impl Default for ModalFocusDemo {
    fn default() -> Self {
        Self::new()
    }
}

impl AppLogic for ModalFocusDemo {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        for (i, row) in self.rows.iter().enumerate() {
            let rect = self.row_rect(backend, i);
            let focused = self.tree.is_focused(&row.id, None);
            self.line(
                backend,
                rect,
                row.id.as_str(),
                format!(" {} ", row.text),
                focused,
            );
        }
        if self.dialog_open {
            let cancel_focused = self.tree.is_focused(&WidgetId::new(CANCEL_ID), None);
            let ok_focused = self.tree.is_focused(&WidgetId::new(OK_ID), None);
            let cancel_rect = self.cancel_rect(backend);
            let ok_rect = self.ok_rect(backend);
            self.line(
                backend,
                cancel_rect,
                CANCEL_ID,
                " [ Cancel ] ".to_string(),
                cancel_focused,
            );
            self.line(backend, ok_rect, OK_ID, " [ OK ] ".to_string(), ok_focused);
        }
        let lh = backend.line_height();
        let viewport = backend.viewport();
        let status_rect = Rect::new(0.0, viewport.height - lh, viewport.width, lh);
        let last = if self.last_action.is_empty() {
            "ready".to_string()
        } else {
            self.last_action.clone()
        };
        let status = StatusBar {
            id: WidgetId::new("modal-focus:status"),
            left_segments: vec![StatusBarSegment {
                text: format!(" {} | {last} ", self.focus_label()),
                fg: Color::rgb(220, 220, 220),
                bg: Color::rgb(40, 40, 60),
                bold: false,
                action_id: None,
            }],
            right_segments: vec![StatusBarSegment {
                text: " Tab=cycle d=delete m=modal q=quit ".to_string(),
                fg: Color::rgb(220, 220, 220),
                bg: Color::rgb(40, 40, 60),
                bold: false,
                action_id: None,
            }],
        };
        let _ = backend.draw_status_bar_interactive(status_rect, &status, &InteractionState::new());
    }

    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
        match event {
            UiEvent::KeyPressed {
                key: Key::Char('q'),
                ..
            } => Reaction::Exit,
            UiEvent::KeyPressed {
                key: Key::Named(named @ (NamedKey::Tab | NamedKey::BackTab)),
                modifiers,
                ..
            } => {
                self.resync(backend);
                let retreat = matches!(named, NamedKey::BackTab) || modifiers.shift;
                if retreat {
                    self.tree.focus_prev();
                } else {
                    self.tree.focus_next();
                }
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Char('m'),
                ..
            } => {
                if !self.dialog_open {
                    self.open_dialog(backend);
                }
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Char('d'),
                ..
            } => {
                if !self.dialog_open {
                    self.delete_focused_row(backend);
                }
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Enter) | Key::Named(NamedKey::Escape),
                ..
            } => {
                if self.dialog_open {
                    self.last_action = "dialog closed".to_string();
                    self.close_dialog(backend);
                    Reaction::Redraw
                } else {
                    Reaction::Continue
                }
            }
            _ => Reaction::Continue,
        }
    }
}
