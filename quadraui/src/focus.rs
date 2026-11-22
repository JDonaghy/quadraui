//! `FocusManager` — the single owner of "what widget currently has
//! keyboard focus".
//!
//! # Ownership: one per backend, one writer
//!
//! Each concrete backend (`TuiBackend`, `GtkBackend`, `MacBackend`,
//! `WinBackend`) owns exactly one `FocusManager`, reachable read-only
//! via [`crate::Backend::focus_manager`]. The only code that *mutates*
//! it is the shared Tab/Shift+Tab intercept in
//! [`crate::runtime::preprocess_event`] (via the crate-private
//! `PreprocessBackend::focus_manager_mut`) — every backend's `run.rs`
//! routes through that one function, so "what's focused" can't drift
//! between backends. Apps never construct or mutate a `FocusManager`
//! directly; they read it off the backend, and drive it indirectly by
//! returning tab stops from [`crate::runner::AppLogic::tab_stops`].
//!
//! # Opt-in, not forced
//!
//! An app participates purely by overriding
//! [`crate::runner::AppLogic::tab_stops`] (default: empty). While it
//! returns `[]`, Tab/Shift+Tab pass through to `AppLogic::handle`
//! completely unclaimed — **zero behavior change** for every app
//! written before this feature. Once an app returns a non-empty list
//! from `tab_stops`, the runner claims Tab/Shift+Tab globally: they
//! stop reaching `AppLogic::handle` as a raw `KeyPressed` and arrive
//! instead as [`crate::UiEvent::FocusChanged`]. An app with a widget
//! that treats a literal Tab keystroke as input (a code editor's
//! indent command, say) should leave that widget out of `tab_stops`
//! entirely, or keep `tab_stops` empty while that widget holds
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
//! `focus_manager().focused()` names a widget this frame's tab stops
//! can still resolve a rect for.

use crate::event::Rect;
use crate::frame_tree::FrameTree;
use crate::interactive::{FocusSpec, SubTarget};
use crate::types::WidgetId;

/// Wrap `current + delta` into `0..n`, the one index-cycling arithmetic
/// every Tab/Shift+Tab-style stepper in this crate shares — [`FocusManager`],
/// [`crate::compose::focus_group::FocusGroup`], and the menu/workspace/
/// dialog/toast/form controllers that step a bounded index with wrap-around.
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

// Focus-ring stroke width is not a constant in this module: it is the
// app-overridable [`crate::Style::focus_ring_width`] token (native
// units — GTK/macOS/Win-GUI pixels), which every `PaintSurface`-backed
// pixel backend's `Backend::draw_focus_ring` reads via
// `Backend::style()` so all three paint the same weight without three
// independent literals to keep in sync. See that field's doc for the
// TUI story (`crate::tui::draw_focus_ring` paints a 1-cell border and
// consumes no `Style` token).

/// Single owner of keyboard focus. See the module doc for
/// the full rationale and the six representations this replaces (or, in
/// three documented cases, deliberately leaves for a later migration).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FocusManager {
    focused: Option<WidgetId>,
    tab_order: Vec<WidgetId>,
}

impl FocusManager {
    /// A manager with nothing focused and an empty tab order.
    pub fn new() -> Self {
        Self::default()
    }

    /// Rebuild the tab order from this frame's tab stops — typically
    /// [`crate::frame::ScreenLayout::tab_stops`]'s output, already
    /// sorted in reading order.
    ///
    /// If the previously-focused widget's id is still present in the
    /// new order, focus is preserved (matched **by id**, not position —
    /// a widget that moved on screen keeps focus; one that vanished
    /// doesn't hand focus to whatever now sits at its old index).
    /// Otherwise focus is cleared.
    pub fn sync_tab_order(&mut self, stops: &[(WidgetId, Rect)]) {
        self.tab_order = stops.iter().map(|(id, _)| *id).collect();
        if let Some(cur) = &self.focused {
            if !self.tab_order.contains(cur) {
                self.focused = None;
            }
        }
    }

    /// The current tab order, in cycle order.
    pub fn tab_order(&self) -> &[WidgetId] {
        &self.tab_order
    }

    /// The currently focused widget, if any.
    pub fn focused(&self) -> Option<&WidgetId> {
        self.focused.as_ref()
    }

    /// Whether `id` currently has focus.
    pub fn is_focused(&self, id: &WidgetId) -> bool {
        self.focused.as_ref() == Some(id)
    }

