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
//!
//! ## Keyboard editing
//!
//! `KeyPressed` / `CharTyped` / `ClipboardPaste` route to the field named
//! by `Form.focused_field`, and only while [`FormController::has_focus`]
//! is true — a host is expected to forward every event in (see the two
//! event-handling paths above), so without this gate a form that isn't
//! even the active panel would still swallow `Tab` and every other key.
//! Toggle it with [`FormController::set_has_focus`], the same shape
//! `compose::sidebar_system` uses.
//!
//! - `Tab` / `Shift+Tab` (or `BackTab`) move focus to the next/previous
//!   non-`Label`, non-`ReadOnly`, non-disabled field, scroll it into
//!   view, and emit [`FormEvent::FocusChanged`]. The app stores the new
//!   id and feeds it back as `focused_field` on the next
//!   [`FormController::set_form`] — same contract `primitives/form.rs`'s
//!   module doc already describes.
//! - While a `TextInput` / `TextArea` / `PasswordInput` field is
//!   focused, every other key is decoded through
//!   [`crate::EditOp::from_key`] and applied via [`crate::TextEditor::apply`]
//!   against that field's `value`, emitting
//!   [`FormEvent::TextInputChanged`] with the new value.
//!   `Ctrl`/`Cmd`+`<character>` combos are rejected before
//!   `EditOp::from_key` sees them — every backend delivers those as
//!   `Key::Char` with the modifier set, so an unregistered accelerator
//!   (Ctrl+C, Cmd+S, ...) doesn't get typed as a literal character.
//!   `Enter` commits instead of inserting a newline, emitting
//!   [`FormEvent::TextInputCommitted`] — except on a `TextArea`, where
//!   `Enter` inserts a newline, since that is the one field kind meant
//!   to hold them. `ClipboardPaste` inserts its text via
//!   [`crate::EditOp::InsertText`].
//! - The cursor/selection position is **owned by `FormController`**, not
//!   the app — it survives across keystrokes internally (see
//!   `text_edit` below) even though the app only ever has to persist the
//!   `value` string from `TextInputChanged`/`TextInputCommitted`. This is
//!   what makes "no app-side key plumbing" true: the app's model needs a
//!   `String` per text field, nothing else. Each keystroke builds a
//!   throwaway [`crate::TextEditor`] (see `apply_text_edit_op`), so
//!   `EditOp::Undo`/`Redo` never have history to act on for a form
//!   field — out of scope here, since nothing in `Form`'s per-frame data
//!   carries edit history across keystrokes.

use crate::primitives::form::{
    FieldKind, Form, FormEvent, FormFieldMeasure, FormHit, FormItemMeasure,
};
use crate::primitives::scrollbar::{scroll_by_clamped, PressZone, ThumbDrag};
use crate::primitives::text_input::{EditOp, TextEditor, TextInput};
use crate::text_util::snap_to_char_boundary;
use crate::{
    Backend, ButtonMask, Key, Modifiers, MouseButton, NamedKey, Point, Rect, Scrollbar, UiEvent,
    WidgetId,
};

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

/// Live cursor/selection for the field currently being typed into.
/// `value` is **not** cached here — it is always read fresh from
/// `FormController::form` (the app's own canonical string, pushed via
/// `set_form`), so the app staying the single source of truth for field
/// *content* still holds; only cursor placement is controller-owned.
/// See this module's "Keyboard editing" doc section for why.
struct TextEditState {
    field_id: WidgetId,
    cursor: usize,
    selection_anchor: Option<usize>,
}

pub struct FormController {
    id: String,
    form: Option<Form>,
    scroll_offset: usize,
    has_focus: bool,
    scroll_drag: Option<ThumbDrag>,
    cached_lh: Option<f32>,
    text_edit: Option<TextEditState>,
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
            text_edit: None,
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
        let max = self.field_count().saturating_sub(viewport_rows);
        self.scroll_offset = scroll_by_clamped(self.scroll_offset, delta, max);
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

            // Gate on `self.has_focus`: `FormController::handle`'s
            // documented usage is "forward every event in" (see this
            // module's top doc), so without this a host that always
            // routes `KeyPressed` through a form would lose global `Tab`
            // (and every other key) to a form that isn't even the
            // active panel. `set_has_focus`/`form_has_focus` exist for
            // exactly this (mirrors `compose/sidebar_system.rs`).
            UiEvent::KeyPressed { key, modifiers, .. } if self.has_focus => {
                let vr = viewport_rows_lh(lh, rect);
                self.handle_key(key, modifiers, vr)
            }

            UiEvent::CharTyped(ch) if self.has_focus => {
                self.handle_text_op(EditOp::InsertChar(*ch))
            }

