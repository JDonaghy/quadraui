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
//! ## Keyboard, focus and clicks live in the widget
//!
//! `KeyPressed` / `CharTyped` / `ClipboardPaste` / `MouseDown` route to
//! [`crate::primitives::form::contract`], the one place this widget's
//! behaviour lives — per-`FieldKind` keys, Tab/Shift-Tab traversal with
//! scroll-into-view, text editing, click routing and the slider drag.
//! That module's own doc describes each field kind's contract; this
//! controller adds only the three things around it:
//!
//! - the [`FormController::has_focus`] gate. A host is expected to
//!   forward every event in (see the two event-handling paths above), so
//!   without it a form that isn't even the active panel would still
//!   swallow `Tab` and every other key. Toggle it with
//!   [`FormController::set_has_focus`], the same shape
//!   `compose::sidebar_system` uses.
//! - the scrollbar: its own thumb drag, track paging and wheel handling,
//!   plus the [`crate::Scrollbar`] it paints beside the form body.
//! - the declarative rules below, applied on every
//!   [`FormController::set_form`].
//!
//! The live caret, scrub position and item cursor are
//! [`crate::primitives::form::FormState`], held here and handed to the
//! widget on every event, so this controller and a
//! `Surface::Widget` form resolve every key and click identically.
//!
//! ## Declarative validation
//!
//! [`FormController::set_rules`] attaches a [`crate::validation::FormRules`]
//! to the controller. From then on, every [`FormController::set_form`] call
//! evaluates those rules against the pushed `Form` via
//! [`crate::validation::validate`] before storing it — so the app gets
//! `required`/`pattern`/cross-field `Error`/`Warning` state on
//! `FormField::validation` for free, with no per-keystroke validation logic
//! of its own. A controller with no rules set (`FormRules::default()`,
//! the starting state) behaves exactly as before: `set_form` stores the
//! `Form` unchanged.
//!
//! ## Declarative visibility
//!
//! [`FormController::set_visibility`] attaches a
//! [`crate::visibility::FormVisibility`] to the controller. From then on,
//! every [`FormController::set_form`] call evaluates those rules against
//! the pushed `Form` via [`crate::visibility::apply_visibility`] — *after*
//! [`Self::set_rules`]'s validation, so cross-field validation still sees
//! every field's value before any of them are hidden — removing fields
//! whose `visible_when` is false and setting `disabled` for fields with an
//! `enabled_when`. A controller with no visibility rules set
//! (`FormVisibility::default()`, the starting state) behaves exactly as
//! before: `set_form` stores the `Form` unchanged beyond validation.
//!
//! `apply_visibility` clears `Form::focused_field` when the field it named
//! is hidden, but `self.text_edit` / `self.date_edit` are left pointing at
//! that field's in-flight edit state. This is deliberate: `build_form`
//! looks the edit state up by id via `.find()`, so it is simply inert
//! while the field stays hidden, and if the field reappears with the same
//! id the cached cursor/scrub position resuming is the desired behaviour
//! rather than a reset.

use crate::behaviour::scroll::ensure_visible;
use crate::compose::Handled;
use crate::primitives::form::contract as form_contract;
// Re-exported under this module's own name: `compose::sidebar_system`
// resolves a form-field click through the same function a `Form`
// widget's own `Interactive` does.
pub(crate) use crate::primitives::form::contract::form_click_event;
use crate::primitives::form::contract::SLIDER_TRACK_CELLS;
use crate::primitives::form::{
    FieldKind, Form, FormEvent, FormFieldMeasure, FormItemMeasure, FormState,
};
use crate::primitives::scrollbar::{scroll_by_clamped, wheel_row_delta, PressZone, ThumbDrag};
use crate::primitives::text_input::EditOp;
use crate::types::StyledSpan;
use crate::validation::{validate, FormRules, Validator};
use crate::visibility::{apply_visibility, FormVisibility};
use crate::{Backend, ButtonMask, MouseButton, Point, Rect, Scrollbar, Theme, UiEvent, WidgetId};

/// Semantic payload of [`Handled::Event`] returned by
/// [`FormController::handle_event`]/[`FormController::handle_cached_event`].
#[derive(Debug, Clone, PartialEq)]
pub enum FormControllerAction {
    /// A field-level event occurred (toggle changed, text input, etc.).
    FormAction(FormEvent),
    /// Scrollbar interaction changed the scroll offset.
    ScrollChanged,
}

/// What happened after [`FormController::handle`] processed an event.
#[derive(Debug, Clone, PartialEq)]
#[deprecated(
    since = "0.9.0",
    note = "use `FormController::handle_event`/`handle_cached_event`'s `Handled<FormControllerAction>` instead — `Consumed`/`Ignored` are `Handled::Consumed`/`Handled::Ignored`, `FormAction`/`ScrollChanged` move to `FormControllerAction` under `Handled::Event`"
)]
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

/// Convert a [`Handled<FormEvent>`] from the widget's own behaviour
/// (`primitives::form`) into this controller's internal result type.
#[allow(deprecated)] // `FormControllerEvent` is `#[deprecated]`; this is its own adapter.
fn handled_to_form_controller_event(handled: Handled<FormEvent>) -> FormControllerEvent {
    match handled {
        Handled::Event(e) => FormControllerEvent::FormAction(e),
        Handled::Consumed => FormControllerEvent::Consumed,
        Handled::Ignored => FormControllerEvent::Ignored,
    }
}

/// Convert the internal [`FormControllerEvent`] result into
/// [`Handled<FormControllerAction>`].
#[allow(deprecated)] // `FormControllerEvent` is `#[deprecated]`; this is its own adapter.
fn handled_from_form_controller_event(event: FormControllerEvent) -> Handled<FormControllerAction> {
    match event {
        FormControllerEvent::FormAction(e) => Handled::Event(FormControllerAction::FormAction(e)),
        FormControllerEvent::ScrollChanged => Handled::Event(FormControllerAction::ScrollChanged),
        FormControllerEvent::Consumed => Handled::Consumed,
        FormControllerEvent::Ignored => Handled::Ignored,
    }
}

pub struct FormController {
    id: String,
    form: Option<Form>,
    scroll_offset: usize,
    has_focus: bool,
    scroll_drag: Option<ThumbDrag>,
    /// The backend's line height, cached by `render` so
    /// `handle_cached_event` can resolve geometry without a backend.
    cached_lh: Option<f32>,
    /// Caret, scrub position, item cursor and in-flight slider drag —
    /// the widget's own interaction state, so this controller and a
    /// `Surface::Widget` form resolve every key and click through the
    /// one implementation in `primitives::form`.
    state: FormState,
    rules: FormRules,
    visibility: FormVisibility,
}

// `handle_inner` and its private helpers still speak `FormControllerEvent`
// internally — `handle_event`/`handle_cached_event` below are the
// `Handled<FormControllerAction>`-returning public surface, adapting the
// internal result at the boundary via `handled_from_form_controller_event`,
// so this one block is exempted rather than threading the new type through
// every match arm of the click/key dispatch tree.
#[allow(deprecated)]
impl FormController {
    pub fn new(id: String) -> Self {
        Self {
            id,
            form: None,
            scroll_offset: 0,
            has_focus: false,
            scroll_drag: None,
            cached_lh: None,
            state: FormState::default(),
            rules: FormRules::default(),
            visibility: FormVisibility::default(),
        }
    }

    // ── Per-frame data ────────────────────────────────────────────────

    /// Store `form`, applying [`Self::set_rules`]'s declarative validation
    /// and then [`Self::set_visibility`]'s declarative visibility/
    /// enablement — see this module's "Declarative validation" and
    /// "Declarative visibility" doc sections.
    pub fn set_form(&mut self, mut form: Form) {
        validate(&mut form, &self.rules);
        apply_visibility(&mut form, &self.visibility);
        self.form = Some(form);
    }

    /// Set the declarative rules [`Self::set_form`] evaluates against every
    /// `Form` pushed from then on. Pass `FormRules::default()` to clear
    /// them (equivalent to never having called this).
    pub fn set_rules(&mut self, rules: FormRules) {
        self.rules = rules;
    }

    /// The rules most recently set via [`Self::set_rules`] (empty/default
    /// if never called).
    pub fn rules(&self) -> &FormRules {
        &self.rules
    }

    /// Set the declarative visibility/enablement rules [`Self::set_form`]
    /// evaluates against every `Form` pushed from then on. Pass
    /// `FormVisibility::default()` to clear them (equivalent to never
    /// having called this).
    pub fn set_visibility(&mut self, visibility: FormVisibility) {
        self.visibility = visibility;
    }

    /// The visibility rules most recently set via [`Self::set_visibility`]
    /// (empty/default if never called).
    pub fn visibility(&self) -> &FormVisibility {
        &self.visibility
    }

    // ── State accessors ───────────────────────────────────────────────

    pub fn form(&self) -> Option<&Form> {
        self.form.as_ref()
    }

    /// The `Form` this controller actually renders/exposes to a11y this
    /// frame: [`Self::form`]'s stored descriptor overlaid with the live
    /// render-only state [`Self::render`] bakes in before painting
    /// (`has_focus`, in-progress text/date-edit cursor, scroll offset,
    /// required-field markers) — see [`Self::build_form`]. Distinct from
    /// `form()` so a composing controller that embeds this `Form` inside
    /// its *own* descriptor (e.g.
    /// [`crate::compose::form_dialog::FormDialogController::dialog`])
    /// reports the state that's actually on screen rather than the raw
    /// one the consumer last pushed via [`Self::set_form`] (which may
    /// have gone stale — e.g. `has_focus` toggled by [`Self::set_has_focus`]
    /// since).
    pub fn live_form(&self) -> Option<Form> {
        self.form
            .as_ref()
            .map(|_| self.build_form(Rect::default(), None))
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
        let theme = backend.theme();
        let form = self.build_form(form_rect, Some(&theme));
        crate::backend::DrawsPrimitives::draw_primitive(backend, form_rect, &form);
        self.draw_item_focus_ring(backend, form_rect, &form);
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

    /// Drive the state machine with a backend-neutral [`UiEvent`].
    pub fn handle_event(
        &mut self,
        event: &UiEvent,
        backend: &mut dyn Backend,
        rect: Rect,
    ) -> Handled<FormControllerAction> {
        let lh = backend.line_height();
        handled_from_form_controller_event(self.handle_inner(event, rect, lh, Some(backend)))
    }

    /// Drive the state machine with a backend-neutral [`UiEvent`].
    #[deprecated(
        since = "0.9.0",
        note = "use `handle_event`, which returns `Handled<FormControllerAction>` instead of `FormControllerEvent`"
    )]
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
    /// [`Handled::Ignored`] if metrics aren't cached yet.
    pub fn handle_cached_event(
        &mut self,
        event: &UiEvent,
        rect: Rect,
    ) -> Handled<FormControllerAction> {
        let Some(lh) = self.cached_lh else {
            return Handled::Ignored;
        };
        handled_from_form_controller_event(self.handle_inner(event, rect, lh, None))
    }

    /// Backend-free event handler.
    #[deprecated(
        since = "0.9.0",
        note = "use `handle_cached_event`, which returns `Handled<FormControllerAction>` instead of `FormControllerEvent`"
    )]
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
        self.scroll_offset = ensure_visible(self.scroll_offset, viewport_rows, field_idx);
    }

    pub fn viewport_rows(&self, backend: &dyn Backend, rect: Rect) -> usize {
        viewport_rows_lh(backend.line_height(), rect)
    }

    /// Resolve where field `id` paints inside `rect` (the same rect
    /// passed to [`Self::render`]/[`Self::handle`]), in `rect`'s own
    /// coordinate space. `None` if `id` isn't a field on the currently
    /// stored form, or is scrolled out of the visible viewport.
    ///
    /// For a multi-field `Form`, the field's own rect is *not* the whole
    /// form rect — a popup-anchored field kind (`ComboBox`, `DatePicker`,
    /// `ColorPicker`) paints only its compact row; the host paints the
    /// open popup itself as a separate overlay (see this module's
    /// "ComboBox fields" doc section) and needs the row's on-screen
    /// position to anchor it. This is that lookup: it reruns the same
    /// [`crate::DrawsPrimitives::layout_primitive`] call
    /// [`Self::click_inner`] already uses
    /// for hit-testing, then offsets the matching field's layout-relative
    /// `bounds` by `rect`'s origin (and the scrollbar-split form rect's,
    /// if a scrollbar is present) to hand back an on-screen rect the host
    /// can paint a field's own overlay at, such as an open `ComboBox`.
    pub fn field_rect(&self, id: WidgetId, backend: &dyn Backend, rect: Rect) -> Option<Rect> {
        let lh = backend.line_height();
        let (form_rect, _sb_rect) = split_rect_lh(self.field_count(), lh, rect);
        let form = self.build_form(form_rect, None);
        let layout =
            crate::backend::DrawsPrimitives::layout_primitive(backend, form_rect, &form).layout;
        layout.visible_fields.iter().find(|f| f.id == id).map(|f| {
            Rect::new(
                form_rect.x + f.bounds.x,
                form_rect.y + f.bounds.y,
                f.bounds.width,
                f.bounds.height,
            )
        })
    }

    /// Run `f` against the stored form and this controller's live
    /// [`FormState`] — the one seam every delegated keystroke goes
    /// through, so the behaviour lives in `primitives::form` and this
    /// controller only owns the scrollbar, the declarative rules and
    /// the [`UiEvent`] adaptation around it.
    ///
    /// The widget's own scroll offset and this controller's public
    /// [`Self::scroll_offset`] are synchronised around the call, so a
    /// scroll-into-view inside `f` lands on both.
    #[allow(deprecated)] // `FormControllerEvent` is `#[deprecated]`; this returns it internally.
    fn delegate<F>(&mut self, f: F) -> FormControllerEvent
    where
        F: FnOnce(&Form, &mut FormState) -> Handled<FormEvent>,
    {
        let Some(form) = self.form.as_ref() else {
            return FormControllerEvent::Ignored;
        };
        self.state.scroll_offset = Some(self.scroll_offset);
        let result = f(form, &mut self.state);
        self.scroll_offset = self.state.scroll_offset.unwrap_or(self.scroll_offset);
        handled_to_form_controller_event(result)
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

            // A `Slider` drag takes priority over scrollbar-thumb drag:
            // the two states are mutually exclusive (only one gesture can
            // be in flight from a single `MouseDown`), but checking
            // `slider_drag` first keeps this arm's intent readable as
            // "whichever press started, keep tracking it" rather than
            // depending on `drag_to`'s own no-op-when-`None` fallthrough.
            UiEvent::MouseMoved {
                position,
                buttons:
                    ButtonMask {
                        left: true,
                        middle: _,
                        right: _,
                    },
            } => {
                if self.state.slider_drag.is_some() {
                    self.slider_drag_to(rect, position.x, lh, backend)
                } else {
                    self.drag_to(position.y)
                }
            }

            UiEvent::MouseUp {
                button: MouseButton::Left,
                ..
            } => {
                self.scroll_drag = None;
                self.state.slider_drag = None;
                FormControllerEvent::Ignored
            }

            UiEvent::Scroll { delta, .. } => {
                let vr = viewport_rows_lh(lh, rect);
                let rows = wheel_row_delta(delta.y, 1.0);
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
                self.delegate(|form, state| {
                    form_contract::handle_key(form, state, key, modifiers, vr)
                })
            }

            // A space is the one `CharTyped` character with activation
            // semantics (Enter/Space on a `ButtonRow`/`ToggleGroup`/
            // `Toolbar` item or a plain `Toggle` — see `handle_key`'s own
            // Enter/Space arms). Unlike a `KeyPressed` space, `CharTyped`
            // never reaches `handle_key` at all, so without this check a
            // backend that delivers Space as `CharTyped(' ')` rather than
            // `KeyPressed { key: Char(' '), .. }` would always fall
            // through to `handle_text_op` below, which is a no-op for
            // every one of those kinds (none has a text value) — the
            // field would just silently swallow the keystroke.
            // `activate_focused_on_space` returns `None` for every other
            // field kind, so a text field's own space character still
            // inserts normally.
            UiEvent::CharTyped(' ') if self.has_focus => self.delegate(|form, state| {
                form_contract::activate_focused_on_space(form, state).unwrap_or_else(|| {
                    form_contract::handle_text_op(form, state, EditOp::InsertChar(' '))
                })
            }),

            UiEvent::CharTyped(ch) if self.has_focus => self.delegate(|form, state| {
                form_contract::handle_text_op(form, state, EditOp::InsertChar(*ch))
            }),

            UiEvent::ClipboardPaste(text) if self.has_focus => self.delegate(|form, state| {
                form_contract::handle_text_op(form, state, EditOp::InsertText(text.clone()))
            }),

            _ => FormControllerEvent::Ignored,
        }
    }

    // ── Internal helpers ──────────────────────────────────────────────

    /// Build the per-render/per-hit-test `Form` by cloning the stored form
    /// and overlaying transient state (scroll, focus, in-progress edits,
    /// required-field markers). `theme` only affects the marker's color
    /// (via [`overlay_required_marker`]); pass `None` from a call site that
    /// has no backend handy (e.g. hit-testing) — the marker text itself is
    /// still appended, so label-width-derived geometry stays identical to
    /// a themed render.
    fn build_form(&self, _rect: Rect, theme: Option<&Theme>) -> Form {
        match &self.form {
            Some(f) => {
                let mut form = f.clone();
                form.has_focus = self.has_focus;
                form_contract::overlay_state(&mut form, &self.state);
                form.scroll_offset = self.scroll_offset;
                overlay_required_marker(&mut form, &self.rules, theme);
                form
            }
            None => Form {
                id: self.default_form_id(),
                fields: Vec::new(),
                focused_field: None,
                scroll_offset: self.scroll_offset,
                has_focus: self.has_focus,
                label_position: Default::default(),
                label_align: Default::default(),
                label_width: None,
                control_width: Default::default(),
                min_control_width: None,
                spacing: Default::default(),
            },
        }
    }

    // ── Keyboard: focus traversal + text editing ──────────────────────

    /// Resolve the focused item's on-screen rect (if any — see
    /// [`Self::current_item_focus`]) via the `item_bounds` of the form's
    /// [`crate::DrawsPrimitives::layout_primitive`] layout, the same per-item geometry
    /// [`crate::primitives::form::form_click_event`]'s caller already
    /// hit-tests against, and paint it through
    /// [`Backend::draw_focus_ring`] — see `primitives::form::contract`'s
    /// "Item-level keyboard focus" doc section for why painting goes through that
    /// shared chrome primitive instead of `primitives/form.rs`'s own
    /// painters. Called by [`Self::render`] right after `draw_form`; a
    /// no-op while `!self.has_focus` or nothing is item-focused.
    fn draw_item_focus_ring(&self, backend: &mut dyn Backend, form_rect: Rect, form: &Form) {
        if !self.has_focus {
            return;
        }
        let Some((focused_id, idx, items)) = form_contract::current_item_focus(form, &self.state)
        else {
            return;
        };
        let Some((item_id, _)) = items.get(idx).copied() else {
            return;
        };
        let layout =
            crate::backend::DrawsPrimitives::layout_primitive(&*backend, form_rect, form).layout;
        let Some(vf) = layout.visible_fields.iter().find(|vf| {
            form.fields
                .get(vf.field_idx)
                .is_some_and(|f| f.id == focused_id)
        }) else {
            return;
        };
        let Some((_, item_rect)) = vf.item_bounds.iter().find(|(id, _)| *id == item_id) else {
            return;
        };
        backend.draw_focus_ring(Rect::new(
            form_rect.x + item_rect.x,
            form_rect.y + item_rect.y,
            item_rect.width,
            item_rect.height,
        ));
    }

    fn click_inner(
        &mut self,
        rect: Rect,
        x: f32,
        y: f32,
        lh: f32,
        backend: Option<&mut dyn Backend>,
    ) -> FormControllerEvent {
        // Every fresh `MouseDown` starts at most one gesture, so clear
        // whatever slider drag a previous press left behind up front —
        // not just when this press lands on the form body, which the
        // widget's own `click_field` already does — so a dropped
        // `MouseUp` (the pointer leaving the window with the button
        // still held, common on GTK) can't leave a stale slider drag
        // that later hijacks an unrelated scrollbar-thumb drag. See
        // `handle_inner`'s `MouseMoved` arm, which checks `slider_drag`
        // before falling back to the scrollbar's own `drag_to`.
        self.state.slider_drag = None;
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
            // Color is irrelevant to hit-testing, so skip the backend
            // dependency here; the marker text itself is still appended
            // (see `build_form`'s doc), keeping label-width-derived
            // geometry identical to what a themed render would produce.
            let form = self.build_form(form_rect, None);
            let layout = if let Some(be) = backend {
                crate::backend::DrawsPrimitives::layout_primitive(&*be, form_rect, &form).layout
            } else {
                let row_h = row_height(lh);
                let char_w = lh * 0.6;
                form.layout(form_rect.width, form_rect.height, |i| {
                    form_field_measure(&form.fields[i], row_h, char_w)
                })
            };
            let local_x = x - form_rect.x;
            let local_y = y - form_rect.y;
            handled_to_form_controller_event(form_contract::click_field(
                &form,
                &mut self.state,
                &layout,
                local_x,
                local_y,
            ))
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

    /// Continues an in-flight `Slider` drag (armed by [`Self::click_inner`]
    /// — see `primitives::form::contract`'s
    /// "Slider fields" doc section) as the pointer
    /// moves to `x` within `rect` — the `MouseMoved` counterpart of
    /// [`Self::drag_to`]'s scrollbar-thumb drag. Unlike `drag_to`, which
    /// reuses geometry `ThumbDrag::begin` cached at press time, this
    /// rebuilds the form/layout fresh (same as [`Self::click_inner`]
    /// itself) rather than caching the track's rect, since a resize
    /// between the press and this move would otherwise drag against a
    /// stale rect. `FormControllerEvent::Ignored` if nothing is being
    /// dragged, or the new position doesn't move the clamped value.
    fn slider_drag_to(
        &mut self,
        rect: Rect,
        x: f32,
        lh: f32,
        backend: Option<&mut dyn Backend>,
    ) -> FormControllerEvent {
        if self.state.slider_drag.is_none() {
            return FormControllerEvent::Ignored;
        }
        let (form_rect, _sb_rect) = split_rect_lh(self.field_count(), lh, rect);
        let form = self.build_form(form_rect, None);
        let layout = if let Some(be) = backend {
            crate::backend::DrawsPrimitives::layout_primitive(&*be, form_rect, &form).layout
        } else {
            let row_h = row_height(lh);
            let char_w = lh * 0.6;
            form.layout(form_rect.width, form_rect.height, |i| {
                form_field_measure(&form.fields[i], row_h, char_w)
            })
        };
        let local_x = x - form_rect.x;
        handled_to_form_controller_event(form_contract::slider_drag_to(
            &form,
            &self.state,
            &layout,
            local_x,
        ))
    }
}

