//! [`Form`]'s widget contract: [`Primitive`] (one measure, one layout,
//! one pixel paint, one cell paint) plus [`Interactive`] (focus
//! traversal, click routing and per-field keyboard editing), so a form
//! reaches every backend through
//! [`crate::backend::DrawsPrimitives::draw_primitive`]/`draw_dyn` and
//! sits in a [`crate::frame_tree::FrameTree`] as a
//! [`crate::frame::Surface::Widget`].
//!
//! ## One layout, both units
//!
//! [`Primitive::layout`] branches on [`crate::primitive::Units`]: a cell
//! grid resolves through [`super::cell_layout`] (the exact resolver the
//! TUI rasteriser paints from), a pixel surface through
//! [`crate::primitives::layout_metrics::form_field_measure`] fed by the
//! calling backend's own [`crate::Backend::measure_text`] — the same
//! measurer every GUI `form_layout` wrapper already uses. Both produce a
//! [`FormLayout`] in the form's **own local space** (origin at the
//! painted rect), the space [`Interactive::sub_regions`] and
//! [`crate::interactive::RoutedEvent::Pointer`]'s `local` both use, so
//! hit-testing needs no translation. [`Primitive::paint`] hands the
//! painted rect's origin to the pixel painter, which draws in
//! surface-absolute coordinates.
//!
//! ## Composition, not a second painter
//!
//! A form owns its rows — the label column, the focused-row wash, the
//! validation strip — and nothing else. Every field *control* is one of
//! this crate's own widgets, painted by that widget's own rasteriser
//! (`NumberInput` through [`crate::tui::draw_number_input`], `Toolbar`
//! through [`crate::primitives::toolbar::cell_paint`], `Switch` /
//! `RadioGroup` / `Button` / `TextInput` / `TextField` through the
//! glyph helpers those modules export), and answered by that widget's
//! own key/click model inside [`Interactive::handle`]. A field kind
//! whose control is an anchored popup (`ComboBox`, `DatePicker`,
//! `ColorPicker`, an open `Dropdown` list) paints its compact row here
//! and leaves the popup to the host, which is why those rows are a
//! value plus an affordance rather than the whole embedded widget.
//!
//! ## Interaction state
//!
//! [`FormState`] is the live counterpart of what the host used to carry
//! for a form between frames: the scroll offset, the text cursor /
//! selection anchor of whichever text field is focused, the date
//! picker's scrub position, and the item cursor inside a multi-item row.
//! The descriptor stays the source of truth for every *value* — the
//! host applies the [`FormEvent`]s this widget emits and pushes the new
//! values back on the next frame, exactly the contract
//! [`super`]'s module doc already describes.

use crate::a11y::A11yAction;
use crate::event::{Key, MouseButton, NamedKey, Point, Rect};
use crate::interactive::{
    EventCx, FocusSpec, Interactive, PointerKind, RoutedEvent, SubTarget, WidgetAction,
};
use crate::primitive::{LayoutCtx, PaintState, Primitive, Size};
use crate::primitives::combo_box::ComboBoxEvent;
use crate::primitives::date_picker::DatePickerEvent;
use crate::primitives::number_input::NumberInputEvent;
use crate::primitives::radio_group::RadioGroupEvent;
use crate::text_util::snap_to_char_boundary;
use crate::types::{Modifiers, WidgetId};
use crate::{EditOp, Handled};

use super::{
    focusable_items, FieldKind, Form, FormEvent, FormHit, FormLayout, LabelPosition,
    VisibleFormField,
};

/// [`Primitive::Layout`] for [`Form`]: the resolved [`FormLayout`] plus
/// the area it was resolved for.
///
/// `#[non_exhaustive]`: a future field add here is additive, the same
/// convention [`FormLayout`] itself follows.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub struct FormPrimitiveLayout {
    /// The rect the layout was resolved for, in surface coordinates.
    /// [`FormLayout`]'s own rects are local to this rect's origin.
    pub area: Rect,
    /// The resolved rows, item rects, hit regions and clamped scroll
    /// offset — form-local, exactly as [`Form::layout`] produced them.
    pub layout: FormLayout,
}

impl FormPrimitiveLayout {
    /// Which field sits under form-local `(x, y)` — the descriptor's own
    /// [`FormLayout::hit_test`], named here so a caller holding this
    /// layout doesn't have to reach through the `layout` field for it.
    pub fn hit_test(&self, x: f32, y: f32) -> FormHit {
        self.layout.hit_test(x, y)
    }

    /// The resolved row for field `id`, or `None` when `id` names no
    /// field on this form or the field is scrolled out of the viewport.
    pub fn visible_field(&self, id: &WidgetId) -> Option<&VisibleFormField> {
        self.layout.visible_fields.iter().find(|f| &f.id == id)
    }
}

/// Viewport height [`Primitive::measure`] lays a form out against to
/// learn its natural height — a finite stand-in for "unbounded" that no
/// real form's rows can exceed, and small enough that `f32` row
/// positions within it stay exact.
const UNBOUNDED_PROBE_HEIGHT: f32 = 1.0e6;

/// `0, 0`-origin cell rect of `rect` — the area
/// [`super::cell_layout::tui_form_layout`] resolves against, so the
/// layout a hit test reads and the cells a paint writes can't disagree
/// about the form's width by a rounding step on a fractional rect.
#[cfg(feature = "tui")]
fn cell_area(rect: Rect) -> ratatui::layout::Rect {
    let absolute = crate::tui::backend::q_rect_to_ratatui(rect);
    ratatui::layout::Rect {
        x: 0,
        y: 0,
        width: absolute.width,
        height: absolute.height,
    }
}

impl Primitive for Form {
    type Layout = FormPrimitiveLayout;
    type Event = FormEvent;

    fn id(&self) -> Option<&WidgetId> {
        Some(&self.id)
    }

    /// Round-trips through [`crate::frame::OwnedContent::Form`] — the
    /// same owned snapshot [`crate::frame::Surface::Form`] captures
    /// into, replayed through `Backend::draw_form`, which reaches this
    /// module's own layout and painters.
    fn capture(&self) -> crate::frame::OwnedContent {
        crate::frame::OwnedContent::Form(self.clone())
    }

    /// The form's own intrinsic size: the widest row's content width and
    /// the summed height of every field, so a container that sizes a
    /// form to its content gets a box that needs no scrolling.
    ///
    /// Measured by laying the form out against `avail` — row heights
    /// are per-[`FieldKind`] (a `TextArea`, an open `Dropdown` and a
    /// multi-option `RadioGroup` each claim more than one row), so
    /// there is no shorter formula that stays in step with
    /// [`Self::layout`].
    fn measure(&self, ctx: &LayoutCtx, avail: Size) -> crate::NaturalSize {
        // A finite stand-in for "unbounded": tall enough that no row is
        // clipped out of `visible_fields`, small enough that the row
        // positions stay exact in `f32`.
        let probe = Rect::new(0.0, 0.0, avail.width, UNBOUNDED_PROBE_HEIGHT);
        let resolved = Primitive::layout(self, ctx, probe);
        let height = resolved
            .layout
            .visible_fields
            .iter()
            .map(|f| f.bounds.y + f.bounds.height)
            .fold(0.0_f32, f32::max);
        let width = resolved
            .layout
            .visible_fields
            .iter()
            .map(|f| f.bounds.x + f.bounds.width)
            .fold(0.0_f32, f32::max);
        crate::NaturalSize::new(Some(width.min(avail.width)), Some(height))
    }

    /// Resolve the form-relative layout for `rect`, in whichever unit
    /// the caller measures in — see this module's own doc.
    fn layout(&self, ctx: &LayoutCtx, rect: Rect) -> Self::Layout {
        // `ctx.units()` is a reliable *runtime* discriminator between a
        // TUI caller and a pixel-backend one even when both features are
        // compiled into the same binary — see `LayoutCtx::units`'s own
        // doc.
        #[cfg(feature = "tui")]
        {
            if matches!(ctx.units(), crate::primitive::Units::Cells) {
                return FormPrimitiveLayout {
                    area: rect,
                    layout: super::cell_layout::tui_form_layout(self, cell_area(rect)),
                };
            }
        }
        // Labels, values and hints are chrome — painted in the backend's
        // UI font by `super::native_surface_paint` — so this measures
        // them through the calling backend's own `Backend::measure_text`
        // for `FontRole::Chrome`, the exact measurer every pixel
        // `form_layout` wrapper shares. Without a measurer in reach
        // (`LayoutCtx::new`) it degrades to the `ctx.char_width`
        // approximation.
        let measure = crate::primitives::layout_metrics::LayoutCtxMeasure {
            ctx,
            role: crate::FontRole::Chrome,
        };
        let row_h = crate::primitives::layout_metrics::form_row_height(ctx.line_height as f64);
        let layout = self.layout(rect.width, rect.height, |i| {
            crate::primitives::layout_metrics::form_field_measure(&self.fields[i], row_h, &measure)
        });
        FormPrimitiveLayout { area: rect, layout }
    }

