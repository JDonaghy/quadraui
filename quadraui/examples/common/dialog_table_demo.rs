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
//! - [`Dialog::measure_generic`] (quadraui#419) for deriving the
//!   `DialogMeasure` itself — this demo used to hand-roll the same
//!   char-cell arithmetic inline; it now dogfoods the library helper.
//! - The `draw_dialog` table rendering path (column separators + header row)

use quadraui::{
    AppLogic, Backend, Dialog, DialogButton, DialogMeasure, DialogTable, FontRole, Key, NamedKey,
    Reaction, Rect, StyledText, ToolbarItemMeasure, UiEvent, WidgetId,
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

    /// Compute a [`DialogMeasure`] from `backend.measure()` via
    /// [`Dialog::measure_generic`] (quadraui#419), then overrides
    /// `button_width` with a real per-label measurement via
    /// [`crate::Backend::measure_text`] (quadraui#1132).
    ///
    /// `line_height` is 1.0 on TUI (one character cell) and the pixel line
    /// height on GTK/macOS. `char_width` is likewise the real glyph width
    /// on pixel backends — [`crate::Backend::measure`]'s [`crate::Metrics`]
    /// is the one source of truth, same as pre-#419. `measure_generic`
    /// itself still sizes `button_width` as a flat `char_width * 8.0`
    /// guess (its own doc: "does not measure … buttons' rendered pixel
    /// width beyond the char-cell approximation … callers with real text
    /// measurement available should prefer measuring directly") — right
    /// for this dialog's one `"Close"` button on TUI's fixed grid, but a
    /// proportional chrome font on GTK/macOS/Win has no single
    /// `char_width` to multiply by 8. This method is that "measure
    /// directly" caller: the real button-label width in the chrome font,
    /// plus the same two-char-width padding `measure_generic` already
    /// budgets. `border_chrome_inset` is `0.0`: both this repo's TUI and
    /// GTK `draw_dialog` paint the border *inside* `DialogLayout::bounds`,
    /// so no extra inset is needed.
    fn measure(&self, backend: &dyn Backend) -> DialogMeasure {
        let m = backend.measure();
        let viewport = backend.viewport();
        let viewport_rect = Rect::new(0.0, 0.0, viewport.width, viewport.height);
        let mut measure =
            self.dialog
                .measure_generic(m.char_width, m.line_height, viewport_rect, 0.0);
        if let Some(max_label_w) = self
            .dialog
            .buttons
            .iter()
            .map(|b| backend.measure_text(&b.label, FontRole::Chrome).0)
            .fold(None::<f32>, |acc, w| Some(acc.map_or(w, |a| a.max(w))))
        {
            measure.button_width = max_label_w + m.char_width * 2.0;
        }
        measure
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
        // quadraui#1132: `button_width` is no longer the flat `char_width *
        // 8.0` guess -- it's "Close".len() (5) char-widths of real
        // measurement plus 2 char-widths of padding, which coincide with
        // the old 8-char-width approximation only because `measure_text`
        // on `RecordingBackend` is itself char-count-proportional (no real
        // font to shape against). TUI driver tests still see the same
        // layout either way: 5 + 2 == 7, one char-width narrower than the
        // old flat 8, with no label that would notice the difference.
        assert_eq!(m.button_width, legacy_char_w * 7.0);
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
        // quadraui#1132: real "Close"-label measurement (5 char-widths) +
        // 2 char-widths of padding, not a flat 8 -- see the TUI test's doc
        // above for why these are both still char_width-proportional on
        // `RecordingBackend` specifically.
        assert_eq!(
            m.button_width,
            backend.char_width * 7.0,
            "measure() should use the backend's real char_width via \
             measure_text, not the line_height * 0.6 guess"
        );
    }

    /// quadraui#1132's actual point: `button_width` must track each
    /// button's *own* label length through [`Backend::measure_text`]
    /// instead of a dialog-shape-independent flat guess -- a dialog with
    /// a much longer button label must get a wider button box, which the
    /// pre-#1132 `char_width * 8.0` formula could never reflect (every
    /// dialog got the same width regardless of its label).
    #[test]
    fn measure_button_width_tracks_the_actual_label_length() {
        let backend = pixel_shaped();
        let short = DialogTableDemo::new(); // "Close", 5 chars
        let mut dialog = short.dialog.clone();
        dialog.buttons[0].label = "Discard All Changes".into(); // 20 chars
        let long = DialogTableDemo { dialog };

        let short_w = short.measure(&backend).button_width;
        let long_w = long.measure(&backend).button_width;
        assert!(
            long_w > short_w,
            "a longer button label must widen the button box: short={short_w}, long={long_w}"
        );
    }
}