// ── Free functions (shared by FormController + SidebarSystem) ────────

// ── Keyboard editing helpers ──────────────────────────────────────────

/// Focusable field ids of `form`, in Tab-order — shared by
/// [`FormController::move_focus`] (which wraps within the form) and
/// [`crate::compose::form_dialog::FormDialogController`] (which needs to
/// detect stepping off either end *before* forwarding a Tab/Shift-Tab
/// keypress into the form, so it can hand focus to its own button row
/// instead — something `move_focus`'s own wrap-within-the-form traversal
/// can't signal). Delegates to [`crate::primitives::form::focus_order`],
/// the canonical implementation — also used by a `quadraui-py`/
/// `quadraui-node` host's `InteractionEngine` for the identical "detect
/// either end" check, to hand focus to the next app-level tab stop
/// instead of a dialog's buttons.
pub(crate) fn focus_order(form: &Form) -> Vec<WidgetId> {
    crate::primitives::form::focus_order(form)
}

/// Append a trailing `" *"` span to the label of every field that carries
/// a [`Validator::Required`] rule in `rules`, so the shared painter
/// (`primitives::form`) and the TUI renderer (`tui::form`) both show a
/// required-field marker without either needing its own notion of "which
/// fields are required" — they just render `label.spans` as given.
///
/// Inert when `rules.fields` is empty (the common, rules-less case): no
/// spans are ever appended, so a form with no `FormRules` renders exactly
/// as before this overlay existed.
fn overlay_required_marker(form: &mut Form, rules: &FormRules, theme: Option<&Theme>) {
    if rules.fields.is_empty() {
        return;
    }
    for field in &mut form.fields {
        let is_required = rules
            .fields
            .get(field.id.as_str())
            .is_some_and(|rules| rules.iter().any(|rule| rule.check == Validator::Required));
        if is_required {
            let marker = match theme {
                Some(t) => StyledSpan::with_fg(" *", t.error_fg),
                None => StyledSpan::plain(" *"),
            };
            field.label.spans.push(marker);
        }
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

/// Backend-free item layout for [`FormController::handle_cached`]'s
/// fallback path (used when no live [`Backend`] is available to hit-test
/// against, e.g. vimcode's TUI mouse handler). Approximates the pixel
/// `Form` layout (the pixel branch of `Form`'s `Primitive::layout`, via
/// `primitives::layout_metrics`) geometry proportionally to `char_w`/`row_h` — it is not cell-accurate
/// for the TUI grid's own `line_height() == 1.0` regime, where a
/// [`FieldKind::Toolbar`]'s pixel-unit button padding (`ACTION_H_PAD`,
/// `SEPARATOR_PX`) measures far wider than the cells `tui::toolbar`
/// actually paints.
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
                    id: t.id,
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
                        id: b.id,
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
        FieldKind::Toolbar(toolbar) => {
            use crate::primitives::toolbar::{measure_button, ToolbarButton};
            let measure = CharWidthMeasure(char_w);
            let label_w = field.label.visible_width() as f32 * char_w;
            // Mirrors the pixel backends' `items_start_x` (see
            // `primitives::layout_metrics::items_start_x`): a labelled
            // row reserves `label_w + 2 * char_w`, same as the sibling
            // arms above; an unlabelled toolbar still reserves the
            // backends' fixed 6px row inset — approximated here as
            // `char_w` (== 6.0 at this fallback's `lh = 10.0` convention),
            // the same approximation the sibling arms use for their own
            // no-label case.
            let start_x = if label_w > 0.0 {
                label_w + char_w * 2.0
            } else {
                char_w
            };
            let items = toolbar
                .buttons
                .iter()
                .map(|btn| {
                    let id = match btn {
                        ToolbarButton::Action { id, .. } => *id,
                        _ => field.id,
                    };
                    FormItemMeasure {
                        id,
                        width: measure_button(&measure, btn),
                    }
                })
                .collect();
            // item_gap = 0: buttons pack edge-to-edge, same as
            // `Toolbar::layout`'s own placement.
            FormFieldMeasure::with_items(row_h, start_x, 0.0, items)
        }
        FieldKind::TextArea { visible_rows, .. } => {
            FormFieldMeasure::new(row_h * *visible_rows as f32)
        }
        // See `click_inner`/`number_input_click_event`'s doc: the single
        // item (the field's own id, not a synthetic one) exists purely
        // so this fallback's `item_bounds` lookup finds the same control
        // rect a live backend's `form_layout` would.
        FieldKind::NumberInput(number) => {
            let label_w = field.label.visible_width() as f32 * char_w;
            let start_x = if label_w > 0.0 {
                label_w + char_w * 2.0
            } else {
                char_w
            };
            let text = match &number.draft {
                Some(draft) => draft.clone(),
                None => format!("{:.*}", number.precision as usize, number.value),
            };
            let width = (text.chars().count() as f32 + 5.0) * char_w;
            let items = vec![FormItemMeasure {
                id: field.id,
                width,
            }];
            FormFieldMeasure::with_items(row_h, start_x, 0.0, items)
        }
        // A single item (the field's own id) whose rect is the slider's
        // *track* alone, so this fallback's `item_bounds` lookup finds
        // the same track rect a live backend's `form_layout` would — see
        // `slider_click_event`'s doc. The track's natural
        // `SLIDER_TRACK_CELLS` sits one cell past the control column's
        // start, leaving that first cell for the `[` bracket the TUI
        // painter draws there.
        FieldKind::Slider { .. } => {
            let label_w = field.label.visible_width() as f32 * char_w;
            let start_x = if label_w > 0.0 {
                label_w + char_w * 2.0
            } else {
                char_w
            };
            let items = vec![FormItemMeasure {
                id: field.id,
                width: SLIDER_TRACK_CELLS * char_w,
            }];
            FormFieldMeasure::with_items(row_h, start_x + char_w, 0.0, items)
        }
        // One item per `RadioOption`, stacked vertically — see
        // `FormFieldMeasure::vertical`'s doc; the field's row is `row_h`
        // *per option*, not a single `row_h`.
        FieldKind::RadioGroup(group) => {
            let label_w = field.label.visible_width() as f32 * char_w;
            let start_x = if label_w > 0.0 {
                label_w + char_w * 2.0
            } else {
                char_w
            };
            let items: Vec<FormItemMeasure> = group
                .options
                .iter()
                .map(|o| FormItemMeasure {
                    id: o.id,
                    width: row_h,
                })
                .collect();
            FormFieldMeasure::with_vertical_items(
                row_h * group.options.len().max(1) as f32,
                start_x,
                items,
            )
        }
        _ => FormFieldMeasure::new(row_h),
    }
}

/// [`crate::primitives::layout_metrics::TextMeasure`] adapter used only by
/// [`form_field_measure`]'s backend-free fallback: approximates every
/// glyph as `char_w` wide (the same `lh * 0.6` estimate `click_inner`
/// already uses for the other row-item kinds) so it can call
/// [`crate::primitives::toolbar::measure_button`] — the same formula
/// every pixel backend's real `TextMeasure` feeds — without a live font.
struct CharWidthMeasure(f32);

impl crate::primitives::layout_metrics::TextMeasure for CharWidthMeasure {
    fn width_of(&self, text: &str) -> f32 {
        text.chars().count() as f32 * self.0
    }
}

#[cfg(test)]
// This whole module exercises `handle`/`handle_cached`/`FormControllerEvent`
// directly — deliberately, since those are now the deprecated shims and
// their own test coverage is what "stays passing" means for them. The
// `Handled<FormControllerAction>`-returning `handle_event`/
// `handle_cached_event` are thin wrappers around the same `handle_inner`
// these tests already exercise (see `handled_from_form_controller_event`).
#[allow(deprecated)]
mod tests {
    use super::*;
    use crate::event::{Key, NamedKey};
    use crate::primitives::combo_box::ComboBox;
    use crate::primitives::date_picker::CalendarDate;
    use crate::primitives::form::contract::slider_track_rect;
    use crate::primitives::form::{FieldKind, FormField, ToggleGroupItem};
    use crate::primitives::number_input::NumberInput;
    use crate::primitives::radio_group::{RadioGroup, RadioOption};
    use crate::types::{Modifiers, StyledText};

