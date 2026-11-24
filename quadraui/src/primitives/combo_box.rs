//! `ComboBox` primitive: a free-text field backed by a fixed list of
//! options, with a type-ahead filtered dropdown — layout, hit-test,
//! keyboard handling, validation/commit/cancel events, and both
//! rasterisers.
//!
//! The whole widget contract lives here: [`Primitive`] resolves the
//! layout and paints it ([`native_surface_paint`] on a pixel surface,
//! [`cell_paint`] on a cell grid), and [`Interactive`] answers pointer,
//! keyboard and assistive-technology input. A host reaches all of it
//! type-erased through [`crate::frame::Surface::Widget`].
//!
//! Additive alongside [`crate::TextInput`], which is untouched — a
//! `ComboBox` isn't a `TextInput` with a dropdown bolted on, it's its
//! own primitive with its own commit model.
//!
//! # Filtering and placement
//!
//! [`ComboBox::filtered_indices`] is a self-contained, case-insensitive
//! substring match over [`ComboBox::options`] (no shared filter utility
//! exists elsewhere in-crate to call into). [`ComboBox::layout`] reuses
//! [`crate::Completions`]'s placement algorithm directly: it builds a
//! transient [`crate::Completions`] value out of the filtered options
//! and delegates to [`crate::Completions::layout`] for the prefer-below
//! / flip-above / shift-left / clip-to-viewport popup math, embedding
//! the result as [`ComboBoxLayout::dropdown`].
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
//! # Focus and activation
//!
//! Like [`crate::Switch`], a `ComboBox` tracks its own
//! [`ComboBox::focused`] flag; [`ComboBox::handle_key`] is gated on
//! `focused` and not [`ComboBox::disabled`]. [`ComboBox::click`] (a
//! dropdown-item activation) is gated on `disabled` alone, matching
//! [`crate::Switch::click`]'s "a pointer click doesn't require prior
//! keyboard focus" contract.

use crate::a11y::A11yAction;
use crate::behaviour::nav::{FlatNav, NavKey};
use crate::behaviour::typeahead::Typeahead;
use crate::event::{Key, NamedKey, Point, Rect};
use crate::interactive::{
    EventCx, FocusSpec, Interactive, KeyEvent, PointerKind, RoutedEvent, SubTarget, WidgetAction,
};
use crate::primitive::{LayoutCtx, Primitive, Size, Units};
use crate::primitives::completions::{
    CompletionItem, CompletionItemMeasure, CompletionKind, Completions, CompletionsHit,
    CompletionsLayout,
};
use crate::types::{Modifiers, StyledText, WidgetId};
use crate::Handled;
use serde::{Deserialize, Serialize};

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

    /// Resolve the field's own bounds plus, when `open`, the dropdown
    /// popup's layout — built by handing the filtered options to a
    /// transient [`Completions`] value and delegating to its
    /// placement algorithm (prefer below the field, flip above on
    /// overflow, shift left on overflow, clip to viewport).
    pub fn layout<F>(
        &self,
        field_bounds: Rect,
        viewport: Rect,
        popup_width: f32,
        max_popup_height: f32,
        measure_item: F,
    ) -> ComboBoxLayout
    where
        F: Fn(usize) -> CompletionItemMeasure,
    {
        let filtered = self.filtered_indices();
        let dropdown = if self.open && !filtered.is_empty() {
            let completions = Completions {
                id: self.id,
                items: filtered
                    .iter()
                    .map(|&i| CompletionItem {
                        label: StyledText::plain(self.options[i].clone()),
                        detail: None,
                        documentation: None,
                        kind: CompletionKind::Text,
                        icon: None,
                    })
                    .collect(),
                selected_idx: self.highlighted.unwrap_or(0),
                scroll_offset: 0,
                has_focus: self.focused,
            };
            Some(completions.layout(
                field_bounds.x,
                field_bounds.y,
                field_bounds.height,
                viewport,
                popup_width,
                max_popup_height,
                measure_item,
            ))
        } else {
            None
        };
        ComboBoxLayout {
            field_bounds,
            dropdown,
        }
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

    /// Pointer activation dispatcher: the host calls this after
    /// resolving a `MouseDown`/`MouseUp` position through
    /// [`ComboBoxLayout::hit_test`]. A hit on the field itself or
    /// outside the control produces no event — focusing the field is
    /// the host's job, same as elsewhere in this crate.
    pub fn click(&self, hit: ComboBoxHit) -> Option<ComboBoxEvent> {
        if self.disabled {
            return None;
        }
        match hit {
            ComboBoxHit::DropdownItem(index) => {
                let filtered = self.filtered_indices();
                let option_idx = *filtered.get(index)?;
                Some(ComboBoxEvent::Committed {
                    id: self.id,
                    value: self.options[option_idx].clone(),
                })
            }
            ComboBoxHit::Field | ComboBoxHit::Outside => None,
        }
    }
}