    /// Runs the shared pixel painter every GUI `draw_form` wrapper
    /// already paints through, at the already-resolved layout, with the
    /// painted rect's origin as the form's local `(0, 0)`.
    fn paint(
        &self,
        layout: &Self::Layout,
        surface: &mut dyn crate::paint_surface::PaintSurface,
        theme: &crate::theme::Theme,
        style: &crate::style::Style,
    ) {
        #[cfg(any(
            feature = "gtk",
            feature = "win",
            all(feature = "macos", target_os = "macos")
        ))]
        {
            super::native_surface_paint::paint(
                self,
                &layout.layout,
                surface,
                theme,
                crate::Point::new(layout.area.x, layout.area.y),
            );
            let _ = style;
        }
        #[cfg(not(any(
            feature = "gtk",
            feature = "win",
            all(feature = "macos", target_os = "macos")
        )))]
        {
            let _ = (layout, surface, theme, style);
        }
    }

    /// Delegates to the same [`super::cell_paint::draw_form`] entry
    /// point `TuiBackend::draw_form` itself calls — so painting a form
    /// through this contract is byte-for-byte identical to painting it
    /// through the legacy `Backend` pair by construction, not by
    /// separately maintained parity.
    #[cfg(feature = "tui")]
    fn paint_cells(
        &self,
        layout: &Self::Layout,
        buf: &mut ratatui::buffer::Buffer,
        theme: &crate::theme::Theme,
    ) {
        let area = crate::tui::backend::q_rect_to_ratatui(layout.area);
        super::cell_paint::draw_form(buf, area, self, theme);
    }
}



// ── Interaction state ───────────────────────────────────────────────

/// Live caret and selection anchor for the field currently being typed
/// into. The *value* is **not** cached here — it is always read fresh
/// off the descriptor (the host's own canonical string), so the host
/// staying the single source of truth for field content still holds;
/// only cursor placement lives here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextCursor {
    /// The field this caret belongs to, so moving focus away and back
    /// can't resurrect a stale caret on a different field.
    pub field_id: WidgetId,
    /// Byte offset of the caret within the field's value.
    pub cursor: usize,
    /// Byte offset the current selection started at, if any.
    pub selection_anchor: Option<usize>,
}

/// Live focused-day / visible-month for the [`FieldKind::DatePicker`]
/// field currently being navigated. Mirrors [`TextCursor`]'s split: the
/// host stays the source of truth for the confirmed `selected` value
/// (pushed back via [`FormEvent::DateChanged`] once Enter confirms a
/// day), while the scrubbing position between keystrokes lives here so
/// the arrow keys keep moving the calendar without a host round-trip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DateCursor {
    /// The field this scrub position belongs to.
    pub field_id: WidgetId,
    /// The day the cursor is on, confirmed or not.
    pub focused: crate::primitives::date_picker::CalendarDate,
    /// The month on screen, which a scrub past a month edge advances.
    pub visible_month: crate::primitives::date_picker::CalendarDate,
}

/// Which item inside the focused `ButtonRow` / `ToggleGroup` /
/// `Toolbar` field holds keyboard item-focus.
///
/// `item_idx` indexes into
/// [`crate::primitives::form::focusable_items`]'s enumeration of the
/// field's items, not into the field's items directly — they are the
/// same order, but going through that one shared enumeration keeps this
/// from drifting out of step with [`current_item_focus`]'s own reading
/// of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ItemCursor {
    /// The field whose items this cursor walks.
    pub field_id: WidgetId,
    /// Index into that field's `focusable_items` enumeration.
    pub item_idx: usize,
}

/// [`current_item_focus`]'s return shape: the focused field's id, the
/// focused item's index into `focusable_items`'s enumeration, and that
/// enumeration itself.
pub(crate) type ItemFocusLookup = (WidgetId, usize, Vec<(WidgetId, bool)>);

/// Live interaction state for one painted [`Form`] — the editing
/// position the descriptor deliberately doesn't carry.
///
/// Every *value* stays on the descriptor: the host applies the
/// [`FormEvent`]s [`Interactive::handle`] emits and pushes the new
/// values back on the next frame. What lives here is the position
/// within a value that a keystroke moves but no event names — the text
/// caret, the calendar scrub cursor, the item cursor inside a
/// multi-item row — plus the scroll offset and the in-flight slider
/// drag, so a form scrolls and scrubs without a host round-trip.
///
/// `#[non_exhaustive]`: a future field add here stays additive for a
/// host that only ever reaches this through
/// [`crate::widget_state::WidgetStateStore`].
#[non_exhaustive]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FormState {
    /// Rows scrolled off the top. `None` means "follow the
    /// descriptor's own [`Form::scroll_offset`]" — set the moment a
    /// wheel, a page key or a scroll-into-view moves the viewport, so a
    /// host that drives scrolling itself keeps control until something
    /// here scrolls.
    pub scroll_offset: Option<usize>,
    /// Caret and selection anchor within the focused text field.
    pub text_edit: Option<TextCursor>,
    /// The focused date picker's live scrub position.
    pub date_edit: Option<DateCursor>,
    /// Cursor within the focused multi-item row's items.
    pub item_focus: Option<ItemCursor>,
    /// The `FieldKind::Slider` field a press captured the pointer for,
    /// read by every following pointer move so the value keeps tracking
    /// the cursor, and cleared on release.
    pub slider_drag: Option<WidgetId>,
}

impl PaintState for FormState {}

impl FormState {
    /// The scroll offset to lay out with: whatever a wheel, page key or
    /// scroll-into-view put here, else the descriptor's own.
    pub fn live_scroll_offset(&self, form: &Form) -> usize {
        self.scroll_offset.unwrap_or(form.scroll_offset)
    }

    /// Scroll so field index `field_idx` is inside a `viewport_rows`-tall
    /// viewport, moving the offset by the least amount that does it.
    pub fn scroll_to_field(&mut self, form: &Form, field_idx: usize, viewport_rows: usize) {
        self.scroll_offset = Some(crate::behaviour::scroll::ensure_visible(
            self.live_scroll_offset(form),
            viewport_rows,
            field_idx,
        ));
    }

    /// Scroll by `delta` rows, clamped so the last row can't scroll past
    /// the top of a `viewport_rows`-tall viewport.
    pub fn scroll_by(&mut self, form: &Form, delta: isize, viewport_rows: usize) {
        let max = form.fields.len().saturating_sub(viewport_rows);
        self.scroll_offset = Some(crate::primitives::scrollbar::scroll_by_clamped(
            self.live_scroll_offset(form),
            delta,
            max,
        ));
    }
}

// ── Behaviour: keyboard, focus traversal and click routing ──────────
//
// The one implementation of this widget's behaviour. `Interactive` below
// drives it from a `RoutedEvent`; `crate::compose::FormController` drives
// the same functions from a `UiEvent`, so the two paths resolve every key
// and click identically by construction.

