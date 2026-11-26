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
//! [`WidgetRoutingState`] bundles the store with the two other pieces of
//! cross-event bookkeeping the routing engine needs: which widget is
//! currently hovered (for Enter/Leave synthesis) and which widget, if
//! any, holds an active pointer capture (see
//! [`crate::interactive::EventHost::capture_pointer`]). A backend adds
//! one such field to its [`crate::backend_core::BackendCore`] and exposes
//! it through `crate::runtime::PreprocessBackend::widget_routing_mut` to
//! opt into the engine — see that method's doc for why the default (no
//! field, `None`) is exactly zero behavior change for a backend that
//! hasn't wired it up yet.

use std::any::Any;
use std::collections::HashMap;

use crate::primitive::DynWidget;
use crate::types::WidgetId;

/// Persistent per-widget interaction state, type-erased behind `dyn Any`
/// — the same erasure [`DynWidget::new_state`] already produces — and
/// keyed by [`WidgetId`] so it survives across frames. See the module
/// doc for why this can't just live on the widget descriptor itself.
#[derive(Default)]
pub struct WidgetStateStore {
    states: HashMap<WidgetId, Box<dyn Any>>,
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
    /// stored for it.
    pub fn state_for(&mut self, id: WidgetId, widget: &dyn DynWidget) -> &mut dyn Any {
        self.states
            .entry(id)
            .or_insert_with(|| widget.new_state())
            .as_mut()
    }

    /// Drop every stored state whose id does not satisfy `is_live` —
    /// call once per frame with the current
    /// [`crate::frame_tree::FrameTree`]'s ids (`|id| tree.find(id).is_some()`)
    /// so a widget that stopped being rendered doesn't leak its state
    /// forever.
    pub fn retain(&mut self, mut is_live: impl FnMut(&WidgetId) -> bool) {
        self.states.retain(|id, _| is_live(id));
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

/// The widget-contract routing engine's full cross-event bookkeeping:
/// [`WidgetStateStore`] plus which widget is hovered/captured. See the
/// module doc.
#[derive(Default)]
pub struct WidgetRoutingState {
    /// Persistent per-widget interaction state — see [`WidgetStateStore`].
    pub states: WidgetStateStore,
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
    /// Fresh bookkeeping — no state, nothing hovered or captured.
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
    fn widget_routing_state_defaults_to_nothing_hovered_or_captured() {
        let routing = WidgetRoutingState::new();
        assert!(routing.hovered.is_none());
        assert!(routing.captured.is_none());
        assert!(routing.states.is_empty());
    }
}
