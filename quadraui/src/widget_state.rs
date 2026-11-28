//! Per-widget [`crate::interactive::Interactive::State`] persistence +
//! hover/capture bookkeeping for the widget-contract routing engine (see
//! [`crate::frame_tree`] and `crate::runtime::preprocess_event`).
//!
//! # Why state needs its own store
//!
//! A [`crate::frame_tree::FrameTree`] node's widget value itself is
//! rebuilt fresh every time an app's [`crate::runner::AppLogic::frame_tree`]
//! is called — the same immediate-mode convention every other `Primitive`
//! descriptor already follows (a `ListView::new(...)` call inside
//! `render()` carries no memory of its own; the app's own fields are the
//! memory). [`crate::interactive::Interactive::State`] is the opposite:
//! it's the thing a widget's own `handle()` mutates between events (a toy
//! counter's count, a composite widget's hover index) and it must survive
//! across every rebuild of the widget descriptor that owns it.
//! [`WidgetStateStore`] is that survival — keyed by [`WidgetId`] so it
//! outlives the short-lived [`crate::frame_tree::FrameTree`] each event
//! builds and tears down.
//!
//! # One store per thread
//!
//! The runtime's store is a per-thread one, reached through
//! [`with_widget_states`]. The routing engine writes it while delivering
//! an event, and [`crate::DrawsPrimitives::draw_dyn`] reads it to hand a
//! widget its own state at paint
//! ([`crate::primitive::PaintEnv::widget_state`]). `draw_dyn` reaches the
//! backend only through `Backend` methods, so a store on the backend
//! would need a new trait method; a per-thread store needs none, and each
//! backend already routes and paints on the one thread that owns its
//! event loop. [`crate::primitive::with_painted_widgets`] and the ambient
//! hover/press state follow the same rule.
//!
//! A widget's state lives as long as the widget keeps appearing in the
//! [`crate::frame_tree::FrameTree`] the engine routes against: every
//! routed event drops the state of any id that tree does not name, so
//! a widget that is removed and later re-added starts from
//! `State::default()`.
//!
//! [`WidgetRoutingState`] holds the two other pieces of cross-event
//! bookkeeping the routing engine needs: which widget is currently
//! hovered (for Enter/Leave synthesis) and which widget, if any, holds an
//! active pointer capture (see
//! [`crate::interactive::EventHost::capture_pointer`]). A backend adds
//! one such field to its [`crate::backend_core::BackendCore`] and exposes
//! it through `crate::runtime::PreprocessBackend::widget_routing_mut` to
//! opt into the engine — see that method's doc for why the default (no
//! field, `None`) is exactly zero behavior change for a backend that
//! hasn't wired it up yet.

use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;

use crate::primitive::DynWidget;
use crate::types::WidgetId;

/// One stored value plus the frame sweep's "seen this frame" mark (see
/// [`WidgetStateStore::gc`]).
struct Slot {
    value: Box<dyn Any>,
    seen: bool,
}

impl Slot {
    fn new(value: Box<dyn Any>) -> Self {
        Self { value, seen: true }
    }
}

/// Persistent per-widget interaction state, type-erased behind `dyn Any`
/// — the same erasure [`DynWidget::new_state`] already produces — and
/// keyed by [`WidgetId`] so it survives across frames. See the module
/// doc for why this can't just live on the widget descriptor itself.
///
/// Besides a widget's own [`crate::interactive::Interactive::State`]
/// ([`Self::state_for`]), a host keeps plain typed values here — a
/// container's scroll offset, a split's ratio — through [`Self::resolve`],
/// [`Self::value`] and [`Self::set`].
///
/// # Controlled / uncontrolled resolution
///
/// A descriptor can optionally carry a value for the same piece of state
/// this store tracks. [`Self::resolve`] decides which one wins:
///
/// - `controlled` is `Some(value)` — the descriptor set the field, so the
///   app's value wins outright. It is also written back into the store, so
///   that if a later frame's descriptor goes back to `None` for the same
///   id, resolution resumes from this value instead of an older stored one.
/// - `controlled` is `None` — the stored value wins: the entry already in
///   the store for `id`, or `default()` the first time `id` is seen (or
///   when the entry was stored under a different concrete type).
///
/// # Frame sweep
///
/// A host that renders frames drops the state of anything that stopped
/// rendering with [`Self::gc`]: every [`Self::state_for`], [`Self::resolve`]
/// and [`Self::set`] marks its id seen, [`Self::mark_seen`] and
/// [`Self::keep_alive`] mark ids without touching their values, and `gc` —
/// called once per frame, after everything still live has been marked —
/// removes every unmarked entry and clears the marks for the next frame.
/// A widget that stops rendering is therefore collected after exactly one
/// frame of absence. [`Self::retain`] is the alternative for a host that
/// already knows the live ids.
#[derive(Default)]
pub struct WidgetStateStore {
    states: HashMap<WidgetId, Slot>,
}

