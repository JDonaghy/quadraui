//! `ComboBox` primitive: a free-text field backed by a fixed list of
//! options, with a type-ahead filtered dropdown — layout, keyboard and
//! pointer handling, validation/commit/cancel events, and both
//! rasterisers.
//!
//! The whole widget contract lives here: [`Primitive`] resolves the
//! field's layout and paints it ([`native_surface_paint`] on a pixel
//! surface, [`cell_paint`] on a cell grid), and [`Interactive`] answers
//! pointer, keyboard and assistive-technology input. A host reaches all
//! of it type-erased through [`crate::frame::Surface::Widget`].
//!
//! Additive alongside [`crate::TextInput`], which is untouched — a
//! `ComboBox` isn't a `TextInput` with a dropdown bolted on, it's its
//! own primitive with its own commit model.
//!
//! # The dropdown is a popup
//!
//! The box itself is only ever its field row. While [`ComboBox::open`]
//! is set, [`Interactive::popups`] answers one [`Popup`] anchored under
//! the field, whose content lists the options
//! [`ComboBox::filtered_indices`] matches (none, when nothing does). The runtime paints
//! it above every other widget in the frame, hit-tests it before them,
//! and routes a press on one of its rows back to the box as a
//! [`RoutedEvent::PopupEvent`] naming that row's filtered index. A press
//! outside the box and its list, or Escape, arrives as
//! [`RoutedEvent::PopupDismissed`] and cancels the edit.
//!
//! The list is at most [`MAX_DROPDOWN_ROWS`] rows tall (and never
//! taller than the viewport); a longer filter scrolls so the highlighted
//! row stays visible.
//!
//! # Editing model
//!
//! [`ComboBox::text`] is the live edit buffer (what the field displays
//! while typing); [`ComboBox::committed`] is the last confirmed value.
//! [`ComboBox::handle_key`]'s `Enter` commits — either the currently
//! highlighted dropdown option (when [`ComboBox::open`] and
//! [`ComboBox::highlighted`] is `Some`) or the typed `text` verbatim —
//! and the host applies [`ComboBoxEvent::Committed`] by setting both
//! `committed` and `text` to the event's `value` and closing the
//! dropdown. `Escape` cancels: the host applies
//! [`ComboBoxEvent::Cancelled`] by resetting `text` back to `committed`
//! and closing the dropdown. Typed characters
//! ([`ComboBoxEvent::TextChanged`]) are expected to also open the
//! dropdown — the host sets `open = true` alongside updating `text`.
//! `Up`/`Down` move [`ComboBox::highlighted`] through the *filtered*
//! list with wraparound, mirroring [`crate::RadioGroup`]'s cursor
//! movement, via [`crate::behaviour::nav::FlatNav`].
//! [`ComboBox::typeahead_highlight`] offers a second, debounced way to
//! reach the same `highlighted` field — a buffered keystroke sequence via
//! [`crate::behaviour::typeahead::Typeahead`] — for a host that wants
//! type-to-jump instead of (or alongside) the live `text` filter.
//!
//! Everything the box paints — `text`, `committed`, `open`,
//! `highlighted` — is a descriptor field the host owns; the box's
//! persistent [`ComboBoxState`] holds only keystroke timing.
//!
//! # Focus and activation
//!
//! Like [`crate::Switch`], a `ComboBox` tracks its own
//! [`ComboBox::focused`] flag; [`ComboBox::handle_key`] is gated on
//! `focused` and not [`ComboBox::disabled`]. [`ComboBox::pick`] (a
//! dropdown-row activation) is gated on `disabled` alone, matching
//! [`crate::Switch::click`]'s "a pointer click doesn't require prior
//! keyboard focus" contract.

use crate::a11y::{A11yAction, A11yInfo, A11yPrimitive, A11yRole};
use crate::behaviour::nav::{FlatNav, NavKey};
use crate::behaviour::typeahead::Typeahead;
use crate::event::{Key, NamedKey, Rect};
use crate::interactive::{
    EventCx, FocusSpec, Interactive, KeyEvent, Never, PointerKind, RoutedEvent, SubTarget,
    WidgetAction,
};
use crate::popup::{Popup, PopupKey};
use crate::primitive::{LayoutCtx, Primitive, Size, Units};
use crate::types::{Modifiers, WidgetId};
use crate::Handled;
use serde::{Deserialize, Serialize};

/// The most rows the dropdown asks for. A longer filtered list scrolls
/// inside them, keeping [`ComboBox::highlighted`] in view.
pub const MAX_DROPDOWN_ROWS: usize = 8;

/// The key of the box's one popup, its dropdown list.
const DROPDOWN: PopupKey = PopupKey(0);

/// Declarative description of an editable combo box: free text plus a
/// filtered dropdown of fixed options.
///
/// # Examples
///
/// ```
/// use quadraui::{ComboBox, ComboBoxEvent, Key, Modifiers, WidgetId};
///
/// let mut combo = ComboBox::new(
///     WidgetId::new("form:fruit"),
///     vec!["Apple".into(), "Apricot".into(), "Banana".into()],
/// );
/// combo.focused = true;
///
/// let event = combo.handle_key(&Key::Char('a'), &Modifiers::default());
/// assert_eq!(
///     event,
///     Some(ComboBoxEvent::TextChanged { id: combo.id.clone(), text: "a".into() })
/// );
///
/// combo.text = "ap".into();
/// assert_eq!(combo.filtered_indices(), vec![0, 1]);
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct ComboBox {
    pub id: WidgetId,
    /// Fixed candidate list [`ComboBox::filtered_indices`] matches
    /// against.
    pub options: Vec<String>,
    /// Live edit buffer — what the field currently displays.
    #[serde(default)]
    pub text: String,
    /// Last confirmed value. Only changes via
    /// [`ComboBoxEvent::Committed`] — the host applies it the same way
    /// every other primitive's events are applied.
    #[serde(default)]
    pub committed: String,
    /// Whether the filtered dropdown is currently shown.
    #[serde(default)]
    pub open: bool,
    /// Cursor index into the *filtered* list (from
    /// [`ComboBox::filtered_indices`]), not the raw `options` list.
    #[serde(default)]
    pub highlighted: Option<usize>,
    /// When `true`, every method below returns `None` — same
    /// "disabled means inert, not hidden" contract every other
    /// primitive's `disabled` flag follows.
    #[serde(default)]
    pub disabled: bool,
    /// Whether this field currently holds keyboard focus. Set by the
    /// host's focus-traversal logic; read by
    /// [`ComboBox::handle_key`] to gate typed/arrow-key handling.
    #[serde(default)]
    pub focused: bool,
}

impl ComboBox {
    /// A combo box over `options`, with an empty text buffer, closed
    /// dropdown, enabled and unfocused. Set `text`/`open`/`disabled`/
    /// `focused` directly afterwards (every field is `pub`) rather than
    /// via a `with_*` chain — see `docs/PRIMITIVE_RULES.md`'s rule-8
    /// history of why this crate stopped adding those.
    pub fn new(id: WidgetId, options: Vec<String>) -> Self {
        Self {
            id,
            options,
            text: String::new(),
            committed: String::new(),
            open: false,
            highlighted: None,
            disabled: false,
            focused: false,
        }
    }

    /// Indices into [`ComboBox::options`] that case-insensitively
    /// contain [`ComboBox::text`] as a substring. Empty `text` matches
    /// every option.
    pub fn filtered_indices(&self) -> Vec<usize> {
        if self.text.is_empty() {
            return (0..self.options.len()).collect();
        }
        let needle = self.text.to_lowercase();
        self.options
            .iter()
            .enumerate()
            .filter(|(_, option)| option.to_lowercase().contains(&needle))
            .map(|(i, _)| i)
            .collect()
    }

    /// The id the open dropdown list paints and registers under — what a
    /// driver passes to [`crate::primitive::painted_sub_center`] with
    /// `SubTarget(n)` to aim at filtered row `n`.
    pub fn dropdown_id(&self) -> WidgetId {
        WidgetId::new(format!("{}:dropdown", self.id))
    }

    /// Move [`ComboBox::highlighted`] by `delta` through the filtered
    /// list with wraparound, mirroring
    /// [`crate::RadioGroup`]'s cursor movement: `None` seeds at index 0
    /// moving forward or the last index moving backward, so the first
    /// step lands correctly. Delegates to [`FlatNav`] for the
    /// clamped/wrapping index arithmetic — `delta` is always `±1` here
    /// (`Up`/`Down`), so it maps straight onto [`NavKey::Up`]/[`NavKey::Down`].
    fn move_highlight(&self, delta: isize) -> Option<usize> {
        let len = self.filtered_indices().len();
        let key = if delta >= 0 { NavKey::Down } else { NavKey::Up };
        FlatNav::mv(len, self.highlighted, key, true, |_| true)
    }

    /// Resolve which filtered-list index a buffered keystroke sequence
    /// should highlight, continuing the search from the current
    /// [`ComboBox::highlighted`] position with wraparound, driven by
    /// [`Typeahead`]'s debounced keystroke buffer instead of the live
    /// `text` query. Matching is case-insensitive like
    /// [`ComboBox::filtered_indices`], but on an option's *prefix* (the
    /// type-to-jump semantic) where `filtered_indices` matches a substring
    /// (the filter-as-you-type semantic). The host owns `typeahead` across
    /// keystrokes the same way it owns every other piece of input-timing
    /// state this crate exposes, rather than `ComboBox` carrying it on
    /// the wire.
    pub fn typeahead_highlight(
        &self,
        typeahead: &mut Typeahead,
        ch: char,
        now_ms: u64,
    ) -> Option<usize> {
        let filtered = self.filtered_indices();
        if filtered.is_empty() {
            return None;
        }
        let from = self.highlighted.unwrap_or(0).min(filtered.len() - 1);
        typeahead.push(
            ch,
            now_ms,
            |i| self.options[filtered[i]].as_str(),
            filtered.len(),
            from,
        )
    }

