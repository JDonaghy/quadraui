//! The **behaviour** half of the widget contract: [`Interactive`] and its
//! supporting types.
//!
//! [`crate::primitive::Primitive`] is the *paint* half — `layout()` +
//! `paint()`, resolved once per frame and handed to a backend. `Interactive`
//! is the other half every widget needs: what happens when a
//! [`RoutedEvent`] reaches it — focus, hover/pressed, keyboard navigation,
//! hit-testing and the semantic event(s) it produces.
//!
//! # Why a separate trait instead of folding this into `Primitive`
//!
//! A widget's paint shape (`Layout`) and its behaviour shape (`State`,
//! `Action`) vary independently — a `Primitive` with zero behaviour (a
//! `Progress` bar, a static `Divider`) still needs `layout()`/`paint()` but
//! has nothing to `handle()`. [`non_interactive!`] is the one-line opt-out
//! for exactly that case.
//!
//! # What's here
//!
//! - [`RoutedEvent`] — the one event shape [`Interactive::handle`] receives,
//!   already resolved against this widget's own layout (local coordinates,
//!   which [`SubTarget`] if any) by the runtime that calls it.
//! - [`EventHost`] / [`EventCx`] — the narrow seam a widget's `handle` uses
//!   to ask for focus, capture the pointer, change the cursor, or emit a
//!   [`crate::UiEvent`], without reaching for a concrete backend type.
//! - [`FocusSpec`] / [`FocusStop`] — how a widget declares its own
//!   keyboard-navigation stops (none, itself, or a list of sub-targets) so
//!   a host can build a tab order without the widget's internals.
//! - [`WidgetAction`] / [`Never`] — the trait a widget's `Action` type
//!   implements to turn itself into a [`crate::UiEvent`] the app routes
//!   further, plus the uninhabited type a non-interactive widget uses for
//!   "this never produces an action".
//!
//! # What's explicitly not here
//!
//! This module only defines the contract. The runtime that calls
//! [`Interactive::handle`] — translating raw backend input into
//! [`RoutedEvent`]s, walking focus order, owning the live [`EventHost`]
//! implementation — is `crate::runtime::preprocess_event`'s widget-routing
//! step, built on [`crate::frame_tree::FrameTree`] and
//! [`crate::widget_state::WidgetStateStore`].

use crate::a11y::A11yAction;
use crate::backend::PointerShape;
use crate::event::{Key, MouseButton, Point, UiEvent};
use crate::primitive::{Primitive, Units};
use crate::types::{Modifiers, WidgetId};
use crate::Handled;

/// Opaque address of one sub-part inside a composite widget's own
/// internal structure — a cell, a row, a column, whichever shape this
/// widget's own [`Interactive::focus`]/[`Interactive::handle`] needs.
///
/// Not to be confused with [`crate::primitive::SubTarget`], the *paint*
/// module's unrelated pixel-surface/cell-grid tag — same short name,
/// different concept, different module. This `SubTarget` is never
/// interpreted by the host that walks tab order or dispatches
/// [`RoutedEvent`]s: it is produced by one widget's own [`FocusStop`]/
/// hit-test and handed back to that same widget's [`Interactive::handle`]
/// verbatim, so a `DataTable` can pack a `(row, col)` pair into it and a
/// `TreeTable` can pack a row index, without either shape leaking into
/// this module.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SubTarget(pub u64);

/// A widget's `Interactive::Action` payload turns itself into the
/// [`UiEvent`] the app routes further.
///
/// `id` is the widget's own [`WidgetId`] — most impls just wrap it into a
/// `UiEvent` variant that already carries a `widget: WidgetId`/`Option<WidgetId>`
/// field, the same way every existing `compose::*Controller::handle_event`
/// caller already attaches the controller's id to the `Handled::Event`
/// payload it gets back.
pub trait WidgetAction {
    /// Turn `self` into the [`UiEvent`] this widget's owner (`id`) produced.
    fn into_ui_event(self, id: WidgetId) -> UiEvent;
}

/// The uninhabited `Action` type a non-interactive widget uses — see
/// [`non_interactive!`]. No value of this type can ever exist, so
/// [`WidgetAction::into_ui_event`] is unreachable by construction, not by
/// convention.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Never {}

impl WidgetAction for Never {
    fn into_ui_event(self, _id: WidgetId) -> UiEvent {
        match self {}
    }
}

