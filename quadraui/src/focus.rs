//! `FocusTree` — the single owner of "what widget (and, for a composite
//! widget, which of its own children) currently has keyboard focus".
//!
//! # Ownership: one per backend, one writer
//!
//! Each concrete backend (`TuiBackend`, `GtkBackend`, `MacBackend`,
//! `WinBackend`) owns exactly one `FocusTree`, reachable read-only via
//! [`crate::Backend::focus_tree`]. The only code that *mutates* it is
//! [`crate::Backend::set_focus`] (whole-widget focus changes, e.g.
//! click-to-focus) and the shared Tab/Shift+Tab intercept in
//! [`crate::runtime::preprocess_event`] (via the crate-private
//! `PreprocessBackend::focus_tree_mut`) — every backend's `run.rs` routes
//! through those, so "what's focused" can't drift between backends. Apps
//! never construct or mutate a `FocusTree` directly; they read it off the
//! backend, and drive it indirectly by returning tab stops from
//! [`crate::runner::AppLogic::extra_tab_stops`] or by declaring
//! [`crate::interactive::Interactive::focus`] on a [`crate::runner::AppLogic::frame_tree`]
//! node.
//!
//! # Opt-in, not forced
//!
//! An app participates purely by overriding
//! [`crate::runner::AppLogic::extra_tab_stops`] (default: empty). While it
//! returns `[]`, Tab/Shift+Tab pass through to `AppLogic::handle`
//! completely unclaimed — **zero behavior change** for every app
//! written before this feature. Once an app returns a non-empty list
//! from `extra_tab_stops`, the runner claims Tab/Shift+Tab globally: they
//! stop reaching `AppLogic::handle` as a raw `KeyPressed` and arrive
//! instead as [`crate::UiEvent::FocusChanged`]. An app with a widget
//! that treats a literal Tab keystroke as input (a code editor's
//! indent command, say) should leave that widget out of `extra_tab_stops`
//! entirely, or keep `extra_tab_stops` empty while that widget holds
//! focus — see that method's doc for the full contract.
//!
//! # Focus-ring paint convention
//!
//! [`crate::Backend::draw_focus_ring`] paints the visual cue for
//! whichever widget [`Self::focused`] currently names — a themed,
//! border-only stroke (`Theme::accent_fg`, the same colour `Form`
//! already uses for "this control has focus") drawn *after* the app's
//! own `render`, so it overlays without needing per-primitive support.
//! Every runner (`tui::run`, `gtk::run`, `macos::run`, `win::run`) calls
//! it once per frame, right after `AppLogic::render`, whenever
//! `focus_tree().focused()` names a widget this frame's tab stops
//! can still resolve a rect for.

use crate::event::Rect;
use crate::frame_tree::FrameTree;
use crate::interactive::{FocusSpec, SubTarget};
use crate::types::WidgetId;

/// Wrap `current + delta` into `0..n`, the one index-cycling arithmetic
/// every Tab/Shift+Tab-style stepper in this crate shares — [`FocusTree::step`]
/// and the menu/dialog/toast/form controllers that step a bounded index with wrap-around.
///
/// `delta` may be negative (Shift+Tab) or larger than `n` in magnitude;
/// both wrap correctly. `n == 0` has no valid index to land on, so it
/// returns `0` rather than panicking — callers with an empty collection
/// should check that before stepping it, as every call site above already
/// does.
pub fn cycle_index(current: usize, delta: isize, n: usize) -> usize {
    if n == 0 {
        return 0;
    }
    let n = n as isize;
    let current = current as isize;
    (current + delta).rem_euclid(n) as usize
}

/// `Some(1)` for Tab, `Some(-1)` for Shift+Tab or `BackTab`, with no
/// Ctrl/Alt/Cmd held — the keys a focus walk steps on; `None` for every
/// other key, and for a Tab chord an app keeps for itself.
pub fn tab_step(key: &crate::Key, modifiers: &crate::Modifiers) -> Option<isize> {
    if modifiers.ctrl || modifiers.alt || modifiers.cmd {
        return None;
    }
    match key {
        crate::Key::Named(crate::NamedKey::Tab) if modifiers.shift => Some(-1),
        crate::Key::Named(crate::NamedKey::Tab) => Some(1),
        crate::Key::Named(crate::NamedKey::BackTab) => Some(-1),
        _ => None,
    }
}

/// The child stop after (`delta > 0`) or before (`delta < 0`) `current`
/// in a [`FocusSpec::Children`] widget's own declared order — the step
/// Tab/Shift+Tab takes inside a widget that leaves the key to its host.
/// `None` when `spec` declares no such stops, `current` is not one of
/// them, or it is already the first/last stop in that direction: Tab
/// never wraps inside a widget, so it can leave it.
pub fn step_child_stop(spec: &FocusSpec, current: SubTarget, delta: isize) -> Option<SubTarget> {
    let FocusSpec::Children(stops) = spec else {
        return None;
    };
    let idx = stops.iter().position(|stop| stop.sub == current)?;
    let next = idx.checked_add_signed(delta.signum())?;
    stops.get(next).map(|stop| stop.sub)
}

