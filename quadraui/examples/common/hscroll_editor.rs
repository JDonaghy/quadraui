//! Smoke test for horizontal scrolling: a single-line 500-char editor.
//!
//! Keys:
//! - `$` / `End` — jump cursor to end of line, scroll to show it
//! - `0` / `Home` — jump cursor to start of line
//! - `l` / `Right` — move cursor right
//! - `h` / `Left` — move cursor left
//! - `q` / `Esc` — quit

use quadraui::{
    AppLogic, Backend, Color, Editor, EditorCursor, EditorCursorPos, EditorCursorShape, EditorLine,
    EditorStyle, EditorStyledSpan, Key, NamedKey, Reaction, Rect, StatusBar, StatusBarSegment,
    UiEvent, WidgetId,
};

const LINE_LEN: usize = 500;

pub struct HScrollEditor {
    cursor_col: usize,
    scroll_left: usize,
}

impl HScrollEditor {
    pub fn new() -> Self {
        Self {
            cursor_col: 0,
            scroll_left: 0,
        }
    }

    fn line_text(&self) -> String {
        let mut s = String::with_capacity(LINE_LEN);
        for i in 0..LINE_LEN {
            let digit = (i % 10).to_string();
            s.push_str(&digit);
        }
        s
    }

    fn viewport_cols(&self, backend: &dyn Backend) -> usize {
        let vp = backend.viewport();
        let cw = backend.char_width();
        let gutter_chars = 4;
        let gutter_w = gutter_chars as f32 * cw;
        ((vp.width - gutter_w) / cw).floor().max(1.0) as usize
    }

    fn ensure_cursor_visible(&mut self, viewport_cols: usize) {
        if self.cursor_col < self.scroll_left {
            self.scroll_left = self.cursor_col;
        } else if self.cursor_col >= self.scroll_left + viewport_cols {
            self.scroll_left = self.cursor_col + 1 - viewport_cols;
        }
    }

    fn build_editor(&self, backend: &dyn Backend) -> Editor {
        let vp = backend.viewport();
        let lh = backend.line_height();
        let bar_h = if lh > 1.5 { lh * 1.5 } else { lh };
        let editor_h = vp.height - bar_h;
        let text = self.line_text();
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
                    col: self.cursor_col,
                },
                shape: EditorCursorShape::Block,
            }),
            extra_cursors: vec![],
            selection: None,
            extra_selections: vec![],
            yank_highlight: None,
            scroll_top: 0,
            scroll_left: self.scroll_left,
            total_lines: 1,
            max_col: LINE_LEN,
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

    fn status_bar(&self, viewport_cols: usize) -> StatusBar {
        let info = format!(
            " col {} / {}  scroll_left {}  viewport_cols {} ",
            self.cursor_col + 1,
            LINE_LEN,
            self.scroll_left,
            viewport_cols,
        );
        StatusBar {
            id: WidgetId::new("status"),
            left_segments: vec![StatusBarSegment {
                text: " h-scroll smoke test ".into(),
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

impl Default for HScrollEditor {
    fn default() -> Self {
        Self::new()
    }
}

impl AppLogic for HScrollEditor {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let editor = self.build_editor(backend);
        backend.draw_editor(editor.rect, &editor);

        let vp = backend.viewport();
        let lh = backend.line_height();
        let bar_h = if lh > 1.5 { lh * 1.5 } else { lh };
        let bar_rect = Rect::new(0.0, vp.height - bar_h, vp.width, bar_h);
        let vpc = self.viewport_cols(backend);
        let bar = self.status_bar(vpc);
        let _ = backend.draw_status_bar(bar_rect, &bar, None, None);
    }

    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
        let vpc = self.viewport_cols(backend);
        match event {
            UiEvent::KeyPressed { key, .. } => {
                match key {
                    Key::Char('q') | Key::Named(NamedKey::Escape) => return Reaction::Exit,
                    Key::Char('$') | Key::Named(NamedKey::End) => {
                        self.cursor_col = LINE_LEN - 1;
                    }
                    Key::Char('0') | Key::Named(NamedKey::Home) => {
                        self.cursor_col = 0;
                    }
                    Key::Char('l') | Key::Named(NamedKey::Right) => {
                        if self.cursor_col < LINE_LEN - 1 {
                            self.cursor_col += 1;
                        }
                    }
                    Key::Char('h') | Key::Named(NamedKey::Left) => {
                        self.cursor_col = self.cursor_col.saturating_sub(1);
                    }
                    _ => {}
                }
                self.ensure_cursor_visible(vpc);
                Reaction::Redraw
            }
            UiEvent::WindowResized { .. } => {
                self.ensure_cursor_visible(vpc);
                Reaction::Redraw
            }
            _ => Reaction::Continue,
        }
    }
}
