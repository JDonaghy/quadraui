//! `FormController` — a composed controller for a single `Form` with
//! built-in scrollbar support.
//!
//! Mirrors [`TreeController`](super::tree_controller::TreeController):
//! owns scroll state, renders the form + scrollbar, and handles scroll
//! wheel / scrollbar click / thumb-drag events. Apps push field data
//! per frame via [`FormController::set_form`], call
//! [`FormController::render`] + [`FormController::handle`], and match
//! on [`FormControllerEvent`] for semantic actions.

use crate::primitives::form::{FieldKind, Form, FormEvent, FormHit};
use crate::{Backend, ButtonMask, MouseButton, Rect, Scrollbar, UiEvent, WidgetId};

/// What happened after [`FormController::handle`] processed an event.
#[derive(Debug, Clone, PartialEq)]
pub enum FormControllerEvent {
    /// A field-level event occurred (toggle changed, text input, etc.).
    FormAction(FormEvent),
    /// Scrollbar interaction changed the scroll offset.
    ScrollChanged,
    /// Event consumed (drag update, hover) — caller should redraw.
    Consumed,
    /// Event not relevant to the form.
    Ignored,
}

struct ScrollDrag {
    origin_y: f32,
    origin_offset: usize,
    travel: f32,
    max_offset: usize,
}

pub struct FormController {
    id: String,
    form: Option<Form>,
    scroll_offset: usize,
    has_focus: bool,
    scroll_drag: Option<ScrollDrag>,
}

impl FormController {
    pub fn new(id: String) -> Self {
        Self {
            id,
            form: None,
            scroll_offset: 0,
            has_focus: false,
            scroll_drag: None,
        }
    }

    // ── Per-frame data ────────────────────────────────────────────────

    pub fn set_form(&mut self, form: Form) {
        self.form = Some(form);
    }

    // ── State accessors ───────────────────────────────────────────────