impl WidgetStateStore {
    /// An empty store — no widget has produced state yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// Mutable access to `id`'s state, creating it via
    /// [`DynWidget::new_state`] on first use. `widget` only needs to be
    /// *a* value of the right concrete type (any two `Counter` values
    /// produce the same `State` shape) — it's consulted only on the
    /// first call for a given `id` and never overwrites state already
    /// stored for it. Marks `id` seen this frame.
    pub fn state_for(&mut self, id: WidgetId, widget: &dyn DynWidget) -> &mut dyn Any {
        let slot = self
            .states
            .entry(id)
            .or_insert_with(|| Slot::new(widget.new_state()));
        slot.seen = true;
        slot.value.as_mut()
    }

    /// Resolves `id`'s value of type `V` against the descriptor's
    /// `controlled` value — see "Controlled / uncontrolled resolution" —
    /// and marks `id` seen this frame. An entry stored under a different
    /// concrete type is discarded and reseeded from `default()`.
    pub fn resolve<V>(
        &mut self,
        id: WidgetId,
        controlled: Option<V>,
        default: impl FnOnce() -> V,
    ) -> V
    where
        V: Clone + 'static,
    {
        if let Some(value) = controlled {
            self.states.insert(id, Slot::new(Box::new(value.clone())));
            return value;
        }
        match self.states.get_mut(&id) {
            Some(slot) => {
                slot.seen = true;
                match slot.value.downcast_ref::<V>() {
                    Some(value) => value.clone(),
                    None => {
                        let value = default();
                        slot.value = Box::new(value.clone());
                        value
                    }
                }
            }
            None => {
                let value = default();
                self.states.insert(id, Slot::new(Box::new(value.clone())));
                value
            }
        }
    }

    /// `id`'s stored value of type `V` — `None` when nothing is stored for
    /// `id` or it was stored under a different concrete type. Does not mark
    /// `id` seen.
    pub fn value<V>(&self, id: &WidgetId) -> Option<V>
    where
        V: Clone + 'static,
    {
        self.states
            .get(id)
            .and_then(|slot| slot.value.downcast_ref::<V>())
            .cloned()
    }

    /// Overwrites `id`'s stored value and marks `id` seen this frame.
    pub fn set<V>(&mut self, id: WidgetId, value: V)
    where
        V: 'static,
    {
        self.states.insert(id, Slot::new(Box::new(value)));
    }

    /// Marks `id`'s entry, if any, seen this frame without reading it.
    pub fn mark_seen(&mut self, id: &WidgetId) {
        if let Some(slot) = self.states.get_mut(id) {
            slot.seen = true;
        }
    }

    /// Marks every entry whose id `keep` accepts as seen this frame,
    /// without reading, resolving or reseeding it — for state owned by a
    /// subtree that has no geometry this frame (an unselected tab, a
    /// scroll child outside the viewport) but must survive [`Self::gc`]
    /// until it is shown again.
    pub fn keep_alive(&mut self, keep: impl Fn(&str) -> bool) {
        for (id, slot) in self.states.iter_mut() {
            if keep(id.as_str()) {
                slot.seen = true;
            }
        }
    }

    /// Drops every entry not marked seen since the last `gc`, then clears
    /// the marks for the next frame — see "Frame sweep".
    pub fn gc(&mut self) {
        self.states.retain(|_, slot| slot.seen);
        for slot in self.states.values_mut() {
            slot.seen = false;
        }
    }

    /// Drop every stored state whose id does not satisfy `is_live` —
    /// call once per frame with the current
    /// [`crate::frame_tree::FrameTree`]'s ids (`|id| tree.find(id).is_some()`)
    /// so a widget that stopped being rendered doesn't leak its state
    /// forever.
    pub fn retain(&mut self, mut is_live: impl FnMut(&WidgetId) -> bool) {
        self.states.retain(|id, _| is_live(id));
    }

