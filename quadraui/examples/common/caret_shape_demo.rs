//! Hardware-caret demo (issue #1015) — exercises `Backend::set_caret_shape`
//! from app code instead of a consumer hand-rolling DECSCUSR itself (the
//! rule-6 violation this method closes off; see vimcode's old
//! `shell_app.rs` `execute!(SetCursorStyle …)` call site).
//!
//! Cycles an `Editor`'s [`EditorCursorShape`] through the three modes the
//! primitive defines and calls `backend.set_caret_shape` exactly once per
//! mode *change* — never every frame — matching
//! [`quadraui::Backend::set_caret_shape`]'s doc on why debouncing is the
//! caller's job, not `draw_editor`'s.
//!
//! There is no paintable, headless-observable effect for the *hardware*
//! caret itself (DECSCUSR is a raw terminal write with nothing in a
//! `TestBackend` buffer to assert against — the same "acts on something
//! outside the frame buffer" shape `window_control_demo`'s real capability
//! has). What is paintable and driver-testable is that the call was
//! issued for the right mode, which is what the status bar shows.
//!
//! Keys: `i` Insert (Bar), `r` Replace-pending (Underline), `n` Normal
//! (Block). Esc quits.

use quadraui::{
    AppLogic, Backend, Color, Editor, EditorCursor, EditorCursorPos, EditorCursorShape, EditorLine,
    EditorStyle, EditorStyledSpan, InteractionState, Key, NamedKey, Reaction, Rect, StatusBar,
    StatusBarSegment, UiEvent, WidgetId,
};

pub struct CaretShapeDemo {
    shape: EditorCursorShape,
    calls: u32,
}

impl CaretShapeDemo {
    pub fn new() -> Self {
        Self {
            shape: EditorCursorShape::Block,
            calls: 0,
        }
    }

    fn mode_name(shape: EditorCursorShape) -> &'static str {
        match shape {
            EditorCursorShape::Block => "Normal",
            EditorCursorShape::Bar => "Insert",
            EditorCursorShape::Underline => "Replace-pending",
        }
    }

    /// Applies a mode change: only calls `set_caret_shape` when the shape
    /// actually differs from the current one, since repainting with the
    /// same shape every frame is exactly the "comparatively heavy terminal
    /// write" the trait method's doc says callers should avoid issuing
    /// redundantly.
    fn set_mode(&mut self, shape: EditorCursorShape, backend: &mut dyn Backend) {
        if self.shape != shape {
            self.shape = shape;
            backend.set_caret_shape(shape);
            self.calls += 1;
        }
    }

    fn build_editor(&self, backend: &dyn Backend) -> Editor {
        let vp = backend.viewport();
        let lh = backend.line_height();
        let bar_h = if lh > 1.5 { lh * 1.5 } else { lh };
        let editor_h = vp.height - bar_h;
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

        Editor {
            id: WidgetId::new("editor"),
            rect: Rect::new(0.0, 0.0, vp.width, editor_h),
            lines: vec![line],
            cursor: Some(EditorCursor {
                pos: EditorCursorPos {
                    view_line: 0,
                    col: 0,
                },
                shape: self.shape,
            }),
            extra_cursors: vec![],
            selection: None,
            extra_selections: vec![],
            yank_highlight: None,
            scroll_top: 0,
            scroll_left: 0,
            total_lines: 1,
            max_col: 20,
            gutter_char_width: 4,
            is_active: true,
            show_active_bg: false,
            has_git_diff: false,
            has_breakpoints: false,
            diagnostic_gutter: Default::default(),
            code_action_lines: Default::default(),
            bracket_match_positions: vec![],
            active_indent_col: None,
            tabstop: 4,
            cursorline: true,
            lightbulb_glyph: '\0',
        }
    }

    fn status_bar(&self) -> StatusBar {
        let info = format!(
            " mode: {}  set_caret_shape calls: {} ",
            Self::mode_name(self.shape),
            self.calls,
        );
        StatusBar {
            id: WidgetId::new("caret-shape-demo:status"),
            left_segments: vec![StatusBarSegment {
                text: " Caret shape demo (#1015) — i=insert r=replace n=normal ".into(),
                fg: Color::rgb(255, 255, 255),
                bg: Color::rgb(40, 80, 120),
                bold: true,
                action_id: None,
            }],
            right_segments: vec![StatusBarSegment {
                text: info,
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

impl AppLogic for CaretShapeDemo {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let editor = self.build_editor(backend);
        backend.draw_editor(editor.rect, &editor);

        let vp = backend.viewport();
        let lh = backend.line_height();
        let bar_h = if lh > 1.5 { lh * 1.5 } else { lh };
        let bar_rect = Rect::new(0.0, vp.height - bar_h, vp.width, bar_h);
        let _ = backend.draw_status_bar_interactive(
            bar_rect,
            &self.status_bar(),
            &InteractionState::new(),
        );
    }

    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
        match event {
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Escape),
                ..
            } => Reaction::Exit,
            UiEvent::KeyPressed {
                key: Key::Char('i'),
                ..
            } => {
                self.set_mode(EditorCursorShape::Bar, backend);
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Char('r'),
                ..
            } => {
                self.set_mode(EditorCursorShape::Underline, backend);
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Char('n'),
                ..
            } => {
                self.set_mode(EditorCursorShape::Block, backend);
                Reaction::Redraw
            }
            UiEvent::WindowResized { .. } => Reaction::Redraw,
            _ => Reaction::Continue,
        }
    }
}