/// How a widget participates in keyboard tab order.
///
/// Returned by [`Interactive::focus`]. A composite widget (a `DataTable`
/// with per-cell navigation, a `TreeTable` with per-row navigation) names
/// its own internal stops via [`Self::Children`] instead of exposing one
/// opaque stop for the whole widget — the host's tab-order walk sees the
/// real stop count without knowing anything about the widget's internals.
#[derive(Debug, Clone, PartialEq)]
pub enum FocusSpec {
    /// This widget never receives keyboard focus (a `Progress` bar, a
    /// static `Divider`).
    None,
    /// This widget is one focus stop as a whole (a `Button`, a single-line
    /// `TextInput`).
    Stop,
    /// This widget contributes one focus stop per listed [`FocusStop`] —
    /// e.g. one per focusable cell/row/column.
    Children(Vec<FocusStop>),
}

/// One focusable stop inside a [`FocusSpec::Children`] list.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FocusStop {
    /// Which part of the widget this stop addresses.
    pub sub: SubTarget,
}

/// Which pointer gesture a [`RoutedEvent::Pointer`] carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum PointerKind {
    /// A button went down.
    Down(MouseButton),
    /// A button was released.
    Up(MouseButton),
    /// The pointer moved with no button transition.
    Move,
    /// The pointer entered this widget's bounds.
    Enter,
    /// The pointer left this widget's bounds.
    Leave,
    /// A double-click resolved on this button.
    DoubleClick(MouseButton),
}

/// A keystroke, carried by [`RoutedEvent::Key`] — the same `key` /
/// `modifiers` / `repeat` triple [`UiEvent::KeyPressed`] carries, named as
/// one value so [`RoutedEvent`] doesn't need three loose fields of its own.
#[derive(Debug, Clone, PartialEq)]
pub struct KeyEvent {
    /// The key pressed.
    pub key: Key,
    /// Modifier keys held during the keystroke.
    pub modifiers: Modifiers,
    /// Whether this is an auto-repeat of a held key, not its initial
    /// press.
    pub repeat: bool,
}

/// The one event shape [`Interactive::handle`] receives.
///
/// Already resolved against this widget's own [`Primitive::layout`]
/// output by the runtime that calls `handle` — `local` is relative to the
/// widget's own area, not window/screen coordinates, and `sub` (where
/// present) names which part of a composite widget the event landed on,
/// when the runtime's hit-test could tell.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum RoutedEvent {
    /// A mouse/touch gesture.
    Pointer {
        /// Which gesture resolved.
        kind: PointerKind,
        /// Which part of a composite widget the gesture landed on, when
        /// the runtime's hit-test could tell.
        sub: Option<SubTarget>,
        /// Position relative to this widget's own area.
        local: Point,
        /// Modifier keys held during the gesture.
        mods: Modifiers,
    },
    /// A scroll-wheel gesture.
    Wheel {
        /// Horizontal scroll delta.
        dx: f32,
        /// Vertical scroll delta.
        dy: f32,
        /// Position relative to this widget's own area.
        local: Point,
        /// Modifier keys held during the gesture.
        mods: Modifiers,
    },
    /// A raw keystroke, routed because this widget held focus.
    Key(KeyEvent),
    /// Committed IME text, routed because this widget held focus — the
    /// [`RoutedEvent`] counterpart of [`UiEvent::CharTyped`].
    Text(String),
    /// An assistive-technology action, routed the same way a mouse/keyboard
    /// gesture would be.
    Action(A11yAction),
    /// This widget (or, for a composite widget, the named sub-target)
    /// gained keyboard focus.
    FocusIn(Option<SubTarget>),
    /// This widget lost keyboard focus.
    FocusOut,
    /// An event that landed on one of this widget's open popups — see
    /// [`Interactive::popups`].
    ///
    /// `event` is resolved against the popup, not the owner: a
    /// [`Self::Pointer`]/[`Self::Wheel`]'s `local` is relative to the
    /// popup's rect, and a `Pointer`'s `sub` is the sub-target the
    /// popup's content names under the pointer
    /// ([`Interactive::sub_regions`] on [`crate::popup::Popup::content`]).
    /// A popup swallows the events that land on it, so answering
    /// [`Handled::Ignored`] stops the event rather than passing it to
    /// whatever is painted underneath.
    PopupEvent {
        /// Which of this widget's popups the event landed on.
        popup: crate::popup::PopupKey,
        /// The event, resolved against that popup.
        event: Box<RoutedEvent>,
    },
    /// The runtime dismissed one of this widget's open popups — an
    /// outside press or Escape, as that popup's
    /// [`crate::popup::PopupDismiss`] allows. The widget closes it the
    /// way it closes it for any other reason: emitting the event that
    /// makes the app clear the field it reads in [`Interactive::popups`],
    /// or clearing its own state.
    PopupDismissed {
        /// Which of this widget's popups was dismissed.
        popup: crate::popup::PopupKey,
        /// What dismissed it.
        reason: crate::popup::DismissReason,
    },
}

