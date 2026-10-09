//! `Caret Shape` demo — adapted from
//! `quadraui/examples/common/caret_shape_demo.rs`.
//!
//! Exercises `Backend::set_caret_shape` — steering the terminal/OS
//! **hardware** caret to match an `Editor`'s `EditorCursorShape`. Each
//! variant is one of the three shapes the primitive defines; switching
//! variants calls `backend.set_caret_shape` exactly once per shape
//! *change*, mirroring that method's debouncing contract (never call it
//! every frame).

use std::cell::Cell;

use quadraui::{
    Backend, BackendCaps, Color, Editor, EditorCursor, EditorCursorPos, EditorCursorShape,
    EditorLine, EditorStyle, EditorStyledSpan, InteractionState, Reaction, Rect, StatusBar,
    StatusBarSegment, UiEvent, WidgetId,
};

use crate::demo::{extract_region, Demo};

const SOURCE: &str = include_str!("caret_shape.rs");

// gallery:begin
pub struct CaretShapeDemo {
    /// Last shape actually applied via `set_caret_shape`, so repeat
    /// renders of the same variant don't re-issue the (comparatively
    /// heavy) terminal write.
    applied: Cell<Option<EditorCursorShape>>,
    calls: Cell<u32>,
}

impl CaretShapeDemo {
    pub fn new() -> Self {
        Self {
            applied: Cell::new(None),
            calls: Cell::new(0),
        }
    }

    fn shape_for_variant(variant: usize) -> EditorCursorShape {
        match variant {
            1 => EditorCursorShape::Bar,
            2 => EditorCursorShape::Underline,
            _ => EditorCursorShape::Block,
        }
    }

    fn mode_name(shape: EditorCursorShape) -> &'static str {
        match shape {
            EditorCursorShape::Block => "Normal",
            EditorCursorShape::Bar => "Insert",
            EditorCursorShape::Underline => "Replace-pending",
        }
    }

    fn build_editor(shape: EditorCursorShape, area: Rect) -> Editor {
        let text = "the quick brown fox".to_string();
        let text_len = text.len();
        let fg = Color::rgb(220, 220, 220);
        let line = EditorLine {
            raw_text: text,
            gutter_text: "   1".into(),
            spans: vec![EditorStyledSpan {
                start_byte: 0,
                end_byte: text_len,
                style: EditorStyle {
                    fg,
                    bg: None,
                    bold: false,
                    italic: false,
                    font_scale: 1.0,
                },
            }],
            line_idx: 0,
            is_current_line: true,
            is_fold_header: false,
            folded_line_count: 0,
            git_diff: None,
            diff_status: None,
            diagnostics: vec![],
            spell_errors: vec![],
            is_breakpoint: false,
            is_conditional_bp: false,
            is_dap_current: false,
            is_wrap_continuation: false,
            segment_col_offset: 0,
            annotation: None,
            ghost_suffix: None,
            is_ghost_continuation: false,
            indent_guides: vec![],
            colorcolumns: vec![],
        };
        Editor::new(WidgetId::new("gallery:caret-shape:editor"), area)
            .with_lines(vec![line])
            .with_cursor(EditorCursor {
                pos: EditorCursorPos {
                    view_line: 0,
                    col: 0,
                },
                shape,
            })
            .with_total_lines(1)
            .with_max_col(20)
            .with_gutter_char_width(4)
            .with_is_active(true)
            .with_cursorline(true)
            .with_lightbulb_glyph('\0')
    }

    fn status_bar(&self, shape: EditorCursorShape) -> StatusBar {
        StatusBar {
            id: WidgetId::new("gallery:caret-shape:status"),
            left_segments: vec![StatusBarSegment {
                text: " Backend::set_caret_shape ".into(),
                fg: Color::rgb(255, 255, 255),
                bg: Color::rgb(40, 80, 120),
                bold: true,
                action_id: None,
            }],
            right_segments: vec![StatusBarSegment {
                text: format!(
                    " mode: {}  calls: {} ",
                    Self::mode_name(shape),
                    self.calls.get()
                ),
                fg: Color::rgb(220, 220, 220),
                bg: Color::rgb(40, 80, 120),
                bold: false,
                action_id: None,
            }],
        }
    }
}

impl Default for CaretShapeDemo {
    fn default() -> Self {
        Self::new()
    }
}

impl Demo for CaretShapeDemo {
    fn name(&self) -> &'static str {
        "Caret Shape"
    }

    fn group(&self) -> &'static str {
        "Content"
    }

    fn variants(&self) -> &'static [&'static str] {
        &["Normal (Block)", "Insert (Bar)", "Replace (Underline)"]
    }

    fn render(&self, variant: usize, backend: &mut dyn Backend, area: Rect) {
        let shape = Self::shape_for_variant(variant);
        if self.applied.get() != Some(shape) {
            backend.set_caret_shape(shape);
            self.applied.set(Some(shape));
            self.calls.set(self.calls.get() + 1);
        }

        let lh = backend.line_height();
        let bar_h = if lh > 1.5 { lh * 1.5 } else { lh };
        let editor_area = Rect::new(area.x, area.y, area.width, (area.height - bar_h).max(0.0));
        let editor = Self::build_editor(shape, editor_area);
        backend.draw_editor(editor.rect, &editor);

        let bar_rect = Rect::new(area.x, area.y + editor_area.height, area.width, bar_h);
        let _ = backend.draw_status_bar_interactive(
            bar_rect,
            &self.status_bar(shape),
            &InteractionState::new(),
        );
    }

    fn handle(
        &mut self,
        _variant: usize,
        _event: &UiEvent,
        _backend: &mut dyn Backend,
        _area: Rect,
    ) -> Reaction {
        Reaction::Continue
    }

    fn source(&self) -> &'static str {
        extract_region(SOURCE)
    }

    fn data(&self, variant: usize) -> serde_json::Value {
        let shape = Self::shape_for_variant(variant);
        serde_json::json!({
            "mode": Self::mode_name(shape),
            "set_caret_shape_calls": self.calls.get(),
        })
    }

    fn caps_note(&self, _variant: usize, _caps: &BackendCaps) -> Option<String> {
        // `Backend::set_caret_shape` is a genuine, permanent no-op on
        // every GUI backend by design (they paint their own caret and
        // have no separate hardware cursor to steer) — not a capability
        // gap `BackendCaps` tracks, so there is nothing to report here.
        None
    }
}
// gallery:end

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shape_for_variant_covers_all_three_variants() {
        assert_eq!(
            CaretShapeDemo::shape_for_variant(0),
            EditorCursorShape::Block
        );
        assert_eq!(CaretShapeDemo::shape_for_variant(1), EditorCursorShape::Bar);
        assert_eq!(
            CaretShapeDemo::shape_for_variant(2),
            EditorCursorShape::Underline
        );
    }
}

// The "switch variants → `set_caret_shape` calls once per change" claim
// needs a real ratatui `Frame` (`Backend::draw_editor` panics outside
// `TuiBackend::enter_frame_scope`, which only a `Terminal::draw` call —
// i.e. a full `TuiDriver` render — opens). That integration-level check
// lives in `tests/gallery_driver.rs`, driven through the real gallery
// shell, rather than here against a bare `TuiBackend`.