    /// `id`'s stored state, or `None` when it has none yet.
    pub fn get(&self, id: &WidgetId) -> Option<&dyn Any> {
        self.states.get(id).map(|slot| slot.value.as_ref())
    }

    /// Whether `id` has stored state.
    pub fn contains(&self, id: &WidgetId) -> bool {
        self.states.contains_key(id)
    }

    /// Drop `id`'s stored state, so its next use starts from
    /// `State::default()`.
    pub fn remove(&mut self, id: &WidgetId) {
        self.states.remove(id);
    }

    /// Moves `id`'s stored value out of the store — for a host that keeps
    /// a widget's state in a store of its own and lends it to this
    /// thread's runtime store (see [`with_widget_states`]) for one paint,
    /// so [`crate::DrawsPrimitives::draw_dyn`] hands the widget its state.
    pub fn take(&mut self, id: &WidgetId) -> Option<Box<dyn Any>> {
        self.states.remove(id).map(|slot| slot.value)
    }

    /// Stores `state` as `id`'s value, replacing any — the counterpart of
    /// [`Self::take`]. Marks `id` seen this frame.
    pub fn insert(&mut self, id: WidgetId, state: Box<dyn Any>) {
        self.states.insert(id, Slot::new(state));
    }

    /// Drop every stored state.
    pub fn clear(&mut self) {
        self.states.clear();
    }

    /// Whether any widget has stored state.
    pub fn is_empty(&self) -> bool {
        self.states.is_empty()
    }

    /// How many widgets have stored state.
    pub fn len(&self) -> usize {
        self.states.len()
    }
}

thread_local! {
    /// This thread's runtime store — see the module doc's "One store per
    /// thread".
    static WIDGET_STATES: RefCell<WidgetStateStore> = RefCell::new(WidgetStateStore::new());
}

/// Run `f` against this thread's runtime [`WidgetStateStore`] — the one
/// the routing engine delivers events against and
/// [`crate::DrawsPrimitives::draw_dyn`] paints from.
///
/// `f` must not paint or route: both read this same store, and a nested
/// borrow panics. Copy out what is needed and act after the call returns.
pub fn with_widget_states<R>(f: impl FnOnce(&mut WidgetStateStore) -> R) -> R {
    WIDGET_STATES.with(|s| f(&mut s.borrow_mut()))
}

/// Run `f` with `id`'s stored state from this thread's runtime store, or
/// `None` when `id` is `None`, has no state yet, or the store is already
/// borrowed further up the stack.
pub(crate) fn with_widget_state_ref<R>(
    id: Option<&WidgetId>,
    f: impl FnOnce(Option<&dyn Any>) -> R,
) -> R {
    WIDGET_STATES.with(|s| match (id, s.try_borrow()) {
        (Some(id), Ok(store)) => f(store.get(id)),
        _ => f(None),
    })
}

/// The widget-contract routing engine's cross-event pointer bookkeeping:
/// which widget is hovered/captured. Per-widget state lives in this
/// thread's [`WidgetStateStore`] instead — see the module doc.
#[derive(Default)]
pub struct WidgetRoutingState {
    /// The widget the pointer was last known to be over, for Enter/Leave
    /// synthesis on the next `MouseMoved`. `None` means the pointer is
    /// over no routed widget (or hasn't moved yet).
    pub hovered: Option<WidgetId>,
    /// The widget currently holding an active pointer capture, if any —
    /// see [`crate::interactive::EventHost::capture_pointer`]. While set,
    /// every pointer event routes to this widget regardless of where the
    /// pointer actually is.
    pub captured: Option<WidgetId>,
}