/// The runtime seam [`Interactive::handle`] reaches through [`EventCx`] to
/// ask for focus, capture the pointer, change the cursor, or emit a
/// [`UiEvent`] — implemented by the runtime that drives [`Interactive`]
/// widgets (a later issue), never by a widget itself.
pub trait EventHost {
    /// Whether `id` (or, for a composite widget, its `sub` part) currently
    /// holds keyboard focus.
    fn is_focused(&self, id: &WidgetId, sub: Option<SubTarget>) -> bool;
    /// Ask the host to move keyboard focus to `id` (or its `sub` part).
    fn request_focus(&mut self, id: &WidgetId, sub: Option<SubTarget>);
    /// Route every subsequent pointer event to `id` until
    /// [`Self::release_pointer`], regardless of where the pointer moves —
    /// a drag/resize/scrollbar-track gesture's usual shape.
    fn capture_pointer(&mut self, id: &WidgetId);
    /// End a pointer capture started by [`Self::capture_pointer`].
    fn release_pointer(&mut self);
    /// Request the given pointer cursor while over this widget.
    fn set_cursor(&mut self, cursor: PointerShape);
    /// Ask the host to repaint — the same "something changed, redraw"
    /// signal [`crate::compose::Handled::Consumed`] carries for a
    /// `compose::*Controller`.
    fn request_redraw(&mut self);
    /// Hand a semantic event to the host for routing.
    fn emit(&mut self, event: UiEvent);
    /// Which unit system this host's layouts are expressed in — see
    /// [`Units`].
    fn units(&self) -> Units;
    /// The pixel surface this host paints on, for a widget that measures
    /// its painted text to resolve a pointer position to a text position
    /// (the [`crate::PaintSurface`] text verbs shape exactly as paint
    /// does). `None` on a cell grid or with no surface in reach, where
    /// the widget measures in display cells. Defaults to `None`.
    fn text_surface(&mut self) -> Option<&mut dyn crate::PaintSurface> {
        None
    }
}

/// [`Interactive::handle`]'s narrow view of the live [`EventHost`], bound
/// to the calling widget's own [`WidgetId`] so every call reads as "about
/// me" rather than repeating `id` at every call site.
pub struct EventCx<'a> {
    /// The widget this context was constructed for — every method below
    /// implicitly addresses the live [`EventHost`] with this id.
    pub id: WidgetId,
    host: &'a mut dyn EventHost,
}

impl<'a> EventCx<'a> {
    /// Bind `host` to `id` for the duration of one [`Interactive::handle`]
    /// call.
    pub fn new(id: WidgetId, host: &'a mut dyn EventHost) -> Self {
        Self { id, host }
    }

    /// Whether this widget (or its `sub` part) currently holds keyboard
    /// focus — see [`EventHost::is_focused`].
    pub fn is_focused(&self, sub: Option<SubTarget>) -> bool {
        self.host.is_focused(&self.id, sub)
    }

    /// Ask the host to move keyboard focus to this widget (or its `sub`
    /// part) — see [`EventHost::request_focus`].
    pub fn request_focus(&mut self, sub: Option<SubTarget>) {
        self.host.request_focus(&self.id, sub);
    }

    /// The host's pixel surface for measuring painted text — see
    /// [`EventHost::text_surface`].
    pub fn text_surface(&mut self) -> Option<&mut dyn crate::PaintSurface> {
        self.host.text_surface()
    }

    /// Capture the pointer for this widget — see
    /// [`EventHost::capture_pointer`].
    pub fn capture_pointer(&mut self) {
        self.host.capture_pointer(&self.id);
    }

    /// Release a pointer capture started by [`Self::capture_pointer`] —
    /// see [`EventHost::release_pointer`].
    pub fn release_pointer(&mut self) {
        self.host.release_pointer();
    }

    /// Request a pointer cursor — see [`EventHost::set_cursor`].
    pub fn set_cursor(&mut self, cursor: PointerShape) {
        self.host.set_cursor(cursor);
    }

    /// Ask the host to repaint — see [`EventHost::request_redraw`].
    pub fn request_redraw(&mut self) {
        self.host.request_redraw();
    }

