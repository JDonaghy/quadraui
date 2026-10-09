//! `Float` demo — adapted from `quadraui/examples/common/float_app.rs`.
//!
//! Two floats open at once: an interactive actions menu anchored to the
//! selected command row (focusable, steals arrow/Enter keys while open)
//! and a which-key-style hint popup (non-focusable, never steals keys).
//! Both are pushed onto `Backend::modal_stack_handle()` — the host never
//! hand-rolls an "is a float open" bool chain; it asks
//! `top_focusable()`.
//!
//! Two variants show the menu anchored on either side of the selected
//! row (`Side::Bottom` vs `Side::Top`).
//!
//! The pushed floats are only popped on explicit close (Escape / `m` /
//! clicking elsewhere); navigating away from this demo while one is open
//! — switching variants, tabs, or sidebar rows — leaves it registered on
//! `Backend::modal_stack_handle()`, which can swallow clicks in that
//! screen region for whatever demo renders next. The gallery's `Demo`
//! trait has no deactivate hook to pop on, so this is a known gap rather
//! than a fixed one.

use quadraui::{
    Anchor, Backend, BackendCaps, Color, Float, FloatLayout, FloatMeasure, InteractionState, Key,
    NamedKey, Reaction, Rect, Side, StatusBar, StatusBarSegment, UiEvent, WidgetId,
};

use crate::demo::{extract_region, Demo};

const SOURCE: &str = include_str!("float.rs");

// gallery:begin
const HINT_ID: &str = "gallery:float:hint";
const MENU_ID: &str = "gallery:float:menu";

const COMMANDS: &[&str] = &["Open File", "Save", "Rename", "Duplicate", "Close"];
const MENU_ITEMS: &[&str] = &["Run now", "Run later", "Cancel"];

pub struct FloatDemo {
    selected: usize,
    hint_open: bool,
    menu_open: bool,
    menu_selected: usize,
    last_action: String,
}

impl FloatDemo {
    pub fn new() -> Self {
        Self {
            selected: 0,
            hint_open: false,
            menu_open: false,
            menu_selected: 0,
            last_action: String::new(),
        }
    }

    fn row_rect(&self, area: Rect, backend: &dyn Backend, idx: usize) -> Rect {
        let lh = backend.line_height();
        Rect::new(area.x, area.y + idx as f32 * lh, area.width, lh)
    }

    fn menu_anchor(&self, variant: usize, area: Rect, backend: &dyn Backend) -> Anchor {
        let side = if variant == 0 {
            Side::Bottom
        } else {
            Side::Top
        };
        Anchor::new(self.row_rect(area, backend, self.selected), side)
    }

    fn menu_float(&self, variant: usize, area: Rect, backend: &dyn Backend) -> Float {
        Float::new(
            WidgetId::new(MENU_ID),
            self.menu_anchor(variant, area, backend),
        )
    }

    fn menu_layout(&self, variant: usize, area: Rect, backend: &dyn Backend) -> FloatLayout {
        let cw = backend.char_width();
        let lh = backend.line_height();
        let float = self.menu_float(variant, area, backend);
        let measure = FloatMeasure::new(cw * 16.0, lh * (MENU_ITEMS.len() as f32 + 2.0));
        float.layout(area, measure)
    }

    fn hint_anchor(&self, area: Rect, backend: &dyn Backend) -> Anchor {
        let lh = backend.line_height();
        let cw = backend.char_width();
        let w = cw * 7.0;
        Anchor::new(
            Rect::new(area.x + area.width - w, area.y + area.height - lh, w, lh),
            Side::Top,
        )
    }

    fn hint_float(&self, area: Rect, backend: &dyn Backend) -> Float {
        let mut f = Float::new(WidgetId::new(HINT_ID), self.hint_anchor(area, backend));
        f.focusable = false;
        f
    }

    fn hint_layout(&self, area: Rect, backend: &dyn Backend) -> FloatLayout {
        let cw = backend.char_width();
        let lh = backend.line_height();
        let float = self.hint_float(area, backend);
        let measure = FloatMeasure::new(cw * 22.0, lh * 5.0);
        float.layout(area, measure)
    }