// Focus-ring stroke width is not a constant in this module: it is the
// app-overridable [`crate::Style::focus_ring_width`] token (native
// units — GTK/macOS/Win-GUI pixels), which every `PaintSurface`-backed
// pixel backend's `Backend::draw_focus_ring` reads via
// `Backend::style()` so all three paint the same weight without three
// independent literals to keep in sync. See that field's doc for the
// TUI story (`crate::tui::draw_focus_ring` paints a 1-cell border and
// consumes no `Style` token).

/// A tab stop's full address: a widget's own id, plus — for a composite
/// widget's [`FocusSpec::Children`] — which sub-part this stop names.
/// `None` addresses the whole widget, the same shape
/// [`crate::interactive::EventHost::is_focused`]/`request_focus` take.
pub type FocusAddress = (WidgetId, Option<SubTarget>);

#[derive(Debug, Clone, PartialEq)]
struct FocusTreeStop {
    address: FocusAddress,
    rect: Rect,
}

/// The real, per-stop tab order for one frame's [`FrameTree`], plus the
/// `:focus-visible` bookkeeping described below.
///
/// # Why a per-stop tree, not a flat widget list
///
/// [`crate::frame::ScreenLayout::tab_stops`] collapses a widget's
/// [`FocusSpec::Children`] down to one stop for the whole widget, because
/// its `(WidgetId, Rect)` shape has nowhere to carry a child's
/// [`SubTarget`] — see that method's doc. `FocusTree` is built straight
/// from a [`FrameTree`]'s real [`DynWidget::focus`] values instead, so a
/// `DataTable` with ten focusable cells contributes ten real
/// [`FocusAddress`] stops, each `(widget_id, Some(sub))`, not one stop a
/// host would have to re-decompose.
///
/// # Reading order
///
/// Stops are sorted top-to-bottom, then left-to-right by the owning
/// node's rect — the same convention [`crate::frame::ScreenLayout::tab_stops`]
/// uses — with [`Vec::sort_by`]'s stable ordering preserving each
/// widget's own child order for stops that share one rect (every child
/// of one [`FocusSpec::Children`] widget sits at that widget's rect).
/// Stepping through that order uses the exact same wraparound arithmetic
/// as [`Self::step`]: [`cycle_index`].
///
/// # Modal trapping
///
/// [`Self::restrict`] narrows this tree's stops down to whatever a host
/// decides is "in scope" — the primitive a modal built on top of
/// [`crate::ModalStack`] uses to trap Tab/Shift+Tab cycling inside
/// itself instead of letting it escape to a base-layer stop hidden
/// behind it. A host checks [`crate::ModalStack::is_empty`] (or reads
/// [`crate::ModalStack::top`]'s bounds) each time it resyncs this tree
/// and, while a modal is open, calls `restrict` with a predicate that
/// keeps only the modal's own stops — by id, or by containment against
/// the modal's own [`crate::ModalEntry::bounds`]. `restrict` only ever
/// narrows: resync from [`Self::sync`]/[`Self::from_frame_tree`]/
/// [`Self::sync_tab_order`] once the modal closes to restore the full
/// stop set.
///
/// # Focus-visible
///
/// VS Code's `:focus-visible` convention: [`Self::focus_next`]/
/// [`Self::focus_prev`] (a keyboard move) mark the ring visible;
/// [`Self::set_focus`] (the click-to-focus path) suppresses it. A host
/// composes `tree.is_focused(id, sub) && tree.is_focus_visible()` into a
/// primitive's own `has_focus`/`focused` descriptor field.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FocusTree {
    stops: Vec<FocusTreeStop>,
    focused: Option<FocusAddress>,
    focus_visible: bool,
}

impl FocusTree {
    /// An empty tree — no stops, nothing focused.
    pub fn new() -> Self {
        Self::default()
    }

    /// Build a fresh tree from this frame's [`FrameTree`], expanding
    /// each node's [`DynWidget::focus`] into zero, one, or many stops:
    /// [`FocusSpec::None`] contributes none, [`FocusSpec::Stop`]
    /// contributes one stop addressed `(id, None)`, and
    /// [`FocusSpec::Children`] contributes one `(id, Some(sub))` stop
    /// per listed [`crate::interactive::FocusStop`], in the order the
    /// widget declared them. Nothing is focused in the result — a host
    /// that wants to carry focus across a rebuild uses [`Self::sync`]
    /// instead.
    pub fn from_frame_tree(tree: &FrameTree) -> Self {
        let mut stops: Vec<FocusTreeStop> = Vec::new();
        for node in tree.nodes() {
            match node.widget.focus() {
                FocusSpec::None => {}
                FocusSpec::Stop => stops.push(FocusTreeStop {
                    address: (node.id, None),
                    rect: node.rect,
                }),
                FocusSpec::Children(children) => {
                    for child in children {
                        stops.push(FocusTreeStop {
                            address: (node.id, Some(child.sub)),
                            rect: node.rect,
                        });
                    }
                }
            }
        }
        stops.sort_by(|a, b| {
            a.rect
                .y
                .partial_cmp(&b.rect.y)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(
                    a.rect
                        .x
                        .partial_cmp(&b.rect.x)
                        .unwrap_or(std::cmp::Ordering::Equal),
                )
        });
        Self {
            stops,
            focused: None,
            focus_visible: false,
        }
    }

