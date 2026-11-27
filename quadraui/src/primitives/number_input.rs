//! `NumberInput` primitive: a bounded numeric field with stepper
//! buttons — layout, hit-test, keyboard/wheel handling, validation, its
//! event, and both rasterisers.
//!
//! The whole widget contract lives here: [`Primitive`] resolves the
//! layout and paints it ([`native_surface_paint`] on a pixel surface,
//! [`cell_paint`] on a cell grid), and [`Interactive`] answers pointer,
//! wheel, keyboard and assistive-technology input. A host reaches all of
//! it type-erased through [`crate::frame::Surface::Widget`].
//!
//! Additive alongside [`crate::TextInput`], which is untouched — a
//! `NumberInput` isn't a constrained `TextInput`, it's a separate
//! primitive with its own numeric-only editing model: a committed
//! [`NumberInput::value`] plus an optional in-progress
//! [`NumberInput::draft`] string that overrides the display until the
//! host applies a [`NumberInputEvent::Committed`] or
//! [`NumberInputEvent::Cancelled`] event back onto the struct.
//!
//! # Focus and activation
//!
//! Like [`crate::Switch`], a `NumberInput` tracks its own
//! [`NumberInput::focused`] flag — there is one keyboard target, so no
//! wraparound cursor is needed (contrast [`crate::RadioGroup`]).
//! [`NumberInput::handle_key`] is gated on `focused` and not
//! [`NumberInput::disabled`]; the stepper buttons
//! ([`NumberInput::increment`]/[`NumberInput::decrement`], and the
//! [`NumberInput::click`] dispatcher built on top of them) are gated on
//! `disabled` alone, matching [`crate::Switch::click`]'s "a pointer
//! click doesn't require prior keyboard focus" contract.
//! [`NumberInput::handle_wheel`] mirrors
//! [`crate::ScrollView::handle_wheel`]'s shape and is likewise ungated on
//! focus — wheel events route to whatever the pointer is over, not
//! whatever holds keyboard focus.
//!
//! # Validation
//!
//! Typed digits accumulate in [`NumberInput::draft`] via
//! [`NumberInputEvent::DraftChanged`]; `Enter` parses the draft (falling
//! back to the current [`NumberInput::value`] on a parse failure),
//! clamps it to `[min, max]`, rounds it to [`NumberInput::precision`]
//! decimal places, and emits [`NumberInputEvent::Committed`]. `Escape`
//! discards the draft via [`NumberInputEvent::Cancelled`] without
//! touching `value`. Routing app-wide Tab into
//! [`NumberInput::focused`] is the host's job, same division of
//! responsibility as every other primitive here.

use crate::a11y::A11yAction;
use crate::event::{Key, NamedKey, Point, Rect, ScrollDelta};
use crate::interactive::{
    EventCx, FocusSpec, Interactive, KeyEvent, PointerKind, RoutedEvent, WidgetAction,
};
use crate::primitive::{LayoutCtx, Primitive, Size, Units};
use crate::types::{Modifiers, WidgetId};
use crate::Handled;
use serde::{Deserialize, Serialize};

/// Width (in the backend's layout units) reserved for the
/// increment/decrement stepper column.
const STEPPER_WIDTH: f32 = 3.0;

/// Declarative description of a bounded numeric field with stepper
/// buttons.
///
/// # Examples
///
/// ```
/// use quadraui::{
///     Key, Modifiers, NamedKey, NumberInput, NumberInputEvent, NumberInputHit, Point, Rect,
///     WidgetId,
/// };
///
/// let mut input = NumberInput::new(WidgetId::new("settings:volume"), 0.0, 100.0);
/// input.focused = true;
///
/// let layout = input.layout(Rect::new(0.0, 0.0, 10.0, 2.0));
/// assert_eq!(layout.hit_test(Point::new(9.0, 0.0)), NumberInputHit::Increment);
///
/// let event = input.handle_key(&Key::Named(NamedKey::Up), &Modifiers::default());
/// assert_eq!(
///     event,
///     Some(NumberInputEvent::ValueChanged { id: input.id.clone(), value: 1.0 })
/// );
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct NumberInput {
    pub id: WidgetId,
    /// Last committed value. Only changes via
    /// [`NumberInputEvent::ValueChanged`] (stepper/wheel) or
    /// [`NumberInputEvent::Committed`] (typed entry) — the host applies
    /// both the same way every other primitive's events are applied.
    #[serde(default)]
    pub value: f64,
    pub min: f64,
    pub max: f64,
    /// Amount [`NumberInput::increment`]/[`NumberInput::decrement`] and
    /// [`NumberInput::handle_wheel`] step by.
    #[serde(default = "NumberInput::default_step")]
    pub step: f64,
    /// Decimal places `value` is rounded to on commit/step.
    #[serde(default)]
    pub precision: u32,
    /// In-progress typed text overriding the displayed value until
    /// committed or cancelled. `None` means "showing `value` verbatim,
    /// no pending edit."
    #[serde(default)]
    pub draft: Option<String>,
    /// When `true`, every method below returns `None` — same
    /// "disabled means inert, not hidden" contract every other
    /// primitive's `disabled` flag follows.
    #[serde(default)]
    pub disabled: bool,
    /// Whether this field currently holds keyboard focus. Set by the
    /// host's focus-traversal logic; read by
    /// [`NumberInput::handle_key`] to gate typed/arrow-key handling.
    #[serde(default)]
    pub focused: bool,
}

impl NumberInput {
    fn default_step() -> f64 {
        1.0
    }

    /// A field clamped to `[min, max]`, seeded at the nearest in-range
    /// value to zero, enabled, unfocused, with no pending draft. Set
    /// `step`/`precision`/`disabled`/`focused` directly afterwards
    /// (every field is `pub`) rather than via a `with_*` chain — see
    /// `docs/PRIMITIVE_RULES.md`'s rule-8 history of why this crate
    /// stopped adding those.
    pub fn new(id: WidgetId, min: f64, max: f64) -> Self {
        Self {
            id,
            value: 0.0_f64.clamp(min, max),
            min,
            max,
            step: Self::default_step(),
            precision: 0,
            draft: None,
            disabled: false,
            focused: false,
        }
    }

    /// The text the field currently shows: the in-progress
    /// [`NumberInput::draft`] verbatim when one is pending, otherwise
    /// `value` formatted to [`NumberInput::precision`] decimal places.
    ///
    /// The single source of this formula — every painter
    /// ([`native_surface_paint::paint`], [`cell_paint::paint`]),
    /// [`Primitive::measure`] and
    /// [`crate::primitives::layout_metrics`]'s form-row measurement all
    /// read it here rather than re-deriving it, so a field can never
    /// measure wider or narrower than the text actually painted into it.
    pub fn display_text(&self) -> String {
        match &self.draft {
            Some(draft) => draft.clone(),
            None => format!("{:.*}", self.precision as usize, self.value),
        }
    }