            UiEvent::ClipboardPaste(text) if self.has_focus => {
                self.handle_text_op(EditOp::InsertText(text.clone()))
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
                if let Some(e) = &self.text_edit {
                    if let Some(field) = form.fields.iter_mut().find(|fld| fld.id == e.field_id) {
                        overlay_text_cursor(&mut field.kind, e.cursor, e.selection_anchor);
                    }
                }
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

    // ── Keyboard: focus traversal + text editing ──────────────────────

    fn handle_key(
        &mut self,
        key: &Key,
        modifiers: &Modifiers,
        viewport_rows: usize,
    ) -> FormControllerEvent {
        match key {
            Key::Named(NamedKey::Tab) if !modifiers.ctrl && !modifiers.alt && !modifiers.cmd => {
                self.move_focus(if modifiers.shift { -1 } else { 1 }, viewport_rows)
            }
            Key::Named(NamedKey::BackTab) => self.move_focus(-1, viewport_rows),
            // `Enter` commits for `TextInput`/`PasswordInput` (a single
            // commit boundary), but `TextArea` is the one field kind
            // that is meant to hold newlines, so `Enter` inserts one
            // there instead — same split VS Code makes between a
            // single-line input and a multi-line text area.
            Key::Named(NamedKey::Enter) if self.focused_field_is_text_area() => {
                self.handle_text_op(EditOp::InsertChar('\n'))
            }
            Key::Named(NamedKey::Enter) => self.commit_focused_text(),
            // Every backend delivers Ctrl/Cmd+<letter> as `Key::Char`
            // with the modifier set (so an unregistered accelerator can
            // still be recovered), not as a dedicated key variant — see
            // `EditOp::from_key`'s callers in `examples/common/text_input_demo.rs`
            // for the same guard. Without this, Ctrl+C/Ctrl+V/Ctrl+W (or
            // Cmd+<letter> on macOS) would type a stray literal character
            // into the focused field instead of falling through to an
            // app-level accelerator.
            Key::Char(_) if modifiers.ctrl || modifiers.cmd => FormControllerEvent::Ignored,
            _ => match EditOp::from_key(key, *modifiers) {
                Some(op) => self.handle_text_op(op),
                None => FormControllerEvent::Ignored,
            },
        }
    }

    /// Whether the currently-focused field is a `TextArea` — gives
    /// `Enter` its kind-specific meaning (insert a newline there, commit
    /// everywhere else).
    fn focused_field_is_text_area(&self) -> bool {
        let Some(form) = &self.form else {
            return false;
        };
        let Some(focused_id) = &form.focused_field else {
            return false;
        };
        form.fields
            .iter()
            .find(|f| &f.id == focused_id)
            .is_some_and(|f| matches!(f.kind, FieldKind::TextArea { .. }))
    }

    /// Move focus to the next/previous non-`Label`, non-`ReadOnly`,
    /// non-disabled field, wrapping around, and scroll it into view.
    /// Emits [`FormEvent::FocusChanged`] — the app persists the new
    /// `focused_field` the same way it already persists a toggle flip
    /// (see this module's "Keyboard editing" doc section).
    fn move_focus(&mut self, delta: isize, viewport_rows: usize) -> FormControllerEvent {
        let Some(form) = &self.form else {
            return FormControllerEvent::Ignored;
        };
        let stops: Vec<&WidgetId> = form
            .fields
            .iter()
            .filter(|f| field_is_focusable(f))
            .map(|f| &f.id)
            .collect();
        if stops.is_empty() {
            return FormControllerEvent::Ignored;
        }
        let n = stops.len() as isize;
        let cur_idx = form
            .focused_field
            .as_ref()
            .and_then(|id| stops.iter().position(|fid| *fid == id));
        let next_idx = match cur_idx {
            Some(i) => (((i as isize + delta) % n + n) % n) as usize,
            None if delta >= 0 => 0,
            None => stops.len() - 1,
        };
        let id = stops[next_idx].clone();
        // The stop's index among `stops` isn't necessarily its index in
        // `form.fields` (non-focusable fields are filtered out above),
        // so re-find its field index for scrolling and for seeding the
        // new field's starting cursor below. Resolved up front (and
        // `form`'s borrow dropped here) because both steps below need
        // `&mut self`.
        let field_idx = form.fields.iter().position(|f| f.id == id);
        // Seed a starting cursor for the newly-focused field if it's
        // text-editable, so `build_form` overlays a caret immediately
        // instead of the field rendering as read-only (no cursor, per
        // `primitives/form.rs`) until the first keystroke.
        let new_text_edit = field_idx
            .and_then(|i| form.fields.get(i))
            .filter(|f| text_field_value(&f.kind).is_some())
            .map(|f| {
                let (cursor, selection_anchor) = default_text_cursor(&f.kind);
                TextEditState {
                    field_id: id.clone(),
                    cursor,
                    selection_anchor,
                }
            });

        if let Some(field_idx) = field_idx {
            self.scroll_to_field(field_idx, viewport_rows);
        }
        self.text_edit = new_text_edit;
        FormControllerEvent::FormAction(FormEvent::FocusChanged { id })
    }

    /// `Enter` on a focused `TextInput`/`PasswordInput` field commits
    /// instead of inserting a newline — see this module's "Keyboard
    /// editing" doc section. (`TextArea` is handled separately in
    /// `handle_key`.)
    fn commit_focused_text(&mut self) -> FormControllerEvent {
        let Some(form) = &self.form else {
            return FormControllerEvent::Ignored;
        };
        let Some(focused_id) = form.focused_field.clone() else {
            return FormControllerEvent::Ignored;
        };
        let Some(field) = form.fields.iter().find(|f| f.id == focused_id) else {
            return FormControllerEvent::Ignored;
        };
        if field.disabled {
            return FormControllerEvent::Ignored;
        }
        let Some(value) = text_field_value(&field.kind) else {
            return FormControllerEvent::Ignored;
        };
        let value = value.to_string();
        self.text_edit = None;
        FormControllerEvent::FormAction(FormEvent::TextInputCommitted {
            id: focused_id,
            value,
        })
    }

    /// Apply `op` to the currently-focused text field (if any), emitting
    /// [`FormEvent::TextInputChanged`] with the new value. The cursor
    /// produced by `op` is cached on `self.text_edit` so the next
    /// keystroke continues from it — see `TextEditState`'s doc.
    fn handle_text_op(&mut self, op: EditOp) -> FormControllerEvent {
        let Some(form) = &self.form else {
            return FormControllerEvent::Ignored;
        };
        let Some(focused_id) = form.focused_field.clone() else {
            return FormControllerEvent::Ignored;
        };
        let Some(field) = form.fields.iter().find(|f| f.id == focused_id) else {
            return FormControllerEvent::Ignored;
        };
        if field.disabled {
            return FormControllerEvent::Ignored;
        }
        let Some(value) = text_field_value(&field.kind) else {
            return FormControllerEvent::Ignored;
        };

        let (cursor, anchor) = match &self.text_edit {
            Some(e) if e.field_id == focused_id => (e.cursor, e.selection_anchor),
            _ => default_text_cursor(&field.kind),
        };
        let cursor = snap_to_char_boundary(value, cursor.min(value.len()));

        let value_before = value.to_string();
        let (new_value, new_cursor, new_anchor) = apply_text_edit_op(value, cursor, anchor, op);
        let text_changed = new_value != value_before;
        // `TextEditor::move_cursor_to` leaves a degenerate `anchor ==
        // cursor` selection in place for a no-op extend (e.g. Shift+Left
        // at offset 0). Left uncleared, the *next* keystroke would see a
        // phantom one-character-wide selection and silently eat a
        // character. Normalise it away here, where the controller caches
        // the anchor across keystrokes (unlike a one-shot `TextEditor`
        // call, which never notices).
        let new_anchor = new_anchor.filter(|a| *a != new_cursor);
        self.text_edit = Some(TextEditState {
            field_id: focused_id.clone(),
            cursor: new_cursor,
            selection_anchor: new_anchor,
        });
        if text_changed {
            FormControllerEvent::FormAction(FormEvent::TextInputChanged {
                id: focused_id,
                value: new_value,
            })
        } else {
            // Cursor-only movement (or selection extend): the field's
            // text content is unchanged, so there is nothing for the app
            // to persist — but the cursor/selection did move, so the
            // caller should still redraw to show it.
            FormControllerEvent::Consumed
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
        if !rect.contains(Point::new(x, y)) {
            return FormControllerEvent::Ignored;
        }
        let (form_rect, sb_rect) = split_rect_lh(self.field_count(), lh, rect);

        if let Some(sb_rect) = sb_rect {
            if sb_rect.contains(Point::new(x, y)) {
                return self.click_scrollbar_lh(lh, form_rect, sb_rect, y);
            }
        }

        if form_rect.contains(Point::new(x, y)) {
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

        match sb.press_zone(y) {
            PressZone::Thumb => {
                self.scroll_drag = Some(ThumbDrag::begin(
                    y,
                    self.scroll_offset as f32,
                    sb.travel(),
                    max_offset as f32,
                ));
                FormControllerEvent::ScrollChanged
            }
            PressZone::Before => {
                self.page_scroll(-(vr as isize), vr);
                FormControllerEvent::ScrollChanged
            }
            PressZone::After => {
                self.page_scroll(vr as isize, vr);
                FormControllerEvent::ScrollChanged
            }
        }
    }

    fn drag_to(&mut self, y: f32) -> FormControllerEvent {
        let Some(drag) = &self.scroll_drag else {
            return FormControllerEvent::Ignored;
        };
        let Some(new) = drag.offset_at(y) else {
            return FormControllerEvent::Ignored;
        };
        let new = new.round() as usize;
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
        if let FieldKind::Toolbar(toolbar) = &field.kind {
            if toolbar.buttons.iter().any(|btn| {
                matches!(btn,
                    crate::primitives::toolbar::ToolbarButton::Action { id, .. }
                    if id == clicked_id
                )
            }) {
                return FormEvent::ToolbarButtonClicked {
                    field_id: field.id.clone(),
                    button_id: clicked_id.clone(),
                };
            }
        }
    }
    FormEvent::FocusChanged {
        id: clicked_id.clone(),
    }
}

// ── Keyboard editing helpers ──────────────────────────────────────────

/// Whether `field` is a Tab stop: not a `Label`, not a non-interactive
/// `ReadOnly` display field, and not disabled. Mirrors
/// `primitives/form.rs`'s module doc ("Tab / Shift-Tab moves
/// focused_field forward/backward through interactive fields (skip
/// Label)") — VS Code, the stated GUI reference, does not focus a
/// read-only row either.
fn field_is_focusable(field: &crate::primitives::form::FormField) -> bool {
    !matches!(field.kind, FieldKind::Label | FieldKind::ReadOnly { .. }) && !field.disabled
}

/// The editable text of a `TextInput` / `TextArea` / `PasswordInput`
/// field, or `None` for every other `FieldKind` (not text-editable).
fn text_field_value(kind: &FieldKind) -> Option<&str> {
    match kind {
        FieldKind::TextInput { value, .. }
        | FieldKind::TextArea { value, .. }
        | FieldKind::PasswordInput { value, .. } => Some(value.as_str()),
        _ => None,
    }
}

/// Starting cursor/selection for a text field that has no cached
/// [`TextEditState`] yet — the field's own `cursor` (defaulting to
/// end-of-text) and `selection_anchor` where the kind carries one.
fn default_text_cursor(kind: &FieldKind) -> (usize, Option<usize>) {
    match kind {
        FieldKind::TextInput {
            value,
            cursor,
            selection_anchor,
            ..
        } => (cursor.unwrap_or(value.len()), *selection_anchor),
        FieldKind::TextArea { value, cursor, .. } => (cursor.unwrap_or(value.len()), None),
        FieldKind::PasswordInput { value, cursor, .. } => (cursor.unwrap_or(value.len()), None),
        _ => (0, None),
    }
}

/// Write `cursor`/`selection_anchor` back onto `kind` for rendering.
/// No-op for `FieldKind` variants with no cursor (anything but the three
/// text-editable kinds).
fn overlay_text_cursor(kind: &mut FieldKind, cursor: usize, selection_anchor: Option<usize>) {
    match kind {
        FieldKind::TextInput {
            cursor: c,
            selection_anchor: a,
            ..
        } => {
            *c = Some(cursor);
            *a = selection_anchor;
        }
        FieldKind::TextArea { cursor: c, .. } | FieldKind::PasswordInput { cursor: c, .. } => {
            *c = Some(cursor);
        }
        _ => {}
    }
}

/// Byte offset `byte_offset` into `value` (which may contain `'\n'`,
/// e.g. a `TextArea`) → `(line, char_column)`, the coordinate space
/// [`TextEditor`]/[`EditOp`] operate in.
fn byte_offset_to_line_col(value: &str, byte_offset: usize) -> (usize, usize) {
    let byte_offset = byte_offset.min(value.len());
    let prefix = &value[..byte_offset];
    let line = prefix.matches('\n').count();
    let line_start = prefix.rfind('\n').map(|i| i + 1).unwrap_or(0);
    let col = prefix[line_start..].chars().count();
    (line, col)
}

/// Inverse of [`byte_offset_to_line_col`]: `(line, char_column)` →
/// byte offset into `value`.
fn line_col_to_byte_offset(value: &str, line: usize, col: usize) -> usize {
    let mut offset = 0usize;
    for (i, l) in value.split('\n').enumerate() {
        if i == line {
            let take: usize = l.chars().take(col).map(char::len_utf8).sum();
            return offset + take.min(l.len());
        }
        offset += l.len() + 1; // +1 for the '\n' separator consumed between lines
    }
    value.len()
}

/// Route one [`EditOp`] through [`TextEditor::apply`] against a flat
/// `value` + byte-offset `cursor`/`selection_anchor` — the shape every
/// `FieldKind::TextInput`/`TextArea`/`PasswordInput` field uses — and
/// hand back the resulting `(value, cursor, selection_anchor)`.
///
/// `TextEditor` itself works in `(line, char_column)` coordinates (see
/// `text_input.rs`'s module doc), so this wraps `value` in a throwaway
/// single-use [`TextInput`] for the one `apply` call and converts back
/// via [`byte_offset_to_line_col`]/[`line_col_to_byte_offset`] — the
/// conversion this controller owns so every other call site keeps
/// working in the byte-offset space `Form`'s `FieldKind` already
/// documents.
fn apply_text_edit_op(
    value: &str,
    cursor: usize,
    selection_anchor: Option<usize>,
    op: EditOp,
) -> (String, usize, Option<usize>) {
    let lines: Vec<String> = if value.is_empty() {
        vec![String::new()]
    } else {
        value.split('\n').map(str::to_string).collect()
    };
    let (line, col) = byte_offset_to_line_col(value, cursor);
    let input = TextInput::new(WidgetId::new("form-field-edit"))
        .with_lines(lines)
        .with_cursor_line(line)
        .with_cursor_col(col);
    let mut editor = TextEditor::new(input);
    if let Some(anchor_byte) = selection_anchor {
        // `selection_anchor` is app-owned public data (see
        // `primitives/form.rs`'s doc on `FieldKind::TextInput`) and isn't
        // guaranteed to land on a char boundary — snap and clamp it the
        // same way `cursor` already is above, so a bogus byte offset
        // (e.g. an app computing it in char counts) clamps the
        // selection instead of panicking on a mid-char slice.
        let anchor_byte = snap_to_char_boundary(value, anchor_byte.min(value.len()));
        editor.set_selection_anchor(Some(byte_offset_to_line_col(value, anchor_byte)));
    }
    editor.apply(op);

    let new_value = editor.lines.join("\n");
    let new_cursor = line_col_to_byte_offset(&new_value, editor.cursor_line, editor.cursor_col);
    let new_anchor = editor
        .selection_anchor()
        .map(|(l, c)| line_col_to_byte_offset(&new_value, l, c));
    (new_value, new_cursor, new_anchor)
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
                .map(|b| {
                    let icon_w = b
                        .icon
                        .as_ref()
                        .map(|i| {
                            let gw = i.fallback.chars().count() as f32;
                            if b.label.is_empty() {
                                gw
                            } else {
                                gw + 1.0
                            }
                        })
                        .unwrap_or(0.0);
                    FormItemMeasure {
                        id: b.id.clone(),
                        width: (b.label.chars().count() as f32 + icon_w + 2.0) * char_w,
                    }
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

    #[test]
    fn click_toolbar_button_emits_toolbar_button_clicked() {
        use crate::primitives::toolbar::{Toolbar, ToolbarButton};
        let mut form = make_form(1);
        form.fields[0].kind = FieldKind::Toolbar(Toolbar {
            id: WidgetId::new("tb"),
            buttons: vec![
                ToolbarButton::Action {
                    id: WidgetId::new("reset"),
                    label: "Reset".into(),
                    icon: None,
                    key_hint: None,
                    enabled: true,
                    is_active: false,
                    tooltip: String::new(),
                },
                ToolbarButton::Action {
                    id: WidgetId::new("export"),
                    label: "Export".into(),
                    icon: None,
                    key_hint: None,
                    enabled: true,
                    is_active: false,
                    tooltip: String::new(),
                },
            ],
            bg: None,
            focused_index: None,
        });

        // Clicking the first button should emit ToolbarButtonClicked with
        // the owning field_id and the button_id.
        let ev = form_click_event(&form, &WidgetId::new("reset"));
        assert_eq!(
            ev,
            FormEvent::ToolbarButtonClicked {
                field_id: WidgetId::new("field-0"),
                button_id: WidgetId::new("reset"),
            },
            "clicking 'reset' should emit ToolbarButtonClicked"
        );

        // Clicking the second button should emit ToolbarButtonClicked too.
        let ev2 = form_click_event(&form, &WidgetId::new("export"));
        assert_eq!(
            ev2,
            FormEvent::ToolbarButtonClicked {
                field_id: WidgetId::new("field-0"),
                button_id: WidgetId::new("export"),
            },
            "clicking 'export' should emit ToolbarButtonClicked"
        );
    }

    #[test]
    fn click_toolbar_disabled_button_still_emits_toolbar_button_clicked() {
        // A click on a disabled action still routes through form_click_event
        // (the primitive layout marks it non-clickable, so the hit won't
        // fire in practice). But if a caller does pass a disabled action id
        // in, we should still recover the field_id correctly.
        use crate::primitives::toolbar::{Toolbar, ToolbarButton};
        let mut form = make_form(1);
        form.fields[0].kind = FieldKind::Toolbar(Toolbar {
            id: WidgetId::new("tb"),
            buttons: vec![ToolbarButton::Action {
                id: WidgetId::new("noop"),
                label: "Noop".into(),
                icon: None,
                key_hint: None,
                enabled: false,
                is_active: false,
                tooltip: String::new(),
            }],
            bg: None,
            focused_index: None,
        });

        // form_click_event matches by id, not by enabled state.
        let ev = form_click_event(&form, &WidgetId::new("noop"));
        assert_eq!(
            ev,
            FormEvent::ToolbarButtonClicked {
                field_id: WidgetId::new("field-0"),
                button_id: WidgetId::new("noop"),
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

    // ── Keyboard editing ───────────────────────────────────────────

    fn key_event(key: Key, modifiers: Modifiers) -> UiEvent {
        UiEvent::KeyPressed {
            key,
            modifiers,
            repeat: false,
        }
    }

    fn shift() -> Modifiers {
        Modifiers {
            shift: true,
            ..Default::default()
        }
    }

    fn text_field(id: &str, value: &str, cursor: Option<usize>) -> FormField {
        FormField {
            id: WidgetId::new(id),
            label: StyledText::plain("Label"),
            kind: FieldKind::TextInput {
                value: value.to_string(),
                placeholder: String::new(),
                cursor,
                selection_anchor: None,
            },
            hint: StyledText::default(),
            disabled: false,
            validation: None,
        }
    }

    fn label_field(id: &str) -> FormField {
        FormField {
            id: WidgetId::new(id),
            label: StyledText::plain("Header"),
            kind: FieldKind::Label,
            hint: StyledText::default(),
            disabled: false,
            validation: None,
        }
    }

    fn toggle_field(id: &str, disabled: bool) -> FormField {
        FormField {
            id: WidgetId::new(id),
            label: StyledText::plain("On"),
            kind: FieldKind::Toggle { value: false },
            hint: StyledText::default(),
            disabled,
            validation: None,
        }
    }

    fn controller_with(fields: Vec<FormField>, focused: Option<&str>) -> FormController {
        let mut fc = FormController::new("f".into());
        fc.set_form(Form {
            id: WidgetId::new("form"),
            fields,
            focused_field: focused.map(WidgetId::new),
            scroll_offset: 0,
            has_focus: true,
        });
        // The controller's own `has_focus` gates `KeyPressed`/`CharTyped`/
        // `ClipboardPaste` (not `Form.has_focus` above, which only
        // affects rendering) — these keyboard-editing tests need it set.
        fc.set_has_focus(true);
        fc.set_backend_info(1.0);
        fc
    }

    const RECT: Rect = Rect::new(0.0, 0.0, 40.0, 10.0);

    #[test]
    fn tab_moves_focus_to_first_focusable_field_skipping_label() {
        let mut fc = controller_with(
            vec![
                label_field("hdr"),
                text_field("name", "", None),
                toggle_field("tgl", false),
            ],
            None,
        );
        let ev = fc.handle_cached(
            &key_event(Key::Named(NamedKey::Tab), Modifiers::default()),
            RECT,
        );
        assert_eq!(
            ev,
            FormControllerEvent::FormAction(FormEvent::FocusChanged {
                id: WidgetId::new("name")
            })
        );
    }

    #[test]
    fn tab_advances_and_wraps_around() {
        let mut fc = controller_with(
            vec![text_field("a", "", None), toggle_field("b", false)],
            Some("b"),
        );
        let ev = fc.handle_cached(
            &key_event(Key::Named(NamedKey::Tab), Modifiers::default()),
            RECT,
        );
        assert_eq!(
            ev,
            FormControllerEvent::FormAction(FormEvent::FocusChanged {
                id: WidgetId::new("a")
            }),
            "tab from the last field should wrap to the first"
        );
    }

    #[test]
    fn shift_tab_retreats_and_wraps() {
        let mut fc = controller_with(
            vec![text_field("a", "", None), toggle_field("b", false)],
            Some("a"),
        );
        let ev = fc.handle_cached(&key_event(Key::Named(NamedKey::Tab), shift()), RECT);
        assert_eq!(
            ev,
            FormControllerEvent::FormAction(FormEvent::FocusChanged {
                id: WidgetId::new("b")
            }),
            "shift+tab from the first field should wrap to the last"
        );
    }

    #[test]
    fn back_tab_also_retreats() {
        let mut fc = controller_with(
            vec![text_field("a", "", None), toggle_field("b", false)],
            Some("b"),
        );
        let ev = fc.handle_cached(
            &key_event(Key::Named(NamedKey::BackTab), Modifiers::default()),
            RECT,
        );
        assert_eq!(
            ev,
            FormControllerEvent::FormAction(FormEvent::FocusChanged {
                id: WidgetId::new("a")
            })
        );
    }

    #[test]
    fn tab_skips_disabled_field() {
        let mut fc = controller_with(
            vec![
                text_field("a", "", None),
                toggle_field("disabled-tgl", true),
                toggle_field("c", false),
            ],
            Some("a"),
        );
        let ev = fc.handle_cached(
            &key_event(Key::Named(NamedKey::Tab), Modifiers::default()),
            RECT,
        );
        assert_eq!(
            ev,
            FormControllerEvent::FormAction(FormEvent::FocusChanged {
                id: WidgetId::new("c")
            }),
            "tab should skip the disabled field entirely"
        );
    }

    #[test]
    fn typing_char_appends_and_updates_value() {
        let mut fc = controller_with(vec![text_field("name", "ab", Some(2))], Some("name"));
        let ev = fc.handle_cached(&key_event(Key::Char('c'), Modifiers::default()), RECT);
        assert_eq!(
            ev,
            FormControllerEvent::FormAction(FormEvent::TextInputChanged {
                id: WidgetId::new("name"),
                value: "abc".to_string(),
            })
        );
    }

    #[test]
    fn cursor_persists_across_keystrokes_even_if_app_never_echoes_it_back() {
        let mut fc = controller_with(vec![text_field("name", "ab", Some(2))], Some("name"));
        let ev1 = fc.handle_cached(&key_event(Key::Char('c'), Modifiers::default()), RECT);
        assert_eq!(
            ev1,
            FormControllerEvent::FormAction(FormEvent::TextInputChanged {
                id: WidgetId::new("name"),
                value: "abc".to_string(),
            })
        );

        // The app updates `value` per the emitted event but — as the
        // issue requires — never tracks `cursor` itself; it stays `None`.
        fc.set_form(Form {
            id: WidgetId::new("form"),
            fields: vec![text_field("name", "abc", None)],
            focused_field: Some(WidgetId::new("name")),
            scroll_offset: 0,
            has_focus: true,
        });

        let ev2 = fc.handle_cached(&key_event(Key::Char('d'), Modifiers::default()), RECT);
        assert_eq!(
            ev2,
            FormControllerEvent::FormAction(FormEvent::TextInputChanged {
                id: WidgetId::new("name"),
                value: "abcd".to_string(),
            }),
            "the controller's cached cursor (after 'c') should carry over, \
             appending 'd' at the end rather than re-deriving a stale position"
        );
    }

    #[test]
    fn backspace_deletes_char_before_cursor() {
        let mut fc = controller_with(vec![text_field("name", "abc", Some(3))], Some("name"));
        let ev = fc.handle_cached(
            &key_event(Key::Named(NamedKey::Backspace), Modifiers::default()),
            RECT,
        );
        assert_eq!(
            ev,
            FormControllerEvent::FormAction(FormEvent::TextInputChanged {
                id: WidgetId::new("name"),
                value: "ab".to_string(),
            })
        );
    }

    #[test]
    fn enter_commits_text_field_instead_of_inserting_newline() {
        let mut fc = controller_with(vec![text_field("name", "hello", Some(5))], Some("name"));
        let ev = fc.handle_cached(
            &key_event(Key::Named(NamedKey::Enter), Modifiers::default()),
            RECT,
        );
        assert_eq!(
            ev,
            FormControllerEvent::FormAction(FormEvent::TextInputCommitted {
                id: WidgetId::new("name"),
                value: "hello".to_string(),
            })
        );
    }

    #[test]
    fn char_typed_event_also_inserts_into_the_focused_field() {
        let mut fc = controller_with(vec![text_field("name", "", None)], Some("name"));
        let ev = fc.handle_cached(&UiEvent::CharTyped('x'), RECT);
        assert_eq!(
            ev,
            FormControllerEvent::FormAction(FormEvent::TextInputChanged {
                id: WidgetId::new("name"),
                value: "x".to_string(),
            })
        );
    }

    #[test]
    fn disabled_text_field_ignores_keystrokes() {
        let mut fc = controller_with(
            vec![FormField {
                id: WidgetId::new("name"),
                label: StyledText::plain("Name"),
                kind: FieldKind::TextInput {
                    value: "ab".to_string(),
                    placeholder: String::new(),
                    cursor: Some(2),
                    selection_anchor: None,
                },
                hint: StyledText::default(),
                disabled: true,
                validation: None,
            }],
            Some("name"),
        );
        let ev = fc.handle_cached(&key_event(Key::Char('c'), Modifiers::default()), RECT);
        assert_eq!(ev, FormControllerEvent::Ignored);
    }

    #[test]
    fn no_focused_field_ignores_text_keys() {
        let mut fc = controller_with(vec![text_field("name", "ab", Some(2))], None);
        let ev = fc.handle_cached(&key_event(Key::Char('c'), Modifiers::default()), RECT);
        assert_eq!(ev, FormControllerEvent::Ignored);
    }

    #[test]
    fn shift_left_then_char_replaces_the_selection() {
        // "ab|c" (cursor after 'b') — shift+Left selects "b", typing 'X'
        // replaces the selection (EditOp::InsertChar's documented
        // selection-replace behaviour), landing on "aXc".
        let mut fc = controller_with(vec![text_field("name", "abc", Some(2))], Some("name"));
        let ev1 = fc.handle_cached(&key_event(Key::Named(NamedKey::Left), shift()), RECT);
        assert_eq!(
            ev1,
            FormControllerEvent::Consumed,
            "pure cursor/selection movement changes no text, so it's Consumed not a FormEvent"
        );

        let ev2 = fc.handle_cached(&key_event(Key::Char('X'), Modifiers::default()), RECT);
        assert_eq!(
            ev2,
            FormControllerEvent::FormAction(FormEvent::TextInputChanged {
                id: WidgetId::new("name"),
                value: "aXc".to_string(),
            })
        );
    }

    #[test]
    fn build_form_overlays_the_cached_cursor_for_rendering() {
        let mut fc = controller_with(vec![text_field("name", "ab", Some(2))], Some("name"));
        fc.handle_cached(&key_event(Key::Char('c'), Modifiers::default()), RECT);

        // Realistic round-trip: the app persists `value` from the
        // emitted `TextInputChanged` (same as it already does for a
        // toggle flip) but — per this module's "Keyboard editing" doc
        // section — never has to track `cursor` itself, so it stays
        // `None` here.
        fc.set_form(Form {
            id: WidgetId::new("form"),
            fields: vec![text_field("name", "abc", None)],
            focused_field: Some(WidgetId::new("name")),
            scroll_offset: 0,
            has_focus: true,
        });

        let rendered = fc.build_form(RECT);
        match &rendered.fields[0].kind {
            FieldKind::TextInput { value, cursor, .. } => {
                assert_eq!(value, "abc");
                assert_eq!(
                    *cursor,
                    Some(3),
                    "controller-cached cursor should overlay the app's \
                     None, sitting after the just-typed 'c'"
                );
            }
            other => panic!("expected TextInput, got {other:?}"),
        }
    }

    #[test]
    fn text_area_newline_round_trips_through_byte_offset_conversion() {
        // TextArea's `value` embeds '\n' directly — exercise the
        // multi-line byte_offset_to_line_col / line_col_to_byte_offset
        // conversion, not just the single-line TextInput path.
        let field = FormField {
            id: WidgetId::new("notes"),
            label: StyledText::plain("Notes"),
            kind: FieldKind::TextArea {
                value: "line one\nline two".to_string(),
                placeholder: String::new(),
                cursor: Some("line one\n".len()), // start of "line two"
                visible_rows: 3,
            },
            hint: StyledText::default(),
            disabled: false,
            validation: None,
        };
        let mut fc = controller_with(vec![field], Some("notes"));
        let ev = fc.handle_cached(&key_event(Key::Char('X'), Modifiers::default()), RECT);
        assert_eq!(
            ev,
            FormControllerEvent::FormAction(FormEvent::TextInputChanged {
                id: WidgetId::new("notes"),
                value: "line one\nXline two".to_string(),
            })
        );
    }

    fn ctrl() -> Modifiers {
        Modifiers {
            ctrl: true,
            ..Default::default()
        }
    }

    fn cmd() -> Modifiers {
        Modifiers {
            cmd: true,
            ..Default::default()
        }
    }

    #[test]
    fn ctrl_c_on_a_focused_text_field_is_ignored_not_typed() {
        // Every backend delivers Ctrl+C as `Key::Char('c')` with
        // `modifiers.ctrl == true` (see this module's "Keyboard editing"
        // doc section) — without the guard in `handle_key`, this would
        // insert a literal 'c' into the field *and* report
        // `TextInputChanged`, i.e. the keystroke was "consumed" as text
        // instead of falling through to an app-level Copy accelerator.
        let mut fc = controller_with(vec![text_field("name", "ab", Some(2))], Some("name"));
        let ev = fc.handle_cached(&key_event(Key::Char('c'), ctrl()), RECT);
        assert_eq!(ev, FormControllerEvent::Ignored);
        // The value is genuinely untouched, not just the return value.
        match &fc.form().unwrap().fields[0].kind {
            FieldKind::TextInput { value, .. } => assert_eq!(value, "ab"),
            other => panic!("expected TextInput, got {other:?}"),
        }
    }

    #[test]
    fn cmd_s_on_a_focused_text_field_is_ignored_not_typed() {
        // Same guard, macOS's modifier.
        let mut fc = controller_with(vec![text_field("name", "ab", Some(2))], Some("name"));
        let ev = fc.handle_cached(&key_event(Key::Char('s'), cmd()), RECT);
        assert_eq!(ev, FormControllerEvent::Ignored);
    }

    #[test]
    fn selection_anchor_mid_multibyte_char_snaps_instead_of_panicking() {
        // `selection_anchor` is app-owned public data (`primitives/form.rs`
        // documents it as "a byte offset into `value`") and isn't
        // guaranteed to land on a char boundary — e.g. an app that
        // accidentally computes it in char counts instead of bytes.
        // `byte_offset_to_line_col` slices `value[..byte_offset]`, which
        // would panic on a non-boundary offset into "h\u{e9}llo" (byte 2
        // is the second byte of the two-byte 'é').
        let field = FormField {
            id: WidgetId::new("name"),
            label: StyledText::plain("Name"),
            kind: FieldKind::TextInput {
                value: "h\u{e9}llo".to_string(),
                placeholder: String::new(),
                cursor: Some(5),
                selection_anchor: Some(2), // mid-char: not a valid boundary
            },
            hint: StyledText::default(),
            disabled: false,
            validation: None,
        };
        let mut fc = controller_with(vec![field], Some("name"));
        // Must not panic. `EditOp::MoveRight` with `extend: false` drops
        // the selection, so the exact resulting event only needs to be
        // some non-panicking outcome — the regression is the absence of
        // a panic, not this particular value.
        let _ = fc.handle_cached(
            &key_event(Key::Named(NamedKey::Right), Modifiers::default()),
            RECT,
        );
    }
}