    /// `Up`/`Down` move the highlighted dropdown cursor; `Enter`
    /// commits; `Escape` cancels an open edit; digits/letters and
    /// `Backspace` edit `text`. Gated on `focused` and not `disabled`
    /// — any other key is unhandled (`None`).
    pub fn handle_key(&self, key: &Key, _modifiers: &Modifiers) -> Option<ComboBoxEvent> {
        if self.disabled || !self.focused {
            return None;
        }
        match key {
            Key::Named(NamedKey::Down) => {
                let index = self.move_highlight(1)?;
                Some(ComboBoxEvent::HighlightChanged { id: self.id, index })
            }
            Key::Named(NamedKey::Up) => {
                let index = self.move_highlight(-1)?;
                Some(ComboBoxEvent::HighlightChanged { id: self.id, index })
            }
            Key::Named(NamedKey::Enter) => Some(self.commit()),
            Key::Named(NamedKey::Escape) => self
                .open
                .then_some(ComboBoxEvent::Cancelled { id: self.id }),
            Key::Named(NamedKey::Backspace) => {
                let mut text = self.text.clone();
                text.pop();
                Some(ComboBoxEvent::TextChanged { id: self.id, text })
            }
            Key::Char(c) => {
                let mut text = self.text.clone();
                text.push(*c);
                Some(ComboBoxEvent::TextChanged { id: self.id, text })
            }
            _ => None,
        }
    }

    /// Resolve the committed value: the highlighted filtered option
    /// when the dropdown is open and something is highlighted,
    /// otherwise the typed text verbatim.
    fn commit(&self) -> ComboBoxEvent {
        let filtered = self.filtered_indices();
        let value = match self.highlighted.and_then(|i| filtered.get(i)) {
            Some(&option_idx) if self.open => self.options[option_idx].clone(),
            _ => self.text.clone(),
        };
        ComboBoxEvent::Committed { id: self.id, value }
    }

    /// Commit filtered option `index` — what a press on that dropdown row
    /// does. `None` while [`ComboBox::disabled`] or when `index` is past
    /// the filtered list. Focus is not required, matching
    /// [`crate::Switch::click`].
    pub fn pick(&self, index: usize) -> Option<ComboBoxEvent> {
        if self.disabled {
            return None;
        }
        let option_idx = *self.filtered_indices().get(index)?;
        Some(ComboBoxEvent::Committed {
            id: self.id,
            value: self.options[option_idx].clone(),
        })
    }
}

impl Default for ComboBox {
    /// An empty combo box with no options. See [`ComboBox::new`] for one
    /// with a real id/options list.
    fn default() -> Self {
        Self::new(WidgetId::new(""), Vec::new())
    }
}

/// Emitted by [`ComboBox`]'s methods. `TextChanged` carries the new
/// edit-buffer text (host also opens the dropdown in response);
/// `HighlightChanged` carries the new filtered-list cursor index;
/// `Committed` carries the resolved final value; `Cancelled` carries
/// only the id — the host reverts `text` to `committed` and closes the
/// dropdown.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ComboBoxEvent {
    TextChanged { id: WidgetId, text: String },
    HighlightChanged { id: WidgetId, index: usize },
    Committed { id: WidgetId, value: String },
    Cancelled { id: WidgetId },
}

impl WidgetAction for ComboBoxEvent {
    /// Every outcome rides [`crate::UiEvent::Form`] under the box's own
    /// id: [`crate::FormEvent`]'s four `ComboBox*` variants name exactly
    /// the four outcomes resolved above, one for one, with the same
    /// "typing fires every keystroke, Enter fires once" split. A
    /// standalone `ComboBox` therefore reaches an app through the same
    /// event vocabulary an embedded one does, and the apply contract
    /// each variant documents — `TextChanged` also sets `open = true`,
    /// `Cancelled` reverts `text` to `committed` and closes — is the
    /// same on both paths.
    fn into_ui_event(self, id: WidgetId) -> crate::UiEvent {
        let event = match self {
            ComboBoxEvent::TextChanged { id, text } => {
                crate::FormEvent::ComboBoxTextChanged { id, value: text }
            }
            ComboBoxEvent::HighlightChanged { id, index } => {
                crate::FormEvent::ComboBoxHighlightChanged { id, index }
            }
            ComboBoxEvent::Committed { id, value } => {
                crate::FormEvent::ComboBoxCommitted { id, value }
            }
            ComboBoxEvent::Cancelled { id } => crate::FormEvent::ComboBoxCancelled { id },
        };
        crate::UiEvent::Form(id, event)
    }
}

/// [`ComboBox`]'s fully-resolved widget-contract layout: the absolute
/// `area` the box was laid out in, its field row, and how many rows the
/// open dropdown asks for.
///
/// `#[non_exhaustive]`: produced by [`Primitive::layout`], never built by
/// a consumer via struct literal.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct ComboBoxPrimitiveLayout {
    /// The rect [`Primitive::layout`] was called with.
    pub area: Rect,
    /// Row pitch the field and every dropdown row resolve at, from
    /// [`crate::LayoutCtx::line_height`].
    pub row_height: f32,
    /// The field: the first row of [`Self::area`], in the same absolute
    /// space.
    pub field: Rect,
    /// Rows the dropdown popup asks for — `0` while closed or while
    /// nothing matches, otherwise the filtered count capped at
    /// [`MAX_DROPDOWN_ROWS`].
    pub dropdown_rows: usize,
}

impl ComboBoxPrimitiveLayout {
    /// [`Self::field`] relative to [`Self::area`]'s origin — the space
    /// [`RoutedEvent::Pointer`]'s `local` and a popup anchor use.
    fn local_field(&self) -> Rect {
        Rect::new(
            self.field.x - self.area.x,
            self.field.y - self.area.y,
            self.field.width,
            self.field.height,
        )
    }
}

impl ComboBox {
    /// The widest option's own content, in the units `ctx` measures in.
    fn widest_option(&self, ctx: &LayoutCtx) -> f32 {
        let cells = matches!(ctx.units(), Units::Cells);
        self.options
            .iter()
            .map(|option| {
                if cells {
                    crate::text_util::display_width(option) as f32
                } else {
                    ctx.text_width(option, crate::FontRole::Chrome)
                }
            })
            .fold(0.0_f32, f32::max)
    }

    /// The popup content listing the current filter.
    fn dropdown(&self) -> ComboBoxDropdown {
        ComboBoxDropdown {
            id: self.dropdown_id(),
            labels: self
                .filtered_indices()
                .into_iter()
                .map(|i| self.options[i].clone())
                .collect(),
            highlighted: self.highlighted,
        }
    }
}

impl Primitive for ComboBox {
    type Layout = ComboBoxPrimitiveLayout;
    type Event = ComboBoxEvent;

    fn id(&self) -> Option<&WidgetId> {
        Some(&self.id)
    }

    /// The widest option plus the chrome a dropdown row frames it with,
    /// by one row: the field. The open dropdown is a popup painted above
    /// the frame, so it takes no room from the box's container.
    fn measure(&self, ctx: &LayoutCtx, _avail: Size) -> crate::NaturalSize {
        let cells = matches!(ctx.units(), Units::Cells);
        // Two side borders plus the one-column pad a dropdown row insets
        // its label by.
        let chrome = if cells { 3.0 } else { 3.0 * ctx.char_width };
        crate::NaturalSize::new(
            Some(self.widest_option(ctx) + chrome),
            Some(ctx.line_height),
        )
    }

    /// The field takes the first row of `rect`; the rest of `rect` is
    /// left alone. [`Interactive::popups`] places the dropdown from this
    /// layout.
    fn layout(&self, ctx: &LayoutCtx, rect: Rect) -> ComboBoxPrimitiveLayout {
        let row_height = if ctx.line_height > 0.0 {
            ctx.line_height
        } else {
            1.0
        };
        let field = Rect::new(rect.x, rect.y, rect.width, row_height.min(rect.height));
        let dropdown_rows = if self.open {
            self.filtered_indices().len().min(MAX_DROPDOWN_ROWS)
        } else {
            0
        };
        ComboBoxPrimitiveLayout {
            area: rect,
            row_height,
            field,
            dropdown_rows,
        }
    }

    fn paint(
        &self,
        layout: &ComboBoxPrimitiveLayout,
        surface: &mut dyn crate::paint_surface::PaintSurface,
        env: &crate::PaintEnv,
    ) {
        native_surface_paint::field(self, layout.field, surface, env.theme());
    }

    #[cfg(feature = "tui")]
    fn paint_cells(
        &self,
        layout: &ComboBoxPrimitiveLayout,
        cells: &mut dyn crate::CellSurface,
        env: &crate::PaintEnv,
    ) {
        cell_paint::field(self, layout.field, cells, env.theme());
    }
}

/// [`ComboBox`]'s own interaction state: the debounced typeahead buffer
/// that lets a few quick keystrokes jump the dropdown highlight to the
/// best prefix match.
///
/// Everything else a `ComboBox` renders — `text`, `committed`, `open`,
/// `highlighted` — lives on the descriptor, which the host updates from
/// [`ComboBoxEvent`]; only the keystroke *timing* can't, since it is not
/// something a frame paints.
#[derive(Debug, Default)]
pub struct ComboBoxState {
    typeahead: Typeahead,
}

impl ComboBox {
    /// This box with `text` replaced — the post-edit box a typed
    /// character's own typeahead jump resolves against, so the
    /// highlighted index names a row of the filter the host is about to
    /// apply rather than the one being replaced.
    fn with_text(&self, text: String) -> ComboBox {
        let mut next = self.clone();
        next.text = text;
        next
    }

