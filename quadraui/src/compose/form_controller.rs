//! `FormController` — a composed controller for a single `Form` with
//! built-in scrollbar support.
//!
//! Mirrors [`TreeController`](super::tree_controller::TreeController):
//! owns scroll state, renders the form + scrollbar, and handles scroll
//! wheel / scrollbar click / thumb-drag events. Apps push field data
//! per frame via [`FormController::set_form`], call
//! [`FormController::render`] + [`FormController::handle`], and match
//! on [`FormControllerEvent`] for semantic actions.
//!
//! Two event-handling paths:
//!
//! - [`FormController::handle`] — requires `&mut dyn Backend`. Use when
//!   the event handler has a backend reference (e.g. `AppLogic::handle`).
//! - [`FormController::handle_cached`] — backend-free. Uses metrics
//!   cached by [`FormController::render`] or [`FormController::set_backend_info`].
//!   Use when the event handler runs without a backend (e.g. vimcode's
//!   TUI mouse handler).

use crate::primitives::form::{
    FieldKind, Form, FormEvent, FormFieldMeasure, FormHit, FormItemMeasure,
};
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
    cached_lh: Option<f32>,
}

impl FormController {
    pub fn new(id: String) -> Self {
        Self {
            id,
            form: None,
            scroll_offset: 0,
            has_focus: false,
            scroll_drag: None,
            cached_lh: None,
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

    /// Cache backend metrics so [`Self::handle_cached`] can process
    /// events without a `Backend` reference. Call once at init, or
    /// again if `line_height` changes (font/DPI change).
    ///
    /// [`Self::render`] caches this automatically, so explicit calls
    /// are only needed when `handle_cached` must work before the first
    /// `render`.
    pub fn set_backend_info(&mut self, line_height: f32) {
        self.cached_lh = Some(line_height);
    }

    // ── Render ────────────────────────────────────────────────────────

    pub fn render(&self, backend: &mut dyn Backend, rect: Rect) {
        let lh = backend.line_height();
        // Cache metrics for handle_cached(). The &self receiver prevents
        // mutation here, so we defer to a post-render cache call pattern:
        // callers that need handle_cached() should call set_backend_info()
        // or render_and_cache().
        let (form_rect, sb_rect) = split_rect_lh(self.field_count(), lh, rect);
        let form = self.build_form(form_rect);
        backend.draw_form(form_rect, &form);
        if let Some(sb_rect) = sb_rect {
            let sb = build_scrollbar_lh(
                &self.id,
                self.field_count(),
                self.scroll_offset,
                self.scroll_drag.is_some(),
                lh,
                sb_rect,
            );
            backend.draw_scrollbar(sb_rect, &sb);
        }
    }

    /// Render and cache backend metrics for [`Self::handle_cached`].
    ///
    /// Equivalent to calling [`Self::set_backend_info`] then
    /// [`Self::render`], but in one step.
    pub fn render_and_cache(&mut self, backend: &mut dyn Backend, rect: Rect) {
        self.cached_lh = Some(backend.line_height());
        self.render(backend, rect);
    }

    // ── Handle (with backend) ────────────────────────────────────────

    pub fn handle(
        &mut self,
        event: &UiEvent,
        backend: &mut dyn Backend,
        rect: Rect,
    ) -> FormControllerEvent {
        let lh = backend.line_height();
        self.handle_inner(event, rect, lh, Some(backend))
    }

    // ── Handle (cached, no backend) ──────────────────────────────────

    /// Backend-free event handler. Requires [`Self::set_backend_info`]
    /// or [`Self::render_and_cache`] called first. Returns
    /// [`FormControllerEvent::Ignored`] if metrics aren't cached yet.
    pub fn handle_cached(&mut self, event: &UiEvent, rect: Rect) -> FormControllerEvent {
        let Some(lh) = self.cached_lh else {
            return FormControllerEvent::Ignored;
        };
        self.handle_inner(event, rect, lh, None)
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
        viewport_rows_lh(backend.line_height(), rect)
    }

    // ── Internal: shared handle dispatch ─────────────────────────────

    fn handle_inner(
        &mut self,
        event: &UiEvent,
        rect: Rect,
        lh: f32,
        backend: Option<&mut dyn Backend>,
    ) -> FormControllerEvent {
        match event {
            UiEvent::MouseDown {
                button: MouseButton::Left,
                position,
                ..
            } => self.click_inner(rect, position.x, position.y, lh, backend),

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
                let vr = viewport_rows_lh(lh, rect);
                let rows = if delta.y > 0.0 { -1 } else { 1 };
                self.scroll_by(rows, vr);
                FormControllerEvent::Consumed
            }

            _ => FormControllerEvent::Ignored,
        }
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

    fn click_inner(
        &mut self,
        rect: Rect,
        x: f32,
        y: f32,
        lh: f32,
        backend: Option<&mut dyn Backend>,
    ) -> FormControllerEvent {
        if !rect_contains(rect, x, y) {
            return FormControllerEvent::Ignored;
        }
        let (form_rect, sb_rect) = split_rect_lh(self.field_count(), lh, rect);

        if let Some(sb_rect) = sb_rect {
            if rect_contains(sb_rect, x, y) {
                return self.click_scrollbar_lh(lh, form_rect, sb_rect, y);
            }
        }

        if rect_contains(form_rect, x, y) {
            let form = self.build_form(form_rect);
            let layout = if let Some(be) = backend {
                be.form_layout(form_rect, &form)
            } else {
                let row_h = row_height(lh);
                let char_w = lh * 0.6;
                form.layout(form_rect.width, form_rect.height, |i| {
                    form_field_measure(&form.fields[i], row_h, char_w)
                })
            };
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

    fn click_scrollbar_lh(
        &mut self,
        lh: f32,
        form_rect: Rect,
        sb_rect: Rect,
        y: f32,
    ) -> FormControllerEvent {
        let vr = viewport_rows_lh(lh, form_rect);
        let max_offset = self.field_count().saturating_sub(vr);
        if max_offset == 0 {
            return FormControllerEvent::Ignored;
        }

        let sb = build_scrollbar_lh(
            &self.id,
            self.field_count(),
            self.scroll_offset,
            self.scroll_drag.is_some(),
            lh,
            sb_rect,
        );
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
            self.page_scroll(-(vr as isize), vr);
            FormControllerEvent::ScrollChanged
        } else {
            self.page_scroll(vr as isize, vr);
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

// ── Free functions (shared by FormController + SidebarSystem) ────────

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

// ── Pure helpers (line_height → derived values) ─────────────────────

fn row_height(lh: f32) -> f32 {
    (lh * 1.4).round()
}

fn track_width(lh: f32) -> f32 {
    (lh * 0.4).max(1.0).round()
}

fn viewport_rows_lh(lh: f32, rect: Rect) -> usize {
    if lh <= 0.0 {
        return 0;
    }
    let rh = row_height(lh);
    if rh <= 0.0 {
        return 0;
    }
    (rect.height / rh).floor() as usize
}

fn split_rect_lh(field_count: usize, lh: f32, rect: Rect) -> (Rect, Option<Rect>) {
    let tw = track_width(lh);
    if rect.width <= tw {
        return (rect, None);
    }
    let form_rect = Rect::new(rect.x, rect.y, rect.width - tw, rect.height);
    let vr = viewport_rows_lh(lh, form_rect);
    if field_count <= vr {
        return (rect, None);
    }
    let sb_rect = Rect::new(rect.x + rect.width - tw, rect.y, tw, rect.height);
    (form_rect, Some(sb_rect))
}

fn build_scrollbar_lh(
    id: &str,
    field_count: usize,
    scroll_offset: usize,
    is_dragging: bool,
    lh: f32,
    sb_rect: Rect,
) -> Scrollbar {
    let total = field_count as f32;
    let rh = row_height(lh);
    let visible = if rh > 0.0 {
        (sb_rect.height / rh).floor()
    } else {
        total
    };
    let min_thumb = lh.max(1.0);
    let mut sb = Scrollbar::vertical(
        format!("{id}-scrollbar"),
        sb_rect,
        scroll_offset as f32,
        total,
        visible,
        min_thumb,
    );
    sb.dragging = is_dragging;
    sb
}

fn form_field_measure(
    field: &crate::primitives::form::FormField,
    row_h: f32,
    char_w: f32,
) -> FormFieldMeasure {
    match &field.kind {
        FieldKind::ToggleGroup { toggles } => {
            let label_w = field.label.visible_width() as f32 * char_w;
            let start_x = if label_w > 0.0 {
                label_w + char_w * 2.0
            } else {
                char_w
            };
            let items = toggles
                .iter()
                .map(|t| FormItemMeasure {
                    id: t.id.clone(),
                    width: (t.label.chars().count() as f32 + 2.0) * char_w,
                })
                .collect();
            FormFieldMeasure::with_items(row_h, start_x, char_w, items)
        }
        FieldKind::ButtonRow { buttons } => {
            let label_w = field.label.visible_width() as f32 * char_w;
            let start_x = if label_w > 0.0 {
                label_w + char_w * 2.0
            } else {
                char_w
            };
            let items = buttons
                .iter()
                .map(|b| FormItemMeasure {
                    id: b.id.clone(),
                    width: (b.label.chars().count() as f32 + 2.0) * char_w,
                })
                .collect();
            FormFieldMeasure::with_items(row_h, start_x, char_w, items)
        }
        FieldKind::SegmentedControl { options, .. } => {
            let label_w = field.label.visible_width() as f32 * char_w;
            let start_x = if label_w > 0.0 {
                label_w + char_w * 2.0
            } else {
                char_w
            };
            let items = options
                .iter()
                .enumerate()
                .map(|(idx, opt)| FormItemMeasure {
                    id: WidgetId::new(format!("{}__seg_{idx}", field.id.as_str())),
                    width: (opt.chars().count() as f32 + 2.0) * char_w,
                })
                .collect();
            FormFieldMeasure::with_items(row_h, start_x, 0.0, items)
        }
        FieldKind::TextArea { visible_rows, .. } => {
            FormFieldMeasure::new(row_h * *visible_rows as f32)
        }
        _ => FormFieldMeasure::new(row_h),
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

    // ── handle_cached ───────────────────────────────────────────────

    #[test]
    fn handle_cached_returns_ignored_without_backend_info() {
        let mut fc = test_controller(20);
        let ev = fc.handle_cached(
            &UiEvent::Scroll {
                widget: None,
                delta: crate::ScrollDelta { x: 0.0, y: 1.0 },
                position: crate::Point { x: 5.0, y: 5.0 },
            },
            Rect::new(0.0, 0.0, 40.0, 10.0),
        );
        assert_eq!(ev, FormControllerEvent::Ignored);
    }

    #[test]
    fn handle_cached_scroll_works_after_set_backend_info() {
        let mut fc = test_controller(20);
        // TUI: lh=1.0, row_h=1.0, rect height=5 → 5 viewport rows
        fc.set_backend_info(1.0);
        let ev = fc.handle_cached(
            &UiEvent::Scroll {
                widget: None,
                delta: crate::ScrollDelta { x: 0.0, y: -1.0 },
                position: crate::Point { x: 5.0, y: 2.0 },
            },
            Rect::new(0.0, 0.0, 40.0, 5.0),
        );
        assert_eq!(ev, FormControllerEvent::Consumed);
        assert_eq!(fc.scroll_offset(), 1);
    }

    #[test]
    fn handle_cached_click_uses_fallback_layout() {
        let mut fc = test_controller(3);
        fc.set_backend_info(1.0);
        let ev = fc.handle_cached(
            &UiEvent::MouseDown {
                widget: None,
                button: MouseButton::Left,
                position: crate::Point { x: 5.0, y: 0.5 },
                modifiers: Default::default(),
            },
            Rect::new(0.0, 0.0, 40.0, 5.0),
        );
        match ev {
            FormControllerEvent::FormAction(FormEvent::ToggleChanged { id, value }) => {
                assert_eq!(id.as_str(), "field-0");
                assert!(value);
            }
            other => panic!("expected FormAction(ToggleChanged), got {other:?}"),
        }
    }

    // ── Pure helper tests ───────────────────────────────────────────

    #[test]
    fn viewport_rows_lh_tui() {
        // TUI: lh=1.0, row_h=round(1.4)=1.0, height=10 → 10 rows
        assert_eq!(viewport_rows_lh(1.0, Rect::new(0.0, 0.0, 40.0, 10.0)), 10);
    }

    #[test]
    fn viewport_rows_lh_gtk() {
        // GTK: lh=20.0, row_h=round(28.0)=28.0, height=280 → 10 rows
        assert_eq!(
            viewport_rows_lh(20.0, Rect::new(0.0, 0.0, 400.0, 280.0)),
            10
        );
    }

    #[test]
    fn split_rect_lh_no_scrollbar_when_fits() {
        let (form_rect, sb) = split_rect_lh(5, 1.0, Rect::new(0.0, 0.0, 40.0, 10.0));
        assert!(sb.is_none());
        assert_eq!(form_rect.width, 40.0);
    }

    #[test]
    fn split_rect_lh_scrollbar_when_overflows() {
        let (form_rect, sb) = split_rect_lh(20, 1.0, Rect::new(0.0, 0.0, 40.0, 10.0));
        assert!(sb.is_some());
        assert!(form_rect.width < 40.0);
    }
}