    fn make_fields(n: usize) -> Vec<FormField> {
        (0..n)
            .map(|i| FormField {
                id: WidgetId::new(format!("field-{i}")),
                label: StyledText::plain(format!("Field {i}")),
                kind: FieldKind::Toggle { value: false },
                hint: StyledText::default(),
                disabled: false,
                validation: None,
                label_position: None,
                label_align: None,
                label_width: None,
                tab_index: None,
                control_width: None,
                min_control_width: None,
                row_align: None,
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
            label_position: Default::default(),
            label_align: Default::default(),
            label_width: None,
            control_width: Default::default(),
            min_control_width: None,
            spacing: Default::default(),
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
    fn set_rules_validates_on_every_set_form_call() {
        use crate::validation::{Rule, Severity, Validator};

        let mut fc = FormController::new("fc".into());
        let mut rules = FormRules::default();
        rules.fields.insert(
            "field-0".to_string(),
            vec![Rule {
                check: Validator::Required,
                message: "Required".to_string(),
                severity: Severity::Error,
            }],
        );
        fc.set_rules(rules);
        assert_eq!(fc.rules().fields.len(), 1);

        // Empty text field fails `Required`.
        let mut form = make_form(1);
        form.fields[0].kind = FieldKind::TextInput {
            value: String::new(),
            placeholder: String::new(),
            cursor: None,
            selection_anchor: None,
        };
        fc.set_form(form);
        assert_eq!(
            fc.form().unwrap().fields[0].validation,
            Some(crate::primitives::form::ValidationState::Error(
                "Required".to_string()
            ))
        );

        // Filling it in and pushing the next frame clears the error with no
        // host-side validation call.
        let mut form = make_form(1);
        form.fields[0].kind = FieldKind::TextInput {
            value: "Ada".to_string(),
            placeholder: String::new(),
            cursor: Some(3),
            selection_anchor: None,
        };
        fc.set_form(form);
        assert_eq!(fc.form().unwrap().fields[0].validation, None);

        // A field with no declared rules is left untouched even when the
        // controller has other rules set.
        let mut form = make_form(2);
        form.fields[1].validation = Some(crate::primitives::form::ValidationState::Warning(
            "host-set".to_string(),
        ));
        fc.set_form(form);
        assert_eq!(
            fc.form().unwrap().fields[1].validation,
            Some(crate::primitives::form::ValidationState::Warning(
                "host-set".to_string()
            ))
        );
    }

    #[test]
    fn blurring_a_required_field_populates_validation_without_manual_validate_call() {
        use crate::validation::{Rule, Severity, Validator};

        fn name_email_form(name_value: &str, focused: &str) -> Form {
            Form {
                id: WidgetId::new("form"),
                fields: vec![
                    FormField {
                        id: WidgetId::new("name"),
                        label: StyledText::plain("Name"),
                        kind: FieldKind::TextInput {
                            value: name_value.to_string(),
                            placeholder: String::new(),
                            cursor: None,
                            selection_anchor: None,
                        },
                        hint: StyledText::default(),
                        disabled: false,
                        validation: None,
                        label_position: None,
                        label_align: None,
                        label_width: None,
                        tab_index: None,
                        control_width: None,
                        min_control_width: None,
                        row_align: None,
                    },
                    text_field("email", "", None),
                ],
                focused_field: Some(WidgetId::new(focused)),
                scroll_offset: 0,
                has_focus: true,
                label_position: Default::default(),
                label_align: Default::default(),
                label_width: None,
                control_width: Default::default(),
                min_control_width: None,
                spacing: Default::default(),
            }
        }

        let mut fc = FormController::new("f".into());
        fc.set_form(name_email_form("", "name"));
        fc.set_has_focus(true);
        fc.set_backend_info(1.0);
        let mut rules = FormRules::default();
        rules.fields.insert(
            "name".to_string(),
            vec![Rule {
                check: Validator::Required,
                message: "Required".to_string(),
                severity: Severity::Error,
            }],
        );
        fc.set_rules(rules);

        // Tab away from "name" — the controller reports the focus move; the
        // host persists it the same way the module doc's "Keyboard editing"
        // section describes, by feeding the new `focused_field` back on the
        // next `set_form` call. Nothing here calls `validate()` directly.
        let ev = fc.handle_cached(
            &key_event(Key::Named(NamedKey::Tab), Modifiers::default()),
            RECT,
        );
        assert_eq!(
            ev,
            FormControllerEvent::FormAction(FormEvent::FocusChanged {
                id: WidgetId::new("email")
            }),
            "tab should blur \"name\" and focus \"email\""
        );
        fc.set_form(name_email_form("", "email"));
        assert_eq!(
            fc.form().unwrap().fields[0].validation,
            Some(crate::primitives::form::ValidationState::Error(
                "Required".to_string()
            )),
            "blurring the empty required field should populate its \
             validation state without a manual validate() call"
        );

        // Filling the field in and pushing the next frame (still away from
        // "name") clears the error, again with no manual validate() call.
        fc.set_form(name_email_form("Ada", "email"));
        assert_eq!(
            fc.form().unwrap().fields[0].validation,
            None,
            "a now-passing field should have its validation state cleared"
        );

        // A controller with no `FormRules` set never shows a validation
        // state after the same focus/blur sequence — the inert path.
        let mut fc_no_rules = FormController::new("f2".into());
        fc_no_rules.set_form(name_email_form("", "name"));
        fc_no_rules.set_has_focus(true);
        fc_no_rules.set_backend_info(1.0);
        fc_no_rules.handle_cached(
            &key_event(Key::Named(NamedKey::Tab), Modifiers::default()),
            RECT,
        );
        fc_no_rules.set_form(name_email_form("", "email"));
        assert_eq!(
            fc_no_rules.form().unwrap().fields[0].validation,
            None,
            "a form with no FormRules shows no validation state after blur"
        );
    }

    #[test]
    fn set_visibility_applies_on_every_set_form_call() {
        use crate::expr::{Expr, Value};
        use crate::visibility::{FieldVisibility, FormVisibility};

        let mut fc = FormController::new("fc".into());
        let mut visibility = FormVisibility::default();
        visibility.fields.insert(
            "field-1".to_string(),
            FieldVisibility {
                visible_when: Some(Expr::Eq(
                    Box::new(Expr::Field("field-0".into())),
                    Box::new(Expr::Lit(Value::Bool(true))),
                )),
                enabled_when: None,
            },
        );
        fc.set_visibility(visibility);
        assert_eq!(fc.visibility().fields.len(), 1);

        // `field-0` toggled off hides `field-1` from the pushed form.
        let form = make_form(2);
        fc.set_form(form);
        assert_eq!(fc.field_count(), 1);

        // Flipping `field-0` back on and pushing the next frame restores
        // `field-1` with no host-side rebuild of the field list.
        let mut form = make_form(2);
        form.fields[0].kind = FieldKind::Toggle { value: true };
        fc.set_form(form);
        assert_eq!(fc.field_count(), 2);
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

    // ── field_rect ───────────────────────────────────────────────────
    //
    // Drives a real `TuiBackend` (not a hand-rolled mock — see
    // `context_menu_controller.rs`'s tests module doc for why): the one
    // thing `field_rect` needs from a `Backend` is `line_height` +
    // `form_layout`, both cheap and headless on `TuiBackend::new()`.

    #[cfg(feature = "tui")]
    mod field_rect_tests {
        use super::*;
        use crate::tui::TuiBackend;

        #[test]
        fn resolves_second_field_below_the_first() {
            let fc = test_controller(3);
            let backend = TuiBackend::new();
            let rect = Rect::new(0.0, 0.0, 40.0, 20.0);
            let r0 = fc
                .field_rect(WidgetId::new("field-0"), &backend, rect)
                .unwrap();
            let r1 = fc
                .field_rect(WidgetId::new("field-1"), &backend, rect)
                .unwrap();
            // Same column (and width) as the first field, strictly below
            // it — this is the whole point of the lookup: a multi-field
            // form's second field is *not* at the form's own rect.
            assert_eq!(r0.x, r1.x);
            assert_eq!(r0.width, r1.width);
            assert!(r1.y > r0.y);
        }

        #[test]
        fn offsets_by_the_passed_rect_origin() {
            let fc = test_controller(1);
            let backend = TuiBackend::new();
            let origin_rect = Rect::new(0.0, 0.0, 40.0, 20.0);
            let shifted_rect = Rect::new(5.0, 7.0, 40.0, 20.0);
            let at_origin = fc
                .field_rect(WidgetId::new("field-0"), &backend, origin_rect)
                .unwrap();
            let shifted = fc
                .field_rect(WidgetId::new("field-0"), &backend, shifted_rect)
                .unwrap();
            assert_eq!(shifted.x, at_origin.x + 5.0);
            assert_eq!(shifted.y, at_origin.y + 7.0);
        }

        #[test]
        fn unknown_field_id_returns_none() {
            let fc = test_controller(3);
            let backend = TuiBackend::new();
            let rect = Rect::new(0.0, 0.0, 40.0, 20.0);
            assert_eq!(
                fc.field_rect(WidgetId::new("no-such-field"), &backend, rect),
                None
            );
        }

        // The rect handed back has to be the *same* rect the controller's
        // own hit-testing resolves to that field — a host that anchors a
        // popup to it and then routes the user's next click back through
        // `handle` depends on the two agreeing. Click the centre of every
        // reported rect and assert the click lands on that field rather
        // than a neighbour.
        #[test]
        fn reported_rect_round_trips_through_hit_testing() {
            let rect = Rect::new(3.0, 2.0, 40.0, 20.0);
            let mut backend = TuiBackend::new();
            for i in 0..3 {
                let id = WidgetId::new(format!("field-{i}"));
                let mut fc = test_controller(3);
                let field = fc.field_rect(id, &backend, rect).unwrap();
                let ev = fc.handle(
                    &UiEvent::MouseDown {
                        widget: None,
                        button: MouseButton::Left,
                        position: crate::Point {
                            x: field.x + field.width / 2.0,
                            y: field.y + field.height / 2.0,
                        },
                        modifiers: Default::default(),
                    },
                    &mut backend,
                    rect,
                );
                match ev {
                    FormControllerEvent::FormAction(FormEvent::ToggleChanged {
                        id: hit, ..
                    }) => {
                        assert_eq!(hit, id, "field-{i}'s reported rect hit-tested elsewhere");
                    }
                    other => panic!("expected ToggleChanged for field-{i}, got {other:?}"),
                }
            }
        }

        #[test]
        fn empty_controller_returns_none() {
            let fc = FormController::new("fc".into());
            let backend = TuiBackend::new();
            let rect = Rect::new(0.0, 0.0, 40.0, 20.0);
            assert_eq!(
                fc.field_rect(WidgetId::new("field-0"), &backend, rect),
                None
            );
        }

        #[test]
        fn scrolled_out_of_view_field_returns_none() {
            let mut fc = test_controller(20);
            // One row per field at `TuiBackend`'s line height leaves a
            // 20.0-tall viewport room for only a handful of rows — scroll
            // far enough that field-0 is no longer among the visible ones.
            fc.set_scroll_offset(15);
            let backend = TuiBackend::new();
            let rect = Rect::new(0.0, 0.0, 40.0, 20.0);
            assert_eq!(
                fc.field_rect(WidgetId::new("field-0"), &backend, rect),
                None
            );
        }
    }

    // ── build_form injects controller state ──────────────────────────

    #[test]
    fn build_form_injects_scroll_offset() {
        let mut fc = test_controller(10);
        fc.set_scroll_offset(3);
        fc.set_has_focus(true);
        let form = fc.build_form(Rect::new(0.0, 0.0, 40.0, 100.0), None);
        assert_eq!(form.scroll_offset, 3);
        assert!(form.has_focus);
    }

    #[test]
    fn build_form_with_no_data_returns_empty() {
        let fc = FormController::new("empty".into());
        let form = fc.build_form(Rect::new(0.0, 0.0, 40.0, 100.0), None);
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
                    spec: None,
                    id: WidgetId::new("reset"),
                    label: "Reset".into(),
                    icon: None,
                    key_hint: None,
                    enabled: true,
                    is_active: false,
                    tooltip: String::new(),
                },
                ToolbarButton::Action {
                    spec: None,
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
            ..Default::default()
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
                spec: None,
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
            ..Default::default()
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
    fn handle_cached_scroll_consumes_the_delta_magnitude_not_just_its_sign() {
        let mut fc = test_controller(20);
        fc.set_backend_info(1.0);
        let ev = fc.handle_cached(
            &UiEvent::Scroll {
                widget: None,
                delta: crate::ScrollDelta { x: 0.0, y: -3.0 },
                position: crate::Point { x: 5.0, y: 2.0 },
            },
            Rect::new(0.0, 0.0, 40.0, 5.0),
        );
        assert_eq!(ev, FormControllerEvent::Consumed);
        assert_eq!(fc.scroll_offset(), 3);
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

    #[test]
    fn handle_cached_click_on_second_toolbar_button_resolves_that_button() {
        use crate::primitives::toolbar::{Toolbar, ToolbarButton};

        let mut fc = test_controller(1);
        fc.set_form({
            let mut form = make_form(1);
            form.fields[0].label = StyledText::default();
            form.fields[0].kind = FieldKind::Toolbar(Toolbar {
                id: WidgetId::new("tb"),
                buttons: vec![
                    ToolbarButton::Action {
                        spec: None,
                        id: WidgetId::new("btn-a"),
                        label: "A".into(),
                        icon: None,
                        key_hint: None,
                        enabled: true,
                        is_active: false,
                        tooltip: String::new(),
                    },
                    ToolbarButton::Action {
                        spec: None,
                        id: WidgetId::new("btn-b"),
                        label: "BB".into(),
                        icon: None,
                        key_hint: None,
                        enabled: true,
                        is_active: false,
                        tooltip: String::new(),
                    },
                ],
                bg: None,
                focused_index: None,
                ..Default::default()
            });
            form
        });
        // lh=10.0 → row_h=14.0, char_w=6.0. An unlabelled toolbar still
        // reserves `char_w` (6.0) for the row inset, so items_start_x =
        // 6.0, not 0.0 (see `form_field_measure`'s `FieldKind::Toolbar`
        // arm): "A" button is 1*6.0 + 2*8.0 = 22.0 wide, spanning x in
        // [6, 28); the "BB" button follows immediately (no item gap),
        // spanning x in [28, 56). x=30 lands inside the second button.
        fc.set_backend_info(10.0);

        // A click left of items_start_x must miss both buttons and fall
        // through to the row's own hit region (FocusChanged on the
        // field), pinning that items_start_x is 6.0 and not 0.0 -
        // the pixel `Form` layout's row inset, not
        // tui_form_layout's flush-left TUI cell convention.
        let miss = fc.handle_cached(
            &UiEvent::MouseDown {
                widget: None,
                button: MouseButton::Left,
                position: crate::Point { x: 3.0, y: 5.0 },
                modifiers: Default::default(),
            },
            Rect::new(0.0, 0.0, 200.0, 14.0),
        );
        assert_eq!(
            miss,
            FormControllerEvent::FormAction(FormEvent::FocusChanged {
                id: WidgetId::new("field-0"),
            }),
            "a click left of items_start_x must miss both buttons"
        );

        let ev = fc.handle_cached(
            &UiEvent::MouseDown {
                widget: None,
                button: MouseButton::Left,
                position: crate::Point { x: 30.0, y: 5.0 },
                modifiers: Default::default(),
            },
            Rect::new(0.0, 0.0, 200.0, 14.0),
        );
        assert_eq!(
            ev,
            FormControllerEvent::FormAction(FormEvent::ToolbarButtonClicked {
                field_id: WidgetId::new("field-0"),
                button_id: WidgetId::new("btn-b"),
            }),
            "click on the second toolbar button must resolve to its own id, \
             not fall back to the field's generic FocusChanged"
        );
    }

    #[test]
    fn handle_cached_click_on_toolbar_separator_falls_back_to_field_focus() {
        use crate::primitives::toolbar::{Toolbar, ToolbarButton};

        let mut fc = test_controller(1);
        fc.set_form({
            let mut form = make_form(1);
            form.fields[0].label = StyledText::default();
            form.fields[0].kind = FieldKind::Toolbar(Toolbar {
                id: WidgetId::new("tb"),
                buttons: vec![
                    ToolbarButton::Action {
                        spec: None,
                        id: WidgetId::new("btn-a"),
                        label: "A".into(),
                        icon: None,
                        key_hint: None,
                        enabled: true,
                        is_active: false,
                        tooltip: String::new(),
                    },
                    ToolbarButton::Separator,
                    ToolbarButton::Action {
                        spec: None,
                        id: WidgetId::new("btn-b"),
                        label: "B".into(),
                        icon: None,
                        key_hint: None,
                        enabled: true,
                        is_active: false,
                        tooltip: String::new(),
                    },
                ],
                bg: None,
                focused_index: None,
                ..Default::default()
            });
            form
        });
        // lh=10.0 → row_h=14.0, char_w=6.0, items_start_x=6.0. "A" is
        // 1*6.0 + 2*8.0 = 22.0 wide, spanning [6, 28); the separator is a
        // fixed SEPARATOR_PX=12.0 regardless of char_w, spanning [28, 40);
        // "B" follows at [40, 62). A `Separator` has no id of its own, so
        // `form_field_measure` falls back to `field.id` for its item - a
        // click inside the separator's zone must resolve the same way a
        // click outside every item does: FocusChanged on the field, not a
        // ToolbarButtonClicked for either neighbouring button.
        fc.set_backend_info(10.0);

        let ev = fc.handle_cached(
            &UiEvent::MouseDown {
                widget: None,
                button: MouseButton::Left,
                position: crate::Point { x: 34.0, y: 5.0 },
                modifiers: Default::default(),
            },
            Rect::new(0.0, 0.0, 200.0, 14.0),
        );
        assert_eq!(
            ev,
            FormControllerEvent::FormAction(FormEvent::FocusChanged {
                id: WidgetId::new("field-0"),
            }),
            "a click on a separator's hit-zone must fall back to the field's \
             generic FocusChanged, not resolve to either neighbouring button"
        );
    }

    #[test]
    fn handle_cached_toolbar_items_start_x_accounts_for_the_label() {
        use crate::primitives::toolbar::{Toolbar, ToolbarButton};

        let mut fc = test_controller(1);
        fc.set_form({
            let mut form = make_form(1);
            form.fields[0].label = StyledText::plain("Lbl"); // 3 chars
            form.fields[0].kind = FieldKind::Toolbar(Toolbar {
                id: WidgetId::new("tb"),
                buttons: vec![ToolbarButton::Action {
                    spec: None,
                    id: WidgetId::new("btn-a"),
                    label: "A".into(),
                    icon: None,
                    key_hint: None,
                    enabled: true,
                    is_active: false,
                    tooltip: String::new(),
                }],
                bg: None,
                focused_index: None,
                ..Default::default()
            });
            form
        });
        // lh=10.0 → row_h=14.0, char_w=6.0. A labelled toolbar row uses the
        // same `label_w + 2 * char_w` reservation as every sibling arm:
        // label_w = 3*6.0 = 18.0, so items_start_x = 18.0 + 12.0 = 30.0,
        // and "A" (1*6.0 + 2*8.0 = 22.0 wide) spans [30, 52).
        fc.set_backend_info(10.0);

        let miss = fc.handle_cached(
            &UiEvent::MouseDown {
                widget: None,
                button: MouseButton::Left,
                position: crate::Point { x: 20.0, y: 5.0 },
                modifiers: Default::default(),
            },
            Rect::new(0.0, 0.0, 200.0, 14.0),
        );
        assert_eq!(
            miss,
            FormControllerEvent::FormAction(FormEvent::FocusChanged {
                id: WidgetId::new("field-0"),
            }),
            "a click inside the label's reserved space must miss the button"
        );

        let hit = fc.handle_cached(
            &UiEvent::MouseDown {
                widget: None,
                button: MouseButton::Left,
                position: crate::Point { x: 40.0, y: 5.0 },
                modifiers: Default::default(),
            },
            Rect::new(0.0, 0.0, 200.0, 14.0),
        );
        assert_eq!(
            hit,
            FormControllerEvent::FormAction(FormEvent::ToolbarButtonClicked {
                field_id: WidgetId::new("field-0"),
                button_id: WidgetId::new("btn-a"),
            }),
            "a click past the label's reserved space must resolve to the button"
        );
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
            label_position: None,
            label_align: None,
            label_width: None,
            tab_index: None,
            control_width: None,
            min_control_width: None,
            row_align: None,
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
            label_position: None,
            label_align: None,
            label_width: None,
            tab_index: None,
            control_width: None,
            min_control_width: None,
            row_align: None,
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
            label_position: None,
            label_align: None,
            label_width: None,
            tab_index: None,
            control_width: None,
            min_control_width: None,
            row_align: None,
        }
    }

    fn date_field(id: &str, focused: CalendarDate, disabled: bool) -> FormField {
        FormField {
            id: WidgetId::new(id),
            label: StyledText::plain("Date"),
            kind: FieldKind::DatePicker(crate::primitives::date_picker::DatePicker::new(
                WidgetId::new(id),
                focused,
            )),
            hint: StyledText::default(),
            disabled,
            validation: None,
            label_position: None,
            label_align: None,
            label_width: None,
            tab_index: None,
            control_width: None,
            min_control_width: None,
            row_align: None,
        }
    }

    fn combo_field(
        id: &str,
        options: &[&str],
        text: &str,
        open: bool,
        disabled: bool,
    ) -> FormField {
        combo_field_with_ids(id, id, options, text, open, disabled)
    }

    /// Like [`combo_field`], but lets the `FormField`'s id and the
    /// embedded `ComboBox`'s own id differ, so a test can assert which
    /// one a `FormEvent::ComboBox*` variant actually carries.
    fn combo_field_with_ids(
        field_id: &str,
        combo_id: &str,
        options: &[&str],
        text: &str,
        open: bool,
        disabled: bool,
    ) -> FormField {
        let mut combo = ComboBox::new(
            WidgetId::new(combo_id),
            options.iter().map(|s| s.to_string()).collect(),
        );
        combo.text = text.to_string();
        combo.open = open;
        FormField {
            id: WidgetId::new(field_id),
            label: StyledText::plain("Combo"),
            kind: FieldKind::ComboBox(combo),
            hint: StyledText::default(),
            disabled,
            validation: None,
            label_position: None,
            label_align: None,
            label_width: None,
            tab_index: None,
            control_width: None,
            min_control_width: None,
            row_align: None,
        }
    }

    fn number_field(id: &str, number: NumberInput, disabled: bool) -> FormField {
        FormField {
            id: WidgetId::new(id),
            label: StyledText::default(),
            kind: FieldKind::NumberInput(number),
            hint: StyledText::default(),
            disabled,
            validation: None,
            label_position: None,
            label_align: None,
            label_width: None,
            tab_index: None,
            control_width: None,
            min_control_width: None,
            row_align: None,
        }
    }

    fn slider_field(
        id: &str,
        value: f32,
        min: f32,
        max: f32,
        step: f32,
        disabled: bool,
    ) -> FormField {
        FormField {
            id: WidgetId::new(id),
            label: StyledText::default(),
            kind: FieldKind::Slider {
                value,
                min,
                max,
                step,
            },
            hint: StyledText::default(),
            disabled,
            validation: None,
            label_position: None,
            label_align: None,
            label_width: None,
            tab_index: None,
            control_width: None,
            min_control_width: None,
            row_align: None,
        }
    }

    fn radio_group_field(id: &str, group: RadioGroup, disabled: bool) -> FormField {
        FormField {
            id: WidgetId::new(id),
            label: StyledText::default(),
            kind: FieldKind::RadioGroup(group),
            hint: StyledText::default(),
            disabled,
            validation: None,
            label_position: None,
            label_align: None,
            label_width: None,
            tab_index: None,
            control_width: None,
            min_control_width: None,
            row_align: None,
        }
    }

    fn dropdown_field(
        id: &str,
        options: &[&str],
        selected_idx: usize,
        disabled: bool,
    ) -> FormField {
        FormField {
            id: WidgetId::new(id),
            label: StyledText::plain("Dropdown"),
            kind: FieldKind::Dropdown {
                options: options.iter().map(|s| StyledText::plain(*s)).collect(),
                selected_idx,
            },
            hint: StyledText::default(),
            disabled,
            validation: None,
            label_position: None,
            label_align: None,
            label_width: None,
            tab_index: None,
            control_width: None,
            min_control_width: None,
            row_align: None,
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
            label_position: Default::default(),
            label_align: Default::default(),
            label_width: None,
            control_width: Default::default(),
            min_control_width: None,
            spacing: Default::default(),
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
    fn tab_follows_explicit_tab_index_over_declaration_order() {
        // Declared "a", "b", "c" but `tab_index` reorders the visit to
        // "c", "a", "b" — tabbing from "c" must land on "a", not "b".
        let mut a = text_field("a", "", None);
        a.tab_index = Some(1);
        let mut b = text_field("b", "", None);
        b.tab_index = Some(2);
        let mut c = text_field("c", "", None);
        c.tab_index = Some(0);
        let mut fc = controller_with(vec![a, b, c], Some("c"));
        let ev = fc.handle_cached(
            &key_event(Key::Named(NamedKey::Tab), Modifiers::default()),
            RECT,
        );
        assert_eq!(
            ev,
            FormControllerEvent::FormAction(FormEvent::FocusChanged {
                id: WidgetId::new("a")
            }),
            "tab_index should override declaration order"
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
            label_position: Default::default(),
            label_align: Default::default(),
            label_width: None,
            control_width: Default::default(),
            min_control_width: None,
            spacing: Default::default(),
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
                label_position: None,
                label_align: None,
                label_width: None,
                tab_index: None,
                control_width: None,
                min_control_width: None,
                row_align: None,
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
        // toggle flip) but — per `primitives::form::contract`'s "Keyboard editing" doc
        // section — never has to track `cursor` itself, so it stays
        // `None` here.
        fc.set_form(Form {
            id: WidgetId::new("form"),
            fields: vec![text_field("name", "abc", None)],
            focused_field: Some(WidgetId::new("name")),
            scroll_offset: 0,
            has_focus: true,
            label_position: Default::default(),
            label_align: Default::default(),
            label_width: None,
            control_width: Default::default(),
            min_control_width: None,
            spacing: Default::default(),
        });

        let rendered = fc.build_form(RECT, None);
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
            label_position: None,
            label_align: None,
            label_width: None,
            tab_index: None,
            control_width: None,
            min_control_width: None,
            row_align: None,
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
        // `modifiers.ctrl == true` (see `primitives::form::contract`'s
        // "Keyboard editing" doc section) — without the guard in `handle_key`, this would
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
            label_position: None,
            label_align: None,
            label_width: None,
            tab_index: None,
            control_width: None,
            min_control_width: None,
            row_align: None,
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

    // ── DatePicker routing ──────────────────────────────────────────

    #[test]
    fn right_arrow_moves_date_picker_focus_and_is_consumed_not_a_form_action() {
        let start = CalendarDate::new(2024, 3, 15);
        let mut fc = controller_with(vec![date_field("d", start, false)], Some("d"));
        let ev = fc.handle_cached(
            &key_event(Key::Named(NamedKey::Right), Modifiers::default()),
            RECT,
        );
        assert_eq!(
            ev,
            FormControllerEvent::Consumed,
            "pure focus-day movement changes no persisted value, so it's \
             Consumed not a FormEvent, mirroring text cursor movement"
        );
        let form = fc.build_form(RECT, None);
        match &form.fields[0].kind {
            FieldKind::DatePicker(picker) => {
                assert_eq!(picker.focused, start.add_days(1));
            }
            other => panic!("expected DatePicker, got {other:?}"),
        }
    }

    #[test]
    fn cached_focused_date_persists_across_keystrokes_even_if_app_never_echoes_it_back() {
        let start = CalendarDate::new(2024, 3, 15);
        let mut fc = controller_with(vec![date_field("d", start, false)], Some("d"));
        fc.handle_cached(
            &key_event(Key::Named(NamedKey::Right), Modifiers::default()),
            RECT,
        );

        // The app never tracks `focused` itself (per `DateEditState`'s
        // doc) — it stays at `start` here, exactly as the field was
        // first pushed.
        fc.set_form(Form {
            id: WidgetId::new("form"),
            fields: vec![date_field("d", start, false)],
            focused_field: Some(WidgetId::new("d")),
            scroll_offset: 0,
            has_focus: true,
            label_position: Default::default(),
            label_align: Default::default(),
            label_width: None,
            control_width: Default::default(),
            min_control_width: None,
            spacing: Default::default(),
        });

        let ev2 = fc.handle_cached(
            &key_event(Key::Named(NamedKey::Right), Modifiers::default()),
            RECT,
        );
        assert_eq!(ev2, FormControllerEvent::Consumed);
        let form = fc.build_form(RECT, None);
        match &form.fields[0].kind {
            FieldKind::DatePicker(picker) => assert_eq!(
                picker.focused,
                start.add_days(2),
                "the controller's cached focused day should carry over, \
                 landing two days forward rather than re-deriving a stale position"
            ),
            other => panic!("expected DatePicker, got {other:?}"),
        }
    }

    #[test]
    fn enter_confirms_the_cached_focused_date_and_clears_the_cache() {
        let start = CalendarDate::new(2024, 3, 15);
        let mut fc = controller_with(vec![date_field("d", start, false)], Some("d"));
        fc.handle_cached(
            &key_event(Key::Named(NamedKey::Right), Modifiers::default()),
            RECT,
        );
        let ev = fc.handle_cached(
            &key_event(Key::Named(NamedKey::Enter), Modifiers::default()),
            RECT,
        );
        assert_eq!(
            ev,
            FormControllerEvent::FormAction(FormEvent::DateChanged {
                id: WidgetId::new("d"),
                value: start.add_days(1),
            })
        );

        // The cache clears on confirm, so re-rendering the app's
        // untouched field (still `start`) shows `start` again, not the
        // confirmed day — the app is expected to push the new value
        // back via `set_form` itself.
        let form = fc.build_form(RECT, None);
        match &form.fields[0].kind {
            FieldKind::DatePicker(picker) => assert_eq!(picker.focused, start),
            other => panic!("expected DatePicker, got {other:?}"),
        }
    }

    #[test]
    fn disabled_date_picker_field_ignores_navigation_keys() {
        let start = CalendarDate::new(2024, 3, 15);
        let mut fc = controller_with(vec![date_field("d", start, true)], Some("d"));
        let ev = fc.handle_cached(
            &key_event(Key::Named(NamedKey::Right), Modifiers::default()),
            RECT,
        );
        assert_eq!(ev, FormControllerEvent::Ignored);
    }

    #[test]
    fn tab_moves_focus_away_from_a_focused_date_picker_field_normally() {
        let start = CalendarDate::new(2024, 3, 15);
        let mut fc = controller_with(
            vec![date_field("d", start, false), toggle_field("t", false)],
            Some("d"),
        );
        let ev = fc.handle_cached(
            &key_event(Key::Named(NamedKey::Tab), Modifiers::default()),
            RECT,
        );
        assert_eq!(
            ev,
            FormControllerEvent::FormAction(FormEvent::FocusChanged {
                id: WidgetId::new("t")
            }),
            "Tab isn't one of DatePicker's claimed keys, so focus traversal \
             off a focused DatePicker field still works"
        );
    }

    #[test]
    fn date_edit_cache_resets_when_focus_moves_to_a_different_date_picker_field() {
        let d1_start = CalendarDate::new(2024, 3, 15);
        let d2_start = CalendarDate::new(2024, 6, 1);
        let mut fc = controller_with(
            vec![
                date_field("d1", d1_start, false),
                date_field("d2", d2_start, false),
            ],
            Some("d1"),
        );
        fc.handle_cached(
            &key_event(Key::Named(NamedKey::Right), Modifiers::default()),
            RECT,
        );
        fc.handle_cached(
            &key_event(Key::Named(NamedKey::Tab), Modifiers::default()),
            RECT,
        );

        // `move_focus` only emits `FocusChanged` — per this module's
        // "Keyboard editing" doc section, the app persists the new
        // `focused_field` itself, same as it would for a toggle flip.
        fc.set_form(Form {
            id: WidgetId::new("form"),
            fields: vec![
                date_field("d1", d1_start, false),
                date_field("d2", d2_start, false),
            ],
            focused_field: Some(WidgetId::new("d2")),
            scroll_offset: 0,
            has_focus: true,
            label_position: Default::default(),
            label_align: Default::default(),
            label_width: None,
            control_width: Default::default(),
            min_control_width: None,
            spacing: Default::default(),
        });

        let ev = fc.handle_cached(
            &key_event(Key::Named(NamedKey::Right), Modifiers::default()),
            RECT,
        );
        assert_eq!(ev, FormControllerEvent::Consumed);
        let form = fc.build_form(RECT, None);
        match &form.fields[1].kind {
            FieldKind::DatePicker(picker) => assert_eq!(
                picker.focused,
                d2_start.add_days(1),
                "moving focus to d2 should seed its own focused day, not \
                 carry over d1's cached scrubbing position"
            ),
            other => panic!("expected DatePicker, got {other:?}"),
        }
    }

    #[test]
    fn build_form_shows_live_scrub_position_over_a_confirmed_selected_date() {
        let start = CalendarDate::new(2024, 3, 15);
        let field_with_selected = |selected: CalendarDate| {
            let mut f = date_field("d", start, false);
            if let FieldKind::DatePicker(picker) = &mut f.kind {
                picker.selected = Some(selected);
            }
            f
        };
        let mut fc = controller_with(vec![field_with_selected(start)], Some("d"));

        // Scrub past the already-confirmed date. The renderer (see the
        // "Date picker fields" doc section above) picks `focused` over
        // `selected` while the row is focused, so this must advance
        // `focused` without touching `selected`.
        let ev = fc.handle_cached(
            &key_event(Key::Named(NamedKey::Right), Modifiers::default()),
            RECT,
        );
        assert_eq!(ev, FormControllerEvent::Consumed);

        let form = fc.build_form(RECT, None);
        match &form.fields[0].kind {
            FieldKind::DatePicker(picker) => {
                assert_eq!(
                    picker.selected,
                    Some(start),
                    "scrubbing must not touch the app's confirmed value"
                );
                assert_eq!(
                    picker.focused,
                    start.add_days(1),
                    "the live scrub position must be exposed via `focused` \
                     even once a date has already been confirmed, so the \
                     renderer doesn't show a frozen `selected` instead"
                );
            }
            other => panic!("expected DatePicker, got {other:?}"),
        }
    }

    #[test]
    fn ctrl_modified_date_picker_key_falls_through_instead_of_navigating() {
        let start = CalendarDate::new(2024, 3, 15);
        let mut fc = controller_with(vec![date_field("d", start, false)], Some("d"));
        let ev = fc.handle_cached(
            &key_event(
                Key::Named(NamedKey::Right),
                Modifiers {
                    ctrl: true,
                    ..Modifiers::default()
                },
            ),
            RECT,
        );
        assert_eq!(
            ev,
            FormControllerEvent::Ignored,
            "Ctrl+Right is a host accelerator candidate, not calendar \
             navigation, so it must not be swallowed by the DatePicker"
        );
        let form = fc.build_form(RECT, None);
        match &form.fields[0].kind {
            FieldKind::DatePicker(picker) => {
                assert_eq!(picker.focused, start, "the calendar must not have moved")
            }
            other => panic!("expected DatePicker, got {other:?}"),
        }
    }

    #[test]
    fn build_form_does_not_overlay_a_stale_cached_scrub_onto_an_unfocused_date_picker() {
        let d1_start = CalendarDate::new(2024, 3, 15);
        let d2_start = CalendarDate::new(2024, 6, 1);
        let mut fc = controller_with(
            vec![
                date_field("d1", d1_start, false),
                date_field("d2", d2_start, false),
            ],
            Some("d1"),
        );
        fc.handle_cached(
            &key_event(Key::Named(NamedKey::Right), Modifiers::default()),
            RECT,
        );

        // Move focus to "d2" by a click rather than Tab — unlike
        // `move_focus`, `click_inner`'s path to `FormEvent::FocusChanged`
        // doesn't reseed/clear the cached scrub on "d1", so this is the
        // scenario where a stale `date_edit` could otherwise leak.
        fc.set_form(Form {
            id: WidgetId::new("form"),
            fields: vec![
                date_field("d1", d1_start, false),
                date_field("d2", d2_start, false),
            ],
            focused_field: Some(WidgetId::new("d2")),
            scroll_offset: 0,
            has_focus: true,
            label_position: Default::default(),
            label_align: Default::default(),
            label_width: None,
            control_width: Default::default(),
            min_control_width: None,
            spacing: Default::default(),
        });

        let form = fc.build_form(RECT, None);
        match &form.fields[0].kind {
            FieldKind::DatePicker(picker) => assert_eq!(
                picker.focused, d1_start,
                "the stale cached scrub for \"d1\" must not be overlaid \
                 once focus has moved to \"d2\""
            ),
            other => panic!("expected DatePicker, got {other:?}"),
        }
    }

    // ── ComboBox routing ──────────────────────────────────────────

    #[test]
    fn typed_char_on_a_focused_combo_box_updates_its_text() {
        let mut fc = controller_with(
            vec![combo_field(
                "c",
                &["Apple", "Banana", "Cherry"],
                "",
                false,
                false,
            )],
            Some("c"),
        );
        let ev = fc.handle_cached(&key_event(Key::Char('b'), Modifiers::default()), RECT);
        assert_eq!(
            ev,
            FormControllerEvent::FormAction(FormEvent::ComboBoxTextChanged {
                id: WidgetId::new("c"),
                value: "b".to_string(),
            })
        );
    }

    #[test]
    fn down_arrow_moves_combo_box_highlight() {
        let mut fc = controller_with(
            vec![combo_field(
                "c",
                &["Apple", "Banana", "Cherry"],
                "",
                true,
                false,
            )],
            Some("c"),
        );
        let ev = fc.handle_cached(
            &key_event(Key::Named(NamedKey::Down), Modifiers::default()),
            RECT,
        );
        assert_eq!(
            ev,
            FormControllerEvent::FormAction(FormEvent::ComboBoxHighlightChanged {
                id: WidgetId::new("c"),
                index: 0,
            })
        );
    }

    #[test]
    fn down_arrow_over_an_empty_filtered_list_falls_through_instead_of_navigating() {
        // "xyz" matches none of these options, so `move_highlight` returns
        // `None` and `handle_combo_box_key` must fall through to the
        // generic dispatch (which has nothing else to do with a bare Down
        // either, so this resolves as `Ignored`) rather than returning
        // some synthetic event.
        let mut fc = controller_with(
            vec![combo_field("c", &["Apple", "Banana"], "xyz", true, false)],
            Some("c"),
        );
        let ev = fc.handle_cached(
            &key_event(Key::Named(NamedKey::Down), Modifiers::default()),
            RECT,
        );
        assert_eq!(ev, FormControllerEvent::Ignored);
    }

    #[test]
    fn enter_commits_the_focused_combo_box_text() {
        let mut fc = controller_with(
            vec![combo_field(
                "c",
                &["Apple", "Banana", "Cherry"],
                "Ban",
                false,
                false,
            )],
            Some("c"),
        );
        let ev = fc.handle_cached(
            &key_event(Key::Named(NamedKey::Enter), Modifiers::default()),
            RECT,
        );
        assert_eq!(
            ev,
            FormControllerEvent::FormAction(FormEvent::ComboBoxCommitted {
                id: WidgetId::new("c"),
                value: "Ban".to_string(),
            })
        );
    }

    #[test]
    fn escape_while_open_cancels_the_combo_box() {
        let mut fc = controller_with(
            vec![combo_field("c", &["Apple", "Banana"], "a", true, false)],
            Some("c"),
        );
        let ev = fc.handle_cached(
            &key_event(Key::Named(NamedKey::Escape), Modifiers::default()),
            RECT,
        );
        assert_eq!(
            ev,
            FormControllerEvent::FormAction(FormEvent::ComboBoxCancelled {
                id: WidgetId::new("c"),
            })
        );
    }

    #[test]
    fn escape_while_closed_falls_through_instead_of_cancelling() {
        let mut fc = controller_with(
            vec![combo_field("c", &["Apple", "Banana"], "a", false, false)],
            Some("c"),
        );
        let ev = fc.handle_cached(
            &key_event(Key::Named(NamedKey::Escape), Modifiers::default()),
            RECT,
        );
        assert_eq!(ev, FormControllerEvent::Ignored);
    }

    #[test]
    fn disabled_combo_box_field_ignores_its_keys() {
        let mut fc = controller_with(
            vec![combo_field("c", &["Apple", "Banana"], "", false, true)],
            Some("c"),
        );
        let ev = fc.handle_cached(&key_event(Key::Char('a'), Modifiers::default()), RECT);
        assert_eq!(ev, FormControllerEvent::Ignored);
    }

    #[test]
    fn tab_moves_focus_away_from_a_focused_combo_box_field_normally() {
        let mut fc = controller_with(
            vec![
                combo_field("c", &["Apple", "Banana"], "", false, false),
                toggle_field("t", false),
            ],
            Some("c"),
        );
        let ev = fc.handle_cached(
            &key_event(Key::Named(NamedKey::Tab), Modifiers::default()),
            RECT,
        );
        assert_eq!(
            ev,
            FormControllerEvent::FormAction(FormEvent::FocusChanged {
                id: WidgetId::new("t")
            }),
            "Tab isn't one of ComboBox's claimed keys, so focus traversal \
             off a focused ComboBox field still works"
        );
    }

    #[test]
    fn ctrl_modified_combo_box_key_falls_through_instead_of_editing() {
        let mut fc = controller_with(
            vec![combo_field("c", &["Apple", "Banana"], "ab", false, false)],
            Some("c"),
        );
        let ev = fc.handle_cached(&key_event(Key::Char('a'), ctrl()), RECT);
        assert_eq!(
            ev,
            FormControllerEvent::Ignored,
            "Ctrl+A is a host accelerator candidate, not combo-box text \
             entry, so it must not be swallowed by the ComboBox"
        );
        let form = fc.build_form(RECT, None);
        match &form.fields[0].kind {
            FieldKind::ComboBox(combo) => {
                assert_eq!(combo.text, "ab", "the text must not have changed")
            }
            other => panic!("expected ComboBox, got {other:?}"),
        }
    }

    #[test]
    fn combo_box_events_carry_the_field_id_not_the_embedded_combo_box_id() {
        // The `FormField` and its embedded `ComboBox` are given distinct
        // ids here, mirroring `examples/common/form_all_fields.rs`'s
        // "fruit" field / "fruit-combo" primitive split. Every
        // `FormEvent::ComboBox*` below must carry the *field* id
        // ("field"), not the primitive's own id ("combo") — otherwise a
        // consumer resolving the event back to its `FormField` via
        // `form.fields.iter().find(|f| f.id == id)` would silently find
        // nothing.
        let mut fc = controller_with(
            vec![combo_field_with_ids(
                "field",
                "combo",
                &["Apple", "Banana", "Cherry"],
                "Ban",
                false,
                false,
            )],
            Some("field"),
        );
        let ev = fc.handle_cached(&key_event(Key::Char('b'), Modifiers::default()), RECT);
        assert_eq!(
            ev,
            FormControllerEvent::FormAction(FormEvent::ComboBoxTextChanged {
                id: WidgetId::new("field"),
                value: "Banb".to_string(),
            })
        );

        let mut fc = controller_with(
            vec![combo_field_with_ids(
                "field",
                "combo",
                &["Apple", "Banana", "Cherry"],
                "",
                true,
                false,
            )],
            Some("field"),
        );
        let ev = fc.handle_cached(
            &key_event(Key::Named(NamedKey::Down), Modifiers::default()),
            RECT,
        );
        assert_eq!(
            ev,
            FormControllerEvent::FormAction(FormEvent::ComboBoxHighlightChanged {
                id: WidgetId::new("field"),
                index: 0,
            })
        );

        let mut fc = controller_with(
            vec![combo_field_with_ids(
                "field",
                "combo",
                &["Apple", "Banana", "Cherry"],
                "Ban",
                false,
                false,
            )],
            Some("field"),
        );
        let ev = fc.handle_cached(
            &key_event(Key::Named(NamedKey::Enter), Modifiers::default()),
            RECT,
        );
        assert_eq!(
            ev,
            FormControllerEvent::FormAction(FormEvent::ComboBoxCommitted {
                id: WidgetId::new("field"),
                value: "Ban".to_string(),
            })
        );

        let mut fc = controller_with(
            vec![combo_field_with_ids(
                "field",
                "combo",
                &["Apple", "Banana"],
                "a",
                true,
                false,
            )],
            Some("field"),
        );
        let ev = fc.handle_cached(
            &key_event(Key::Named(NamedKey::Escape), Modifiers::default()),
            RECT,
        );
        assert_eq!(
            ev,
            FormControllerEvent::FormAction(FormEvent::ComboBoxCancelled {
                id: WidgetId::new("field"),
            })
        );
    }

    // ── NumberInput routing ─────────────────────────────────────────

    #[test]
    fn up_arrow_steps_a_focused_number_input_value_up() {
        let mut fc = controller_with(
            vec![number_field(
                "n",
                NumberInput::new(WidgetId::new("n"), 0.0, 10.0),
                false,
            )],
            Some("n"),
        );
        let ev = fc.handle_cached(
            &key_event(Key::Named(NamedKey::Up), Modifiers::default()),
            RECT,
        );
        assert_eq!(
            ev,
            FormControllerEvent::FormAction(FormEvent::NumberChanged {
                id: WidgetId::new("n"),
                value: 1.0,
            })
        );
    }

    #[test]
    fn down_arrow_steps_a_focused_number_input_value_down_clamped_at_min() {
        let mut fc = controller_with(
            vec![number_field(
                "n",
                NumberInput::new(WidgetId::new("n"), 0.0, 10.0),
                false,
            )],
            Some("n"),
        );
        let ev = fc.handle_cached(
            &key_event(Key::Named(NamedKey::Down), Modifiers::default()),
            RECT,
        );
        assert_eq!(
            ev,
            FormControllerEvent::FormAction(FormEvent::NumberChanged {
                id: WidgetId::new("n"),
                value: 0.0,
            }),
            "decrementing below `min` must clamp, not go negative"
        );
    }

    #[test]
    fn up_arrow_on_a_number_input_already_at_max_clamps_instead_of_overshooting() {
        let mut n = NumberInput::new(WidgetId::new("n"), 0.0, 10.0);
        n.value = 9.0;
        n.step = 5.0;
        let mut fc = controller_with(vec![number_field("n", n, false)], Some("n"));
        let ev = fc.handle_cached(
            &key_event(Key::Named(NamedKey::Up), Modifiers::default()),
            RECT,
        );
        assert_eq!(
            ev,
            FormControllerEvent::FormAction(FormEvent::NumberChanged {
                id: WidgetId::new("n"),
                value: 10.0,
            }),
            "a step past `max` must clamp to `max`, not overshoot to 14.0"
        );
    }

    #[test]
    fn typed_digit_on_a_focused_number_input_updates_its_draft() {
        let mut fc = controller_with(
            vec![number_field(
                "n",
                NumberInput::new(WidgetId::new("n"), 0.0, 100.0),
                false,
            )],
            Some("n"),
        );
        let ev = fc.handle_cached(&key_event(Key::Char('5'), Modifiers::default()), RECT);
        assert_eq!(
            ev,
            FormControllerEvent::FormAction(FormEvent::NumberDraftChanged {
                id: WidgetId::new("n"),
                draft: "5".to_string(),
            })
        );
    }

    #[test]
    fn enter_commits_the_focused_number_inputs_draft_clamped_and_rounded() {
        let mut n = NumberInput::new(WidgetId::new("n"), 0.0, 10.0);
        n.draft = Some("999.456".into());
        n.precision = 1;
        let mut fc = controller_with(vec![number_field("n", n, false)], Some("n"));
        let ev = fc.handle_cached(
            &key_event(Key::Named(NamedKey::Enter), Modifiers::default()),
            RECT,
        );
        assert_eq!(
            ev,
            FormControllerEvent::FormAction(FormEvent::NumberCommitted {
                id: WidgetId::new("n"),
                value: 10.0,
            }),
            "the draft must clamp to `max` on commit"
        );
    }

    #[test]
    fn escape_cancels_a_focused_number_inputs_pending_draft() {
        let mut n = NumberInput::new(WidgetId::new("n"), 0.0, 10.0);
        n.draft = Some("3".into());
        let mut fc = controller_with(vec![number_field("n", n, false)], Some("n"));
        let ev = fc.handle_cached(
            &key_event(Key::Named(NamedKey::Escape), Modifiers::default()),
            RECT,
        );
        assert_eq!(
            ev,
            FormControllerEvent::FormAction(FormEvent::NumberCancelled {
                id: WidgetId::new("n"),
            })
        );
    }

    #[test]
    fn disabled_number_input_field_ignores_its_keys() {
        let mut fc = controller_with(
            vec![number_field(
                "n",
                NumberInput::new(WidgetId::new("n"), 0.0, 10.0),
                true,
            )],
            Some("n"),
        );
        let ev = fc.handle_cached(
            &key_event(Key::Named(NamedKey::Up), Modifiers::default()),
            RECT,
        );
        assert_eq!(ev, FormControllerEvent::Ignored);
    }

    #[test]
    fn tab_moves_focus_away_from_a_focused_number_input_field_normally() {
        let mut fc = controller_with(
            vec![
                number_field("n", NumberInput::new(WidgetId::new("n"), 0.0, 10.0), false),
                toggle_field("t", false),
            ],
            Some("n"),
        );
        let ev = fc.handle_cached(
            &key_event(Key::Named(NamedKey::Tab), Modifiers::default()),
            RECT,
        );
        assert_eq!(
            ev,
            FormControllerEvent::FormAction(FormEvent::FocusChanged {
                id: WidgetId::new("t")
            }),
            "Tab isn't one of NumberInput's claimed keys, so focus traversal \
             off a focused NumberInput field still works"
        );
    }

    /// `lh=10.0` → `row_h=14.0`, `char_w=6.0`; an unlabelled `NumberInput`
    /// (no label, so `items_start_x = char_w = 6.0`) showing "0" sizes its
    /// single item to `(1 + 5) * 6.0 = 36.0` wide — see
    /// `form_field_measure`'s `FieldKind::NumberInput` arm — so its
    /// control rect spans x in `[6, 42)`. `NumberInput::layout` then
    /// splits off a 3-wide stepper column at the right: increment is
    /// `[39, 42) x [0, 7)`, decrement is `[39, 42) x [7, 14)`.
    #[test]
    fn stepper_click_increments_a_number_inputs_value() {
        let mut fc = controller_with(
            vec![number_field(
                "n",
                NumberInput::new(WidgetId::new("n"), 0.0, 10.0),
                false,
            )],
            None,
        );
        fc.set_backend_info(10.0);
        let ev = fc.handle_cached(
            &UiEvent::MouseDown {
                widget: None,
                button: MouseButton::Left,
                position: crate::Point { x: 40.0, y: 2.0 },
                modifiers: Default::default(),
            },
            Rect::new(0.0, 0.0, 200.0, 14.0),
        );
        assert_eq!(
            ev,
            FormControllerEvent::FormAction(FormEvent::NumberChanged {
                id: WidgetId::new("n"),
                value: 1.0,
            })
        );
    }

    #[test]
    fn stepper_click_decrements_a_number_inputs_value() {
        let mut n = NumberInput::new(WidgetId::new("n"), 0.0, 10.0);
        n.value = 5.0;
        let mut fc = controller_with(vec![number_field("n", n, false)], None);
        fc.set_backend_info(10.0);
        let ev = fc.handle_cached(
            &UiEvent::MouseDown {
                widget: None,
                button: MouseButton::Left,
                position: crate::Point { x: 40.0, y: 10.0 },
                modifiers: Default::default(),
            },
            Rect::new(0.0, 0.0, 200.0, 14.0),
        );
        assert_eq!(
            ev,
            FormControllerEvent::FormAction(FormEvent::NumberChanged {
                id: WidgetId::new("n"),
                value: 4.0,
            })
        );
    }

    #[test]
    fn stepper_click_on_a_disabled_number_input_just_focuses_the_field() {
        let mut fc = controller_with(
            vec![number_field(
                "n",
                NumberInput::new(WidgetId::new("n"), 0.0, 10.0),
                true,
            )],
            None,
        );
        fc.set_backend_info(10.0);
        let ev = fc.handle_cached(
            &UiEvent::MouseDown {
                widget: None,
                button: MouseButton::Left,
                position: crate::Point { x: 40.0, y: 2.0 },
                modifiers: Default::default(),
            },
            Rect::new(0.0, 0.0, 200.0, 14.0),
        );
        assert_eq!(
            ev,
            FormControllerEvent::FormAction(FormEvent::FocusChanged {
                id: WidgetId::new("n"),
            }),
            "a disabled NumberInput's stepper click must not step the value"
        );
    }

    // ── Slider routing ────────────────────────────────────────────

    #[test]
    fn left_arrow_steps_a_focused_sliders_value_down() {
        let mut fc = controller_with(
            vec![slider_field("s", 5.0, 0.0, 10.0, 1.0, false)],
            Some("s"),
        );
        let ev = fc.handle_cached(
            &key_event(Key::Named(NamedKey::Left), Modifiers::default()),
            RECT,
        );
        assert_eq!(
            ev,
            FormControllerEvent::FormAction(FormEvent::SliderChanged {
                id: WidgetId::new("s"),
                value: 4.0,
            })
        );
    }

    #[test]
    fn right_arrow_steps_a_focused_sliders_value_up() {
        let mut fc = controller_with(
            vec![slider_field("s", 5.0, 0.0, 10.0, 1.0, false)],
            Some("s"),
        );
        let ev = fc.handle_cached(
            &key_event(Key::Named(NamedKey::Right), Modifiers::default()),
            RECT,
        );
        assert_eq!(
            ev,
            FormControllerEvent::FormAction(FormEvent::SliderChanged {
                id: WidgetId::new("s"),
                value: 6.0,
            })
        );
    }

    #[test]
    fn up_and_down_arrows_mirror_right_and_left_for_a_focused_slider() {
        let mut fc = controller_with(
            vec![slider_field("s", 5.0, 0.0, 10.0, 2.0, false)],
            Some("s"),
        );
        let ev = fc.handle_cached(
            &key_event(Key::Named(NamedKey::Up), Modifiers::default()),
            RECT,
        );
        assert_eq!(
            ev,
            FormControllerEvent::FormAction(FormEvent::SliderChanged {
                id: WidgetId::new("s"),
                value: 7.0,
            })
        );

        let mut fc = controller_with(
            vec![slider_field("s", 5.0, 0.0, 10.0, 2.0, false)],
            Some("s"),
        );
        let ev = fc.handle_cached(
            &key_event(Key::Named(NamedKey::Down), Modifiers::default()),
            RECT,
        );
        assert_eq!(
            ev,
            FormControllerEvent::FormAction(FormEvent::SliderChanged {
                id: WidgetId::new("s"),
                value: 3.0,
            })
        );
    }

    #[test]
    fn right_arrow_past_max_clamps_instead_of_overshooting() {
        let mut fc = controller_with(
            vec![slider_field("s", 9.0, 0.0, 10.0, 5.0, false)],
            Some("s"),
        );
        let ev = fc.handle_cached(
            &key_event(Key::Named(NamedKey::Right), Modifiers::default()),
            RECT,
        );
        assert_eq!(
            ev,
            FormControllerEvent::FormAction(FormEvent::SliderChanged {
                id: WidgetId::new("s"),
                value: 10.0,
            }),
            "a step past `max` must clamp to `max`, not overshoot to 14.0"
        );
    }

    #[test]
    fn home_and_end_jump_a_focused_slider_to_its_bounds() {
        let mut fc = controller_with(
            vec![slider_field("s", 5.0, 0.0, 10.0, 1.0, false)],
            Some("s"),
        );
        let ev = fc.handle_cached(
            &key_event(Key::Named(NamedKey::Home), Modifiers::default()),
            RECT,
        );
        assert_eq!(
            ev,
            FormControllerEvent::FormAction(FormEvent::SliderChanged {
                id: WidgetId::new("s"),
                value: 0.0,
            })
        );

        let mut fc = controller_with(
            vec![slider_field("s", 5.0, 0.0, 10.0, 1.0, false)],
            Some("s"),
        );
        let ev = fc.handle_cached(
            &key_event(Key::Named(NamedKey::End), Modifiers::default()),
            RECT,
        );
        assert_eq!(
            ev,
            FormControllerEvent::FormAction(FormEvent::SliderChanged {
                id: WidgetId::new("s"),
                value: 10.0,
            })
        );
    }

    /// Rect every slider click/drag test below aims inside of. Wide
    /// enough that one field needs no scrollbar, so `split_rect_lh`
    /// leaves it whole.
    const SLIDER_RECT: Rect = Rect {
        x: 0.0,
        y: 0.0,
        width: 200.0,
        height: 14.0,
    };

    /// The painted track rect `FormController`'s own backend-free
    /// fallback layout (`lh = 10.0`, the `set_backend_info` these tests
    /// use) reports for slider field `id`.
    ///
    /// Asked for rather than hardcoded, so a test aims at a fraction of
    /// the track the painter actually draws — the whole point of the
    /// `item_bounds` contract — instead of pinning arithmetic that would
    /// still "pass" if click and paint drifted apart.
    fn fallback_track_rect(fc: &FormController, id: &str) -> Rect {
        let lh = 10.0;
        let (form_rect, _sb) = split_rect_lh(fc.field_count(), lh, SLIDER_RECT);
        let form = fc.build_form(form_rect, None);
        let row_h = row_height(lh);
        let char_w = lh * 0.6;
        let layout = form.layout(form_rect.width, form_rect.height, |i| {
            form_field_measure(&form.fields[i], row_h, char_w)
        });
        slider_track_rect(&form, &layout, &WidgetId::new(id))
            .expect("an enabled Slider field has a painted track rect")
    }

    fn mouse_down_at(fc: &mut FormController, x: f32) -> FormControllerEvent {
        fc.handle_cached(
            &UiEvent::MouseDown {
                widget: None,
                button: MouseButton::Left,
                position: crate::Point { x, y: 7.0 },
                modifiers: Default::default(),
            },
            SLIDER_RECT,
        )
    }

    fn left_drag_to(fc: &mut FormController, x: f32) -> FormControllerEvent {
        fc.handle_cached(
            &UiEvent::MouseMoved {
                position: crate::Point { x, y: 7.0 },
                buttons: ButtonMask {
                    left: true,
                    middle: false,
                    right: false,
                },
            },
            SLIDER_RECT,
        )
    }

    fn labeled_slider_controller(value: f32) -> FormController {
        let mut field = slider_field("s", value, 0.0, 10.0, 1.0, false);
        field.label = StyledText::plain("Volume");
        let mut fc = controller_with(vec![field], None);
        fc.set_backend_info(10.0);
        fc
    }

    /// An unlabeled `Slider`: a press at the midpoint of the track the
    /// layout reports lands at fraction `0.5` of `[min, max] = [0, 10]`.
    #[test]
    fn click_on_a_sliders_track_maps_the_click_position_to_a_value() {
        let mut fc = controller_with(vec![slider_field("s", 0.0, 0.0, 10.0, 1.0, false)], None);
        fc.set_backend_info(10.0);
        let track = fallback_track_rect(&fc, "s");
        let ev = mouse_down_at(&mut fc, track.x + track.width / 2.0);
        assert_eq!(
            ev,
            FormControllerEvent::FormAction(FormEvent::SliderChanged {
                id: WidgetId::new("s"),
                value: 5.0,
            })
        );
    }

    /// The same press on a *labeled* `Slider` — `LabelPosition::Left` is
    /// the form-wide default (see `Form::label_position`'s doc), so this
    /// is the layout any ordinary labeled slider field gets. The track
    /// sits past the label column, and a press at each of its quarter
    /// positions resolves to that fraction of `[min, max]`, not to a
    /// fraction of the whole row.
    #[test]
    fn click_on_a_labeled_sliders_track_maps_the_click_position_to_a_value() {
        // Binary-exact fractions, so the expected value needs no float
        // tolerance — the mapping under test is a plain multiply.
        for (fraction, expected) in [(0.0, 0.0), (0.25, 2.5), (0.5, 5.0), (0.75, 7.5)] {
            let mut fc = labeled_slider_controller(10.0);
            let track = fallback_track_rect(&fc, "s");
            assert!(
                track.x > 0.0,
                "a labeled slider's track must start past its label column"
            );
            let ev = mouse_down_at(&mut fc, track.x + track.width * fraction);
            assert_eq!(
                ev,
                FormControllerEvent::FormAction(FormEvent::SliderChanged {
                    id: WidgetId::new("s"),
                    value: expected,
                }),
                "a press at fraction {fraction} of a labeled slider's track"
            );
        }
    }

    /// A press square on a labeled slider's label text is not a track
    /// interaction, so it must not resolve to `fraction == 0.0` → `min`.
    /// It falls through to `form_click_event`'s generic `FocusChanged`
    /// instead, the same outcome a click on any other labeled field's
    /// label already gets.
    #[test]
    fn click_on_a_labeled_sliders_label_column_only_focuses_it() {
        let mut fc = labeled_slider_controller(5.0);
        let track = fallback_track_rect(&fc, "s");
        let ev = mouse_down_at(&mut fc, track.x / 2.0);
        assert_eq!(
            ev,
            FormControllerEvent::FormAction(FormEvent::FocusChanged {
                id: WidgetId::new("s"),
            }),
            "a click on a labeled slider's label column must only focus it, never move its value"
        );
    }

    /// Nor is a press on the value text painted after the track: the row
    /// extends well past the track's right edge, and mapping an `x`
    /// there would snap the value to `max`.
    #[test]
    fn click_right_of_a_sliders_track_only_focuses_it() {
        let mut fc = labeled_slider_controller(5.0);
        let track = fallback_track_rect(&fc, "s");
        let past_end = track.x + track.width + 1.0;
        assert!(
            past_end < SLIDER_RECT.width,
            "the probe must still land inside the field's own row"
        );
        let ev = mouse_down_at(&mut fc, past_end);
        assert_eq!(
            ev,
            FormControllerEvent::FormAction(FormEvent::FocusChanged {
                id: WidgetId::new("s"),
            }),
            "a click past a slider's track must only focus it, never snap its value to max"
        );
    }

    /// A `MouseMoved` with the left button still held, after a `MouseDown`
    /// armed `slider_drag` on the same field, keeps mapping the pointer's
    /// x position to a value the same way the initial click did — and a
    /// drag the track already captured pins to `max` past its right edge
    /// rather than dropping out, unlike a fresh press there.
    #[test]
    fn drag_after_a_sliders_click_continues_mapping_the_pointer_to_a_value() {
        let mut fc = labeled_slider_controller(0.0);
        let track = fallback_track_rect(&fc, "s");
        let down = mouse_down_at(&mut fc, track.x + track.width / 2.0);
        assert_eq!(
            down,
            FormControllerEvent::FormAction(FormEvent::SliderChanged {
                id: WidgetId::new("s"),
                value: 5.0,
            })
        );

        let moved = left_drag_to(&mut fc, track.x + track.width * 0.75);
        assert_eq!(
            moved,
            FormControllerEvent::FormAction(FormEvent::SliderChanged {
                id: WidgetId::new("s"),
                value: 7.5,
            }),
            "a drag after the click should keep mapping the pointer's x to a value"
        );

        let past_end = left_drag_to(&mut fc, track.x + track.width + 50.0);
        assert_eq!(
            past_end,
            FormControllerEvent::FormAction(FormEvent::SliderChanged {
                id: WidgetId::new("s"),
                value: 10.0,
            }),
            "dragging past the track's right edge pins the value to max"
        );

        let up = fc.handle_cached(
            &UiEvent::MouseUp {
                widget: None,
                button: MouseButton::Left,
                position: crate::Point { x: 150.0, y: 7.0 },
            },
            SLIDER_RECT,
        );
        assert_eq!(up, FormControllerEvent::Ignored);

        // `MouseUp` cleared `slider_drag` — a further move with the left
        // button held (as if a fresh, unrelated drag started elsewhere)
        // must not still be attributed to this slider.
        let moved_after_release = left_drag_to(&mut fc, track.x + 1.0);
        assert_eq!(moved_after_release, FormControllerEvent::Ignored);
    }

    /// A press on a labeled slider's label column focuses the row
    /// without capturing the pointer, so the drag that follows it is not
    /// attributed to the slider either — otherwise a press meant to
    /// focus the field would start rewriting its value the moment the
    /// pointer moved.
    #[test]
    fn a_label_column_press_does_not_arm_a_sliders_drag() {
        let mut fc = labeled_slider_controller(5.0);
        let track = fallback_track_rect(&fc, "s");
        mouse_down_at(&mut fc, track.x / 2.0);
        assert_eq!(
            left_drag_to(&mut fc, track.x + track.width),
            FormControllerEvent::Ignored,
            "a drag from a label-column press must not move the slider's value"
        );
    }

    #[test]
    fn a_disabled_sliders_click_and_keys_produce_focus_changed_or_ignored_only() {
        let mut fc = controller_with(vec![slider_field("s", 5.0, 0.0, 10.0, 1.0, true)], None);
        fc.set_backend_info(10.0);
        let click = mouse_down_at(&mut fc, 100.0);
        assert_eq!(
            click,
            FormControllerEvent::FormAction(FormEvent::FocusChanged {
                id: WidgetId::new("s"),
            }),
            "a disabled Slider's click must only focus it, never move its value"
        );

        fc.set_has_focus(true);
        if let Some(form) = &mut fc.form {
            form.focused_field = Some(WidgetId::new("s"));
        }
        let key = fc.handle_cached(
            &key_event(Key::Named(NamedKey::Right), Modifiers::default()),
            RECT,
        );
        assert_eq!(
            key,
            FormControllerEvent::Ignored,
            "a disabled Slider's keys must not intercept at all"
        );
    }

    // ── RadioGroup routing ──────────────────────────────────────────

    fn radio_group(selected: Option<usize>) -> RadioGroup {
        let mut group = RadioGroup::new(
            WidgetId::new("theme"),
            vec![
                RadioOption::new(WidgetId::new("theme:light"), "Light"),
                RadioOption::new(WidgetId::new("theme:dark"), "Dark"),
            ],
        );
        group.selected = selected;
        group
    }

    #[test]
    fn down_arrow_moves_a_focused_radio_groups_selection() {
        let mut fc = controller_with(
            vec![radio_group_field("g", radio_group(Some(0)), false)],
            Some("g"),
        );
        let ev = fc.handle_cached(
            &key_event(Key::Named(NamedKey::Down), Modifiers::default()),
            RECT,
        );
        assert_eq!(
            ev,
            FormControllerEvent::FormAction(FormEvent::RadioChanged {
                id: WidgetId::new("g"),
                selected: 1,
            })
        );
    }

    #[test]
    fn left_arrow_moves_a_focused_radio_groups_selection_back() {
        let mut fc = controller_with(
            vec![radio_group_field("g", radio_group(Some(1)), false)],
            Some("g"),
        );
        let ev = fc.handle_cached(
            &key_event(Key::Named(NamedKey::Left), Modifiers::default()),
            RECT,
        );
        assert_eq!(
            ev,
            FormControllerEvent::FormAction(FormEvent::RadioChanged {
                id: WidgetId::new("g"),
                selected: 0,
            })
        );
    }

    #[test]
    fn disabled_radio_group_field_ignores_its_keys() {
        let mut fc = controller_with(
            vec![radio_group_field("g", radio_group(Some(0)), true)],
            Some("g"),
        );
        let ev = fc.handle_cached(
            &key_event(Key::Named(NamedKey::Down), Modifiers::default()),
            RECT,
        );
        assert_eq!(ev, FormControllerEvent::Ignored);
    }

    #[test]
    fn tab_moves_focus_away_from_a_focused_radio_group_field_normally() {
        let mut fc = controller_with(
            vec![
                radio_group_field("g", radio_group(Some(0)), false),
                toggle_field("t", false),
            ],
            Some("g"),
        );
        let ev = fc.handle_cached(
            &key_event(Key::Named(NamedKey::Tab), Modifiers::default()),
            RECT,
        );
        assert_eq!(
            ev,
            FormControllerEvent::FormAction(FormEvent::FocusChanged {
                id: WidgetId::new("t")
            }),
            "Tab isn't one of RadioGroup's claimed keys, so focus traversal \
             off a focused RadioGroup field still works"
        );
    }

    #[test]
    fn click_radio_group_option_emits_radio_changed_with_the_field_id() {
        let mut form = make_form(1);
        form.fields[0] = radio_group_field("g", radio_group(Some(0)), false);
        let ev = form_click_event(&form, &WidgetId::new("theme:dark"));
        assert_eq!(
            ev,
            FormEvent::RadioChanged {
                id: WidgetId::new("g"),
                selected: 1,
            },
            "the event must carry the *field's* id, not the clicked option's"
        );
    }

    #[test]
    fn click_disabled_radio_group_option_focuses_the_field_instead_of_selecting() {
        let mut group = radio_group(Some(0));
        group.options[1].disabled = true;
        let mut form = make_form(1);
        form.fields[0] = radio_group_field("g", group, false);
        let ev = form_click_event(&form, &WidgetId::new("theme:dark"));
        assert_eq!(
            ev,
            FormEvent::FocusChanged {
                id: WidgetId::new("g"),
            }
        );
    }

    /// A real `MouseDown`, routed through `Form::layout`/`hit_test` via
    /// `TuiBackend` (not `form_click_event` called directly, which
    /// bypasses layout entirely) — a `LabelPosition::Top`-resolved
    /// `RadioGroup`'s painted "Light" row must select "Light", not the
    /// option one row below it (see `tui_form_layout_with_label_width`'s
    /// `FieldKind::RadioGroup` arm and `items_start_y` on
    /// `FormFieldMeasure`).
    #[test]
    #[cfg(feature = "tui")]
    fn click_on_a_top_labelled_radio_groups_painted_row_selects_the_right_option() {
        use crate::primitives::form::LabelPosition;
        use crate::tui::TuiBackend;

        let mut field = radio_group_field("g", radio_group(Some(0)), false);
        field.label = StyledText::plain("Theme");
        field.label_position = Some(LabelPosition::Top);
        let mut fc = controller_with(vec![field], None);
        let mut backend = TuiBackend::new();
        let rect = Rect::new(0.0, 0.0, 40.0, 10.0);

        // Row 0 of the field's own bounds is the label ("Theme"); row 1
        // is "Light" (the first, currently-selected option); row 2 is
        // "Dark".
        let field_rect = fc.field_rect(WidgetId::new("g"), &backend, rect).unwrap();
        let light_y = field_rect.y + 1.0;
        let ev = fc.handle(
            &UiEvent::MouseDown {
                widget: None,
                button: MouseButton::Left,
                position: crate::Point {
                    x: field_rect.x + 1.0,
                    y: light_y + 0.5,
                },
                modifiers: Default::default(),
            },
            &mut backend,
            rect,
        );
        assert_eq!(
            ev,
            FormControllerEvent::FormAction(FormEvent::RadioChanged {
                id: WidgetId::new("g"),
                selected: 0,
            }),
            "a click on the row painted \"Light\" must select \"Light\" (index 0), not \"Dark\""
        );
    }

    // ── Dropdown routing ───────────────────────────────────────────

    #[test]
    fn down_arrow_moves_and_commits_dropdown_selection() {
        let mut fc = controller_with(
            vec![dropdown_field(
                "d",
                &["Apple", "Banana", "Cherry"],
                0,
                false,
            )],
            Some("d"),
        );
        let ev = fc.handle_cached(
            &key_event(Key::Named(NamedKey::Down), Modifiers::default()),
            RECT,
        );
        assert_eq!(
            ev,
            FormControllerEvent::FormAction(FormEvent::DropdownChanged {
                id: WidgetId::new("d"),
                selected_idx: 1,
            }),
            "Down must move AND commit in the same keystroke — Dropdown \
             has no separate open/highlight step"
        );
    }

    #[test]
    fn up_arrow_moves_dropdown_selection_back() {
        let mut fc = controller_with(
            vec![dropdown_field(
                "d",
                &["Apple", "Banana", "Cherry"],
                2,
                false,
            )],
            Some("d"),
        );
        let ev = fc.handle_cached(
            &key_event(Key::Named(NamedKey::Up), Modifiers::default()),
            RECT,
        );
        assert_eq!(
            ev,
            FormControllerEvent::FormAction(FormEvent::DropdownChanged {
                id: WidgetId::new("d"),
                selected_idx: 1,
            })
        );
    }

    #[test]
    fn up_arrow_at_first_option_falls_through_instead_of_wrapping() {
        let mut fc = controller_with(
            vec![dropdown_field("d", &["Apple", "Banana"], 0, false)],
            Some("d"),
        );
        let ev = fc.handle_cached(
            &key_event(Key::Named(NamedKey::Up), Modifiers::default()),
            RECT,
        );
        assert_eq!(ev, FormControllerEvent::Ignored);
    }

    #[test]
    fn down_arrow_at_last_option_falls_through_instead_of_wrapping() {
        let mut fc = controller_with(
            vec![dropdown_field("d", &["Apple", "Banana"], 1, false)],
            Some("d"),
        );
        let ev = fc.handle_cached(
            &key_event(Key::Named(NamedKey::Down), Modifiers::default()),
            RECT,
        );
        assert_eq!(ev, FormControllerEvent::Ignored);
    }

    #[test]
    fn disabled_dropdown_field_ignores_arrow_keys() {
        let mut fc = controller_with(
            vec![dropdown_field("d", &["Apple", "Banana"], 0, true)],
            Some("d"),
        );
        let ev = fc.handle_cached(
            &key_event(Key::Named(NamedKey::Down), Modifiers::default()),
            RECT,
        );
        assert_eq!(ev, FormControllerEvent::Ignored);
    }

    #[test]
    fn tab_moves_focus_away_from_a_focused_dropdown_field_normally() {
        let mut fc = controller_with(
            vec![
                dropdown_field("d", &["Apple", "Banana"], 0, false),
                toggle_field("t", false),
            ],
            Some("d"),
        );
        let ev = fc.handle_cached(
            &key_event(Key::Named(NamedKey::Tab), Modifiers::default()),
            RECT,
        );
        assert_eq!(
            ev,
            FormControllerEvent::FormAction(FormEvent::FocusChanged {
                id: WidgetId::new("t")
            }),
            "Tab isn't one of Dropdown's claimed keys, so focus traversal \
             off a focused Dropdown field still works"
        );
    }

    #[test]
    fn ctrl_modified_dropdown_key_falls_through_instead_of_navigating() {
        let mut fc = controller_with(
            vec![dropdown_field("d", &["Apple", "Banana"], 0, false)],
            Some("d"),
        );
        let ev = fc.handle_cached(&key_event(Key::Named(NamedKey::Down), ctrl()), RECT);
        assert_eq!(
            ev,
            FormControllerEvent::Ignored,
            "Ctrl+Down is a host accelerator candidate, not dropdown \
             navigation, so it must not be swallowed"
        );
        let form = fc.build_form(RECT, None);
        match &form.fields[0].kind {
            FieldKind::Dropdown { selected_idx, .. } => {
                assert_eq!(*selected_idx, 0, "selection must not have changed")
            }
            other => panic!("expected Dropdown, got {other:?}"),
        }
    }

    // ── Item-level keyboard focus ──────────────────────────────────────
    //
    // See `primitives::form::contract`'s "Item-level keyboard focus"
    // doc section.

    fn button_row_field(id: &str, buttons: &[(&str, bool)]) -> FormField {
        FormField {
            id: WidgetId::new(id),
            label: StyledText::default(),
            kind: FieldKind::ButtonRow {
                buttons: buttons
                    .iter()
                    .map(|(bid, disabled)| crate::primitives::form::ButtonRowItem {
                        id: WidgetId::new(*bid),
                        label: (*bid).to_string(),
                        disabled: *disabled,
                        icon: None,
                        spec: None,
                    })
                    .collect(),
            },
            hint: StyledText::default(),
            disabled: false,
            validation: None,
            label_position: None,
            label_align: None,
            label_width: None,
            tab_index: None,
            control_width: None,
            min_control_width: None,
            row_align: None,
        }
    }

    fn toggle_group_field(id: &str, toggles: &[(&str, bool)]) -> FormField {
        FormField {
            id: WidgetId::new(id),
            label: StyledText::default(),
            kind: FieldKind::ToggleGroup {
                toggles: toggles
                    .iter()
                    .map(|(tid, value)| ToggleGroupItem {
                        id: WidgetId::new(*tid),
                        label: (*tid).to_string(),
                        value: *value,
                    })
                    .collect(),
            },
            hint: StyledText::default(),
            disabled: false,
            validation: None,
            label_position: None,
            label_align: None,
            label_width: None,
            tab_index: None,
            control_width: None,
            min_control_width: None,
            row_align: None,
        }
    }

    fn toolbar_field(id: &str, buttons: &[(&str, bool)]) -> FormField {
        use crate::primitives::toolbar::{Toolbar, ToolbarButton};
        FormField {
            id: WidgetId::new(id),
            label: StyledText::default(),
            kind: FieldKind::Toolbar(Toolbar {
                id: WidgetId::new(format!("{id}-toolbar")),
                buttons: buttons
                    .iter()
                    .map(|(bid, enabled)| ToolbarButton::Action {
                        spec: None,
                        id: WidgetId::new(*bid),
                        label: (*bid).to_string(),
                        icon: None,
                        key_hint: None,
                        enabled: *enabled,
                        is_active: false,
                        tooltip: String::new(),
                    })
                    .collect(),
                bg: None,
                focused_index: None,
                ..Default::default()
            }),
            hint: StyledText::default(),
            disabled: false,
            validation: None,
            label_position: None,
            label_align: None,
            label_width: None,
            tab_index: None,
            control_width: None,
            min_control_width: None,
            row_align: None,
        }
    }

    #[test]
    fn right_then_enter_activates_the_second_button_in_a_button_row() {
        let mut fc = controller_with(
            vec![button_row_field("row", &[("a", false), ("b", false)])],
            Some("row"),
        );
        let moved = fc.handle_cached(
            &key_event(Key::Named(NamedKey::Right), Modifiers::default()),
            RECT,
        );
        assert_eq!(moved, FormControllerEvent::Consumed);
        let activated = fc.handle_cached(
            &key_event(Key::Named(NamedKey::Enter), Modifiers::default()),
            RECT,
        );
        assert_eq!(
            activated,
            FormControllerEvent::FormAction(FormEvent::ButtonClicked {
                id: WidgetId::new("b")
            })
        );
    }

    #[test]
    fn space_also_activates_the_focused_button() {
        let mut fc = controller_with(
            vec![button_row_field("row", &[("a", false), ("b", false)])],
            Some("row"),
        );
        let ev = fc.handle_cached(&key_event(Key::Char(' '), Modifiers::default()), RECT);
        assert_eq!(
            ev,
            FormControllerEvent::FormAction(FormEvent::ButtonClicked {
                id: WidgetId::new("a")
            }),
            "Space activates the first (default-focused) item without a prior Right"
        );
    }

    #[test]
    fn space_and_enter_flip_a_focused_plain_toggle() {
        let mut fc = controller_with(vec![toggle_field("flag", false)], Some("flag"));
        let ev = fc.handle_cached(&key_event(Key::Char(' '), Modifiers::default()), RECT);
        assert_eq!(
            ev,
            FormControllerEvent::FormAction(FormEvent::ToggleChanged {
                id: WidgetId::new("flag"),
                value: true,
            }),
            "Space must flip a focused plain Toggle field, not just a mouse click"
        );

        let ev = fc.handle_cached(
            &key_event(Key::Named(NamedKey::Enter), Modifiers::default()),
            RECT,
        );
        assert_eq!(
            ev,
            FormControllerEvent::FormAction(FormEvent::ToggleChanged {
                id: WidgetId::new("flag"),
                value: true,
            }),
            "Enter must flip a focused plain Toggle field too"
        );
    }

    #[test]
    fn disabled_toggle_ignores_space_and_enter() {
        let mut fc = controller_with(vec![toggle_field("flag", true)], Some("flag"));
        let ev = fc.handle_cached(&key_event(Key::Char(' '), Modifiers::default()), RECT);
        assert_eq!(ev, FormControllerEvent::Ignored);
    }

    #[test]
    fn char_typed_space_flips_a_focused_toggle_the_same_as_a_keypressed_space() {
        // A space delivered as a raw `CharTyped` (some backends don't
        // route a printable key through `KeyPressed` at all) must
        // activate the toggle too, not fall through to
        // `handle_text_op`'s no-op for a field with no text value.
        let mut fc = controller_with(vec![toggle_field("flag", false)], Some("flag"));
        let ev = fc.handle_cached(&UiEvent::CharTyped(' '), RECT);
        assert_eq!(
            ev,
            FormControllerEvent::FormAction(FormEvent::ToggleChanged {
                id: WidgetId::new("flag"),
                value: true,
            })
        );
    }

    #[test]
    fn char_typed_space_activates_a_focused_button_row_item_too() {
        let mut fc = controller_with(
            vec![button_row_field("row", &[("a", false), ("b", false)])],
            Some("row"),
        );
        let ev = fc.handle_cached(&UiEvent::CharTyped(' '), RECT);
        assert_eq!(
            ev,
            FormControllerEvent::FormAction(FormEvent::ButtonClicked {
                id: WidgetId::new("a")
            })
        );
    }

    #[test]
    fn char_typed_space_still_inserts_into_a_focused_text_field() {
        // `activate_focused_on_space` must return `None` for a field
        // kind it has no activation story for, so a text field's own
        // space character keeps inserting normally.
        let mut fc = controller_with(vec![text_field("name", "ab", Some(2))], Some("name"));
        let ev = fc.handle_cached(&UiEvent::CharTyped(' '), RECT);
        assert_eq!(
            ev,
            FormControllerEvent::FormAction(FormEvent::TextInputChanged {
                id: WidgetId::new("name"),
                value: "ab ".to_string(),
            })
        );
    }

    #[test]
    fn disabled_first_button_item_focus_starts_on_the_second() {
        let mut fc = controller_with(
            vec![button_row_field("row", &[("a", true), ("b", false)])],
            Some("row"),
        );
        let ev = fc.handle_cached(
            &key_event(Key::Named(NamedKey::Enter), Modifiers::default()),
            RECT,
        );
        assert_eq!(
            ev,
            FormControllerEvent::FormAction(FormEvent::ButtonClicked {
                id: WidgetId::new("b")
            }),
            "item-focus must skip the disabled first button and default to \
             the second, with no prior Right keypress needed"
        );
    }

    #[test]
    fn right_skips_a_disabled_middle_button() {
        let mut fc = controller_with(
            vec![button_row_field(
                "row",
                &[("a", false), ("b", true), ("c", false)],
            )],
            Some("row"),
        );
        fc.handle_cached(
            &key_event(Key::Named(NamedKey::Right), Modifiers::default()),
            RECT,
        );
        let ev = fc.handle_cached(
            &key_event(Key::Named(NamedKey::Enter), Modifiers::default()),
            RECT,
        );
        assert_eq!(
            ev,
            FormControllerEvent::FormAction(FormEvent::ButtonClicked {
                id: WidgetId::new("c")
            }),
            "Right from 'a' must land on 'c', skipping disabled 'b'"
        );
    }

    #[test]
    fn right_at_the_last_enabled_item_is_a_noop() {
        let mut fc = controller_with(
            vec![button_row_field("row", &[("a", false), ("b", false)])],
            Some("row"),
        );
        fc.handle_cached(
            &key_event(Key::Named(NamedKey::Right), Modifiers::default()),
            RECT,
        );
        let ev = fc.handle_cached(
            &key_event(Key::Named(NamedKey::Right), Modifiers::default()),
            RECT,
        );
        assert_eq!(
            ev,
            FormControllerEvent::Ignored,
            "no wrap: Right at the last item stays put"
        );
        let activated = fc.handle_cached(
            &key_event(Key::Named(NamedKey::Enter), Modifiers::default()),
            RECT,
        );
        assert_eq!(
            activated,
            FormControllerEvent::FormAction(FormEvent::ButtonClicked {
                id: WidgetId::new("b")
            }),
            "item-focus is still on 'b', not pushed past it"
        );
    }

    #[test]
    fn left_at_the_first_enabled_item_is_a_noop() {
        let mut fc = controller_with(
            vec![button_row_field("row", &[("a", false), ("b", false)])],
            Some("row"),
        );
        let ev = fc.handle_cached(
            &key_event(Key::Named(NamedKey::Left), Modifiers::default()),
            RECT,
        );
        assert_eq!(
            ev,
            FormControllerEvent::Ignored,
            "no wrap: Left at the first item stays put"
        );
    }

    #[test]
    fn ctrl_modified_right_falls_through_instead_of_moving_item_focus() {
        let mut fc = controller_with(
            vec![button_row_field("row", &[("a", false), ("b", false)])],
            Some("row"),
        );
        let ev = fc.handle_cached(&key_event(Key::Named(NamedKey::Right), ctrl()), RECT);
        assert_eq!(
            ev,
            FormControllerEvent::Ignored,
            "Ctrl+Right is a host accelerator candidate, not item navigation"
        );
        let activated = fc.handle_cached(
            &key_event(Key::Named(NamedKey::Enter), Modifiers::default()),
            RECT,
        );
        assert_eq!(
            activated,
            FormControllerEvent::FormAction(FormEvent::ButtonClicked {
                id: WidgetId::new("a")
            }),
            "item-focus must still be on the first item — Ctrl+Right never moved it"
        );
    }

    #[test]
    fn disabled_button_row_field_ignores_item_navigation_keys() {
        let mut fc = controller_with(
            vec![FormField {
                disabled: true,
                ..button_row_field("row", &[("a", false), ("b", false)])
            }],
            Some("row"),
        );
        let ev = fc.handle_cached(
            &key_event(Key::Named(NamedKey::Right), Modifiers::default()),
            RECT,
        );
        assert_eq!(ev, FormControllerEvent::Ignored);
    }

    #[test]
    fn tab_moves_focus_away_from_a_button_row_normally() {
        let mut fc = controller_with(
            vec![
                button_row_field("row", &[("a", false), ("b", false)]),
                toggle_field("t", false),
            ],
            Some("row"),
        );
        let ev = fc.handle_cached(
            &key_event(Key::Named(NamedKey::Tab), Modifiers::default()),
            RECT,
        );
        assert_eq!(
            ev,
            FormControllerEvent::FormAction(FormEvent::FocusChanged {
                id: WidgetId::new("t")
            }),
            "Tab moves between fields, never into an item"
        );
    }

    #[test]
    fn tab_into_a_button_row_seeds_item_focus_on_the_first_enabled_item() {
        let mut fc = controller_with(
            vec![
                toggle_field("t", false),
                button_row_field("row", &[("a", true), ("b", false)]),
            ],
            Some("t"),
        );
        fc.handle_cached(
            &key_event(Key::Named(NamedKey::Tab), Modifiers::default()),
            RECT,
        );
        // `move_focus` only emits `FocusChanged` — the app persists the new
        // `focused_field` itself (`primitives::form::contract`'s "Keyboard editing" doc
        // section) — so push it back before the next keystroke.
        fc.set_form(Form {
            focused_field: Some(WidgetId::new("row")),
            ..make_form_with(vec![
                toggle_field("t", false),
                button_row_field("row", &[("a", true), ("b", false)]),
            ])
        });
        let ev = fc.handle_cached(
            &key_event(Key::Named(NamedKey::Enter), Modifiers::default()),
            RECT,
        );
        assert_eq!(
            ev,
            FormControllerEvent::FormAction(FormEvent::ButtonClicked {
                id: WidgetId::new("b")
            }),
            "Tab onto the row must seed item-focus on 'b', skipping disabled 'a'"
        );
    }

    #[test]
    fn item_focus_resets_when_leaving_and_returning_to_a_toggle_group() {
        let fields = || {
            vec![
                toggle_group_field("toggles", &[("x", false), ("y", false)]),
                toggle_field("t", false),
            ]
        };
        let mut fc = controller_with(fields(), Some("toggles"));
        fc.handle_cached(
            &key_event(Key::Named(NamedKey::Right), Modifiers::default()),
            RECT,
        );
        // Tab away, then Tab back (BackTab), round-tripping through the app
        // persisting `focused_field` the same way `tab_into_a_button_row_...`
        // above does.
        fc.handle_cached(
            &key_event(Key::Named(NamedKey::Tab), Modifiers::default()),
            RECT,
        );
        fc.set_form(Form {
            focused_field: Some(WidgetId::new("t")),
            ..make_form_with(fields())
        });
        fc.handle_cached(
            &key_event(Key::Named(NamedKey::BackTab), Modifiers::default()),
            RECT,
        );
        fc.set_form(Form {
            focused_field: Some(WidgetId::new("toggles")),
            ..make_form_with(fields())
        });

        let ev = fc.handle_cached(
            &key_event(Key::Named(NamedKey::Enter), Modifiers::default()),
            RECT,
        );
        assert_eq!(
            ev,
            FormControllerEvent::FormAction(FormEvent::ToggleChanged {
                id: WidgetId::new("x"),
                value: true,
            }),
            "returning to the toggle group must reset item-focus to the \
             first item, not resume at 'y' from before Tab left"
        );
    }

    #[test]
    fn right_then_enter_activates_the_second_toggle_in_a_toggle_group() {
        let mut fc = controller_with(
            vec![toggle_group_field("toggles", &[("x", true), ("y", false)])],
            Some("toggles"),
        );
        fc.handle_cached(
            &key_event(Key::Named(NamedKey::Right), Modifiers::default()),
            RECT,
        );
        let ev = fc.handle_cached(
            &key_event(Key::Named(NamedKey::Enter), Modifiers::default()),
            RECT,
        );
        assert_eq!(
            ev,
            FormControllerEvent::FormAction(FormEvent::ToggleChanged {
                id: WidgetId::new("y"),
                value: true,
            }),
            "activating 'y' must flip its own value, not 'x's"
        );
    }

    #[test]
    fn enter_activates_the_focused_toolbar_button() {
        let mut fc = controller_with(
            vec![toolbar_field("tb", &[("reset", true), ("export", true)])],
            Some("tb"),
        );
        let ev = fc.handle_cached(
            &key_event(Key::Named(NamedKey::Enter), Modifiers::default()),
            RECT,
        );
        assert_eq!(
            ev,
            FormControllerEvent::FormAction(FormEvent::ToolbarButtonClicked {
                field_id: WidgetId::new("tb"),
                button_id: WidgetId::new("reset"),
            })
        );
    }

    #[test]
    fn right_skips_a_disabled_toolbar_button() {
        let mut fc = controller_with(
            vec![toolbar_field(
                "tb",
                &[("reset", true), ("broken", false), ("export", true)],
            )],
            Some("tb"),
        );
        fc.handle_cached(
            &key_event(Key::Named(NamedKey::Right), Modifiers::default()),
            RECT,
        );
        let ev = fc.handle_cached(
            &key_event(Key::Named(NamedKey::Enter), Modifiers::default()),
            RECT,
        );
        assert_eq!(
            ev,
            FormControllerEvent::FormAction(FormEvent::ToolbarButtonClicked {
                field_id: WidgetId::new("tb"),
                button_id: WidgetId::new("export"),
            }),
            "Right from 'reset' must land on 'export', skipping the \
             disabled ('enabled: false') 'broken' button"
        );
    }

    #[test]
    fn mouse_click_on_an_item_does_not_touch_item_focus_state() {
        // Mouse behaviour is unchanged: `form_click_event` resolves a click
        // straight to the clicked item's event without going through
        // `item_focus` at all.
        let form = make_form_with(vec![button_row_field("row", &[("a", false), ("b", false)])]);
        let ev = form_click_event(&form, &WidgetId::new("b"));
        assert_eq!(
            ev,
            FormEvent::ButtonClicked {
                id: WidgetId::new("b")
            }
        );
    }

    fn make_form_with(fields: Vec<FormField>) -> Form {
        Form {
            id: WidgetId::new("form"),
            fields,
            focused_field: None,
            scroll_offset: 0,
            has_focus: true,
            label_position: Default::default(),
            label_align: Default::default(),
            label_width: None,
            control_width: Default::default(),
            min_control_width: None,
            spacing: Default::default(),
        }
    }

    // ── Item-level keyboard focus: painted focus ring ───────────────────
    //
    // Drives a real backend through a minimal `AppLogic` wrapper — see
    // `field_rect_tests`'s doc comment above for why a hand-rolled
    // `Backend` mock won't do: the ring's rect (resolved via the
    // `item_bounds` of `DrawsPrimitives::layout_primitive`, same as
    // `Self::draw_item_focus_ring`'s doc describes) needs a backend with
    // a real layout algorithm. `ItemFocusRingApp` is backend-agnostic
    // (it only ever sees `&mut dyn Backend`), shared by the TUI and GTK
    // driver tests below it — the two backends disagree on what a
    // "reasonable" rect looks like (cells vs. device pixels), so each
    // picks its own.
    #[cfg(any(feature = "tui", feature = "gtk"))]
    struct ItemFocusRingApp {
        fc: FormController,
        rect: Rect,
    }

    #[cfg(any(feature = "tui", feature = "gtk"))]
    impl ItemFocusRingApp {
        fn new(buttons: &[(&str, bool)], rect: Rect) -> Self {
            Self::with_field(button_row_field("row", buttons), rect)
        }

        fn new_toolbar(buttons: &[(&str, bool)], rect: Rect) -> Self {
            Self::with_field(toolbar_field("row", buttons), rect)
        }

        fn with_field(field: FormField, rect: Rect) -> Self {
            let mut fc = FormController::new("row-fc".into());
            fc.set_form(Form {
                focused_field: Some(WidgetId::new("row")),
                ..make_form_with(vec![field])
            });
            fc.set_has_focus(true);
            Self { fc, rect }
        }
    }

    #[cfg(any(feature = "tui", feature = "gtk"))]
    impl crate::runner::AppLogic for ItemFocusRingApp {
        type AreaId = ();
        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            self.fc.render(backend, self.rect);
        }
        fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> crate::runner::Reaction {
            self.fc.handle(&event, backend, self.rect);
            crate::runner::Reaction::Redraw
        }
    }

    #[cfg(feature = "tui")]
    mod item_focus_ring_tui_tests {
        use super::*;
        use crate::tui::testing::{Modifier, TuiDriver};

        const ITEM_ROW_RECT: Rect = Rect::new(0.0, 0.0, 40.0, 3.0);

        fn driver(buttons: &[(&str, bool)]) -> TuiDriver<ItemFocusRingApp> {
            TuiDriver::new(ItemFocusRingApp::new(buttons, ITEM_ROW_RECT), 40, 3)
        }

        fn underlined_at(driver: &TuiDriver<ItemFocusRingApp>, needle: &str) -> bool {
            let bounds = driver
                .find_bounds(needle)
                .unwrap_or_else(|| panic!("{needle:?} not painted on screen"));
            driver
                .style_at(bounds.x as u16, bounds.y as u16)
                .is_some_and(|s| s.modifiers.contains(Modifier::UNDERLINED))
        }

        #[test]
        fn the_ring_starts_on_the_first_enabled_button() {
            let driver = driver(&[("Connect", false), ("Cancel", false)]);
            assert!(underlined_at(&driver, "Connect"));
            assert!(!underlined_at(&driver, "Cancel"));
        }

        #[test]
        fn right_moves_the_ring_to_the_second_button() {
            let mut driver = driver(&[("Connect", false), ("Cancel", false)]);
            driver.press_named(NamedKey::Right);
            assert!(
                !underlined_at(&driver, "Connect"),
                "the ring must leave the first button"
            );
            assert!(
                underlined_at(&driver, "Cancel"),
                "the ring must land on the second button"
            );
        }

        #[test]
        fn disabled_first_button_the_ring_starts_on_the_second() {
            let driver = driver(&[("Connect", true), ("Cancel", false)]);
            assert!(!underlined_at(&driver, "Connect"));
            assert!(underlined_at(&driver, "Cancel"));
        }

        #[test]
        fn right_moves_the_ring_between_toolbar_buttons_too() {
            let mut driver = TuiDriver::new(
                ItemFocusRingApp::new_toolbar(&[("Reset", true), ("Export", true)], ITEM_ROW_RECT),
                40,
                3,
            );
            assert!(underlined_at(&driver, "Reset"));
            driver.press_named(NamedKey::Right);
            assert!(
                !underlined_at(&driver, "Reset"),
                "the ring must leave the first toolbar button"
            );
            assert!(
                underlined_at(&driver, "Export"),
                "the ring must land on the second toolbar button"
            );
        }
    }

    // GTK: `Backend::draw_focus_ring` strokes the exact rect it's given
    // (`GtkBackend::draw_focus_ring`, via `surface_stroke_rect`) rather
    // than TUI's border-box/underline special case, so — unlike the TUI
    // tests above — this samples pixel colour instead of a cell
    // modifier: `theme.accent_fg` at the stroke's edge when the ring
    // paints there, the row's own background otherwise.
    #[cfg(feature = "gtk")]
    mod item_focus_ring_gtk_tests {
        use super::*;
        use crate::gtk::testing::GtkDriver;

        const ITEM_ROW_RECT: Rect = Rect::new(0.0, 0.0, 400.0, 120.0);

        fn driver(buttons: &[(&str, bool)]) -> GtkDriver<ItemFocusRingApp> {
            GtkDriver::new(ItemFocusRingApp::new(buttons, ITEM_ROW_RECT), 400, 120)
        }

        /// `needle`'s item's own rect (not its text glyph's, which sits
        /// well inside the item's padded background) — the same
        /// `DrawsPrimitives::layout_primitive` lookup
        /// `FormController::draw_item_focus_ring` uses, so the sample
        /// point agrees with what the ring actually outlines.
        fn item_rect(driver: &GtkDriver<ItemFocusRingApp>, item_id: &str) -> Rect {
            let app = driver.app();
            let form = app.fc.form().expect("form set").clone();
            let layout =
                crate::DrawsPrimitives::layout_primitive(driver.backend(), app.rect, &form).layout;
            let vf = layout
                .visible_fields
                .iter()
                .find(|vf| form.fields[vf.field_idx].id == WidgetId::new("row"))
                .expect("row field painted");
            let (_, r) = vf
                .item_bounds
                .iter()
                .find(|(id, _)| id.as_str() == item_id)
                .unwrap_or_else(|| panic!("item {item_id:?} not in item_bounds"));
            Rect::new(app.rect.x + r.x, app.rect.y + r.y, r.width, r.height)
        }

        /// Sample one pixel on the stroke along `item_id`'s left edge —
        /// `focus_ring_width` defaults to 2.0px (`Style::default`), and
        /// `GtkBackend::draw_focus_ring` strokes centred on the rect's
        /// own edge, so the rect's left-edge column, vertically centred,
        /// lands solidly inside the stroke band when the ring painted
        /// there.
        fn ring_painted_at(driver: &mut GtkDriver<ItemFocusRingApp>, item_id: &str) -> bool {
            let r = item_rect(driver, item_id);
            let theme = driver.backend().theme();
            let accent = (theme.accent_fg.r, theme.accent_fg.g, theme.accent_fg.b);
            let x = r.x.round().max(0.0) as i32;
            let y = (r.y + r.height / 2.0).round() as i32;
            driver.pixel(x, y) == accent
        }

        #[test]
        fn the_ring_starts_on_the_first_enabled_button() {
            let mut driver = driver(&[("Connect", false), ("Cancel", false)]);
            assert!(ring_painted_at(&mut driver, "Connect"));
            assert!(!ring_painted_at(&mut driver, "Cancel"));
        }

        #[test]
        fn right_moves_the_ring_to_the_second_button() {
            let mut driver = driver(&[("Connect", false), ("Cancel", false)]);
            driver.press_named(NamedKey::Right);
            assert!(
                !ring_painted_at(&mut driver, "Connect"),
                "the ring must leave the first button"
            );
            assert!(
                ring_painted_at(&mut driver, "Cancel"),
                "the ring must land on the second button"
            );
        }

        #[test]
        fn disabled_first_button_the_ring_starts_on_the_second() {
            let mut driver = driver(&[("Connect", true), ("Cancel", false)]);
            assert!(!ring_painted_at(&mut driver, "Connect"));
            assert!(ring_painted_at(&mut driver, "Cancel"));
        }

        #[test]
        fn right_moves_the_ring_between_toolbar_buttons_too() {
            let mut driver = GtkDriver::new(
                ItemFocusRingApp::new_toolbar(&[("Reset", true), ("Export", true)], ITEM_ROW_RECT),
                400,
                120,
            );
            assert!(ring_painted_at(&mut driver, "Reset"));
            driver.press_named(NamedKey::Right);
            assert!(
                !ring_painted_at(&mut driver, "Reset"),
                "the ring must leave the first toolbar button"
            );
            assert!(
                ring_painted_at(&mut driver, "Export"),
                "the ring must land on the second toolbar button"
            );
        }
    }
}