pub(crate) fn handle_key(
    form: &Form,
    state: &mut FormState,
    key: &Key,
    modifiers: &Modifiers,
    viewport_rows: usize,
) -> Handled<FormEvent> {
    // `DatePicker` claims Left/Right/Up/Down/PageUp/PageDown/Enter
    // ahead of the generic dispatch below — those keys would
    // otherwise fall through to `EditOp::from_key` (a no-op for a
    // non-text field) or, for `Enter`, to `commit_focused_text`
    // (also a no-op). `Tab`/`BackTab` aren't in this set, so focus
    // traversal off a `DatePicker` field still works normally.
    //
    // Guarded the same way the `Tab` arm below is: without the
    // modifier check, a host accelerator like Ctrl+Enter would be
    // swallowed into `navigate` whenever a date field has focus
    // instead of falling through to the app. A disabled focused
    // field is excluded too, so its keys reach the generic dispatch
    // below (which is a no-op for a non-text field) rather than
    // being eaten outright.
    if !modifiers.ctrl && !modifiers.alt && !modifiers.cmd {
        if let Some(key_name) = date_picker_key_name(key) {
            if focused_field_is_enabled_date_picker(form) {
                return handle_date_picker_key(form, state, &key_name);
            }
        }
        // `ComboBox` has no pre-filtered key set the way
        // `date_picker_key_name` gives `DatePicker` above — it needs
        // Char/Backspace too, not just navigation keys — so
        // `handle_combo_box_key` returns `Option` instead and a
        // `None` here (Tab/BackTab, Escape while closed, Up/Down
        // over an empty filtered list) falls through to the match
        // below rather than being swallowed outright. Tab/BackTab have
        // a dedicated arm there; Escape and Up/Down don't, so those
        // fall all the way to `EditOp::from_key` and resolve to
        // `Handled::Ignored`, not a second round of
        // combo-box-specific handling.
        if focused_field_is_enabled_combo_box(form) {
            if let Some(result) = handle_combo_box_key(form, key, modifiers) {
                return result;
            }
        }
        // `Dropdown` has no open/close step to track (unlike
        // `ComboBox`'s `open`/`highlighted`) — Up/Down move and commit
        // `selected_idx` in the same keystroke, so `handle_dropdown_key`
        // only ever claims those two keys and returns `None` for
        // everything else (Tab/BackTab, Enter), which falls through to
        // the generic dispatch below exactly like an unclaimed
        // combo-box key does.
        if focused_field_is_enabled_dropdown(form) {
            if let Some(result) = handle_dropdown_key(form, key) {
                return result;
            }
        }
        // `ButtonRow`/`ToggleGroup`/`Toolbar` item focus — see this
        // module's "Item-level keyboard focus" doc section.
        // `handle_item_row_key` returns `None` for every key it
        // doesn't claim (Tab/BackTab among them), which falls
        // through to the match below exactly like an unclaimed
        // combo-box/dropdown key does.
        if focused_field_is_enabled_item_row(form) {
            if let Some(result) = handle_item_row_key(form, state, key) {
                return result;
            }
        }
        // `NumberInput`/`RadioGroup` — see this module's "Number
        // input fields"/"Radio group fields" doc sections. Both
        // return `Option` for the same reason `handle_combo_box_key`
        // does: an unclaimed key (Tab/BackTab among them) must fall
        // through to the generic dispatch below.
        if focused_field_is_enabled_number_input(form) {
            if let Some(result) = handle_number_input_key(form, key, modifiers) {
                return result;
            }
        }
        if focused_field_is_enabled_radio_group(form) {
            if let Some(result) = handle_radio_group_key(form, key, modifiers) {
                return result;
            }
        }
        // `Slider` — see [`slider_value_at`]'s own doc.
        // `handle_slider_key` returns `None` for every key it
        // doesn't claim (Tab/BackTab among them), which falls through
        // to the generic dispatch below exactly like an unclaimed
        // number-input/radio-group key does.
        if focused_field_is_enabled_slider(form) {
            if let Some(result) = handle_slider_key(form, key) {
                return result;
            }
        }
        // Plain `Toggle` — see this module's "Toggle fields" doc
        // section. Mirrors `handle_item_row_key`'s Enter/Space arm:
        // `handle_toggle_key` returns `None` for every other key,
        // which falls through to the match below exactly like an
        // unclaimed item-row/number-input/radio-group key does.
        if focused_field_is_enabled_toggle(form) {
            if let Some(result) = handle_toggle_key(form, key) {
                return result;
            }
        }
    }
    match key {
        Key::Named(NamedKey::Tab) if !modifiers.ctrl && !modifiers.alt && !modifiers.cmd => {
            move_focus(form, state, if modifiers.shift { -1 } else { 1 }, viewport_rows)
        }
        Key::Named(NamedKey::BackTab) => move_focus(form, state, -1, viewport_rows),
        // `Enter` commits for `TextInput`/`PasswordInput` (a single
        // commit boundary), but `TextArea` is the one field kind
        // that is meant to hold newlines, so `Enter` inserts one
        // there instead — same split VS Code makes between a
        // single-line input and a multi-line text area.
        Key::Named(NamedKey::Enter) if focused_field_is_text_area(form) => {
            handle_text_op(form, state, EditOp::InsertChar('\n'))
        }
        Key::Named(NamedKey::Enter) => commit_focused_text(form, state),
        // Every backend delivers Ctrl/Cmd+<letter> as `Key::Char`
        // with the modifier set (so an unregistered accelerator can
        // still be recovered), not as a dedicated key variant — see
        // `EditOp::from_key`'s callers in `examples/common/text_input_demo.rs`
        // for the same guard. Without this, Ctrl+C/Ctrl+V/Ctrl+W (or
        // Cmd+<letter> on macOS) would type a stray literal character
        // into the focused field instead of falling through to an
        // app-level accelerator.
        Key::Char(_) if modifiers.ctrl || modifiers.cmd => Handled::Ignored,
        _ => match EditOp::from_key(key, *modifiers) {
            Some(op) => handle_text_op(form, state, op),
            None => Handled::Ignored,
        },
    }
}

/// Whether the currently-focused field is a `TextArea` — gives
/// `Enter` its kind-specific meaning (insert a newline there, commit
/// everywhere else).
fn focused_field_is_text_area(form: &Form) -> bool {
    let Some(focused_id) = &form.focused_field else {
        return false;
    };
    form.fields
        .iter()
        .find(|f| &f.id == focused_id)
        .is_some_and(|f| matches!(f.kind, FieldKind::TextArea { .. }))
}

/// Whether the currently-focused field is a non-disabled
/// `DatePicker` — gates `handle_key`'s date-navigation interception.
/// Excluding the disabled case here (rather than inside
/// `handle_date_picker_key`) lets its keys fall through to the
/// generic dispatch below instead of being swallowed outright.
fn focused_field_is_enabled_date_picker(form: &Form) -> bool {
    let Some(focused_id) = &form.focused_field else {
        return false;
    };
    form.fields
        .iter()
        .find(|f| &f.id == focused_id)
        .is_some_and(|f| !f.disabled && matches!(f.kind, FieldKind::DatePicker(_)))
}

/// Route `key_name` into the focused `DatePicker`'s own
/// [`crate::primitives::date_picker::DatePicker::navigate`] and apply
/// the result: a `FocusMoved` updates the cached scrubbing position
/// (see `DateCursor`) without touching the app's `selected` value,
/// while a `DateSelected` (Enter) clears the cache and hands the app
/// [`FormEvent::DateChanged`] to persist.
fn handle_date_picker_key(form: &Form, state: &mut FormState, key_name: &str) -> Handled<FormEvent> {
    let Some(focused_id) = form.focused_field else {
        return Handled::Ignored;
    };
    let Some(field) = form.fields.iter().find(|f| f.id == focused_id) else {
        return Handled::Ignored;
    };
    if field.disabled {
        return Handled::Ignored;
    }
    let FieldKind::DatePicker(picker) = &field.kind else {
        return Handled::Ignored;
    };

    // Overlay the cached scrubbing position before calling `navigate`
    // so consecutive keystrokes compose on top of each other instead
    // of resetting to the app-provided `focused`/`visible_month`
    // every time (the app only ever sees the field again once a
    // `DateChanged` round-trip persists a new `selected`).
    let mut picker = picker.clone();
    if let Some(e) = &state.date_edit {
        if e.field_id == focused_id {
            picker.focused = e.focused;
            picker.visible_month = e.visible_month;
        }
    }

    match picker.navigate(key_name) {
        Some(DatePickerEvent::FocusMoved(date)) => {
            picker.focused = date;
            if !picker.is_in_visible_month(date) {
                picker.visible_month = date.first_of_month();
            }
            state.date_edit = Some(DateCursor {
                field_id: focused_id,
                focused: picker.focused,
                visible_month: picker.visible_month,
            });
            Handled::Consumed
        }
        Some(DatePickerEvent::DateSelected(date)) => {
            state.date_edit = None;
            Handled::Event(FormEvent::DateChanged {
                id: focused_id,
                value: date,
            })
        }
        None => Handled::Ignored,
    }
}

/// Whether the currently-focused field is a non-disabled `ComboBox`
/// — gates `handle_key`'s combo-box-key interception. Mirrors
/// [`focused_field_is_enabled_date_picker`].
fn focused_field_is_enabled_combo_box(form: &Form) -> bool {
    let Some(focused_id) = &form.focused_field else {
        return false;
    };
    form.fields
        .iter()
        .find(|f| &f.id == focused_id)
        .is_some_and(|f| !f.disabled && matches!(f.kind, FieldKind::ComboBox(_)))
}

/// Route `key`/`modifiers` into the focused `ComboBox`'s own
/// [`crate::primitives::combo_box::ComboBox::handle_key`] and
/// translate the result into a `FormEvent::ComboBox*` variant.
/// Unlike [`handle_date_picker_key`], this returns `Option`
/// rather than an unconditional [`Handled`]: there is no
/// `DateCursor`-style shadow cache to maintain here (the field's
/// own `text`/`open`/`highlighted` are already the live state, fed
/// back fresh by the app each frame), so a plain `None` is enough to
/// tell `handle_key` to fall through to its generic dispatch for any
/// key the combo box doesn't recognise.
fn handle_combo_box_key(
    form: &Form,
    key: &Key,
    modifiers: &Modifiers,
) -> Option<Handled<FormEvent>> {
    let focused_id = form.focused_field?;
    let field = form.fields.iter().find(|f| f.id == focused_id)?;
    if field.disabled {
        return None;
    }
    let FieldKind::ComboBox(combo) = &field.kind else {
        return None;
    };

    // `ComboBox::focused` is documented as "set by the host's
    // focus-traversal logic" — `Form::focused_field` already
    // singled this field out above, so force it here rather than
    // depending on the app to have mirrored that onto the embedded
    // primitive itself.
    let mut combo = combo.clone();
    combo.focused = true;

    // `FormEvent::ComboBox*` carries the *field's* id (`focused_id`),
    // not the embedded `ComboBox`'s own id from `ComboBoxEvent` —
    // mirroring `FormEvent::DateChanged`'s `id: focused_id` shape so
    // a consumer can always resolve the event back to a `FormField`
    // with `form.fields.iter().find(|f| f.id == id)`.
    match combo.handle_key(key, modifiers)? {
        ComboBoxEvent::TextChanged { text, .. } => Some(Handled::Event(
            FormEvent::ComboBoxTextChanged {
                id: focused_id,
                value: text,
            },
        )),
        ComboBoxEvent::HighlightChanged { index, .. } => Some(Handled::Event(
            FormEvent::ComboBoxHighlightChanged {
                id: focused_id,
                index,
            },
        )),
        ComboBoxEvent::Committed { value, .. } => Some(Handled::Event(
            FormEvent::ComboBoxCommitted {
                id: focused_id,
                value,
            },
        )),
        ComboBoxEvent::Cancelled { .. } => Some(Handled::Event(
            FormEvent::ComboBoxCancelled { id: focused_id },
        )),
    }
}