    fn line_at(backend: &mut dyn Backend, row: Rect, id: &str, text: String, selected: bool) {
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

    fn paint_menu_content(&self, backend: &mut dyn Backend, layout: &FloatLayout) {
        let lh = backend.line_height();
        let cb = layout.content_bounds;
        for (i, item) in MENU_ITEMS.iter().enumerate() {
            let y = cb.y + i as f32 * lh;
            if y + lh > cb.y + cb.height {
                break;
            }
            Self::line_at(
                backend,
                Rect::new(cb.x, y, cb.width, lh),
                &format!("gallery:float:menu-item:{i}"),
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
            Self::line_at(
                backend,
                Rect::new(cb.x, y, cb.width, lh),
                &format!("gallery:float:hint-line:{i}"),
                format!(" {text} "),
                false,
            );
        }
    }

    fn open_menu(&mut self, variant: usize, area: Rect, backend: &mut dyn Backend) {
        self.menu_open = true;
        self.menu_selected = 0;
        let bounds = self.menu_layout(variant, area, backend).bounds;
        let float = self.menu_float(variant, area, backend);
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

    fn toggle_hint(&mut self, area: Rect, backend: &mut dyn Backend) {
        if self.hint_open {
            self.hint_open = false;
            backend
                .modal_stack_handle()
                .borrow_mut()
                .pop(&WidgetId::new(HINT_ID));
        } else {
            self.hint_open = true;
            let bounds = self.hint_layout(area, backend).bounds;
            let float = self.hint_float(area, backend);
            backend
                .modal_stack_handle()
                .borrow_mut()
                .push_float(&float, bounds);
        }
    }
}

impl Default for FloatDemo {
    fn default() -> Self {
        Self::new()
    }
}

impl Demo for FloatDemo {
    fn name(&self) -> &'static str {
        "Float"
    }

    fn group(&self) -> &'static str {
        "Chrome"
    }

    fn variants(&self) -> &'static [&'static str] {
        &["Menu below row", "Menu above row"]
    }

    fn render(&self, variant: usize, backend: &mut dyn Backend, area: Rect) {
        for (i, cmd) in COMMANDS.iter().enumerate() {
            let row = self.row_rect(area, backend, i);
            Self::line_at(
                backend,
                row,
                &format!("gallery:float:cmd:{i}"),
                format!(" {cmd} "),
                i == self.selected,
            );
        }

        if self.menu_open {
            let layout = self.menu_layout(variant, area, backend);
            let float = self.menu_float(variant, area, backend);
            backend.draw_float(&float, &layout);
            self.paint_menu_content(backend, &layout);
        }
        if self.hint_open {
            let layout = self.hint_layout(area, backend);
            let float = self.hint_float(area, backend);
            backend.draw_float(&float, &layout);
            self.paint_hint_content(backend, &layout);
        }
    }

    fn handle(
        &mut self,
        variant: usize,
        event: &UiEvent,
        backend: &mut dyn Backend,
        area: Rect,
    ) -> Reaction {
        match event {
            UiEvent::KeyPressed {
                key: Key::Char('?'),
                ..
            } => {
                self.toggle_hint(area, backend);
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Char('m'),
                ..
            } => {
                if self.menu_open {
                    self.close_menu(backend);
                } else {
                    self.open_menu(variant, area, backend);
                }
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Escape),
                ..
            } if self.menu_open => {
                self.close_menu(backend);
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Escape),
                ..
            } if self.hint_open => {
                self.toggle_hint(area, backend);
                Reaction::Redraw
            }
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
                } else {
                    self.last_action = COMMANDS[self.selected].to_string();
                }
                Reaction::Redraw
            }
            UiEvent::MouseDown {
                widget: Some(ref id),
                position,
                ..
            } if id.as_str() == MENU_ID => {
                let layout = self.menu_layout(variant, area, backend);
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
                self.toggle_hint(area, backend);
                Reaction::Redraw
            }
            UiEvent::MouseDown {
                widget: None,
                position,
                ..
            } => {
                let lh = backend.line_height();
                let idx = ((position.y - area.y) / lh).floor() as usize;
                if idx < COMMANDS.len() {
                    self.selected = idx;
                }
                Reaction::Redraw
            }
            _ => Reaction::Continue,
        }
    }

    fn source(&self) -> &'static str {
        extract_region(SOURCE)
    }

    fn data(&self, _variant: usize) -> serde_json::Value {
        serde_json::json!({
            "selected": COMMANDS[self.selected],
            "menu_open": self.menu_open,
            "hint_open": self.hint_open,
            "last_action": self.last_action,
        })
    }

    fn caps_note(&self, _variant: usize, _caps: &BackendCaps) -> Option<String> {
        None
    }
}
// gallery:end

// `TuiBackend` only exists under `feature = "tui"`, and this crate has
// no default features: the `macos` and `win` gates compile these test
// targets with `tui` off, so the module itself must carry the feature.
#[cfg(all(test, feature = "tui"))]
mod tests {
    use super::*;

    #[test]
    fn opening_the_menu_pushes_it_onto_the_modal_stack() {
        let mut demo = FloatDemo::new();
        let mut backend = quadraui::tui::TuiBackend::new();
        let area = Rect::new(0.0, 0.0, 40.0, 10.0);
        assert!(!demo.menu_open);
        assert_eq!(backend.modal_stack_handle().borrow().len(), 0);

        demo.open_menu(0, area, &mut backend);
        assert!(demo.menu_open);
        assert_eq!(backend.modal_stack_handle().borrow().len(), 1);

        demo.close_menu(&mut backend);
        assert!(!demo.menu_open);
        assert_eq!(backend.modal_stack_handle().borrow().len(), 0);
    }
}
