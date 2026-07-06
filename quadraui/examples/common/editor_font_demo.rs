//! GTK-only demo for `Backend::set_editor_font` / `ShellConfig::with_editor_font`
//! (#422): proves an app can override the editor's painted font (family +
//! size) instead of being stuck with the runner's hardcoded `"Monospace 11"`.
//!
//! `config()` calls `.with_editor_font("DejaVu Sans Mono", 24.0)` — a large
//! size chosen so the difference from the historical 11pt default is
//! obvious when the window opens. `ShellAdapter::setup()` applies it via
//! `Backend::set_editor_font` before the first frame; `gtk/run.rs`'s draw
//! closure reads it back every frame to build the shared Pango layout, so
//! the painted glyphs and the `line_height()`/`char_width()` metrics used
//! for click-column math both derive from the configured font.
//!
//! No TUI pair: TUI is a fixed-cell backend with no font concept —
//! `Backend::set_editor_font` no-ops there, so there is nothing new to see.
//! This mirrors existing GTK-only runner capabilities (e.g. CSD
//! window-drag/resize, #400/#406) that also shipped without a TUI demo.

use quadraui::compose::app_shell::AppShellLayout;
use quadraui::{
    Backend, Color, Editor, EditorCursor, EditorCursorPos, EditorCursorShape, EditorLine,
    EditorStyle, EditorStyledSpan, Key, NamedKey, Reaction, Rect, ShellApp, ShellConfig,
    ShellContext, StatusBar, StatusBarSegment, UiEvent, WidgetId,
};

const DEMO_FONT_FAMILY: &str = "DejaVu Sans Mono";
const DEMO_FONT_SIZE_PT: f32 = 24.0;

pub struct EditorFontDemo;

impl EditorFontDemo {
    pub fn new() -> Self {
        Self
    }

    /// `ShellConfig` with the editor font overridden to a large monospace
    /// size — the visible proof this demo exists to show.
    pub fn config() -> ShellConfig {
        ShellConfig::new("Editor Font Demo", Vec::new())
            .with_editor_font(DEMO_FONT_FAMILY, DEMO_FONT_SIZE_PT)
    }

    fn build_editor(&self, main: Rect) -> Editor {
        let fg = Color::rgb(220, 220, 220);
        let texts = [
            "The quick brown fox jumps over the lazy dog",
            "set_editor_font() painted this at 24pt DejaVu Sans Mono",
        ];
        let lines = texts
            .iter()
            .enumerate()
            .map(|(idx, text)| {
                let text = text.to_string();
                let len = text.len();
                EditorLine {
                    raw_text: text,
                    gutter_text: format!("{:>4}", idx + 1),
                    spans: vec![EditorStyledSpan {
                        start_byte: 0,
                        end_byte: len,
                        style: EditorStyle {
                            fg,
                            bg: None,
                            bold: false,
                            italic: false,
                            font_scale: 1.0,
                        },
                    }],
                    line_idx: idx,
                    is_current_line: idx == 0,
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
                }
            })
            .collect::<Vec<_>>();

        Editor {
            id: WidgetId::new("editor-font-demo"),
            rect: main,
            lines,
            cursor: Some(EditorCursor {
                pos: EditorCursorPos {
                    view_line: 0,
                    col: 0,
                },
                shape: EditorCursorShape::Block,
            }),
            extra_cursors: vec![],
            selection: None,
            extra_selections: vec![],
            yank_highlight: None,
            scroll_top: 0,
            scroll_left: 0,
            total_lines: texts.len(),
            max_col: 0,
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
}

impl Default for EditorFontDemo {
    fn default() -> Self {
        Self::new()
    }
}

impl ShellApp for EditorFontDemo {
    fn render_content(&self, backend: &mut dyn Backend, layout: &AppShellLayout) {
        let lh = backend.line_height();
        let main = layout.main_content_bounds;
        let editor_rect = Rect::new(main.x, main.y, main.width, main.height - lh);
        let editor = self.build_editor(editor_rect);
        backend.draw_editor(editor.rect, &editor);

        let status = StatusBar {
            id: WidgetId::new("editor-font-demo-status"),
            left_segments: vec![StatusBarSegment {
                text: format!(
                    " editor font: {DEMO_FONT_FAMILY} {DEMO_FONT_SIZE_PT}pt (q to quit) "
                ),
                fg: Color::rgb(255, 255, 255),
                bg: Color::rgb(40, 80, 120),
                bold: true,
                action_id: None,
            }],
            right_segments: vec![],
        };
        let status_rect = Rect::new(main.x, main.y + main.height - lh, main.width, lh);
        backend.draw_status_bar(status_rect, &status, None, None);
    }

    fn handle(
        &mut self,
        event: UiEvent,
        _backend: &mut dyn Backend,
        _ctx: &ShellContext,
    ) -> Reaction {
        match event {
            UiEvent::KeyPressed {
                key: Key::Char('q') | Key::Named(NamedKey::Escape),
                ..
            } => Reaction::Exit,
            _ => Reaction::Continue,
        }
    }
}