    /// Rebuild the tab order from this frame's [`FrameTree`] in place: if
    /// the previously-focused address is still present in the rebuilt
    /// order, focus (and its current focus-visible state) is preserved;
    /// otherwise focus is cleared.
    ///
    /// A whole-widget address (`cur.1 == None`) counts as "still
    /// present" when the rebuilt order carries *any* stop for that same
    /// id, even a `FocusSpec::Children` widget's own per-child
    /// `(id, Some(sub))` stops — `None` names "the whole widget, no
    /// child singled out" (see [`FocusAddress`]'s own doc), the address
    /// [`Self::sync_tab_order`] always lands a [`crate::runner::AppLogic::extra_tab_stops`]
    /// landing on since that flat shape carries no [`SubTarget`] to be
    /// specific with. Without this, Tab landing on a `FocusSpec::Children`
    /// widget through `extra_tab_stops` would lose focus on the very
    /// next routed event — this method runs on every one of them
    /// ([`crate::runtime::route_widget_event`]) — because the rebuilt
    /// stops are only ever addressed per-child, never `(id, None)`.
    pub fn sync(&mut self, tree: &FrameTree) {
        let rebuilt = Self::from_frame_tree(tree);
        let keep_focus = self.focused.as_ref().is_some_and(|cur| {
            rebuilt
                .stops
                .iter()
                .any(|stop| stop.address == *cur || (cur.1.is_none() && stop.address.0 == cur.0))
        });
        self.stops = rebuilt.stops;
        if !keep_focus {
            self.focused = None;
        }
    }

    /// Rebuild this tree's stops from a flat `(WidgetId, Rect)` list, for
    /// a caller that has [`crate::runner::AppLogic::extra_tab_stops`]'s
    /// flat shape rather than a [`FrameTree`] to rebuild from
    /// ([`Self::sync`]'s own source), or a host managing its own
    /// [`FocusTree`] directly with no [`FrameTree`]/
    /// [`crate::interactive::Interactive`] widgets at all (see the type
    /// doc's "Modal trapping" section). Every stop is addressed
    /// `(id, None)` — the flat shape carries no [`SubTarget`] to
    /// preserve. If the previously-focused address is still present in
    /// the rebuilt order, focus (and its focus-visible state) is
    /// preserved; otherwise focus is cleared.
    pub fn sync_tab_order(&mut self, stops: &[(WidgetId, Rect)]) {
        let rebuilt: Vec<FocusTreeStop> = stops
            .iter()
            .map(|(id, rect)| FocusTreeStop {
                address: (*id, None),
                rect: *rect,
            })
            .collect();
        let keep_focus = self
            .focused
            .as_ref()
            .is_some_and(|cur| rebuilt.iter().any(|stop| &stop.address == cur));
        self.stops = rebuilt;
        if !keep_focus {
            self.focused = None;
        }
    }

    /// Drop every stop for which `keep` answers `false`, each tested
    /// against its own [`FocusAddress`] and rect — the primitive a host
    /// builds "modal trapping" from (see the type doc's "Modal
    /// trapping" section): once a modal opens, call with a predicate
    /// that keeps only the modal's own stops, by id or by containment
    /// against the modal's bounds, so [`Self::focus_next`]/
    /// [`Self::focus_prev`] cycle within it instead of escaping to a
    /// base-layer stop hidden behind it. Clears focus — same as
    /// [`Self::sync`] — if the previously-focused stop doesn't pass.
    ///
    /// Only ever narrows: there is no "widen back" counterpart, because
    /// undoing a restriction is just rebuilding from the source of
    /// truth again — resync with [`Self::sync`]/[`Self::from_frame_tree`]/
    /// [`Self::sync_tab_order`] once the modal closes.
    pub fn restrict(&mut self, mut keep: impl FnMut(FocusAddress, Rect) -> bool) {
        self.stops.retain(|s| keep(s.address, s.rect));
        if let Some(cur) = &self.focused {
            if !self.stops.iter().any(|s| &s.address == cur) {
                self.focused = None;
            }
        }
    }

    /// Every stop's address, in reading order.
    pub fn tab_order(&self) -> Vec<FocusAddress> {
        self.stops.iter().map(|s| s.address).collect()
    }

    /// Whether this tree has no stops at all.
    pub fn is_empty(&self) -> bool {
        self.stops.is_empty()
    }

    /// The number of stops in this tree.
    pub fn len(&self) -> usize {
        self.stops.len()
    }

    /// The currently focused address, if any.
    pub fn focused(&self) -> Option<&FocusAddress> {
        self.focused.as_ref()
    }

    /// The widget id half of [`Self::focused`], for a caller that addresses
    /// focus by whole widget and never sets a [`SubTarget`].
    pub fn focused_id(&self) -> Option<&WidgetId> {
        self.focused.as_ref().map(|(id, _)| id)
    }

