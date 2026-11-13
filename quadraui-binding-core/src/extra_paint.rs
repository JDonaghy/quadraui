//! The `ExtraPaint` trait, one impl per additive primitive, and the
//! `extra_primitives!` table decoding each one — the paint/hit-test and
//! wire-decode logic for every primitive painted outside the six fixed
//! slots (`Button`/`TextDisplay`/`List`/`Tree`/`DataTable`/`TabBar`).
//! Registering a further additive primitive needs exactly one impl here
//! plus one appended line in the `extra_primitives!` table at the bottom
//! of this file — no enum variant, no central match arm, and no second
//! decode registry in a binding to keep in lockstep with it.

use std::rc::Rc;

use quadraui::{
    render_markdown_to_styled, ActivityBar, Backend, BoardModel, Canvas, Chart, ComboBox,
    CommandCenter, CommandLine, CompletionItemMeasure, DatePicker, Decoration, DiffView, Editor,
    FieldKind, FindReplacePanel, Form, FormField, Image, InteractionState, LabelPosition, MenuBar,
    MessageList, Minimap, MultiSectionView, NumberInput, NumberInputHit, Palette, PipelineView,
    Point, ProgressBar, RadioGroup, Rect, RenderedMarkdown, ScrollAxis, ScrollView, Scrollbar,
    Spinner, StatusBar, StatusBarHit, StyledText, Switch, SwitchHit, Terminal, TextDisplay,
    TextDisplayLine, TextInput, TextRegion, Theme, Toolbar, ToolbarButton, WidgetId,
};

/// Paints one additive primitive descriptor painted outside the six fixed
/// slots above. Implemented once per primitive type (see the `impl`s just
/// below) so that registering a kind in a binding's own `EXTRA_PRIMITIVES`
/// decode table never needs a central match arm.
///
/// [`ExtraPaint::id`] is mechanical (`self.id`) on every impl and backs the
/// [`crate::hit::PaintedHit::Extra`] entry pushed for this descriptor.
/// [`ExtraPaint::hit_test`] and [`ExtraPaint::tab_stop`] default to "not
/// interactive", which most additive primitives keep; [`quadraui::Switch`]
/// and [`quadraui::TextInput`] override both to
/// route pointer and keyboard focus through their own `layout`/`hit_test`.
/// This trait only gets a hit/focus to the binding's own event handler with
/// `widget` set — the per-kind semantic event synthesis that turns it into a
/// `FormEvent` (toggling the switch's value, editing the text input, ...)
/// lives in `semantic_event_for_hit`, `focus_scoped_key_event`,
/// `text_input_key_event` and `text_input_insert_event`, below.
pub trait ExtraPaint: std::any::Any {
    fn paint(&self, backend: &mut dyn Backend, area: Rect);

    /// The widget id this descriptor paints under, used to populate
    /// [`crate::hit::PaintedHit::Extra`] without threading a leaf-tree
    /// address through.
    fn id(&self) -> WidgetId;

    /// Whether `point` lands on an interactive region of this descriptor,
    /// given the ABSOLUTE `area` it was painted into (the same frame
    /// `point` itself is in — no translation, matching `Button`/`Split`'s
    /// own layout convention). `false` by default: most additive
    /// primitives are pure display and a miss here lets
    /// [`crate::hit::hit_test_painted`] fall through to whatever painted
    /// behind them.
    fn hit_test(&self, area: Rect, point: Point) -> bool {
        let _ = (area, point);
        false
    }

    /// Whether this descriptor should be included in the binding's own tab-stop list.
    /// `false` by default, matching [`ExtraPaint::hit_test`]'s "not
    /// interactive unless overridden" default.
    fn tab_stop(&self) -> bool {
        false
    }

    /// Paints with real hover/pressed state, returning the absolute bounds
    /// and action id of each clickable child this descriptor just painted —
    /// a chrome bar's button ([`quadraui::Toolbar`]), segment
    /// ([`quadraui::StatusBar`]), or item ([`quadraui::ActivityBar`]).
    /// `crate::render::draw_bands`/`paint_leaf` call this (not
    /// [`ExtraPaint::paint`] directly) so the returned regions can be
    /// recorded into [`crate::hit::PaintedHit::Extra::chrome_hits`] and
    /// hit-tested ahead of [`ExtraPaint::hit_test`] on a later click —
    /// `hit_test`'s plain `bool` has no way to say *which* child a point
    /// landed on.
    ///
    /// Default: ignores `interaction` and forwards to [`ExtraPaint::paint`]
    /// with no clickable children — correct for every extra except
    /// `Toolbar`, `StatusBar` and `ActivityBar`, each of which overrides
    /// this to report its own per-child click targets.
    fn paint_chrome(
        &self,
        backend: &mut dyn Backend,
        area: Rect,
        interaction: &InteractionState,
    ) -> Vec<(Rect, WidgetId)> {
        let _ = interaction;
        self.paint(backend, area);
        Vec::new()
    }
}

/// Recovers the concrete descriptor (`Switch`, `TextInput`, ...) kept alive
/// behind the `Rc<dyn ExtraPaint>` in a [`crate::hit::PaintedHit::Extra`] entry, via
/// trait-upcasting coercion to [`ExtraPaint`]'s `std::any::Any` supertrait.
/// Used by [`crate::semantic::semantic_event_for_hit`] and the binding's own
/// event handler to route a hit/key to the matched extra's own semantics
/// (`Switch::click`, `TextEditor::apply`, ...) instead of leaving every
/// extra paint-only.
pub fn extra_as<T: 'static>(extra: &Rc<dyn ExtraPaint>) -> Option<&T> {
    (extra.as_ref() as &dyn std::any::Any).downcast_ref::<T>()
}

/// Recovers the plain [`TextInput`] driving the matched extra's own
/// typing/cursor/selection, whether `extra` is a bare standalone
/// `TextInput` or one of [`PasswordInput`]/[`TextArea`]'s `inner` — all
/// three reuse [`quadraui::TextEditor`] against an ordinary `TextInput`
/// buffer verbatim (masked or multi-line only at paint time, via each's
/// own `as_form`), so [`crate::text_input::text_input_click_event`]/
/// [`crate::text_input::text_input_insert_event`] can place a caret or
/// insert typed/pasted text on any of the three through one call site.
pub fn extra_as_text_input(extra: &Rc<dyn ExtraPaint>) -> Option<&TextInput> {
    if let Some(text_input) = extra_as::<TextInput>(extra) {
        return Some(text_input);
    }
    if let Some(password) = extra_as::<PasswordInput>(extra) {
        return Some(&password.inner);
    }
    if let Some(text_area) = extra_as::<TextArea>(extra) {
        return Some(&text_area.inner);
    }
    None
}

impl ExtraPaint for ProgressBar {
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        backend.draw_progress(area, self);
    }

    fn id(&self) -> WidgetId {
        self.id
    }
}

impl ExtraPaint for Spinner {
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        backend.draw_spinner(area, self);
    }

    fn id(&self) -> WidgetId {
        self.id
    }
}

impl ExtraPaint for Image {
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        backend.draw_image(area, self);
    }

    fn id(&self) -> WidgetId {
        self.id
    }
}

impl ExtraPaint for Form {
    /// The bare, non-interactive paint this trait's own `paint` always
    /// does for every extra — `crate::engine::InteractionEngine::sync_forms`
    /// repaints on top of it with the real [`quadraui::compose::FormController`]
    /// state (scroll offset, in-progress edits, ...) once wired, the same
    /// "paint plain first, then repaint interactive on top" sequencing
    /// `crate::engine::InteractionEngine::sync_menu_bar` already uses for
    /// `"menu_bar"`.
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        backend.draw_form(area, self);
    }

    fn id(&self) -> WidgetId {
        self.id
    }

    /// Whole-area bounds check, like [`TextInput::hit_test`] above — a
    /// click's precise field-level routing is
    /// `crate::engine::InteractionEngine::handle`'s own `FormController`
    /// dispatch, which resolves it via `FormController`'s own internal
    /// layout rather than this trait's `bool`-only answer.
    fn hit_test(&self, area: Rect, point: Point) -> bool {
        area.contains(point)
    }

    fn tab_stop(&self) -> bool {
        true
    }
}

impl ExtraPaint for TextInput {
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        backend.draw_text_input(area, self);
    }

    fn id(&self) -> WidgetId {
        self.id
    }

    /// Whole-area bounds check: [`ExtraPaint::hit_test`] has no
    /// backend-metrics parameter to resolve a click to a specific
    /// character column, so this only ever decides whether a click landed
    /// on the input at all — `InteractionEngine::handle`'s own
    /// `HitDetail::ExtraBody` arm has `Backend::measure()` in hand at the
    /// click site and threads it through `text_input_click_event` for the
    /// metrics-aware part (resolving which character column, and placing
    /// the caret there).
    fn hit_test(&self, area: Rect, point: Point) -> bool {
        area.contains(point)
    }

    fn tab_stop(&self) -> bool {
        true
    }
}

impl ExtraPaint for RadioGroup {
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        backend.draw_radio_group(area, self);
    }

    fn id(&self) -> WidgetId {
        self.id
    }

    /// Whole-area bounds check: [`ExtraPaint::hit_test`]'s own signature
    /// has no parameter for the per-option `measure` closure
    /// [`quadraui::RadioGroup::layout`] needs (unlike
    /// [`NumberInput::hit_test`]'s fixed stepper column). That alone
    /// doesn't rule out resolving a click to a specific option —
    /// `InteractionEngine::handle`'s own click site has a real `backend`
    /// (and so real metrics) in hand, the same way it does for
    /// [`TextInput::hit_test`]'s equivalent limitation — it just isn't
    /// wired there yet, so a click still only focuses the whole group;
    /// `Up`/`Down`/`Left`/`Right` move the selection once focused via
    /// [`ExtraPaint::tab_stop`].
    fn hit_test(&self, area: Rect, point: Point) -> bool {
        area.contains(point)
    }

    fn tab_stop(&self) -> bool {
        true
    }
}

impl ExtraPaint for Switch {
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        backend.draw_switch(area, self);
    }

    fn id(&self) -> WidgetId {
        self.id
    }

    fn hit_test(&self, area: Rect, point: Point) -> bool {
        self.layout(area).hit_test(point) == SwitchHit::Body
    }

    fn tab_stop(&self) -> bool {
        true
    }
}

impl ExtraPaint for NumberInput {
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        let layout = self.layout(area);
        backend.draw_number_input(self, &layout);
    }

    fn id(&self) -> WidgetId {
        self.id
    }

    /// Precise, not whole-area, unlike [`TextInput::hit_test`] above:
    /// [`quadraui::NumberInput::layout`] needs no backend metrics (its
    /// stepper column is a fixed `STEPPER_WIDTH`), so the real
    /// increment/decrement/text-area regions are resolvable here, which is
    /// what lets a stepper click be routed precisely (see
    /// `crate::engine::InteractionEngine::handle`'s own `NumberInput`
    /// click arm) instead of only coarse click-to-focus.
    fn hit_test(&self, area: Rect, point: Point) -> bool {
        self.layout(area).hit_test(point) != NumberInputHit::Outside
    }

    fn tab_stop(&self) -> bool {
        true
    }
}