impl Default for ComboBox {
    /// An empty combo box with no options. See [`ComboBox::new`] for one
    /// with a real id/options list.
    fn default() -> Self {
        Self::new(WidgetId::new(""), Vec::new())
    }
}

/// Fully-resolved combo-box layout.
#[derive(Debug, Clone, PartialEq)]
pub struct ComboBoxLayout {
    pub field_bounds: Rect,
    /// `None` when the dropdown is closed or has no matches.
    pub dropdown: Option<CompletionsLayout>,
}

/// Classification of a hit-test result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComboBoxHit {
    Field,
    /// Index into the *filtered* list, not the raw `options` list.
    DropdownItem(usize),
    Outside,
}

impl ComboBoxLayout {
    pub fn hit_test(&self, p: Point) -> ComboBoxHit {
        if let Some(dropdown) = &self.dropdown {
            if let CompletionsHit::Item(idx) = dropdown.hit_test(p.x, p.y) {
                return ComboBoxHit::DropdownItem(idx);
            }
        }
        if self.field_bounds.contains(p) {
            ComboBoxHit::Field
        } else {
            ComboBoxHit::Outside
        }
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
    /// this primitive's four outcomes, one for one, with the same
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
/// `area` the box was laid out in, the row pitch its field and dropdown
/// rows were resolved at, and the box-local [`ComboBoxLayout`] itself.
///
/// `#[non_exhaustive]`: produced by [`Primitive::layout`], never built by
/// a consumer via struct literal — the same convention
/// [`ComboBoxLayout`] itself follows.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct ComboBoxPrimitiveLayout {
    /// The rect [`Primitive::layout`] was called with: the field on its
    /// first row, and the room the open dropdown drops into below.
    pub area: Rect,
    /// Row pitch the field and every dropdown row were resolved at, from
    /// [`crate::LayoutCtx::line_height`].
    pub row_height: f32,
    /// The resolved box layout. Its `field_bounds`/`dropdown` are in the
    /// same absolute space as [`Self::area`].
    pub layout: ComboBoxLayout,
}

impl ComboBoxPrimitiveLayout {
    /// Hit-test at a position relative to [`Self::area`]'s own origin —
    /// the space [`RoutedEvent::Pointer`]'s `local` field uses.
    pub fn hit_test_local(&self, x: f32, y: f32) -> ComboBoxHit {
        self.layout
            .hit_test(Point::new(self.area.x + x, self.area.y + y))
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
}

impl Primitive for ComboBox {
    type Layout = ComboBoxPrimitiveLayout;
    type Event = ComboBoxEvent;

    fn id(&self) -> Option<&WidgetId> {
        Some(&self.id)
    }

    /// The widest option plus the chrome a dropdown row frames it with,
    /// and one row for the field plus one per currently-filtered option
    /// while [`ComboBox::open`] — so a container that sizes this box to
    /// its content leaves the dropdown somewhere to drop into.
    fn measure(&self, ctx: &LayoutCtx, _avail: Size) -> crate::NaturalSize {
        let cells = matches!(ctx.units(), Units::Cells);
        // Two side borders plus the one-column pad a dropdown row insets
        // its label by.
        let chrome = if cells { 3.0 } else { 3.0 * ctx.char_width };
        let rows = if self.open {
            1.0 + self.filtered_indices().len() as f32
        } else {
            1.0
        };
        crate::NaturalSize::new(
            Some(self.widest_option(ctx) + chrome),
            Some(rows * ctx.line_height),
        )
    }