    /// The screen rect of the currently focused stop's owning widget, or
    /// `None` when nothing is focused — the [`FocusTree`] counterpart of
    /// [`crate::runtime::focused_stop_rect`]'s linear scan, free because
    /// this tree already carries each stop's rect.
    pub fn focused_rect(&self) -> Option<Rect> {
        let cur = self.focused.as_ref()?;
        self.stops
            .iter()
            .find(|stop| &stop.address == cur)
            .map(|stop| stop.rect)
    }

    /// Whether `id` (or, for a composite widget, its `sub` part)
    /// currently holds focus — the same `(id, sub)` shape
    /// [`crate::interactive::EventHost::is_focused`] takes.
    pub fn is_focused(&self, id: &WidgetId, sub: Option<SubTarget>) -> bool {
        self.focused.as_ref() == Some(&(*id, sub))
    }

    /// Whether the focus ring should currently be painted — `true` after
    /// a keyboard move ([`Self::focus_next`]/[`Self::focus_prev`]),
    /// `false` after [`Self::set_focus`] (a mouse click). See the type
    /// doc's "Focus-visible" section for how a host applies this to a
    /// primitive's own descriptor field.
    pub fn is_focus_visible(&self) -> bool {
        self.focus_visible
    }

    /// Force the focus-visible flag directly, independent of how focus
    /// last moved — e.g. a host that wants the ring shown immediately
    /// after a programmatic [`Self::set_focus`] (focus restored by code,
    /// not a click).
    pub fn set_focus_visible(&mut self, visible: bool) {
        self.focus_visible = visible;
    }

    /// Move focus to the next stop in reading order, wrapping around,
    /// and mark the ring focus-visible. Starting from "nothing
    /// focused", lands on the first stop. No-op (returns `false`) if
    /// this tree has no stops. Returns whether focus actually changed.
    pub fn focus_next(&mut self) -> bool {
        self.step(1)
    }

    /// Move focus to the previous stop in reading order, wrapping
    /// around, and mark the ring focus-visible. Starting from "nothing
    /// focused", lands on the last stop. No-op (returns `false`) if this
    /// tree has no stops. Returns whether focus actually changed.
    pub fn focus_prev(&mut self) -> bool {
        self.step(-1)
    }

    fn step(&mut self, dir: isize) -> bool {
        if self.stops.is_empty() {
            return self.set_focus_internal(None, true);
        }
        let len = self.stops.len();
        let next_idx = match &self.focused {
            None if dir >= 0 => 0,
            None => len - 1,
            Some(cur) => {
                let idx = self
                    .stops
                    .iter()
                    .position(|stop| &stop.address == cur)
                    .unwrap_or(0);
                cycle_index(idx, dir, len)
            }
        };
        let next = self.stops[next_idx].address;
        self.set_focus_internal(Some(next), true)
    }

    /// Set focus explicitly to `address` — the click-to-focus path.
    /// `None` clears focus. Marks the ring **not** focus-visible: a
    /// click focuses the stop without showing the ring, matching VS
    /// Code — see the type doc's "Focus-visible" section. A caller that
    /// wants the ring shown after a programmatic `set_focus` anyway
    /// (e.g. restoring focus after a dialog closes) should call
    /// [`Self::set_focus_visible`] immediately after. Returns whether
    /// focus actually changed.
    pub fn set_focus(&mut self, address: Option<FocusAddress>) -> bool {
        self.set_focus_internal(address, false)
    }

    /// Clear focus. Equivalent to `set_focus(None)`.
    pub fn clear(&mut self) -> bool {
        self.set_focus(None)
    }