    /// Hand a semantic event to the host — see [`EventHost::emit`].
    pub fn emit(&mut self, event: UiEvent) {
        self.host.emit(event);
    }

    /// Which unit system the host's layouts are expressed in — see
    /// [`EventHost::units`].
    pub fn units(&self) -> Units {
        self.host.units()
    }
}

/// The **behaviour** half of the widget contract — every widget
/// implements this, either with real logic or via [`non_interactive!`].
///
/// Mirrors [`Primitive`]'s own shape: `Self::Layout` (from `Primitive`) is
/// what [`Self::handle`] hit-tests against, the same fully-resolved value
/// [`Primitive::paint`] receives, so a widget never needs a second,
/// behaviour-only layout pass.
pub trait Interactive: Primitive {
    /// This widget's own interaction state — hover/pressed index, scroll
    /// offset, in-progress drag, whatever [`Self::handle`] needs to
    /// remember between events. `Default` so a host can create one before
    /// the first event ever arrives.
    type State: Default + 'static;

    /// The semantic event(s) this widget's [`Self::handle`] can produce —
    /// see [`WidgetAction`]. [`Never`] for a widget that never produces
    /// one (still possible even for an [`Interactive`] impl that only ever
    /// returns [`crate::compose::Handled::Consumed`]/[`crate::compose::Handled::Ignored`]).
    type Action: WidgetAction;

    /// How this widget participates in keyboard tab order. Default:
    /// [`FocusSpec::None`] — correct for a widget with no keyboard story
    /// at all.
    fn focus(&self) -> FocusSpec {
        FocusSpec::None
    }

    /// Handle one [`RoutedEvent`] against `layout` (as produced by
    /// [`Primitive::layout`]), reading/updating `state` and reaching
    /// through `cx` for focus/capture/cursor/emit.
    fn handle(
        &self,
        state: &mut Self::State,
        event: &RoutedEvent,
        layout: &Self::Layout,
        cx: &mut EventCx,
    ) -> Handled<Self::Action>;

    /// Assistive-technology actions this widget currently supports, given
    /// `state` — e.g. a collapsed tree row supports `Expand` but not
    /// `Collapse`. Default: none.
    fn a11y_actions(&self, state: &Self::State) -> Vec<A11yAction> {
        let _ = state;
        Vec::new()
    }

    /// This widget's addressable sub-parts for `layout`, each paired with
    /// the [`SubTarget`] that names it — the pointer-side counterpart of
    /// [`Self::focus`]'s [`FocusSpec::Children`].
    ///
    /// Rects are in the **widget's own local space** (origin at the
    /// widget's painted rect, the same space [`RoutedEvent::Pointer`]'s
    /// `local` field uses), so a widget derives them from `layout`
    /// without knowing where it was painted.
    ///
    /// Order is **most-specific first**: a close button before the tab
    /// body that contains it, so the first rect containing a point is the
    /// right answer — the same ordering
    /// [`crate::TabBarLayout::hit_test`] already walks its own hit
    /// regions in.
    ///
    /// The host side of this is
    /// [`crate::primitive::PaintedWidgets`]: every widget painted through
    /// [`crate::DrawsPrimitives::draw_dyn`] gets these rects recorded
    /// (translated into absolute coordinates) for the frame, so a driver
    /// or binding layer can target one sub-part and
    /// [`crate::primitive::painted_hit`] can answer "which sub-part is
    /// under this point" for any contract widget through one code path.
    ///
    /// Default: empty — correct for a widget with no addressable
    /// sub-parts (a `Button`, a `Divider`), which is then hit-tested as
    /// one whole rect.
    fn sub_regions(&self, layout: &Self::Layout) -> Vec<(crate::Rect, SubTarget)> {
        let _ = layout;
        Vec::new()
    }