/// Whether the currently-focused field is a non-disabled
/// `NumberInput` — gates `handle_key`'s number-input-key
/// interception. Mirrors [`focused_field_is_enabled_combo_box`].
fn focused_field_is_enabled_number_input(form: &Form) -> bool {
    let Some(focused_id) = &form.focused_field else {
        return false;
    };
    form.fields
        .iter()
        .find(|f| &f.id == focused_id)
        .is_some_and(|f| !f.disabled && matches!(f.kind, FieldKind::NumberInput(_)))
}

/// Route `key`/`modifiers` into the focused `NumberInput`'s own
/// [`crate::primitives::number_input::NumberInput::handle_key`] and
/// translate the result into a `FormEvent::Number*` variant. Needs no
/// shadow-state struct — see this module's "Number input fields" doc
/// section.
fn handle_number_input_key(
    form: &Form,
    key: &Key,
    modifiers: &Modifiers,
) -> Option<Handled<FormEvent>> {
    let focused_id = form.focused_field?;
    let field = form.fields.iter().find(|f| f.id == focused_id)?;
    if field.disabled {
        return None;
    }
    let FieldKind::NumberInput(number) = &field.kind else {
        return None;
    };

    // `NumberInput::focused` is documented as "set by the host's
    // focus-traversal logic" — `Form::focused_field` already singled
    // this field out above, so force it here rather than depending
    // on the app to have mirrored that onto the embedded primitive
    // itself (mirrors `handle_combo_box_key`'s identical override).
    let mut number = number.clone();
    number.focused = true;

    match number.handle_key(key, modifiers)? {
        NumberInputEvent::ValueChanged { value, .. } => {
            Some(Handled::Event(FormEvent::NumberChanged {
                id: focused_id,
                value,
            }))
        }
        NumberInputEvent::DraftChanged { draft, .. } => Some(Handled::Event(
            FormEvent::NumberDraftChanged {
                id: focused_id,
                draft,
            },
        )),
        NumberInputEvent::Committed { value, .. } => Some(Handled::Event(
            FormEvent::NumberCommitted {
                id: focused_id,
                value,
            },
        )),
        NumberInputEvent::Cancelled { .. } => Some(Handled::Event(
            FormEvent::NumberCancelled { id: focused_id },
        )),
    }
}

/// Whether the currently-focused field is a non-disabled
/// `RadioGroup` — gates `handle_key`'s radio-group-key interception.
/// Mirrors [`focused_field_is_enabled_number_input`].
fn focused_field_is_enabled_radio_group(form: &Form) -> bool {
    let Some(focused_id) = &form.focused_field else {
        return false;
    };
    form.fields
        .iter()
        .find(|f| &f.id == focused_id)
        .is_some_and(|f| !f.disabled && matches!(f.kind, FieldKind::RadioGroup(_)))
}

/// Route `key`/`modifiers` into the focused `RadioGroup`'s own
/// [`crate::primitives::radio_group::RadioGroup::handle_key`] and
/// translate the result into [`FormEvent::RadioChanged`]. Needs no
/// shadow-state struct — see this module's "Radio group fields" doc
/// section.
fn handle_radio_group_key(
    form: &Form,
    key: &Key,
    modifiers: &Modifiers,
) -> Option<Handled<FormEvent>> {
    let focused_id = form.focused_field?;
    let field = form.fields.iter().find(|f| f.id == focused_id)?;
    if field.disabled {
        return None;
    }
    let FieldKind::RadioGroup(group) = &field.kind else {
        return None;
    };

    let mut group = group.clone();
    group.focused = true;

    let RadioGroupEvent::SelectionChanged { index } = group.handle_key(key, modifiers)?;
    Some(Handled::Event(FormEvent::RadioChanged {
        id: focused_id,
        selected: index,
    }))
}

/// Whether the currently-focused field is a non-disabled `Slider` —
/// gates `handle_key`'s slider-key interception. Mirrors
/// [`focused_field_is_enabled_number_input`].
fn focused_field_is_enabled_slider(form: &Form) -> bool {
    let Some(focused_id) = &form.focused_field else {
        return false;
    };
    form.fields
        .iter()
        .find(|f| &f.id == focused_id)
        .is_some_and(|f| !f.disabled && matches!(f.kind, FieldKind::Slider { .. }))
}

/// Left/Down steps the focused `Slider` field's value down by
/// `step`, Right/Up steps it up, Home/End jump straight to
/// `min`/`max` — the same keyboard contract `slider_key_event`
/// (`quadraui-binding-core`'s `semantic` module) gives the
/// standalone leaf, and `FieldKind::Slider`'s own doc promises
/// ("arrow keys step by `step`; Home/End jump to bounds").
/// `FieldKind::Slider`'s fields are already the live state (no
/// shadow-state struct needed, the same as `NumberInput`/`RadioGroup`
/// above), so there's no clone-and-force-focused step here. `None`
/// for every other key or a step/jump that doesn't move the clamped
/// value, which falls through to `handle_key`'s generic dispatch
/// exactly like an unclaimed number-input/radio-group key does.
fn handle_slider_key(form: &Form, key: &Key) -> Option<Handled<FormEvent>> {
    let focused_id = form.focused_field?;
    let field = form.fields.iter().find(|f| f.id == focused_id)?;
    if field.disabled {
        return None;
    }
    let FieldKind::Slider {
        value,
        min,
        max,
        step,
    } = &field.kind
    else {
        return None;
    };
    let (value, min, max, step) = (*value, *min, *max, *step);
    let new_value = match key {
        Key::Named(NamedKey::Left) | Key::Named(NamedKey::Down) => {
            slider_clamp(value - step, min, max)
        }
        Key::Named(NamedKey::Right) | Key::Named(NamedKey::Up) => {
            slider_clamp(value + step, min, max)
        }
        Key::Named(NamedKey::Home) => min,
        Key::Named(NamedKey::End) => max,
        _ => return None,
    };
    if new_value == value {
        return None;
    }
    Some(Handled::Event(FormEvent::SliderChanged {
        id: focused_id,
        value: new_value,
    }))
}

/// Whether the currently-focused field is a non-disabled, non-empty
/// `Dropdown` — gates `handle_key`'s Up/Down interception. Mirrors
/// [`focused_field_is_enabled_combo_box`]; an empty `options`
/// list excludes itself so Up/Down fall through to the generic
/// dispatch rather than being swallowed with nothing to move between.
fn focused_field_is_enabled_dropdown(form: &Form) -> bool {
    let Some(focused_id) = &form.focused_field else {
        return false;
    };
    form.fields
        .iter()
        .find(|f| &f.id == focused_id)
        .is_some_and(|f| {
            !f.disabled
                && matches!(&f.kind, FieldKind::Dropdown { options, .. } if !options.is_empty())
        })
}

/// Up/Down moves the focused `Dropdown`'s `selected_idx` by one,
/// clamped (not wrapping) to `[0, options.len() - 1]`, and emits
/// [`FormEvent::DropdownChanged`] immediately — there's no separate
/// open/highlight/commit sequence the way `ComboBox` has, so unlike
/// [`handle_combo_box_key`] there's no `Handled`
/// variant for an in-progress-but-uncommitted move. Any other key
/// (including Up/Down already at an end of the list) returns `None`,
/// which `handle_key` falls through to its generic dispatch — a
/// no-op there, since a `Dropdown` has no text to edit.
fn handle_dropdown_key(form: &Form, key: &Key) -> Option<Handled<FormEvent>> {
    let focused_id = form.focused_field?;
    let field = form.fields.iter().find(|f| f.id == focused_id)?;
    if field.disabled {
        return None;
    }
    let FieldKind::Dropdown {
        options,
        selected_idx,
    } = &field.kind
    else {
        return None;
    };
    let delta: i32 = match key {
        Key::Named(NamedKey::Up) => -1,
        Key::Named(NamedKey::Down) => 1,
        _ => return None,
    };
    let max_idx = options.len().saturating_sub(1) as i32;
    let new_idx = (*selected_idx as i32 + delta).clamp(0, max_idx) as usize;
    if new_idx == *selected_idx {
        return None;
    }
    Some(Handled::Event(
        FormEvent::DropdownChanged {
            id: focused_id,
            selected_idx: new_idx,
        },
    ))
}