    /// Move focus to the next tab stop, wrapping around. Starting from
    /// "nothing focused", lands on the first stop. No-op (returns
    /// `false`) if the tab order is empty. Returns whether focus
    /// actually changed.
    pub fn focus_next(&mut self) -> bool {
        self.step(1)
    }

    /// Move focus to the previous tab stop, wrapping around. Starting
    /// from "nothing focused", lands on the last stop. No-op (returns
    /// `false`) if the tab order is empty. Returns whether focus
    /// actually changed.
    pub fn focus_prev(&mut self) -> bool {
        self.step(-1)
    }

    fn step(&mut self, dir: isize) -> bool {
        if self.tab_order.is_empty() {
            return self.set_focus(None);
        }
        let len = self.tab_order.len();
        let next_idx = match &self.focused {
            None if dir >= 0 => 0,
            None => len - 1,
            Some(cur) => {
                let idx = self.tab_order.iter().position(|w| w == cur).unwrap_or(0);
                cycle_index(idx, dir, len)
            }
        };
        let next = self.tab_order[next_idx];
        self.set_focus(Some(next))
    }

    /// Set focus explicitly — e.g. click-to-focus. `None` clears focus.
    /// Returns whether it actually changed.
    pub fn set_focus(&mut self, id: Option<WidgetId>) -> bool {
        if self.focused != id {
            self.focused = id;
            true
        } else {
            false
        }
    }

    /// Clear focus. Equivalent to `set_focus(None)`.
    pub fn clear(&mut self) -> bool {
        self.set_focus(None)
    }
}

/// A tab stop's full address: a widget's own id, plus — for a composite
/// widget's [`FocusSpec::Children`] — which sub-part this stop names.
/// `None` addresses the whole widget, the same shape
/// [`crate::interactive::EventHost::is_focused`]/`request_focus` take.
pub type FocusAddress = (WidgetId, Option<SubTarget>);

struct FocusTreeStop {
    address: FocusAddress,
    rect: Rect,
}