    /// The name [`crate::DrawsPrimitives::draw_dyn`] publishes sub-part
    /// `sub` under, as a frame-inventory zone (see
    /// [`crate::Backend::register_zone`]) whose id is `"<widget id>:<name>"`.
    ///
    /// This is how a composite keeps its regions observable to drivers and
    /// structural tests once it paints its children itself: an app shell
    /// names its status bar `"status-bar"`, so a shell with id
    /// `app-shell` publishes the zone `app-shell:status-bar`. A sub-part
    /// answered with `None` is hit-testable through
    /// [`Self::sub_regions`] but not published.
    ///
    /// Default: `None` for every sub-part, so nothing is published.
    fn sub_region_name(&self, sub: SubTarget) -> Option<&'static str> {
        let _ = sub;
        None
    }

    /// Whether the pointer counts as on this widget only over the
    /// sub-parts it does not publish as zones (those with no
    /// [`Self::sub_region_name`]); everywhere else in its rect the pointer
    /// passes through to whatever is painted beneath, so moving across it
    /// changes no hover and repaints nothing. For a composite that lays
    /// out the whole window but reacts to the pointer on a few parts of it
    /// — an app shell, whose activity rows tint and whose chrome bands do
    /// not. Default: `false`, the whole rect.
    const HOVER_ON_SUB_REGIONS_ONLY: bool = false;

    /// Whether this widget receives [`PointerKind::Move`] for every
    /// pointer move inside it while it is hovered, not only
    /// [`PointerKind::Enter`] when the pointer arrives and
    /// [`PointerKind::Leave`] when it goes. For a widget whose hover
    /// target is finer than its own rect but not a published sub-region —
    /// a chart's nearest data point, a canvas's declared region — so its
    /// hover follows the pointer across it.
    ///
    /// Without it a widget sees `Move` only while it holds the pointer
    /// capture ([`EventHost::capture_pointer`]). A widget that sets it
    /// still gets `Enter` (never a `Move`) for the move that brings the
    /// pointer onto it, and `Leave` for the one that takes it off.
    /// Default: `false`.
    const RECEIVES_HOVER_MOVES: bool = false;

    /// Whether this widget can ever answer [`Self::popups`] with anything.
    /// The runtime asks [`Self::popups`] only of widgets that set this,
    /// so routing an event never lays out a widget that has no popups to
    /// offer. A widget that overrides [`Self::popups`] sets it to `true`;
    /// one that leaves it `false` has its popups neither painted nor
    /// routed.
    const OPENS_POPUPS: bool = false;

    /// The popups this widget has open, given `state` and `layout` — a
    /// dropdown list, a calendar grid, a submenu. Default: none. Only
    /// asked when [`Self::OPENS_POPUPS`] is set.
    ///
    /// Open is whatever this answer says it is: the runtime keeps no
    /// popup state of its own. A widget in controlled mode reads a
    /// descriptor field the app sets (`open`), and closes the popup by
    /// emitting the event that makes the app clear it. Each popup is
    /// painted above the whole frame, hit-tested before any widget, and
    /// its events reach this widget's [`Self::handle`] as
    /// [`RoutedEvent::PopupEvent`]; an outside press or Escape arrives as
    /// [`RoutedEvent::PopupDismissed`]. See [`crate::popup`].
    ///
    /// A popup's [`crate::popup::Popup::anchor`] is in this widget's
    /// local space, the same space as [`Self::sub_regions`].
    ///
    /// ```
    /// use quadraui::popup::{Popup, PopupKey};
    /// use quadraui::primitive::Size;
    /// use quadraui::{Rect, StatusBar, WidgetId};
    ///
    /// /// What a dropdown's `popups` returns while its `open` field is
    /// /// set: one list, anchored under the field's first row.
    /// fn dropdown(open: bool, field: Rect, rows: usize) -> Vec<Popup> {
    ///     if !open {
    ///         return Vec::new();
    ///     }
    ///     let list = StatusBar {
    ///         id: WidgetId::new("dropdown:list"),
    ///         left_segments: Vec::new(),
    ///         right_segments: Vec::new(),
    ///     };
    ///     let size = Size::new(field.width, rows as f32);
    ///     vec![Popup::new(PopupKey(0), field, size, list)]
    /// }
    ///
    /// assert!(dropdown(false, Rect::new(0.0, 0.0, 20.0, 1.0), 4).is_empty());
    /// assert_eq!(dropdown(true, Rect::new(0.0, 0.0, 20.0, 1.0), 4).len(), 1);
    /// ```
    fn popups(&self, state: &Self::State, layout: &Self::Layout) -> Vec<crate::popup::Popup> {
        let _ = (state, layout);
        Vec::new()
    }

    /// Content this widget needs loaded before `layout` can paint fully —
    /// a windowed collection's [`UiEvent::RowsRequested`], a sheet's
    /// `CellGridEvent::CellsRequested`.
    ///
    /// A host calls it after every input the widget handled (any outcome
    /// but a bare [`Handled::Ignored`]), against a layout taken from the
    /// state [`Self::handle`] just wrote, and delivers the answer like any
    /// other event. `None` when everything visible is loaded, or when this
    /// exact request was already made. Default: `None`.
    fn data_requested(&self, state: &mut Self::State, layout: &Self::Layout) -> Option<UiEvent> {
        let _ = (state, layout);
        None
    }

    /// Record what laying the widget out just resolved into its own state,
    /// before it paints — a selection the app moved scrolled into view.
    /// The host calls it for every frame it paints the widget, after
    /// `layout`, and lays the widget out again when it answers `true`
    /// (it changed `state`). Default: nothing to record.
    fn settle(&self, state: &mut Self::State, layout: &Self::Layout) -> bool {
        let _ = (state, layout);
        false
    }

    /// An event that names this widget which its host delivered on the
    /// app's behalf rather than this widget emitting it — an app or a test
    /// harness dispatching a `TabActivated` for a tab bar, say. A widget
    /// whose state is uncontrolled by default follows it as if it had
    /// produced the event itself, so the next frame shows the activated
    /// tab without the app echoing it onto the descriptor. A host calls
    /// this for every painted widget; each ignores an event that does not
    /// name it. Default: ignore it.
    fn follow_event(&self, state: &mut Self::State, event: &UiEvent) {
        let _ = (state, event);
    }

    /// A host-facing summary of the interaction state this widget keeps —
    /// the selection, scroll position, active tab or expanded rows it
    /// resolves for the next frame — for a binding that reports a widget's
    /// state back to its app. Default: an empty summary.
    fn state_summary(&self, state: &Self::State) -> StateSummary {
        let _ = state;
        StateSummary::default()
    }
}