    /// `rect` is the whole box: the field occupies its first row and the
    /// open dropdown drops into the rest, clipped to `rect` as its own
    /// viewport. Placement itself is
    /// [`crate::Completions`]'s algorithm, reached through
    /// [`ComboBox::layout`] — the dropdown flips above the field when
    /// the room below it runs out, exactly as it does on the legacy
    /// `Backend::draw_combo_box` path.
    fn layout(&self, ctx: &LayoutCtx, rect: Rect) -> ComboBoxPrimitiveLayout {
        let row_height = if ctx.line_height > 0.0 {
            ctx.line_height
        } else {
            1.0
        };
        let field_height = row_height.min(rect.height);
        let field_bounds = Rect::new(rect.x, rect.y, rect.width, field_height);
        let max_popup_height = (rect.height - field_height).max(0.0);
        let layout = self.layout(field_bounds, rect, rect.width, max_popup_height, |_| {
            CompletionItemMeasure::new(row_height)
        });
        ComboBoxPrimitiveLayout {
            area: rect,
            row_height,
            layout,
        }
    }

    /// The shared pixel painter — the same
    /// [`native_surface_paint::paint`] every pixel backend's
    /// `draw_combo_box` wrapper calls.
    fn paint(
        &self,
        layout: &ComboBoxPrimitiveLayout,
        surface: &mut dyn crate::paint_surface::PaintSurface,
        theme: &crate::theme::Theme,
        _style: &crate::style::Style,
    ) {
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
    /// [`crate::tui::draw_combo_box`] because the `Buffer` →
    /// [`crate::CellSurface`] adapter is private to `crate::tui`.
    #[cfg(feature = "tui")]
    fn paint_cells(
        &self,
        layout: &ComboBoxPrimitiveLayout,
        buf: &mut ratatui::buffer::Buffer,
        theme: &crate::theme::Theme,
    ) {
        crate::tui::draw_combo_box(buf, self, &layout.layout, theme);
    }
}

/// [`ComboBox`]'s own interaction state: the debounced typeahead buffer
/// that lets a few quick keystrokes jump the dropdown highlight to the
/// best prefix match, plus the monotonic origin its 500ms debounce
/// window is measured from.
///
/// Everything else a `ComboBox` renders — `text`, `committed`, `open`,
/// `highlighted` — lives on the descriptor, which the host updates from
/// [`ComboBoxEvent`]; only the keystroke *timing* can't, since it is not
/// something a frame paints.
#[derive(Debug, Default)]
pub struct ComboBoxState {
    typeahead: Typeahead,
    origin: Option<std::time::Instant>,
}

impl ComboBoxState {
    /// Milliseconds since this state's first keystroke — the clock
    /// [`Typeahead::push`]'s debounce window reads. Seeded on first use,
    /// so a state that never sees a keystroke never reads a clock.
    fn now_ms(&mut self) -> u64 {
        let origin = *self.origin.get_or_insert_with(std::time::Instant::now);
        origin.elapsed().as_millis() as u64
    }
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
    /// alongside the four this primitive already has, and reusing this
    /// one keeps the standalone box and a `Form` row applying the same
    /// vocabulary.
    fn open_request(&self) -> Handled<ComboBoxEvent> {
        Handled::Event(ComboBoxEvent::TextChanged {
            id: self.id,
            text: self.text.clone(),
        })
    }
}

impl Interactive for ComboBox {
    type State = ComboBoxState;
    type Action = ComboBoxEvent;

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
            RoutedEvent::Pointer {
                kind: PointerKind::Down(crate::MouseButton::Left),
                local,
                ..
            } => match layout.hit_test_local(local.x, local.y) {
                // Choosing an option commits it, the same thing Enter
                // does to the highlighted one.
                ComboBoxHit::DropdownItem(index) => {
                    match self.click(ComboBoxHit::DropdownItem(index)) {
                        Some(event) => Handled::Event(event),
                        None => Handled::Ignored,
                    }
                }
                // Clicking the field focuses it and drops the list open,
                // the affordance that makes the closed box look clickable
                // at all.
                ComboBoxHit::Field => {
                    cx.request_focus(None);
                    if self.open {
                        Handled::Consumed
                    } else {
                        self.open_request()
                    }
                }
                ComboBoxHit::Outside => Handled::Ignored,
            },
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

