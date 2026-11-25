//! `Dialog` demo.
//!
//! The smallest app that makes the **click-through-a-modal** bug class
//! visible as painted text: a scrollable list of rows sits behind a
//! centred [`Dialog`], and clicking inside the dialog must never fall
//! through to the row underneath it.
//!
//! This app has no `if dialog_open { return }` guard anywhere — modal
//! arbitration is quadraui's job. [`quadraui::ModalStack`] (reached here
//! via [`Backend::modal_stack_handle`]) plus the backend's own
//! `dispatch_click` tag a `MouseDown` that lands inside the topmost
//! modal's bounds with that modal's [`WidgetId`], so the app routes
//! purely on the event's shape: a tagged click is the dialog's, an
//! untagged one is the row list's.
//!
//! Known gap shared with `FloatDemo`: the gallery's `Demo` trait has no
//! "this demo is no longer active" hook to pop the dialog's registration
//! on. Closing it explicitly (Cancel/OK/Esc) always pops it; only
//! switching away from this demo mid-dialog leaves a stale entry behind.
//!
//! Three variants vary the [`Dialog`] shape itself: a neutral two-button
//! confirm, a destructive action with a tinted button and an error
//! severity tint, and a three-button stack laid out vertically.

use quadraui::DrawsPrimitives;
use quadraui::{
    Backend, BackendCaps, Color, Dialog, DialogButton, DialogHit, DialogLayout, DialogSeverity,
    InteractionState, Key, NamedKey, Reaction, Rect, StatusBar, StatusBarSegment, StyledText,
    UiEvent, WidgetId,
};

use crate::demo::{extract_region, CaptureAction, Demo};

const SOURCE: &str = include_str!("dialog.rs");

// gallery:begin
/// Id the dialog is registered under in the [`quadraui::ModalStack`]. The
/// same id comes back on `MouseDown { widget }` for clicks inside it.
const DIALOG_ID: &str = "gallery:dialog:confirm-delete";

/// Rows in the list — enough that the centred dialog covers a
/// contiguous middle band, including one row's own text.
const ROW_COUNT: usize = 14;

pub struct DialogDemo {
    rows: Vec<String>,
    selected: Option<String>,
    dialog_open: bool,
}

impl DialogDemo {
    pub fn new() -> Self {
        Self {
            rows: (0..ROW_COUNT).map(|i| format!("pod-{i:02}")).collect(),
            selected: None,
            dialog_open: false,
        }
    }

    fn dialog(&self, variant: usize) -> Dialog {
        match variant {
            0 => Dialog {
                id: WidgetId::new(DIALOG_ID),
                title: StyledText::plain("Confirm delete"),
                body: vec![
                    StyledText::plain("Really delete?"),
                    StyledText::plain("This cannot be undone."),
                ],
                table: None,
                buttons: vec![
                    DialogButton {
                        spec: None,
                        id: WidgetId::new("gallery:dialog:ok"),
                        label: "OK".into(),
                        is_default: true,
                        is_cancel: false,
                        tint: None,
                    },
                    DialogButton {
                        spec: None,
                        id: WidgetId::new("gallery:dialog:cancel"),
                        label: "Cancel".into(),
                        is_default: false,
                        is_cancel: true,
                        tint: None,
                    },
                ],
                severity: None,
                vertical_buttons: false,
                input: None,
            },
            1 => Dialog {
                id: WidgetId::new(DIALOG_ID),
                title: StyledText::plain("Delete permanently?"),
                body: vec![
                    StyledText::plain("This removes the pod and all its logs."),
                    StyledText::plain("This cannot be undone."),
                ],
                table: None,
                buttons: vec![
                    DialogButton {
                        spec: None,
                        id: WidgetId::new("gallery:dialog:keep"),
                        label: "Keep".into(),
                        is_default: true,
                        is_cancel: true,
                        tint: None,
                    },
                    DialogButton {
                        spec: None,
                        id: WidgetId::new("gallery:dialog:delete"),
                        label: "Delete".into(),
                        is_default: false,
                        is_cancel: false,
                        tint: Some(Color::rgb(200, 60, 60)),
                    },
                ],
                severity: Some(DialogSeverity::Error),
                vertical_buttons: false,
                input: None,
            },
            _ => Dialog {
                id: WidgetId::new(DIALOG_ID),
                title: StyledText::plain("Unsaved changes"),
                body: vec![StyledText::plain(
                    "What should happen to your changes before closing?",
                )],
                table: None,
                buttons: vec![
                    DialogButton {
                        spec: None,
                        id: WidgetId::new("gallery:dialog:save"),
                        label: "Save".into(),
                        is_default: true,
                        is_cancel: false,
                        tint: None,
                    },
                    DialogButton {
                        spec: None,
                        id: WidgetId::new("gallery:dialog:discard"),
                        label: "Discard".into(),
                        is_default: false,
                        is_cancel: false,
                        tint: Some(Color::rgb(200, 60, 60)),
                    },
                    DialogButton {
                        spec: None,
                        id: WidgetId::new("gallery:dialog:cancel-vertical"),
                        label: "Cancel".into(),
                        is_default: false,
                        is_cancel: true,
                        tint: None,
                    },
                ],
                severity: Some(DialogSeverity::Warning),
                vertical_buttons: true,
                input: None,
            },
        }
    }