impl WidgetRoutingState {
    /// Fresh bookkeeping — nothing hovered or captured.
    pub fn new() -> Self {
        Self::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::Rect;
    use crate::interactive::{EventCx, EventHost, FocusSpec, Never, RoutedEvent};
    use crate::primitive::{LayoutCtx, Units};
    use crate::Handled;

    /// A toy widget whose `State` is just a counter, mirroring
    /// `crate::interactive`'s own test fixture — kept local here rather
    /// than shared, since it only needs to exist long enough to prove
    /// `WidgetStateStore` persists it.
    #[derive(Debug, Default)]
    struct Toy;

    #[derive(Debug, Default, PartialEq)]
    struct ToyState {
        hits: u32,
    }

    impl crate::a11y::A11yPrimitive for Toy {
        fn a11y_info(&self) -> crate::a11y::A11yInfo {
            crate::a11y::A11yInfo::default()
        }
    }

    impl crate::primitive::Primitive for Toy {
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

    impl crate::interactive::Interactive for Toy {
        type State = ToyState;
        type Action = Never;

        fn focus(&self) -> FocusSpec {
            FocusSpec::Stop
        }

        fn handle(
            &self,
            state: &mut ToyState,
            _event: &RoutedEvent,
            _layout: &Rect,
            _cx: &mut EventCx,
        ) -> Handled<Never> {
            state.hits += 1;
            Handled::Consumed
        }
    }

    #[derive(Debug, Default)]
    struct MockHost;

    impl EventHost for MockHost {
        fn is_focused(&self, _id: &WidgetId, _sub: Option<crate::interactive::SubTarget>) -> bool {
            false
        }
        fn request_focus(&mut self, _id: &WidgetId, _sub: Option<crate::interactive::SubTarget>) {}
        fn capture_pointer(&mut self, _id: &WidgetId) {}
        fn release_pointer(&mut self) {}
        fn set_cursor(&mut self, _cursor: crate::backend::PointerShape) {}
        fn request_redraw(&mut self) {}
        fn emit(&mut self, _event: crate::UiEvent) {}
        fn units(&self) -> Units {
            Units::Cells
        }
    }

    #[test]
    fn controlled_value_wins_over_a_stored_uncontrolled_one() {
        let mut store = WidgetStateStore::new();
        let id = WidgetId::new("widget-state-test:controlled-wins");
        assert_eq!(store.resolve(id, None::<i32>, || 1), 1);
        assert_eq!(store.resolve(id, Some(99), || unreachable!()), 99);
    }

    #[test]
    fn controlled_value_is_written_back_so_a_later_uncontrolled_frame_resumes_from_it() {
        let mut store = WidgetStateStore::new();
        let id = WidgetId::new("widget-state-test:controlled-writes-back");
        assert_eq!(store.resolve(id, Some(42), || 0), 42);
        assert_eq!(store.resolve(id, None::<i32>, || unreachable!()), 42);
    }

    #[test]
    fn uncontrolled_value_persists_across_frames_without_recomputing_default() {
        let mut store = WidgetStateStore::new();
        let id = WidgetId::new("widget-state-test:persists");
        let calls = std::cell::Cell::new(0);
        let seed = || {
            calls.set(calls.get() + 1);
            7
        };
        assert_eq!(store.resolve(id, None::<i32>, seed), 7);
        assert_eq!(store.resolve(id, None::<i32>, || unreachable!()), 7);
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn set_overwrites_and_value_reads_back_by_type() {
        let mut store = WidgetStateStore::new();
        let id = WidgetId::new("widget-state-test:set-get");
        store.set(id, "hello".to_string());
        assert_eq!(store.value::<String>(&id), Some("hello".to_string()));
        assert_eq!(store.value::<i32>(&id), None);
    }

    #[test]
    fn gc_keeps_an_id_marked_this_frame_and_resets_its_mark() {
        let mut store = WidgetStateStore::new();
        let id = WidgetId::new("widget-state-test:gc-keeps");
        store.resolve(id, None::<i32>, || 5);
        store.gc();
        assert_eq!(store.value::<i32>(&id), Some(5));
        store.gc();
        assert_eq!(store.value::<i32>(&id), None);
    }

    #[test]
    fn gc_keeps_widget_state_only_while_it_is_marked() {
        let mut store = WidgetStateStore::new();
        let id = WidgetId::new("toy:gc");
        store.state_for(id, &Toy);
        store.gc();
        store.mark_seen(&id);
        store.gc();
        assert!(store.contains(&id));
        store.gc();
        assert!(!store.contains(&id));
    }

    #[test]
    fn keep_alive_marks_matching_ids_without_touching_their_values() {
        let mut store = WidgetStateStore::new();
        let kept = WidgetId::new("widget-state-test:kept");
        let dropped = WidgetId::new("widget-state-test:dropped");
        store.set(kept, 1_i32);
        store.set(dropped, 2_i32);
        store.gc();
        store.keep_alive(|id| id.ends_with(":kept"));
        store.gc();
        assert_eq!(store.value::<i32>(&kept), Some(1));
        assert!(!store.contains(&dropped));
    }

    #[test]
    fn take_and_insert_move_a_value_between_stores() {
        let mut lender = WidgetStateStore::new();
        let mut borrower = WidgetStateStore::new();
        let id = WidgetId::new("toy:lent");
        lender.state_for(id, &Toy);
        let state = lender.take(&id).expect("the lender holds the state");
        assert!(!lender.contains(&id));
        borrower.insert(id, state);
        assert!(borrower
            .get(&id)
            .unwrap()
            .downcast_ref::<ToyState>()
            .is_some());
    }

    #[test]
    fn resolve_reseeds_from_default_when_the_stored_type_changes() {
        let mut store = WidgetStateStore::new();
        let id = WidgetId::new("widget-state-test:type-change");
        store.set(id, 123_i32);
        assert_eq!(
            store.resolve(id, None::<String>, || "fresh".to_string()),
            "fresh".to_string()
        );
        assert_eq!(store.value::<String>(&id), Some("fresh".to_string()));
        assert_eq!(store.value::<i32>(&id), None);
    }

    #[test]
    fn state_for_creates_default_state_on_first_use() {
        let mut store = WidgetStateStore::new();
        let id = WidgetId::new("toy:1");
        let state = store
            .state_for(id, &Toy)
            .downcast_mut::<ToyState>()
            .expect("Toy::State is ToyState");
        assert_eq!(*state, ToyState::default());
    }

    #[test]
    fn state_for_persists_mutations_across_calls() {
        let mut store = WidgetStateStore::new();
        let id = WidgetId::new("toy:2");
        let mut host = MockHost;
        let mut cx = EventCx::new(id, &mut host);

        {
            let state = store
                .state_for(id, &Toy)
                .downcast_mut::<ToyState>()
                .unwrap();
            let outcome = Toy.handle(
                state,
                &RoutedEvent::FocusIn(None),
                &Rect::default(),
                &mut cx,
            );
            assert_eq!(outcome, Handled::Consumed);
        }

        // A fresh `Toy` value (as `frame_tree()` would construct every
        // call) must not reset the stored state.
        let state = store
            .state_for(id, &Toy)
            .downcast_mut::<ToyState>()
            .unwrap();
        assert_eq!(state.hits, 1);
    }

    #[test]
    fn retain_drops_state_for_ids_the_predicate_rejects() {
        let mut store = WidgetStateStore::new();
        let a = WidgetId::new("toy:a");
        let b = WidgetId::new("toy:b");
        store.state_for(a, &Toy);
        store.state_for(b, &Toy);
        assert_eq!(store.len(), 2);

        store.retain(|id| *id == a);
        assert_eq!(store.len(), 1);
        assert!(!store.is_empty());
    }

    #[test]
    fn get_contains_and_remove_address_one_id() {
        let mut store = WidgetStateStore::new();
        let id = WidgetId::new("toy:get");
        assert!(store.get(&id).is_none());
        store.state_for(id, &Toy);
        assert!(store.contains(&id));
        assert!(store.get(&id).unwrap().downcast_ref::<ToyState>().is_some());
        store.remove(&id);
        assert!(!store.contains(&id));
    }

    /// The runtime store is per thread, and a read through
    /// `with_widget_state_ref` sees what `with_widget_states` wrote.
    #[test]
    fn the_runtime_store_is_shared_within_a_thread_and_private_to_it() {
        let id = WidgetId::new("toy:thread");
        with_widget_states(|s| {
            s.state_for(id, &Toy)
                .downcast_mut::<ToyState>()
                .unwrap()
                .hits = 4;
        });
        let hits = with_widget_state_ref(Some(&id), |state| {
            state
                .and_then(|s| s.downcast_ref::<ToyState>())
                .map(|s| s.hits)
        });
        assert_eq!(hits, Some(4));
        let elsewhere = std::thread::spawn(move || with_widget_states(|s| s.contains(&id)))
            .join()
            .unwrap();
        assert!(!elsewhere);
        assert!(with_widget_state_ref(None, |state| state.is_none()));
    }

    #[test]
    fn widget_routing_state_defaults_to_nothing_hovered_or_captured() {
        let routing = WidgetRoutingState::new();
        assert!(routing.hovered.is_none());
        assert!(routing.captured.is_none());
    }
}
