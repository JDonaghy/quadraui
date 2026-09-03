//! Backend-agnostic app code for the modal-occlusion demo
//! ([`tui_modal_occlusion`] / [`gtk_modal_occlusion`]).
//!
//! [`ModalOcclusionDemo`] is the smallest app that makes the **#455 bug
//! class** — "a click lands inside an open modal but still reaches the
//! widget behind it" — visible as painted text, so a coordinate-free
//! conformance scenario can assert it on every backend
//! (`tests/conformance/scenarios/modal/dialog.blocks_click_through.scn.json`,
//! quadraui#491).
//!
//! Layout:
//!
//! - A full-height list of rows (`pod-00` … `pod-NN`), one
//!   [`StatusBar`] line each. Clicking a row selects it; the bottom
//!   status bar echoes `selected: <row>`.
//! - A `Open dialog` button on the last row.
//! - When open, a centred [`Dialog`] titled `Confirm delete` whose body
//!   reads `Really delete?`. It covers rows in the middle of the list.
//!
//! ## The contract this demo exercises
//!
//! **This app has no `if dialog_open { return }` guard anywhere.** Modal
//! arbitration is quadraui's job: [`crate::ModalStack`] plus
//! `dispatch_click` tag a `MouseDown` that lands inside the topmost
//! modal's bounds with that modal's [`WidgetId`]. So the app routes
//! purely on the event's shape:
//!
//! - `MouseDown { widget: Some(DIALOG_ID), .. }` → the click is the
//!   dialog's; ask [`DialogLayout::hit_test`] which button (if any).
//! - `MouseDown { widget: None, .. }` → base layer; hit-test the rows.
//!
//! If a backend forgets to route mouse-downs through the modal stack,
//! the click arrives as `widget: None`, the row list selects the row
//! under the dialog, and `selected: …` changes — which is exactly what
//! the scenario asserts must *not* happen. That is the bug made
//! executable rather than described.
//!
//! Controls:
//! - click a row              select it
//! - click `Open dialog`      open the modal
//! - click `Cancel` / `OK`    close the modal
//! - Esc                      close the modal, or quit when none is open
//! - q                        quit

use quadraui::{
    AppLogic, Backend, Color, Dialog, DialogButton, DialogHit, DialogLayout, DialogMeasure, Key,
    NamedKey, Reaction, Rect, StatusBar, StatusBarSegment, StyledText, ToolbarItemMeasure, UiEvent,
    WidgetId,
};

/// Id the dialog is registered under in the [`crate::ModalStack`]. The
/// same id comes back on `MouseDown { widget }` for clicks inside it.
const DIALOG_ID: &str = "confirm-delete";

/// Rows in the list. Enough of them that the centred dialog covers a
/// contiguous middle band — the scenario clicks the dialog's own body
/// text, which sits directly over one of these rows.
const ROW_COUNT: usize = 18;

pub struct ModalOcclusionDemo {
    rows: Vec<String>,
    selected: Option<String>,
    dialog_open: bool,
}

impl ModalOcclusionDemo {
    pub fn new() -> Self {
        Self {
            rows: (0..ROW_COUNT).map(|i| format!("pod-{i:02}")).collect(),
            selected: None,
            dialog_open: false,
        }
    }

    fn dialog(&self) -> Dialog {
        Dialog {
            id: WidgetId::new(DIALOG_ID),
            title: StyledText::plain("Confirm delete"),
            body: vec![
                StyledText::plain("Really delete?"),
                StyledText::plain("This cannot be undone."),
            ],
            table: None,
            buttons: vec![
                DialogButton {
                    id: WidgetId::new("ok"),
                    label: "OK".into(),
                    is_default: true,
                    is_cancel: false,
                    tint: None,
                },
                DialogButton {
                    id: WidgetId::new("cancel"),
                    label: "Cancel".into(),
                    is_default: false,
                    is_cancel: true,
                    tint: None,
                },
            ],
            severity: None,
            vertical_buttons: false,
            input: None,
        }
    }

    /// Dialog layout for the current viewport. Called from both `render`
    /// and `handle` so paint and hit-test can never disagree — the
    /// `docs/LESSONS.md` "one layout fn, two callers" rule.
    fn dialog_layout(&self, backend: &dyn Backend) -> DialogLayout {
        let lh = backend.line_height();
        let viewport = backend.viewport();
        // TUI's line_height is 1.0 (one cell); pixel backends report the
        // real line height, so approximate a char width from it exactly
        // as `dialog_table_demo` does.
        let char_w = if lh > 1.0 { lh * 0.6 } else { 1.0 };
        let measure = DialogMeasure {
            width: (viewport.width * 0.5).clamp(char_w * 24.0, char_w * 48.0),
            title_height: lh,
            body_height: lh * self.dialog().body.len() as f32,
            table_height: 0.0,
            input_height: 0.0,
            button_row_height: lh,
            button_width: char_w * 10.0,
            button_gap: char_w * 2.0,
            padding: lh,
        };
        // NOTE: `Dialog::layout` centres the box, so an odd total height in
        // a viewport with an even row count lands the box on a half-line.
        // TUI rounds paint to whole cells but hit-tests the unrounded rect,
        // so a half-line offset makes the painted button row and its hit
        // region disagree by one cell. Two body lines keep the total even
        // (padding 2 + title 1 + body 2 + buttons 1 = 6) and the two agree.
        let viewport_rect = Rect::new(0.0, 0.0, viewport.width, viewport.height);
        self.dialog()
            .layout(viewport_rect, measure, |_| ToolbarItemMeasure::new(0.0))
    }