    /// Dialog layout for `area`, resolved through the widget contract.
    /// Called from both `render` and `handle` so paint and hit-test can
    /// never disagree — `Dialog`'s own `Primitive::layout` centres the
    /// modal box in `area` and measures it with the calling backend's
    /// real font metrics (cell-snapped on TUI, `Style::control_height`
    /// button row on a pixel surface).
    fn dialog_layout(&self, backend: &dyn Backend, area: Rect, variant: usize) -> DialogLayout {
        backend.layout_primitive(area, &self.dialog(variant)).layout
    }

    fn row_at(&self, backend: &dyn Backend, area: Rect, y: f32) -> Option<usize> {
        let lh = backend.line_height();
        if lh <= 0.0 || y < area.y {
            return None;
        }
        let idx = ((y - area.y) / lh).floor() as usize;
        (idx < self.rows.len()).then_some(idx)
    }

    /// True if `y` falls on the `Open dialog` button row (immediately
    /// below the last list row).
    fn on_open_button(&self, backend: &dyn Backend, area: Rect, y: f32) -> bool {
        let lh = backend.line_height();
        lh > 0.0 && self.row_at(backend, area, y).is_none() && {
            let idx = ((y - area.y) / lh).floor() as usize;
            idx == self.rows.len()
        }
    }

    fn status_bar(&self, variant: usize) -> StatusBar {
        let selected = self.selected.clone().unwrap_or_else(|| "nothing".into());
        let variant_label = match variant {
            0 => "Confirm",
            1 => "Destructive",
            _ => "Vertical",
        };
        StatusBar {
            id: WidgetId::new("gallery:dialog:status"),
            left_segments: vec![StatusBarSegment {
                text: format!(" [{variant_label}] selected: {selected} "),
                fg: Color::rgb(255, 255, 255),
                bg: Color::rgb(40, 80, 120),
                bold: false,
                action_id: None,
            }],
            right_segments: vec![StatusBarSegment {
                text: " click a row, or use the button below ".into(),
                fg: Color::rgb(220, 220, 220),
                bg: Color::rgb(40, 80, 120),
                bold: false,
                action_id: None,
            }],
        }
    }

    fn line(&self, backend: &mut dyn Backend, rect: Rect, id: &str, text: String) {
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
        let _ = backend.draw_status_bar_interactive(rect, &bar, &InteractionState::new());
    }

    fn open_dialog(&mut self, backend: &mut dyn Backend, area: Rect, variant: usize) {
        self.dialog_open = true;
        let bounds = self.dialog_layout(backend, area, variant).bounds;
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

impl Default for DialogDemo {
    fn default() -> Self {
        Self::new()
    }
}

impl Demo for DialogDemo {
    fn name(&self) -> &'static str {
        "Dialog"
    }

    fn group(&self) -> &'static str {
        "Overlays"
    }

