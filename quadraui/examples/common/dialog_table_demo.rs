//! Backend-agnostic app code for the dialog-table demo
//! ([`tui_dialog_table`]).
//!
//! [`DialogTableDemo`] shows a `Dialog` whose body is a two-column keybindings
//! table (the use-case from vimcode's Source Control help dialog).  The table
//! uses auto-sized column widths.  Press **Esc**, **q**, or **Close** to exit.
//!
//! The demo exercises:
//! - [`DialogTable`] with headers and multi-row data
//! - Generic layout using `backend.measure()` (quadraui#817) so the dialog
//!   renders at the right scale on both TUI (1.0 = one cell) and GTK (real
//!   pixel line height / char width, not an approximation of either).
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

    /// Compute a generic [`DialogMeasure`] from `backend.measure()` and the
    /// table's auto-sized column widths.
    ///
    /// `line_height` is 1.0 on TUI (one character cell) and the pixel line
    /// height on GTK/macOS. Column widths from `tui_total_width()` are in
    /// character cells, so a pixel backend's `char_width` (also bundled into
    /// [`crate::Backend::measure`]'s [`crate::Metrics`]) converts them to
    /// pixels directly.
    ///
    /// Pre-#817 this approximated `char_width` as `line_height * 0.6`
    /// instead of asking the backend for the real value — exactly the
    /// duplicated-font-metric-knowledge issue #817 exists to remove. On
    /// TUI the two happened to agree (both `1.0`), which is why the
    /// approximation went unnoticed here; on a pixel backend they don't,
    /// and `backend.measure().char_width` is the actual glyph width, not
    /// a guess.
    fn measure(&self, backend: &dyn Backend) -> DialogMeasure {
        let m = backend.measure();
        let lh = m.line_height;
        let char_w = m.char_width;
        let viewport = backend.viewport();

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

#[cfg(test)]
mod measure_tests {
    use super::*;
    use quadraui::testing::RecordingBackend;
    use quadraui::Viewport;

    /// TUI-shaped metrics: `line_height == char_width == 1.0`, matching
    /// what `tests/tui_example_driver.rs`'s `TuiDriver`-based
    /// `dialog_table_*` tests actually exercise.
    fn tui_shaped() -> RecordingBackend {
        RecordingBackend::with_viewport(Viewport::new(100.0, 30.0, 1.0), 1.0, 1.0)
    }

    /// A pixel-shaped backend (GTK-ish numbers): `line_height` and
    /// `char_width` are independent, unlike TUI where both are `1.0`.
    fn pixel_shaped() -> RecordingBackend {
        RecordingBackend::with_viewport(Viewport::new(800.0, 600.0, 1.0), 20.0, 9.0)
    }

    /// Pre-#817 this file approximated `char_width` as `line_height *
    /// 0.6` instead of asking the backend. Reproduced here only to prove
    /// the two formulas coincide on TUI metrics -- the switch to
    /// `backend.measure().char_width` does not move
    /// `DialogTableDemo`'s TUI layout (the layout every
    /// `dialog_table_*` driver test in `tests/tui_example_driver.rs`
    /// actually paints and asserts against).
    fn pre_817_char_width_approximation(line_height: f32) -> f32 {
        if line_height > 1.0 {
            line_height * 0.6
        } else {
            1.0
        }
    }

    #[test]
    fn measure_matches_pre_817_approximation_on_tui_metrics() {
        let backend = tui_shaped();
        let demo = DialogTableDemo::new();
        let m = demo.measure(&backend);
        let legacy_char_w = pre_817_char_width_approximation(backend.line_height);
        assert_eq!(
            m.button_width,
            legacy_char_w * 8.0,
            "on TUI metrics, backend.measure().char_width and the pre-#817 \
             approximation must agree -- this is what makes the switch a \
             no-op for every TUI driver test"
        );
    }

    /// On a pixel backend the pre-#817 approximation and the real
    /// `char_width` diverge (12.0 guessed vs. 9.0 real) -- this is the
    /// duplicated-font-metric-knowledge bug #817 exists to remove.
    /// Asserts `measure()` now reports the real value.
    #[test]
    fn measure_uses_real_char_width_not_the_pre_817_approximation_on_pixel_metrics() {
        let backend = pixel_shaped();
        let demo = DialogTableDemo::new();
        let m = demo.measure(&backend);
        let legacy_char_w = pre_817_char_width_approximation(backend.line_height);
        assert_ne!(
            legacy_char_w, backend.char_width,
            "test setup should pick metrics where the approximation is wrong"
        );
        assert_eq!(
            m.button_width,
            backend.char_width * 8.0,
            "measure() should use the backend's real char_width, not the \
             line_height * 0.6 guess"
        );
    }
}