    /// Ask the host to show the dropdown.
    ///
    /// [`ComboBoxEvent::TextChanged`] carries no edit here — the text is
    /// the box's own, unchanged — because its documented apply contract
    /// is exactly "set `text`, and set `open = true`", which is the whole
    /// of what opening needs. There is no second "open" event to add
    /// alongside the four [`ComboBoxEvent`] already names, and reusing
    /// this one keeps the standalone box and a `Form` row applying the
    /// same vocabulary.
    fn open_request(&self) -> Handled<ComboBoxEvent> {
        Handled::Event(ComboBoxEvent::TextChanged {
            id: self.id,
            text: self.text.clone(),
        })
    }

    /// [`Interactive::handle`]'s answer to an event that landed on the
    /// open dropdown: a left press on a row commits that row, and
    /// anything else over the list is swallowed.
    fn handle_dropdown_event(&self, event: &RoutedEvent) -> Handled<ComboBoxEvent> {
        match event {
            RoutedEvent::Pointer {
                kind: PointerKind::Down(crate::MouseButton::Left),
                sub: Some(SubTarget(index)),
                ..
            } => match self.pick(*index as usize) {
                Some(event) => Handled::Event(event),
                None => Handled::Consumed,
            },
            _ => Handled::Consumed,
        }
    }
}

impl Interactive for ComboBox {
    type State = ComboBoxState;
    type Action = ComboBoxEvent;

    const OPENS_POPUPS: bool = true;

    /// One focus stop as a whole — the box navigates its dropdown
    /// internally through [`Self::handle`]'s own key dispatch rather
    /// than declaring a stop per option. [`FocusSpec::None`] while
    /// [`ComboBox::disabled`], matching the "disabled means inert, not
    /// hidden" contract every method here follows.
    fn focus(&self) -> FocusSpec {
        if self.disabled {
            FocusSpec::None
        } else {
            FocusSpec::Stop
        }
    }

    fn handle(
        &self,
        state: &mut ComboBoxState,
        event: &RoutedEvent,
        layout: &ComboBoxPrimitiveLayout,
        cx: &mut EventCx,
    ) -> Handled<ComboBoxEvent> {
        if self.disabled {
            return Handled::Ignored;
        }
        match event {
            // Clicking the field focuses it and drops the list open, the
            // affordance that makes the closed box look clickable at all.
            RoutedEvent::Pointer {
                kind: PointerKind::Down(crate::MouseButton::Left),
                local,
                ..
            } if layout.local_field().contains(*local) => {
                cx.request_focus(None);
                if self.open {
                    Handled::Consumed
                } else {
                    self.open_request()
                }
            }
            RoutedEvent::PopupEvent { popup, event } if *popup == DROPDOWN => {
                self.handle_dropdown_event(event)
            }
            // An outside press or Escape closes the list the way Escape
            // always has: the edit is cancelled back to `committed`.
            RoutedEvent::PopupDismissed { popup, .. } if *popup == DROPDOWN => {
                Handled::Event(ComboBoxEvent::Cancelled { id: self.id })
            }
            RoutedEvent::Key(key) => self.handle_routed_key(state, key, cx),
            // `A11yAction` names no option through this generic seam, so
            // `Click` activates whichever option is highlighted — the
            // same one Enter commits — and opens the list when there is
            // none, matching the assistive-technology default-action
            // convention.
            RoutedEvent::Action(A11yAction::Click) => {
                if self.open {
                    Handled::Event(self.commit())
                } else {
                    self.open_request()
                }
            }
            _ => Handled::Ignored,
        }
    }

    fn a11y_actions(&self, _state: &ComboBoxState) -> Vec<A11yAction> {
        if self.disabled {
            Vec::new()
        } else {
            vec![A11yAction::Click]
        }
    }

    /// The field. The dropdown's rows are named by the popup content's
    /// own sub-regions, and arrive as [`RoutedEvent::PopupEvent`].
    fn sub_regions(&self, layout: &ComboBoxPrimitiveLayout) -> Vec<(Rect, SubTarget)> {
        vec![(layout.local_field(), SubTarget(SUB_FIELD))]
    }

    /// The filtered option list, under the field, while
    /// [`ComboBox::open`]. A filter that matches nothing leaves the list
    /// open with no rows, so an outside press still dismisses it.
    fn popups(&self, _state: &ComboBoxState, layout: &ComboBoxPrimitiveLayout) -> Vec<Popup> {
        if !self.open {
            return Vec::new();
        }
        let field = layout.local_field();
        let size = Size::new(field.width, layout.dropdown_rows as f32 * layout.row_height);
        vec![Popup::new(DROPDOWN, field, size, self.dropdown())]
    }
}

/// [`Interactive::sub_regions`]'s tag for the editable field row.
const SUB_FIELD: u64 = 0;

impl ComboBox {
    /// [`Interactive::handle`]'s `Key` arm.
    ///
    /// Dispatch order mirrors what the keystroke means to a *closed* box
    /// versus an open one, and everything a key means in both states is
    /// resolved by the one shared [`ComboBox::handle_key`] so the
    /// standalone box and a `Form` row can never disagree:
    ///
    /// - `Alt+Down` and, while closed, `Down`/`Enter` drop the list open.
    ///   (`Space`, like every other printable character, types — and
    ///   [`ComboBoxEvent::TextChanged`]'s apply contract opens the list
    ///   as a result, so it opens too.) This is the one place the
    ///   standalone box's key map is wider than
    ///   [`ComboBox::handle_key`]'s: a `Form` row reaches its fields
    ///   through the form's own focus model and so has no need of an
    ///   Enter-to-open affordance, where a standalone box is the only
    ///   thing on screen and does.
    /// - While open, `Up`/`Down` move the highlight through
    ///   [`crate::behaviour::nav::FlatNav`] and `Enter` commits the
    ///   highlighted option — [`ComboBox::handle_key`]'s own answers.
    ///   Escape over an open list reaches [`Interactive::handle`] as a
    ///   [`RoutedEvent::PopupDismissed`] instead, with the same outcome.
    /// - A printable character edits `text` *and* jumps the highlight to
    ///   the best prefix match in the filter the edit produces, through
    ///   [`crate::behaviour::typeahead::Typeahead`]'s debounced buffer.
    ///   The jump is reported on [`EventCx::emit`] alongside the returned
    ///   edit, since one keystroke changes two independent descriptor
    ///   fields.
    fn handle_routed_key(
        &self,
        state: &mut ComboBoxState,
        key: &KeyEvent,
        cx: &mut EventCx,
    ) -> Handled<ComboBoxEvent> {
        let opens = match key.key {
            Key::Named(NamedKey::Down) => key.modifiers.alt || !self.open,
            Key::Named(NamedKey::Enter) => !self.open,
            _ => false,
        };
        if opens {
            return self.open_request();
        }

        if let Key::Char(ch) = key.key {
            let Some(ComboBoxEvent::TextChanged { text, .. }) =
                self.handle_key(&key.key, &key.modifiers)
            else {
                return Handled::Ignored;
            };
            let edited = self.with_text(text.clone());
            let now_ms = crate::behaviour::typeahead::monotonic_now_ms();
            if let Some(index) = edited.typeahead_highlight(&mut state.typeahead, ch, now_ms) {
                if self.highlighted != Some(index) {
                    cx.emit(
                        ComboBoxEvent::HighlightChanged { id: self.id, index }
                            .into_ui_event(self.id),
                    );
                }
            }
            return Handled::Event(ComboBoxEvent::TextChanged { id: self.id, text });
        }

        match self.handle_key(&key.key, &key.modifiers) {
            Some(event) => Handled::Event(event),
            None => Handled::Ignored,
        }
    }
}

/// The open dropdown's content: the filtered option labels, one row
/// each, with the highlighted one tinted. It only paints and names its
/// rows; every event over it reaches the owning [`ComboBox`].
#[derive(Debug, Clone)]
struct ComboBoxDropdown {
    id: WidgetId,
    /// The filtered options' labels, in filtered order.
    labels: Vec<String>,
    /// Index into `labels`.
    highlighted: Option<usize>,
}

/// [`ComboBoxDropdown`]'s resolved rows.
#[derive(Debug, Clone, PartialEq)]
struct ComboBoxDropdownLayout {
    /// The popup's absolute rect.
    area: Rect,
    /// Each visible row's filtered index and absolute rect, top to
    /// bottom.
    rows: Vec<(usize, Rect)>,
}

impl ComboBoxDropdown {
    /// The first visible row when `visible` rows show: scrolled just far
    /// enough that the highlighted row is the last one in view.
    fn first_visible(&self, visible: usize) -> usize {
        let last_start = self.labels.len().saturating_sub(visible);
        match self.highlighted {
            Some(h) if h >= visible => (h + 1 - visible).min(last_start),
            _ => 0,
        }
    }
}

impl A11yPrimitive for ComboBoxDropdown {
    fn a11y_info(&self) -> A11yInfo {
        A11yInfo::new()
            .with_role(A11yRole::List)
            .with_set_size(self.labels.len() as u32)
    }
}

impl Primitive for ComboBoxDropdown {
    type Layout = ComboBoxDropdownLayout;
    type Event = ();

    fn id(&self) -> Option<&WidgetId> {
        Some(&self.id)
    }

    fn layout(&self, ctx: &LayoutCtx, rect: Rect) -> ComboBoxDropdownLayout {
        let row_height = if ctx.line_height > 0.0 {
            ctx.line_height
        } else {
            1.0
        };
        let fits = ((rect.height + 0.001) / row_height).floor().max(0.0) as usize;
        let visible = fits.min(self.labels.len());
        let first = self.first_visible(visible);
        let rows = (0..visible)
            .map(|k| {
                let y = rect.y + k as f32 * row_height;
                (first + k, Rect::new(rect.x, y, rect.width, row_height))
            })
            .collect();
        ComboBoxDropdownLayout { area: rect, rows }
    }

    fn paint(
        &self,
        layout: &ComboBoxDropdownLayout,
        surface: &mut dyn crate::paint_surface::PaintSurface,
        env: &crate::PaintEnv,
    ) {
        native_surface_paint::dropdown(self, layout, surface, env.theme());
    }