/// The real, per-stop tab order for one frame's [`FrameTree`], plus the
/// `:focus-visible` bookkeeping [`crate::compose::FocusRing`] already
/// tracks for a flat `WidgetId` list.
///
/// # Why this exists alongside [`FocusManager`]
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
/// as [`FocusManager::step`]: [`cycle_index`].
///
/// # Focus-visible
///
/// Mirrors [`crate::compose::FocusRing`]'s VS Code `:focus-visible`
/// convention: [`Self::focus_next`]/[`Self::focus_prev`] (a keyboard
/// move) mark the ring visible; [`Self::set_focus`] (the click-to-focus
/// path) suppresses it. See that type's doc for how a host composes
/// `tree.is_focused(id, sub) && tree.is_focus_visible()` into a
/// primitive's own `has_focus`/`focused` descriptor field.
#[derive(Default)]
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

    /// Rebuild the tab order from this frame's [`FrameTree`] in place,
    /// the [`FocusAddress`]-keyed counterpart of
    /// [`FocusManager::sync_tab_order`]: if the previously-focused
    /// address is still present in the rebuilt order, focus (and its
    /// current focus-visible state) is preserved; otherwise focus is
    /// cleared.
    pub fn sync(&mut self, tree: &FrameTree) {
        let rebuilt = Self::from_frame_tree(tree);
        let keep_focus = self
            .focused
            .as_ref()
            .is_some_and(|cur| rebuilt.stops.iter().any(|stop| &stop.address == cur));
        self.stops = rebuilt.stops;
        if !keep_focus {
            self.focused = None;
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

    fn stops(ids: &[&str]) -> Vec<(WidgetId, Rect)> {
        ids.iter()
            .enumerate()
            .map(|(i, id)| {
                (
                    WidgetId::new(*id),
                    Rect::new(0.0, i as f32 * 3.0, 10.0, 3.0),
                )
            })
            .collect()
    }

    #[test]
    fn new_manager_has_no_focus() {
        let fm = FocusManager::new();
        assert_eq!(fm.focused(), None);
        assert!(fm.tab_order().is_empty());
    }

    #[test]
    fn sync_tab_order_populates_order() {
        let mut fm = FocusManager::new();
        fm.sync_tab_order(&stops(&["a", "b", "c"]));
        assert_eq!(
            fm.tab_order(),
            &[WidgetId::new("a"), WidgetId::new("b"), WidgetId::new("c")]
        );
    }

    #[test]
    fn focus_next_from_none_lands_on_first() {
        let mut fm = FocusManager::new();
        fm.sync_tab_order(&stops(&["a", "b", "c"]));
        assert!(fm.focus_next());
        assert_eq!(fm.focused(), Some(&WidgetId::new("a")));
    }

    #[test]
    fn focus_prev_from_none_lands_on_last() {
        let mut fm = FocusManager::new();
        fm.sync_tab_order(&stops(&["a", "b", "c"]));
        assert!(fm.focus_prev());
        assert_eq!(fm.focused(), Some(&WidgetId::new("c")));
    }

    #[test]
    fn focus_next_wraps() {
        let mut fm = FocusManager::new();
        fm.sync_tab_order(&stops(&["a", "b", "c"]));
        fm.focus_next(); // a
        fm.focus_next(); // b
        fm.focus_next(); // c
        assert_eq!(fm.focused(), Some(&WidgetId::new("c")));
        fm.focus_next(); // wraps to a
        assert_eq!(fm.focused(), Some(&WidgetId::new("a")));
    }

    #[test]
    fn focus_prev_wraps() {
        let mut fm = FocusManager::new();
        fm.sync_tab_order(&stops(&["a", "b", "c"]));
        fm.focus_next(); // a
        fm.focus_prev(); // wraps to c
        assert_eq!(fm.focused(), Some(&WidgetId::new("c")));
    }

    #[test]
    fn focus_next_noop_on_empty_order() {
        let mut fm = FocusManager::new();
        assert!(!fm.focus_next());
        assert_eq!(fm.focused(), None);
    }

    #[test]
    fn set_focus_returns_whether_changed() {
        let mut fm = FocusManager::new();
        assert!(fm.set_focus(Some(WidgetId::new("a"))));
        assert!(
            !fm.set_focus(Some(WidgetId::new("a"))),
            "same id is a no-op"
        );
        assert!(fm.set_focus(Some(WidgetId::new("b"))));
        assert!(fm.clear());
        assert!(!fm.clear(), "clearing an already-clear focus is a no-op");
    }

    #[test]
    fn sync_tab_order_preserves_focus_by_id() {
        let mut fm = FocusManager::new();
        fm.sync_tab_order(&stops(&["a", "b", "c"]));
        fm.set_focus(Some(WidgetId::new("b")));
        // Geometry moved (b now at index 0), but the id is still present.
        fm.sync_tab_order(&stops(&["b", "a", "c"]));
        assert_eq!(
            fm.focused(),
            Some(&WidgetId::new("b")),
            "focus follows the id, not the index"
        );
    }

    #[test]
    fn sync_tab_order_clears_focus_when_id_removed() {
        let mut fm = FocusManager::new();
        fm.sync_tab_order(&stops(&["a", "b", "c"]));
        fm.set_focus(Some(WidgetId::new("b")));
        fm.sync_tab_order(&stops(&["a", "c"]));
        assert_eq!(fm.focused(), None);
    }

    #[test]
    fn is_focused_checks_current_id() {
        let mut fm = FocusManager::new();
        fm.sync_tab_order(&stops(&["a", "b"]));
        fm.set_focus(Some(WidgetId::new("a")));
        assert!(fm.is_focused(&WidgetId::new("a")));
        assert!(!fm.is_focused(&WidgetId::new("b")));
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

    #[test]
    fn single_stop_next_and_prev_stay_put() {
        let mut fm = FocusManager::new();
        fm.sync_tab_order(&stops(&["only"]));
        assert!(fm.focus_next());
        assert_eq!(fm.focused(), Some(&WidgetId::new("only")));
        assert!(
            !fm.focus_next(),
            "focusing the same single stop again is a no-op"
        );
        assert!(!fm.focus_prev());
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
                _theme: &crate::theme::Theme,
                _style: &crate::style::Style,
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
                _theme: &crate::theme::Theme,
                _style: &crate::style::Style,
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
                _theme: &crate::theme::Theme,
                _style: &crate::style::Style,
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
        /// convention, carried over from [`crate::compose::FocusRing`]:
        /// a Tab-reached stop shows the ring, and clicking a different
        /// one suppresses it even though focus did move.
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
    }
}
