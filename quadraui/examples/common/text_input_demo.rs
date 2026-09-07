//! TextInput demo — visual smoke test for the `TextInput` primitive's
//! editing behaviour.
//!
//! Renders a single multi-line `TextInput` filling most of the viewport.
//! All editing goes through `TextInput::apply(EditOp)` (issue #833) —
//! this file no longer hand-rolls insert/backspace/cursor-movement logic
//! itself. Exercises cursor positioning, line wrap on Enter, scroll
//! auto-clamp, placeholder rendering, Shift+arrow selection, and
//! Ctrl+Z/Ctrl+Shift+Z undo/redo (bound to the universal
//! `KeyBinding::Undo`/`KeyBinding::Redo` accelerator names).

use quadraui::{
    Accelerator, AcceleratorId, AcceleratorScope, AppLogic, Backend, Color, EditOp,
    InteractionState, Key, KeyBinding, MouseButton, NamedKey, Reaction, Rect, StatusBar,
    StatusBarSegment, TextInput, TextInputHit, UiEvent, UndoableTextInput, WidgetId,
};

const UNDO_ACCEL: &str = "text_input_demo.undo";
const REDO_ACCEL: &str = "text_input_demo.redo";

pub struct TextInputDemo {
    input: UndoableTextInput,
}

impl TextInputDemo {
    pub fn new() -> Self {
        let mut input = TextInput::new(WidgetId::new("demo:input"));
        input.placeholder = Some(
            "Type something. Enter for newline. Arrows/Home/End to move, Shift to select. \
             Ctrl+Z/Ctrl+Shift+Z to undo/redo. Esc to quit."
                .into(),
        );
        input.has_focus = true;
        Self {
            input: UndoableTextInput::new(input),
        }
    }

    /// Same `input_rect` geometry `render` uses — shared so
    /// `handle`'s click routing can't drift from where the primitive
    /// was actually painted (quadraui#818).
    fn input_rect(backend: &dyn Backend) -> Rect {
        let viewport = backend.viewport();
        let lh = backend.line_height();
        let status_h = lh;
        let pad = lh;
        Rect::new(
            pad,
            pad,
            viewport.width - pad * 2.0,
            viewport.height - status_h - pad * 2.0,
        )
    }

    fn status(&self) -> StatusBar {
        let cursor = format!(
            " line {} col {} — Esc to quit ",
            self.input.cursor_line + 1,
            self.input.cursor_col + 1,
        );
        StatusBar {
            id: WidgetId::new("demo:status"),
            left_segments: vec![StatusBarSegment {
                text: " TextInput demo ".into(),
                fg: Color::rgb(255, 255, 255),
                bg: Color::rgb(40, 80, 120),
                bold: true,
                action_id: None,
            }],
            right_segments: vec![StatusBarSegment {
                text: cursor,
                fg: Color::rgb(220, 220, 220),
                bg: Color::rgb(40, 80, 120),
                bold: false,
                action_id: None,
            }],
        }
    }
}

impl Default for TextInputDemo {
    fn default() -> Self {
        Self::new()
    }
}

impl AppLogic for TextInputDemo {
    type AreaId = ();

    /// Registers the demo's undo/redo accelerators — see #833's module
    /// doc. `Backend::register_accelerator` resolves `Global`-scope
    /// bindings natively, translating a matching raw key press straight
    /// into `UiEvent::Accelerator` before `handle` ever sees it.
    fn setup(&mut self, backend: &mut dyn Backend) {
        backend.register_accelerator(&Accelerator {
            id: AcceleratorId::new(UNDO_ACCEL),
            binding: KeyBinding::Undo,
            scope: AcceleratorScope::Global,
            label: None,
        });
        backend.register_accelerator(&Accelerator {
            id: AcceleratorId::new(REDO_ACCEL),
            binding: KeyBinding::Redo,
            scope: AcceleratorScope::Global,
            label: None,
        });
    }

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let viewport = backend.viewport();
        let status_h = backend.line_height();

        let status_rect = Rect::new(0.0, viewport.height - status_h, viewport.width, status_h);
        backend.draw_status_bar_interactive(status_rect, &self.status(), &InteractionState::new());

        let input_rect = Self::input_rect(backend);
        backend.draw_text_input(input_rect, &self.input);
    }

    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
        match event {
            // Accelerators fire ahead of raw key events (quadraui's
            // backends match registered accelerators first) — this is
            // #833's undo/redo wiring: the long-declared
            // `KeyBinding::Undo`/`Redo` names now reach
            // `TextInput::apply` via `EditOp::from_key_binding`.
            UiEvent::Accelerator(id, _modifiers) => {
                let op = if id.as_str() == UNDO_ACCEL {
                    EditOp::from_key_binding(&KeyBinding::Undo)
                } else if id.as_str() == REDO_ACCEL {
                    EditOp::from_key_binding(&KeyBinding::Redo)
                } else {
                    None
                };
                match op.map(|op| self.input.apply(op)) {
                    Some(true) => Reaction::Redraw,
                    Some(false) | None => Reaction::Continue,
                }
            }
            // Click routing (#818): move the cursor to the clicked line
            // via `TextInputLayout::hit_test` — the same layout `render`
            // just painted from.
            UiEvent::MouseDown {
                button: MouseButton::Left,
                position,
                ..
            } => {
                let rect = Self::input_rect(backend);
                let layout = backend.text_input_layout(rect, &self.input);
                let op = match layout.hit_test(position.x, position.y) {
                    TextInputHit::Line { line_idx } => EditOp::SetCursor {
                        line: line_idx,
                        col: self.input.cursor_col,
                        extend: false,
                    },
                    TextInputHit::EmptyArea => {
                        let last = self.input.lines.len().saturating_sub(1);
                        EditOp::SetCursor {
                            line: last,
                            col: usize::MAX,
                            extend: false,
                        }
                    }
                };
                if self.input.apply(op) {
                    Reaction::Redraw
                } else {
                    Reaction::Continue
                }
            }
            UiEvent::KeyPressed {
                key,
                modifiers,
                repeat: _,
            } => {
                if matches!(key, Key::Named(NamedKey::Escape)) {
                    return Reaction::Exit;
                }
                // Plain typing and cursor movement (with Shift-to-select)
                // go through `EditOp::from_key`, which already maps
                // Ctrl+Home/End to MoveDocStart/MoveDocEnd. Only
                // Ctrl/Cmd+<character> combos (copy/paste/etc. — none
                // needed here, since Undo/Redo are handled above as
                // accelerators) are rejected before `from_key`, so they
                // don't get typed as literal characters; named keys like
                // Home/End still reach `from_key` with their modifiers
                // intact.
                if matches!(key, Key::Char(_)) && (modifiers.ctrl || modifiers.cmd) {
                    return Reaction::Continue;
                }
                match EditOp::from_key(&key, modifiers) {
                    Some(op) => {
                        if self.input.apply(op) {
                            Reaction::Redraw
                        } else {
                            Reaction::Continue
                        }
                    }
                    None => Reaction::Continue,
                }
            }
            UiEvent::WindowResized { .. } => Reaction::Redraw,
            _ => Reaction::Continue,
        }
    }
}