    #[cfg(feature = "tui")]
    fn paint_cells(
        &self,
        layout: &ComboBoxDropdownLayout,
        cells: &mut dyn crate::CellSurface,
        env: &crate::PaintEnv,
    ) {
        cell_paint::dropdown(self, layout, cells, env.theme());
    }
}

impl Interactive for ComboBoxDropdown {
    type State = ();
    type Action = Never;

    fn handle(
        &self,
        _state: &mut (),
        _event: &RoutedEvent,
        _layout: &ComboBoxDropdownLayout,
        _cx: &mut EventCx,
    ) -> Handled<Never> {
        Handled::Ignored
    }

    /// One sub-part per visible row, tagged with its filtered index.
    fn sub_regions(&self, layout: &ComboBoxDropdownLayout) -> Vec<(Rect, SubTarget)> {
        layout
            .rows
            .iter()
            .map(|(index, r)| {
                (
                    Rect::new(r.x - layout.area.x, r.y - layout.area.y, r.width, r.height),
                    SubTarget(*index as u64),
                )
            })
            .collect()
    }
}

/// The cell-grid painters for [`ComboBox`]'s field and its dropdown,
/// written against [`crate::CellSurface`].
///
/// The field is a box-drawing frame around [`ComboBox::text`], the same
/// chrome `crate::tui::text_input` paints. The dropdown uses the row
/// chrome `crate::primitives::completions::cell_paint` paints: side `│`
/// borders either edge of every row, and the highlighted row's
/// background tinted.
///
/// Colours: [`crate::Theme::input_bg`] washes the field,
/// [`crate::Theme::accent_fg`] replaces [`crate::Theme::border_fg`] on a
/// focused box and [`crate::Theme::muted_fg`] replaces
/// [`crate::Theme::surface_fg`] on a disabled one; the dropdown uses the
/// `completion_*` family. A dropdown narrower than three columns (two
/// borders plus one content column) paints no list.
#[cfg(feature = "tui")]
pub(crate) mod cell_paint {
    use super::{ComboBox, ComboBoxDropdown, ComboBoxDropdownLayout};
    use crate::cell_surface::{draw_text_row, solid_style, stroke_square_frame, CellSurface};
    use crate::event::Rect;
    use crate::theme::Theme;

    /// Paint `combo_box`'s field into `field`.
    pub(crate) fn field(
        combo_box: &ComboBox,
        field: Rect,
        surface: &mut dyn CellSurface,
        theme: &Theme,
    ) {
        if field.width <= 0.0 || field.height <= 0.0 {
            return;
        }
        let border = if combo_box.disabled || !combo_box.focused {
            theme.border_fg
        } else {
            theme.accent_fg
        };
        let fg = if combo_box.disabled {
            theme.muted_fg
        } else {
            theme.surface_fg
        };
        let field_style = solid_style(fg, theme.input_bg);
        surface.fill(field, field_style);
        stroke_square_frame(surface, field, solid_style(border, theme.background));
        if !combo_box.text.is_empty() {
            draw_text_row(surface, field, &combo_box.text, field_style);
        }
    }

    /// Paint the open dropdown's rows.
    pub(super) fn dropdown(
        dropdown: &ComboBoxDropdown,
        layout: &ComboBoxDropdownLayout,
        surface: &mut dyn CellSurface,
        theme: &Theme,
    ) {
        let pop_bg = theme.completion_bg;
        let border_style = solid_style(theme.completion_border, pop_bg);
        let x = layout.area.x.round() as u16;
        let w = layout.area.width.round() as u16;
        if w < 3 {
            return;
        }
        for (index, row) in &layout.rows {
            let y = row.y.round() as u16;
            let row_bg = if dropdown.highlighted == Some(*index) {
                theme.completion_selected_bg
            } else {
                pop_bg
            };
            let row_style = solid_style(theme.completion_fg, row_bg);
            surface.fill(Rect::new(x as f32, y as f32, w as f32, 1.0), row_style);
            surface.put_char(x, y, '│', border_style);
            surface.put_char(x + w - 1, y, '│', border_style);
            if let Some(label) = dropdown.labels.get(*index) {
                // One leading pad column, and the trailing border column
                // the label must stop short of.
                surface.put_str_clipped(x + 1, y, &format!(" {label}"), w - 2, row_style);
            }
        }
    }
}

/// The pixel painters for [`ComboBox`]'s field and its dropdown, shared
/// by every pixel backend — a rasteriser calls into these rather than
/// re-deriving geometry or re-filtering `options` itself (see
/// `docs/PRIMITIVE_RULES.md`'s "Primitive-first" rule).
pub(crate) mod native_surface_paint {
    use super::{ComboBox, ComboBoxDropdown, ComboBoxDropdownLayout};
    use crate::event::Rect;
    use crate::paint_surface::PaintSurface;
    use crate::theme::Theme;

    /// Paint `combo_box`'s field into `field`: a bordered text box
    /// showing [`ComboBox::text`].
    pub(crate) fn field(
        combo_box: &ComboBox,
        field: Rect,
        surface: &mut dyn PaintSurface,
        theme: &Theme,
    ) {
        if field.width <= 0.0 || field.height <= 0.0 {
            return;
        }
        let border = if !combo_box.disabled && combo_box.focused {
            theme.accent_fg
        } else {
            theme.border_fg
        };
        let fg = if combo_box.disabled {
            theme.muted_fg
        } else {
            theme.surface_fg
        };
        surface.surface_fill_rect(field, theme.input_bg);
        surface.surface_stroke_rect(field, border, 1.0);
        if !combo_box.text.is_empty() {
            surface.surface_draw_text_run(field, &combo_box.text, fg);
        }
    }