impl ExtraPaint for ComboBox {
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        // Standalone (non-`Form`-embedded) display: the field itself only
        // ever needs its own row, so `field_bounds` is the top row of
        // `area` while the full `area` is passed as the dropdown's
        // viewport -- otherwise `Completions::layout`'s placement math has
        // no room above or below the field to open the dropdown into and
        // always clips it to zero height. A single-row item height is
        // assumed since no measurement pass runs outside a real UI loop.
        // The dropdown itself only paints when `self.open` is true with a
        // non-empty filtered list, so a closed/default combo box renders
        // just its field regardless of these choices. "Row" is one line
        // scaled by `line_height`, the backend-native unit (cells on TUI,
        // pixels on native), so the field and its popup rows size correctly
        // on every backend instead of collapsing to one pixel on native.
        let line_height = backend.line_height();
        let field_bounds = Rect::new(area.x, area.y, area.width.max(line_height), line_height);
        let max_popup_height = (area.height - line_height).max(line_height);
        let layout = self.layout(
            field_bounds,
            area,
            area.width.max(line_height),
            max_popup_height,
            |_| CompletionItemMeasure::new(line_height),
        );
        backend.draw_combo_box(self, &layout);
    }

    fn id(&self) -> WidgetId {
        self.id
    }

    /// Whole-area bounds check, same limitation [`RadioGroup::hit_test`]
    /// documents: resolving a click to the field or a specific filtered
    /// dropdown item needs the backend's `line_height` [`ExtraPaint::paint`]
    /// uses to build [`quadraui::ComboBox::layout`]'s `field_bounds`/
    /// `measure_item`, which this trait's `hit_test` has no parameter for
    /// — `InteractionEngine::handle` has that `backend` in hand at the
    /// click site, same as it does for `TextInput`, so this is not wired
    /// yet rather than impossible. Typing, `Up`/`Down`/`Enter`/`Escape`
    /// still work once focused via [`ExtraPaint::tab_stop`].
    fn hit_test(&self, area: Rect, point: Point) -> bool {
        area.contains(point)
    }

    fn tab_stop(&self) -> bool {
        true
    }
}

impl ExtraPaint for DatePicker {
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        // Standalone (non-`Form`-embedded) display: derive a 7-column,
        // 6-row month grid plus a one-row header (and a one-row time field
        // when `self.time` is set), clipped to the given area. The header
        // and time rows are one line each, scaled by `line_height` (the
        // backend-native unit: cells on TUI, pixels on native) so they size
        // correctly on every backend instead of collapsing to one pixel on
        // native; the grid rows are then whatever is left over divided by
        // six, floored at one line, so a date picker painted into a band
        // shorter than 7 lines deliberately overflows the band rather than
        // shrinking its cells below a usable size.
        let line_height = backend.line_height();
        let header_height = line_height;
        let time_height = if self.time.is_some() {
            line_height
        } else {
            0.0
        };
        let cell_width = (area.width / 7.0).max(line_height);
        let cell_height = ((area.height - header_height - time_height) / 6.0).max(line_height);
        let layout = self.layout(
            area.x,
            area.y,
            cell_width,
            cell_height,
            header_height,
            time_height,
        );
        backend.draw_date_picker(self, &layout);
    }

    fn id(&self) -> WidgetId {
        self.id
    }

    /// Whole-area bounds check, same limitation [`RadioGroup::hit_test`]
    /// documents: resolving a click to a prev/next control or a specific
    /// day cell needs the backend's `line_height` [`ExtraPaint::paint`]
    /// uses to derive the grid's `cell_width`/`cell_height`, which this
    /// trait's `hit_test` has no parameter for — not wired yet rather than
    /// impossible, same caveat. `Left`/`Right`/`Up`/`Down`/`PageUp`/
    /// `PageDown`/`Enter` navigation still works once focused via
    /// [`ExtraPaint::tab_stop`].
    fn hit_test(&self, area: Rect, point: Point) -> bool {
        area.contains(point)
    }

    fn tab_stop(&self) -> bool {
        true
    }
}

/// Wraps `kind` as the sole field of a throwaway single-field [`Form`]
/// sharing `id` with both the form and its one field — the bridge every
/// standalone `"checkbox"`/`"slider"`/`"select"`/`"password"`/`"textarea"`
/// leaf below uses to paint through [`quadraui::Backend::draw_form`]
/// rather than a second, leaf-specific rasteriser: a `Form`-embedded field
/// of the same [`FieldKind`] paints through the exact same backend call,
/// so there is nothing for any of these leaves to duplicate. The label
/// column is hidden form-wide (there is only ever one field, with no
/// label of its own) so the leaf's whole area goes to the control, the
/// same "just the control, no label gutter" footprint
/// [`quadraui::Switch`]/[`quadraui::NumberInput`]/[`quadraui::ComboBox`]
/// already have as bare standalone extras.
fn single_field_form(id: WidgetId, kind: FieldKind, disabled: bool, focused: bool) -> Form {
    Form {
        id,
        fields: vec![FormField {
            id,
            label: StyledText::plain(""),
            kind,
            hint: StyledText::plain(""),
            disabled,
            validation: None,
            label_position: Some(LabelPosition::Hidden),
            label_align: None,
            label_width: None,
            tab_index: None,
            control_width: None,
            min_control_width: None,
            row_align: None,
        }],
        focused_field: if focused { Some(id) } else { None },
        scroll_offset: 0,
        has_focus: focused,
        label_position: LabelPosition::Hidden,
        label_align: Default::default(),
        label_width: None,
        control_width: Default::default(),
        min_control_width: None,
        spacing: Default::default(),
    }
}

/// Mirrors [`quadraui::primitives::form`]'s own private
/// `slider_default_step` — the standalone leaf's wire default needs the
/// identical `1.0`, since it decodes straight into a [`FieldKind::Slider`]
/// via [`single_field_form`].
fn slider_default_step() -> f32 {
    1.0
}

/// A standalone boolean toggle — the layout-leaf lift of
/// [`FieldKind::Toggle`], reached via the `"checkbox"` extra kind. Paints
/// and whole-area hit-tests through [`single_field_form`] (see its doc);
/// [`crate::semantic::checkbox_click_event`]/
/// [`crate::semantic::checkbox_key_event`] supply the Space/Enter/click
/// semantics `Form` itself has no generic keyboard handling for on a bare
/// `Toggle` field (only `FormController`'s mouse `click_inner` flips one
/// today), so this leaf is keyboard-operable in a way a `Form`-embedded
/// `Toggle` field, used directly, is not yet.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct Checkbox {
    pub id: WidgetId,
    #[serde(default)]
    pub value: bool,
    #[serde(default)]
    pub disabled: bool,
    #[serde(default)]
    pub focused: bool,
}

impl Checkbox {
    pub(crate) fn as_form(&self) -> Form {
        single_field_form(
            self.id,
            FieldKind::Toggle { value: self.value },
            self.disabled,
            self.focused,
        )
    }
}

impl ExtraPaint for Checkbox {
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        backend.draw_form(area, &self.as_form());
    }

    fn id(&self) -> WidgetId {
        self.id
    }

    fn hit_test(&self, area: Rect, point: Point) -> bool {
        area.contains(point)
    }

    fn tab_stop(&self) -> bool {
        true
    }
}

/// A standalone numeric slider — the layout-leaf lift of
/// [`FieldKind::Slider`], reached via the `"slider"` extra kind. Paints
/// through [`single_field_form`] (see its doc);
/// [`crate::semantic::slider_click_event`]/[`crate::semantic::slider_key_event`]
/// supply Left/Right/Up/Down/Home/End stepping and a click-to-track-position
/// mapping, since (like [`Checkbox`] above) `Form` itself has no keyboard
/// handling for a bare `Slider` field.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct Slider {
    pub id: WidgetId,
    pub value: f32,
    pub min: f32,
    pub max: f32,
    #[serde(default = "slider_default_step")]
    pub step: f32,
    #[serde(default)]
    pub disabled: bool,
    #[serde(default)]
    pub focused: bool,
}

impl Slider {
    pub(crate) fn as_form(&self) -> Form {
        single_field_form(
            self.id,
            FieldKind::Slider {
                value: self.value,
                min: self.min,
                max: self.max,
                step: self.step,
            },
            self.disabled,
            self.focused,
        )
    }

    /// Clamps `raw` to `[self.min, self.max]` without
    /// [`f32::clamp`]'s panic on an inverted `min > max` — a wire payload
    /// controls both bounds, so a host typo inverting them degrades to a
    /// well-defined (if useless) clamp instead of a panic that would take
    /// the whole render down.
    pub(crate) fn clamp(&self, raw: f32) -> f32 {
        raw.max(self.min).min(self.max)
    }
}

impl ExtraPaint for Slider {
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        backend.draw_form(area, &self.as_form());
    }

    fn id(&self) -> WidgetId {
        self.id
    }

    fn hit_test(&self, area: Rect, point: Point) -> bool {
        area.contains(point)
    }

    fn tab_stop(&self) -> bool {
        true
    }
}

/// A standalone single-choice option list — the layout-leaf lift of
/// [`FieldKind::Dropdown`], reached via the `"select"` extra kind. Paints
/// through [`single_field_form`] (see its doc);
/// [`crate::semantic::select_click_event`]/[`crate::semantic::select_key_event`]
/// move/commit `selected_idx` on Up/Down/Enter/Space and on click, mirroring
/// `FieldKind::Dropdown`'s own "no separate open step; a move commits
/// immediately" contract.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct Select {
    pub id: WidgetId,
    pub options: Vec<String>,
    #[serde(default)]
    pub selected_idx: usize,
    #[serde(default)]
    pub disabled: bool,
    #[serde(default)]
    pub focused: bool,
}

impl Select {
    pub(crate) fn as_form(&self) -> Form {
        single_field_form(
            self.id,
            FieldKind::Dropdown {
                options: self
                    .options
                    .iter()
                    .map(|o| StyledText::plain(o.clone()))
                    .collect(),
                selected_idx: self.selected_idx,
            },
            self.disabled,
            self.focused,
        )
    }
}

impl ExtraPaint for Select {
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        backend.draw_form(area, &self.as_form());
    }

    fn id(&self) -> WidgetId {
        self.id
    }

    fn hit_test(&self, area: Rect, point: Point) -> bool {
        area.contains(point)
    }

    fn tab_stop(&self) -> bool {
        true
    }
}

/// Mirrors [`quadraui::primitives::form`]'s own private
/// `password_default_mask`.
fn password_input_default_mask() -> char {
    '•'
}

/// A standalone masked single-line input — the layout-leaf lift of
/// [`FieldKind::PasswordInput`], reached via the `"password"` extra kind.
/// The real value lives in `inner` (a plain [`quadraui::TextInput`], always
/// kept to exactly one line — `Enter` commits rather than inserting `'\n'`
/// (via [`crate::semantic::password_key_event`]'s forward to
/// [`crate::text_input::text_input_key_event`]), and
/// [`crate::text_input::text_input_insert_event`] strips any `'\n'` out of
/// an IME commit or clipboard paste before it ever reaches `inner`), so
/// typing/cursor/selection otherwise reuse [`quadraui::TextEditor`]
/// verbatim rather than a second text-editing implementation; only
/// [`PasswordInput::as_form`]'s paint-time bridge differs from a bare
/// standalone `TextInput`, masking every character so the screen never
/// shows the plaintext — matching `FieldKind::PasswordInput`'s own "value
/// is stored in plaintext; only rendering is masked" contract.
#[derive(Debug, Clone)]
pub struct PasswordInput {
    pub inner: TextInput,
    pub mask_char: char,
}

impl PasswordInput {
    pub(crate) fn as_form(&self) -> Form {
        let line = self.inner.lines.first().map(String::as_str).unwrap_or("");
        let cursor = line
            .char_indices()
            .nth(self.inner.cursor_col)
            .map(|(b, _)| b)
            .unwrap_or(line.len());
        single_field_form(
            self.inner.id,
            FieldKind::PasswordInput {
                value: line.to_string(),
                placeholder: String::new(),
                cursor: Some(cursor),
                mask_char: self.mask_char,
            },
            false,
            self.inner.has_focus,
        )
    }
}

