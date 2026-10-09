//! `Editor` demo — adapted from `quadraui/examples/common/hscroll_editor.rs`
//! and `editor_font_demo.rs`.
//!
//! Two variants exercise two unrelated `Editor` capabilities: scrolling
//! a single very long line horizontally (keeping the cursor visible),
//! and overriding the painted font via `Backend::set_editor_font` —
//! called directly here rather than through `ShellConfig::with_editor_font`,
//! since the gallery's single shared `ShellConfig` can't vary per-demo.

use std::cell::Cell;

use quadraui::{
    Backend, BackendCaps, Color, Editor, EditorCursor, EditorCursorPos, EditorCursorShape,
    EditorLine, EditorStyle, EditorStyledSpan, InteractionState, Key, NamedKey, Reaction, Rect,
    StatusBar, StatusBarSegment, UiEvent, WidgetId,
};

use crate::demo::{extract_region, Demo};

const SOURCE: &str = include_str!("editor.rs");

// gallery:begin
const LINE_LEN: usize = 500;
const DEMO_FONT_FAMILY: &str = "DejaVu Sans Mono";
const DEMO_FONT_SIZE_PT: f32 = 24.0;

fn plain_line(raw_text: String, gutter: &str, idx: usize) -> EditorLine {
    let fg = Color::rgb(220, 220, 220);
    let len = raw_text.len();
    EditorLine {
        raw_text,
        gutter_text: gutter.to_string(),
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
}

/// Variant 0 state: horizontal-scroll smoke test.
struct HScrollState {
    cursor_col: usize,
    scroll_left: usize,
}

impl HScrollState {
    fn new() -> Self {
        Self {
            cursor_col: 0,
            scroll_left: 0,
        }
    }

    fn line_text() -> String {
        (0..LINE_LEN)
            .map(|i| ((i % 10) as u8 + b'0') as char)
            .collect()
    }

    fn viewport_cols(area: Rect, backend: &dyn Backend) -> usize {
        let cw = backend.char_width();
        let gutter_w = 4.0 * cw;
        ((area.width - gutter_w) / cw).floor().max(1.0) as usize
    }

    fn ensure_cursor_visible(&mut self, viewport_cols: usize) {
        if self.cursor_col < self.scroll_left {
            self.scroll_left = self.cursor_col;
        } else if self.cursor_col >= self.scroll_left + viewport_cols {
            self.scroll_left = self.cursor_col + 1 - viewport_cols;
        }
    }

    fn build_editor(&self, area: Rect) -> Editor {
        let line = plain_line(Self::line_text(), "   1", 0);
        Editor::new(WidgetId::new("gallery:editor:hscroll"), area)
            .with_lines(vec![line])
            .with_cursor(EditorCursor {
                pos: EditorCursorPos {
                    view_line: 0,
                    col: self.cursor_col,
                },
                shape: EditorCursorShape::Block,
            })
            .with_scroll_left(self.scroll_left)
            .with_total_lines(1)
            .with_max_col(LINE_LEN)
            .with_gutter_char_width(4)
            .with_is_active(true)
            .with_cursorline(true)
            .with_lightbulb_glyph('\0')
    }
}

/// Variant 1 state: font-override smoke test. `applied` tracks the font
/// actually pushed to the backend so repeated renders of the same
/// variant don't re-issue `set_editor_font` every frame — mirroring the
/// `Caret Shape` demo's debounce pattern.
struct FontState {
    applied: Cell<bool>,
    calls: Cell<u32>,
}

impl FontState {
    fn new() -> Self {
        Self {
            applied: Cell::new(false),
            calls: Cell::new(0),
        }
    }

    fn build_editor(area: Rect) -> Editor {
        let texts = [
            "The quick brown fox jumps over the lazy dog",
            "set_editor_font() painted this at 24pt DejaVu Sans Mono",
        ];
        let lines = texts
            .iter()
            .enumerate()
            .map(|(idx, text)| plain_line(text.to_string(), &format!("{:>4}", idx + 1), idx))
            .collect();
        Editor::new(WidgetId::new("gallery:editor:font"), area)
            .with_lines(lines)
            .with_cursor(EditorCursor {
                pos: EditorCursorPos {
                    view_line: 0,
                    col: 0,
                },
                shape: EditorCursorShape::Block,
            })
            .with_total_lines(texts.len())
            .with_gutter_char_width(4)
            .with_is_active(true)
            .with_cursorline(true)
            .with_lightbulb_glyph('\0')
    }
}

pub struct EditorDemo {
    hscroll: HScrollState,
    font: FontState,
}

impl EditorDemo {
    pub fn new() -> Self {
        Self {
            hscroll: HScrollState::new(),
            font: FontState::new(),
        }
    }
}

impl Default for EditorDemo {
    fn default() -> Self {
        Self::new()
    }
}

impl Demo for EditorDemo {
    fn name(&self) -> &'static str {
        "Editor"
    }

    fn group(&self) -> &'static str {
        "Content"
    }

    fn variants(&self) -> &'static [&'static str] {
        &["Horizontal scroll", "Font override"]
    }

    fn render(&self, variant: usize, backend: &mut dyn Backend, area: Rect) {
        let lh = backend.line_height();
        let bar_h = if lh > 1.5 { lh * 1.5 } else { lh };
        let editor_area = Rect::new(area.x, area.y, area.width, (area.height - bar_h).max(0.0));
        let bar_rect = Rect::new(area.x, area.y + editor_area.height, area.width, bar_h);

        let status = match variant {
            1 => {
                if !self.font.applied.get() {
                    backend.set_editor_font(DEMO_FONT_FAMILY, DEMO_FONT_SIZE_PT);
                    self.font.applied.set(true);
                    self.font.calls.set(self.font.calls.get() + 1);
                }
                let editor = FontState::build_editor(editor_area);
                backend.draw_editor(editor.rect, &editor);
                format!(
                    " editor font: {DEMO_FONT_FAMILY} {DEMO_FONT_SIZE_PT}pt  calls: {} ",
                    self.font.calls.get()
                )
            }
            _ => {
                let editor = self.hscroll.build_editor(editor_area);
                backend.draw_editor(editor.rect, &editor);
                let vpc = HScrollState::viewport_cols(editor_area, backend);
                format!(
                    " col {} / {}  scroll_left {}  viewport_cols {} ",
                    self.hscroll.cursor_col + 1,
                    LINE_LEN,
                    self.hscroll.scroll_left,
                    vpc
                )
            }
        };

        let bar = StatusBar {
            id: WidgetId::new("gallery:editor:status"),
            left_segments: vec![StatusBarSegment {
                text: status,
                fg: Color::rgb(255, 255, 255),
                bg: Color::rgb(40, 80, 120),
                bold: false,
                action_id: None,
            }],
            right_segments: vec![],
        };
        let _ = backend.draw_status_bar_interactive(bar_rect, &bar, &InteractionState::new());
    }

    fn handle(
        &mut self,
        variant: usize,
        event: &UiEvent,
        backend: &mut dyn Backend,
        area: Rect,
    ) -> Reaction {
        if variant != 0 {
            return Reaction::Continue;
        }
        let vpc = HScrollState::viewport_cols(area, backend);
        match event {
            UiEvent::KeyPressed { key, .. } => {
                match key {
                    Key::Char('$') | Key::Named(NamedKey::End) => {
                        self.hscroll.cursor_col = LINE_LEN - 1;
                    }
                    Key::Char('0') | Key::Named(NamedKey::Home) => {
                        self.hscroll.cursor_col = 0;
                    }
                    Key::Char('l') | Key::Named(NamedKey::Right) => {
                        if self.hscroll.cursor_col < LINE_LEN - 1 {
                            self.hscroll.cursor_col += 1;
                        }
                    }
                    Key::Char('h') | Key::Named(NamedKey::Left) => {
                        self.hscroll.cursor_col = self.hscroll.cursor_col.saturating_sub(1);
                    }
                    _ => return Reaction::Continue,
                }
                self.hscroll.ensure_cursor_visible(vpc);
                Reaction::Redraw
            }
            _ => Reaction::Continue,
        }
    }

    fn source(&self) -> &'static str {
        extract_region(SOURCE)
    }

    fn data(&self, variant: usize) -> serde_json::Value {
        match variant {
            1 => serde_json::json!({
                "font_family": DEMO_FONT_FAMILY,
                "font_size_pt": DEMO_FONT_SIZE_PT,
                "set_editor_font_calls": self.font.calls.get(),
            }),
            _ => serde_json::json!({
                "cursor_col": self.hscroll.cursor_col,
                "scroll_left": self.hscroll.scroll_left,
                "line_len": LINE_LEN,
            }),
        }
    }

    fn caps_note(&self, variant: usize, caps: &BackendCaps) -> Option<String> {
        if variant == 1 && !caps.generic_font_families {
            Some(
                "This backend has a fixed cell grid — set_editor_font is a structural \
                 no-op here; the font override has no visible effect."
                    .to_string(),
            )
        } else {
            None
        }
    }
}
// gallery:end

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hscroll_end_key_jumps_cursor_and_scrolls() {
        let mut demo = EditorDemo::new();
        let mut backend = quadraui::testing::RecordingBackend::new();
        let area = Rect::new(0.0, 0.0, 40.0, 10.0);
        let reaction = demo.handle(
            0,
            &UiEvent::KeyPressed {
                key: Key::Char('$'),
                modifiers: Default::default(),
                repeat: false,
            },
            &mut backend,
            area,
        );
        assert!(matches!(reaction, Reaction::Redraw));
        assert_eq!(demo.hscroll.cursor_col, LINE_LEN - 1);
        assert!(demo.hscroll.scroll_left > 0);
    }

    #[test]
    fn font_variant_ignores_hscroll_keys() {
        let mut demo = EditorDemo::new();
        let mut backend = quadraui::testing::RecordingBackend::new();
        let area = Rect::new(0.0, 0.0, 40.0, 10.0);
        let reaction = demo.handle(
            1,
            &UiEvent::KeyPressed {
                key: Key::Char('$'),
                modifiers: Default::default(),
                repeat: false,
            },
            &mut backend,
            area,
        );
        assert!(matches!(reaction, Reaction::Continue));
        assert_eq!(demo.hscroll.cursor_col, 0);
    }
}