    /// One sub-part per visible dropdown option, addressed by its index
    /// into the *filtered* list, then the field itself — most-specific
    /// first, so an option that overlaps the field's own row wins.
    fn sub_regions(&self, layout: &ComboBoxPrimitiveLayout) -> Vec<(Rect, SubTarget)> {
        let local =
            |r: Rect| Rect::new(r.x - layout.area.x, r.y - layout.area.y, r.width, r.height);
        let mut regions: Vec<(Rect, SubTarget)> = layout
            .layout
            .dropdown
            .iter()
            .flat_map(|dropdown| dropdown.visible_items.iter())
            .map(|vis| {
                (
                    local(vis.bounds),
                    SubTarget(SUB_OPTION_BASE + vis.item_idx as u64),
                )
            })
            .collect();
        regions.push((local(layout.layout.field_bounds), SubTarget(SUB_FIELD)));
        regions
    }
}

/// [`Interactive::sub_regions`]'s tag for the editable field row.
const SUB_FIELD: u64 = 0;
/// [`Interactive::sub_regions`]'s tag for dropdown option `n`, offset
/// past [`SUB_FIELD`] so the field's own tag stays `0`.
const SUB_OPTION_BASE: u64 = 1;

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
    ///   as a result, so it opens too.)
    /// - While open, `Up`/`Down` move the highlight through
    ///   [`crate::behaviour::nav::FlatNav`], `Enter` commits the
    ///   highlighted option and `Escape` cancels the edit — all of them
    ///   [`ComboBox::handle_key`]'s own answers.
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
            let now_ms = state.now_ms();
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

/// The cell-grid painter for [`ComboBox`], written against
/// [`crate::CellSurface`].
///
/// The field first — a box-drawing frame around [`ComboBox::text`], the
/// same chrome `crate::tui::text_input` paints — then, when
/// `layout.dropdown` is `Some`, the filtered option list beneath it in
/// the same row chrome `crate::primitives::completions::cell_paint`
/// paints: side `│` borders either edge of every row, and the
/// highlighted row's background tinted. Row labels resolve through
/// [`ComboBox::filtered_indices`] back into `combo_box.options`, the
/// same index chase [`native_surface_paint::paint`] does.
///
/// Colours: [`crate::Theme::input_bg`] washes the field,
/// [`crate::Theme::accent_fg`] replaces [`crate::Theme::border_fg`] on a
/// focused box and [`crate::Theme::muted_fg`] replaces
/// [`crate::Theme::surface_fg`] on a disabled one; the dropdown uses the
/// `completion_*` family. A dropdown narrower than three columns (two
/// borders plus one content column) paints no list.
#[cfg(feature = "tui")]
pub(crate) mod cell_paint {
    use super::{ComboBox, ComboBoxLayout};
    use crate::cell_surface::{draw_text_row, solid_style, stroke_square_frame, CellSurface};
    use crate::theme::Theme;