    /// Round `value` to [`NumberInput::precision`] decimal places and
    /// clamp it to `[min, max]`.
    fn clamp_value(&self, value: f64) -> f64 {
        let factor = 10f64.powi(self.precision as i32);
        let rounded = (value * factor).round() / factor;
        rounded.clamp(self.min, self.max)
    }

    /// Resolve the field's hit-testable regions: a text area on the
    /// left, and stacked increment/decrement buttons in a fixed-width
    /// column on the right (split evenly top/bottom, mirroring
    /// [`crate::RadioGroup::layout`]'s multi-region approach rather
    /// than [`crate::Switch::layout`]'s single body).
    pub fn layout(&self, bounds: Rect) -> NumberInputLayout {
        let stepper_width = STEPPER_WIDTH.min(bounds.width);
        let text_width = (bounds.width - stepper_width).max(0.0);
        let text_area = Rect::new(bounds.x, bounds.y, text_width, bounds.height);
        let stepper_x = bounds.x + text_width;
        let half_height = bounds.height / 2.0;
        let increment = Rect::new(stepper_x, bounds.y, stepper_width, half_height);
        let decrement = Rect::new(
            stepper_x,
            bounds.y + half_height,
            stepper_width,
            bounds.height - half_height,
        );
        NumberInputLayout {
            bounds,
            text_area,
            increment,
            decrement,
        }
    }

    /// `Up`/`Down` step the value; `Enter` commits the current
    /// [`NumberInput::draft`] (or re-affirms `value` when there is no
    /// draft); `Escape` cancels a pending draft; digits/`-`/`.` and
    /// `Backspace` edit the draft. Gated on `focused` and not
    /// `disabled` — any other key is unhandled (`None`).
    pub fn handle_key(&self, key: &Key, _modifiers: &Modifiers) -> Option<NumberInputEvent> {
        if self.disabled || !self.focused {
            return None;
        }
        match key {
            Key::Named(NamedKey::Up) => self.increment(),
            Key::Named(NamedKey::Down) => self.decrement(),
            Key::Named(NamedKey::Enter) => Some(self.commit()),
            Key::Named(NamedKey::Escape) => self
                .draft
                .is_some()
                .then_some(NumberInputEvent::Cancelled { id: self.id }),
            Key::Named(NamedKey::Backspace) => {
                let mut draft = self.draft.clone().unwrap_or_default();
                draft.pop();
                Some(NumberInputEvent::DraftChanged { id: self.id, draft })
            }
            Key::Char(c) if is_number_char(*c) => {
                let mut draft = self.draft.clone().unwrap_or_default();
                draft.push(*c);
                Some(NumberInputEvent::DraftChanged { id: self.id, draft })
            }
            _ => None,
        }
    }

    /// Parse [`NumberInput::draft`] (falling back to the current
    /// `value` on an empty draft or a parse failure — invalid text
    /// reverts rather than commits garbage), clamp/round it, and
    /// produce the [`NumberInputEvent::Committed`] event.
    fn commit(&self) -> NumberInputEvent {
        let parsed = match &self.draft {
            Some(text) if !text.is_empty() => text.parse::<f64>().unwrap_or(self.value),
            _ => self.value,
        };
        NumberInputEvent::Committed {
            id: self.id,
            value: self.clamp_value(parsed),
        }
    }

    /// Step `value` up by [`NumberInput::step`], clamped/rounded. Gated
    /// on `disabled` alone — stepper buttons don't require prior
    /// keyboard focus, matching [`crate::Switch::click`].
    pub fn increment(&self) -> Option<NumberInputEvent> {
        if self.disabled {
            return None;
        }
        Some(NumberInputEvent::ValueChanged {
            id: self.id,
            value: self.clamp_value(self.value + self.step),
        })
    }

    /// Step `value` down by [`NumberInput::step`], clamped/rounded.
    pub fn decrement(&self) -> Option<NumberInputEvent> {
        if self.disabled {
            return None;
        }
        Some(NumberInputEvent::ValueChanged {
            id: self.id,
            value: self.clamp_value(self.value - self.step),
        })
    }

    /// Pointer activation dispatcher: the host calls this after
    /// resolving a `MouseDown`/`MouseUp` position through
    /// [`NumberInputLayout::hit_test`]. Hits on the text area or
    /// outside the control produce no event (focusing the text area is
    /// the host's job, same as elsewhere in this crate).
    pub fn click(&self, hit: NumberInputHit) -> Option<NumberInputEvent> {
        match hit {
            NumberInputHit::Increment => self.increment(),
            NumberInputHit::Decrement => self.decrement(),
            NumberInputHit::TextArea | NumberInputHit::Outside => None,
        }
    }

    /// Wheel-scroll stepping, mirroring
    /// [`crate::ScrollView::handle_wheel`]'s exact shape: the vertical
    /// delta notch count multiplied by `step`. Gated on `disabled`
    /// alone, not `focused` — wheel events route to whatever the
    /// pointer sits over.
    pub fn handle_wheel(&self, delta: ScrollDelta, step: f64) -> Option<NumberInputEvent> {
        if self.disabled || delta.y == 0.0 {
            return None;
        }
        Some(NumberInputEvent::ValueChanged {
            id: self.id,
            value: self.clamp_value(self.value + (delta.y as f64) * step),
        })
    }
}

impl Default for NumberInput {
    /// A `[0, 100]`-bounded field with an empty id. See
    /// [`NumberInput::new`] for a field with a real id/range.
    fn default() -> Self {
        Self::new(WidgetId::new(""), 0.0, 100.0)
    }
}

fn is_number_char(c: char) -> bool {
    c.is_ascii_digit() || c == '-' || c == '.'
}

/// Fully-resolved number-input layout.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NumberInputLayout {
    pub bounds: Rect,
    pub text_area: Rect,
    pub increment: Rect,
    pub decrement: Rect,
}

/// Classification of a hit-test result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NumberInputHit {
    TextArea,
    Increment,
    Decrement,
    Outside,
}

impl NumberInputLayout {
    pub fn hit_test(&self, p: Point) -> NumberInputHit {
        if self.increment.contains(p) {
            NumberInputHit::Increment
        } else if self.decrement.contains(p) {
            NumberInputHit::Decrement
        } else if self.text_area.contains(p) {
            NumberInputHit::TextArea
        } else {
            NumberInputHit::Outside
        }
    }
}

/// Emitted by [`NumberInput`]'s methods. `ValueChanged` carries the new
/// committed value directly (stepper/wheel never go through a draft);
/// `DraftChanged` carries the in-progress typed text; `Committed`
/// carries the parsed, clamped value a typed edit resolves to;
/// `Cancelled` carries only the id — the host clears `draft` and leaves
/// `value` untouched.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum NumberInputEvent {
    ValueChanged { id: WidgetId, value: f64 },
    DraftChanged { id: WidgetId, draft: String },
    Committed { id: WidgetId, value: f64 },
    Cancelled { id: WidgetId },
}