/// What [`Interactive::state_summary`] reports: each field is `Some` only
/// for a widget that keeps that kind of state.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct StateSummary {
    /// A tree's resolved selected row.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selected_path: Option<crate::types::TreePath>,
    /// A list's or table's resolved selected row (absolute when
    /// windowed).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selected_idx: Option<usize>,
    /// The resolved scroll row (absolute when windowed).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scroll_offset: Option<usize>,
    /// A tab bar's resolved active tab.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_tab: Option<usize>,
    /// A tree's expanded branches, when it resolves expansion itself.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expanded: Option<Vec<crate::types::TreePath>>,
}

impl StateSummary {
    /// A summary with only `scroll_offset` set.
    pub fn scroll(scroll_offset: usize) -> Self {
        Self {
            scroll_offset: Some(scroll_offset),
            ..Self::default()
        }
    }

    /// This summary with `selected_idx` set.
    pub fn with_selected_idx(mut self, idx: Option<usize>) -> Self {
        self.selected_idx = idx;
        self
    }

    /// This summary with `selected_path` set.
    pub fn with_selected_path(mut self, path: Option<crate::types::TreePath>) -> Self {
        self.selected_path = path;
        self
    }

    /// This summary with `active_tab` set.
    pub fn with_active_tab(mut self, active: Option<usize>) -> Self {
        self.active_tab = active;
        self
    }

    /// This summary with `expanded` set.
    pub fn with_expanded(mut self, expanded: Option<Vec<crate::types::TreePath>>) -> Self {
        self.expanded = expanded;
        self
    }
}