/// Whether the currently-focused field is a non-disabled
/// `ButtonRow`/`ToggleGroup`/`Toolbar` with at least one *enabled*
/// item — gates `handle_key`'s Left/Right/Enter/Space interception.
/// Mirrors [`focused_field_is_enabled_dropdown`]; an
/// all-disabled item list excludes itself the same way an empty
/// `options` list does there, so those keys fall through to the
/// generic dispatch rather than being swallowed with nothing
/// reachable to move to or activate.
fn focused_field_is_enabled_item_row(form: &Form) -> bool {
    let Some(focused_id) = &form.focused_field else {
        return false;
    };
    form.fields
        .iter()
        .find(|f| &f.id == focused_id)
        .is_some_and(|f| {
            !f.disabled
                && focusable_items(&f.kind).is_some_and(|items| items.iter().any(|(_, en)| *en))
        })
}

/// The focused field's current item-focus: its id, the index into
/// [`crate::primitives::form::focusable_items`]'s enumeration, and that enumeration itself.
/// Reads `state.item_focus` when it still names this field at a
/// still-valid index, otherwise falls back to the first enabled item
/// — see [`ItemCursor`]'s own doc for
/// why a stale/missing cache falls back instead of focusing nothing.
/// `None` when there's no focused field, it's disabled, it isn't one
/// of the three item-bearing kinds, or every one of its items is
/// disabled.
pub(crate) fn current_item_focus(form: &Form, state: &FormState) -> Option<ItemFocusLookup> {
    let focused_id = form.focused_field?;
    let field = form.fields.iter().find(|f| f.id == focused_id)?;
    if field.disabled {
        return None;
    }
    let items = focusable_items(&field.kind)?;
    let idx = match &state.item_focus {
        Some(s)
            if s.field_id == focused_id && items.get(s.item_idx).is_some_and(|(_, en)| *en) =>
        {
            s.item_idx
        }
        _ => first_enabled_item_idx(&items)?,
    };
    Some((focused_id, idx, items))
}

/// Left/Right move the focused field's item-focus by one, skipping
/// any disabled item, without wrapping; Enter/Space activate the
/// item `state.item_focus` currently names. Any other key (including
/// Left at the first enabled item, or Right at the last) returns
/// `None`, which `handle_key` falls through to its generic dispatch
/// — a no-op there, since none of the three item-bearing kinds have
/// editable text.
fn handle_item_row_key(
    form: &Form,
    state: &mut FormState,
    key: &Key,
) -> Option<Handled<FormEvent>> {
    let (focused_id, cur_idx, items) = current_item_focus(form, state)?;
    let field = form.fields.iter().find(|f| f.id == focused_id)?;

    match key {
        Key::Named(NamedKey::Left) => {
            let new_idx = prev_enabled_item_idx(&items, cur_idx)?;
            state.item_focus = Some(ItemCursor {
                field_id: focused_id,
                item_idx: new_idx,
            });
            Some(Handled::Consumed)
        }
        Key::Named(NamedKey::Right) => {
            let new_idx = next_enabled_item_idx(&items, cur_idx)?;
            state.item_focus = Some(ItemCursor {
                field_id: focused_id,
                item_idx: new_idx,
            });
            Some(Handled::Consumed)
        }
        Key::Named(NamedKey::Enter) | Key::Char(' ') => {
            state.item_focus = Some(ItemCursor {
                field_id: focused_id,
                item_idx: cur_idx,
            });
            let (item_id, _) = items[cur_idx];
            Some(Handled::Event(activate_item_event(
                field, item_id,
            )))
        }
        _ => None,
    }
}

/// Whether the currently-focused field is a non-disabled plain
/// `Toggle` — gates `handle_key`'s Enter/Space interception. Mirrors
/// [`focused_field_is_enabled_item_row`]; a `ToggleGroup`'s
/// individual toggles are covered by that item-row path instead (see
/// [`crate::primitives::form::focusable_items`]), so this only ever matches the single-value
/// `FieldKind::Toggle`.
fn focused_field_is_enabled_toggle(form: &Form) -> bool {
    let Some(focused_id) = &form.focused_field else {
        return false;
    };
    form.fields
        .iter()
        .find(|f| &f.id == focused_id)
        .is_some_and(|f| !f.disabled && matches!(f.kind, FieldKind::Toggle { .. }))
}

/// Enter/Space flip the focused `Toggle`'s value — the keyboard
/// equivalent of [`form_click_event`]'s own `FieldKind::Toggle` arm.
/// Any other key returns `None`, which `handle_key` falls through to
/// its generic dispatch, same as [`handle_item_row_key`].
fn handle_toggle_key(form: &Form, key: &Key) -> Option<Handled<FormEvent>> {
    if !matches!(key, Key::Named(NamedKey::Enter) | Key::Char(' ')) {
        return None;
    }
    let focused_id = form.focused_field?;
    let field = form.fields.iter().find(|f| f.id == focused_id)?;
    let FieldKind::Toggle { value } = &field.kind else {
        return None;
    };
    Some(Handled::Event(FormEvent::ToggleChanged {
        id: focused_id,
        value: !value,
    }))
}

/// Enter/Space equivalent of this module's Enter/Space activation for
/// an item-row field or a plain `Toggle`, invoked from
/// [`Interactive::handle`]'s `CharTyped(' ')` arm before it falls back
/// to inserting a literal space — see that arm's own doc for why a
/// space delivered as `CharTyped` needs this at all (it bypasses
/// `handle_key` entirely, unlike a `KeyPressed` space). `None` when
/// the focused field is neither kind, leaving the caller to insert
/// the space as text.
pub(crate) fn activate_focused_on_space(
    form: &Form,
    state: &mut FormState,
) -> Option<Handled<FormEvent>> {
    if focused_field_is_enabled_item_row(form) {
        if let Some(result) = handle_item_row_key(form, state, &Key::Char(' ')) {
            return Some(result);
        }
    }
    if focused_field_is_enabled_toggle(form) {
        if let Some(result) = handle_toggle_key(form, &Key::Char(' ')) {
            return Some(result);
        }
    }
    None
}

/// Move focus to the next/previous non-`Label`, non-`ReadOnly`,
/// non-disabled field, wrapping around, and scroll it into view.
/// Emits [`FormEvent::FocusChanged`] — the app persists the new
/// `focused_field` the same way it already persists a toggle flip
/// (see [`handle_text_op`]'s own doc).
pub(crate) fn move_focus(
    form: &Form,
    state: &mut FormState,
    delta: isize,
    viewport_rows: usize,
) -> Handled<FormEvent> {
    let stops = super::focus_order(form);
    if stops.is_empty() {
        return Handled::Ignored;
    }
    let n = stops.len();
    let cur_idx = form
        .focused_field
        .and_then(|id| stops.iter().position(|fid| *fid == id));
    let next_idx = match cur_idx {
        Some(i) => crate::focus::cycle_index(i, delta, n),
        None if delta >= 0 => 0,
        None => n - 1,
    };
    let id = stops[next_idx];
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
            TextCursor {
                field_id: id,
                cursor,
                selection_anchor,
            }
        });
    // Same seeding as `new_text_edit` above, but for a `DatePicker`:
    // the newly-focused field's own `focused`/`visible_month` become
    // the starting scrubbing position, so `build_form` overlays them
    // immediately rather than waiting for the first keystroke.
    let new_date_edit =
        field_idx
            .and_then(|i| form.fields.get(i))
            .and_then(|f| match &f.kind {
                FieldKind::DatePicker(picker) => Some(DateCursor {
                    field_id: id,
                    focused: picker.focused,
                    visible_month: picker.visible_month,
                }),
                _ => None,
            });
    // Reseed item-focus to the newly-focused field's first enabled
    // item (see this module's "Item-level keyboard focus" doc
    // section) — `None` both clears a stale index left over from
    // the field being left, and leaves it cleared for a field that
    // isn't one of the three item-bearing kinds.
    let new_item_focus = field_idx
        .and_then(|i| form.fields.get(i))
        .and_then(|f| focusable_items(&f.kind))
        .and_then(|items| first_enabled_item_idx(&items))
        .map(|item_idx| ItemCursor {
            field_id: id,
            item_idx,
        });

    if let Some(field_idx) = field_idx {
        state.scroll_to_field(form, field_idx, viewport_rows);
    }
    state.text_edit = new_text_edit;
    state.date_edit = new_date_edit;
    state.item_focus = new_item_focus;
    Handled::Event(FormEvent::FocusChanged { id })
}