impl<'de> serde::Deserialize<'de> for PasswordInput {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(serde::Deserialize)]
        struct Wire {
            id: WidgetId,
            #[serde(default)]
            value: String,
            #[serde(default)]
            cursor: Option<usize>,
            #[serde(default)]
            has_focus: bool,
            #[serde(default = "password_input_default_mask")]
            mask_char: char,
        }
        let wire = Wire::deserialize(deserializer)?;
        let char_count = wire.value.chars().count();
        let cursor_col = wire.cursor.unwrap_or(char_count).min(char_count);
        Ok(PasswordInput {
            inner: TextInput::new(wire.id)
                .with_lines(vec![wire.value])
                .with_cursor_col(cursor_col)
                .with_has_focus(wire.has_focus),
            mask_char: wire.mask_char,
        })
    }
}

impl ExtraPaint for PasswordInput {
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        backend.draw_form(area, &self.as_form());
    }

    fn id(&self) -> WidgetId {
        self.inner.id
    }

    fn hit_test(&self, area: Rect, point: Point) -> bool {
        area.contains(point)
    }

    fn tab_stop(&self) -> bool {
        true
    }
}

/// Mirrors [`quadraui::primitives::form`]'s own private
/// `textarea_default_rows`.
fn text_area_default_rows() -> usize {
    3
}

/// A standalone multi-line input — the layout-leaf lift of
/// [`FieldKind::TextArea`], reached via the `"textarea"` extra kind. Like
/// [`PasswordInput`], `inner` is a plain [`quadraui::TextInput`] (already
/// multi-line), so [`crate::semantic::text_area_key_event`] reuses
/// [`quadraui::EditOp::from_key`] and
/// [`crate::text_input::apply_text_input_edit`] unchanged — `inner.lines`
/// *is* the value, with no bridging needed on the way in. Only the
/// paint-time bridge differs from a bare standalone `TextInput`: it
/// round-trips through a [`FieldKind::TextArea`] so the leaf paints
/// pixel-for-pixel as a `Form`-embedded `TextArea` field would, down to
/// `visible_rows`' height hint.
#[derive(Debug, Clone)]
pub struct TextArea {
    pub inner: TextInput,
    pub visible_rows: usize,
}

impl<'de> serde::Deserialize<'de> for TextArea {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(serde::Deserialize)]
        struct Wire {
            id: WidgetId,
            #[serde(default)]
            value: String,
            #[serde(default)]
            placeholder: Option<String>,
            #[serde(default)]
            cursor: Option<usize>,
            #[serde(default)]
            has_focus: bool,
            #[serde(default = "text_area_default_rows")]
            visible_rows: usize,
        }
        let wire = Wire::deserialize(deserializer)?;
        let lines: Vec<String> = wire.value.split('\n').map(str::to_string).collect();
        // `cursor` is a byte offset into the flat multi-line `value`
        // (`FieldKind::TextArea`'s own convention) — resolve it to a
        // `(line, col)` pair by walking line boundaries, the inverse of
        // `TextArea::as_form`'s own byte-offset computation.
        let (cursor_line, cursor_col) = {
            let mut remaining = wire.cursor.unwrap_or(0);
            let mut line_idx = 0;
            for (i, line) in lines.iter().enumerate() {
                let line_len = line.len();
                if remaining <= line_len {
                    let col = line
                        .char_indices()
                        .take_while(|(b, _)| *b < remaining)
                        .count();
                    line_idx = i;
                    remaining = col;
                    break;
                }
                remaining -= line_len + 1;
                line_idx = i;
            }
            (line_idx, remaining)
        };
        let mut inner = TextInput::new(wire.id)
            .with_lines(lines)
            .with_cursor_line(cursor_line)
            .with_cursor_col(cursor_col)
            .with_has_focus(wire.has_focus);
        if let Some(placeholder) = wire.placeholder {
            inner = inner.with_placeholder(placeholder);
        }
        Ok(TextArea {
            inner,
            visible_rows: wire.visible_rows,
        })
    }
}

impl TextArea {
    pub(crate) fn as_form(&self) -> Form {
        let value = self.inner.lines.join("\n");
        let mut cursor = 0usize;
        for line in &self.inner.lines[..self.inner.cursor_line.min(self.inner.lines.len())] {
            cursor += line.len() + 1;
        }
        if let Some(line) = self.inner.lines.get(self.inner.cursor_line) {
            cursor += line
                .char_indices()
                .nth(self.inner.cursor_col)
                .map(|(b, _)| b)
                .unwrap_or(line.len());
        }
        single_field_form(
            self.inner.id,
            FieldKind::TextArea {
                value,
                placeholder: self.inner.placeholder.clone().unwrap_or_default(),
                cursor: Some(cursor),
                visible_rows: self.visible_rows,
            },
            false,
            self.inner.has_focus,
        )
    }
}

impl ExtraPaint for TextArea {
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        backend.draw_form(area, &self.as_form());
    }

    fn id(&self) -> WidgetId {
        self.inner.id
    }

    fn hit_test(&self, area: Rect, point: Point) -> bool {
        area.contains(point)
    }

    fn tab_stop(&self) -> bool {
        true
    }
}

/// A scrollable, read-only markdown viewer — the layout-leaf binding
/// surface for the shared `quadraui::compose::markdown` adapter
/// ([`render_markdown_to_styled`]), reached via the `"markdown"` extra
/// kind. `source` is raw markdown text; [`Markdown::render`] (called fresh
/// every paint, same "cheap enough to redo" posture [`Select`]/[`Slider`]
/// already take with their own `as_form` bridge) turns it into the
/// [`TextDisplay`] this type paints through, so headings/bold/italic/code
/// spans/lists/links all reuse the one markdown→styled-lines renderer
/// instead of a second parser. `scroll_offset` mirrors
/// [`TextDisplay::scroll_offset`]'s own controlled-component contract (same
/// wire key, too) — the host owns it and feeds the updated value back in
/// on the next render after a scroll gesture. A wheel notch over the leaf
/// (its body or a link) is translated the same way one over a plain
/// `"text_display"` leaf is — see [`crate::drag::markdown_at`] and its call
/// site in [`crate::engine::InteractionEngine::handle`] — into
/// `UiEvent::TextDisplay(id, TextDisplayEvent::Scrolled { new_offset })`,
/// which the host applies back onto this field.
///
/// Links resolve to an exact clicked line through
/// [`ExtraPaint::paint_chrome`]'s per-link `chrome_hits` (the same
/// mechanism [`Toolbar`]'s buttons use), each keyed by
/// [`link_child_id`] and reported through
/// [`crate::semantic::semantic_event_for_hit`]'s `ExtraChrome` arm as
/// `UiEvent::RichTextLinkClicked` — the event `RichTextPopup`'s own link
/// click already uses, so a host handles both the same way. Hit
/// resolution stays line-grained (one link per line wins), matching
/// `TextDisplayHit::Line`'s own granularity — this type adds no
/// character-column hit-testing `TextDisplay` doesn't already have.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct Markdown {
    pub id: WidgetId,
    pub source: String,
    #[serde(default)]
    pub scroll_offset: usize,
}

/// Builds the `WidgetId` of the `idx`-th link inside `id`'s markdown
/// leaf — the chrome-hit id [`Markdown::paint_chrome`] pushes and
/// [`Markdown::link_url_for_child`] parses back.
fn link_child_id(id: WidgetId, idx: usize) -> WidgetId {
    WidgetId::new(format!("{}:md-link:{idx}", id.as_str()))
}

impl Markdown {
    /// Renders `source` through the shared markdown adapter and wraps the
    /// result in a [`TextDisplay`] sized to paint identically to a plain
    /// `"text_display"` leaf — [`ExtraPaint::paint`]/[`ExtraPaint::paint_chrome`]
    /// both call this rather than duplicating the conversion.
    fn render(&self, theme: &Theme) -> (TextDisplay, RenderedMarkdown) {
        let rendered = render_markdown_to_styled(&self.source, theme);
        let lines = rendered
            .lines
            .iter()
            .cloned()
            .map(|text| TextDisplayLine {
                spans: text.spans,
                decoration: Decoration::default(),
                timestamp: None,
            })
            .collect();
        let td = TextDisplay {
            id: self.id,
            lines,
            scroll_offset: self.scroll_offset,
            auto_scroll: false,
            max_lines: 0,
            has_focus: false,
            title: None,
            show_scrollbar: false,
        };
        (td, rendered)
    }

    /// Resolves a `paint_chrome`-reported child id back to the
    /// [`quadraui::RichTextLink`]-style URL it names, by re-deriving the
    /// same `source` → links mapping [`Markdown::render`] computed when
    /// the chrome hit was painted. Which theme the re-derive call passes is
    /// irrelevant here — [`render_markdown_to_styled`]'s `links`
    /// side-channel (which `(line, byte_range, url)` triples exist) never
    /// depends on colour, only `self.source`'s own text — so this needs
    /// no backend/theme in scope, unlike [`Markdown::render`] itself.
    pub(crate) fn link_url_for_child(&self, child: WidgetId) -> Option<String> {
        let prefix = format!("{}:md-link:", self.id.as_str());
        let idx: usize = child.as_str().strip_prefix(&prefix)?.parse().ok()?;
        let rendered = render_markdown_to_styled(&self.source, &Theme::default());
        rendered.links.get(idx).map(|(_, _, url)| url.clone())
    }

    /// `(total_lines, viewport_lines)` for this leaf's rendered
    /// `TextDisplay` at `area`, via [`Backend::text_display_layout`] — the
    /// same per-frame layout [`ExtraPaint::paint_chrome`] paints from, so a
    /// wheel notch's `max_offset` clamp agrees with what's actually on
    /// screen. `viewport_lines` is measured at a zero scroll offset (a
    /// shadow clone, painting nothing) the same way
    /// `crate::render::text_display_viewport_lines` measures a plain
    /// `"text_display"` leaf's own capacity, since a layout at the real
    /// offset can read short near the end of a long document.
    pub(crate) fn viewport_lines(&self, backend: &mut dyn Backend, area: Rect) -> (usize, usize) {
        let theme = backend.theme();
        let (td, _) = self.render(&theme);
        let total_lines = td.lines.len();
        let mut shadow = td;
        shadow.scroll_offset = 0;
        let viewport_lines = backend
            .text_display_layout(area, &shadow)
            .visible_lines
            .len();
        (total_lines, viewport_lines)
    }
}

