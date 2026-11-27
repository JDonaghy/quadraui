//! The `ExtraPaint` trait, one impl per additive primitive, and the
//! `extra_primitives!` table decoding each one — the paint/hit-test and
//! wire-decode logic for every primitive painted outside the five fixed
//! slots (`TextDisplay`/`List`/`Tree`/`DataTable`/`TabBar`).
//! Registering a further additive primitive needs exactly one impl here
//! plus one appended line in the `extra_primitives!` table at the bottom
//! of this file — no enum variant, no central match arm, and no second
//! decode registry in a binding to keep in lockstep with it.

use std::rc::Rc;

use quadraui::primitive::DynWidget;
use quadraui::DrawsPrimitives;
use quadraui::{Backend, Form, MenuBar, NaturalSize, Point, Rect, TextField, TextInput, WidgetId};

/// Paints one additive primitive descriptor painted outside the five fixed
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
    /// `point` itself is in — no translation, matching `Split`'s
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

    /// Best-effort intrinsic size of this descriptor — the `ExtraPaint`
    /// counterpart of `crate::render::natural_size`'s per-kind arms for
    /// the five fixed leaf slots (`TextDisplay`/`List`/`Tree`/
    /// `DataTable`/`TabBar`). Consulted by
    /// [`crate::render::layout_container_rects`]'s non-`Stretch`
    /// `quadraui::Align` so a compact additive leaf (`Checkbox`,
    /// `Switch`, `ProgressBar`, `Image`, ...) sizes to its own content
    /// under `Align::Start`/`Center`/`End` instead of always filling the
    /// cross-axis extent — a `None` on either axis still degrades that
    /// axis to filling it, same as [`quadraui::NaturalSize`]'s own doc
    /// describes.
    ///
    /// Defaults to [`quadraui::NaturalSize::NONE`], correct for every
    /// extra with no single natural size to report: a viewport-filling
    /// primitive (`Editor`, `Canvas`, `ScrollView`, ...), a
    /// multi-cell grid (`DatePicker`), or a chrome bar meant to span
    /// its whole axis (`Toolbar`, `MenuBar`, ...). The
    /// impls below override it for the primitives with a genuinely
    /// compact or fixed-content shape.
    fn natural_size(&self, backend: &mut dyn Backend) -> NaturalSize {
        let _ = backend;
        NaturalSize::NONE
    }

    /// This descriptor's [`crate::scripted::ScriptedExtra`] facet, for an
    /// extra that is a script-defined widget — `None` by default, correct
    /// for every hand-built extra above, which has no script-side
    /// `on_event` for `hit.rs`/`semantic.rs` to forward a resolved region
    /// click or a focused key press to. Only
    /// `crate::scripted::ScriptedPrimitive<W>` overrides this, and does so
    /// generically (no `W` type parameter escapes that one `impl`) — see
    /// that type's own doc.
    fn as_scripted(&self) -> Option<&dyn crate::scripted::ScriptedExtra> {
        None
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
/// own `as_widget`), so [`crate::text_input::text_input_click_event`]/
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

impl ExtraPaint for Form {
    /// The bare, non-interactive paint this trait's own `paint` always
    /// does for every extra — `crate::engine::InteractionEngine::sync_forms`
    /// repaints on top of it with the real [`quadraui::compose::FormController`]
    /// state (scroll offset, in-progress edits, ...) once wired, the same
    /// "paint plain first, then repaint interactive on top" sequencing
    /// `crate::engine::InteractionEngine::sync_menu_bar` already uses for
    /// `"menu_bar"`.
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        quadraui::DrawsPrimitives::draw_primitive(backend, area, self);
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
        backend.draw_dyn(area, self, &());
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

    /// No natural width — a standalone text input fills whatever width
    /// its container gives it, same as `crate::render::natural_size`'s
    /// own `List`/`Tree`/`DataTable` arms. Height is `lines.len()` rows:
    /// unlike [`PasswordInput`]/[`TextField`] (always exactly one line
    /// by construction) a bare standalone `TextInput` leaf may hold
    /// several, the same count [`quadraui::TextInput::layout`] paints.
    fn natural_size(&self, backend: &mut dyn Backend) -> NaturalSize {
        let rows = self.lines.len().max(1) as f32;
        NaturalSize::new(None, Some(rows * backend.line_height()))
    }
}

impl ExtraPaint for TextField {
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        backend.draw_dyn(area, self, &());
    }

    fn id(&self) -> WidgetId {
        self.id
    }

    /// Whole-area bounds check, same limitation [`TextInput::hit_test`]'s
    /// doc documents: resolving a click to the text column vs. the clear
    /// button needs the backend metrics [`quadraui::TextField::layout`]
    /// takes, which this trait's `hit_test` has no parameter for — not
    /// wired to a precise [`quadraui::TextFieldHit`] yet, same as
    /// `TextInput`.
    fn hit_test(&self, area: Rect, point: Point) -> bool {
        area.contains(point)
    }

    fn tab_stop(&self) -> bool {
        true
    }

    /// Always exactly one line — [`quadraui::TextField`] has no
    /// multi-line mode, unlike a bare [`TextInput`]. No natural width,
    /// same reasoning as [`TextInput::natural_size`].
    fn natural_size(&self, backend: &mut dyn Backend) -> NaturalSize {
        NaturalSize::new(None, Some(backend.line_height()))
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
/// verbatim rather than a second text-editing implementation; it paints
/// as the core [`quadraui::PasswordInput`] ([`PasswordInput::as_widget`]),
/// masking every character so the screen never shows the plaintext.
#[derive(Debug, Clone)]
pub struct PasswordInput {
    pub inner: TextInput,
    pub mask_char: char,
}

impl PasswordInput {
    /// The core [`quadraui::PasswordInput`] this leaf paints as: `inner`'s
    /// one line and caret, masked with `mask_char`.
    pub(crate) fn as_widget(&self) -> quadraui::PasswordInput {
        let line = self.inner.lines.first().cloned().unwrap_or_default();
        let mut widget = quadraui::PasswordInput::new(self.inner.id);
        widget.cursor = Some(quadraui::behaviour::text_edit::col_to_byte(
            &line,
            self.inner.cursor_col,
        ));
        widget.value = line;
        widget.has_focus = self.inner.has_focus;
        widget.mask_char = self.mask_char;
        widget
    }
}

/// [`PasswordInput`]'s wire shape: the plaintext value and a caret byte
/// offset, which `Deserialize` turns into the one-line [`TextInput`].
#[derive(serde::Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "schema", schemars(rename = "PasswordInput"))]
struct PasswordInputWire {
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

#[cfg(feature = "schema")]
impl schemars::JsonSchema for PasswordInput {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        PasswordInputWire::schema_name()
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        PasswordInputWire::json_schema(generator)
    }
}

impl<'de> serde::Deserialize<'de> for PasswordInput {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = PasswordInputWire::deserialize(deserializer)?;
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
        let _ = backend.draw_dyn(area, &self.as_widget(), &());
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

    /// Always exactly one line: `Enter` commits, never inserts `'\n'`. No natural width, same reasoning as
    /// [`TextInput::natural_size`].
    fn natural_size(&self, backend: &mut dyn Backend) -> NaturalSize {
        NaturalSize::new(None, Some(backend.line_height()))
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
/// *is* the value, with no bridging needed on the way in. It paints as the
/// core [`quadraui::TextArea`] ([`TextArea::as_widget`]), `visible_rows`
/// rows tall.
#[derive(Debug, Clone)]
pub struct TextArea {
    pub inner: TextInput,
    pub visible_rows: usize,
}

/// [`TextArea`]'s wire shape: the whole value as one string with `'\n'`
/// line breaks and a caret byte offset.
#[derive(serde::Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "schema", schemars(rename = "TextArea"))]
struct TextAreaWire {
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

#[cfg(feature = "schema")]
impl schemars::JsonSchema for TextArea {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        TextAreaWire::schema_name()
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        TextAreaWire::json_schema(generator)
    }
}

impl<'de> serde::Deserialize<'de> for TextArea {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = TextAreaWire::deserialize(deserializer)?;
        let (cursor_line, cursor_col) = quadraui::behaviour::text_edit::byte_offset_to_line_col(
            &wire.value,
            wire.cursor.unwrap_or(0),
        );
        let lines: Vec<String> = wire.value.split('\n').map(str::to_string).collect();
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
    /// The core [`quadraui::TextArea`] this leaf paints as: `inner`'s
    /// lines joined, its caret as a byte offset.
    pub(crate) fn as_widget(&self) -> quadraui::TextArea {
        let value = self.inner.lines.join("\n");
        let mut widget = quadraui::TextArea::new(self.inner.id);
        widget.cursor = Some(quadraui::behaviour::text_edit::line_col_to_byte_offset(
            &value,
            self.inner.cursor_line,
            self.inner.cursor_col,
        ));
        widget.value = value;
        widget.placeholder = self.inner.placeholder.clone().unwrap_or_default();
        widget.has_focus = self.inner.has_focus;
        widget.visible_rows = self.visible_rows;
        widget
    }
}

impl ExtraPaint for TextArea {
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        let _ = backend.draw_dyn(area, &self.as_widget(), &());
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

    /// `visible_rows` rows tall — the field's own declared viewport
    /// height, independent of how many lines `inner` actually holds
    /// (scrolling, not growth, absorbs the difference). No natural
    /// width, same reasoning as [`TextInput::natural_size`].
    fn natural_size(&self, backend: &mut dyn Backend) -> NaturalSize {
        let rows = self.visible_rows.max(1) as f32;
        NaturalSize::new(None, Some(rows * backend.line_height()))
    }
}

impl ExtraPaint for MenuBar {
    /// Installed as the platform's own menu bar under a native menu
    /// style, painted otherwise — see [`MenuBar::present`].
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        let _ = self.present(backend, area);
    }

    fn id(&self) -> WidgetId {
        self.id
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
        /// through the same table [`EXTRA_PRIMITIVE_KINDS`] names — both
        /// the layout-tree leaf decode and the flat render-frame extra
        /// slot route through this. Returns a [`crate::DecodeError`]
        /// naming every registered kind when `kind` isn't one of them.
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

        /// Hands each [`EXTRA_PRIMITIVE_KINDS`] entry's descriptor type to
        /// `visitor`, in table order.
        pub fn visit_extra_primitive_kinds<V: quadraui::KindVisitor>(visitor: &mut V) {
            $( visitor.visit::<$ty>($kind); )+
        }

        /// [`decode_extra_primitive`] against an already-[`serde_json::Value`]
        /// descriptor body, routing an unrecognized key inside it through
        /// `diagnostics` the same way [`crate::wire::decode_value_checked`]
        /// does for a fixed leaf's own body. The layout-tree leaf decode
        /// (`crate::wire::layout_leaf_from_value`) and the flat render-frame
        /// extra slot (`crate::wire::extra_slot_entry`) both already hold a
        /// materialized `Value` and a session to report through, so they
        /// call this rather than [`decode_extra_primitive`].
        pub(crate) fn decode_extra_primitive_checked(
            diagnostics: &crate::wire::DecodeDiagnostics,
            what: &str,
            kind: &str,
            value: &serde_json::Value,
        ) -> Result<Rc<dyn ExtraPaint>, crate::DecodeError> {
            match kind {
                $(
                    $kind => Ok(Rc::new(crate::wire::decode_value_checked::<$ty>(
                        diagnostics, what, kind, value, &[],
                    )?) as Rc<dyn ExtraPaint>),
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
    "form" => Form,
    "text_input" => TextInput,
    "text_field" => TextField,
    "menu_bar" => MenuBar,
    "password" => PasswordInput,
    "textarea" => TextArea,
}

/// The `widget_kinds!`-registered kind strings this crate decodes into
/// [`crate::layout::LeafDescriptor::Widget`] rather than
/// [`crate::layout::LeafDescriptor::Extra`] — the [`DynWidget`] seam's own
/// kind table, parallel to [`EXTRA_PRIMITIVE_KINDS`] above but never
/// overlapping it (a kind lives in exactly one of the two tables). Each
/// entry here must also be a [`quadraui::widget_kinds!`] entry, so
/// [`quadraui::decode_widget`] actually knows how to decode it —
/// [`decode_widget_leaf`] is this table's own decode step, the `Widget`
/// counterpart of [`decode_extra_primitive_checked`].
///
/// `"icon_grid"`, `"cell_grid"`, `"switch"`, `"radio_group"`, `"button"`,
/// `"command_center"`, `"terminal"`, `"status_bar"`, `"activity_bar"`,
/// `"palette"`, `"context_menu"`, `"editor"`, `"minimap"`, `"image"`,
/// `"chart"`, `"canvas"`, `"scroll_view"`, `"board"`, `"pipeline_view"`,
/// `"diff_view"`, `"checkbox"`, `"slider"`, `"select"` and `"markdown"` are this table's
/// members: unlike every [`EXTRA_PRIMITIVE_KINDS`] entry,
/// none of [`quadraui::IconGrid`], [`quadraui::CellGrid`],
/// [`quadraui::Switch`], [`quadraui::RadioGroup`], [`quadraui::Button`],
/// [`quadraui::CommandCenter`], [`quadraui::Terminal`],
/// [`quadraui::StatusBar`], [`quadraui::ActivityBar`], [`quadraui::Palette`],
/// [`quadraui::ContextMenu`], [`quadraui::Editor`], [`quadraui::Minimap`],
/// [`quadraui::Image`], [`quadraui::Chart`], [`quadraui::Canvas`], [`quadraui::ScrollView`],
/// [`quadraui::BoardModel`], [`quadraui::PipelineView`],
/// [`quadraui::DiffView`], [`quadraui::Checkbox`], [`quadraui::Slider`],
/// [`quadraui::Select`] or [`quadraui::Markdown`] has a bespoke
/// [`ExtraPaint`] impl to write
/// here — painting, whole-area hit testing and [`DynWidget::handle`]
/// routing are already fully generic (see
/// [`crate::layout::LeafDescriptor::Widget`]'s own doc), so each kind's
/// entire wiring is its one line in this array.
/// [`quadraui::CellGrid`]'s own [`crate::interactive::Interactive`] impl
/// (keyboard navigation, inline editing, click-and-drag selection) and
/// [`quadraui::CellGrid::cells_requested`] (read generically through
/// [`crate::engine::InteractionEngine::dyn_widget_cells_requested`]'s own
/// downcast) cover every bit of state a `"cell_grid"` leaf needs;
/// [`quadraui::Switch`]'s own [`crate::interactive::Interactive`] impl
/// (Enter/Space/click toggling) covers `"switch"` the same way;
/// [`quadraui::RadioGroup`]'s own [`crate::interactive::Interactive`] impl
/// (arrow-key cursor, click-to-select, one [`crate::interactive::FocusStop`]
/// per option via [`crate::interactive::FocusSpec::Children`]) covers
/// `"radio_group"`; [`quadraui::Button`]'s own
/// [`crate::interactive::Interactive`] impl (a single
/// [`crate::interactive::FocusSpec::Stop`], Enter/Space/click activation)
/// covers `"button"`; [`quadraui::CommandCenter`]'s own
/// [`crate::interactive::Interactive`] impl (a press on an arrow or the
/// search box, Enter/Space on the focused strip) covers
/// `"command_center"`; [`quadraui::Terminal`]'s own
/// [`crate::interactive::Interactive`] impl (click-to-focus, selection
/// drag, wheel scroll) covers `"terminal"`; [`quadraui::StatusBar`]'s own
/// [`crate::interactive::Interactive`] impl (press and release on one
/// clickable segment) covers `"status_bar"`;
/// [`quadraui::ActivityBar`]'s own [`crate::interactive::Interactive`] impl
/// (click-to-activate, Up/Down cursor, Enter) covers `"activity_bar"`; and
/// [`quadraui::Palette`]'s own [`crate::interactive::Interactive`] impl
/// (query editing, list navigation, click-to-select, Enter/Escape) covers
/// `"palette"`; and [`quadraui::ContextMenu`]'s own
/// [`crate::interactive::Interactive`] impl (row hover, click-to-activate,
/// arrow-key selection, Enter) covers a `"context_menu"` placed as a
/// layout leaf. [`quadraui::Editor`]'s and [`quadraui::Minimap`]'s own
/// [`crate::interactive::Interactive`] impls hold only gesture state (a
/// drag anchor, a seek drag), and [`quadraui::Image`] is non-interactive
/// with its natural size from its own `Primitive::measure`, so `"editor"`,
/// `"minimap"` and `"image"` need nothing more. [`quadraui::Chart`]'s hover
/// and crosshair are descriptor fields the app owns and its `State` is
/// empty, and [`quadraui::ScrollView`]'s offset is a descriptor field with
/// only the scrollbar drag in its `State`, so `"chart"` and
/// `"scroll_view"` need nothing more either. [`quadraui::Canvas`]'s own
/// [`crate::interactive::Interactive`] impl resolves its declared regions
/// (hover, leave, click and double-click, in canvas-local coordinates),
/// so `"canvas"` needs nothing more. [`quadraui::BoardModel`],
/// [`quadraui::PipelineView`] and [`quadraui::DiffView`] keep their
/// selection, focus and scroll on the descriptor and hold no state at all,
/// so `"board"`, `"pipeline_view"` and `"diff_view"` need nothing more
/// either. [`quadraui::Checkbox`], [`quadraui::Slider`] and
/// [`quadraui::Select`] paint through `Form`'s own per-kind control painter
/// and report the same `FormEvent` a form row of that kind does;
/// [`quadraui::Markdown`] composes a `TextDisplay` and reports a link press
/// as `UiEvent::RichTextLinkClicked` and a wheel notch as the display's own
/// scroll event. [`quadraui::TreeTableModel`] keeps the user's expand,
/// selection, scroll and sort state in its own `Interactive::State`,
/// which the engine keeps across frames, so `"tree_table_controller"`
/// needs nothing more either. A further kind joins this table only when it
/// likewise needs nothing beyond that generic contract; a kind with its
/// own paint-time chrome state (tab-stop children, chrome click regions,
/// a custom whole-area-vs-sub-region hit test) still belongs in
/// [`EXTRA_PRIMITIVE_KINDS`] instead, same as every entry already there.
pub const WIDGET_KINDS: [&str; 37] = [
    "tree_table",
    "tree_table_controller",
    "toolbar",
    "number_input",
    "combo_box",
    "date_picker",
    "progress",
    "spinner",
    "scrollbar",
    "command_line",
    "message_list",
    "multi_section_view",
    "find_replace",
    "icon_grid",
    "cell_grid",
    "switch",
    "radio_group",
    "button",
    "command_center",
    "terminal",
    "status_bar",
    "activity_bar",
    "palette",
    "context_menu",
    "editor",
    "minimap",
    "image",
    "chart",
    "canvas",
    "scroll_view",
    "board",
    "pipeline_view",
    "diff_view",
    "checkbox",
    "slider",
    "select",
    "markdown",
];

/// Every kind a layout leaf or a flat render dict decodes into a generic
/// [`crate::layout::LeafDescriptor::Widget`]: [`WIDGET_KINDS`], then every
/// kind registered at runtime whose name is not a reserved node key (see
/// [`crate::wire::is_reserved_node_key`]). A binding (or any crate linking
/// this one) that registers a kind through [`register_widget_kind`] gets it
/// decoded, painted and routed with no table edit here.
pub fn widget_leaf_kinds() -> Vec<&'static str> {
    let mut kinds = WIDGET_KINDS.to_vec();
    kinds.extend(
        quadraui::widget_kinds::runtime_widget_kinds()
            .into_iter()
            .filter(|kind| {
                !crate::wire::is_reserved_node_key(kind) && !WIDGET_KINDS.contains(kind)
            }),
    );
    kinds
}

/// Registers a widget kind a binding defines at runtime, so a layout leaf
/// `{"id": .., kind: {..}}` decodes through `decode` into a generic
/// [`crate::layout::LeafDescriptor::Widget`]. Fails with
/// [`quadraui::widget_kinds::KindError::Duplicate`] when `kind` is a
/// reserved node key ([`crate::wire::is_reserved_node_key`]) as well as
/// when [`quadraui::register_widget_kind`] itself refuses it, so a name
/// that layout decoding would shadow is rejected rather than silently
/// never used.
pub fn register_widget_kind(
    kind: &'static str,
    decode: quadraui::widget_kinds::DecodeFn,
) -> Result<(), quadraui::widget_kinds::KindError> {
    reject_reserved_kind(kind)?;
    quadraui::register_widget_kind(kind, decode)
}

/// [`register_widget_kind`] plus the JSON Schema of the kind's payload,
/// which [`crate::wire::schema::wire_schema`] lists alongside the static kinds.
pub fn register_widget_kind_with_schema(
    kind: &'static str,
    decode: quadraui::widget_kinds::DecodeFn,
    schema: quadraui::widget_kinds::SchemaFn,
) -> Result<(), quadraui::widget_kinds::KindError> {
    reject_reserved_kind(kind)?;
    quadraui::register_widget_kind_with_schema(kind, decode, schema)
}

fn reject_reserved_kind(kind: &str) -> Result<(), quadraui::widget_kinds::KindError> {
    if crate::wire::is_reserved_node_key(kind) {
        return Err(quadraui::widget_kinds::KindError::Duplicate(
            kind.to_string(),
        ));
    }
    Ok(())
}

/// Decodes one [`WIDGET_KINDS`] leaf dict body into a type-erased
/// [`DynWidget`] — the `Widget` counterpart of
/// [`decode_extra_primitive_checked`], and like it routed through
/// `diagnostics`, so an unrecognized key inside `value` is a hard
/// `unknown_key` error under `strict` and a one-shot warning under the
/// default policy rather than being silently ignored.
pub(crate) fn decode_widget_leaf(
    diagnostics: &crate::wire::DecodeDiagnostics,
    what: &str,
    kind: &str,
    value: &serde_json::Value,
) -> Result<Rc<dyn DynWidget>, crate::DecodeError> {
    crate::wire::decode_widget_checked(diagnostics, what, kind, value).map(Rc::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hit::PaintedHit;
    use crate::layout::{LayoutLeaf, LeafDescriptor};
    use quadraui::tui::testing::TuiDriver;
    use quadraui::StyledText;
    use quadraui::{AppLogic, FormEvent, Reaction, Toolbar, ToolbarButton, TreeTable, UiEvent};
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

    /// A `Switch` placed as a `LeafDescriptor::Widget` (not `Extra` —
    /// [`Switch`] has no [`ExtraPaint`] impl, see
    /// [`crate::layout::LeafDescriptor::Widget`]'s own doc) paints through
    /// [`quadraui::DrawsPrimitives::draw_dyn`] and a `MouseDown` landing
    /// on it round-trips through
    /// [`crate::engine::InteractionEngine::handle`]'s `HitDetail::WidgetBody`
    /// arm into [`quadraui::primitive::DynWidget::handle`], which — unlike
    /// [`IconGrid`]'s own no-op — resolves [`Switch::click`] and comes back
    /// out as a real `UiEvent::SwitchToggled`, substituted for the raw
    /// `MouseDown` entirely (see [`InteractionEngine::dispatch_dyn_widget_click`]'s
    /// own doc for why a resolved event clears the attached hit detail).
    #[test]
    fn switch_leaf_click_toggles_through_the_dyn_widget_seam() {
        use crate::engine::InteractionEngine;
        use crate::layout::{LayoutContainer, LayoutNode};
        use crate::render::paint_layout;
        use quadraui::Spacing;
        use quadraui::{Modifiers, MouseButton, Switch};

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
                        descriptor: LeafDescriptor::Widget {
                            kind: "switch",
                            widget: Rc::new(Switch::new(WidgetId::new("sw:1"))),
                        },
                        scroll_px: None,
                    })],
                    Spacing::default(),
                ));
                let viewport = backend.viewport();
                let area = Rect::new(0.0, 0.0, viewport.width, viewport.height);
                let mut hits = Vec::new();
                paint_layout(backend, area, &tree, &mut hits);
                self.engine.set_hits(hits);
            }

            fn extra_tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
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
                    PaintedHit::Widget { id, area, .. } if *id == WidgetId::new("sw:1") => {
                        Some(*area)
                    }
                    _ => None,
                })
                .expect("switch leaf should have painted a Widget hit")
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
                UiEvent::SwitchToggled { widget, value: true }
                    if *widget == WidgetId::new("sw:1")
            )),
            "expected a SwitchToggled event for the inline switch leaf, got {dispatched:?}"
        );
    }

    /// A `Toolbar` placed directly as a `LayoutNode::Leaf` is a generic
    /// widget leaf: a `MouseDown` on its button round-trips through
    /// [`crate::engine::InteractionEngine::handle`] into the toolbar's own
    /// `Interactive::handle`, which reports
    /// `UiEvent::Toolbar(.., ToolbarEvent::ButtonClicked)`.
    #[test]
    fn toolbar_leaf_click_emits_button_clicked_through_the_interaction_engine() {
        use crate::engine::InteractionEngine;
        use crate::layout::{LayoutContainer, LayoutNode};
        use crate::render::paint_layout;
        use quadraui::Spacing;
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
                        descriptor: LeafDescriptor::Widget {
                            kind: "toolbar",
                            widget: Rc::new(toolbar),
                        },
                        scroll_px: None,
                    })],
                    Spacing::default(),
                ));
                let viewport = backend.viewport();
                let area = Rect::new(0.0, 0.0, viewport.width, viewport.height);
                let mut hits = Vec::new();
                paint_layout(backend, area, &tree, &mut hits);
                crate::host::render_overlays(&self.engine, backend, hits);
            }

            fn extra_tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
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

        let button_point = toolbar_button_points(5)
            .into_iter()
            .next()
            .expect("toolbar leaf should have painted its button");

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

    /// The first painted point of each distinct `"toolbar:main"` button
    /// on row 0, scanning `width` columns of the painted widget store.
    fn toolbar_button_points(width: usize) -> Vec<Point> {
        let mut seen = Vec::new();
        let mut points = Vec::new();
        for x in 0..width {
            let point = Point::new(x as f32, 0.0);
            if let Some((id, Some(sub))) = quadraui::primitive::painted_hit(point.x, point.y) {
                if id == WidgetId::new("toolbar:main") && !seen.contains(&sub) {
                    seen.push(sub);
                    points.push(point);
                }
            }
        }
        points
    }

    /// A toolbar leaf paints through `draw_dyn`, so it is recorded in the
    /// painted widget store under its own id with one sub-part per button,
    /// and [`crate::engine::InteractionEngine::handle`] feeds a pointer move
    /// over a button into the ambient pointer state, asking for a redraw
    /// when the hovered button changes — also for a host that never runs
    /// the move through a runner of its own.
    #[test]
    fn toolbar_extra_is_hit_tested_by_the_ambient_pointer_state() {
        use crate::engine::InteractionEngine;
        use crate::layout::{LayoutContainer, LayoutNode};
        use crate::render::paint_layout;
        use quadraui::Spacing;

        struct ToolbarLeafApp {
            engine: InteractionEngine,
            sink: RefCell<RecordingSink>,
        }

        impl AppLogic for ToolbarLeafApp {
            type AreaId = ();

            fn render(&self, backend: &mut dyn Backend, _area: ()) {
                let save = |id: &str| ToolbarButton::Action {
                    id: WidgetId::new(id),
                    label: "Save".into(),
                    icon: None,
                    key_hint: None,
                    enabled: true,
                    is_active: false,
                    tooltip: String::new(),
                    spec: None,
                };
                let toolbar = Toolbar::new(WidgetId::new("toolbar:main"))
                    .with_buttons(vec![save("toolbar:save"), save("toolbar:keep")]);
                let tree = LayoutNode::Row(LayoutContainer::new(
                    vec![LayoutNode::Leaf(LayoutLeaf {
                        id: "toolbar:main".into(),
                        descriptor: LeafDescriptor::Widget {
                            kind: "toolbar",
                            widget: Rc::new(toolbar),
                        },
                        scroll_px: None,
                    })],
                    Spacing::default(),
                ));
                let viewport = backend.viewport();
                let area = Rect::new(0.0, 0.0, viewport.width, viewport.height);
                let mut hits = Vec::new();
                paint_layout(backend, area, &tree, &mut hits);
                crate::host::render_overlays(&self.engine, backend, hits);
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
        let mut driver = TuiDriver::new(app, 30, 5);
        quadraui::interaction::reset_ambient_interaction();

        let button_points = toolbar_button_points(30);
        assert_eq!(button_points.len(), 2);

        let painted = |p: Point| quadraui::primitive::painted_hit(p.x, p.y);
        let first = painted(button_points[0]).expect("first button is in the painted store");
        let second = painted(button_points[1]).expect("second button is in the painted store");
        assert_eq!(first.0, WidgetId::new("toolbar:main"));
        assert!(first.1.is_some() && second.1.is_some() && first.1 != second.1);

        let mut move_to = |p: Point| {
            let (backend, app) = driver.parts_mut();
            let sink = &mut *app.sink.borrow_mut();
            app.engine.handle(
                UiEvent::MouseMoved {
                    position: p,
                    buttons: Default::default(),
                },
                backend,
                sink,
            )
        };
        assert_eq!(move_to(button_points[0]), Reaction::Redraw);
        assert_eq!(
            quadraui::interaction::ambient_interaction().hovered_target(),
            Some(first)
        );
        assert_eq!(move_to(button_points[1]), Reaction::Redraw);
        assert_eq!(
            quadraui::interaction::ambient_interaction().hovered_target(),
            Some(second)
        );
    }

    /// An `IconGrid` placed as a `LeafDescriptor::Widget` (not `Extra` —
    /// [`IconGrid`] has no [`ExtraPaint`] impl at all, see
    /// [`crate::layout::LeafDescriptor::Widget`]'s own doc) paints through
    /// [`quadraui::DrawsPrimitives::draw_dyn`] and records a
    /// [`PaintedHit::Widget`], proving the generic seam carries a click
    /// decode-to-paint-to-hit-test-to-dispatch end to end: a `MouseDown`
    /// landing on the painted area resolves [`crate::hit::HitDetail::WidgetBody`] (not some
    /// fallback "unrecognised hit" shape), and
    /// [`crate::engine::InteractionEngine::handle`] routes it into
    /// [`quadraui::primitive::DynWidget::handle`] rather than skipping
    /// dispatch outright — observable here because
    /// [`IconGrid::handle`][quadraui::primitive::DynWidget::handle] always
    /// returns [`quadraui::Handled::Ignored`] (`IconGrid` has no
    /// `Interactive` override), so the dispatch falls through to the same
    /// raw `MouseDown` [`RecordingSink::dispatch`] would have seen without
    /// this seam — but now carrying `Some(&HitDetail::WidgetBody)`, which
    /// only happens once [`InteractionEngine::handle`] has actually walked
    /// the `dispatch_dyn_widget_click` branch rather than treating the hit
    /// as unrecognised.
    #[test]
    fn icon_grid_leaf_click_round_trips_through_the_dyn_widget_seam() {
        use crate::engine::InteractionEngine;
        use crate::hit::HitDetail;
        use crate::layout::{LayoutContainer, LayoutNode};
        use crate::render::paint_layout;
        use quadraui::Spacing;
        use quadraui::{IconGrid, IconGridItem, Modifiers, MouseButton};

        /// Unlike [`RecordingSink`], also keeps the [`HitDetail`] each
        /// dispatch carried, so the test can assert the click resolved to
        /// [`HitDetail::WidgetBody`] specifically, not merely that some
        /// event arrived.
        #[derive(Default)]
        struct DetailRecordingSink {
            dispatched: Vec<(UiEvent, Option<HitDetail>)>,
        }

        impl crate::engine::EventSink for DetailRecordingSink {
            fn dispatch(&mut self, event: &UiEvent, hit_detail: Option<&HitDetail>) -> Reaction {
                self.dispatched.push((event.clone(), hit_detail.cloned()));
                Reaction::Continue
            }
        }

        struct IconGridLeafApp {
            engine: InteractionEngine,
            sink: RefCell<DetailRecordingSink>,
        }

        impl AppLogic for IconGridLeafApp {
            type AreaId = ();

            fn render(&self, backend: &mut dyn Backend, _area: ()) {
                let grid = IconGrid {
                    id: WidgetId::new("grid:1"),
                    items: vec![IconGridItem {
                        id: WidgetId::new("grid:1:item-0"),
                        icon: quadraui::Icon::new("\u{f15b}", "F"),
                        image: None,
                        label: StyledText::plain("README.md"),
                        detail: None,
                        decoration: Default::default(),
                    }],
                    selected_idx: 0,
                    has_focus: true,
                    scroll_offset: 0,
                    tile_size: None,
                };
                let tree = LayoutNode::Row(LayoutContainer::new(
                    vec![LayoutNode::Leaf(LayoutLeaf {
                        id: "grid:1".into(),
                        descriptor: LeafDescriptor::Widget {
                            kind: "icon_grid",
                            widget: Rc::new(grid),
                        },
                        scroll_px: None,
                    })],
                    Spacing::default(),
                ));
                let viewport = backend.viewport();
                let area = Rect::new(0.0, 0.0, viewport.width, viewport.height);
                let mut hits = Vec::new();
                paint_layout(backend, area, &tree, &mut hits);
                self.engine.set_hits(hits);
            }

            fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
                self.engine
                    .handle(event, backend, &mut *self.sink.borrow_mut())
            }
        }

        let app = IconGridLeafApp {
            engine: InteractionEngine::new(),
            sink: RefCell::new(DetailRecordingSink::default()),
        };
        let mut driver = TuiDriver::new(app, 20, 5);

        let grid_area = {
            let hits = driver.app().engine.hit_map();
            hits.iter()
                .find_map(|hit| match hit {
                    PaintedHit::Widget { id, area, .. } if *id == WidgetId::new("grid:1") => {
                        Some(*area)
                    }
                    _ => None,
                })
                .expect("icon_grid leaf should have painted a Widget hit")
        };

        driver.dispatch(UiEvent::MouseDown {
            widget: None,
            button: MouseButton::Left,
            position: Point::new(grid_area.x, grid_area.y),
            modifiers: Modifiers::default(),
        });

        let dispatched = &driver.app().sink.borrow().dispatched;
        assert!(
            dispatched.iter().any(|(event, hit_detail)| matches!(
                (event, hit_detail),
                (UiEvent::MouseDown { .. }, Some(HitDetail::WidgetBody))
            )),
            "expected the icon_grid click to resolve HitDetail::WidgetBody \
             through the InteractionEngine, got {dispatched:?}"
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
    /// round-trips through the generic widget seam into
    /// [`quadraui::Markdown`]'s own `Interactive::handle` and out as
    /// `UiEvent::RichTextLinkClicked` — the same event `RichTextPopup`'s own
    /// link click already dispatches.
    #[test]
    fn markdown_leaf_renders_formatting_and_link_click_emits_rich_text_link_clicked() {
        use crate::engine::InteractionEngine;
        use crate::layout::{LayoutContainer, LayoutNode};
        use crate::render::paint_layout;
        use quadraui::Spacing;
        use quadraui::{Modifiers, MouseButton};

        struct MarkdownLeafApp {
            engine: InteractionEngine,
            sink: RefCell<RecordingSink>,
        }

        impl AppLogic for MarkdownLeafApp {
            type AreaId = ();

            fn render(&self, backend: &mut dyn Backend, _area: ()) {
                let markdown = quadraui::Markdown::new(
                    WidgetId::new("md:1"),
                    "# Title\n**bold** *italic* `code`\n- item\n[docs](https://example.com/docs)",
                );
                let tree = LayoutNode::Row(LayoutContainer::new(
                    vec![LayoutNode::Leaf(LayoutLeaf {
                        id: "md:1".into(),
                        descriptor: LeafDescriptor::Widget {
                            kind: "markdown",
                            widget: Rc::new(markdown),
                        },
                        scroll_px: None,
                    })],
                    Spacing::default(),
                ));
                let viewport = backend.viewport();
                let area = Rect::new(0.0, 0.0, viewport.width, viewport.height);
                let mut hits = Vec::new();
                paint_layout(backend, area, &tree, &mut hits);
                self.engine.set_hits(hits);
            }

            fn extra_tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
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
            let (x, y) = driver.find("docs").expect("the link line is painted");
            Point::new(x, y)
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
    /// same scroll a plain `"text_display"` leaf's own wheel reports, since
    /// the composed display handles it — since the host
    /// owns `Markdown::scroll_offset` as a controlled component and has no
    /// other way to learn a scroll gesture happened. The document has more
    /// lines than the 10-row viewport fits, so there is something to
    /// scroll.
    #[test]
    fn markdown_leaf_wheel_scroll_emits_text_display_scrolled() {
        use crate::engine::InteractionEngine;
        use crate::layout::{LayoutContainer, LayoutNode};
        use crate::render::paint_layout;
        use quadraui::Spacing;
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
                let markdown = quadraui::Markdown::new(WidgetId::new("md:scroll"), source);
                let tree = LayoutNode::Row(LayoutContainer::new(
                    vec![LayoutNode::Leaf(LayoutLeaf {
                        id: "md:scroll".into(),
                        descriptor: LeafDescriptor::Widget {
                            kind: "markdown",
                            widget: Rc::new(markdown),
                        },
                        scroll_px: None,
                    })],
                    Spacing::default(),
                ));
                let viewport = backend.viewport();
                let area = Rect::new(0.0, 0.0, viewport.width, viewport.height);
                let mut hits = Vec::new();
                paint_layout(backend, area, &tree, &mut hits);
                self.engine.set_hits(hits);
            }

            fn extra_tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
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
    /// [`crate::engine::InteractionEngine`]'s own `AppMenu` by
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
        use crate::layout::{LayoutContainer, LayoutNode};
        use crate::render::paint_layout;
        use quadraui::Spacing;
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
                paint_layout(backend, area, &tree, &mut hits);
                crate::host::render_overlays(&self.engine, backend, hits);
            }

            fn extra_tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
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
        leaf: Box<dyn Fn() -> LeafDescriptor>,
        engine: crate::engine::InteractionEngine,
        sink: RefCell<RecordingSink>,
    }

    impl AppLogic for SingleLeafApp {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            use crate::layout::{LayoutContainer, LayoutNode};
            use crate::render::paint_layout;
            use quadraui::Spacing;

            let tree = LayoutNode::Row(LayoutContainer::new(
                vec![LayoutNode::Leaf(LayoutLeaf {
                    id: self.id.as_str().to_string(),
                    descriptor: (self.leaf)(),
                    scroll_px: None,
                })],
                Spacing::default(),
            ));
            let viewport = backend.viewport();
            let area = Rect::new(0.0, 0.0, viewport.width, viewport.height);
            let mut hits = Vec::new();
            paint_layout(backend, area, &tree, &mut hits);
            self.engine.set_hits(hits);
        }

        fn extra_tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
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
        leaf_driver(id, Box::new(move || LeafDescriptor::Extra(extra.clone())))
    }

    /// [`single_leaf_driver`] for a [`WIDGET_KINDS`] leaf of `kind`.
    fn single_widget_driver(
        id: WidgetId,
        kind: &'static str,
        widget: Rc<dyn DynWidget>,
    ) -> TuiDriver<SingleLeafApp> {
        leaf_driver(
            id,
            Box::new(move || LeafDescriptor::Widget {
                kind,
                widget: widget.clone(),
            }),
        )
    }

    fn leaf_driver(
        id: WidgetId,
        leaf: Box<dyn Fn() -> LeafDescriptor>,
    ) -> TuiDriver<SingleLeafApp> {
        let app = SingleLeafApp {
            id,
            leaf,
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
                }
                | PaintedHit::Widget {
                    id: hit_id, area, ..
                } if *hit_id == id => Some(*area),
                _ => None,
            })
            .expect("leaf should have painted an Extra or Widget hit")
    }

    /// Render shows the unchecked glyph; a click toggles it (mouse); Space
    /// toggles it again (keyboard) — both routed through the generic
    /// widget seam into [`quadraui::Checkbox`]'s own `Interactive::handle`,
    /// both reaching the handler as [`FormEvent::ToggleChanged`]. The
    /// `[ ]`/`[x]` glyph is the form's own `FieldKind::Toggle` painter.
    #[test]
    fn checkbox_leaf_renders_clicks_and_keys_toggle_through_the_interaction_engine() {
        let id = WidgetId::new("cb:1");
        let checkbox = || -> Rc<dyn DynWidget> {
            let mut checkbox = quadraui::Checkbox::new(id);
            checkbox.focused = true;
            Rc::new(checkbox)
        };

        let mut driver = single_widget_driver(id, "checkbox", checkbox());
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
        let mut driver = single_widget_driver(id, "checkbox", checkbox());
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
    /// maps the click's x-position along the painted track to a value —
    /// reaching the handler as [`FormEvent::SliderChanged`] either way.
    #[test]
    fn slider_leaf_renders_clicks_and_keys_step_through_the_interaction_engine() {
        let id = WidgetId::new("sl:1");
        let mut slider = quadraui::Slider::new(id, 0.0, 0.0, 10.0);
        slider.focused = true;
        let mut driver = single_widget_driver(id, "slider", Rc::new(slider));

        // The last track cell, so the mapped fraction is close to (but
        // short of) `1.0` — proving the click landed where the pointer
        // was, not just "somewhere on the body".
        let (close_x, close_y) = driver.find("]").expect("the track's closing bracket");
        driver.click(close_x - 1.0, close_y);
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

    /// An unrecognised key inside a `"slider"` descriptor body -- a
    /// generic-widget leaf, reached through [`decode_widget_leaf`] rather
    /// than the six fixed leaf kinds -- is a hard [`crate::DecodeError::UnknownKey`] under
    /// `strict`, naming the key, the dict it was found on, and the keys
    /// `Slider` actually accepts, the same policy a layout-tree node's own
    /// sibling keys already get.
    #[test]
    fn slider_leaf_body_rejects_an_unknown_key_under_strict() {
        let value = serde_json::json!({
            "id": "sl:1",
            "value": 0.0,
            "min": 0.0,
            "max": 10.0,
            "stpe": 1.0,
        });
        let diagnostics = crate::wire::DecodeDiagnostics::strict();
        let error = match decode_widget_leaf(&diagnostics, "test", "slider", &value) {
            Ok(_) => panic!("a typo'd \"stpe\" key must be rejected under strict"),
            Err(err) => err,
        };
        assert_eq!(error.code(), "unknown_key");
        assert!(
            error.to_string().contains("\"stpe\""),
            "{}",
            error.to_string()
        );
    }

    /// The same typo'd key [`slider_leaf_body_rejects_an_unknown_key_under_strict`]
    /// rejects under `strict` only warns once, through the registered hook,
    /// under the default (warn) policy -- the decode still succeeds despite
    /// the unrecognised key.
    #[test]
    fn slider_leaf_body_warns_once_under_default_policy() {
        let value = serde_json::json!({
            "id": "sl:1",
            "value": 0.0,
            "min": 0.0,
            "max": 10.0,
            "stpe": 1.0,
        });
        let diagnostics = crate::wire::DecodeDiagnostics::warn();
        if let Err(err) = decode_widget_leaf(&diagnostics, "test", "slider", &value) {
            panic!("warn mode still decodes despite the unrecognised key: {err}");
        }
        assert_eq!(
            diagnostics.warned_keys(),
            std::collections::HashSet::from(["stpe".to_string()])
        );
    }

    /// Down (keyboard) and a click (mouse) both move to the next option,
    /// committing immediately — reaching the handler as
    /// [`FormEvent::DropdownChanged`], mirroring `FieldKind::Dropdown`'s
    /// own "no separate open step" contract.
    #[test]
    fn select_leaf_renders_clicks_and_keys_move_through_the_interaction_engine() {
        let id = WidgetId::new("se:1");
        let select = || -> Rc<dyn DynWidget> {
            let mut select = quadraui::Select::new(
                id,
                vec!["One".to_string(), "Two".to_string(), "Three".to_string()],
            );
            select.focused = true;
            Rc::new(select)
        };

        let mut driver = single_widget_driver(id, "select", select());
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
        let mut driver = single_widget_driver(id, "select", select());
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
    /// convention `ComboBox`/`DatePicker` already have) —
    /// reaching the handler as [`FormEvent::TextInputChanged`] via
    /// [`password_key_event`]'s forward to [`text_input_key_event`]. The
    /// render assertion checks the masked glyph appears and the plaintext
    /// never does, proving [`PasswordInput::as_widget`] actually masks
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
    /// [`PasswordInput::as_widget`]'s "always exactly one line" invariant
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

    /// A `"tree_table"` widget leaf paints a branch row's label and chevron
    /// glyph, and a click on the chevron resolves through the table's own
    /// `Interactive::handle` to
    /// [`quadraui::UiEvent::TreeTable`]`(`[`quadraui::TreeTableEvent::ToggleExpanded`]`)`.
    #[test]
    fn tree_table_extra_renders_label_and_chevron_and_passes_through_a_click() {
        let id = WidgetId::new("tt:files");
        let table = quadraui::DataTable {
            id,
            columns: vec![quadraui::Column {
                title: "Name".to_string(),
                width: Default::default(),
                align: Default::default(),
            }],
            rows: vec![
                quadraui::DataRow {
                    cells: vec![quadraui::StyledText::plain("src")],
                    decoration: Default::default(),
                },
                quadraui::DataRow {
                    cells: vec![quadraui::StyledText::plain("main.rs")],
                    decoration: Default::default(),
                },
            ],
            selected_idx: None,
            scroll_offset: 0,
            sort: None,
            has_focus: true,
            show_scrollbar: false,
            min_total_width: None,
            h_scroll: 0.0,
            column_overrides: vec![],
            footer: None,
            bordered: false,
            striped: false,
            glyphs: None,
        };
        let tree_table = TreeTable::new(
            table,
            vec![
                quadraui::TreeTableNode::new(vec![0], 0).with_expanded(true),
                quadraui::TreeTableNode::new(vec![0, 0], 1),
            ],
        );

        let mut driver = single_widget_driver(id, "tree_table", Rc::new(tree_table));
        assert!(driver.screen_contains("src"), "{}", driver.screen());
        assert!(driver.screen_contains("main.rs"), "{}", driver.screen());
        assert!(
            driver.screen_contains("▾"),
            "the expanded branch row's chevron glyph must reach the screen: {}",
            driver.screen()
        );

        let area = leaf_area(&driver, id);
        // Row 0 is the header; "src" (the only expanded branch, indent 0)
        // paints its chevron at column 0 of row 1 — see
        // `tree_table::paint_cells::tests::branch_row_paints_a_chevron_and_the_label_still_appears`.
        driver.click(area.x, area.y + 1.0);
        let dispatched = &driver.app().sink.borrow().dispatched;
        assert!(
            dispatched.iter().any(|event| matches!(
                event,
                UiEvent::TreeTable(
                    ev_id,
                    quadraui::TreeTableEvent::ToggleExpanded { path, expand: false }
                ) if *ev_id == id && path.as_slice() == [0u16]
            )),
            "a click on the chevron should resolve to TreeTableEvent::ToggleExpanded: \
                 {dispatched:?}"
        );
    }
}