/// `Enter` on a focused `TextInput`/`PasswordInput` field commits
/// instead of inserting a newline — see this module's "Keyboard
/// editing" doc section. (`TextArea` is handled separately in
/// `handle_key`.)
fn commit_focused_text(form: &Form, state: &mut FormState) -> Handled<FormEvent> {
    let Some(focused_id) = form.focused_field else {
        return Handled::Ignored;
    };
    let Some(field) = form.fields.iter().find(|f| f.id == focused_id) else {
        return Handled::Ignored;
    };
    if field.disabled {
        return Handled::Ignored;
    }
    let Some(value) = text_field_value(&field.kind) else {
        return Handled::Ignored;
    };
    let value = value.to_string();
    state.text_edit = None;
    Handled::Event(FormEvent::TextInputCommitted {
        id: focused_id,
        value,
    })
}

/// Apply `op` to the currently-focused text field (if any), emitting
/// [`FormEvent::TextInputChanged`] with the new value. The cursor
/// produced by `op` is cached on `state.text_edit` so the next
/// keystroke continues from it — see `TextCursor`'s doc.
pub(crate) fn handle_text_op(form: &Form, state: &mut FormState, op: EditOp) -> Handled<FormEvent> {
    let Some(focused_id) = form.focused_field else {
        return Handled::Ignored;
    };
    let Some(field) = form.fields.iter().find(|f| f.id == focused_id) else {
        return Handled::Ignored;
    };
    if field.disabled {
        return Handled::Ignored;
    }
    let Some(value) = text_field_value(&field.kind) else {
        return Handled::Ignored;
    };

    let (cursor, anchor) = match &state.text_edit {
        Some(e) if e.field_id == focused_id => (e.cursor, e.selection_anchor),
        _ => default_text_cursor(&field.kind),
    };
    let cursor = snap_to_char_boundary(value, cursor.min(value.len()));

    let value_before = value.to_string();
    let (new_value, new_cursor, new_anchor) =
        crate::primitives::text_input::apply_flat_edit_op(value, cursor, anchor, op);
    let text_changed = new_value != value_before;
    // `TextEditor::move_cursor_to` leaves a degenerate `anchor ==
    // cursor` selection in place for a no-op extend (e.g. Shift+Left
    // at offset 0). Left uncleared, the *next* keystroke would see a
    // phantom one-character-wide selection and silently eat a
    // character. Normalise it away here, where the controller caches
    // the anchor across keystrokes (unlike a one-shot `TextEditor`
    // call, which never notices).
    let new_anchor = new_anchor.filter(|a| *a != new_cursor);
    state.text_edit = Some(TextCursor {
        field_id: focused_id,
        cursor: new_cursor,
        selection_anchor: new_anchor,
    });
    if text_changed {
        Handled::Event(FormEvent::TextInputChanged {
            id: focused_id,
            value: new_value,
        })
    } else {
        // Cursor-only movement (or selection extend): the field's
        // text content is unchanged, so there is nothing for the app
        // to persist — but the cursor/selection did move, so the
        // caller should still redraw to show it.
        Handled::Consumed
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
                    id: *clicked_id,
                    value: !value,
                },
                FieldKind::Button => FormEvent::ButtonClicked { id: *clicked_id },
                _ => FormEvent::FocusChanged { id: *clicked_id },
            };
        }
        if let FieldKind::ToggleGroup { toggles } = &field.kind {
            if let Some(t) = toggles.iter().find(|t| &t.id == clicked_id) {
                return FormEvent::ToggleChanged {
                    id: *clicked_id,
                    value: !t.value,
                };
            }
        }
        if let FieldKind::ButtonRow { buttons } = &field.kind {
            if buttons.iter().any(|b| &b.id == clicked_id) {
                return FormEvent::ButtonClicked { id: *clicked_id };
            }
        }
        if let FieldKind::SegmentedControl { .. } = &field.kind {
            let prefix = format!("{}__seg_", field.id.as_str());
            if clicked_id.as_str().starts_with(&prefix) {
                if let Ok(idx) = clicked_id.as_str()[prefix.len()..].parse::<usize>() {
                    return FormEvent::SegmentedControlChanged {
                        id: field.id,
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
                    field_id: field.id,
                    button_id: *clicked_id,
                };
            }
        }
        // Each `RadioOption` carries its own `WidgetId` (one
        // `item_bounds` entry per option — see this module's "Radio
        // group fields" doc section), so a click resolves directly to
        // the option's index, the same generic per-item path
        // `ToggleGroup` above uses. `RadioGroup::select` already gates
        // on the option's own `disabled` flag, so a disabled option's
        // click falls through to focusing the field instead of
        // selecting it.
        if let FieldKind::RadioGroup(group) = &field.kind {
            if let Some(idx) = group.options.iter().position(|o| &o.id == clicked_id) {
                if let Some(RadioGroupEvent::SelectionChanged { index }) = group.select(idx) {
                    return FormEvent::RadioChanged {
                        id: field.id,
                        selected: index,
                    };
                }
                return FormEvent::FocusChanged { id: field.id };
            }
        }
    }
    FormEvent::FocusChanged { id: *clicked_id }
}

/// `FieldKind::NumberInput`'s stepper buttons are sub-regions of its
/// single `item_bounds` entry rather than separate `WidgetId`s, so
/// resolving a click needs the clicked field's exact on-screen rect —
/// unlike [`form_click_event`]'s generic per-id dispatch, which only
/// needs `clicked_id`. Returns `None` for every other `FieldKind`, or a
/// `NumberInput` click that landed outside the stepper (text area, or no
/// `item_bounds` entry at all) — [`click_field`] falls
/// through to [`form_click_event`] in both cases, which still resolves a
/// plain click there to `FormEvent::FocusChanged`.
fn number_input_click_event(
    form: &Form,
    layout: &FormLayout,
    clicked_id: &WidgetId,
    x: f32,
    y: f32,
) -> Option<FormEvent> {
    let field = form.fields.iter().find(|f| &f.id == clicked_id)?;
    if field.disabled {
        return None;
    }
    let FieldKind::NumberInput(number) = &field.kind else {
        return None;
    };
    let vf = layout
        .visible_fields
        .iter()
        .find(|vf| &vf.id == clicked_id)?;
    let (_, control_rect) = vf.item_bounds.first()?;
    let hit = number.layout(*control_rect).hit_test(Point::new(x, y));
    let NumberInputEvent::ValueChanged { value, .. } = number.click(hit)? else {
        return None;
    };
    Some(FormEvent::NumberChanged {
        id: *clicked_id,
        value,
    })
}

/// Clamps `raw` to `[min, max]` without [`f32::clamp`]'s panic on an
/// inverted `min > max` — `FieldKind::Slider`'s `min`/`max` come from a
/// wire payload, so a host typo inverting them degrades to a
/// well-defined (if useless) clamp instead of a panic that would take
/// the whole render down. Mirrors `quadraui-binding-core`'s
/// `Slider::clamp`, which the standalone leaf already uses this same way.
fn slider_clamp(raw: f32, min: f32, max: f32) -> f32 {
    raw.max(min).min(max)
}

/// Cells of track [`crate::primitives::layout_metrics::form_field_measure`]'s backend-free fallback gives a
/// [`FieldKind::Slider`] field — `tui::form`'s own natural track width,
/// which is what `ControlWidth::Content` resolves to there. This
/// fallback has no row width to resolve `ControlWidth::Fill` against, so
/// it reports the natural track for every mode; a host with a live
/// [`crate::Backend`] gets that backend's own resolved track instead.
pub(crate) const SLIDER_TRACK_CELLS: f32 = 12.0;

/// The on-screen rect of a non-disabled `FieldKind::Slider` field's
/// painted track, in the same form-local space as
/// `number_input_click_event`'s `x`/`y`.
///
/// It is the field's own
/// [`VisibleFormField::item_bounds`] entry, the
/// same lookup [`number_input_click_event`] uses for its stepper. Every
/// layout path populates it for a `FieldKind::Slider` field — the pixel
/// backends' `layout_metrics::form_field_measure`, `tui::form`'s
/// cell-unit measure, and [`crate::primitives::layout_metrics::form_field_measure`]'s backend-free fallback
/// — and each one reports the exact rect its own painter fills the track
/// into, so the value under the cursor is the value a click produces
/// regardless of label position or `ControlWidth`.
///
/// A layout from a measure closure that reports no items at all falls
/// back to the field's whole row (`vf.bounds`), but only when that row
/// and the painted track actually coincide — an empty label, or a
/// resolved [`super::LabelPosition::Hidden`] — since only then does the row's
/// left edge match the track's. For every other resolved label position
/// the row also spans the label column, so this reports no track rather
/// than one whose left edge is really the label's.
///
/// `None` for every other `FieldKind`, a disabled slider, a field with
/// no visible rect at all, and a zero-width track.
pub(crate) fn slider_track_rect(form: &Form, layout: &FormLayout, id: &WidgetId) -> Option<Rect> {
    let field = form.fields.iter().find(|f| &f.id == id)?;
    if field.disabled || !matches!(field.kind, FieldKind::Slider { .. }) {
        return None;
    }
    let vf = layout.visible_fields.iter().find(|vf| &vf.id == id)?;
    let track = match vf.item_bounds.first() {
        Some((_, rect)) => *rect,
        None => {
            let has_label_column = field.label.visible_width() > 0
                && form.resolved_label_position(field) != LabelPosition::Hidden;
            if has_label_column {
                return None;
            }
            vf.bounds
        }
    };
    (track.width > 0.0).then_some(track)
}