/// Implement [`Interactive`] for a [`Primitive`] with no behaviour at all
/// — `State = ()`, `Action = `[`Never`], `handle` always
/// [`crate::compose::Handled::Ignored`]. For a widget whose real
/// interaction logic hasn't shipped yet, or that genuinely never reacts to
/// input (`Progress`, `Divider`).
#[macro_export]
macro_rules! non_interactive {
    ($ty:ty) => {
        impl $crate::interactive::Interactive for $ty {
            type State = ();
            type Action = $crate::interactive::Never;

            fn handle(
                &self,
                _state: &mut Self::State,
                _event: &$crate::interactive::RoutedEvent,
                _layout: &Self::Layout,
                _cx: &mut $crate::interactive::EventCx,
            ) -> $crate::Handled<Self::Action> {
                $crate::Handled::Ignored
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::a11y::{A11yInfo, A11yPrimitive};
    use crate::event::NamedKey;
    use crate::primitive::LayoutCtx;
    use crate::Rect;

    /// A toy counter widget: focus `Stop`, `Up`/`Down` arrow keys change
    /// its count, Enter emits an [`Handled::Event`].
    #[derive(Debug, Default)]
    struct Counter;

    #[derive(Debug, Default)]
    struct CounterState {
        count: i32,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct CounterActivated(i32);

    impl WidgetAction for CounterActivated {
        fn into_ui_event(self, id: WidgetId) -> UiEvent {
            UiEvent::DoubleClick {
                widget: Some(id),
                position: Point::default(),
            }
        }
    }

    impl A11yPrimitive for Counter {
        fn a11y_info(&self) -> A11yInfo {
            A11yInfo::default()
        }
    }

    impl Primitive for Counter {
        type Layout = Rect;
        type Event = ();

        fn layout(&self, _ctx: &LayoutCtx, rect: Rect) -> Rect {
            rect
        }

        fn paint(
            &self,
            _layout: &Rect,
            _surface: &mut dyn crate::paint_surface::PaintSurface,
            _env: &crate::PaintEnv,
        ) {
        }
    }

    impl Interactive for Counter {
        type State = CounterState;
        type Action = CounterActivated;

        fn focus(&self) -> FocusSpec {
            FocusSpec::Stop
        }

        fn handle(
            &self,
            state: &mut CounterState,
            event: &RoutedEvent,
            _layout: &Rect,
            cx: &mut EventCx,
        ) -> Handled<CounterActivated> {
            match event {
                RoutedEvent::Key(KeyEvent {
                    key: Key::Named(NamedKey::Up),
                    ..
                }) => {
                    state.count += 1;
                    cx.request_redraw();
                    Handled::Consumed
                }
                RoutedEvent::Key(KeyEvent {
                    key: Key::Named(NamedKey::Down),
                    ..
                }) => {
                    state.count -= 1;
                    cx.request_redraw();
                    Handled::Consumed
                }
                RoutedEvent::Key(KeyEvent {
                    key: Key::Named(NamedKey::Enter),
                    ..
                }) => Handled::Event(CounterActivated(state.count)),
                _ => Handled::Ignored,
            }
        }
    }

    /// Minimal [`EventHost`] mock: records every call so a test can assert
    /// on what [`Interactive::handle`] asked for.
    #[derive(Debug, Default)]
    struct MockHost {
        focused: Option<(WidgetId, Option<SubTarget>)>,
        focus_requests: Vec<(WidgetId, Option<SubTarget>)>,
        redraws: usize,
        emitted: Vec<UiEvent>,
    }

    impl EventHost for MockHost {
        fn is_focused(&self, id: &WidgetId, sub: Option<SubTarget>) -> bool {
            self.focused == Some((*id, sub))
        }

        fn request_focus(&mut self, id: &WidgetId, sub: Option<SubTarget>) {
            self.focus_requests.push((*id, sub));
            self.focused = Some((*id, sub));
        }

        fn capture_pointer(&mut self, _id: &WidgetId) {}

        fn release_pointer(&mut self) {}

        fn set_cursor(&mut self, _cursor: PointerShape) {}

        fn request_redraw(&mut self) {
            self.redraws += 1;
        }

        fn emit(&mut self, event: UiEvent) {
            self.emitted.push(event);
        }

        fn units(&self) -> Units {
            Units::Cells
        }
    }

    fn key(named: NamedKey) -> RoutedEvent {
        RoutedEvent::Key(KeyEvent {
            key: Key::Named(named),
            modifiers: Modifiers::default(),
            repeat: false,
        })
    }

    #[test]
    fn counter_declares_a_single_focus_stop() {
        assert_eq!(Counter.focus(), FocusSpec::Stop);
    }

    /// A composite widget (a `DataTable`-shaped `focus()`) names one
    /// [`FocusStop`] per cell, packing `(row, col)` into [`SubTarget`]'s
    /// opaque `u64` — proving the type actually carries per-cell/per-row
    /// addresses rather than just the two paint-target tags of
    /// [`crate::primitive::SubTarget`].
    #[test]
    fn focus_children_names_one_stop_per_cell() {
        fn cell(row: u64, col: u64) -> SubTarget {
            SubTarget((row << 32) | col)
        }

        let spec = FocusSpec::Children(vec![
            FocusStop { sub: cell(0, 0) },
            FocusStop { sub: cell(0, 1) },
            FocusStop { sub: cell(1, 0) },
        ]);

        match spec {
            FocusSpec::Children(stops) => {
                assert_eq!(stops.len(), 3);
                assert_eq!(stops[0].sub, cell(0, 0));
                assert_eq!(stops[2].sub, cell(1, 0));
                assert_ne!(stops[0].sub, stops[1].sub);
            }
            other => panic!("expected FocusSpec::Children, got {other:?}"),
        }
    }

    #[test]
    fn arrow_keys_change_state_and_request_a_redraw() {
        let widget = Counter;
        let mut state = CounterState::default();
        let mut host = MockHost::default();
        let id = WidgetId::new("counter:1");

        let mut cx = EventCx::new(id, &mut host);
        let outcome = widget.handle(&mut state, &key(NamedKey::Up), &Rect::default(), &mut cx);
        assert_eq!(outcome, Handled::Consumed);
        assert_eq!(state.count, 1);
        assert_eq!(host.redraws, 1);

        let mut cx = EventCx::new(id, &mut host);
        let outcome = widget.handle(&mut state, &key(NamedKey::Down), &Rect::default(), &mut cx);
        assert_eq!(outcome, Handled::Consumed);
        assert_eq!(state.count, 0);
        assert_eq!(host.redraws, 2);
    }

    #[test]
    fn enter_emits_a_handled_event_carrying_the_current_count() {
        let widget = Counter;
        let mut state = CounterState { count: 3 };
        let mut host = MockHost::default();
        let id = WidgetId::new("counter:2");
        let mut cx = EventCx::new(id, &mut host);

        let outcome = widget.handle(&mut state, &key(NamedKey::Enter), &Rect::default(), &mut cx);
        assert_eq!(outcome, Handled::Event(CounterActivated(3)));
    }

    #[test]
    fn unrelated_events_are_ignored() {
        let widget = Counter;
        let mut state = CounterState::default();
        let mut host = MockHost::default();
        let id = WidgetId::new("counter:3");
        let mut cx = EventCx::new(id, &mut host);

        let outcome = widget.handle(
            &mut state,
            &RoutedEvent::Pointer {
                kind: PointerKind::Move,
                sub: None,
                local: Point::default(),
                mods: Modifiers::default(),
            },
            &Rect::default(),
            &mut cx,
        );
        assert_eq!(outcome, Handled::Ignored);
    }

    #[test]
    fn event_cx_requests_focus_against_its_own_bound_id() {
        let mut host = MockHost::default();
        let id = WidgetId::new("counter:4");
        let mut cx = EventCx::new(id, &mut host);

        assert!(!cx.is_focused(None));
        cx.request_focus(None);
        assert!(cx.is_focused(None));
        assert_eq!(host.focus_requests, vec![(id, None)]);
    }

    #[test]
    fn widget_action_turns_itself_into_a_ui_event_carrying_the_widget_id() {
        let id = WidgetId::new("counter:5");
        let event = CounterActivated(7).into_ui_event(id);
        match event {
            UiEvent::DoubleClick { widget, .. } => assert_eq!(widget, Some(id)),
            other => panic!("expected UiEvent::DoubleClick, got {other:?}"),
        }
    }

    #[test]
    fn never_implements_widget_action() {
        fn _assert_into_ui_event_exists<T: WidgetAction>() {}
        _assert_into_ui_event_exists::<Never>();
    }

    /// A toy stand-in for a `Primitive` with no behaviour — proves
    /// [`crate::non_interactive!`] compiles against an arbitrary type and
    /// always ignores events.
    #[derive(Debug, Default)]
    struct StaticDivider;

    impl A11yPrimitive for StaticDivider {
        fn a11y_info(&self) -> A11yInfo {
            A11yInfo::default()
        }
    }

    impl Primitive for StaticDivider {
        type Layout = Rect;
        type Event = ();

        fn layout(&self, _ctx: &LayoutCtx, rect: Rect) -> Rect {
            rect
        }

        fn paint(
            &self,
            _layout: &Rect,
            _surface: &mut dyn crate::paint_surface::PaintSurface,
            _env: &crate::PaintEnv,
        ) {
        }
    }

    crate::non_interactive!(StaticDivider);

    #[test]
    fn non_interactive_macro_ignores_every_event() {
        let widget = StaticDivider;
        let mut state = ();
        let mut host = MockHost::default();
        let id = WidgetId::new("divider:1");
        let mut cx = EventCx::new(id, &mut host);

        let outcome = widget.handle(
            &mut state,
            &RoutedEvent::Pointer {
                kind: PointerKind::Down(MouseButton::Left),
                sub: None,
                local: Point::default(),
                mods: Modifiers::default(),
            },
            &Rect::default(),
            &mut cx,
        );
        assert_eq!(outcome, Handled::Ignored);
        assert!(widget.a11y_actions(&state).is_empty());
        assert_eq!(widget.focus(), FocusSpec::None);
    }
}
