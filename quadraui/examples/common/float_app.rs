//! Backend-agnostic app code for the `Float` primitive demo
//! ([`tui_float`] / [`gtk_float`]).
//!
//! Two floats, exercising placement, edge clamping, z-order and focus
//! vs. non-focus key routing:
//!
//! - **Placement + edge clamping**: the actions menu anchors to the
//!   currently selected command row and prefers to open below it
//!   (`Side::Bottom`). Navigate to one of the last rows and the float
//!   automatically flips to open above instead — [`Anchor::resolve`]'s
//!   fallback, driven by real state, not a hand-picked example.
//! - **Z-order**: both floats can be open at the same time. Opening the
//!   hint while the menu is already open pushes the hint *on top* of
//!   it in [`crate::ModalStack`] — the menu doesn't move, doesn't
//!   close, and (see the next point) doesn't lose keyboard focus just
//!   because something painted over part of it.
//! - **Focus vs. non-focus**: the hint float
//!   (`focusable: false`) is a which-key-style popup — it must never
//!   steal keys. The actions menu (`focusable: true`) is interactive —
//!   arrow keys and Enter must reach it while it's open. `handle` below
//!   never hand-rolls an "is a float open" bool chain to decide which:
//!   it asks `backend.modal_stack_handle().borrow().top_focusable()`.
//!
//! Controls:
//! - Up / Down: move the command list cursor, or (when the menu is
//!   open) the menu's own cursor
//! - Enter: run the selected command, or (menu open) pick the
//!   selected menu item
//! - m: toggle the actions menu for the selected command
//! - ?: toggle the which-key hint (never steals focus)
//! - Esc: close the menu if open, else the hint if open, else quit
//! - q: quit

use quadraui::{
    Anchor, AppLogic, Backend, Color, Float, FloatLayout, FloatMeasure, InteractionState, Key,
    NamedKey, Reaction, Rect, Side, StatusBar, StatusBarSegment, UiEvent, WidgetId,
};

const HINT_ID: &str = "float:hint";
const MENU_ID: &str = "float:menu";

const COMMANDS: &[&str] = &["Open File", "Save", "Rename", "Duplicate", "Close", "Quit"];
const MENU_ITEMS: &[&str] = &["Run now", "Run later", "Cancel"];

pub struct FloatApp {
    selected: usize,
    hint_open: bool,
    menu_open: bool,
    menu_selected: usize,
    last_action: String,
}

impl FloatApp {
    pub fn new() -> Self {
        Self {
            selected: 0,
            hint_open: false,
            menu_open: false,
            menu_selected: 0,
            last_action: String::new(),
        }
    }

    /// The selected command row's rect, in the active backend's native
    /// units — one `line_height` tall, full width, starting at row 0.
    fn row_rect(&self, backend: &dyn Backend, idx: usize) -> Rect {
        let lh = backend.line_height();
        let width = backend.viewport().width;
        Rect::new(0.0, idx as f32 * lh, width, lh)
    }

    /// Anchors the actions menu to the selected row, preferring to open
    /// below it. Near the bottom of the list this overflows the
    /// viewport and [`Anchor::resolve`] flips it to open above instead
    /// — see this module's doc.
    fn menu_anchor(&self, backend: &dyn Backend) -> Anchor {
        Anchor::new(self.row_rect(backend, self.selected), Side::Bottom)
    }

    fn menu_float(&self, backend: &dyn Backend) -> Float {
        Float::new(WidgetId::new(MENU_ID), self.menu_anchor(backend))
    }

    fn menu_layout(&self, backend: &dyn Backend) -> FloatLayout {
        let cw = backend.char_width();
        let lh = backend.line_height();
        let float = self.menu_float(backend);
        // N content lines + 2 border rows.
        let measure = FloatMeasure::new(cw * 16.0, lh * (MENU_ITEMS.len() as f32 + 2.0));
        float.layout(
            Rect::new(
                0.0,
                0.0,
                backend.viewport().width,
                backend.viewport().height,
            ),
            measure,
        )
    }