    /// Paint the open dropdown: its background and border, the
    /// highlighted row's selection fill, and every visible label.
    pub(super) fn dropdown(
        dropdown: &ComboBoxDropdown,
        layout: &ComboBoxDropdownLayout,
        surface: &mut dyn PaintSurface,
        theme: &Theme,
    ) {
        if layout.area.width <= 0.0 || layout.area.height <= 0.0 {
            return;
        }
        surface.surface_fill_rect(layout.area, theme.completion_bg);
        surface.surface_stroke_rect(layout.area, theme.completion_border, 1.0);
        for (index, row) in &layout.rows {
            if dropdown.highlighted == Some(*index) {
                surface.surface_fill_rect(*row, theme.completion_selected_bg);
            }
            if let Some(label) = dropdown.labels.get(*index) {
                surface.surface_draw_text_run(*row, label, theme.completion_fg);
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::backend::ImagePaintResult;
        use crate::event::{Point, Viewport};
        use crate::primitive::{LayoutCtx, Primitive};
        use crate::types::{Color, WidgetId};
        use crate::Image;

        /// Records every surface verb `ComboBox`'s paint uses — mirrors
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

        fn combo() -> ComboBox {
            ComboBox::new(
                WidgetId::new("form:fruit"),
                vec!["Apple".into(), "Apricot".into(), "Banana".into()],
            )
        }

        const FIELD: Rect = Rect::new(0.0, 0.0, 20.0, 1.0);

        fn paint_dropdown(c: &ComboBox, rect: Rect) -> (ComboBoxDropdownLayout, RecordingSurface) {
            let content = c.dropdown();
            let layout = Primitive::layout(&content, &LayoutCtx::new(1.0, 1.0, false), rect);
            let mut surface = RecordingSurface::default();
            dropdown(&content, &layout, &mut surface, &Theme::default());
            (layout, surface)
        }

        #[test]
        fn paints_field_background_border_and_text() {
            let mut c = combo();
            c.text = "Cher".into();
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            field(&c, FIELD, &mut surface, &theme);

            assert_eq!(surface.fills[0], (FIELD, theme.input_bg));
            assert!(surface.strokes.contains(&(FIELD, theme.border_fg)));
            assert_eq!(surface.text_runs[0].1, "Cher");
        }

        #[test]
        fn the_dropdown_paints_every_filtered_option_label() {
            let mut c = combo();
            c.text = "ap".into(); // filtered: Apple, Apricot
            let theme = Theme::default();
            let (_, surface) = paint_dropdown(&c, Rect::new(0.0, 1.0, 20.0, 2.0));

            assert!(surface
                .fills
                .iter()
                .any(|(_, color)| *color == theme.completion_bg));
            let labels: Vec<&str> = surface
                .text_runs
                .iter()
                .map(|(_, text, _)| text.as_str())
                .collect();
            assert_eq!(labels, vec!["Apple", "Apricot"]);
        }

        #[test]
        fn the_highlighted_row_gets_a_selection_fill() {
            let mut c = combo();
            c.text = "ap".into(); // filtered: [0 Apple, 1 Apricot]
            c.highlighted = Some(1);
            let theme = Theme::default();
            let (layout, surface) = paint_dropdown(&c, Rect::new(0.0, 1.0, 20.0, 2.0));

            assert!(surface
                .fills
                .contains(&(layout.rows[1].1, theme.completion_selected_bg)));
            assert!(!surface
                .fills
                .contains(&(layout.rows[0].1, theme.completion_selected_bg)));
        }

        #[test]
        fn zero_size_field_paints_nothing_for_the_field() {
            let mut surface = RecordingSurface::default();
            field(
                &combo(),
                Rect::new(0.0, 0.0, 0.0, 0.0),
                &mut surface,
                &Theme::default(),
            );
            assert!(surface.fills.is_empty());
            assert!(surface.strokes.is_empty());
            assert!(surface.text_runs.is_empty());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options() -> Vec<String> {
        vec!["Apple".into(), "Apricot".into(), "Banana".into()]
    }

    fn combo() -> ComboBox {
        ComboBox::new(WidgetId::new("form:fruit"), options())
    }

    #[test]
    fn new_starts_empty_closed_enabled_unfocused() {
        let c = combo();
        assert_eq!(c.text, "");
        assert_eq!(c.committed, "");
        assert!(!c.open);
        assert_eq!(c.highlighted, None);
        assert!(!c.disabled);
        assert!(!c.focused);
    }

    #[test]
    fn filtered_indices_matches_case_insensitive_substring() {
        let mut c = combo();
        assert_eq!(c.filtered_indices(), vec![0, 1, 2]);

        c.text = "ap".into();
        assert_eq!(c.filtered_indices(), vec![0, 1]);

        c.text = "AP".into();
        assert_eq!(c.filtered_indices(), vec![0, 1]);

        c.text = "nana".into();
        assert_eq!(c.filtered_indices(), vec![2]);

        c.text = "zzz".into();
        assert_eq!(c.filtered_indices(), Vec::<usize>::new());
    }

    #[test]
    fn typeahead_highlight_jumps_to_the_matching_option() {
        let c = combo();
        let mut typeahead = Typeahead::new();
        // "Apple" (0), "Apricot" (1), "Banana" (2) — typing "b" jumps to
        // "Banana".
        assert_eq!(c.typeahead_highlight(&mut typeahead, 'b', 0), Some(2));
    }

    #[test]
    fn typeahead_highlight_accumulates_and_resets_like_combo_box_matching() {
        let mut c = combo();
        let mut typeahead = Typeahead::new();
        assert_eq!(c.typeahead_highlight(&mut typeahead, 'a', 0), Some(0));
        c.highlighted = Some(0);
        // "ap" still matches "Apple" before "Apricot".
        assert_eq!(c.typeahead_highlight(&mut typeahead, 'p', 100), Some(0));
        c.highlighted = Some(0);
        // "apr" skips "Apple", matches "Apricot".
        assert_eq!(c.typeahead_highlight(&mut typeahead, 'r', 200), Some(1));
    }

    #[test]
    fn typed_chars_accumulate_in_text_when_focused() {
        let mut c = combo();
        c.focused = true;

        let e1 = c.handle_key(&Key::Char('a'), &Modifiers::default());
        assert_eq!(
            e1,
            Some(ComboBoxEvent::TextChanged {
                id: c.id.clone(),
                text: "a".into()
            })
        );
        c.text = "a".into();

        let e2 = c.handle_key(&Key::Char('p'), &Modifiers::default());
        assert_eq!(
            e2,
            Some(ComboBoxEvent::TextChanged {
                id: c.id.clone(),
                text: "ap".into()
            })
        );
    }

    #[test]
    fn backspace_pops_the_last_character() {
        let mut c = combo();
        c.focused = true;
        c.text = "ap".into();
        assert_eq!(
            c.handle_key(&Key::Named(NamedKey::Backspace), &Modifiers::default()),
            Some(ComboBoxEvent::TextChanged {
                id: c.id.clone(),
                text: "a".into()
            })
        );
    }

    #[test]
    fn down_and_up_move_the_highlight_through_the_filtered_list_with_wraparound() {
        let mut c = combo();
        c.focused = true;
        c.text = "ap".into(); // filtered: [0, 1]

        assert_eq!(
            c.handle_key(&Key::Named(NamedKey::Down), &Modifiers::default()),
            Some(ComboBoxEvent::HighlightChanged {
                id: c.id.clone(),
                index: 0
            })
        );
        c.highlighted = Some(0);

        assert_eq!(
            c.handle_key(&Key::Named(NamedKey::Down), &Modifiers::default()),
            Some(ComboBoxEvent::HighlightChanged {
                id: c.id.clone(),
                index: 1
            })
        );
        c.highlighted = Some(1);

        // Wraps back to 0.
        assert_eq!(
            c.handle_key(&Key::Named(NamedKey::Down), &Modifiers::default()),
            Some(ComboBoxEvent::HighlightChanged {
                id: c.id.clone(),
                index: 0
            })
        );

        c.highlighted = Some(0);
        // Up from 0 wraps to the last filtered index.
        assert_eq!(
            c.handle_key(&Key::Named(NamedKey::Up), &Modifiers::default()),
            Some(ComboBoxEvent::HighlightChanged {
                id: c.id.clone(),
                index: 1
            })
        );
    }

    #[test]
    fn a_highlight_left_past_the_end_of_a_narrowed_filter_reseeds_at_an_end() {
        let mut c = combo();
        c.focused = true;
        // A host that carried `highlighted` across a `TextChanged` which
        // narrowed the filter can hold an index past the filtered list; the
        // next move seeds at the end the direction implies rather than
        // stepping from an index outside that list.
        c.highlighted = Some(2);
        c.text = "ap".into(); // filtered: [0, 1]

        assert_eq!(
            c.handle_key(&Key::Named(NamedKey::Down), &Modifiers::default()),
            Some(ComboBoxEvent::HighlightChanged {
                id: c.id.clone(),
                index: 0
            })
        );
        assert_eq!(
            c.handle_key(&Key::Named(NamedKey::Up), &Modifiers::default()),
            Some(ComboBoxEvent::HighlightChanged {
                id: c.id.clone(),
                index: 1
            })
        );
    }

    #[test]
    fn navigation_is_inert_when_the_filtered_list_is_empty() {
        let mut c = combo();
        c.focused = true;
        c.text = "zzz".into();
        assert_eq!(
            c.handle_key(&Key::Named(NamedKey::Down), &Modifiers::default()),
            None
        );
    }

    #[test]
    fn enter_commits_the_highlighted_option_when_open() {
        let mut c = combo();
        c.focused = true;
        c.open = true;
        c.text = "ap".into();
        c.highlighted = Some(1); // "Apricot"
        assert_eq!(
            c.handle_key(&Key::Named(NamedKey::Enter), &Modifiers::default()),
            Some(ComboBoxEvent::Committed {
                id: c.id.clone(),
                value: "Apricot".into()
            })
        );
    }

    #[test]
    fn enter_commits_typed_text_verbatim_when_nothing_is_highlighted() {
        let mut c = combo();
        c.focused = true;
        c.text = "Cherry".into();
        assert_eq!(
            c.handle_key(&Key::Named(NamedKey::Enter), &Modifiers::default()),
            Some(ComboBoxEvent::Committed {
                id: c.id.clone(),
                value: "Cherry".into()
            })
        );
    }

    #[test]
    fn escape_cancels_an_open_edit_and_is_inert_otherwise() {
        let mut c = combo();
        c.focused = true;
        c.open = true;
        c.text = "ap".into();
        assert_eq!(
            c.handle_key(&Key::Named(NamedKey::Escape), &Modifiers::default()),
            Some(ComboBoxEvent::Cancelled { id: c.id.clone() })
        );

        c.open = false;
        assert_eq!(
            c.handle_key(&Key::Named(NamedKey::Escape), &Modifiers::default()),
            None
        );
    }

    #[test]
    fn handle_key_is_inert_when_not_focused_or_disabled() {
        let c = combo();
        assert_eq!(c.handle_key(&Key::Char('a'), &Modifiers::default()), None);

        let mut c = combo();
        c.focused = true;
        c.disabled = true;
        assert_eq!(c.handle_key(&Key::Char('a'), &Modifiers::default()), None);
    }

    #[test]
    fn pick_commits_a_filtered_row_without_requiring_focus() {
        let mut c = combo();
        c.text = "ap".into(); // filtered: [0, 1]
        assert_eq!(
            c.pick(1),
            Some(ComboBoxEvent::Committed {
                id: c.id.clone(),
                value: "Apricot".into()
            })
        );
        assert_eq!(c.pick(2), None, "past the filtered list");
    }

    #[test]
    fn pick_is_inert_when_disabled() {
        let mut c = combo();
        c.disabled = true;
        assert_eq!(c.pick(0), None);
    }

    #[test]
    fn layout_asks_for_dropdown_rows_only_while_open_and_matching() {
        let ctx = LayoutCtx::new(1.0, 1.0, false);
        let rect = Rect::new(0.0, 0.0, 20.0, 1.0);
        let mut c = combo();
        assert_eq!(Primitive::layout(&c, &ctx, rect).dropdown_rows, 0);

        c.open = true;
        let layout = Primitive::layout(&c, &ctx, rect);
        assert_eq!(layout.dropdown_rows, 3);
        assert_eq!(layout.field, rect);

        c.text = "zzz".into();
        assert_eq!(Primitive::layout(&c, &ctx, rect).dropdown_rows, 0);
    }

    #[test]
    fn layout_caps_the_dropdown_and_the_list_scrolls_to_the_highlight() {
        let ctx = LayoutCtx::new(1.0, 1.0, false);
        let mut c = ComboBox::new(
            WidgetId::new("form:n"),
            (0..20).map(|n| format!("item {n}")).collect(),
        );
        c.open = true;
        let layout = Primitive::layout(&c, &ctx, Rect::new(0.0, 0.0, 20.0, 1.0));
        assert_eq!(layout.dropdown_rows, MAX_DROPDOWN_ROWS);

        c.highlighted = Some(12);
        let content = c.dropdown();
        let rect = Rect::new(0.0, 1.0, 20.0, MAX_DROPDOWN_ROWS as f32);
        let rows = Primitive::layout(&content, &ctx, rect).rows;
        assert_eq!(rows.len(), MAX_DROPDOWN_ROWS);
        assert_eq!(
            rows.last().unwrap().0,
            12,
            "the highlight is the last row in view"
        );
        assert_eq!(rows[0].0, 12 + 1 - MAX_DROPDOWN_ROWS);

        c.highlighted = Some(19);
        let rows = Primitive::layout(&c.dropdown(), &ctx, rect).rows;
        assert_eq!(rows.last().unwrap().0, 19);
    }

    #[test]
    fn serde_round_trip() {
        let mut c = combo();
        c.text = "ap".into();
        c.open = true;
        c.highlighted = Some(1);
        c.focused = true;
        let json = serde_json::to_string(&c).unwrap();
        let back: ComboBox = serde_json::from_str(&json).unwrap();
        assert_eq!(c, back);
    }

    #[test]
    fn serde_defaults_optional_fields_when_omitted() {
        let json = r#"{"id":"form:fruit","options":["Apple","Banana"]}"#;
        let c: ComboBox = serde_json::from_str(json).unwrap();
        assert_eq!(c.text, "");
        assert_eq!(c.committed, "");
        assert!(!c.open);
        assert_eq!(c.highlighted, None);
        assert!(!c.disabled);
        assert!(!c.focused);
    }
    // ── Cell-path paint (`cell_paint`) ──────────────────────────────────

    #[cfg(feature = "tui")]
    mod cells {
        use super::super::cell_test_paint::{draw_dropdown, draw_field};
        use super::*;
        use crate::theme::Theme;
        use ratatui::buffer::Buffer;
        use ratatui::layout::Rect as RRect;

        fn cell_char(buf: &Buffer, x: u16, y: u16) -> char {
            buf[(x, y)].symbol().chars().next().unwrap_or(' ')
        }

        fn mk_combo_box() -> ComboBox {
            let mut c = ComboBox::new(
                WidgetId::new("combo"),
                vec!["alpha".to_string(), "beta".to_string(), "gamma".to_string()],
            );
            c.open = true;
            c.highlighted = Some(0);
            c.focused = true;
            c
        }

        fn paint_at(origin_x: u16, origin_y: u16) {
            let c = mk_combo_box();
            let field = Rect::new(origin_x as f32, origin_y as f32, 20.0, 1.0);
            let list = Rect::new(origin_x as f32, origin_y as f32 + 1.0, 20.0, 3.0);
            let mut buf = Buffer::empty(RRect::new(0, 0, 80, 24));
            draw_field(&mut buf, &c, field, &Theme::default());
            draw_dropdown(&mut buf, &c, list, &Theme::default());

            // Field border top-left corner paints at the absolute origin.
            assert_eq!(cell_char(&buf, origin_x, origin_y), '┌');
            // First option label ("alpha") renders inside the first row,
            // after the left border + 1-cell pad.
            assert_eq!(cell_char(&buf, origin_x + 2, origin_y + 1), 'a');
            assert_eq!(cell_char(&buf, origin_x + 2, origin_y + 3), 'g');
        }

        #[test]
        fn field_and_dropdown_paint_at_the_origin() {
            paint_at(0, 0);
        }

        /// Paint must work when the control is not at the screen origin.
        #[test]
        fn field_and_dropdown_paint_at_a_nonzero_origin() {
            paint_at(7, 3);
        }

        #[test]
        fn the_dropdown_frames_every_row_with_side_borders() {
            let c = mk_combo_box();
            let list = Rect::new(0.0, 1.0, 20.0, 3.0);
            let mut buf = Buffer::empty(RRect::new(0, 0, 80, 24));
            draw_dropdown(&mut buf, &c, list, &Theme::default());
            for y in 1..4 {
                assert_eq!(cell_char(&buf, 0, y), '│');
                assert_eq!(cell_char(&buf, 19, y), '│');
            }
        }

        #[test]
        fn the_field_paints_only_its_own_row() {
            let mut c = mk_combo_box();
            c.text = "alpha".into();
            let mut buf = Buffer::empty(RRect::new(0, 0, 80, 24));
            draw_field(
                &mut buf,
                &c,
                Rect::new(0.0, 0.0, 20.0, 1.0),
                &Theme::default(),
            );

            assert_eq!(cell_char(&buf, 0, 0), 'a', "text paints over the frame");
            assert_eq!(cell_char(&buf, 0, 1), ' ');
        }
    }
}

/// [`Interactive`] dispatch, driven directly the way an out-of-process
/// binding host reaches a widget — the in-runtime leg is
/// `tests/widget_contract_combo_box.rs`.
#[cfg(test)]
mod interactive_tests {
    use super::*;
    use crate::event::Point;
    use crate::interactive::EventHost;
    use crate::popup::DismissReason;
    use crate::PointerShape;

    /// Minimal [`EventHost`] mock: records every call so a test can
    /// assert on what [`Interactive::handle`] asked for.
    #[derive(Debug, Default)]
    struct MockHost {
        focus_requests: Vec<Option<SubTarget>>,
        emitted: Vec<crate::UiEvent>,
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
        fn emit(&mut self, event: crate::UiEvent) {
            self.emitted.push(event);
        }
        fn units(&self) -> Units {
            Units::Cells
        }
    }

    const AREA: Rect = Rect::new(2.0, 1.0, 20.0, 6.0);

    fn combo(open: bool) -> ComboBox {
        let mut c = ComboBox::new(
            WidgetId::new("form:fruit"),
            vec!["Apple".into(), "Apricot".into(), "Banana".into()],
        );
        c.focused = true;
        c.open = open;
        c
    }

    fn layout_for(c: &ComboBox) -> ComboBoxPrimitiveLayout {
        let ctx = LayoutCtx::new(1.0, 1.0, false);
        Primitive::layout(c, &ctx, AREA)
    }

    fn dispatch(c: &ComboBox, event: &RoutedEvent) -> (Handled<ComboBoxEvent>, MockHost) {
        let layout = layout_for(c);
        let mut state = ComboBoxState::default();
        let mut host = MockHost::default();
        let handled = {
            let mut cx = EventCx::new(c.id, &mut host);
            c.handle(&mut state, event, &layout, &mut cx)
        };
        (handled, host)
    }

    fn key(key: Key, modifiers: Modifiers) -> RoutedEvent {
        RoutedEvent::Key(KeyEvent {
            key,
            modifiers,
            repeat: false,
        })
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
    fn an_enabled_box_is_one_focus_stop_and_a_disabled_one_is_none() {
        assert_eq!(combo(false).focus(), FocusSpec::Stop);
        let mut disabled = combo(false);
        disabled.disabled = true;
        assert_eq!(disabled.focus(), FocusSpec::None);
        assert_eq!(
            dispatch(&disabled, &key(Key::Char('a'), Modifiers::default())).0,
            Handled::Ignored
        );
    }

    #[test]
    fn an_open_box_answers_one_popup_dropped_below_its_field() {
        let c = combo(true);
        let layout = layout_for(&c);
        assert_eq!(layout.row_height, 1.0);
        assert_eq!(layout.field, Rect::new(2.0, 1.0, 20.0, 1.0));
        let popups = c.popups(&ComboBoxState::default(), &layout);
        assert_eq!(popups.len(), 1);
        let popup = &popups[0];
        assert_eq!(popup.key, DROPDOWN);
        assert_eq!(popup.anchor, Rect::new(0.0, 0.0, 20.0, 1.0), "box-local");
        assert_eq!(popup.size, Size::new(20.0, 3.0), "one row per match");
        assert_eq!(popup.content.id(), Some(&c.dropdown_id()));
        assert_eq!(
            popup.resolve(AREA, Rect::new(0.0, 0.0, 80.0, 24.0)),
            Rect::new(2.0, 2.0, 20.0, 3.0),
            "one row below the field"
        );

        assert!(combo(false)
            .popups(&ComboBoxState::default(), &layout_for(&combo(false)))
            .is_empty());
    }

    #[test]
    fn alt_down_and_a_closed_box_enter_ask_the_host_to_open_the_list() {
        let c = combo(false);
        let alt = Modifiers {
            alt: true,
            ..Modifiers::default()
        };
        let expected = Handled::Event(ComboBoxEvent::TextChanged {
            id: c.id,
            text: String::new(),
        });
        assert_eq!(
            dispatch(&c, &key(Key::Named(NamedKey::Down), alt)).0,
            expected
        );
        assert_eq!(
            dispatch(&c, &key(Key::Named(NamedKey::Down), Modifiers::default())).0,
            expected
        );
        assert_eq!(
            dispatch(&c, &key(Key::Named(NamedKey::Enter), Modifiers::default())).0,
            expected
        );
    }

    #[test]
    fn an_open_box_navigates_the_filtered_list_and_commits_on_enter() {
        let mut c = combo(true);
        assert_eq!(
            dispatch(&c, &key(Key::Named(NamedKey::Down), Modifiers::default())).0,
            Handled::Event(ComboBoxEvent::HighlightChanged { id: c.id, index: 0 })
        );
        c.highlighted = Some(1);
        assert_eq!(
            dispatch(&c, &key(Key::Named(NamedKey::Enter), Modifiers::default())).0,
            Handled::Event(ComboBoxEvent::Committed {
                id: c.id,
                value: "Apricot".into()
            })
        );
        assert_eq!(
            dispatch(&c, &key(Key::Named(NamedKey::Escape), Modifiers::default())).0,
            Handled::Event(ComboBoxEvent::Cancelled { id: c.id })
        );
    }

    #[test]
    fn a_typed_character_edits_the_text_and_jumps_the_highlight_by_typeahead() {
        let c = combo(true);
        let (handled, host) = dispatch(&c, &key(Key::Char('b'), Modifiers::default()));
        assert_eq!(
            handled,
            Handled::Event(ComboBoxEvent::TextChanged {
                id: c.id,
                text: "b".into()
            })
        );
        // "b" narrows the filter to ["Banana"], whose only index is 0 —
        // the highlight follows the edit rather than pointing into the
        // list that was just replaced.
        assert_eq!(
            host.emitted,
            vec![crate::UiEvent::Form(
                c.id,
                crate::FormEvent::ComboBoxHighlightChanged { id: c.id, index: 0 }
            )]
        );
    }

    #[test]
    fn typeahead_accumulates_across_keystrokes_within_the_debounce_window() {
        let c = combo(true);
        let layout = layout_for(&c);
        let mut state = ComboBoxState::default();
        let mut host = MockHost::default();
        let mut typed = c.clone();
        for ch in ['a', 'p', 'r'] {
            let mut cx = EventCx::new(typed.id, &mut host);
            let handled = typed.handle(
                &mut state,
                &key(Key::Char(ch), Modifiers::default()),
                &layout,
                &mut cx,
            );
            let Handled::Event(ComboBoxEvent::TextChanged { text, .. }) = handled else {
                panic!("a typed character edits the text");
            };
            typed.text = text;
        }
        // "apr" filters to ["Apricot"]; the buffered "apr" prefix-matches
        // it at filtered index 0.
        assert_eq!(typed.text, "apr");
        assert_eq!(typed.filtered_indices(), vec![1]);
        assert_eq!(
            host.emitted.last(),
            Some(&crate::UiEvent::Form(
                typed.id,
                crate::FormEvent::ComboBoxHighlightChanged {
                    id: typed.id,
                    index: 0
                }
            ))
        );
    }

    #[test]
    fn a_press_on_a_dropdown_row_commits_that_option() {
        let c = combo(true);
        let on_row = |sub: Option<SubTarget>| RoutedEvent::PopupEvent {
            popup: DROPDOWN,
            event: Box::new(RoutedEvent::Pointer {
                kind: PointerKind::Down(crate::MouseButton::Left),
                sub,
                local: Point::new(1.0, 2.0),
                mods: Modifiers::default(),
            }),
        };
        assert_eq!(
            dispatch(&c, &on_row(Some(SubTarget(2)))).0,
            Handled::Event(ComboBoxEvent::Committed {
                id: c.id,
                value: "Banana".into()
            })
        );
        assert_eq!(
            dispatch(&c, &on_row(None)).0,
            Handled::Consumed,
            "a press between rows is still swallowed"
        );
    }

    #[test]
    fn a_dismissed_dropdown_cancels_the_edit() {
        for reason in [DismissReason::OutsideClick, DismissReason::Escape] {
            let dismissed = RoutedEvent::PopupDismissed {
                popup: DROPDOWN,
                reason,
            };
            assert_eq!(
                dispatch(&combo(true), &dismissed).0,
                Handled::Event(ComboBoxEvent::Cancelled {
                    id: WidgetId::new("form:fruit")
                })
            );
        }
    }

    #[test]
    fn the_dropdown_names_each_visible_row_by_its_filtered_index() {
        let mut c = combo(true);
        c.text = "ap".into();
        let content = c.dropdown();
        let ctx = LayoutCtx::new(1.0, 1.0, false);
        let layout = Primitive::layout(&content, &ctx, Rect::new(2.0, 2.0, 20.0, 2.0));
        assert_eq!(
            content.sub_regions(&layout),
            vec![
                (Rect::new(0.0, 0.0, 20.0, 1.0), SubTarget(0)),
                (Rect::new(0.0, 1.0, 20.0, 1.0), SubTarget(1)),
            ]
        );
    }

    #[test]
    fn a_click_on_the_field_focuses_it_and_opens_a_closed_list() {
        let c = combo(false);
        let (handled, host) = dispatch(&c, &pointer_down(1.0, 0.0));
        assert_eq!(
            handled,
            Handled::Event(ComboBoxEvent::TextChanged {
                id: c.id,
                text: String::new()
            })
        );
        assert_eq!(host.focus_requests, vec![None]);

        // Already open: the click only focuses.
        let (handled, _) = dispatch(&combo(true), &pointer_down(1.0, 0.0));
        assert_eq!(handled, Handled::Consumed);
    }

    #[test]
    fn the_a11y_click_action_commits_when_open_and_opens_when_closed() {
        let mut open = combo(true);
        open.highlighted = Some(0);
        assert_eq!(
            dispatch(&open, &RoutedEvent::Action(A11yAction::Click)).0,
            Handled::Event(ComboBoxEvent::Committed {
                id: open.id,
                value: "Apple".into()
            })
        );
        assert_eq!(
            dispatch(&combo(false), &RoutedEvent::Action(A11yAction::Click)).0,
            Handled::Event(ComboBoxEvent::TextChanged {
                id: open.id,
                text: String::new()
            })
        );
        assert_eq!(
            combo(false).a11y_actions(&ComboBoxState::default()),
            vec![A11yAction::Click]
        );
    }

    #[test]
    fn the_box_names_only_its_field_as_a_sub_region() {
        let c = combo(true);
        let layout = layout_for(&c);
        assert_eq!(
            c.sub_regions(&layout),
            vec![(Rect::new(0.0, 0.0, 20.0, 1.0), SubTarget(SUB_FIELD))]
        );
    }

    #[test]
    fn every_outcome_rides_the_form_event_channel_under_the_boxes_id() {
        let id = WidgetId::new("form:fruit");
        assert_eq!(
            ComboBoxEvent::Committed {
                id,
                value: "Apple".into()
            }
            .into_ui_event(id),
            crate::UiEvent::Form(
                id,
                crate::FormEvent::ComboBoxCommitted {
                    id,
                    value: "Apple".into()
                }
            )
        );
        assert_eq!(
            ComboBoxEvent::Cancelled { id }.into_ui_event(id),
            crate::UiEvent::Form(id, crate::FormEvent::ComboBoxCancelled { id })
        );
    }

    #[test]
    fn measure_reports_the_widest_option_plus_chrome_by_one_row() {
        let ctx = LayoutCtx::new(1.0, 1.0, false);
        let closed = Primitive::measure(&combo(false), &ctx, Size::new(40.0, 10.0));
        // "Apricot" is seven columns wide, plus two borders and the pad.
        assert_eq!(closed.width, Some(7.0 + 3.0));
        assert_eq!(closed.height, Some(1.0));

        let open = Primitive::measure(&combo(true), &ctx, Size::new(40.0, 10.0));
        assert_eq!(open.height, Some(1.0), "the open list is a popup");
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

    pub fn draw_field(buf: &mut Buffer, combo_box: &ComboBox, field: Rect, theme: &Theme) {
        let mut sink = None;
        cell_paint::field(combo_box, field, &mut surface(buf, &mut sink), theme);
    }

    pub fn draw_dropdown(buf: &mut Buffer, combo_box: &ComboBox, rect: Rect, theme: &Theme) {
        let content = combo_box.dropdown();
        let layout = Primitive::layout(&content, &LayoutCtx::new(1.0, 1.0, false), rect);
        let mut sink = None;
        cell_paint::dropdown(&content, &layout, &mut surface(buf, &mut sink), theme);
    }
}

#[cfg(all(test, feature = "gtk"))]
mod gtk_tests {
    use crate::backend::DrawsPrimitives;
    use crate::gtk::testing::GtkDriver;
    use crate::runner::{AppLogic, Reaction};
    use crate::{
        Backend, ComboBox, ComboBoxEvent, FormEvent, FrameTree, NamedKey, Rect, UiEvent, WidgetId,
    };

    const FIELD_BOUNDS: Rect = Rect {
        x: 0.0,
        y: 3.0,
        width: 200.0,
        height: 24.0,
    };

    /// Minimal `AppLogic` wrapping a single focused `ComboBox`, painted
    /// through the real `DrawsPrimitives::draw_dyn` — used only by this
    /// driver test, routed through `GtkDriver`.
    struct ComboBoxApp {
        combo: ComboBox,
        events: Vec<String>,
    }

    impl ComboBoxApp {
        fn new() -> Self {
            let mut combo = ComboBox::new(
                WidgetId::new("test:fruit"),
                vec![
                    "Apple".to_string(),
                    "Apricot".to_string(),
                    "Banana".to_string(),
                ],
            );
            combo.focused = true;
            Self {
                combo,
                events: Vec::new(),
            }
        }

        fn apply(&mut self, event: ComboBoxEvent) {
            match event {
                ComboBoxEvent::TextChanged { text, .. } => {
                    self.events.push(format!("TextChanged({text})"));
                    self.combo.text = text;
                    self.combo.open = true;
                }
                ComboBoxEvent::HighlightChanged { index, .. } => {
                    self.events.push(format!("HighlightChanged({index})"));
                    self.combo.highlighted = Some(index);
                }
                ComboBoxEvent::Committed { value, .. } => {
                    self.events.push(format!("Committed({value})"));
                    self.combo.committed = value.clone();
                    self.combo.text = value;
                    self.combo.open = false;
                    self.combo.highlighted = None;
                }
                ComboBoxEvent::Cancelled { .. } => {
                    self.events.push("Cancelled".to_string());
                    self.combo.text = self.combo.committed.clone();
                    self.combo.open = false;
                    self.combo.highlighted = None;
                }
            }
        }
    }

    impl AppLogic for ComboBoxApp {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            backend.draw_dyn(FIELD_BOUNDS, &self.combo, &());
        }

        /// The box is routed through the widget-contract engine, so a
        /// press on its open dropdown reaches it as a popup event.
        fn frame_tree(&self, _area: ()) -> FrameTree {
            let mut tree = FrameTree::new();
            tree.push(self.combo.id, FIELD_BOUNDS, Box::new(self.combo.clone()));
            tree
        }

        fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
            match event {
                UiEvent::KeyPressed { key, modifiers, .. } => {
                    if let Some(e) = self.combo.handle_key(&key, &modifiers) {
                        self.apply(e);
                    }
                    Reaction::Redraw
                }
                UiEvent::Form(_, FormEvent::ComboBoxCommitted { id, value }) => {
                    self.apply(ComboBoxEvent::Committed { id, value });
                    Reaction::Redraw
                }
                UiEvent::Form(_, FormEvent::ComboBoxCancelled { id }) => {
                    self.apply(ComboBoxEvent::Cancelled { id });
                    Reaction::Redraw
                }
                _ => Reaction::Continue,
            }
        }
    }

    #[test]
    fn typed_characters_filter_and_open_the_dropdown() {
        let mut driver = GtkDriver::new(ComboBoxApp::new(), 80, 40);
        driver.type_char('a');
        driver.type_char('p');
        assert_eq!(driver.app().combo.text, "ap");
        assert!(driver.app().combo.open);
        assert_eq!(driver.app().combo.filtered_indices(), vec![0, 1]);
    }

    #[test]
    fn down_navigates_the_filtered_list_with_wraparound() {
        let mut driver = GtkDriver::new(ComboBoxApp::new(), 80, 40);
        driver.type_char('a');
        driver.type_char('p');

        driver.press_named(NamedKey::Down);
        assert_eq!(driver.app().combo.highlighted, Some(0));
        driver.press_named(NamedKey::Down);
        assert_eq!(driver.app().combo.highlighted, Some(1));
        driver.press_named(NamedKey::Down);
        assert_eq!(driver.app().combo.highlighted, Some(0));
    }

    #[test]
    fn enter_commits_the_highlighted_option() {
        let mut driver = GtkDriver::new(ComboBoxApp::new(), 80, 40);
        driver.type_char('a');
        driver.type_char('p');
        driver.press_named(NamedKey::Down);
        driver.press_named(NamedKey::Down); // highlight index 1: "Apricot"

        driver.press_named(NamedKey::Enter);
        assert_eq!(driver.app().combo.committed, "Apricot");
        assert_eq!(driver.app().combo.text, "Apricot");
        assert!(!driver.app().combo.open);
    }

    #[test]
    fn enter_commits_typed_text_verbatim_when_nothing_is_highlighted() {
        let mut driver = GtkDriver::new(ComboBoxApp::new(), 80, 40);
        driver.type_char('C');
        driver.type_char('a');

        driver.press_named(NamedKey::Enter);
        assert_eq!(driver.app().combo.committed, "Ca");
        assert_eq!(driver.app().combo.text, "Ca");
    }

    #[test]
    fn escape_cancels_and_reverts_text_to_the_committed_value() {
        let mut driver = GtkDriver::new(ComboBoxApp::new(), 80, 40);
        driver.type_char('a');
        driver.press_named(NamedKey::Enter); // commit "a" verbatim
        assert_eq!(driver.app().combo.committed, "a");

        driver.type_char('p');
        assert_eq!(driver.app().combo.text, "ap");

        driver.press_named(NamedKey::Escape);
        assert_eq!(driver.app().combo.text, "a");
        assert_eq!(driver.app().combo.committed, "a");
        assert_eq!(driver.app().events.last().unwrap(), "Cancelled");
    }

    #[test]
    fn mouse_click_on_a_dropdown_item_commits_without_requiring_focus() {
        let mut driver = GtkDriver::new(ComboBoxApp::new(), 400, 300);
        driver.type_char('a');
        driver.type_char('p');
        assert!(driver.app().combo.open);

        let dropdown = driver.app().combo.dropdown_id();
        let (x, y) =
            crate::primitive::painted_sub_center(&dropdown, crate::interactive::SubTarget(0))
                .expect("the open dropdown paints its first row");
        assert!(y > FIELD_BOUNDS.y, "the list hangs below the field");
        driver.click(x, y);
        assert_eq!(driver.app().combo.committed, "Apple");
        assert_eq!(driver.app().combo.text, "Apple");
        assert!(!driver.app().combo.open);
    }
}

#[cfg(all(test, feature = "win", target_os = "windows"))]
mod win_tests {
    use crate::backend::DrawsPrimitives;
    use crate::runner::{AppLogic, Reaction};
    use crate::win::testing::WinDriver;
    use crate::{
        Backend, ComboBox, ComboBoxEvent, FormEvent, FrameTree, NamedKey, Rect, UiEvent, WidgetId,
    };

    const FIELD_BOUNDS: Rect = Rect {
        x: 0.0,
        y: 3.0,
        width: 200.0,
        height: 24.0,
    };

    /// Minimal `AppLogic` wrapping a single focused `ComboBox`, painted
    /// through the real `DrawsPrimitives::draw_dyn` — used only by this
    /// driver test, mirroring `gtk::combo_box`'s `ComboBoxApp` but routed
    /// through `WinDriver`.
    struct ComboBoxApp {
        combo: ComboBox,
        events: Vec<String>,
    }

    impl ComboBoxApp {
        fn new() -> Self {
            let mut combo = ComboBox::new(
                WidgetId::new("test:fruit"),
                vec![
                    "Apple".to_string(),
                    "Apricot".to_string(),
                    "Banana".to_string(),
                ],
            );
            combo.focused = true;
            Self {
                combo,
                events: Vec::new(),
            }
        }

        fn apply(&mut self, event: ComboBoxEvent) {
            match event {
                ComboBoxEvent::TextChanged { text, .. } => {
                    self.events.push(format!("TextChanged({text})"));
                    self.combo.text = text;
                    self.combo.open = true;
                }
                ComboBoxEvent::HighlightChanged { index, .. } => {
                    self.events.push(format!("HighlightChanged({index})"));
                    self.combo.highlighted = Some(index);
                }
                ComboBoxEvent::Committed { value, .. } => {
                    self.events.push(format!("Committed({value})"));
                    self.combo.committed = value.clone();
                    self.combo.text = value;
                    self.combo.open = false;
                    self.combo.highlighted = None;
                }
                ComboBoxEvent::Cancelled { .. } => {
                    self.events.push("Cancelled".to_string());
                    self.combo.text = self.combo.committed.clone();
                    self.combo.open = false;
                    self.combo.highlighted = None;
                }
            }
        }
    }

    impl AppLogic for ComboBoxApp {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            backend.draw_dyn(FIELD_BOUNDS, &self.combo, &());
        }

        /// The box is routed through the widget-contract engine, so a
        /// press on its open dropdown reaches it as a popup event.
        fn frame_tree(&self, _area: ()) -> FrameTree {
            let mut tree = FrameTree::new();
            tree.push(self.combo.id, FIELD_BOUNDS, Box::new(self.combo.clone()));
            tree
        }

        fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
            match event {
                UiEvent::KeyPressed { key, modifiers, .. } => {
                    if let Some(e) = self.combo.handle_key(&key, &modifiers) {
                        self.apply(e);
                    }
                    Reaction::Redraw
                }
                UiEvent::Form(_, FormEvent::ComboBoxCommitted { id, value }) => {
                    self.apply(ComboBoxEvent::Committed { id, value });
                    Reaction::Redraw
                }
                UiEvent::Form(_, FormEvent::ComboBoxCancelled { id }) => {
                    self.apply(ComboBoxEvent::Cancelled { id });
                    Reaction::Redraw
                }
                _ => Reaction::Continue,
            }
        }
    }

    #[test]
    fn typed_characters_filter_and_open_the_dropdown() {
        let mut driver = WinDriver::new(ComboBoxApp::new(), 80, 40);
        driver.type_char('a');
        driver.type_char('p');
        assert_eq!(driver.app().combo.text, "ap");
        assert!(driver.app().combo.open);
        assert_eq!(driver.app().combo.filtered_indices(), vec![0, 1]);
    }

    #[test]
    fn down_navigates_the_filtered_list_with_wraparound() {
        let mut driver = WinDriver::new(ComboBoxApp::new(), 80, 40);
        driver.type_char('a');
        driver.type_char('p');

        driver.press_named(NamedKey::Down);
        assert_eq!(driver.app().combo.highlighted, Some(0));
        driver.press_named(NamedKey::Down);
        assert_eq!(driver.app().combo.highlighted, Some(1));
        driver.press_named(NamedKey::Down);
        assert_eq!(driver.app().combo.highlighted, Some(0));
    }

    #[test]
    fn enter_commits_the_highlighted_option() {
        let mut driver = WinDriver::new(ComboBoxApp::new(), 80, 40);
        driver.type_char('a');
        driver.type_char('p');
        driver.press_named(NamedKey::Down);
        driver.press_named(NamedKey::Down); // highlight index 1: "Apricot"

        driver.press_named(NamedKey::Enter);
        assert_eq!(driver.app().combo.committed, "Apricot");
        assert_eq!(driver.app().combo.text, "Apricot");
        assert!(!driver.app().combo.open);
    }

    #[test]
    fn enter_commits_typed_text_verbatim_when_nothing_is_highlighted() {
        let mut driver = WinDriver::new(ComboBoxApp::new(), 80, 40);
        driver.type_char('C');
        driver.type_char('a');

        driver.press_named(NamedKey::Enter);
        assert_eq!(driver.app().combo.committed, "Ca");
        assert_eq!(driver.app().combo.text, "Ca");
    }

    #[test]
    fn escape_cancels_and_reverts_text_to_the_committed_value() {
        let mut driver = WinDriver::new(ComboBoxApp::new(), 80, 40);
        driver.type_char('a');
        driver.press_named(NamedKey::Enter); // commit "a" verbatim
        assert_eq!(driver.app().combo.committed, "a");

        driver.type_char('p');
        assert_eq!(driver.app().combo.text, "ap");

        driver.press_named(NamedKey::Escape);
        assert_eq!(driver.app().combo.text, "a");
        assert_eq!(driver.app().combo.committed, "a");
        assert_eq!(driver.app().events.last().unwrap(), "Cancelled");
    }

    #[test]
    fn mouse_click_on_a_dropdown_item_commits_without_requiring_focus() {
        let mut driver = WinDriver::new(ComboBoxApp::new(), 400, 300);
        driver.type_char('a');
        driver.type_char('p');
        assert!(driver.app().combo.open);

        let dropdown = driver.app().combo.dropdown_id();
        let (x, y) =
            crate::primitive::painted_sub_center(&dropdown, crate::interactive::SubTarget(0))
                .expect("the open dropdown paints its first row");
        assert!(y > FIELD_BOUNDS.y, "the list hangs below the field");
        driver.click(x, y);
        assert_eq!(driver.app().combo.committed, "Apple");
        assert_eq!(driver.app().combo.text, "Apple");
        assert!(!driver.app().combo.open);
    }
}