/// Whether a press at form-local `x` landed on the field's painted
/// track — what [`click_field`] arms `slider_drag` on,
/// so a press on a labeled slider's label column focuses the row without
/// capturing the pointer for a drag that would then move the value.
fn slider_press_is_on_track(form: &Form, layout: &FormLayout, id: &WidgetId, x: f32) -> bool {
    slider_track_rect(form, layout, id).is_some_and(|t| x >= t.x && x < t.x + t.width)
}

/// Maps a pointer `x` (local to the form) across a `FieldKind::Slider`
/// field's painted track to a value — mirrors `quadraui-binding-core`'s
/// own `slider_click_event`, which gives the standalone leaf the same
/// "the pointer lands at that fraction of the track" contract
/// `FieldKind::Slider`'s doc promises, rather than a fixed one-`step`
/// nudge regardless of position.
///
/// `overrun` decides what an `x` outside the track's own span means, so a
/// press and the drag it starts can share one mapping without sharing
/// that judgement: [`click_field`] passes
/// [`TrackOverrun::Reject`] (a fresh press has to land on the track to
/// move the value — on the label column or the value text it falls
/// through to [`form_click_event`]'s `FocusChanged`), while
/// [`slider_drag_to`] passes [`TrackOverrun::Clamp`]
/// (the pointer is already captured, so dragging past an end pins to
/// that end instead of dropping the drag).
///
/// `None` whenever [`slider_track_rect`] reports no track, for a
/// rejected overrun, and for an `x` that doesn't actually move the
/// clamped value.
fn slider_value_at(
    form: &Form,
    layout: &FormLayout,
    clicked_id: &WidgetId,
    x: f32,
    overrun: TrackOverrun,
) -> Option<FormEvent> {
    let field = form.fields.iter().find(|f| &f.id == clicked_id)?;
    let FieldKind::Slider {
        value, min, max, ..
    } = &field.kind
    else {
        return None;
    };
    let track = slider_track_rect(form, layout, clicked_id)?;
    let raw = (x - track.x) / track.width;
    if overrun == TrackOverrun::Reject && !(0.0..1.0).contains(&raw) {
        return None;
    }
    let fraction = raw.clamp(0.0, 1.0);
    let new_value = slider_clamp(min + fraction * (max - min), *min, *max);
    if new_value == *value {
        return None;
    }
    Some(FormEvent::SliderChanged {
        id: *clicked_id,
        value: new_value,
    })
}

/// What a pointer x outside a slider's painted track means to
/// [`slider_value_at`].
#[derive(Clone, Copy, PartialEq, Eq)]
enum TrackOverrun {
    /// A fresh press: an x off the track is not a track interaction at
    /// all, so no value comes out of it.
    Reject,
    /// A drag the track already captured: an x off either end pins the
    /// value to that end.
    Clamp,
}

// ── Item-level keyboard focus helpers ─────────────────────────────────
//
// See [`ItemCursor`]'s own doc.

/// The first enabled item's index in `items` (as enumerated by
/// [`crate::primitives::form::focusable_items`]), or `None` if every item is disabled.
fn first_enabled_item_idx(items: &[(WidgetId, bool)]) -> Option<usize> {
    items.iter().position(|(_, enabled)| *enabled)
}

/// The nearest enabled item's index *before* `cur`, or `None` if `cur`
/// is already the first enabled item — the "no wrap" half of Left's
/// contract (see [`ItemCursor`]'s own doc).
fn prev_enabled_item_idx(items: &[(WidgetId, bool)], cur: usize) -> Option<usize> {
    items[..cur]
        .iter()
        .enumerate()
        .rev()
        .find(|(_, (_, enabled))| *enabled)
        .map(|(i, _)| i)
}

/// The nearest enabled item's index *after* `cur`, or `None` if `cur` is
/// already the last enabled item — the "no wrap" half of Right's
/// contract (see [`ItemCursor`]'s own doc).
fn next_enabled_item_idx(items: &[(WidgetId, bool)], cur: usize) -> Option<usize> {
    items
        .iter()
        .enumerate()
        .skip(cur + 1)
        .find(|(_, (_, enabled))| *enabled)
        .map(|(i, _)| i)
}

/// The `FormEvent` a mouse click on this item already emits via
/// [`form_click_event`] — reused by [`handle_item_row_key`]
/// so Enter/Space activates the keyboard-focused item identically to a
/// click on it. `field.kind` must be one of the three kinds
/// [`crate::primitives::form::focusable_items`] recognizes (every caller already established that
/// via [`current_item_focus`]/
/// [`focused_field_is_enabled_item_row`]).
fn activate_item_event(field: &crate::primitives::form::FormField, item_id: WidgetId) -> FormEvent {
    match &field.kind {
        FieldKind::ButtonRow { .. } => FormEvent::ButtonClicked { id: item_id },
        FieldKind::ToggleGroup { toggles } => {
            let value = toggles
                .iter()
                .find(|t| t.id == item_id)
                .map(|t| !t.value)
                .unwrap_or(true);
            FormEvent::ToggleChanged { id: item_id, value }
        }
        FieldKind::Toolbar(_) => FormEvent::ToolbarButtonClicked {
            field_id: field.id,
            button_id: item_id,
        },
        _ => unreachable!(
            "activate_item_event is only called for a field focusable_items recognized"
        ),
    }
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
/// [`TextCursor`] yet — the field's own `cursor` (defaulting to
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

/// Maps a key to the string name [`crate::primitives::date_picker::DatePicker::navigate`]
/// recognises, or `None` for a key it doesn't act on (e.g. `Tab`, so focus
/// traversal off a `DatePicker` field keeps working in `handle_key`).
///
/// The name itself comes from the shared [`NamedKey::name`] table — this
/// function only narrows *which* keys `DatePicker` claims, not how they're
/// spelled.
fn date_picker_key_name(key: &Key) -> Option<std::borrow::Cow<'static, str>> {
    match key {
        Key::Named(
            named @ (NamedKey::Left
            | NamedKey::Right
            | NamedKey::Up
            | NamedKey::Down
            | NamedKey::PageUp
            | NamedKey::PageDown
            | NamedKey::Enter),
        ) => Some(named.name()),
        _ => None,
    }
}

// ── Pointer routing ─────────────────────────────────────────────────

/// Resolve a left-button press at form-local `(x, y)` against `layout`.
///
/// Arms [`FormState::slider_drag`] whenever the press landed on a
/// non-disabled `FieldKind::Slider` field's painted track, regardless
/// of whether the press itself moves the value — a press that lands
/// exactly on the current value still starts a drag the next pointer
/// move should keep tracking. A press elsewhere in the row (the label
/// column, the value text) only focuses the field, so it must not
/// capture the pointer.
pub(crate) fn click_field(
    form: &Form,
    state: &mut FormState,
    layout: &FormLayout,
    x: f32,
    y: f32,
) -> Handled<FormEvent> {
    // Every fresh press starts at most one gesture, so clear whatever
    // drag a previous press left behind up front — not just when this
    // press lands on a field — so a dropped release (the pointer
    // leaving the window with the button still held, common on GTK)
    // can't leave a stale slider drag that later hijacks an unrelated
    // gesture.
    state.slider_drag = None;
    match layout.hit_test(x, y) {
        FormHit::Field(id) => {
            state.slider_drag = slider_press_is_on_track(form, layout, &id, x).then_some(id);
            // `NumberInput`'s stepper buttons are sub-regions of its
            // single `item_bounds` entry, not separate `WidgetId`s the
            // way a `RadioGroup` option or a `ToggleGroup` item is — so
            // unlike `form_click_event`'s generic per-id dispatch, this
            // needs the clicked field's exact on-screen rect. `None`
            // (not a `NumberInput`, or the click landed outside the
            // stepper) falls through to the next candidate, and
            // ultimately to the generic path, which still resolves a
            // plain field click to `FocusChanged`.
            let event = number_input_click_event(form, layout, &id, x, y)
                .or_else(|| slider_value_at(form, layout, &id, x, TrackOverrun::Reject))
                .unwrap_or_else(|| form_click_event(form, &id));
            Handled::Event(event)
        }
        FormHit::Empty => Handled::Consumed,
    }
}