impl ExtraPaint for Markdown {
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        let theme = backend.theme();
        let (td, _) = self.render(&theme);
        backend.draw_text_display(area, &td);
    }

    fn id(&self) -> WidgetId {
        self.id
    }

    fn hit_test(&self, area: Rect, point: Point) -> bool {
        area.contains(point)
    }

    /// Paints through [`TextDisplay`] (same as [`ExtraPaint::paint`]),
    /// then reports one clickable chrome region per visible line that
    /// carries a link — `crate::render::paint_leaf`/`draw_bands` call
    /// this instead of [`ExtraPaint::paint`] for every `Extra` leaf, so
    /// this is the one real paint path; [`ExtraPaint::paint`] above stays
    /// for trait completeness and direct (non-leaf) use.
    ///
    /// Link rects come from [`Backend::text_display_layout`], never a
    /// hand-rolled measure — it's the same per-row layout
    /// `draw_text_display` itself paints from (word-wrap on TUI, Pango
    /// wrap on GTK), so a chrome rect never drifts from where the backend
    /// actually drew that row the way a flat one-row-per-line guess would
    /// on any line long enough to wrap.
    fn paint_chrome(
        &self,
        backend: &mut dyn Backend,
        area: Rect,
        _interaction: &InteractionState,
    ) -> Vec<(Rect, WidgetId)> {
        let theme = backend.theme();
        let (td, rendered) = self.render(&theme);
        backend.draw_text_display(area, &td);
        if rendered.links.is_empty() {
            return Vec::new();
        }
        let layout = backend.text_display_layout(area, &td);
        layout
            .visible_lines
            .iter()
            .filter_map(|visible| {
                let link_idx = rendered
                    .links
                    .iter()
                    .position(|(line_idx, _, _)| *line_idx == visible.line_idx)?;
                let bounds = Rect::new(
                    area.x + visible.bounds.x,
                    area.y + visible.bounds.y,
                    visible.bounds.width,
                    visible.bounds.height,
                );
                Some((bounds, link_child_id(self.id, link_idx)))
            })
            .collect()
    }
}

impl ExtraPaint for Chart {
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        // Display-only: no hover/crosshair state is tracked by this
        // binding, so both are `None` and the returned `ChartLayout` (for
        // click/nearest-point routing) is discarded.
        backend.draw_chart(area, self, None, None);
    }

    fn id(&self) -> WidgetId {
        self.id
    }
}

impl ExtraPaint for MessageList {
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        backend.draw_message_list(area, self);
        // Register a selectable text region every frame so drag-select and
        // Ctrl-C copy work against the transcript. TUI ignores `lines` (it
        // reads the live cell buffer); pixel backends extract from it, so
        // `lines` must be the rows actually painted into `area` this frame:
        // `rows[scroll_top..]`, clipped to how many whole rows fit in
        // `area.height` at this frame's line height — matching
        // `Backend::draw_message_list`'s own row walk (TUI's
        // `draw_message_list`, GTK's `draw_message_list` free function) so
        // row `i` of `bounds` always lines up with `lines[i]`. Each row is
        // already one display row (wrapping happens at the call site, not
        // in this primitive), so unlike `TextDisplay` there's no wrap/
        // timestamp misalignment to worry about here.
        let line_height = backend.line_height();
        let visible_rows = if line_height > 0.0 {
            (area.height / line_height).floor() as usize
        } else {
            0
        };
        let lines = self
            .rows
            .iter()
            .skip(self.scroll_top)
            .take(visible_rows)
            .map(|row| row.text.clone())
            .collect();
        backend.register_text_region(TextRegion {
            id: self.id,
            bounds: area,
            lines,
        });
    }

    fn id(&self) -> WidgetId {
        self.id
    }
}

impl ExtraPaint for BoardModel {
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        backend.draw_board(area, self);
    }

    fn id(&self) -> WidgetId {
        self.id
    }
}

impl ExtraPaint for PipelineView {
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        backend.draw_pipeline_view(area, self);
    }

    fn id(&self) -> WidgetId {
        self.id
    }
}

impl ExtraPaint for Minimap {
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        backend.draw_minimap(area, self);
    }

    fn id(&self) -> WidgetId {
        self.id
    }
}

impl ExtraPaint for MultiSectionView {
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        backend.draw_multi_section_view(area, self);
    }

    fn id(&self) -> WidgetId {
        self.id
    }
}

impl ExtraPaint for DiffView {
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        backend.draw_diff_view(area, self);
    }

    fn id(&self) -> WidgetId {
        self.id
    }
}

impl ExtraPaint for ActivityBar {
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        self.paint_chrome(backend, area, &InteractionState::default());
    }

    fn id(&self) -> WidgetId {
        self.id
    }

    /// [`quadraui::Backend::draw_activity_bar`] takes a positional
    /// `hovered_idx: Option<usize>` — a flat index into
    /// [`quadraui::ActivityBarLayout::visible_items`], not a [`WidgetId`] —
    /// so `interaction.hovered()` is first resolved against a no-paint
    /// [`quadraui::Backend::activity_bar_layout`] call to recover that
    /// index before the real paint. Row spans
    /// ([`quadraui::ActivityBarRowHit::y_start`]/`y_end`) are relative to
    /// `area`'s own top edge, so they're shifted by `area.x`/`area.y`
    /// before being recorded — [`crate::hit::PaintedHit::Extra::chrome_hits`]
    /// is always in the ABSOLUTE frame [`ExtraPaint::hit_test`] uses.
    fn paint_chrome(
        &self,
        backend: &mut dyn Backend,
        area: Rect,
        interaction: &InteractionState,
    ) -> Vec<(Rect, WidgetId)> {
        let hovered_idx = interaction.hovered().and_then(|hovered_id| {
            backend
                .activity_bar_layout(area, self)
                .iter()
                .position(|row| row.id == *hovered_id)
        });
        // `ActivityBarRowHit` carries only `y_start`/`y_end` — no x/width of
        // its own — so `area.width` is the only width the `Backend` trait
        // exposes for this row; it matches what `draw_bands` actually fills
        // the row's background with (a full-band-width horizontal strip in
        // the fixed-slot layout), not a narrower vertical-strip assumption.
        backend
            .draw_activity_bar(area, self, hovered_idx)
            .into_iter()
            .map(|row| {
                (
                    Rect::new(
                        area.x,
                        area.y + row.y_start,
                        area.width,
                        row.y_end - row.y_start,
                    ),
                    row.id,
                )
            })
            .collect()
    }
}

impl ExtraPaint for Toolbar {
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        self.paint_chrome(backend, area, &InteractionState::default());
    }

    fn id(&self) -> WidgetId {
        self.id
    }

    /// [`quadraui::Backend::draw_toolbar_interactive`]'s own
    /// [`quadraui::ToolbarLayout::visible_items`] is already in the
    /// ABSOLUTE frame [`ExtraPaint::hit_test`] uses, so no translation is
    /// needed — unlike [`ActivityBar::paint_chrome`]'s bar-relative row
    /// spans. Disabled actions and non-action items (separators, labels)
    /// have no `action_id` or aren't `clickable`, so they never become a
    /// hit region, matching [`quadraui::ToolbarLayout::hit_test`]'s own
    /// skip rule.
    fn paint_chrome(
        &self,
        backend: &mut dyn Backend,
        area: Rect,
        interaction: &InteractionState,
    ) -> Vec<(Rect, WidgetId)> {
        backend
            .draw_toolbar_interactive(area, self, interaction)
            .visible_items
            .into_iter()
            .filter(|item| item.clickable)
            .filter_map(|item| item.action_id.map(|id| (item.bounds, id)))
            .collect()
    }

    /// A tab stop whenever at least one button could ever be
    /// [`Self::activate_focused`]'s target — matching `Switch`'s own
    /// "interactive, so focusable" rule. A toolbar with no enabled
    /// `Action` (all separators/labels, or every action disabled) is
    /// skipped, the same way an empty `List`/`Tree` still counts (unlike
    /// those, nothing here could ever be activated, so there is no reason
    /// to spend a tab stop on it).
    fn tab_stop(&self) -> bool {
        self.buttons
            .iter()
            .any(|button| matches!(button, ToolbarButton::Action { enabled: true, .. }))
    }
}

impl ExtraPaint for MenuBar {
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        backend.draw_menu_bar(area, self);
    }

    fn id(&self) -> WidgetId {
        self.id
    }
}

impl ExtraPaint for StatusBar {
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        self.paint_chrome(backend, area, &InteractionState::default());
    }

    fn id(&self) -> WidgetId {
        self.id
    }

    /// Unlike [`Toolbar::paint_chrome`], [`quadraui::StatusBarLayout::hit_regions`]
    /// are bar-LOCAL (relative to `area`'s own origin — see
    /// [`quadraui::Backend::draw_status_bar_interactive`]'s own doc), so
    /// each region is shifted by `area.x`/`area.y` before being recorded.
    fn paint_chrome(
        &self,
        backend: &mut dyn Backend,
        area: Rect,
        interaction: &InteractionState,
    ) -> Vec<(Rect, WidgetId)> {
        backend
            .draw_status_bar_interactive(area, self, interaction)
            .hit_regions
            .into_iter()
            .filter_map(|(rect, hit)| match hit {
                StatusBarHit::Segment(id) => Some((
                    Rect::new(area.x + rect.x, area.y + rect.y, rect.width, rect.height),
                    id,
                )),
                StatusBarHit::Empty => None,
            })
            .collect()
    }
}

impl ExtraPaint for CommandLine {
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        backend.draw_command_line(area, self);
    }

    fn id(&self) -> WidgetId {
        self.id
    }
}

impl ExtraPaint for CommandCenter {
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        backend.draw_command_center(area, self);
    }

    fn id(&self) -> WidgetId {
        self.id
    }
}

impl ExtraPaint for Palette {
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        self.paint_chrome(backend, area, &InteractionState::default());
    }

    fn id(&self) -> WidgetId {
        self.id
    }

    /// [`quadraui::Backend::draw_palette_with_state`] only tints the
    /// pinned create-action row — the one row in a `Palette` with a
    /// stable id, `palette.id` itself (see that method's own doc for
    /// why [`quadraui::PaletteItem`] rows stay out of `InteractionState`) —
    /// so the one chrome hit recorded here is that row, when present.
    /// Its bounds come from a no-paint [`quadraui::Backend::palette_layout`]
    /// call — the same layout `draw_palette_with_state` paints from —
    /// shifted into the ABSOLUTE frame [`ExtraPaint::hit_test`] uses,
    /// mirroring [`ActivityBar::paint_chrome`]'s own translation.
    fn paint_chrome(
        &self,
        backend: &mut dyn Backend,
        area: Rect,
        interaction: &InteractionState,
    ) -> Vec<(Rect, WidgetId)> {
        backend.draw_palette_with_state(area, self, interaction);
        backend
            .palette_layout(area, self)
            .create_bounds
            .map(|b| {
                vec![(
                    Rect::new(area.x + b.x, area.y + b.y, b.width, b.height),
                    self.id,
                )]
            })
            .unwrap_or_default()
    }
}

impl ExtraPaint for Scrollbar {
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        backend.draw_scrollbar(area, self);
    }

    fn id(&self) -> WidgetId {
        self.id
    }
}

impl ExtraPaint for Editor {
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        let _ = backend.draw_editor(area, self);
    }

    fn id(&self) -> WidgetId {
        self.id
    }

    /// Whole-area bounds check, same limitation [`TextInput::hit_test`]
    /// documents above: this trait has no backend-metrics parameter to
    /// resolve a click to a buffer line/column via
    /// [`quadraui::Editor::layout`]'s own `cell_width`/`line_height`
    /// arguments, so any click inside `area` counts as landing on the
    /// editor. A host that wants `Editor::layout(..).hit_test(..)`'s
    /// buffer-position/gutter/scrollbar detail re-derives it from the
    /// enriched `MouseDown`'s position plus its own metrics.
    fn hit_test(&self, area: Rect, point: Point) -> bool {
        area.contains(point)
    }

    fn tab_stop(&self) -> bool {
        true
    }
}

impl ExtraPaint for Terminal {
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        backend.draw_terminal(area, self);
    }

    fn id(&self) -> WidgetId {
        self.id
    }

    /// Whole-area bounds check — same shape as [`Editor::hit_test`]:
    /// resolving a click to a specific cell is [`quadraui::Terminal`]'s own
    /// `layout(..).hit_test(..)`, which a host calls itself once it has the
    /// enriched `MouseDown` position, since mouse/keyboard interaction is
    /// deliberately outside this paint-snapshot primitive (see the module
    /// doc on `quadraui::Terminal`). This still gives click-to-focus so the
    /// app can route subsequent key presses to the live PTY.
    fn hit_test(&self, area: Rect, point: Point) -> bool {
        area.contains(point)
    }

    fn tab_stop(&self) -> bool {
        true
    }
}