impl WidgetAction for NumberInputEvent {
    /// Every outcome rides [`crate::UiEvent::Form`] under the field's own
    /// id: [`crate::FormEvent`]'s four `Number*` variants name exactly
    /// the four outcomes resolved above, one for one, with the same
    /// "stepper/arrow steps immediately, a typed draft waits for
    /// Enter" split. A standalone `NumberInput` therefore reaches an app
    /// through the same event vocabulary an embedded one does, so a host
    /// that already applies [`crate::FormEvent::NumberCommitted`] needs
    /// no second handler.
    ///
    /// `id` from the contract is the id the host registered this widget
    /// under; each variant already carries its own, and the two agree
    /// for a widget reached through [`crate::frame::Surface::Widget`],
    /// so the payload's own id is preserved and the parameter names the
    /// [`crate::UiEvent::Form`] envelope.
    fn into_ui_event(self, id: WidgetId) -> crate::UiEvent {
        let event = match self {
            NumberInputEvent::ValueChanged { id, value } => {
                crate::FormEvent::NumberChanged { id, value }
            }
            NumberInputEvent::DraftChanged { id, draft } => {
                crate::FormEvent::NumberDraftChanged { id, draft }
            }
            NumberInputEvent::Committed { id, value } => {
                crate::FormEvent::NumberCommitted { id, value }
            }
            NumberInputEvent::Cancelled { id } => crate::FormEvent::NumberCancelled { id },
        };
        crate::UiEvent::Form(id, event)
    }
}

/// [`NumberInput`]'s fully-resolved widget-contract layout: the absolute
/// `area` the field was laid out in plus the field-local
/// [`NumberInputLayout`] itself.
///
/// `#[non_exhaustive]`: produced by [`Primitive::layout`], never built by
/// a consumer via struct literal — the same convention
/// [`crate::primitives::completions::CompletionsPrimitiveLayout`]
/// follows.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct NumberInputPrimitiveLayout {
    /// The rect [`Primitive::layout`] was called with — the whole
    /// control, text area plus stepper column.
    pub area: Rect,
    /// The resolved field layout. Its rects are in the same absolute
    /// space as [`Self::area`].
    pub layout: NumberInputLayout,
}

impl NumberInputPrimitiveLayout {
    /// Hit-test at a position relative to [`Self::area`]'s own origin —
    /// the space [`RoutedEvent::Pointer`]'s `local` field uses.
    pub fn hit_test_local(&self, x: f32, y: f32) -> NumberInputHit {
        self.layout
            .hit_test(Point::new(self.area.x + x, self.area.y + y))
    }
}

impl Primitive for NumberInput {
    type Layout = NumberInputPrimitiveLayout;
    type Event = NumberInputEvent;

    fn id(&self) -> Option<&WidgetId> {
        Some(&self.id)
    }

    /// One row of [`NumberInput::display_text`] plus the fixed-width
    /// stepper column, plus the chrome the painters frame the field
    /// with.
    ///
    /// The chrome is units-dependent because the border is: on a cell
    /// grid it costs two whole columns and two whole rows (the box-drawing
    /// frame [`cell_paint::paint`] draws), while a pixel surface strokes
    /// it inside the row it already occupies, so it costs one row of
    /// `line_height` and two character advances of width.
    fn measure(&self, ctx: &LayoutCtx, _avail: Size) -> crate::NaturalSize {
        let text = self.display_text();
        let cells = matches!(ctx.units(), Units::Cells);
        let (text_w, chrome_w, rows) = if cells {
            (crate::text_util::display_width(&text) as f32, 2.0, 3.0_f32)
        } else {
            (
                ctx.text_width(&text, crate::FontRole::Chrome),
                2.0 * ctx.char_width,
                1.0_f32,
            )
        };
        crate::NaturalSize::new(
            Some(text_w + chrome_w + STEPPER_WIDTH),
            Some(rows * ctx.line_height),
        )
    }

    /// `rect` is the whole control. The split into text area and stacked
    /// stepper buttons is [`NumberInput::layout`]'s own, so paint,
    /// hit-testing and the legacy `Backend::draw_number_input` path can
    /// never disagree about where the `+`/`-` cells are.
    fn layout(&self, _ctx: &LayoutCtx, rect: Rect) -> NumberInputPrimitiveLayout {
        NumberInputPrimitiveLayout {
            area: rect,
            layout: self.layout(rect),
        }
    }

    /// The shared pixel painter — the same
    /// [`native_surface_paint::paint`] every pixel backend's
    /// `draw_number_input` wrapper calls.
    fn paint(
        &self,
        layout: &NumberInputPrimitiveLayout,
        surface: &mut dyn crate::paint_surface::PaintSurface,
        env: &crate::PaintEnv,
    ) {
        let theme = env.theme();
        #[cfg(any(
            feature = "gtk",
            feature = "win",
            all(feature = "macos", target_os = "macos")
        ))]
        {
            native_surface_paint::paint(self, &layout.layout, surface, theme);
        }
        #[cfg(not(any(
            feature = "gtk",
            feature = "win",
            all(feature = "macos", target_os = "macos")
        )))]
        {
            let _ = (layout, surface, theme);
        }
    }

    /// [`cell_paint::paint`] over `buf`, reached through
    /// [`crate::tui::draw_number_input`] because the `Buffer` →
    /// [`crate::CellSurface`] adapter is private to `crate::tui`.
    #[cfg(feature = "tui")]
    fn paint_cells(
        &self,
        layout: &NumberInputPrimitiveLayout,
        cells: &mut dyn crate::CellSurface,
        env: &crate::PaintEnv,
    ) {
        let theme = env.theme();
        cell_paint::paint(self, &layout.layout, cells, theme);
    }
}

impl NumberInput {
    /// One wheel notch's worth of stepping — [`NumberInput::handle_wheel`]
    /// against this field's own [`NumberInput::step`], which is what a
    /// standalone field steps by when no host supplies a coarser
    /// per-notch amount.
    fn wheel(&self, dy: f32) -> Handled<NumberInputEvent> {
        match self.handle_wheel(ScrollDelta::new(0.0, dy), self.step) {
            Some(event) => Handled::Event(event),
            None => Handled::Ignored,
        }
    }

    /// Resolve a keystroke through [`NumberInput::handle_key`], reporting
    /// a recognised-but-inert key (`Escape` with no pending draft) as
    /// [`Handled::Ignored`] so a host's own Escape handling still runs.
    fn key(&self, key: &KeyEvent) -> Handled<NumberInputEvent> {
        match self.handle_key(&key.key, &key.modifiers) {
            Some(event) => Handled::Event(event),
            None => Handled::Ignored,
        }
    }
}