    fn set_focus_internal(&mut self, address: Option<FocusAddress>, visible: bool) -> bool {
        self.focus_visible = visible;
        if self.focused != address {
            self.focused = address;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn children(n: u64) -> FocusSpec {
        FocusSpec::Children(
            (0..n)
                .map(|i| crate::interactive::FocusStop { sub: SubTarget(i) })
                .collect(),
        )
    }

    #[test]
    fn step_child_stop_moves_through_the_declared_children_without_wrapping() {
        let spec = children(3);
        assert_eq!(step_child_stop(&spec, SubTarget(0), 1), Some(SubTarget(1)));
        assert_eq!(step_child_stop(&spec, SubTarget(1), 1), Some(SubTarget(2)));
        assert_eq!(step_child_stop(&spec, SubTarget(2), 1), None);
        assert_eq!(step_child_stop(&spec, SubTarget(2), -1), Some(SubTarget(1)));
        assert_eq!(step_child_stop(&spec, SubTarget(0), -1), None);
    }

    #[test]
    fn tab_step_reads_tab_shift_tab_and_back_tab_but_not_chords() {
        use crate::{Key, Modifiers, NamedKey};
        let plain = Modifiers::default();
        let shift = Modifiers {
            shift: true,
            ..Modifiers::default()
        };
        let ctrl = Modifiers {
            ctrl: true,
            ..Modifiers::default()
        };
        assert_eq!(tab_step(&Key::Named(NamedKey::Tab), &plain), Some(1));
        assert_eq!(tab_step(&Key::Named(NamedKey::Tab), &shift), Some(-1));
        assert_eq!(tab_step(&Key::Named(NamedKey::BackTab), &plain), Some(-1));
        assert_eq!(tab_step(&Key::Named(NamedKey::Tab), &ctrl), None);
        assert_eq!(tab_step(&Key::Char('t'), &plain), None);
    }

    #[test]
    fn step_child_stop_needs_a_declared_child_to_start_from() {
        assert_eq!(step_child_stop(&children(3), SubTarget(7), 1), None);
        assert_eq!(step_child_stop(&FocusSpec::Stop, SubTarget(0), 1), None);
        assert_eq!(step_child_stop(&FocusSpec::None, SubTarget(0), 1), None);
    }

    #[test]
    fn cycle_index_empty_returns_zero() {
        assert_eq!(cycle_index(0, 1, 0), 0);
        assert_eq!(cycle_index(5, -3, 0), 0);
    }

    #[test]
    fn cycle_index_single_stays_at_zero() {
        assert_eq!(cycle_index(0, 1, 1), 0);
        assert_eq!(cycle_index(0, -1, 1), 0);
        assert_eq!(cycle_index(0, 42, 1), 0);
    }

    #[test]
    fn cycle_index_wraps_forward_and_backward() {
        assert_eq!(cycle_index(2, 1, 3), 0);
        assert_eq!(cycle_index(0, -1, 3), 2);
    }

    #[test]
    fn cycle_index_handles_negative_and_large_deltas() {
        assert_eq!(cycle_index(1, -5, 3), 2);
        assert_eq!(cycle_index(0, 7, 3), 1);
        assert_eq!(cycle_index(2, -2, 5), 0);
    }

    mod focus_tree {
        use super::*;
        use crate::a11y::{A11yInfo, A11yPrimitive};
        use crate::interactive::{EventCx, FocusStop, Interactive, RoutedEvent};
        use crate::primitive::{LayoutCtx, Primitive};
        use crate::Handled;

        /// A widget that is one whole tab stop (a `Button`-shaped
        /// `focus()`).
        #[derive(Debug, Default)]
        struct StopWidget;

        impl A11yPrimitive for StopWidget {
            fn a11y_info(&self) -> A11yInfo {
                A11yInfo::default()
            }
        }

        impl Primitive for StopWidget {
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

        impl Interactive for StopWidget {
            type State = ();
            type Action = crate::interactive::Never;

            fn focus(&self) -> FocusSpec {
                FocusSpec::Stop
            }

            fn handle(
                &self,
                _state: &mut (),
                _event: &RoutedEvent,
                _layout: &Rect,
                _cx: &mut EventCx,
            ) -> Handled<crate::interactive::Never> {
                Handled::Ignored
            }
        }

        /// A composite widget (a `DataTable`-shaped `focus()`) with a
        /// fixed number of per-cell stops, in declared order.
        #[derive(Debug)]
        struct ChildrenWidget {
            cells: u64,
        }

        impl A11yPrimitive for ChildrenWidget {
            fn a11y_info(&self) -> A11yInfo {
                A11yInfo::default()
            }
        }

        impl Primitive for ChildrenWidget {
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

        impl Interactive for ChildrenWidget {
            type State = ();
            type Action = crate::interactive::Never;

            fn focus(&self) -> FocusSpec {
                FocusSpec::Children(
                    (0..self.cells)
                        .map(|cell| FocusStop {
                            sub: SubTarget(cell),
                        })
                        .collect(),
                )
            }

            fn handle(
                &self,
                _state: &mut (),
                _event: &RoutedEvent,
                _layout: &Rect,
                _cx: &mut EventCx,
            ) -> Handled<crate::interactive::Never> {
                Handled::Ignored
            }
        }

        /// A widget that never receives keyboard focus (a `Divider`-shaped
        /// `focus()`).
        #[derive(Debug, Default)]
        struct NoneWidget;

        impl A11yPrimitive for NoneWidget {
            fn a11y_info(&self) -> A11yInfo {
                A11yInfo::default()
            }
        }

        impl Primitive for NoneWidget {
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

        crate::non_interactive!(NoneWidget);

        fn rect(y: f32) -> Rect {
            Rect::new(0.0, y, 10.0, 3.0)
        }

        #[test]
        fn empty_frame_tree_has_no_stops() {
            let tree = FocusTree::from_frame_tree(&FrameTree::new());
            assert!(tree.is_empty());
            assert_eq!(tree.len(), 0);
            assert_eq!(tree.focused(), None);
        }

        #[test]
        fn none_widgets_contribute_no_stops() {
            let mut frame = FrameTree::new();
            frame.push(WidgetId::new("divider"), rect(0.0), Box::new(NoneWidget));
            let tree = FocusTree::from_frame_tree(&frame);
            assert!(tree.is_empty());
        }

        #[test]
        fn stop_widget_contributes_one_address_with_no_sub_target() {
            let mut frame = FrameTree::new();
            frame.push(WidgetId::new("button"), rect(0.0), Box::new(StopWidget));
            let tree = FocusTree::from_frame_tree(&frame);
            assert_eq!(tree.tab_order(), vec![(WidgetId::new("button"), None)]);
        }

        /// The entire reason [`FocusTree`] exists instead of reusing
        /// [`crate::frame::ScreenLayout::tab_stops`]'s shape: a composite
        /// widget's [`FocusSpec::Children`] expands into one real stop
        /// per child, each addressed by its own [`SubTarget`], not
        /// collapsed to a single stop for the whole widget.
        #[test]
        fn children_widget_expands_into_one_stop_per_child() {
            let mut frame = FrameTree::new();
            let id = WidgetId::new("table");
            frame.push(id, rect(0.0), Box::new(ChildrenWidget { cells: 3 }));
            let tree = FocusTree::from_frame_tree(&frame);
            assert_eq!(
                tree.tab_order(),
                vec![
                    (id, Some(SubTarget(0))),
                    (id, Some(SubTarget(1))),
                    (id, Some(SubTarget(2))),
                ]
            );
        }

        #[test]
        fn stops_are_sorted_into_reading_order_regardless_of_push_order() {
            let mut frame = FrameTree::new();
            let bottom = WidgetId::new("bottom");
            let top = WidgetId::new("top");
            // Pushed back-to-front (bottom first), the opposite of
            // reading order.
            frame.push(bottom, rect(6.0), Box::new(StopWidget));
            frame.push(top, rect(0.0), Box::new(StopWidget));
            let tree = FocusTree::from_frame_tree(&frame);
            assert_eq!(tree.tab_order(), vec![(top, None), (bottom, None)]);
        }

        #[test]
        fn focus_next_cycles_through_every_expanded_stop() {
            let mut frame = FrameTree::new();
            let id = WidgetId::new("table");
            frame.push(id, rect(0.0), Box::new(ChildrenWidget { cells: 2 }));
            let mut tree = FocusTree::from_frame_tree(&frame);

            assert!(tree.focus_next());
            assert_eq!(tree.focused(), Some(&(id, Some(SubTarget(0)))));
            assert!(tree.focus_next());
            assert_eq!(tree.focused(), Some(&(id, Some(SubTarget(1)))));
            assert!(tree.focus_next(), "wraps back to the first child");
            assert_eq!(tree.focused(), Some(&(id, Some(SubTarget(0)))));
        }

        #[test]
        fn focus_prev_from_none_lands_on_the_last_stop() {
            let mut frame = FrameTree::new();
            let a = WidgetId::new("a");
            let b = WidgetId::new("b");
            frame.push(a, rect(0.0), Box::new(StopWidget));
            frame.push(b, rect(3.0), Box::new(StopWidget));
            let mut tree = FocusTree::from_frame_tree(&frame);

            assert!(tree.focus_prev());
            assert_eq!(tree.focused(), Some(&(b, None)));
        }

        #[test]
        fn focus_next_noop_on_empty_tree() {
            let mut tree = FocusTree::from_frame_tree(&FrameTree::new());
            assert!(!tree.focus_next());
            assert_eq!(tree.focused(), None);
        }

        #[test]
        fn focused_rect_resolves_the_owning_widgets_rect() {
            let mut frame = FrameTree::new();
            let id = WidgetId::new("button");
            frame.push(id, rect(3.0), Box::new(StopWidget));
            let mut tree = FocusTree::from_frame_tree(&frame);
            assert_eq!(tree.focused_rect(), None, "nothing focused yet");

            tree.focus_next();
            assert_eq!(tree.focused_rect(), Some(rect(3.0)));
        }

        #[test]
        fn is_focused_checks_the_full_address_including_sub_target() {
            let mut frame = FrameTree::new();
            let id = WidgetId::new("table");
            frame.push(id, rect(0.0), Box::new(ChildrenWidget { cells: 2 }));
            let mut tree = FocusTree::from_frame_tree(&frame);
            tree.focus_next();

            assert!(tree.is_focused(&id, Some(SubTarget(0))));
            assert!(!tree.is_focused(&id, Some(SubTarget(1))));
            assert!(!tree.is_focused(&id, None));
        }

        /// The acceptance scenario for VS Code's `:focus-visible`
        /// convention: a Tab-reached stop shows the ring, and clicking a
        /// different one suppresses it even though focus did move.
        #[test]
        fn keyboard_move_shows_the_ring_click_suppresses_it() {
            let mut frame = FrameTree::new();
            let a = WidgetId::new("a");
            let b = WidgetId::new("b");
            frame.push(a, rect(0.0), Box::new(StopWidget));
            frame.push(b, rect(3.0), Box::new(StopWidget));
            let mut tree = FocusTree::from_frame_tree(&frame);

            tree.focus_next();
            assert_eq!(tree.focused(), Some(&(a, None)));
            assert!(tree.is_focus_visible(), "Tab landed here, so it's visible");

            tree.set_focus(Some((b, None)));
            assert_eq!(tree.focused(), Some(&(b, None)), "click did move focus");
            assert!(
                !tree.is_focus_visible(),
                "a mouse click must suppress the ring"
            );

            tree.focus_prev();
            assert!(
                tree.is_focus_visible(),
                "a subsequent keyboard move restores the ring"
            );
        }

        #[test]
        fn set_focus_visible_overrides_directly() {
            let mut frame = FrameTree::new();
            let id = WidgetId::new("a");
            frame.push(id, rect(0.0), Box::new(StopWidget));
            let mut tree = FocusTree::from_frame_tree(&frame);

            tree.set_focus(Some((id, None)));
            assert!(!tree.is_focus_visible());
            tree.set_focus_visible(true);
            assert!(tree.is_focus_visible());
        }

        #[test]
        fn clear_resets_focus() {
            let mut frame = FrameTree::new();
            let id = WidgetId::new("a");
            frame.push(id, rect(0.0), Box::new(StopWidget));
            let mut tree = FocusTree::from_frame_tree(&frame);
            tree.focus_next();
            assert!(tree.focused().is_some());

            assert!(tree.clear());
            assert_eq!(tree.focused(), None);
            assert!(!tree.clear(), "clearing an already-clear focus is a no-op");
        }

        #[test]
        fn sync_preserves_focus_by_address_across_rebuild() {
            let mut frame = FrameTree::new();
            let a = WidgetId::new("a");
            let b = WidgetId::new("b");
            frame.push(a, rect(0.0), Box::new(StopWidget));
            frame.push(b, rect(3.0), Box::new(StopWidget));
            let mut tree = FocusTree::from_frame_tree(&frame);
            tree.focus_next();
            tree.focus_next();
            assert_eq!(tree.focused(), Some(&(b, None)));

            // Rebuild with the same widgets in a different push order —
            // geometry moved, but the address is still present.
            let mut rebuilt_frame = FrameTree::new();
            rebuilt_frame.push(b, rect(0.0), Box::new(StopWidget));
            rebuilt_frame.push(a, rect(3.0), Box::new(StopWidget));
            tree.sync(&rebuilt_frame);

            assert_eq!(
                tree.focused(),
                Some(&(b, None)),
                "focus follows the address, not position"
            );
        }

        #[test]
        fn sync_clears_focus_when_address_removed() {
            let mut frame = FrameTree::new();
            let a = WidgetId::new("a");
            let b = WidgetId::new("b");
            frame.push(a, rect(0.0), Box::new(StopWidget));
            frame.push(b, rect(3.0), Box::new(StopWidget));
            let mut tree = FocusTree::from_frame_tree(&frame);
            tree.focus_next();
            tree.focus_next();
            assert_eq!(tree.focused(), Some(&(b, None)));

            let mut rebuilt_frame = FrameTree::new();
            rebuilt_frame.push(a, rect(0.0), Box::new(StopWidget));
            tree.sync(&rebuilt_frame);

            assert_eq!(tree.focused(), None);
        }

        /// `(id, None)` — the only address [`Self::sync_tab_order`] can
        /// ever land on, since the flat `AppLogic::extra_tab_stops` shape
        /// carries no [`SubTarget`] — must survive a [`Self::sync`]
        /// rebuild even for a [`FocusSpec::Children`] widget, whose own
        /// [`Self::from_frame_tree`] stops are never addressed `None`
        /// (see [`children_widget_expands_into_one_stop_per_child`]).
        /// Without this, Tab landing on a `FocusSpec::Children` widget
        /// through `extra_tab_stops` would lose focus on the very next
        /// routed event, since [`crate::runtime::route_widget_event`]
        /// calls [`Self::sync`] before routing every one of them.
        #[test]
        fn sync_keeps_a_whole_widget_address_focused_on_a_children_widget() {
            let id = WidgetId::new("table");
            let mut tree = FocusTree::new();
            tree.set_focus(Some((id, None)));

            let mut frame = FrameTree::new();
            frame.push(id, rect(0.0), Box::new(ChildrenWidget { cells: 3 }));
            tree.sync(&frame);

            assert_eq!(
                tree.focused(),
                Some(&(id, None)),
                "a whole-widget address names the widget regardless of which \
                 per-child stops it rebuilds into"
            );
        }

        /// The flip side of the test above: a whole-widget address for an
        /// id that dropped out of the frame entirely still clears focus —
        /// the relaxed id-only match in [`Self::sync`] only waives the
        /// `sub` field, never the id itself.
        #[test]
        fn sync_still_clears_a_whole_widget_address_when_the_id_is_gone() {
            let id = WidgetId::new("table");
            let other = WidgetId::new("other");
            let mut tree = FocusTree::new();
            tree.set_focus(Some((id, None)));

            let mut frame = FrameTree::new();
            frame.push(other, rect(0.0), Box::new(ChildrenWidget { cells: 3 }));
            tree.sync(&frame);

            assert_eq!(tree.focused(), None);
        }

        /// [`FocusTree::sync_tab_order`] — the flat `(WidgetId, Rect)`
        /// counterpart of [`Self::sync`] that
        /// `crate::runtime::preprocess_event`'s Tab/Shift+Tab step feeds
        /// from `AppLogic::extra_tab_stops` — addresses every stop
        /// `(id, None)` and preserves focus by id.
        #[test]
        fn sync_tab_order_preserves_focus_by_id() {
            let a = WidgetId::new("a");
            let b = WidgetId::new("b");
            let mut tree = FocusTree::new();
            tree.sync_tab_order(&[(a, rect(0.0)), (b, rect(3.0))]);
            tree.focus_next();
            assert_eq!(tree.focused(), Some(&(a, None)));

            // Geometry moved (b now first), but the id is still present.
            tree.sync_tab_order(&[(b, rect(0.0)), (a, rect(3.0))]);
            assert_eq!(
                tree.focused(),
                Some(&(a, None)),
                "focus follows the id, not position"
            );
        }

        #[test]
        fn sync_tab_order_clears_focus_when_id_removed() {
            let a = WidgetId::new("a");
            let b = WidgetId::new("b");
            let mut tree = FocusTree::new();
            tree.sync_tab_order(&[(a, rect(0.0)), (b, rect(3.0))]);
            tree.focus_next();
            tree.focus_next();
            assert_eq!(tree.focused(), Some(&(b, None)));

            tree.sync_tab_order(&[(a, rect(0.0))]);
            assert_eq!(tree.focused(), None);
        }

        /// A composite widget's children are addressed by distinct
        /// [`SubTarget`]s but share the owning widget's single rect — a
        /// rebuild that preserves one child's focus must not also
        /// collapse it onto a sibling child at the same rect.
        #[test]
        fn sync_distinguishes_siblings_sharing_one_rect() {
            let mut frame = FrameTree::new();
            let id = WidgetId::new("table");
            frame.push(id, rect(0.0), Box::new(ChildrenWidget { cells: 2 }));
            let mut tree = FocusTree::from_frame_tree(&frame);
            tree.focus_next();
            tree.focus_next();
            assert_eq!(tree.focused(), Some(&(id, Some(SubTarget(1)))));

            let mut rebuilt_frame = FrameTree::new();
            rebuilt_frame.push(id, rect(0.0), Box::new(ChildrenWidget { cells: 2 }));
            tree.sync(&rebuilt_frame);

            assert_eq!(tree.focused(), Some(&(id, Some(SubTarget(1)))));
        }

        // ─── Modal trapping (`restrict`) ────────────────────────────────

        /// The core "modal trap" scenario: a modal's own stop sits
        /// alongside base-layer stops in the same tree; `restrict` to
        /// just the modal's id narrows cycling down to it alone, so
        /// `focus_next`/`focus_prev` stay confined to the modal and
        /// cannot reach the base layer.
        #[test]
        fn restrict_traps_cycling_to_the_kept_stops() {
            let a = WidgetId::new("base-a");
            let b = WidgetId::new("base-b");
            let modal = WidgetId::new("modal-button");
            let mut tree = FocusTree::new();
            tree.sync_tab_order(&[(a, rect(0.0)), (b, rect(3.0)), (modal, rect(6.0))]);

            tree.restrict(|addr, _rect| addr.0 == modal);
            assert_eq!(tree.tab_order(), vec![(modal, None)]);

            assert!(tree.focus_next());
            assert_eq!(tree.focused(), Some(&(modal, None)));
            assert!(
                !tree.focus_next(),
                "the only kept stop is the one already focused — cycling can't escape to a's or b's"
            );
        }

        /// A geometric predicate is just as valid as an id-based one —
        /// `restrict` hands the stop's rect to the predicate precisely so
        /// a host can trap by containment against a modal's own bounds
        /// ([`crate::ModalEntry::bounds`]) instead of enumerating ids.
        #[test]
        fn restrict_accepts_a_geometric_predicate() {
            let outside = WidgetId::new("outside");
            let inside = WidgetId::new("inside");
            let mut tree = FocusTree::new();
            tree.sync_tab_order(&[(outside, rect(0.0)), (inside, rect(10.0))]);

            let modal_bounds = Rect::new(0.0, 9.0, 10.0, 5.0);
            tree.restrict(|_addr, rect| {
                rect.x >= modal_bounds.x
                    && rect.y >= modal_bounds.y
                    && rect.x < modal_bounds.x + modal_bounds.width
                    && rect.y < modal_bounds.y + modal_bounds.height
            });

            assert_eq!(tree.tab_order(), vec![(inside, None)]);
        }

        /// Restricting away the currently-focused stop clears focus —
        /// the same "fallback" contract [`FocusTree::sync`] already
        /// gives a disappeared stop, not a dangling address pointing at
        /// a stop absent from this tree.
        #[test]
        fn restrict_clears_focus_when_the_focused_stop_is_filtered_out() {
            let a = WidgetId::new("a");
            let b = WidgetId::new("b");
            let mut tree = FocusTree::new();
            tree.sync_tab_order(&[(a, rect(0.0)), (b, rect(3.0))]);
            tree.focus_next();
            assert_eq!(tree.focused(), Some(&(a, None)));

            tree.restrict(|addr, _rect| addr.0 == b);
            assert_eq!(tree.focused(), None, "a's own stop was dropped");
            assert_eq!(tree.tab_order(), vec![(b, None)]);
        }

        /// Restricting around the currently-focused stop leaves it
        /// focused — trapping must not itself move focus.
        #[test]
        fn restrict_preserves_focus_when_the_focused_stop_survives() {
            let a = WidgetId::new("a");
            let b = WidgetId::new("b");
            let mut tree = FocusTree::new();
            tree.sync_tab_order(&[(a, rect(0.0)), (b, rect(3.0))]);
            tree.focus_next();
            assert_eq!(tree.focused(), Some(&(a, None)));

            tree.restrict(|addr, _rect| addr.0 == a);
            assert_eq!(tree.focused(), Some(&(a, None)));
        }
    }
}