impl ExtraPaint for Canvas {
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        let _ = backend.draw_canvas(area, self);
    }

    fn id(&self) -> WidgetId {
        self.id
    }

    /// Matches [`quadraui::CanvasLayout::hit_test`]'s own "inside these
    /// bounds at all" answer (see [`quadraui::CanvasHit`]'s doc) — `Canvas`
    /// has no sub-regions of its own, so there is nothing finer to resolve
    /// here.
    fn hit_test(&self, area: Rect, point: Point) -> bool {
        area.contains(point)
    }
}

impl ExtraPaint for ScrollView {
    /// `ScrollView` carries no backend painter of its own (module doc: "no
    /// backend painter lives here" — a host clips/paints its own content
    /// inside the viewport this descriptor describes). The one thing this
    /// leaf still paints is the scrollbar indicator itself, via
    /// [`quadraui::ScrollView::scrollbar`], in a fixed `line_height`-wide
    /// strip along the scrolling axis's trailing edge — the same
    /// cell-width convention `Tree`'s/`List`'s own TUI vertical-scrollbar
    /// track uses. [`quadraui::ScrollView::scrollbar`] returns `None` when
    /// there's nothing to scroll, so a `ScrollView` whose content fits its
    /// viewport paints nothing at all.
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        let line_height = backend.line_height();
        let track = match self.axis {
            ScrollAxis::Vertical => Rect::new(
                area.x + area.width - line_height,
                area.y,
                line_height,
                area.height,
            ),
            ScrollAxis::Horizontal => Rect::new(
                area.x,
                area.y + area.height - line_height,
                area.width,
                line_height,
            ),
        };
        if let Some(scrollbar) = self.scrollbar(track, line_height) {
            backend.draw_scrollbar(track, &scrollbar);
        }
    }

    fn id(&self) -> WidgetId {
        self.id
    }

    /// Whole-area bounds check: a wheel `Scroll` or click landing anywhere
    /// inside the viewport counts as targeting this `ScrollView`, so the
    /// enriched event's `widget` resolves to it — a host applies
    /// `ScrollView::handle_wheel`/`handle_key` itself against the position/
    /// key it already receives on that enriched event.
    fn hit_test(&self, area: Rect, point: Point) -> bool {
        area.contains(point)
    }

    fn tab_stop(&self) -> bool {
        true
    }
}

impl ExtraPaint for FindReplacePanel {
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        backend.draw_find_replace(area, self);
    }

    /// `FindReplacePanel` carries no `id` field of its own — it is a
    /// singleton overlay anchored to the active editor group (module doc:
    /// "the inline find/replace panel that anchors at top-right of the
    /// active editor group"), so only one instance is ever live at a time,
    /// the same way the `"find_replace"` slot in a binding's own
    /// `EXTRA_PRIMITIVES` decode table admits only one active value. A
    /// fixed sentinel id is therefore enough to address it in
    /// [`crate::hit::PaintedHit::Extra`] without adding a field to the
    /// descriptor itself.
    fn id(&self) -> WidgetId {
        WidgetId::new("find_replace")
    }

    /// Whole-area bounds check, same limitation [`Editor::hit_test`]
    /// documents: resolving a click to one of [`quadraui::FindReplacePanel::hit_regions`]'s
    /// char-cell targets needs `content_origin`/`char_width`/`line_height`
    /// that this trait has no parameter for — [`quadraui::FindReplacePanel::hit_test`]
    /// is still available to a host that has those metrics in hand.
    fn hit_test(&self, area: Rect, point: Point) -> bool {
        area.contains(point)
    }

    fn tab_stop(&self) -> bool {
        true
    }
}

/// Counts its `literal` arguments at compile time — the array-length
/// companion `extra_primitives!` needs so [`EXTRA_PRIMITIVE_KINDS`]'s
/// size tracks the table below with no hand-maintained length to drift.
macro_rules! extra_primitive_count {
    () => (0usize);
    ($head:literal $(, $tail:literal)* $(,)?) => (1usize + extra_primitive_count!($($tail),*));
}

/// Generates [`EXTRA_PRIMITIVE_KINDS`] and [`decode_extra_primitive`] from
/// one `"kind" => Type` table — the single source of truth both the kind
/// list and its decode dispatch share, so a further additive primitive
/// needs exactly one appended line here (plus that type's own
/// [`ExtraPaint`] impl above) and never a second table to keep in
/// lockstep with it.
macro_rules! extra_primitives {
    ($( $kind:literal => $ty:ty ),+ $(,)?) => {
        /// The additive primitive kind names, in [`decode_extra_primitive`]'s
        /// dispatch order. [`crate::render::draw_bands`] iterates this
        /// (rather than the `extra` slot `HashMap` itself) so the paint
        /// order a frame produces is deterministic, independent of the
        /// map's iteration order.
        pub const EXTRA_PRIMITIVE_KINDS: [&str; extra_primitive_count!($($kind),+)] =
            [$($kind),+];

        /// Decodes one additive primitive descriptor, dispatched by `kind`
        /// through the same table [`EXTRA_PRIMITIVE_KINDS`] names — the
        /// core counterpart of a binding's own `EXTRA_PRIMITIVES` decode
        /// registry (`quadraui-py`'s `insert_extra`/`insert_progress`/...
        /// and its layout-tree leaf/render-frame decode all route through
        /// this). Returns a [`crate::DecodeError`] naming every registered
        /// kind when `kind` isn't one of them.
        pub fn decode_extra_primitive<'de, D: serde::Deserializer<'de>>(
            what: &str,
            kind: &str,
            deserializer: D,
        ) -> Result<Rc<dyn ExtraPaint>, crate::DecodeError> {
            match kind {
                $(
                    $kind => Ok(Rc::new(crate::decode_value::<$ty, D>(what, deserializer)?)
                        as Rc<dyn ExtraPaint>),
                )+
                _ => Err(crate::DecodeError::unknown_variant(
                    what,
                    "kind",
                    kind,
                    &EXTRA_PRIMITIVE_KINDS,
                )),
            }
        }
    };
}