/// Continue an in-flight slider drag (armed by [`click_field`]) as the
/// pointer moves to form-local `x`.
///
/// Unlike a scrollbar-thumb drag, this resolves against the layout
/// handed in fresh rather than geometry cached at press time, so a
/// resize between the press and this move drags against the track where
/// it actually is. [`Handled::Ignored`] when nothing is being dragged,
/// or the new position doesn't move the clamped value.
pub(crate) fn slider_drag_to(
    form: &Form,
    state: &FormState,
    layout: &FormLayout,
    x: f32,
) -> Handled<FormEvent> {
    let Some(dragging_id) = state.slider_drag else {
        return Handled::Ignored;
    };
    match slider_value_at(form, layout, &dragging_id, x, TrackOverrun::Clamp) {
        Some(event) => Handled::Event(event),
        None => Handled::Ignored,
    }
}

/// Write [`FormState`]'s caret and scrub positions back onto a clone of
/// `form` so a painter shows them.
///
/// [`Primitive::paint`]/[`Primitive::paint_cells`] take the descriptor
/// alone, and the three text kinds plus `DatePicker` carry their cursor
/// as a descriptor field — so a host painting a live, keyboard-driven
/// form overlays the live positions through this first. A `None` state
/// field leaves the descriptor's own value in place.
pub(crate) fn overlay_state(form: &mut Form, state: &FormState) {
    if let Some(e) = &state.text_edit {
        if let Some(field) = form.fields.iter_mut().find(|fld| fld.id == e.field_id) {
            overlay_text_cursor(&mut field.kind, e.cursor, e.selection_anchor);
        }
    }
    // Gated on the field still being focused (unlike the caret overlay
    // above): a click that moves focus off a `DatePicker` field goes
    // through `form_click_event`, which doesn't reseed or clear the
    // scrub position, so a stale one could otherwise outlive the row it
    // was scrubbed on.
    if let Some(e) = &state.date_edit {
        if form.focused_field == Some(e.field_id) {
            if let Some(field) = form.fields.iter_mut().find(|fld| fld.id == e.field_id) {
                overlay_date_edit(&mut field.kind, e.focused, e.visible_month);
            }
        }
    }
    if let Some(offset) = state.scroll_offset {
        form.scroll_offset = offset;
    }
}

impl WidgetAction for FormEvent {
    /// Every outcome rides [`crate::UiEvent::Form`] under the form's own
    /// id, carrying the field-level event unchanged — the same channel a
    /// host already matches on after `Backend::draw_form`, so routing a
    /// form through the widget contract needs no new event vocabulary.
    fn into_ui_event(self, id: WidgetId) -> crate::UiEvent {
        crate::UiEvent::Form(id, self)
    }
}

// ── Interactive ─────────────────────────────────────────────────────

/// Rows a `viewport_height`-tall viewport shows, given a uniform
/// `row_height`. Tab's scroll-into-view needs it, and both units
/// measure it the same way — a cell grid's row height is `1.0`.
fn viewport_rows(layout: &FormPrimitiveLayout, row_height: f32) -> usize {
    if row_height <= 0.0 {
        return 0;
    }
    (layout.area.height / row_height).floor().max(0.0) as usize
}

impl Interactive for Form {
    type State = FormState;
    type Action = FormEvent;

    /// One focus stop per field, in the descriptor's own Tab order
    /// ([`super::focus_order`], which skips `Label`, `ReadOnly` and
    /// disabled fields) — so a host's focus traversal can land on a
    /// single field without the form having to be entered first, and
    /// Tab inside the form walks the same stops.
    fn focus(&self) -> FocusSpec {
        let stops: Vec<crate::interactive::FocusStop> = super::focus_order(self)
            .iter()
            .enumerate()
            .map(|(i, _)| crate::interactive::FocusStop {
                sub: SubTarget(i as u64),
            })
            .collect();
        if stops.is_empty() {
            FocusSpec::None
        } else {
            FocusSpec::Children(stops)
        }
    }

    /// Routes every event to the one behaviour implementation above:
    /// a press through [`click_field`], a drag through
    /// [`slider_drag_to`], a wheel notch through
    /// [`FormState::scroll_by`], and a keystroke through
    /// [`handle_key`], which hands it to the focused field's own widget
    /// (`ComboBox::handle_key`, `NumberInput::handle_key`,
    /// `RadioGroup::handle_key`, `DatePicker::navigate`,
    /// `text_input::apply_flat_edit_op`, …) and translates that
    /// widget's own event into the matching [`FormEvent`].
    fn handle(
        &self,
        state: &mut FormState,
        event: &RoutedEvent,
        layout: &FormPrimitiveLayout,
        cx: &mut EventCx,
    ) -> Handled<FormEvent> {
        let row_height = layout
            .layout
            .visible_fields
            .first()
            .map(|f| f.bounds.height)
            .filter(|h| *h > 0.0)
            .unwrap_or(1.0);
        let rows = viewport_rows(layout, row_height);
        match event {
            RoutedEvent::Pointer {
                kind: PointerKind::Down(MouseButton::Left),
                local,
                ..
            } => {
                cx.request_focus(None);
                click_field(self, state, &layout.layout, local.x, local.y)
            }
            RoutedEvent::Pointer {
                kind: PointerKind::Move,
                local,
                ..
            } if state.slider_drag.is_some() => {
                slider_drag_to(self, state, &layout.layout, local.x)
            }
            RoutedEvent::Pointer {
                kind: PointerKind::Up(MouseButton::Left),
                ..
            } => {
                state.slider_drag = None;
                Handled::Consumed
            }
            RoutedEvent::Wheel { dy, .. } => {
                let delta = crate::primitives::scrollbar::wheel_row_delta(*dy, 1.0);
                state.scroll_by(self, delta, rows);
                Handled::Consumed
            }
            RoutedEvent::Key(key) => handle_key(self, state, &key.key, &key.modifiers, rows),
            // A space delivered as text rather than as a key still
            // activates a focused item row or plain `Toggle` — the one
            // text character with activation semantics. Every other
            // character, and a space on a text field, inserts.
            RoutedEvent::Text(text) => {
                let mut result = Handled::Ignored;
                for ch in text.chars() {
                    result = if ch == ' ' {
                        activate_focused_on_space(self, state)
                            .unwrap_or_else(|| handle_text_op(self, state, EditOp::InsertChar(' ')))
                    } else {
                        handle_text_op(self, state, EditOp::InsertChar(ch))
                    };
                }
                result
            }
            // `A11yAction` names no field through this generic seam, so
            // `Click` activates whichever field is focused — the same
            // one Enter/Space activates, matching the
            // assistive-technology default-action convention.
            RoutedEvent::Action(A11yAction::Click) => self
                .focused_field
                .map(|id| Handled::Event(form_click_event(self, &id)))
                .unwrap_or(Handled::Ignored),
            _ => Handled::Ignored,
        }
    }

    fn a11y_actions(&self, _state: &FormState) -> Vec<A11yAction> {
        vec![A11yAction::Click]
    }

    /// One sub-part per field row, addressed by its index into
    /// [`super::focus_order`] so a sub-target and a focus stop name the
    /// same field — then one per item of a multi-item row, after every
    /// row, so the more specific item region wins over the row it sits
    /// in.
    fn sub_regions(&self, layout: &FormPrimitiveLayout) -> Vec<(Rect, SubTarget)> {
        let stops = super::focus_order(self);
        let mut regions: Vec<(Rect, SubTarget)> = Vec::new();
        let mut items: Vec<(Rect, SubTarget)> = Vec::new();
        for (i, id) in stops.iter().enumerate() {
            let Some(vf) = layout.visible_field(id) else {
                continue;
            };
            for (item_idx, (_, rect)) in vf.item_bounds.iter().enumerate() {
                items.push((*rect, SubTarget(sub_item(i, item_idx))));
            }
            regions.push((vf.bounds, SubTarget(i as u64)));
        }
        items.extend(regions);
        items
    }
}

/// [`Interactive::sub_regions`]'s tag for item `item_idx` of focus stop
/// `stop`, offset past the per-row tags (which are the stop indices
/// themselves).
fn sub_item(stop: usize, item_idx: usize) -> u64 {
    SUB_ITEM_BASE + (stop as u64) * SUB_ITEMS_PER_ROW + item_idx as u64
}

/// First [`Interactive::sub_regions`] tag reserved for a row's items,
/// past every per-row tag a form could have.
const SUB_ITEM_BASE: u64 = 1 << 32;
/// Item tags reserved per row, so row `n`'s items can never collide
/// with row `n + 1`'s.
const SUB_ITEMS_PER_ROW: u64 = 1 << 16;

/// Write `cursor`/`selection_anchor` back onto `kind` for painting.
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

/// Write `focused`/`visible_month` back onto `kind` for painting. No-op
/// for every `FieldKind` other than `DatePicker` — mirrors
/// [`overlay_text_cursor`]'s shape.
fn overlay_date_edit(
    kind: &mut FieldKind,
    focused: crate::primitives::date_picker::CalendarDate,
    visible_month: crate::primitives::date_picker::CalendarDate,
) {
    if let FieldKind::DatePicker(picker) = kind {
        picker.focused = focused;
        picker.visible_month = visible_month;
    }
}