    pub fn form(&self) -> Option<&Form> {
        self.form.as_ref()
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn default_form_id(&self) -> WidgetId {
        WidgetId::new(format!("{}-form", self.id))
    }

    pub fn scroll_offset(&self) -> usize {
        self.scroll_offset
    }

    pub fn has_focus(&self) -> bool {
        self.has_focus
    }

    pub fn field_count(&self) -> usize {
        self.form.as_ref().map_or(0, |f| f.fields.len())
    }

    // ── Programmatic state control ────────────────────────────────────

    pub fn set_scroll_offset(&mut self, offset: usize) {
        self.scroll_offset = offset;
    }

    pub fn set_has_focus(&mut self, has_focus: bool) {
        self.has_focus = has_focus;
    }

    // ── Render ────────────────────────────────────────────────────────

    pub fn render(&self, backend: &mut dyn Backend, rect: Rect) {
        let (form_rect, sb_rect) = self.split_rect(backend, rect);
        let form = self.build_form(form_rect);
        backend.draw_form(form_rect, &form);
        if let Some(sb_rect) = sb_rect {
            let sb = self.build_scrollbar(backend, sb_rect);
            backend.draw_scrollbar(sb_rect, &sb);
        }
    }

    // ── Handle ────────────────────────────────────────────────────────

    pub fn handle(
        &mut self,
        event: &UiEvent,
        backend: &mut dyn Backend,
        rect: Rect,
    ) -> FormControllerEvent {
        match event {
            UiEvent::MouseDown {
                button: MouseButton::Left,
                position,
                ..
            } => self.click(backend, rect, position.x, position.y),

            UiEvent::MouseMoved {
                position,
                buttons:
                    ButtonMask {
                        left: true,
                        middle: _,
                        right: _,
                    },
            } => self.drag_to(position.y),

            UiEvent::MouseUp {
                button: MouseButton::Left,
                ..
            } => {
                self.scroll_drag = None;
                FormControllerEvent::Ignored
            }

            UiEvent::Scroll { delta, .. } => {
                let vr = self.viewport_rows(backend, rect);
                let rows = if delta.y > 0.0 { -1 } else { 1 };
                self.scroll_by(rows, vr);
                FormControllerEvent::Consumed
            }

            _ => FormControllerEvent::Ignored,
        }
    }

    // ── Scroll primitives (pub for SidebarSystem reuse) ──────────────

    pub fn scroll_by(&mut self, delta: isize, viewport_rows: usize) {
        let total = self.field_count();
        let max = total.saturating_sub(viewport_rows) as isize;
        let cur = self.scroll_offset as isize;
        let new = (cur + delta).max(0).min(max) as usize;
        self.scroll_offset = new;
    }

    pub fn page_scroll(&mut self, delta: isize, viewport_rows: usize) {
        self.scroll_by(delta, viewport_rows);
    }

    pub fn scroll_to_field(&mut self, field_idx: usize, viewport_rows: usize) {
        if viewport_rows == 0 {
            return;
        }
        if field_idx < self.scroll_offset {
            self.scroll_offset = field_idx;
        } else if field_idx >= self.scroll_offset + viewport_rows {
            self.scroll_offset = field_idx.saturating_sub(viewport_rows.saturating_sub(1));
        }
    }

    pub fn viewport_rows(&self, backend: &dyn Backend, rect: Rect) -> usize {
        let lh = backend.line_height();
        if lh <= 0.0 {
            return self.field_count();
        }
        let row_h = (lh * 1.4).round();
        if row_h <= 0.0 {
            return self.field_count();
        }
        (rect.height / row_h).floor() as usize
    }

    // ── Internal helpers ──────────────────────────────────────────────

    fn build_form(&self, _rect: Rect) -> Form {
        match &self.form {
            Some(f) => {
                let mut form = f.clone();
                form.scroll_offset = self.scroll_offset;
                form.has_focus = self.has_focus;
                form
            }
            None => Form {
                id: self.default_form_id(),
                fields: Vec::new(),
                focused_field: None,
                scroll_offset: self.scroll_offset,
                has_focus: self.has_focus,
            },
        }
    }

    fn scrollbar_track_width(&self, backend: &dyn Backend) -> f32 {
        // 1 cell on TUI (lh=1.0), ~8px on GTK (lh≈20) — matches MSV's
        // scrollbar_size convention.
        (backend.line_height() * 0.4).max(1.0).round()
    }

    fn needs_scrollbar(&self, backend: &dyn Backend, form_rect: Rect) -> bool {
        self.field_count() > self.viewport_rows(backend, form_rect)
    }

    fn split_rect(&self, backend: &dyn Backend, rect: Rect) -> (Rect, Option<Rect>) {
        let track_w = self.scrollbar_track_width(backend);
        if rect.width <= track_w {
            return (rect, None);
        }
        let form_rect = Rect::new(rect.x, rect.y, rect.width - track_w, rect.height);
        if !self.needs_scrollbar(backend, form_rect) {
            return (rect, None);
        }
        let sb_rect = Rect::new(rect.x + rect.width - track_w, rect.y, track_w, rect.height);
        (form_rect, Some(sb_rect))
    }

    fn build_scrollbar(&self, backend: &dyn Backend, sb_rect: Rect) -> Scrollbar {
        let total = self.field_count() as f32;
        let visible = {
            let lh = backend.line_height();
            if lh > 0.0 {
                let row_h = (lh * 1.4).round();
                if row_h > 0.0 {
                    (sb_rect.height / row_h).floor()
                } else {
                    total
                }
            } else {
                total
            }
        };
        let min_thumb = backend.line_height().max(1.0);
        let is_dragging = self.scroll_drag.is_some();
        let mut sb = Scrollbar::vertical(
            format!("{}-scrollbar", self.id),
            sb_rect,
            self.scroll_offset as f32,
            total,
            visible,
            min_thumb,
        );
        sb.dragging = is_dragging;
        sb
    }

    fn click(
        &mut self,
        backend: &mut dyn Backend,
        rect: Rect,
        x: f32,
        y: f32,
    ) -> FormControllerEvent {
        if !rect_contains(rect, x, y) {
            return FormControllerEvent::Ignored;
        }
        let (form_rect, sb_rect) = self.split_rect(backend, rect);

        if let Some(sb_rect) = sb_rect {
            if rect_contains(sb_rect, x, y) {
                return self.click_scrollbar(backend, form_rect, sb_rect, y);
            }
        }

        if rect_contains(form_rect, x, y) {
            let form = self.build_form(form_rect);
            let layout = backend.form_layout(form_rect, &form);
            match layout.hit_test(x - form_rect.x, y - form_rect.y) {
                FormHit::Field(id) => {
                    let event = form_click_event(&form, &id);
                    FormControllerEvent::FormAction(event)
                }
                FormHit::Empty => FormControllerEvent::Consumed,
            }
        } else {
            FormControllerEvent::Ignored
        }
    }

    fn click_scrollbar(
        &mut self,
        backend: &dyn Backend,
        form_rect: Rect,
        sb_rect: Rect,
        y: f32,
    ) -> FormControllerEvent {
        let viewport_rows = self.viewport_rows(backend, form_rect);
        let max_offset = self.field_count().saturating_sub(viewport_rows);
        if max_offset == 0 {
            return FormControllerEvent::Ignored;
        }

        let sb = self.build_scrollbar(backend, sb_rect);
        let thumb_top = sb_rect.y + sb.thumb_start;
        let thumb_bottom = thumb_top + sb.thumb_len;

        if y >= thumb_top && y < thumb_bottom {
            let travel = (sb_rect.height - sb.thumb_len).max(0.0);
            self.scroll_drag = Some(ScrollDrag {
                origin_y: y,
                origin_offset: self.scroll_offset,
                travel,
                max_offset,
            });
            FormControllerEvent::ScrollChanged
        } else if y < thumb_top {
            self.page_scroll(-(viewport_rows as isize), viewport_rows);
            FormControllerEvent::ScrollChanged
        } else {
            self.page_scroll(viewport_rows as isize, viewport_rows);
            FormControllerEvent::ScrollChanged
        }
    }

    fn drag_to(&mut self, y: f32) -> FormControllerEvent {
        let Some(drag) = &self.scroll_drag else {
            return FormControllerEvent::Ignored;
        };
        if drag.travel <= 0.0 || drag.max_offset == 0 {
            return FormControllerEvent::Ignored;
        }
        let dy = y - drag.origin_y;
        let drow = dy / drag.travel * drag.max_offset as f32;
        let new = (drag.origin_offset as f32 + drow).round() as i32;
        let new = new.max(0) as usize;
        let new = new.min(drag.max_offset);
        if new == self.scroll_offset {
            return FormControllerEvent::Ignored;
        }
        self.scroll_offset = new;
        FormControllerEvent::Consumed
    }
}

/// Determine the `FormEvent` for a click on a form field.
///
/// Used by `FormController::handle` and `SidebarSystem` click dispatch.
pub(crate) fn form_click_event(form: &Form, clicked_id: &WidgetId) -> FormEvent {
    for field in &form.fields {
        if &field.id == clicked_id {
            return match &field.kind {
                FieldKind::Toggle { value } => FormEvent::ToggleChanged {
                    id: clicked_id.clone(),
                    value: !value,
                },
                FieldKind::Button => FormEvent::ButtonClicked {
                    id: clicked_id.clone(),
                },
                _ => FormEvent::FocusChanged {
                    id: clicked_id.clone(),
                },
            };
        }
        if let FieldKind::ToggleGroup { toggles } = &field.kind {
            if let Some(t) = toggles.iter().find(|t| &t.id == clicked_id) {
                return FormEvent::ToggleChanged {
                    id: clicked_id.clone(),
                    value: !t.value,
                };
            }
        }
        if let FieldKind::ButtonRow { buttons } = &field.kind {
            if buttons.iter().any(|b| &b.id == clicked_id) {
                return FormEvent::ButtonClicked {
                    id: clicked_id.clone(),
                };
            }
        }
        if let FieldKind::SegmentedControl { .. } = &field.kind {
            let prefix = format!("{}__seg_", field.id.as_str());
            if clicked_id.as_str().starts_with(&prefix) {
                if let Ok(idx) = clicked_id.as_str()[prefix.len()..].parse::<usize>() {
                    return FormEvent::SegmentedControlChanged {
                        id: field.id.clone(),
                        selected_idx: idx,
                    };
                }
            }
        }
    }
    FormEvent::FocusChanged {
        id: clicked_id.clone(),
    }
}

fn rect_contains(rect: Rect, x: f32, y: f32) -> bool {
    x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::form::{FieldKind, FormField};
    use crate::types::StyledText;

    fn make_fields(n: usize) -> Vec<FormField> {
        (0..n)
            .map(|i| FormField {
                id: WidgetId::new(format!("field-{i}")),
                label: StyledText::plain(format!("Field {i}")),
                kind: FieldKind::Toggle { value: false },
                hint: StyledText::default(),
                disabled: false,
                validation: None,
            })
            .collect()
    }

    fn make_form(n: usize) -> Form {
        Form {
            id: WidgetId::new("test-form"),
            fields: make_fields(n),
            focused_field: None,
            scroll_offset: 0,
            has_focus: false,
        }
    }

    fn test_controller(field_count: usize) -> FormController {
        let mut fc = FormController::new("test".into());
        fc.set_form(make_form(field_count));
        fc
    }

    // ── Accessors ────────────────────────────────────────────────────

    #[test]
    fn new_starts_empty() {
        let fc = FormController::new("fc".into());
        assert_eq!(fc.scroll_offset(), 0);
        assert!(!fc.has_focus());
        assert!(fc.form().is_none());
        assert_eq!(fc.field_count(), 0);
    }

    #[test]
    fn set_form_and_read_back() {
        let mut fc = FormController::new("fc".into());
        fc.set_form(make_form(5));
        assert!(fc.form().is_some());
        assert_eq!(fc.field_count(), 5);
    }

    #[test]
    fn set_has_focus() {
        let mut fc = FormController::new("fc".into());
        fc.set_has_focus(true);
        assert!(fc.has_focus());
    }

    #[test]
    fn default_form_id() {
        let fc = FormController::new("settings".into());
        assert_eq!(fc.default_form_id().as_str(), "settings-form");
    }

    // ── Scroll-by ───────────────────────────────────────────────────

    #[test]
    fn scroll_by_advances_offset() {
        let mut fc = test_controller(20);
        fc.scroll_by(3, 5);
        assert_eq!(fc.scroll_offset(), 3);
    }

    #[test]
    fn scroll_by_retreats_offset() {
        let mut fc = test_controller(20);
        fc.set_scroll_offset(5);
        fc.scroll_by(-2, 5);
        assert_eq!(fc.scroll_offset(), 3);
    }

    #[test]
    fn scroll_by_clamps_to_bounds() {
        let mut fc = test_controller(10);
        fc.scroll_by(100, 5);
        assert_eq!(fc.scroll_offset(), 5); // 10 - 5
        fc.scroll_by(-100, 5);
        assert_eq!(fc.scroll_offset(), 0);
    }

    // ── Page scroll ─────────────────────────────────────────────────

    #[test]
    fn page_scroll_clamps() {
        let mut fc = test_controller(20);
        fc.page_scroll(100, 5);
        assert_eq!(fc.scroll_offset(), 15); // 20 - 5
        fc.page_scroll(-100, 5);
        assert_eq!(fc.scroll_offset(), 0);
    }

    // ── Scroll-to-field ─────────────────────────────────────────────

    #[test]
    fn scroll_to_field_scrolls_down() {
        let mut fc = test_controller(20);
        fc.scroll_to_field(8, 5);
        assert!(fc.scroll_offset() + 5 > 8);
        assert!(fc.scroll_offset() <= 8);
    }

    #[test]
    fn scroll_to_field_scrolls_up() {
        let mut fc = test_controller(20);
        fc.set_scroll_offset(10);
        fc.scroll_to_field(3, 5);
        assert_eq!(fc.scroll_offset(), 3);
    }

    #[test]
    fn scroll_to_field_noop_when_visible() {
        let mut fc = test_controller(20);
        fc.set_scroll_offset(5);
        fc.scroll_to_field(7, 5);
        assert_eq!(fc.scroll_offset(), 5);
    }

    #[test]
    fn scroll_to_field_noop_with_zero_viewport() {
        let mut fc = test_controller(20);
        fc.scroll_to_field(10, 0);
        assert_eq!(fc.scroll_offset(), 0);
    }

    // ── build_form injects controller state ──────────────────────────

    #[test]
    fn build_form_injects_scroll_offset() {
        let mut fc = test_controller(10);
        fc.set_scroll_offset(3);
        fc.set_has_focus(true);
        let form = fc.build_form(Rect::new(0.0, 0.0, 40.0, 100.0));
        assert_eq!(form.scroll_offset, 3);
        assert!(form.has_focus);
    }

    #[test]
    fn build_form_with_no_data_returns_empty() {
        let fc = FormController::new("empty".into());
        let form = fc.build_form(Rect::new(0.0, 0.0, 40.0, 100.0));
        assert!(form.fields.is_empty());
        assert_eq!(form.id.as_str(), "empty-form");
    }

    // ── form_click_event ────────────────────────────────────────────

    #[test]
    fn click_toggle_flips_value() {
        let form = make_form(3);
        let ev = form_click_event(&form, &WidgetId::new("field-0"));
        assert_eq!(
            ev,
            FormEvent::ToggleChanged {
                id: WidgetId::new("field-0"),
                value: true
            }
        );
    }

    #[test]
    fn click_button_emits_button_clicked() {
        let mut form = make_form(3);
        form.fields[1].kind = FieldKind::Button;
        let ev = form_click_event(&form, &WidgetId::new("field-1"));
        assert_eq!(
            ev,
            FormEvent::ButtonClicked {
                id: WidgetId::new("field-1")
            }
        );
    }

    #[test]
    fn click_text_input_emits_focus_changed() {
        let mut form = make_form(3);
        form.fields[2].kind = FieldKind::TextInput {
            value: "hello".into(),
            placeholder: String::new(),
            cursor: None,
            selection_anchor: None,
        };
        let ev = form_click_event(&form, &WidgetId::new("field-2"));
        assert_eq!(
            ev,
            FormEvent::FocusChanged {
                id: WidgetId::new("field-2")
            }
        );
    }

    #[test]
    fn click_unknown_id_emits_focus_changed() {
        let form = make_form(3);
        let ev = form_click_event(&form, &WidgetId::new("unknown"));
        assert_eq!(
            ev,
            FormEvent::FocusChanged {
                id: WidgetId::new("unknown")
            }
        );
    }

    // ── Empty-form edge cases ────────────────────────────────────────

    #[test]
    fn scroll_by_on_empty_is_noop() {
        let mut fc = FormController::new("fc".into());
        fc.scroll_by(5, 10);
        assert_eq!(fc.scroll_offset(), 0);
    }
}