extra_primitives! {
    "progress" => ProgressBar,
    "spinner" => Spinner,
    "image" => Image,
    "form" => Form,
    "text_input" => TextInput,
    "number_input" => NumberInput,
    "combo_box" => ComboBox,
    "date_picker" => DatePicker,
    "radio_group" => RadioGroup,
    "switch" => Switch,
    "chart" => Chart,
    "message_list" => MessageList,
    "board" => BoardModel,
    "pipeline_view" => PipelineView,
    "minimap" => Minimap,
    "multi_section_view" => MultiSectionView,
    "diff_view" => DiffView,
    "activity_bar" => ActivityBar,
    "toolbar" => Toolbar,
    "menu_bar" => MenuBar,
    "status_bar" => StatusBar,
    "command_line" => CommandLine,
    "command_center" => CommandCenter,
    "palette" => Palette,
    "scrollbar" => Scrollbar,
    "editor" => Editor,
    "terminal" => Terminal,
    "canvas" => Canvas,
    "scroll_view" => ScrollView,
    "find_replace" => FindReplacePanel,
    "checkbox" => Checkbox,
    "slider" => Slider,
    "select" => Select,
    "password" => PasswordInput,
    "textarea" => TextArea,
    "markdown" => Markdown,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hit::PaintedHit;
    use crate::layout::{LayoutLeaf, LeafDescriptor};
    use crate::render::paint_leaf;
    use quadraui::tui::testing::TuiDriver;
    use quadraui::{AppLogic, FormEvent, PaletteItem, Reaction, UiEvent};
    use std::cell::RefCell;

    /// Records every event an [`EventSink`] receives, in order, for a
    /// chrome-leaf test to assert against without any host-language
    /// callback in scope.
    #[derive(Default)]
    struct RecordingSink {
        dispatched: Vec<UiEvent>,
    }

    impl crate::engine::EventSink for RecordingSink {
        fn dispatch(
            &mut self,
            event: &UiEvent,
            _hit_detail: Option<&crate::hit::HitDetail>,
        ) -> Reaction {
            self.dispatched.push(event.clone());
            Reaction::Continue
        }
    }

    fn palette(create_label: Option<&str>) -> Palette {
        Palette {
            id: WidgetId::new("pal"),
            title: "Commands".into(),
            query: String::new(),
            query_cursor: 0,
            items: vec![PaletteItem {
                text: quadraui::StyledText::plain("open file"),
                detail: None,
                icon: None,
                match_positions: vec![],
                depth: 0,
                expandable: false,
                expanded: false,
            }],
            selected_idx: 0,
            scroll_offset: 0,
            total_count: 0,
            has_focus: true,
            show_query: true,
            create_label: create_label.map(|s| s.to_string()),
            preview: None,
            mode: Default::default(),
        }
    }

    struct PaletteChromeApp {
        palette: Palette,
        hits: RefCell<Vec<PaintedHit>>,
    }

    impl AppLogic for PaletteChromeApp {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            let leaf = LayoutLeaf {
                id: "pal-leaf".into(),
                descriptor: LeafDescriptor::Extra(Rc::new(self.palette.clone())),
                scroll_px: None,
            };
            let viewport = backend.viewport();
            let area = Rect::new(0.0, 0.0, viewport.width, viewport.height);
            let mut hits = Vec::new();
            let chrome = |_id| InteractionState::default();
            paint_leaf(backend, area, &leaf, &mut hits, &chrome);
            *self.hits.borrow_mut() = hits;
        }

        fn handle(&mut self, _event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
            Reaction::Continue
        }
    }

    fn chrome_hits_for(create_label: Option<&str>) -> Vec<(Rect, WidgetId)> {
        let app = PaletteChromeApp {
            palette: palette(create_label),
            hits: RefCell::new(Vec::new()),
        };
        let mut driver = TuiDriver::new(app, 40, 20);
        driver.render();
        let found = driver.app().hits.borrow().iter().find_map(|hit| match hit {
            PaintedHit::Extra { chrome_hits, .. } => Some(chrome_hits.clone()),
            _ => None,
        });
        found.expect("a Palette extra was painted")
    }

    /// With no pinned create row, there is nothing to register as a
    /// chrome hit — `chrome_hover`/`chrome_pressed` (and so
    /// `draw_palette_with_state`'s hover tint) never fire for a plain
    /// palette.
    #[test]
    fn no_create_label_means_no_chrome_hits() {
        assert_eq!(chrome_hits_for(None), Vec::new());
    }

    /// A pinned create row is recorded as one chrome hit keyed by
    /// `palette.id` — the same id [`Backend::draw_palette_with_state`]
    /// checks `interaction.is_hovered`/`is_pressed` against, so a
    /// py/node app that sets `create_label` gets hover for free the
    /// moment `InteractionEngine::handle`'s generic `chrome_hover_hit`
    /// bookkeeping (keyed off this exact `chrome_hits` entry) sees a
    /// `MouseMoved` land inside it.
    #[test]
    fn create_label_registers_one_chrome_hit_keyed_by_palette_id() {
        let hits = chrome_hits_for(Some("Create 'foo'"));
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].1, WidgetId::new("pal"));
    }

    /// A `Switch` placed directly as a `LayoutNode::Leaf` (not through the
    /// flat-slot `extra` map) paints via [`crate::render::paint_layout`] and
    /// a `MouseDown` landing on it round-trips through
    /// [`crate::engine::InteractionEngine::handle`] into the same
    /// `UiEvent::Form(.., FormEvent::ToggleChanged)` a slot-painted switch
    /// dispatches — proving the additive-primitive leaf path
    /// (`crate::layout::LeafDescriptor::Extra`, wired generically for every
    /// `EXTRA_PRIMITIVE_KINDS` entry) carries an interactive form control's
    /// semantic event all the way to the handler, not just its paint.
    #[test]
    fn switch_leaf_click_toggles_through_the_interaction_engine() {
        use crate::engine::InteractionEngine;
        use crate::layout::{LayoutContainer, LayoutNode, Spacing};
        use crate::render::paint_layout;
        use quadraui::{FormEvent, Modifiers, MouseButton};

        struct SwitchLeafApp {
            engine: InteractionEngine,
            sink: RefCell<RecordingSink>,
        }

        impl AppLogic for SwitchLeafApp {
            type AreaId = ();

            fn render(&self, backend: &mut dyn Backend, _area: ()) {
                let tree = LayoutNode::Row(LayoutContainer::new(
                    vec![LayoutNode::Leaf(LayoutLeaf {
                        id: "sw:1".into(),
                        descriptor: LeafDescriptor::Extra(Rc::new(Switch::new(WidgetId::new(
                            "sw:1",
                        )))),
                        scroll_px: None,
                    })],
                    Spacing::default(),
                ));
                let viewport = backend.viewport();
                let area = Rect::new(0.0, 0.0, viewport.width, viewport.height);
                let mut hits = Vec::new();
                paint_layout(backend, area, &tree, &mut hits, &|id| {
                    self.engine.chrome_interaction(id)
                });
                self.engine.set_hits(hits);
            }

            fn tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
                self.engine.tab_stops()
            }

            fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
                self.engine
                    .handle(event, backend, &mut *self.sink.borrow_mut())
            }
        }

        let app = SwitchLeafApp {
            engine: InteractionEngine::new(),
            sink: RefCell::new(RecordingSink::default()),
        };
        let mut driver = TuiDriver::new(app, 20, 5);

        let switch_area = {
            let hits = driver.app().engine.hit_map();
            hits.iter()
                .find_map(|hit| match hit {
                    PaintedHit::Extra { id, area, .. } if *id == WidgetId::new("sw:1") => {
                        Some(*area)
                    }
                    _ => None,
                })
                .expect("switch leaf should have painted an Extra hit")
        };

        driver.dispatch(UiEvent::MouseDown {
            widget: None,
            button: MouseButton::Left,
            position: Point::new(switch_area.x, switch_area.y),
            modifiers: Modifiers::default(),
        });

        let dispatched = &driver.app().sink.borrow().dispatched;
        assert!(
            dispatched.iter().any(|event| matches!(
                event,
                UiEvent::Form(id, FormEvent::ToggleChanged { value: true, .. })
                    if *id == WidgetId::new("sw:1")
            )),
            "expected a ToggleChanged event for the inline switch leaf, got {dispatched:?}"
        );
    }

    /// A `Toolbar` placed directly as a `LayoutNode::Leaf` paints through
    /// [`crate::render::paint_layout`]'s `Extra` arm, which calls
    /// [`ExtraPaint::paint_chrome`] (not just [`ExtraPaint::paint`]) and
    /// records its clickable buttons into [`crate::hit::PaintedHit::Extra::chrome_hits`]
    /// — exactly the same chrome-click path [`crate::render::draw_bands`]
    /// uses for a flat-slot `"toolbar"`. A `MouseDown` on the recorded
    /// button rect round-trips through [`crate::engine::InteractionEngine::handle`]'s
    /// `HitDetail::ExtraChrome` arm into the same
    /// `UiEvent::Toolbar(.., ToolbarEvent::ButtonClicked)` a slot-painted
    /// toolbar dispatches, proving the chrome-bar leaf path carries its
    /// semantic click all the way to the handler, not just its paint.
    #[test]
    fn toolbar_leaf_click_emits_button_clicked_through_the_interaction_engine() {
        use crate::engine::InteractionEngine;
        use crate::layout::{LayoutContainer, LayoutNode, Spacing};
        use crate::render::paint_layout;
        use quadraui::{Modifiers, MouseButton, ToolbarEvent};

        struct ToolbarLeafApp {
            engine: InteractionEngine,
            sink: RefCell<RecordingSink>,
        }

        impl AppLogic for ToolbarLeafApp {
            type AreaId = ();

            fn render(&self, backend: &mut dyn Backend, _area: ()) {
                let toolbar = Toolbar::new(WidgetId::new("toolbar:main")).with_buttons(vec![
                    ToolbarButton::Action {
                        id: WidgetId::new("toolbar:save"),
                        label: "Save".into(),
                        icon: None,
                        key_hint: None,
                        enabled: true,
                        is_active: false,
                        tooltip: String::new(),
                        spec: None,
                    },
                ]);
                let tree = LayoutNode::Row(LayoutContainer::new(
                    vec![LayoutNode::Leaf(LayoutLeaf {
                        id: "toolbar:main".into(),
                        descriptor: LeafDescriptor::Extra(Rc::new(toolbar)),
                        scroll_px: None,
                    })],
                    Spacing::default(),
                ));
                let viewport = backend.viewport();
                let area = Rect::new(0.0, 0.0, viewport.width, viewport.height);
                let mut hits = Vec::new();
                paint_layout(backend, area, &tree, &mut hits, &|id| {
                    self.engine.chrome_interaction(id)
                });
                crate::host::render_overlays(&self.engine, backend, hits);
            }

            fn tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
                self.engine.tab_stops()
            }

            fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
                self.engine
                    .handle(event, backend, &mut *self.sink.borrow_mut())
            }
        }

        let app = ToolbarLeafApp {
            engine: InteractionEngine::new(),
            sink: RefCell::new(RecordingSink::default()),
        };
        let mut driver = TuiDriver::new(app, 20, 5);

        let button_point = {
            let hits = driver.app().engine.hit_map();
            hits.iter()
                .find_map(|hit| match hit {
                    PaintedHit::Extra {
                        id, chrome_hits, ..
                    } if *id == WidgetId::new("toolbar:main") => chrome_hits
                        .first()
                        .map(|(rect, _)| Point::new(rect.x, rect.y)),
                    _ => None,
                })
                .expect("toolbar leaf should have painted a chrome hit for its button")
        };

        driver.dispatch(UiEvent::MouseDown {
            widget: None,
            button: MouseButton::Left,
            position: button_point,
            modifiers: Modifiers::default(),
        });

        let dispatched = &driver.app().sink.borrow().dispatched;
        assert!(
            dispatched.iter().any(|event| matches!(
                event,
                UiEvent::Toolbar(id, ToolbarEvent::ButtonClicked { id: button_id, .. })
                    if *id == WidgetId::new("toolbar:main")
                        && *button_id == WidgetId::new("toolbar:save")
            )),
            "expected a ButtonClicked event for the inline toolbar leaf, got {dispatched:?}"
        );
    }

    /// A `Markdown` leaf renders headings, bold/italic, an inline code
    /// span and a list through the shared `render_markdown_to_styled`
    /// adapter — asserted both on the raw markdown syntax having been
    /// *consumed* (every marker absent from the painted screen text) and
    /// on a `fg` distinct from plain body text for the code span, so a
    /// bypass that dumped `source` to the screen verbatim — which still
    /// contains every one of those substrings — fails this test. This
    /// proves the same renderer `push_turn_markdown` uses is reused rather
    /// than a second parser. A `MouseDown` on the link's painted row
    /// round-trips through
    /// [`crate::engine::InteractionEngine::handle`]'s `HitDetail::ExtraChrome`
    /// arm into `UiEvent::RichTextLinkClicked` — the same event
    /// `RichTextPopup`'s own link click already dispatches.
    #[test]
    fn markdown_leaf_renders_formatting_and_link_click_emits_rich_text_link_clicked() {
        use crate::engine::InteractionEngine;
        use crate::layout::{LayoutContainer, LayoutNode, Spacing};
        use crate::render::paint_layout;
        use quadraui::{Modifiers, MouseButton};

        struct MarkdownLeafApp {
            engine: InteractionEngine,
            sink: RefCell<RecordingSink>,
        }

        impl AppLogic for MarkdownLeafApp {
            type AreaId = ();

            fn render(&self, backend: &mut dyn Backend, _area: ()) {
                let markdown = Markdown {
                    id: WidgetId::new("md:1"),
                    source: "# Title\n**bold** *italic* `code`\n- item\n[docs](https://example.com/docs)".to_string(),
                    scroll_offset: 0,
                };
                let tree = LayoutNode::Row(LayoutContainer::new(
                    vec![LayoutNode::Leaf(LayoutLeaf {
                        id: "md:1".into(),
                        descriptor: LeafDescriptor::Extra(Rc::new(markdown)),
                        scroll_px: None,
                    })],
                    Spacing::default(),
                ));
                let viewport = backend.viewport();
                let area = Rect::new(0.0, 0.0, viewport.width, viewport.height);
                let mut hits = Vec::new();
                paint_layout(backend, area, &tree, &mut hits, &|id| {
                    self.engine.chrome_interaction(id)
                });
                self.engine.set_hits(hits);
            }

            fn tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
                self.engine.tab_stops()
            }

            fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
                self.engine
                    .handle(event, backend, &mut *self.sink.borrow_mut())
            }
        }

        let app = MarkdownLeafApp {
            engine: InteractionEngine::new(),
            sink: RefCell::new(RecordingSink::default()),
        };
        let mut driver = TuiDriver::new(app, 40, 10);
        driver.render();

        assert!(driver.screen_contains("Title"), "heading text missing");
        assert!(driver.screen_contains("bold"), "bold text missing");
        assert!(driver.screen_contains("italic"), "italic text missing");
        assert!(driver.screen_contains("code"), "code span text missing");
        assert!(driver.screen_contains("item"), "list item text missing");
        assert!(driver.screen_contains("docs"), "link text missing");

        // Every raw markdown marker must be gone from the painted screen
        // -- a renderer bypass that printed `source` verbatim would still
        // pass every `screen_contains` assertion above, since all of those
        // substrings also appear in the raw source text.
        assert!(
            !driver.screen_contains("# Title"),
            "heading marker should be stripped, not painted literally"
        );
        assert!(
            !driver.screen_contains("**bold**"),
            "bold marker should be stripped, not painted literally"
        );
        assert!(
            !driver.screen_contains("*italic*"),
            "italic marker should be stripped, not painted literally"
        );
        assert!(
            !driver.screen_contains("`code`"),
            "code-span marker should be stripped, not painted literally"
        );
        assert!(
            !driver.screen_contains("[docs]"),
            "link syntax should be stripped, not painted literally"
        );
        assert!(
            driver.screen_contains("\u{2022}"),
            "list item should paint a bullet marker"
        );

        // The adapter's own styling -- not just marker removal -- reaches
        // the screen: the code span's `fg` differs from plain body text's
        // (`Theme::accent_fg`, not the default foreground). `TextDisplay`'s
        // TUI rasteriser (`tui::text_display::draw_text_display`) paints
        // only `span.fg`/`span.bg`, not `span.bold`/`span.italic` (a
        // pre-existing TUI limitation of the primitive this leaf paints
        // through, not something this leaf can fix on its own), so bold/
        // italic have no distinct on-screen signal to assert beyond the
        // marker-stripping above.
        let (item_x, item_y) = driver
            .find("item")
            .expect("plain list text should be painted");
        let item_style = driver
            .style_at(item_x as u16, item_y as u16)
            .expect("style at plain list text");
        let (code_x, code_y) = driver.find("code").expect("code span should be painted");
        let code_style = driver
            .style_at(code_x as u16, code_y as u16)
            .expect("style at code span");
        assert_ne!(
            code_style.fg, item_style.fg,
            "code span should be coloured differently from plain body text"
        );

        let link_point = {
            let hits = driver.app().engine.hit_map();
            hits.iter()
                .find_map(|hit| match hit {
                    PaintedHit::Extra {
                        id, chrome_hits, ..
                    } if *id == WidgetId::new("md:1") => chrome_hits
                        .first()
                        .map(|(rect, _)| Point::new(rect.x, rect.y)),
                    _ => None,
                })
                .expect("markdown leaf should have painted a chrome hit for its link")
        };

        driver.dispatch(UiEvent::MouseDown {
            widget: None,
            button: MouseButton::Left,
            position: link_point,
            modifiers: Modifiers::default(),
        });

        let dispatched = &driver.app().sink.borrow().dispatched;
        assert!(
            dispatched.iter().any(|event| matches!(
                event,
                UiEvent::RichTextLinkClicked(id, url)
                    if *id == WidgetId::new("md:1") && url == "https://example.com/docs"
            )),
            "expected a RichTextLinkClicked event for the inline markdown leaf, got {dispatched:?}"
        );
    }

    /// A wheel notch over a `Markdown` leaf's body must emit
    /// `UiEvent::TextDisplay(id, TextDisplayEvent::Scrolled { .. })` — the
    /// same translation a plain `"text_display"` leaf's own wheel gets
    /// (`InteractionEngine::handle`'s `over_markdown` arm), since the host
    /// owns `Markdown::scroll_offset` as a controlled component and has no
    /// other way to learn a scroll gesture happened. The document has more
    /// lines than the 10-row viewport fits, so there is something to
    /// scroll.
    #[test]
    fn markdown_leaf_wheel_scroll_emits_text_display_scrolled() {
        use crate::engine::InteractionEngine;
        use crate::layout::{LayoutContainer, LayoutNode, Spacing};
        use crate::render::paint_layout;
        use quadraui::{ScrollDelta, TextDisplayEvent};

        struct MarkdownScrollApp {
            engine: InteractionEngine,
            sink: RefCell<RecordingSink>,
        }

        impl AppLogic for MarkdownScrollApp {
            type AreaId = ();

            fn render(&self, backend: &mut dyn Backend, _area: ()) {
                let source = (0..20)
                    .map(|i| format!("line {i}"))
                    .collect::<Vec<_>>()
                    .join("\n");
                let markdown = Markdown {
                    id: WidgetId::new("md:scroll"),
                    source,
                    scroll_offset: 0,
                };
                let tree = LayoutNode::Row(LayoutContainer::new(
                    vec![LayoutNode::Leaf(LayoutLeaf {
                        id: "md:scroll".into(),
                        descriptor: LeafDescriptor::Extra(Rc::new(markdown)),
                        scroll_px: None,
                    })],
                    Spacing::default(),
                ));
                let viewport = backend.viewport();
                let area = Rect::new(0.0, 0.0, viewport.width, viewport.height);
                let mut hits = Vec::new();
                paint_layout(backend, area, &tree, &mut hits, &|id| {
                    self.engine.chrome_interaction(id)
                });
                self.engine.set_hits(hits);
            }

            fn tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
                self.engine.tab_stops()
            }

            fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
                self.engine
                    .handle(event, backend, &mut *self.sink.borrow_mut())
            }
        }

        let app = MarkdownScrollApp {
            engine: InteractionEngine::new(),
            sink: RefCell::new(RecordingSink::default()),
        };
        let mut driver = TuiDriver::new(app, 40, 10);
        driver.render();

        driver.dispatch(UiEvent::Scroll {
            widget: None,
            delta: ScrollDelta::new(0.0, -1.0),
            position: Point::new(1.0, 1.0),
        });

        let dispatched = &driver.app().sink.borrow().dispatched;
        assert!(
            dispatched.iter().any(|event| matches!(
                event,
                UiEvent::TextDisplay(id, TextDisplayEvent::Scrolled { new_offset: 1 })
                    if *id == WidgetId::new("md:scroll")
            )),
            "expected a TextDisplay(Scrolled {{ new_offset: 1 }}) event for a wheel notch \
             over the markdown leaf, got {dispatched:?}"
        );
    }

    /// A `MenuBar` placed directly as a `LayoutNode::Leaf` is synced into
    /// [`crate::engine::InteractionEngine`]'s own `MenuSystem` by
    /// [`crate::host::render_overlays`]'s `sync_menu_bar` call — the same
    /// helper every binding's render path (flat-slot or render-callback)
    /// runs after painting, so a leaf-painted bar gets the identical
    /// dropdown/keyboard-navigation state a slot-painted one does.
    /// `Alt+F` opens the one top-level menu this test builds, and `Enter`
    /// activates its sole leaf item, round-tripping through
    /// [`crate::engine::InteractionEngine::handle`] into the same
    /// `UiEvent::MenuActivated` a slot-painted menu bar dispatches.
    #[test]
    fn menu_bar_leaf_activation_reaches_the_interaction_engine() {
        use crate::engine::InteractionEngine;
        use crate::layout::{LayoutContainer, LayoutNode, Spacing};
        use crate::render::paint_layout;
        use quadraui::{ContextMenuItem, Key, Modifiers, NamedKey, StyledText};

        struct MenuBarLeafApp {
            engine: InteractionEngine,
            sink: RefCell<RecordingSink>,
        }

        impl AppLogic for MenuBarLeafApp {
            type AreaId = ();

            fn render(&self, backend: &mut dyn Backend, _area: ()) {
                let menu_bar = MenuBar {
                    id: WidgetId::new("menu_bar:main"),
                    items: vec![quadraui::MenuBarItem {
                        id: WidgetId::new("menu:file"),
                        label: "&File".into(),
                        disabled: false,
                        submenu: Some(vec![ContextMenuItem {
                            id: Some(WidgetId::new("menu:file:save")),
                            label: StyledText::plain("Save"),
                            ..Default::default()
                        }]),
                    }],
                    open_item: None,
                    focused_item: None,
                };
                let tree = LayoutNode::Row(LayoutContainer::new(
                    vec![LayoutNode::Leaf(LayoutLeaf {
                        id: "menu_bar:main".into(),
                        descriptor: LeafDescriptor::Extra(Rc::new(menu_bar)),
                        scroll_px: None,
                    })],
                    Spacing::default(),
                ));
                let viewport = backend.viewport();
                let area = Rect::new(0.0, 0.0, viewport.width, viewport.height);
                let mut hits = Vec::new();
                paint_layout(backend, area, &tree, &mut hits, &|id| {
                    self.engine.chrome_interaction(id)
                });
                crate::host::render_overlays(&self.engine, backend, hits);
            }

            fn tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
                self.engine.tab_stops()
            }

            fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
                self.engine
                    .handle(event, backend, &mut *self.sink.borrow_mut())
            }
        }

        let app = MenuBarLeafApp {
            engine: InteractionEngine::new(),
            sink: RefCell::new(RecordingSink::default()),
        };
        let mut driver = TuiDriver::new(app, 20, 5);

        driver.dispatch(UiEvent::KeyPressed {
            key: Key::Char('f'),
            modifiers: Modifiers {
                alt: true,
                ..Default::default()
            },
            repeat: false,
        });
        driver.dispatch(UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Enter),
            modifiers: Modifiers::default(),
            repeat: false,
        });

        let dispatched = &driver.app().sink.borrow().dispatched;
        assert!(
            dispatched.iter().any(|event| matches!(
                event,
                UiEvent::MenuActivated(id) if *id == WidgetId::new("menu:file:save")
            )),
            "expected a MenuActivated event for the inline menu_bar leaf, got {dispatched:?}"
        );
    }

    /// A reusable one-leaf [`AppLogic`] for the standalone
    /// `"checkbox"`/`"slider"`/`"select"`/`"password"`/`"textarea"` leaf
    /// tests below — the same shape
    /// [`switch_leaf_click_toggles_through_the_interaction_engine`] builds
    /// by hand, generalised over any [`ExtraPaint`] so each leaf's own test
    /// only needs to build the descriptor and assert on the result.
    struct SingleLeafApp {
        id: WidgetId,
        extra: Rc<dyn ExtraPaint>,
        engine: crate::engine::InteractionEngine,
        sink: RefCell<RecordingSink>,
    }

    impl AppLogic for SingleLeafApp {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            use crate::layout::{LayoutContainer, LayoutNode, Spacing};
            use crate::render::paint_layout;

            let tree = LayoutNode::Row(LayoutContainer::new(
                vec![LayoutNode::Leaf(LayoutLeaf {
                    id: self.id.as_str().to_string(),
                    descriptor: LeafDescriptor::Extra(self.extra.clone()),
                    scroll_px: None,
                })],
                Spacing::default(),
            ));
            let viewport = backend.viewport();
            let area = Rect::new(0.0, 0.0, viewport.width, viewport.height);
            let mut hits = Vec::new();
            paint_layout(backend, area, &tree, &mut hits, &|id| {
                self.engine.chrome_interaction(id)
            });
            self.engine.set_hits(hits);
        }

        fn tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
            self.engine.tab_stops()
        }

        fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
            self.engine
                .handle(event, backend, &mut *self.sink.borrow_mut())
        }
    }

    /// Renders `extra` as the sole leaf of a `20x5` driver and returns it
    /// already focused (via a leading `Tab`, the same mechanism
    /// [`tab_focuses_a_toolbar_and_enter_activates_its_focused_button`]
    /// (`crate::engine`'s own tests) uses) — every leaf below is its
    /// layout tree's only tab stop, so one `Tab` always lands focus on it.
    fn single_leaf_driver(id: WidgetId, extra: Rc<dyn ExtraPaint>) -> TuiDriver<SingleLeafApp> {
        let app = SingleLeafApp {
            id,
            extra,
            engine: crate::engine::InteractionEngine::new(),
            sink: RefCell::new(RecordingSink::default()),
        };
        let mut driver = TuiDriver::new(app, 20, 5);
        driver.press_named(quadraui::NamedKey::Tab);
        driver.app().sink.borrow_mut().dispatched.clear();
        driver
    }

    /// The absolute area the leaf `id` painted into this frame, read back
    /// from the engine's own hit map the same way
    /// [`switch_leaf_click_toggles_through_the_interaction_engine`] does.
    fn leaf_area(driver: &TuiDriver<SingleLeafApp>, id: WidgetId) -> Rect {
        driver
            .app()
            .engine
            .hit_map()
            .iter()
            .find_map(|hit| match hit {
                PaintedHit::Extra {
                    id: hit_id, area, ..
                } if *hit_id == id => Some(*area),
                _ => None,
            })
            .expect("leaf should have painted an Extra hit")
    }

    /// Render shows the unchecked glyph; a click toggles it (mouse); Space
    /// toggles it again (keyboard) — both routed through
    /// [`checkbox_click_event`]/[`checkbox_key_event`], both reaching the
    /// handler as [`FormEvent::ToggleChanged`]. `Checkbox::as_form` is what
    /// paints the `[ ]`/`[x]` glyph — the exact [`FieldKind::Toggle`]
    /// painter a `Form`-embedded field uses, never a second rasteriser.
    #[test]
    fn checkbox_leaf_renders_clicks_and_keys_toggle_through_the_interaction_engine() {
        let id = WidgetId::new("cb:1");
        let checkbox = || Checkbox {
            id,
            value: false,
            disabled: false,
            focused: true,
        };

        let mut driver = single_leaf_driver(id, Rc::new(checkbox()));
        assert!(driver.screen_contains("[ ]"), "{}", driver.screen());

        let area = leaf_area(&driver, id);
        driver.click(area.x, area.y);
        let dispatched = &driver.app().sink.borrow().dispatched;
        assert!(
            dispatched.iter().any(|event| matches!(
                event,
                UiEvent::Form(ev_id, FormEvent::ToggleChanged { value: true, .. })
                    if *ev_id == id
            )),
            "click should emit ToggleChanged{{value: true}}: {dispatched:?}"
        );

        // A fresh driver, same starting `value: false` — the engine only
        // ever sees the last-painted snapshot (the host echoes the new
        // value back on its next render, which this single-frame test
        // never does), so Space is exercised against the same starting
        // state the click above was, not the click's own outcome.
        let mut driver = single_leaf_driver(id, Rc::new(checkbox()));
        driver.type_char(' ');
        let dispatched = &driver.app().sink.borrow().dispatched;
        assert!(
            dispatched.iter().any(|event| matches!(
                event,
                UiEvent::Form(ev_id, FormEvent::ToggleChanged { value: true, .. })
                    if *ev_id == id
            )),
            "Space should emit ToggleChanged{{value: true}}: {dispatched:?}"
        );
    }

    /// Right/Up (keyboard) steps the value up by `step`; a click (mouse)
    /// maps the click's x-position within the track to a value — reaching
    /// the handler as [`FormEvent::SliderChanged`] either way, matching
    /// `FieldKind::Slider`'s own "click-and-drag on the track updates
    /// value" contract rather than a fixed one-`step` nudge.
    #[test]
    fn slider_leaf_renders_clicks_and_keys_step_through_the_interaction_engine() {
        let id = WidgetId::new("sl:1");
        let mut driver = single_leaf_driver(
            id,
            Rc::new(Slider {
                id,
                value: 0.0,
                min: 0.0,
                max: 10.0,
                step: 1.0,
                disabled: false,
                focused: true,
            }),
        );

        let area = leaf_area(&driver, id);
        // One cell short of the track's right edge, so the mapped fraction
        // is close to (but short of) `1.0` — proving the click landed where
        // the pointer was, not just "somewhere on the body".
        driver.click(area.x + area.width - 1.0, area.y);
        {
            let dispatched = &driver.app().sink.borrow().dispatched;
            assert!(
                dispatched.iter().any(|event| matches!(
                    event,
                    UiEvent::Form(ev_id, FormEvent::SliderChanged { value, .. })
                        if *ev_id == id && *value > 9.0 && *value < 10.0
                )),
                "a click near the track's right edge should emit SliderChanged{{value: ~9.5}}: {dispatched:?}"
            );
        }
        driver.app().sink.borrow_mut().dispatched.clear();

        driver.press_named(quadraui::NamedKey::Right);
        let dispatched = &driver.app().sink.borrow().dispatched;
        assert!(
            dispatched.iter().any(|event| matches!(
                event,
                UiEvent::Form(ev_id, FormEvent::SliderChanged { value, .. })
                    if *ev_id == id && (*value - 1.0).abs() < f32::EPSILON
            )),
            "Right should emit SliderChanged{{value: 1.0}}: {dispatched:?}"
        );
    }

    /// Down (keyboard) and a click (mouse) both move to the next option,
    /// committing immediately — reaching the handler as
    /// [`FormEvent::DropdownChanged`], mirroring `FieldKind::Dropdown`'s
    /// own "no separate open step" contract.
    #[test]
    fn select_leaf_renders_clicks_and_keys_move_through_the_interaction_engine() {
        let id = WidgetId::new("se:1");
        let select = || Select {
            id,
            options: vec!["One".to_string(), "Two".to_string(), "Three".to_string()],
            selected_idx: 0,
            disabled: false,
            focused: true,
        };

        let mut driver = single_leaf_driver(id, Rc::new(select()));
        assert!(driver.screen_contains("One"), "{}", driver.screen());

        let area = leaf_area(&driver, id);
        driver.click(area.x, area.y);
        let dispatched = &driver.app().sink.borrow().dispatched;
        assert!(
            dispatched.iter().any(|event| matches!(
                event,
                UiEvent::Form(ev_id, FormEvent::DropdownChanged { selected_idx: 1, .. })
                    if *ev_id == id
            )),
            "click should emit DropdownChanged{{selected_idx: 1}}: {dispatched:?}"
        );

        // A fresh driver at the same starting `selected_idx: 0` — see the
        // identical comment in the checkbox test above for why this
        // doesn't chain off the click's own outcome.
        let mut driver = single_leaf_driver(id, Rc::new(select()));
        driver.press_named(quadraui::NamedKey::Down);
        let dispatched = &driver.app().sink.borrow().dispatched;
        assert!(
            dispatched.iter().any(|event| matches!(
                event,
                UiEvent::Form(ev_id, FormEvent::DropdownChanged { selected_idx: 1, .. })
                    if *ev_id == id
            )),
            "Down should emit DropdownChanged{{selected_idx: 1}}: {dispatched:?}"
        );
    }

    /// Typing a character is the real interactive path for a standalone
    /// password leaf (a whole-area click only focuses it, the same coarse
    /// convention `ComboBox`/`DatePicker`/`RadioGroup` already have) —
    /// reaching the handler as [`FormEvent::TextInputChanged`] via
    /// [`password_key_event`]'s forward to [`text_input_key_event`]. The
    /// render assertion checks the masked glyph appears and the plaintext
    /// never does, proving [`PasswordInput::as_form`] actually masks
    /// rather than echoing `inner` verbatim.
    #[test]
    fn password_leaf_renders_masked_and_typing_reaches_the_interaction_engine() {
        let id = WidgetId::new("pw:1");
        let inner = TextInput::new(id)
            .with_lines(vec!["hi".to_string()])
            .with_cursor_col(2)
            .with_has_focus(true);
        let mut driver = single_leaf_driver(
            id,
            Rc::new(PasswordInput {
                inner,
                mask_char: '*',
            }),
        );
        assert!(driver.screen_contains("**"), "{}", driver.screen());
        assert!(!driver.screen_contains("hi"), "{}", driver.screen());

        driver.type_char('!');
        let dispatched = &driver.app().sink.borrow().dispatched;
        assert!(
            dispatched.iter().any(|event| matches!(
                event,
                UiEvent::Form(ev_id, FormEvent::TextInputChanged { value, .. })
                    if *ev_id == id && value == "hi!"
            )),
            "typing '!' should emit TextInputChanged{{value: \"hi!\"}}: {dispatched:?}"
        );
    }

    /// An IME-committed character (`CharTyped`) or a clipboard paste
    /// (`ClipboardPaste`) both reach a focused standalone password leaf
    /// through [`text_input_insert_event`] -- [`extra_as_text_input`]
    /// matches a [`PasswordInput`]'s `inner`, not only a bare `TextInput`.
    /// A newline embedded in the pasted text is stripped rather than
    /// splitting `inner` into a second line, keeping
    /// [`PasswordInput::as_form`]'s "always exactly one line" invariant
    /// even though this path (unlike `Enter`) has no commit special-case
    /// to fall back on.
    #[test]
    fn password_leaf_reaches_char_typed_and_clipboard_paste_and_strips_newlines() {
        let id = WidgetId::new("pw:2");
        let password = || {
            Rc::new(PasswordInput {
                inner: TextInput::new(id)
                    .with_lines(vec!["hi".to_string()])
                    .with_cursor_col(2)
                    .with_has_focus(true),
                mask_char: '*',
            })
        };

        let mut driver = single_leaf_driver(id, password());
        driver.dispatch(UiEvent::CharTyped('!'));
        let dispatched = &driver.app().sink.borrow().dispatched;
        assert!(
            dispatched.iter().any(|event| matches!(
                event,
                UiEvent::Form(ev_id, FormEvent::TextInputChanged { value, .. })
                    if *ev_id == id && value == "hi!"
            )),
            "CharTyped('!') should emit TextInputChanged{{value: \"hi!\"}}: {dispatched:?}"
        );

        // A fresh driver at the same starting `inner` -- see the identical
        // comment in the select test above for why this doesn't chain off
        // the previous dispatch's own outcome.
        let mut driver = single_leaf_driver(id, password());
        driver.dispatch(UiEvent::ClipboardPaste("yo\nthere".to_string()));
        let dispatched = &driver.app().sink.borrow().dispatched;
        assert!(
            dispatched.iter().any(|event| matches!(
                event,
                UiEvent::Form(ev_id, FormEvent::TextInputChanged { value, .. })
                    if *ev_id == id && value == "hiyothere" && !value.contains('\n')
            )),
            "pasting \"yo\\nthere\" should strip the newline: {dispatched:?}"
        );
    }

    /// Enter inserts a newline (not a commit, unlike a plain standalone
    /// `TextInput`/[`PasswordInput`]) — the one behavioural difference
    /// `FieldKind::TextArea`'s own doc draws against `FieldKind::TextInput`,
    /// given here by [`text_area_key_event`] never special-casing Enter the
    /// way [`password_key_event`]'s forward to [`text_input_key_event`]
    /// does. Reaches the handler as [`FormEvent::TextInputChanged`] with
    /// the embedded `\n`.
    #[test]
    fn text_area_leaf_renders_and_enter_inserts_a_newline_through_the_interaction_engine() {
        let id = WidgetId::new("ta:1");
        let inner = TextInput::new(id)
            .with_lines(vec!["hi".to_string()])
            .with_cursor_col(2)
            .with_has_focus(true);
        let mut driver = single_leaf_driver(
            id,
            Rc::new(TextArea {
                inner,
                visible_rows: 3,
            }),
        );
        assert!(driver.screen_contains("hi"), "{}", driver.screen());

        driver.press_named(quadraui::NamedKey::Enter);
        let dispatched = &driver.app().sink.borrow().dispatched;
        assert!(
            dispatched.iter().any(|event| matches!(
                event,
                UiEvent::Form(ev_id, FormEvent::TextInputChanged { value, .. })
                    if *ev_id == id && value == "hi\n"
            )),
            "Enter should emit TextInputChanged{{value: \"hi\\n\"}}: {dispatched:?}"
        );
    }

    /// A clipboard paste reaches a focused standalone textarea leaf the
    /// same way [`password_leaf_reaches_char_typed_and_clipboard_paste_and_strips_newlines`]
    /// proves it reaches a password leaf -- [`extra_as_text_input`]
    /// matches a [`TextArea`]'s `inner` too. Unlike the password case, a
    /// pasted newline is kept verbatim: `TextArea` has no single-line
    /// invariant to protect.
    #[test]
    fn text_area_leaf_reaches_clipboard_paste_with_embedded_newline() {
        let id = WidgetId::new("ta:2");
        let inner = TextInput::new(id)
            .with_lines(vec!["hi".to_string()])
            .with_cursor_col(2)
            .with_has_focus(true);
        let mut driver = single_leaf_driver(
            id,
            Rc::new(TextArea {
                inner,
                visible_rows: 3,
            }),
        );

        driver.dispatch(UiEvent::ClipboardPaste(" there\nfriend".to_string()));
        let dispatched = &driver.app().sink.borrow().dispatched;
        assert!(
            dispatched.iter().any(|event| matches!(
                event,
                UiEvent::Form(ev_id, FormEvent::TextInputChanged { value, .. })
                    if *ev_id == id && value == "hi there\nfriend"
            )),
            "pasting \" there\\nfriend\" should keep its newline: {dispatched:?}"
        );
    }
}