    /// Row index whose band contains `y`, if any. Rows start at the top
    /// of the viewport, one `line_height` each.
    fn row_at(&self, backend: &dyn Backend, y: f32) -> Option<usize> {
        let lh = backend.line_height();
        if lh <= 0.0 || y < 0.0 {
            return None;
        }
        let idx = (y / lh).floor() as usize;
        (idx < self.rows.len()).then_some(idx)
    }

    /// True if `y` falls on the `Open dialog` button row (immediately
    /// below the last list row).
    fn on_open_button(&self, backend: &dyn Backend, y: f32) -> bool {
        let lh = backend.line_height();
        lh > 0.0 && self.row_at(backend, y).is_none() && {
            let idx = (y / lh).floor() as usize;
            idx == self.rows.len()
        }
    }

    fn status_bar(&self) -> StatusBar {
        let selected = self.selected.clone().unwrap_or_else(|| "nothing".into());
        StatusBar {
            id: WidgetId::new("status"),
            left_segments: vec![StatusBarSegment {
                text: format!(" selected: {selected} "),
                fg: Color::rgb(255, 255, 255),
                bg: Color::rgb(40, 80, 120),
                bold: false,
                action_id: None,
            }],
            right_segments: vec![StatusBarSegment {
                text: " click a row | q=quit ".into(),
                fg: Color::rgb(220, 220, 220),
                bg: Color::rgb(40, 80, 120),
                bold: false,
                action_id: None,
            }],
        }
    }

    fn line(&self, backend: &mut dyn Backend, y: f32, id: &str, text: String) {
        let lh = backend.line_height();
        let width = backend.viewport().width;
        let bar = StatusBar {
            id: WidgetId::new(id),
            left_segments: vec![StatusBarSegment {
                text,
                fg: Color::rgb(210, 210, 210),
                bg: Color::rgb(24, 24, 32),
                bold: false,
                action_id: None,
            }],
            right_segments: vec![],
        };
        let _ = backend.draw_status_bar(Rect::new(0.0, y, width, lh), &bar, None, None);
    }
}

impl Default for ModalOcclusionDemo {
    fn default() -> Self {
        Self::new()
    }
}

impl AppLogic for ModalOcclusionDemo {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let lh = backend.line_height();
        let viewport = backend.viewport();

        for (i, row) in self.rows.iter().enumerate() {
            self.line(
                backend,
                i as f32 * lh,
                &format!("row-{i}"),
                format!(" {row} "),
            );
        }
        self.line(
            backend,
            self.rows.len() as f32 * lh,
            "open-dialog",
            " Open dialog ".into(),
        );

        let status_rect = Rect::new(0.0, viewport.height - lh, viewport.width, lh);
        let _ = backend.draw_status_bar(status_rect, &self.status_bar(), None, None);

        // Modal paints last (highest z) — the ModalStack has no opinion
        // on draw order, only on hit-test precedence.
        if self.dialog_open {
            let layout = self.dialog_layout(backend);
            backend.draw_dialog(&self.dialog(), &layout);
        }
    }

    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
        match event {
            UiEvent::KeyPressed {
                key: Key::Char('q'),
                ..
            } => Reaction::Exit,
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Escape),
                ..
            } => {
                if self.dialog_open {
                    self.close_dialog(backend);
                    Reaction::Redraw
                } else {
                    Reaction::Exit
                }
            }

            // ── Click inside the open modal ─────────────────────────────
            //
            // quadraui tagged this one with the dialog's id, so it can
            // only be the dialog's. Note the absence of any
            // `self.dialog_open` test: the tag *is* the test.
            UiEvent::MouseDown {
                widget: Some(ref id),
                position,
                ..
            } if id.as_str() == DIALOG_ID => {
                let layout = self.dialog_layout(backend);
                if let DialogHit::Button(_) = layout.hit_test(position.x, position.y) {
                    self.close_dialog(backend);
                }
                Reaction::Redraw
            }

            // ── Base-layer click ────────────────────────────────────────
            UiEvent::MouseDown {
                widget: None,
                position,
                ..
            } => {
                if self.on_open_button(backend, position.y) && !self.dialog_open {
                    self.open_dialog(backend);
                } else if let Some(idx) = self.row_at(backend, position.y) {
                    self.selected = Some(self.rows[idx].clone());
                }
                Reaction::Redraw
            }

            UiEvent::WindowResized { .. } => Reaction::Redraw,
            _ => Reaction::Continue,
        }
    }
}

impl ModalOcclusionDemo {
    /// Open the dialog and register its bounds with the backend's modal
    /// stack — the once-per-open push the `ModalStack` docs prescribe.
    fn open_dialog(&mut self, backend: &mut dyn Backend) {
        self.dialog_open = true;
        let bounds = self.dialog_layout(backend).bounds;
        backend
            .modal_stack_handle()
            .borrow_mut()
            .push(WidgetId::new(DIALOG_ID), bounds);
    }

    fn close_dialog(&mut self, backend: &mut dyn Backend) {
        self.dialog_open = false;
        backend
            .modal_stack_handle()
            .borrow_mut()
            .pop(&WidgetId::new(DIALOG_ID));
    }
}