    /// Paint `combo_box` at its resolved `layout` onto `surface`.
    pub(crate) fn paint(
        combo_box: &ComboBox,
        layout: &ComboBoxLayout,
        surface: &mut dyn CellSurface,
        theme: &Theme,
    ) {
        let bg = theme.background;
        let field = layout.field_bounds;
        if field.width > 0.0 && field.height > 0.0 {
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
            stroke_square_frame(surface, field, solid_style(border, bg));
            if !combo_box.text.is_empty() {
                draw_text_row(surface, field, &combo_box.text, field_style);
            }
        }

        let Some(dropdown) = &layout.dropdown else {
            return;
        };
        if dropdown.bounds.width <= 0.0 || dropdown.bounds.height <= 0.0 {
            return;
        }

        let pop_bg = theme.completion_bg;
        let sel_bg = theme.completion_selected_bg;
        let pop_fg = theme.completion_fg;
        let border = theme.completion_border;

        let x = dropdown.bounds.x.round() as u16;
        let y = dropdown.bounds.y.round() as u16;
        let w = dropdown.bounds.width.round() as u16;
        if w < 3 {
            return;
        }

        let filtered = combo_box.filtered_indices();
        for visible in &dropdown.visible_items {
            let row_y = y + (visible.bounds.y - dropdown.bounds.y).round() as u16;
            let is_selected = combo_box.highlighted == Some(visible.item_idx);
            let row_bg = if is_selected { sel_bg } else { pop_bg };
            let row_style = solid_style(pop_fg, row_bg);

            surface.fill(
                crate::Rect::new(x as f32, row_y as f32, w as f32, 1.0),
                row_style,
            );
            let border_style = solid_style(border, pop_bg);
            surface.put_char(x, row_y, '│', border_style);
            surface.put_char(x + w - 1, row_y, '│', border_style);

            if let Some(&option_idx) = filtered.get(visible.item_idx) {
                // One leading pad column, and the trailing border column
                // the label must stop short of.
                surface.put_str_clipped(
                    x + 1,
                    row_y,
                    &format!(" {}", combo_box.options[option_idx]),
                    w - 2,
                    row_style,
                );
            }
        }
    }
}

/// Shared-paint executor for [`ComboBox`] — a rasteriser calls into
/// this (never re-deriving geometry or re-filtering `options` itself)
/// instead of walking the fields directly, matching every other
/// primitive's `native_surface_paint` module (see
/// `docs/PRIMITIVE_RULES.md`'s "Primitive-first" rule). Gated to the
/// backends that actually paint pixels — the TUI backend renders
/// `ComboBox` through its own cell-grid path instead.
#[cfg(any(
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
#[allow(dead_code)]
pub(crate) mod native_surface_paint {
    use super::{ComboBox, ComboBoxLayout};
    use crate::paint_surface::PaintSurface;
    use crate::theme::Theme;