    /// Anchors the which-key hint to a fixed help affordance in the
    /// bottom-right corner, opening upward (`Side::Top`) so it never
    /// covers the status bar it's anchored to. The flip/clamp fallback
    /// itself is exercised by [`Self::menu_anchor`] below, whose anchor
    /// rect moves with `self.selected` — see that method's doc.
    fn hint_anchor(&self, backend: &dyn Backend) -> Anchor {
        let viewport = backend.viewport();
        let lh = backend.line_height();
        let cw = backend.char_width();
        let w = cw * 7.0; // "?=help "
        Anchor::new(
            Rect::new(viewport.width - w, viewport.height - lh, w, lh),
            Side::Top,
        )
    }

    fn hint_float(&self, backend: &dyn Backend) -> Float {
        let mut f = Float::new(WidgetId::new(HINT_ID), self.hint_anchor(backend));
        f.focusable = false;
        f
    }

    fn hint_layout(&self, backend: &dyn Backend) -> FloatLayout {
        let cw = backend.char_width();
        let lh = backend.line_height();
        let float = self.hint_float(backend);
        // 3 content lines + 2 border rows.
        let measure = FloatMeasure::new(cw * 22.0, lh * 5.0);
        float.layout(
            Rect::new(
                0.0,
                0.0,
                backend.viewport().width,
                backend.viewport().height,
            ),
            measure,
        )
    }

    fn status_bar(&self) -> StatusBar {
        let last = if self.last_action.is_empty() {
            "nothing yet".to_string()
        } else {
            self.last_action.clone()
        };
        StatusBar {
            id: WidgetId::new("status"),
            left_segments: vec![StatusBarSegment {
                text: format!(" last: {last} "),
                fg: Color::rgb(255, 255, 255),
                bg: Color::rgb(40, 80, 120),
                bold: false,
                action_id: None,
            }],
            right_segments: vec![StatusBarSegment {
                text: " m=menu ?=help q=quit ".into(),
                fg: Color::rgb(220, 220, 220),
                bg: Color::rgb(40, 80, 120),
                bold: false,
                action_id: None,
            }],
        }
    }

    fn line(&self, backend: &mut dyn Backend, y: f32, id: &str, text: String, selected: bool) {
        let width = backend.viewport().width;
        let lh = backend.line_height();
        self.line_at(backend, Rect::new(0.0, y, width, lh), id, text, selected);
    }

    /// Same as [`Self::line`], but at an explicit `row` rect rather than
    /// always spanning the full viewport from `x = 0` — needed to paint
    /// a float's own content into `layout.content_bounds`, which starts
    /// wherever `Float::layout` resolved the box to, not at the origin.
    fn line_at(
        &self,
        backend: &mut dyn Backend,
        row: Rect,
        id: &str,
        text: String,
        selected: bool,
    ) {
        let bg = if selected {
            Color::rgb(60, 60, 90)
        } else {
            Color::rgb(24, 24, 32)
        };
        let bar = StatusBar {
            id: WidgetId::new(id),
            left_segments: vec![StatusBarSegment {
                text,
                fg: Color::rgb(220, 220, 220),
                bg,
                bold: selected,
                action_id: None,
            }],
            right_segments: vec![],
        };
        let _ = backend.draw_status_bar_interactive(row, &bar, &InteractionState::new());
    }

    /// Paint the menu's own items into `layout.content_bounds` — `Float`
    /// never paints content itself (see its module doc); this is the
    /// host's job, same contract as `Panel::content_bounds`.
    fn paint_menu_content(&self, backend: &mut dyn Backend, layout: &FloatLayout) {
        let lh = backend.line_height();
        let cb = layout.content_bounds;
        for (i, item) in MENU_ITEMS.iter().enumerate() {
            let y = cb.y + i as f32 * lh;
            if y + lh > cb.y + cb.height {
                break;
            }
            self.line_at(
                backend,
                Rect::new(cb.x, y, cb.width, lh),
                &format!("menu-item-{i}"),
                format!(" {item} "),
                i == self.menu_selected,
            );
        }
    }