impl Interactive for NumberInput {
    /// Nothing: the committed value, the in-progress draft and the focus
    /// flag all live on the descriptor, which the host updates from the
    /// events below — so this widget has no interaction state the
    /// descriptor doesn't already carry.
    type State = ();
    type Action = NumberInputEvent;

    /// One focus stop, like [`crate::Switch`] — the field navigates no
    /// internal cursor, so there is nothing to declare a stop per.
    /// [`FocusSpec::None`] while [`NumberInput::disabled`], matching the
    /// "disabled means inert, not hidden" contract every method here
    /// follows.
    fn focus(&self) -> FocusSpec {
        if self.disabled {
            FocusSpec::None
        } else {
            FocusSpec::Stop
        }
    }

    fn handle(
        &self,
        _state: &mut (),
        event: &RoutedEvent,
        layout: &NumberInputPrimitiveLayout,
        cx: &mut EventCx,
    ) -> Handled<NumberInputEvent> {
        match event {
            RoutedEvent::Pointer {
                kind: PointerKind::Down(crate::MouseButton::Left),
                local,
                ..
            } => {
                let hit = layout.hit_test_local(local.x, local.y);
                if matches!(hit, NumberInputHit::TextArea) {
                    // Clicking the text area only moves focus there, the
                    // same division of responsibility `NumberInput::click`
                    // documents — but the field still owns the click
                    // rather than letting it fall through to whatever is
                    // painted behind it.
                    cx.request_focus(None);
                    return Handled::Consumed;
                }
                match self.click(hit) {
                    Some(event) => {
                        cx.request_focus(None);
                        Handled::Event(event)
                    }
                    None => Handled::Ignored,
                }
            }
            RoutedEvent::Wheel { dy, .. } => self.wheel(*dy),
            RoutedEvent::Key(key) => self.key(key),
            // `A11yAction` carries no stepper side through this generic
            // seam, so the two ranged-control actions map onto the two
            // stepper buttons — the assistive-technology equivalent of
            // clicking `+`/`-`.
            RoutedEvent::Action(A11yAction::Increment) => match self.increment() {
                Some(event) => Handled::Event(event),
                None => Handled::Ignored,
            },
            RoutedEvent::Action(A11yAction::Decrement) => match self.decrement() {
                Some(event) => Handled::Event(event),
                None => Handled::Ignored,
            },
            _ => Handled::Ignored,
        }
    }

    fn a11y_actions(&self, _state: &()) -> Vec<A11yAction> {
        if self.disabled {
            Vec::new()
        } else {
            vec![A11yAction::Increment, A11yAction::Decrement]
        }
    }

    /// Three sub-parts — the text area and the two stepper buttons, in
    /// [`Self::handle`]'s own local space, most-specific first so the
    /// stepper cells win over the body they sit beside.
    fn sub_regions(
        &self,
        layout: &NumberInputPrimitiveLayout,
    ) -> Vec<(Rect, crate::interactive::SubTarget)> {
        let local =
            |r: Rect| Rect::new(r.x - layout.area.x, r.y - layout.area.y, r.width, r.height);
        vec![
            (
                local(layout.layout.increment),
                crate::interactive::SubTarget(SUB_INCREMENT),
            ),
            (
                local(layout.layout.decrement),
                crate::interactive::SubTarget(SUB_DECREMENT),
            ),
            (
                local(layout.layout.text_area),
                crate::interactive::SubTarget(SUB_TEXT_AREA),
            ),
        ]
    }
}

/// [`Interactive::sub_regions`]'s tag for the editable text area.
const SUB_TEXT_AREA: u64 = 0;
/// [`Interactive::sub_regions`]'s tag for the `+` stepper button.
const SUB_INCREMENT: u64 = 1;
/// [`Interactive::sub_regions`]'s tag for the `-` stepper button.
const SUB_DECREMENT: u64 = 2;

/// The cell-grid painter for [`NumberInput`], written against
/// [`crate::CellSurface`].
///
/// A 1-cell box-drawing border around the whole control, the text area's
/// [`NumberInput::display_text`] on its first row, and `+`/`-` glyphs in
/// their own bordered stepper cells. The same content
/// [`native_surface_paint::paint`] puts on a pixel surface, degraded onto
/// the cell grid (see [`crate::paint_surface`]'s module doc, "Why TUI
/// stays out").
///
/// Colours follow [`native_surface_paint::paint`]'s own rules:
/// [`crate::Theme::input_bg`] washes the text area,
/// [`crate::Theme::accent_fg`] replaces [`crate::Theme::border_fg`] on a
/// focused field, and [`crate::Theme::muted_fg`] replaces
/// [`crate::Theme::surface_fg`] on a disabled one.
#[cfg(feature = "tui")]
pub(crate) mod cell_paint {
    use super::{NumberInput, NumberInputLayout};
    use crate::cell_surface::{draw_text_row, solid_style, stroke_square_frame, CellSurface};
    use crate::theme::Theme;

    /// Paint `number_input` at its resolved `layout` onto `surface`.
    pub(crate) fn paint(
        number_input: &NumberInput,
        layout: &NumberInputLayout,
        surface: &mut dyn CellSurface,
        theme: &Theme,
    ) {
        if layout.bounds.width <= 0.0 || layout.bounds.height <= 0.0 {
            return;
        }

        let bg = theme.background;
        let input_bg = theme.input_bg;
        let border = if number_input.disabled || !number_input.focused {
            theme.border_fg
        } else {
            theme.accent_fg
        };
        let fg = if number_input.disabled {
            theme.muted_fg
        } else {
            theme.surface_fg
        };

        let text_style = solid_style(fg, input_bg);
        let border_style = solid_style(border, bg);
        let glyph_style = solid_style(fg, bg);

        surface.fill(layout.text_area, text_style);
        stroke_square_frame(surface, layout.bounds, border_style);

        let text = number_input.display_text();
        if !text.is_empty() {
            draw_text_row(surface, layout.text_area, &text, text_style);
        }

        stroke_square_frame(surface, layout.increment, border_style);
        stroke_square_frame(surface, layout.decrement, border_style);
        draw_text_row(surface, layout.increment, "+", glyph_style);
        draw_text_row(surface, layout.decrement, "-", glyph_style);
    }
}

/// Shared-paint executor for [`NumberInput`] — a rasteriser calls into
/// this (never re-deriving geometry) instead of walking the fields
/// itself, matching every other primitive's `native_surface_paint`
/// module (see `docs/PRIMITIVE_RULES.md`'s "Primitive-first" rule).
/// Gated to the backends that actually paint pixels — the TUI backend
/// renders `NumberInput` through its own cell-grid path instead.
#[cfg(any(
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
#[allow(dead_code)]
pub(crate) mod native_surface_paint {
    use super::{NumberInput, NumberInputLayout};
    use crate::paint_surface::PaintSurface;
    use crate::theme::Theme;