    fn variants(&self) -> &'static [&'static str] {
        &["Confirm", "Destructive", "Vertical"]
    }

    fn render(&self, variant: usize, backend: &mut dyn Backend, area: Rect) {
        let lh = backend.line_height();

        for (i, row) in self.rows.iter().enumerate() {
            self.line(
                backend,
                Rect::new(area.x, area.y + i as f32 * lh, area.width, lh),
                &format!("gallery:dialog:row-{i}"),
                format!(" {row} "),
            );
        }
        self.line(
            backend,
            Rect::new(area.x, area.y + self.rows.len() as f32 * lh, area.width, lh),
            "gallery:dialog:open-dialog",
            " Open dialog ".into(),
        );

        let status_rect = Rect::new(area.x, area.y + area.height - lh, area.width, lh);
        let _ = backend.draw_status_bar_interactive(
            status_rect,
            &self.status_bar(variant),
            &InteractionState::new(),
        );

        // Modal paints last (highest z) — the ModalStack has no opinion
        // on draw order, only on hit-test precedence.
        if self.dialog_open {
            backend.draw_dyn(area, &self.dialog(variant), &());
        }
    }

    fn handle(
        &mut self,
        variant: usize,
        event: &UiEvent,
        backend: &mut dyn Backend,
        area: Rect,
    ) -> Reaction {
        match event {
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Escape),
                ..
            } if self.dialog_open => {
                self.close_dialog(backend);
                Reaction::Redraw
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
                let layout = self.dialog_layout(backend, area, variant);
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
                if self.on_open_button(backend, area, position.y) && !self.dialog_open {
                    self.open_dialog(backend, area, variant);
                } else if let Some(idx) = self.row_at(backend, area, position.y) {
                    self.selected = Some(self.rows[idx].clone());
                }
                Reaction::Redraw
            }

            _ => Reaction::Continue,
        }
    }

    fn source(&self) -> &'static str {
        extract_region(SOURCE)
    }

    fn data(&self, _variant: usize) -> serde_json::Value {
        serde_json::json!({
            "selected": self.selected,
            "dialog_open": self.dialog_open,
        })
    }

    fn caps_note(&self, _variant: usize, _caps: &BackendCaps) -> Option<String> {
        // `draw_dialog` + the `ModalStack` click-arbitration pipeline are
        // required, backend-generic machinery with no capability flag —
        // every backend routes clicks through the same stack.
        None
    }

    fn capture_action(&self, _variant: usize) -> CaptureAction {
        CaptureAction::ClickText("Open dialog")
    }
}
// gallery:end

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_has_no_selection_and_closed_dialog() {
        let demo = DialogDemo::new();
        assert!(demo.selected.is_none());
        assert!(!demo.dialog_open);
    }

    #[test]
    fn dialog_id_matches_the_built_dialog_in_every_variant() {
        let demo = DialogDemo::new();
        for variant in 0..3 {
            assert_eq!(demo.dialog(variant).id, WidgetId::new(DIALOG_ID));
        }
    }

    #[test]
    fn vertical_variant_actually_stacks_its_buttons() {
        let demo = DialogDemo::new();
        assert!(!demo.dialog(0).vertical_buttons);
        assert!(!demo.dialog(1).vertical_buttons);
        assert!(demo.dialog(2).vertical_buttons);
        assert_eq!(demo.dialog(2).buttons.len(), 3);
    }

    /// `GalleryApp`'s real demo-content rect for a 100x32 terminal is
    /// `Rect { x: 24, y: 1, width: 76, height: 23 }` — an *odd* height,
    /// while this dialog's own total content height is even.
    /// `Dialog`'s own cell-grid layout must keep every edge of the
    /// laid-out box on a whole cell regardless: centering an even-height
    /// box inside an odd-height area un-snapped would land the button
    /// row on a half-cell boundary, so a click at the whole-cell center
    /// of the row the button glyph actually paints on would fall just
    /// outside its hit region and resolve to `DialogHit::Body` instead
    /// of `DialogHit::Button`.
    #[test]
    #[cfg(feature = "tui")]
    fn dialog_bounds_land_on_whole_cells_even_with_an_odd_height_area() {
        let demo = DialogDemo::new();
        let backend = quadraui::tui::TuiBackend::new();
        let area = Rect::new(24.0, 1.0, 76.0, 23.0);
        let layout = demo.dialog_layout(&backend, area, 0);

        assert_eq!(
            layout.bounds.y.fract(),
            0.0,
            "bounds.y: {:?}",
            layout.bounds
        );
        assert_eq!(
            layout.bounds.x.fract(),
            0.0,
            "bounds.x: {:?}",
            layout.bounds
        );

        let ok_button = layout
            .visible_buttons
            .iter()
            .find(|b| b.id == WidgetId::new("gallery:dialog:ok"))
            .expect("OK button should be laid out");
        // The whole-cell center of the row/column the button's own
        // top-left corner starts on must hit that button — exactly
        // what a `TuiDriver::find("OK")` click resolves to in practice.
        let cell_x = ok_button.bounds.x.floor() + 0.5;
        let cell_y = ok_button.bounds.y.floor() + 0.5;
        assert_eq!(
            layout.hit_test(cell_x, cell_y),
            DialogHit::Button(WidgetId::new("gallery:dialog:ok")),
            "a click at the OK button's own top-left cell must hit it: {:?}",
            ok_button.bounds
        );
    }

    /// Same guarantee as the test above, for the `Vertical` variant,
    /// where the three-button stack adds two extra button rows to the
    /// total content height — `dialog_layout` must fold that into its
    /// own `total_height` or the whole-cell parity breaks for the
    /// bottom buttons specifically.
    #[test]
    #[cfg(feature = "tui")]
    fn vertical_variant_bounds_also_land_on_whole_cells() {
        let demo = DialogDemo::new();
        let backend = quadraui::tui::TuiBackend::new();
        let area = Rect::new(24.0, 1.0, 76.0, 23.0);
        let layout = demo.dialog_layout(&backend, area, 2);

        let cancel_button = layout
            .visible_buttons
            .iter()
            .find(|b| b.id == WidgetId::new("gallery:dialog:cancel-vertical"))
            .expect("Cancel button should be laid out");
        let cell_x = cancel_button.bounds.x.floor() + 0.5;
        let cell_y = cancel_button.bounds.y.floor() + 0.5;
        assert_eq!(
            layout.hit_test(cell_x, cell_y),
            DialogHit::Button(WidgetId::new("gallery:dialog:cancel-vertical")),
            "a click at the bottom-most stacked button's own top-left cell must hit it: {:?}",
            cancel_button.bounds
        );
    }

    /// On a pixel backend (`RecordingBackend`'s `line_height` set above
    /// `1.0`, simulating GTK/macOS/Win), the painted button row is
    /// exactly `Style::control_height` tall — matching
    /// `MessageDialogController::layout`'s own OK/Cancel row, the
    /// acceptance bullet this demo's bare `Dialog` previously missed by
    /// sizing its button row at the raw chrome line height instead.
    #[test]
    fn button_row_matches_control_height_on_a_pixel_backend() {
        let demo = DialogDemo::new();
        let backend = quadraui::testing::RecordingBackend {
            line_height: 20.0,
            char_width: 9.0,
            ..quadraui::testing::RecordingBackend::new()
        };
        let area = Rect::new(0.0, 0.0, 400.0, 300.0);
        let layout = demo.dialog_layout(&backend, area, 0);

        assert_eq!(
            layout.button_row_bounds.height,
            quadraui::Style::default().control_height,
            "the button row must be sized at `Style::control_height` on \
             a pixel backend: {:?}",
            layout.button_row_bounds
        );
    }

    /// Same scenario on a TUI-shaped backend (`line_height == 1.0`): the
    /// button row stays exactly one char cell tall, unaffected by the
    /// `Style::control_height` floor above.
    #[test]
    fn button_row_stays_one_cell_tall_on_a_char_grid_backend() {
        let demo = DialogDemo::new();
        let backend = quadraui::testing::RecordingBackend::new();
        let area = Rect::new(0.0, 0.0, 80.0, 24.0);
        let layout = demo.dialog_layout(&backend, area, 0);

        assert_eq!(layout.button_row_bounds.height, 1.0);
    }
}