    fn paint_hint_content(&self, backend: &mut dyn Backend, layout: &FloatLayout) {
        let lh = backend.line_height();
        let cb = layout.content_bounds;
        let lines = ["Up/Down move", "Enter runs", "m = menu"];
        for (i, text) in lines.iter().enumerate() {
            let y = cb.y + i as f32 * lh;
            if y + lh > cb.y + cb.height {
                break;
            }
            self.line_at(
                backend,
                Rect::new(cb.x, y, cb.width, lh),
                &format!("hint-line-{i}"),
                format!(" {text} "),
                false,
            );
        }
    }

    fn open_menu(&mut self, backend: &mut dyn Backend) {
        self.menu_open = true;
        self.menu_selected = 0;
        let bounds = self.menu_layout(backend).bounds;
        let float = self.menu_float(backend);
        backend
            .modal_stack_handle()
            .borrow_mut()
            .push_float(&float, bounds);
    }

    fn close_menu(&mut self, backend: &mut dyn Backend) {
        self.menu_open = false;
        backend
            .modal_stack_handle()
            .borrow_mut()
            .pop(&WidgetId::new(MENU_ID));
    }

    fn toggle_hint(&mut self, backend: &mut dyn Backend) {
        if self.hint_open {
            self.hint_open = false;
            backend
                .modal_stack_handle()
                .borrow_mut()
                .pop(&WidgetId::new(HINT_ID));
        } else {
            self.hint_open = true;
            let bounds = self.hint_layout(backend).bounds;
            let float = self.hint_float(backend);
            // `push_float` forwards `float.focusable` (`false` here), so
            // the hint popup never steals keys — see `Float`'s module
            // doc's "Focus vs. non-focus floats" section.
            backend
                .modal_stack_handle()
                .borrow_mut()
                .push_float(&float, bounds);
        }
    }
}

impl Default for FloatApp {
    fn default() -> Self {
        Self::new()
    }
}