    /// Paint a [`NumberInput`] onto `surface`: a bordered text area
    /// showing [`NumberInput::display_text`], plus the increment/decrement stepper
    /// buttons stacked in their own bordered cells. `disabled` and
    /// `focused` recolour the border/text, mirroring
    /// [`crate::primitives::text_input`]'s own disabled/focus
    /// treatment — there is no per-button hover/press state to react to
    /// here, since (unlike [`crate::Scrollbar`]) `NumberInput` tracks
    /// none.
    pub(crate) fn paint(
        number_input: &NumberInput,
        layout: &NumberInputLayout,
        surface: &mut dyn PaintSurface,
        theme: &Theme,
    ) {
        if layout.bounds.width <= 0.0 || layout.bounds.height <= 0.0 {
            return;
        }

        let border = if number_input.disabled {
            theme.border_fg
        } else if number_input.focused {
            theme.accent_fg
        } else {
            theme.border_fg
        };
        let fg = if number_input.disabled {
            theme.muted_fg
        } else {
            theme.surface_fg
        };

        surface.surface_fill_rect(layout.text_area, theme.input_bg);
        surface.surface_stroke_rect(layout.bounds, border, 1.0);

        let text = number_input.display_text();
        if !text.is_empty() {
            surface.surface_draw_text_run(layout.text_area, &text, fg);
        }

        surface.surface_stroke_rect(layout.increment, border, 1.0);
        surface.surface_stroke_rect(layout.decrement, border, 1.0);
        surface.surface_draw_text_run(layout.increment, "+", fg);
        surface.surface_draw_text_run(layout.decrement, "-", fg);
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::backend::ImagePaintResult;
        use crate::event::{Point, Rect, Viewport};
        use crate::types::{Color, WidgetId};
        use crate::Image;

        /// Records every surface verb `NumberInput`'s paint uses — mirrors
        /// `primitives::command_line`'s identical test double, so this
        /// test runs on any host without Cairo/Core Graphics/Direct2D.
        #[derive(Default)]
        struct RecordingSurface {
            fills: Vec<(Rect, Color)>,
            strokes: Vec<(Rect, Color)>,
            text_runs: Vec<(Rect, String, Color)>,
        }

        impl PaintSurface for RecordingSurface {
            fn surface_begin_frame(&mut self, _viewport: Viewport) {}
            fn surface_end_frame(&mut self) {}
            fn surface_viewport(&self) -> Viewport {
                Viewport::new(200.0, 100.0, 1.0)
            }
            fn surface_line_height(&self) -> f32 {
                16.0
            }
            fn surface_char_width(&self) -> f32 {
                8.0
            }
            fn surface_measure_text(&self, text: &str) -> (f32, f32) {
                (text.chars().count() as f32 * 8.0, 16.0)
            }
            fn surface_fill_rect(&mut self, rect: Rect, color: Color) {
                self.fills.push((rect, color));
            }
            fn surface_fill_rounded_rect(&mut self, rect: Rect, _radius: f32, color: Color) {
                self.fills.push((rect, color));
            }
            fn surface_stroke_rect(&mut self, rect: Rect, color: Color, _stroke_width: f32) {
                self.strokes.push((rect, color));
            }
            fn surface_stroke_rounded_rect(
                &mut self,
                _rect: Rect,
                _radius: f32,
                _color: Color,
                _stroke_width: f32,
            ) {
            }
            fn surface_draw_text_run(&mut self, rect: Rect, text: &str, color: Color) {
                self.text_runs.push((rect, text.to_string(), color));
            }
            fn surface_draw_line(
                &mut self,
                _from: Point,
                _to: Point,
                _color: Color,
                _stroke_width: f32,
            ) {
            }
            fn surface_push_clip(&mut self, _rect: Rect) {}
            fn surface_pop_clip(&mut self) {}
            fn surface_draw_image(&mut self, _rect: Rect, _image: &Image) -> ImagePaintResult {
                ImagePaintResult::Unsupported
            }
        }

        fn sample() -> (NumberInput, NumberInputLayout) {
            let n = NumberInput::new(WidgetId::new("settings:volume"), 0.0, 100.0);
            let layout = n.layout(Rect::new(0.0, 0.0, 10.0, 2.0));
            (n, layout)
        }

        #[test]
        fn paints_text_area_background_border_and_value() {
            let (n, layout) = sample();
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            paint(&n, &layout, &mut surface, &theme);

            assert_eq!(surface.fills[0], (layout.text_area, theme.input_bg));
            assert!(surface.strokes.contains(&(layout.bounds, theme.border_fg)));
            assert_eq!(surface.text_runs[0].1, "0");
        }

        #[test]
        fn shows_in_progress_draft_instead_of_committed_value() {
            let (mut n, layout) = sample();
            n.draft = Some("4".into());
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            paint(&n, &layout, &mut surface, &theme);

            assert_eq!(surface.text_runs[0].1, "4");
        }

        #[test]
        fn paints_both_stepper_buttons_with_their_own_border() {
            let (n, layout) = sample();
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            paint(&n, &layout, &mut surface, &theme);

            assert!(surface
                .strokes
                .contains(&(layout.increment, theme.border_fg)));
            assert!(surface
                .strokes
                .contains(&(layout.decrement, theme.border_fg)));
            assert!(surface
                .text_runs
                .iter()
                .any(|(r, t, _)| *r == layout.increment && t == "+"));
            assert!(surface
                .text_runs
                .iter()
                .any(|(r, t, _)| *r == layout.decrement && t == "-"));
        }

        #[test]
        fn focused_border_differs_from_unfocused_border() {
            let (mut n, layout) = sample();
            n.focused = true;
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            paint(&n, &layout, &mut surface, &theme);

            assert!(surface.strokes.contains(&(layout.bounds, theme.accent_fg)));
        }

        #[test]
        fn zero_size_bounds_paints_nothing() {
            let n = NumberInput::new(WidgetId::new("v"), 0.0, 100.0);
            let layout = n.layout(Rect::new(0.0, 0.0, 0.0, 0.0));
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            paint(&n, &layout, &mut surface, &theme);
            assert!(surface.fills.is_empty());
            assert!(surface.strokes.is_empty());
            assert!(surface.text_runs.is_empty());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> NumberInput {
        NumberInput::new(WidgetId::new("settings:volume"), 0.0, 100.0)
    }

    #[test]
    fn new_clamps_default_value_into_range_and_starts_enabled_unfocused() {
        let n = input();
        assert_eq!(n.value, 0.0);
        assert!(!n.disabled);
        assert!(!n.focused);
        assert_eq!(n.draft, None);

        let negative_range = NumberInput::new(WidgetId::new("x"), 5.0, 10.0);
        assert_eq!(negative_range.value, 5.0);
    }

    #[test]
    fn layout_splits_text_area_and_stacked_stepper_buttons() {
        let n = input();
        let layout = n.layout(Rect::new(0.0, 0.0, 10.0, 2.0));
        assert_eq!(
            layout.hit_test(Point::new(5.0, 0.0)),
            NumberInputHit::TextArea
        );
        assert_eq!(
            layout.hit_test(Point::new(9.0, 0.0)),
            NumberInputHit::Increment
        );
        assert_eq!(
            layout.hit_test(Point::new(9.0, 1.0)),
            NumberInputHit::Decrement
        );
        assert_eq!(
            layout.hit_test(Point::new(-1.0, -1.0)),
            NumberInputHit::Outside
        );
    }

    #[test]
    fn handle_key_up_down_step_value_when_focused() {
        let mut n = input();
        n.focused = true;
        assert_eq!(
            n.handle_key(&Key::Named(NamedKey::Up), &Modifiers::default()),
            Some(NumberInputEvent::ValueChanged {
                id: n.id.clone(),
                value: 1.0
            })
        );
        n.value = 1.0;
        assert_eq!(
            n.handle_key(&Key::Named(NamedKey::Down), &Modifiers::default()),
            Some(NumberInputEvent::ValueChanged {
                id: n.id.clone(),
                value: 0.0
            })
        );
    }

    #[test]
    fn handle_key_clamps_increment_at_max_and_decrement_at_min() {
        let mut n = input();
        n.focused = true;
        n.value = 100.0;
        assert_eq!(
            n.handle_key(&Key::Named(NamedKey::Up), &Modifiers::default()),
            Some(NumberInputEvent::ValueChanged {
                id: n.id.clone(),
                value: 100.0
            })
        );
        n.value = 0.0;
        assert_eq!(
            n.handle_key(&Key::Named(NamedKey::Down), &Modifiers::default()),
            Some(NumberInputEvent::ValueChanged {
                id: n.id.clone(),
                value: 0.0
            })
        );
    }

    #[test]
    fn typed_digits_accumulate_in_draft_and_enter_commits_the_parsed_value() {
        let mut n = input();
        n.focused = true;

        let e1 = n.handle_key(&Key::Char('4'), &Modifiers::default());
        assert_eq!(
            e1,
            Some(NumberInputEvent::DraftChanged {
                id: n.id.clone(),
                draft: "4".into()
            })
        );
        n.draft = Some("4".into());

        let e2 = n.handle_key(&Key::Char('2'), &Modifiers::default());
        assert_eq!(
            e2,
            Some(NumberInputEvent::DraftChanged {
                id: n.id.clone(),
                draft: "42".into()
            })
        );
        n.draft = Some("42".into());

        let commit = n.handle_key(&Key::Named(NamedKey::Enter), &Modifiers::default());
        assert_eq!(
            commit,
            Some(NumberInputEvent::Committed {
                id: n.id.clone(),
                value: 42.0
            })
        );
    }

    #[test]
    fn invalid_draft_reverts_to_the_current_value_on_commit() {
        let mut n = input();
        n.focused = true;
        n.value = 7.0;
        n.draft = Some("--".into());
        assert_eq!(
            n.handle_key(&Key::Named(NamedKey::Enter), &Modifiers::default()),
            Some(NumberInputEvent::Committed {
                id: n.id.clone(),
                value: 7.0
            })
        );
    }

    #[test]
    fn commit_clamps_and_rounds_to_precision() {
        let mut n = NumberInput::new(WidgetId::new("settings:volume"), 0.0, 10.0);
        n.focused = true;
        n.precision = 1;
        n.draft = Some("999.456".into());
        assert_eq!(
            n.handle_key(&Key::Named(NamedKey::Enter), &Modifiers::default()),
            Some(NumberInputEvent::Committed {
                id: n.id.clone(),
                value: 10.0
            })
        );
    }

    #[test]
    fn escape_cancels_a_pending_draft_without_touching_value() {
        let mut n = input();
        n.focused = true;
        n.draft = Some("5".into());
        assert_eq!(
            n.handle_key(&Key::Named(NamedKey::Escape), &Modifiers::default()),
            Some(NumberInputEvent::Cancelled { id: n.id.clone() })
        );
    }

    #[test]
    fn escape_is_inert_when_there_is_no_pending_draft() {
        let mut n = input();
        n.focused = true;
        assert_eq!(
            n.handle_key(&Key::Named(NamedKey::Escape), &Modifiers::default()),
            None
        );
    }

    #[test]
    fn handle_key_is_inert_when_not_focused() {
        let n = input();
        assert_eq!(
            n.handle_key(&Key::Named(NamedKey::Up), &Modifiers::default()),
            None
        );
    }

    #[test]
    fn handle_key_is_inert_when_disabled() {
        let mut n = input();
        n.focused = true;
        n.disabled = true;
        assert_eq!(
            n.handle_key(&Key::Named(NamedKey::Up), &Modifiers::default()),
            None
        );
    }

    #[test]
    fn click_dispatches_to_increment_and_decrement_without_requiring_focus() {
        let n = input();
        assert_eq!(
            n.click(NumberInputHit::Increment),
            Some(NumberInputEvent::ValueChanged {
                id: n.id.clone(),
                value: 1.0
            })
        );
        assert_eq!(n.click(NumberInputHit::TextArea), None);
        assert_eq!(n.click(NumberInputHit::Outside), None);
    }

    #[test]
    fn click_is_inert_when_disabled() {
        let mut n = input();
        n.disabled = true;
        assert_eq!(n.click(NumberInputHit::Increment), None);
    }

    #[test]
    fn handle_wheel_steps_value_without_requiring_focus() {
        let n = input();
        assert_eq!(
            n.handle_wheel(ScrollDelta::new(0.0, 1.0), 2.0),
            Some(NumberInputEvent::ValueChanged {
                id: n.id.clone(),
                value: 2.0
            })
        );
    }

    #[test]
    fn handle_wheel_is_inert_when_disabled_or_zero_delta() {
        let mut n = input();
        n.disabled = true;
        assert_eq!(n.handle_wheel(ScrollDelta::new(0.0, 1.0), 2.0), None);

        let n = input();
        assert_eq!(n.handle_wheel(ScrollDelta::new(0.0, 0.0), 2.0), None);
    }

    #[test]
    fn serde_round_trip() {
        let mut n = input();
        n.value = 42.0;
        n.focused = true;
        n.draft = Some("4".into());
        let json = serde_json::to_string(&n).unwrap();
        let back: NumberInput = serde_json::from_str(&json).unwrap();
        assert_eq!(n, back);
    }

    #[test]
    fn serde_defaults_optional_fields_when_omitted() {
        let json = r#"{"id":"settings:volume","min":0.0,"max":100.0}"#;
        let n: NumberInput = serde_json::from_str(json).unwrap();
        assert_eq!(n.value, 0.0);
        assert_eq!(n.step, 1.0);
        assert_eq!(n.precision, 0);
        assert_eq!(n.draft, None);
        assert!(!n.disabled);
        assert!(!n.focused);
    }

    // ── Cell-path paint (`cell_paint::paint`) ───────────────────────────

    #[cfg(feature = "tui")]
    mod cells {
        use super::super::cell_test_paint::draw_number_input;
        use super::*;
        use crate::theme::Theme;
        use ratatui::buffer::Buffer;
        use ratatui::layout::Rect as RRect;

        fn cell_char(buf: &Buffer, x: u16, y: u16) -> char {
            buf[(x, y)].symbol().chars().next().unwrap_or(' ')
        }

        fn mk_number_input() -> NumberInput {
            NumberInput {
                id: WidgetId::new("num"),
                value: 3.0,
                min: 0.0,
                max: 10.0,
                step: 1.0,
                precision: 0,
                draft: None,
                disabled: false,
                focused: true,
            }
        }

        fn paint_and_click_round_trip_at(origin_x: u16, origin_y: u16) {
            let bounds = Rect::new(origin_x as f32, origin_y as f32, 10.0, 3.0);
            let number_input = mk_number_input();
            let layout = number_input.layout(bounds);

            let rect = RRect::new(origin_x, origin_y, 10, 3);
            let mut buf = Buffer::empty(rect);
            draw_number_input(&mut buf, &number_input, &layout, &Theme::default());

            // The value renders inside the text area, painted on top of
            // the border (matching `native_surface_paint`'s fill ->
            // stroke -> text layering, where the text run is drawn last).
            let text_x = layout.text_area.x as u16;
            let text_y = layout.text_area.y as u16;
            assert_eq!(cell_char(&buf, text_x, text_y), '3');

            // The border itself is still visible on an edge the text
            // never reaches (the bottom-left corner, one row below the
            // text).
            let border_y = origin_y + layout.bounds.height.round() as u16 - 1;
            assert_eq!(cell_char(&buf, origin_x, border_y), '└');

            // hit_test: a click inside the increment sub-rect resolves to
            // the Increment variant at the absolute screen position.
            let p = Point::new(
                layout.increment.x + layout.increment.width / 2.0,
                layout.increment.y + layout.increment.height / 2.0,
            );
            assert_eq!(layout.hit_test(p), NumberInputHit::Increment);

            let p = Point::new(
                layout.decrement.x + layout.decrement.width / 2.0,
                layout.decrement.y + layout.decrement.height / 2.0,
            );
            assert_eq!(layout.hit_test(p), NumberInputHit::Decrement);
        }

        #[test]
        fn paint_and_click_round_trip() {
            paint_and_click_round_trip_at(0, 0);
        }

        /// Paint and hit-test must work correctly when the control is not
        /// at the screen origin.
        #[test]
        fn paint_and_click_round_trip_at_nonzero_origin() {
            paint_and_click_round_trip_at(7, 13);
        }

        #[test]
        fn stepper_cells_carry_their_glyphs() {
            let n = mk_number_input();
            let layout = n.layout(Rect::new(0.0, 0.0, 10.0, 4.0));
            let mut buf = Buffer::empty(RRect::new(0, 0, 10, 4));
            draw_number_input(&mut buf, &n, &layout, &Theme::default());

            assert_eq!(
                cell_char(&buf, layout.increment.x as u16, layout.increment.y as u16),
                '+'
            );
            assert_eq!(
                cell_char(&buf, layout.decrement.x as u16, layout.decrement.y as u16),
                '-'
            );
        }

        #[test]
        fn an_in_progress_draft_paints_instead_of_the_committed_value() {
            let mut n = mk_number_input();
            n.draft = Some("42".into());
            let layout = n.layout(Rect::new(0.0, 0.0, 10.0, 3.0));
            let mut buf = Buffer::empty(RRect::new(0, 0, 10, 3));
            draw_number_input(&mut buf, &n, &layout, &Theme::default());

            let x = layout.text_area.x as u16;
            let y = layout.text_area.y as u16;
            assert_eq!(cell_char(&buf, x, y), '4');
            assert_eq!(cell_char(&buf, x + 1, y), '2');
        }

        #[test]
        fn zero_size_bounds_paints_nothing() {
            let n = mk_number_input();
            let layout = n.layout(Rect::new(0.0, 0.0, 0.0, 0.0));
            let mut buf = Buffer::empty(RRect::new(0, 0, 10, 3));
            draw_number_input(&mut buf, &n, &layout, &Theme::default());
            assert_eq!(cell_char(&buf, 0, 0), ' ');
        }
    }
}

/// [`Interactive`] dispatch, driven directly the way an out-of-process
/// binding host reaches a widget — the in-runtime leg is
/// `tests/widget_contract_number_input.rs`.
#[cfg(test)]
mod interactive_tests {
    use super::*;
    use crate::interactive::{EventHost, SubTarget};
    use crate::PointerShape;

    /// Minimal [`EventHost`] mock: records every call so a test can
    /// assert on what [`Interactive::handle`] asked for.
    #[derive(Debug, Default)]
    struct MockHost {
        focus_requests: Vec<Option<SubTarget>>,
    }

    impl EventHost for MockHost {
        fn is_focused(&self, _id: &WidgetId, _sub: Option<SubTarget>) -> bool {
            false
        }
        fn request_focus(&mut self, _id: &WidgetId, sub: Option<SubTarget>) {
            self.focus_requests.push(sub);
        }
        fn capture_pointer(&mut self, _id: &WidgetId) {}
        fn release_pointer(&mut self) {}
        fn set_cursor(&mut self, _cursor: PointerShape) {}
        fn request_redraw(&mut self) {}
        fn emit(&mut self, _event: crate::UiEvent) {}
        fn units(&self) -> Units {
            Units::Cells
        }
    }

    fn input() -> NumberInput {
        let mut n = NumberInput::new(WidgetId::new("settings:volume"), 0.0, 100.0);
        n.focused = true;
        n
    }

    fn layout_for(n: &NumberInput) -> NumberInputPrimitiveLayout {
        let ctx = LayoutCtx::new(1.0, 1.0, false);
        Primitive::layout(n, &ctx, Rect::new(4.0, 2.0, 10.0, 4.0))
    }

    fn dispatch(n: &NumberInput, event: &RoutedEvent) -> Handled<NumberInputEvent> {
        let layout = layout_for(n);
        let mut host = MockHost::default();
        let mut cx = EventCx::new(n.id, &mut host);
        n.handle(&mut (), event, &layout, &mut cx)
    }

    fn pointer_down(local_x: f32, local_y: f32) -> RoutedEvent {
        RoutedEvent::Pointer {
            kind: PointerKind::Down(crate::MouseButton::Left),
            sub: None,
            local: Point::new(local_x, local_y),
            mods: Modifiers::default(),
        }
    }

    #[test]
    fn an_enabled_field_is_one_focus_stop_and_a_disabled_one_is_none() {
        assert_eq!(input().focus(), FocusSpec::Stop);
        let mut disabled = input();
        disabled.disabled = true;
        assert_eq!(disabled.focus(), FocusSpec::None);
    }

    #[test]
    fn a_click_on_the_stepper_cells_steps_the_value() {
        let n = input();
        let layout = layout_for(&n);
        // Local coordinates: the increment cell's own centre, relative to
        // the widget's painted rect.
        let inc = layout.layout.increment;
        let local_x = inc.x + inc.width / 2.0 - layout.area.x;
        let local_y = inc.y + inc.height / 2.0 - layout.area.y;
        assert_eq!(
            dispatch(&n, &pointer_down(local_x, local_y)),
            Handled::Event(NumberInputEvent::ValueChanged {
                id: n.id,
                value: 1.0
            })
        );

        let dec = layout.layout.decrement;
        let local_y = dec.y + dec.height / 2.0 - layout.area.y;
        assert_eq!(
            dispatch(&n, &pointer_down(local_x, local_y)),
            Handled::Event(NumberInputEvent::ValueChanged {
                id: n.id,
                value: -0.0
            })
        );
    }

    #[test]
    fn a_click_on_the_text_area_only_takes_focus() {
        let n = input();
        let layout = layout_for(&n);
        let mut host = MockHost::default();
        let mut cx = EventCx::new(n.id, &mut host);
        let handled = n.handle(&mut (), &pointer_down(1.0, 1.0), &layout, &mut cx);
        assert_eq!(handled, Handled::Consumed);
        assert_eq!(host.focus_requests, vec![None]);
    }

    #[test]
    fn a_wheel_notch_steps_by_the_fields_own_step() {
        let mut n = input();
        n.step = 5.0;
        let wheel = |dy: f32| RoutedEvent::Wheel {
            dx: 0.0,
            dy,
            local: Point::new(1.0, 1.0),
            mods: Modifiers::default(),
        };
        assert_eq!(
            dispatch(&n, &wheel(1.0)),
            Handled::Event(NumberInputEvent::ValueChanged {
                id: n.id,
                value: 5.0
            })
        );
        assert_eq!(dispatch(&n, &wheel(0.0)), Handled::Ignored);
    }

    #[test]
    fn arrow_keys_and_typed_digits_route_through_handle_key() {
        let n = input();
        let key = |key: Key| {
            RoutedEvent::Key(KeyEvent {
                key,
                modifiers: Modifiers::default(),
                repeat: false,
            })
        };
        assert_eq!(
            dispatch(&n, &key(Key::Named(NamedKey::Up))),
            Handled::Event(NumberInputEvent::ValueChanged {
                id: n.id,
                value: 1.0
            })
        );
        assert_eq!(
            dispatch(&n, &key(Key::Char('7'))),
            Handled::Event(NumberInputEvent::DraftChanged {
                id: n.id,
                draft: "7".into()
            })
        );
        // An unrecognised key stays unhandled, so a host's own binding
        // still sees it.
        assert_eq!(
            dispatch(&n, &key(Key::Named(NamedKey::Tab))),
            Handled::Ignored
        );
    }

    #[test]
    fn the_a11y_increment_and_decrement_actions_step_the_value() {
        let n = input();
        assert_eq!(
            dispatch(&n, &RoutedEvent::Action(A11yAction::Increment)),
            Handled::Event(NumberInputEvent::ValueChanged {
                id: n.id,
                value: 1.0
            })
        );
        assert_eq!(
            n.a11y_actions(&()),
            vec![A11yAction::Increment, A11yAction::Decrement]
        );
    }

    #[test]
    fn a_disabled_field_answers_nothing_and_offers_no_a11y_actions() {
        let mut n = input();
        n.disabled = true;
        let layout = layout_for(&n);
        let inc = layout.layout.increment;
        let local_x = inc.x + inc.width / 2.0 - layout.area.x;
        let local_y = inc.y + inc.height / 2.0 - layout.area.y;
        assert_eq!(
            dispatch(&n, &pointer_down(local_x, local_y)),
            Handled::Ignored
        );
        assert!(n.a11y_actions(&()).is_empty());
    }

    #[test]
    fn every_outcome_rides_the_form_event_channel_under_the_fields_id() {
        let id = WidgetId::new("settings:volume");
        assert_eq!(
            NumberInputEvent::Committed { id, value: 4.0 }.into_ui_event(id),
            crate::UiEvent::Form(id, crate::FormEvent::NumberCommitted { id, value: 4.0 })
        );
        assert_eq!(
            NumberInputEvent::Cancelled { id }.into_ui_event(id),
            crate::UiEvent::Form(id, crate::FormEvent::NumberCancelled { id })
        );
    }

    #[test]
    fn sub_regions_name_the_stepper_cells_before_the_text_area() {
        let n = input();
        let layout = layout_for(&n);
        let regions = n.sub_regions(&layout);
        assert_eq!(
            regions.iter().map(|(_, s)| s.0).collect::<Vec<_>>(),
            vec![SUB_INCREMENT, SUB_DECREMENT, SUB_TEXT_AREA]
        );
        // Local space: every rect sits inside the widget's own area.
        for (rect, _) in &regions {
            assert!(rect.x >= 0.0 && rect.y >= 0.0, "{rect:?} is not local");
        }
    }

    #[test]
    fn measure_reports_the_display_text_plus_chrome_and_the_stepper_column() {
        let n = input();
        let ctx = LayoutCtx::new(1.0, 1.0, false);
        let size = Primitive::measure(&n, &ctx, Size::new(40.0, 10.0));
        // "0" is one column wide, plus two border columns and the
        // three-column stepper.
        assert_eq!(size.width, Some(1.0 + 2.0 + STEPPER_WIDTH));
        assert_eq!(size.height, Some(3.0));
    }
}

/// Cell-painting entry points used by this module's own `#[cfg(test)]`
/// rasteriser tests: they build the `Buffer` -> [`crate::CellSurface`]
/// adapter (private to `crate::tui`) the same way
/// [`crate::primitive::Primitive::paint_cells`] does, so a test can
/// paint into a bare ratatui buffer without a backend.
#[cfg(all(test, feature = "tui"))]
mod cell_test_paint {
    use super::*;
    use crate::theme::Theme;
    use ratatui::buffer::Buffer;

    fn surface<'a>(
        buf: &'a mut Buffer,
        sink: &'a mut Option<(u16, u16, crate::cell_surface::CursorShape)>,
    ) -> impl crate::CellSurface + 'a {
        crate::tui::cell_surface::TuiCellSurface::over(
            buf,
            crate::backend::ColorDepth::TrueColor,
            sink,
        )
    }

    pub fn draw_number_input(
        buf: &mut Buffer,
        number_input: &NumberInput,
        layout: &NumberInputLayout,
        theme: &Theme,
    ) {
        let mut sink = None;
        cell_paint::paint(number_input, layout, &mut surface(buf, &mut sink), theme);
    }
}