    /// Paint a [`ComboBox`] onto `surface`: a bordered text field
    /// showing [`ComboBox::text`], plus — when `layout.dropdown` is
    /// `Some` — the filtered options popup, highlighting whichever
    /// filtered-list index [`ComboBox::highlighted`] names. Item
    /// labels aren't carried on [`crate::primitives::completions::VisibleCompletion`],
    /// so each visible item's `item_idx` (an index into the *filtered*
    /// list, per [`ComboBox::filtered_indices`]'s own contract) is
    /// mapped back through `combo_box.options` here rather than in the
    /// primitive's own layout code, mirroring how [`ComboBox::commit`]
    /// already does that same index chase.
    pub(crate) fn paint(
        combo_box: &ComboBox,
        layout: &ComboBoxLayout,
        surface: &mut dyn PaintSurface,
        theme: &Theme,
    ) {
        let field = layout.field_bounds;
        if field.width > 0.0 && field.height > 0.0 {
            let border = if combo_box.disabled {
                theme.border_fg
            } else if combo_box.focused {
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

        let Some(dropdown) = &layout.dropdown else {
            return;
        };
        if dropdown.bounds.width <= 0.0 || dropdown.bounds.height <= 0.0 {
            return;
        }
        surface.surface_fill_rect(dropdown.bounds, theme.completion_bg);
        surface.surface_stroke_rect(dropdown.bounds, theme.completion_border, 1.0);

        let filtered = combo_box.filtered_indices();
        for visible in &dropdown.visible_items {
            if combo_box.highlighted == Some(visible.item_idx) {
                surface.surface_fill_rect(visible.bounds, theme.completion_selected_bg);
            }
            if let Some(&option_idx) = filtered.get(visible.item_idx) {
                surface.surface_draw_text_run(
                    visible.bounds,
                    &combo_box.options[option_idx],
                    theme.completion_fg,
                );
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::backend::ImagePaintResult;
        use crate::event::{Point, Rect, Viewport};
        use crate::primitives::completions::CompletionItemMeasure;
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

        fn options() -> Vec<String> {
            vec!["Apple".into(), "Apricot".into(), "Banana".into()]
        }

        fn field_bounds() -> Rect {
            Rect::new(0.0, 0.0, 20.0, 1.0)
        }

        fn viewport() -> Rect {
            Rect::new(0.0, 0.0, 80.0, 24.0)
        }

        #[test]
        fn paints_field_background_border_and_text() {
            let mut c = ComboBox::new(WidgetId::new("form:fruit"), options());
            c.text = "Cher".into();
            let layout = c.layout(field_bounds(), viewport(), 20.0, 10.0, |_| {
                CompletionItemMeasure::new(1.0)
            });
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            paint(&c, &layout, &mut surface, &theme);

            assert_eq!(surface.fills[0], (field_bounds(), theme.input_bg));
            assert!(surface.strokes.contains(&(field_bounds(), theme.border_fg)));
            assert_eq!(surface.text_runs[0].1, "Cher");
        }

        #[test]
        fn closed_dropdown_paints_no_popup_chrome() {
            let c = ComboBox::new(WidgetId::new("form:fruit"), options());
            let layout = c.layout(field_bounds(), viewport(), 20.0, 10.0, |_| {
                CompletionItemMeasure::new(1.0)
            });
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            paint(&c, &layout, &mut surface, &theme);

            assert!(!surface
                .fills
                .iter()
                .any(|(_, color)| *color == theme.completion_bg));
        }

        #[test]
        fn open_dropdown_paints_every_filtered_option_label() {
            let mut c = ComboBox::new(WidgetId::new("form:fruit"), options());
            c.open = true;
            c.text = "ap".into(); // filtered: Apple, Apricot
            let layout = c.layout(field_bounds(), viewport(), 20.0, 10.0, |_| {
                CompletionItemMeasure::new(1.0)
            });
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            paint(&c, &layout, &mut surface, &theme);

            assert!(surface
                .fills
                .iter()
                .any(|(_, color)| *color == theme.completion_bg));
            let labels: Vec<&str> = surface
                .text_runs
                .iter()
                .map(|(_, text, _)| text.as_str())
                .collect();
            assert!(labels.contains(&"Apple"));
            assert!(labels.contains(&"Apricot"));
            assert!(!labels.contains(&"Banana"));
        }

        #[test]
        fn highlighted_item_gets_a_selection_fill() {
            let mut c = ComboBox::new(WidgetId::new("form:fruit"), options());
            c.open = true;
            c.text = "ap".into(); // filtered: [0 Apple, 1 Apricot]
            c.highlighted = Some(1);
            let layout = c.layout(field_bounds(), viewport(), 20.0, 10.0, |_| {
                CompletionItemMeasure::new(1.0)
            });
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            paint(&c, &layout, &mut surface, &theme);

            let highlighted_bounds = layout.dropdown.as_ref().unwrap().visible_items[1].bounds;
            assert!(surface
                .fills
                .contains(&(highlighted_bounds, theme.completion_selected_bg)));
            let unhighlighted_bounds = layout.dropdown.as_ref().unwrap().visible_items[0].bounds;
            assert!(!surface
                .fills
                .contains(&(unhighlighted_bounds, theme.completion_selected_bg)));
        }

        #[test]
        fn zero_size_field_paints_nothing_for_the_field() {
            let c = ComboBox::new(WidgetId::new("form:fruit"), options());
            let layout = c.layout(
                Rect::new(0.0, 0.0, 0.0, 0.0),
                viewport(),
                20.0,
                10.0,
                |_| CompletionItemMeasure::new(1.0),
            );
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            paint(&c, &layout, &mut surface, &theme);
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
    fn click_on_dropdown_item_commits_without_requiring_focus() {
        let mut c = combo();
        c.text = "ap".into(); // filtered: [0, 1]
        assert_eq!(
            c.click(ComboBoxHit::DropdownItem(1)),
            Some(ComboBoxEvent::Committed {
                id: c.id.clone(),
                value: "Apricot".into()
            })
        );
        assert_eq!(c.click(ComboBoxHit::Field), None);
        assert_eq!(c.click(ComboBoxHit::Outside), None);
    }

    #[test]
    fn click_is_inert_when_disabled() {
        let mut c = combo();
        c.disabled = true;
        assert_eq!(c.click(ComboBoxHit::DropdownItem(0)), None);
    }

    #[test]
    fn layout_has_no_dropdown_when_closed_and_a_dropdown_when_open() {
        let c = combo();
        let field_bounds = Rect::new(0.0, 0.0, 20.0, 1.0);
        let viewport = Rect::new(0.0, 0.0, 80.0, 24.0);

        let closed = c.layout(field_bounds, viewport, 20.0, 10.0, |_| {
            CompletionItemMeasure::new(1.0)
        });
        assert!(closed.dropdown.is_none());
        assert_eq!(closed.hit_test(Point::new(5.0, 0.0)), ComboBoxHit::Field);

        let mut open = c;
        open.open = true;
        let layout = open.layout(field_bounds, viewport, 20.0, 10.0, |_| {
            CompletionItemMeasure::new(1.0)
        });
        assert!(layout.dropdown.is_some());
        // The dropdown is placed just below the field (field height 1.0).
        assert_eq!(
            layout.hit_test(Point::new(1.0, 1.0)),
            ComboBoxHit::DropdownItem(0)
        );
    }

    #[test]
    fn layout_has_no_dropdown_when_open_but_no_matches() {
        let mut c = combo();
        c.open = true;
        c.text = "zzz".into();
        let field_bounds = Rect::new(0.0, 0.0, 20.0, 1.0);
        let viewport = Rect::new(0.0, 0.0, 80.0, 24.0);
        let layout = c.layout(field_bounds, viewport, 20.0, 10.0, |_| {
            CompletionItemMeasure::new(1.0)
        });
        assert!(layout.dropdown.is_none());
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

    // ── Cell-path paint (`cell_paint::paint`) ───────────────────────────

    #[cfg(feature = "tui")]
    mod cells {
        use super::*;
        use crate::theme::Theme;
        use crate::tui::draw_combo_box;
        use ratatui::buffer::Buffer;
        use ratatui::layout::Rect as RRect;

        fn cell_char(buf: &Buffer, x: u16, y: u16) -> char {
            buf[(x, y)].symbol().chars().next().unwrap_or(' ')
        }

        fn mk_combo_box() -> ComboBox {
            ComboBox {
                id: WidgetId::new("combo"),
                options: vec!["alpha".to_string(), "beta".to_string(), "gamma".to_string()],
                text: String::new(),
                committed: String::new(),
                open: true,
                highlighted: Some(0),
                disabled: false,
                focused: true,
            }
        }

        fn measure(_idx: usize) -> CompletionItemMeasure {
            CompletionItemMeasure { height: 1.0 }
        }

        fn paint_and_click_round_trip_at(origin_x: u16, origin_y: u16) {
            let field_bounds = Rect::new(origin_x as f32, origin_y as f32, 20.0, 1.0);
            let viewport = Rect::new(0.0, 0.0, 80.0, 24.0);
            let combo_box = mk_combo_box();
            let layout = combo_box.layout(field_bounds, viewport, 20.0, 10.0, measure);

            let rect = RRect::new(0, 0, 80, 24);
            let mut buf = Buffer::empty(rect);
            draw_combo_box(&mut buf, &combo_box, &layout, &Theme::default());

            // Field border top-left corner paints at the absolute origin.
            assert_eq!(cell_char(&buf, origin_x, origin_y), '┌');

            let dropdown = layout.dropdown.as_ref().expect("dropdown should be open");
            let first_item = dropdown
                .visible_items
                .first()
                .expect("at least one filtered option");
            let row_y = first_item.bounds.y as u16;
            let row_x = dropdown.bounds.x as u16;

            // First option label ("alpha") renders inside the dropdown
            // row, after the left border + 1-cell pad.
            assert_eq!(cell_char(&buf, row_x + 2, row_y), 'a');

            // hit_test: a click on the first dropdown row resolves to the
            // DropdownItem variant for that filtered index.
            let p = Point::new(
                first_item.bounds.x + 1.0,
                first_item.bounds.y + first_item.bounds.height / 2.0,
            );
            assert_eq!(layout.hit_test(p), ComboBoxHit::DropdownItem(0));
        }

        #[test]
        fn paint_and_click_round_trip() {
            paint_and_click_round_trip_at(0, 0);
        }

        /// Paint and hit-test must work correctly when the control is not
        /// at the screen origin.
        #[test]
        fn paint_and_click_round_trip_at_nonzero_origin() {
            paint_and_click_round_trip_at(7, 3);
        }

        #[test]
        fn the_dropdown_frames_every_row_with_side_borders() {
            let c = mk_combo_box();
            let layout = c.layout(
                Rect::new(0.0, 0.0, 20.0, 1.0),
                Rect::new(0.0, 0.0, 80.0, 24.0),
                20.0,
                10.0,
                measure,
            );
            let mut buf = Buffer::empty(RRect::new(0, 0, 80, 24));
            draw_combo_box(&mut buf, &c, &layout, &Theme::default());

            let dropdown = layout.dropdown.as_ref().expect("open");
            let w = dropdown.bounds.width.round() as u16;
            for visible in &dropdown.visible_items {
                let y = visible.bounds.y as u16;
                let x = dropdown.bounds.x as u16;
                assert_eq!(cell_char(&buf, x, y), '│');
                assert_eq!(cell_char(&buf, x + w - 1, y), '│');
            }
        }

        #[test]
        fn a_closed_dropdown_paints_only_the_field() {
            let mut c = mk_combo_box();
            c.open = false;
            c.text = "alpha".into();
            let layout = c.layout(
                Rect::new(0.0, 0.0, 20.0, 1.0),
                Rect::new(0.0, 0.0, 80.0, 24.0),
                20.0,
                10.0,
                measure,
            );
            let mut buf = Buffer::empty(RRect::new(0, 0, 80, 24));
            draw_combo_box(&mut buf, &c, &layout, &Theme::default());

            assert_eq!(cell_char(&buf, 0, 0), 'a', "text paints over the frame");
            // Nothing lands on the row the dropdown would have used.
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
    use crate::interactive::EventHost;
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
    fn the_layout_drops_the_dropdown_below_the_field_row() {
        let layout = layout_for(&combo(true));
        assert_eq!(layout.row_height, 1.0);
        assert_eq!(layout.layout.field_bounds, Rect::new(2.0, 1.0, 20.0, 1.0));
        let dropdown = layout.layout.dropdown.as_ref().expect("open");
        assert_eq!(dropdown.bounds.y, 2.0, "one row below the field");
        assert_eq!(dropdown.visible_items.len(), 3);
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
    fn a_click_on_a_dropdown_row_commits_that_option() {
        let c = combo(true);
        let layout = layout_for(&c);
        let row = layout.layout.dropdown.as_ref().expect("open").visible_items[2].bounds;
        let local_x = row.x + 1.0 - layout.area.x;
        let local_y = row.y + row.height / 2.0 - layout.area.y;
        assert_eq!(
            dispatch(&c, &pointer_down(local_x, local_y)).0,
            Handled::Event(ComboBoxEvent::Committed {
                id: c.id,
                value: "Banana".into()
            })
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
    fn sub_regions_name_every_visible_option_before_the_field() {
        let c = combo(true);
        let layout = layout_for(&c);
        let regions = c.sub_regions(&layout);
        assert_eq!(
            regions.iter().map(|(_, s)| s.0).collect::<Vec<_>>(),
            vec![
                SUB_OPTION_BASE,
                SUB_OPTION_BASE + 1,
                SUB_OPTION_BASE + 2,
                SUB_FIELD
            ]
        );
        for (rect, _) in &regions {
            assert!(rect.x >= 0.0 && rect.y >= 0.0, "{rect:?} is not local");
        }
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
    fn measure_reports_the_widest_option_plus_chrome_and_one_row_per_match() {
        let ctx = LayoutCtx::new(1.0, 1.0, false);
        let closed = Primitive::measure(&combo(false), &ctx, Size::new(40.0, 10.0));
        // "Apricot" is seven columns wide, plus two borders and the pad.
        assert_eq!(closed.width, Some(7.0 + 3.0));
        assert_eq!(closed.height, Some(1.0));

        let open = Primitive::measure(&combo(true), &ctx, Size::new(40.0, 10.0));
        assert_eq!(open.height, Some(4.0), "field row plus three matches");
    }
}