impl AppLogic for FloatApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let lh = backend.line_height();
        let viewport = backend.viewport();

        for (i, cmd) in COMMANDS.iter().enumerate() {
            self.line(
                backend,
                i as f32 * lh,
                &format!("cmd-{i}"),
                format!(" {cmd} "),
                i == self.selected,
            );
        }

        let status_rect = Rect::new(0.0, viewport.height - lh, viewport.width, lh);
        let _ = backend.draw_status_bar_interactive(
            status_rect,
            &self.status_bar(),
            &InteractionState::new(),
        );

        // Floats paint last (highest z) — `ModalStack` has no opinion on
        // draw order, only on hit-test precedence (see its module doc).
        // The menu paints before the hint so that, when both are open,
        // the hint visually sits on top — exercising z-order without
        // the hint ever being able to steal the menu's keyboard focus
        // (see `handle` below).
        if self.menu_open {
            let layout = self.menu_layout(backend);
            let float = self.menu_float(backend);
            backend.draw_float(&float, &layout);
            self.paint_menu_content(backend, &layout);
        }
        if self.hint_open {
            let layout = self.hint_layout(backend);
            let float = self.hint_float(backend);
            backend.draw_float(&float, &layout);
            self.paint_hint_content(backend, &layout);
        }
    }

    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
        match event {
            UiEvent::KeyPressed {
                key: Key::Char('q'),
                ..
            } => Reaction::Exit,
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Escape),
                ..
            } => {
                if self.menu_open {
                    self.close_menu(backend);
                    Reaction::Redraw
                } else if self.hint_open {
                    self.toggle_hint(backend);
                    Reaction::Redraw
                } else {
                    Reaction::Exit
                }
            }
            UiEvent::KeyPressed {
                key: Key::Char('?'),
                ..
            } => {
                self.toggle_hint(backend);
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Char('m'),
                ..
            } => {
                if self.menu_open {
                    self.close_menu(backend);
                } else {
                    self.open_menu(backend);
                }
                Reaction::Redraw
            }

            // Route by asking who currently owns the keyboard —
            // never a hand-rolled "is a float open" bool chain. A
            // non-focusable hint on top of the stack is skipped
            // automatically; `top_focusable()` only ever names the menu
            // (when open) or nothing (base layer).
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Up),
                ..
            } => {
                let focused = backend
                    .modal_stack_handle()
                    .borrow()
                    .top_focusable()
                    .map(|e| e.id.clone());
                if focused == Some(WidgetId::new(MENU_ID)) {
                    self.menu_selected = self.menu_selected.saturating_sub(1);
                } else {
                    self.selected = self.selected.saturating_sub(1);
                }
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Down),
                ..
            } => {
                let focused = backend
                    .modal_stack_handle()
                    .borrow()
                    .top_focusable()
                    .map(|e| e.id.clone());
                if focused == Some(WidgetId::new(MENU_ID)) {
                    self.menu_selected = (self.menu_selected + 1).min(MENU_ITEMS.len() - 1);
                } else {
                    self.selected = (self.selected + 1).min(COMMANDS.len() - 1);
                }
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Enter),
                ..
            } => {
                let focused = backend
                    .modal_stack_handle()
                    .borrow()
                    .top_focusable()
                    .map(|e| e.id.clone());
                if focused == Some(WidgetId::new(MENU_ID)) {
                    self.last_action = format!(
                        "{} / {}",
                        COMMANDS[self.selected], MENU_ITEMS[self.menu_selected]
                    );
                    self.close_menu(backend);
                } else if COMMANDS[self.selected] == "Quit" {
                    return Reaction::Exit;
                } else {
                    self.last_action = COMMANDS[self.selected].to_string();
                }
                Reaction::Redraw
            }

            // ── Clicks inside an open float — pre-tagged by `ModalStack`
            // (see `examples/common/modal_occlusion_demo.rs` for the
            // established no-click-guard pattern) ─────────────────────
            UiEvent::MouseDown {
                widget: Some(ref id),
                position,
                ..
            } if id.as_str() == MENU_ID => {
                let layout = self.menu_layout(backend);
                let lh = backend.line_height();
                let rel_y = position.y - layout.content_bounds.y;
                if rel_y >= 0.0 {
                    let idx = (rel_y / lh).floor() as usize;
                    if idx < MENU_ITEMS.len() {
                        self.last_action =
                            format!("{} / {}", COMMANDS[self.selected], MENU_ITEMS[idx]);
                        self.close_menu(backend);
                    }
                }
                Reaction::Redraw
            }
            UiEvent::MouseDown {
                widget: Some(ref id),
                ..
            } if id.as_str() == HINT_ID => {
                // The hint swallows the click (it's registered on the
                // modal stack so it can) but has nothing to activate —
                // click it away.
                self.toggle_hint(backend);
                Reaction::Redraw
            }

            UiEvent::MouseDown {
                widget: None,
                position,
                ..
            } => {
                let lh = backend.line_height();
                let idx = (position.y / lh).floor() as usize;
                if idx < COMMANDS.len() {
                    self.selected = idx;
                }
                Reaction::Redraw
            }

            UiEvent::WindowResized { .. } => {
                // Both anchors are viewport-derived, so a resize moves
                // the resolved bounds; re-push onto `ModalStack` so its
                // hit-test rects stay in sync with what `render` repaints
                // next frame.
                if self.menu_open {
                    let bounds = self.menu_layout(backend).bounds;
                    let float = self.menu_float(backend);
                    backend
                        .modal_stack_handle()
                        .borrow_mut()
                        .push_float(&float, bounds);
                }
                if self.hint_open {
                    let bounds = self.hint_layout(backend).bounds;
                    let float = self.hint_float(backend);
                    backend
                        .modal_stack_handle()
                        .borrow_mut()
                        .push_float(&float, bounds);
                }
                Reaction::Redraw
            }
            _ => Reaction::Continue,
        }
    }
}
