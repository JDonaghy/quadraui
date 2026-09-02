//! Backend-agnostic app code for the dialog-table demo
//! ([`tui_dialog_table`]).
//!
//! [`DialogTableDemo`] shows a `Dialog` whose body is a two-column keybindings
//! table (the use-case from vimcode's Source Control help dialog).  The table
//! uses auto-sized column widths.  Press **Esc**, **q**, or **Close** to exit.
//!
//! The demo exercises:
//! - [`DialogTable`] with headers and multi-row data
//! - Generic layout using `backend.line_height()` so the dialog renders at the
//!   right scale on both TUI (1.0 = one cell) and GTK (pixel line height).
//! - The `draw_dialog` table rendering path (column separators + header row)

use quadraui::{
    AppLogic, Backend, Dialog, DialogButton, DialogMeasure, DialogTable, Key, NamedKey, Reaction,
    Rect, StyledText, ToolbarItemMeasure, UiEvent, WidgetId,
};

pub struct DialogTableDemo {
    dialog: Dialog,
}

impl DialogTableDemo {
    pub fn new() -> Self {
        let dialog = Dialog {
            id: WidgetId::new("help-dialog"),
            title: StyledText::plain("Source Control — Keybindings"),
            body: vec![],
            table: Some(DialogTable {
                headers: Some(vec!["Key".into(), "Action".into()]),
                rows: vec![
                    vec!["Ctrl+Enter".into(), "Stage hunk".into()],
                    vec!["Ctrl+Shift+Enter".into(), "Stage file".into()],
                    vec!["Ctrl+Z".into(), "Revert hunk".into()],
                    vec!["Ctrl+Shift+Z".into(), "Revert file".into()],
                    vec!["[".into(), "Previous change".into()],
                    vec!["]".into(), "Next change".into()],
                    vec!["d".into(), "Toggle inline diff".into()],
                    vec!["Esc".into(), "Close".into()],
                ],
                column_widths: None,
            }),
            buttons: vec![DialogButton {
                id: WidgetId::new("close"),
                label: "Close".into(),
                is_default: true,
                is_cancel: true,
                tint: None,
            }],
            severity: None,
            vertical_buttons: false,
            input: None,
        };

        Self { dialog }
    }

    /// Compute a generic [`DialogMeasure`] from `backend.line_height()` and the
    /// table's auto-sized column widths.
    ///
    /// `line_height` is 1.0 on TUI (one character cell) and the pixel line
    /// height on GTK/macOS. Column widths from `tui_total_width()` are in
    /// character cells; on pixel backends we approximate char pixel width as
    /// `line_height * 0.6`.
    fn measure(&self, backend: &dyn Backend) -> DialogMeasure {
        let lh = backend.line_height();
        let viewport = backend.viewport();

        // On TUI lh == 1.0: char-cell widths are already in the right unit.
        // On pixel backends lh is the pixel line-height; approximate char
        // width as lh × 0.6.
        let char_w = if lh > 1.0 { lh * 0.6 } else { 1.0 };

        let table = self.dialog.table.as_ref();
        let table_total_h = table
            .map(|t| t.tui_total_height() as f32 * lh)
            .unwrap_or(0.0);
        // Preferred table width: char cells × char_w + 2 char-widths of padding.
        let table_preferred_w = table
            .map(|t| t.tui_total_width() as f32 * char_w + char_w * 2.0)
            .unwrap_or(0.0);

        let title_h = if self.dialog.title.spans.iter().any(|s| !s.text.is_empty()) {
            lh
        } else {
            0.0
        };
        let body_h = self.dialog.body.len() as f32 * lh;

        let min_w = char_w * 30.0; // ≈ 30 char-widths
        let max_w = char_w * 60.0; // ≈ 60 char-widths
        let default_w = (viewport.width * 0.5).clamp(min_w, max_w);
        let dialog_w = default_w
            .max(table_preferred_w)
            .min(viewport.width - char_w * 4.0);

        DialogMeasure {
            width: dialog_w,
            title_height: title_h,
            body_height: body_h,
            table_height: table_total_h,
            input_height: 0.0,
            button_row_height: lh,
            button_width: char_w * 8.0,
            button_gap: char_w * 2.0,
            padding: lh,
        }
    }
}

impl Default for DialogTableDemo {
    fn default() -> Self {
        Self::new()
    }
}

impl AppLogic for DialogTableDemo {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let viewport = backend.viewport();
        let measure = self.measure(backend);
        let viewport_rect = Rect::new(0.0, 0.0, viewport.width, viewport.height);
        let layout = self
            .dialog
            .layout(viewport_rect, measure, |_| ToolbarItemMeasure::new(0.0));
        backend.draw_dialog(&self.dialog, &layout);
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        match event {
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Escape) | Key::Char('q'),
                ..
            } => Reaction::Exit,
            UiEvent::WindowResized { .. } => Reaction::Redraw,
            _ => Reaction::Continue,
        }
    }
}
