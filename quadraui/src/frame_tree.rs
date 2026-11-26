//! [`FrameTree`] — the current frame's [`crate::interactive::Interactive`]
//! widgets, as `(id, rect, widget)` nodes an app hands the runner via
//! [`crate::runner::AppLogic::frame_tree`] so the shared routing engine in
//! `crate::runtime::preprocess_event` can hit-test, focus-route, and
//! deliver [`crate::interactive::RoutedEvent`]s to them without the app
//! writing any dispatch code of its own.
//!
//! # Why this isn't cached across the frame like [`crate::frame::FrameHitMap`]
//!
//! [`crate::frame::ScreenLayout::tab_stops`]/`hit_map` return owned,
//! `'static` data (ids, rects, zone tags) that an app's `render()` caches
//! in a `RefCell` and reads back for the rest of the frame — see
//! `examples/common/frame_demo.rs`'s `cached_hit_map`. A [`FrameTree`]
//! node can't follow that pattern: routing an event to a widget means
//! calling [`crate::primitive::DynWidget::handle`] on the widget *value*
//! itself, not just knowing its id and rect, and that value has no
//! lifetime that outlives one `render()` call (the same reason every
//! other `Primitive` descriptor — a `ListView::new(...)`, a `TextInput`
//! — is rebuilt fresh every frame rather than cached).
//!
//! [`crate::runner::AppLogic::frame_tree`] is therefore called fresh each
//! time the routing engine needs it (once per routed event, not once per
//! paint), exactly mirroring the construction cost `render()` already
//! pays for the same widgets — a `Primitive` descriptor is cheap, data-
//! only state to rebuild; the expensive, must-persist part
//! ([`crate::interactive::Interactive::State`]) lives separately in
//! [`crate::widget_state::WidgetStateStore`], keyed by [`WidgetId`] so it
//! survives the rebuild.
//!
//! # Deriving one from a [`crate::frame::ScreenLayout`]
//!
//! An app that already assembles a [`crate::frame::ScreenLayout`] full of
//! [`crate::frame::Surface::Widget`] entries gets a [`FrameTree`] for
//! free via [`crate::frame::ScreenLayout::into_frame_tree`], which drains
//! exactly those entries (every other `Surface` variant is outside this
//! engine's contract and is discarded). An app with only
//! [`crate::interactive::Interactive`] widgets on screen can build the
//! same layout twice — once to paint, once (cheaply) for routing — or
//! construct a [`FrameTree`] directly via [`FrameTree::push`], whichever
//! reads more naturally for its own `render()`/`frame_tree()` split.

use crate::event::{Point, Rect};
use crate::popup::OpenPopup;
use crate::primitive::DynWidget;
use crate::types::WidgetId;

/// One [`crate::interactive::Interactive`] widget's current-frame
/// placement: its own id, screen rect (in the backend's native units —
/// same contract as every other `Rect` in this crate), and the type-
/// erased widget value itself.
pub struct FrameTreeNode {
    /// This node's own widget id.
    pub id: WidgetId,
    /// This node's bounds, in the backend's native units.
    pub rect: Rect,
    /// The type-erased widget value — see [`DynWidget`].
    pub widget: Box<dyn DynWidget>,
}

/// This frame's widget-contract nodes, in back-to-front z-order, plus
/// the popups those nodes have open — see the module doc. Empty by
/// default, mirroring [`crate::runner::AppLogic::tab_stops`]'s own empty
/// default: an app that never overrides `frame_tree` gets zero behavior
/// change from the routing engine this feeds.
///
/// Popups sit above every node: [`Self::hit`] answers a popup before
/// any node, whatever order the nodes were pushed in. The routing engine
/// fills them in from each node's
/// [`crate::interactive::Interactive::popups`] before it routes an
/// event; an app never pushes them itself.
#[derive(Default)]
pub struct FrameTree {
    nodes: Vec<FrameTreeNode>,
    popups: Vec<OpenPopup>,
}

/// What [`FrameTree::hit`] found under a point.
#[derive(Clone, Copy)]
pub enum FrameHit<'a> {
    /// An open popup — its owner receives the event as
    /// [`crate::interactive::RoutedEvent::PopupEvent`].
    Popup(&'a OpenPopup),
    /// A node, when no popup covers the point.
    Node(&'a FrameTreeNode),
}

impl FrameTree {
    /// An empty tree — no widgets this frame.
    pub fn new() -> Self {
        Self::default()
    }

    /// Append one node. Later pushes paint (and hit-test) on top of
    /// earlier ones, matching [`crate::frame::FrameHitMap`]'s own
    /// back-to-front convention.
    pub fn push(&mut self, id: WidgetId, rect: Rect, widget: Box<dyn DynWidget>) {
        self.nodes.push(FrameTreeNode { id, rect, widget });
    }

    /// Add one open popup, kept in paint order: ascending
    /// [`OpenPopup::z`], ties in push order. A popup with the same owner
    /// and key as one already here replaces it.
    pub fn push_popup(&mut self, popup: OpenPopup) {
        crate::popup::insert_by_z(&mut self.popups, popup);
    }

    /// Every open popup, in paint (back-to-front) order.
    pub fn popups(&self) -> &[OpenPopup] {
        &self.popups
    }

    /// The topmost open popup whose rect contains `point`.
    pub fn popup_at(&self, point: Point) -> Option<&OpenPopup> {
        self.popups.iter().rev().find(|p| p.rect.contains(point))
    }

    /// What is under `point`: the topmost popup containing it, otherwise
    /// the topmost node ([`Self::hit_test`]).
    pub fn hit(&self, point: Point) -> Option<FrameHit<'_>> {
        match self.popup_at(point) {
            Some(popup) => Some(FrameHit::Popup(popup)),
            None => self.hit_test(point).map(FrameHit::Node),
        }
    }

    /// Whether this tree has no widgets at all.
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Every node, in push (back-to-front) order.
    pub fn nodes(&self) -> &[FrameTreeNode] {
        &self.nodes
    }

    /// The topmost node whose rect contains `point` — the last-pushed
    /// match, mirroring [`crate::frame::FrameHitMap::hit_test`]'s
    /// "highest-z wins" rule. `None` when no node contains `point`.
    /// Ignores popups; [`Self::hit`] checks those first.
    pub fn hit_test(&self, point: Point) -> Option<&FrameTreeNode> {
        self.nodes
            .iter()
            .rev()
            .find(|node| node.rect.contains(point))
    }

    /// The node addressed by `id`, regardless of position — the lookup
    /// key-based routing (keyboard focus, pointer capture) uses instead
    /// of [`Self::hit_test`].
    pub fn find(&self, id: &WidgetId) -> Option<&FrameTreeNode> {
        self.nodes.iter().find(|node| &node.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::a11y::{A11yInfo, A11yPrimitive};
    use crate::interactive::{EventCx, FocusSpec, Interactive, RoutedEvent};
    use crate::primitive::{LayoutCtx, Primitive};
    use crate::Handled;

    #[derive(Debug, Default)]
    struct Toy;

    impl A11yPrimitive for Toy {
        fn a11y_info(&self) -> A11yInfo {
            A11yInfo::default()
        }
    }

    impl Primitive for Toy {
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

    crate::non_interactive!(Toy);

    fn node(id: &str, rect: Rect) -> (WidgetId, Rect, Box<dyn DynWidget>) {
        (WidgetId::new(id), rect, Box::new(Toy))
    }

    #[test]
    fn empty_tree_has_no_hits() {
        let tree = FrameTree::new();
        assert!(tree.is_empty());
        assert!(tree.hit_test(Point::new(0.0, 0.0)).is_none());
        assert!(tree.find(&WidgetId::new("anything")).is_none());
    }

    #[test]
    fn hit_test_finds_the_containing_node() {
        let mut tree = FrameTree::new();
        let (id, rect, widget) = node("a", Rect::new(0.0, 0.0, 10.0, 10.0));
        tree.push(id, rect, widget);

        let hit = tree.hit_test(Point::new(5.0, 5.0)).expect("inside rect");
        assert_eq!(hit.id, id);
        assert!(tree.hit_test(Point::new(50.0, 50.0)).is_none());
    }

    #[test]
    fn hit_test_prefers_the_last_pushed_overlapping_node() {
        let mut tree = FrameTree::new();
        let (back_id, back_rect, back_widget) = node("back", Rect::new(0.0, 0.0, 10.0, 10.0));
        let (front_id, front_rect, front_widget) = node("front", Rect::new(0.0, 0.0, 10.0, 10.0));
        tree.push(back_id, back_rect, back_widget);
        tree.push(front_id, front_rect, front_widget);

        let hit = tree.hit_test(Point::new(1.0, 1.0)).unwrap();
        assert_eq!(hit.id, front_id);
    }

    fn open_popup(owner: &str, rect: Rect, z: i32) -> OpenPopup {
        let popup = crate::popup::Popup::new(
            crate::popup::PopupKey(0),
            Rect::default(),
            crate::primitive::Size::new(rect.width, rect.height),
            Toy,
        )
        .z(z)
        .side(crate::popup::PopupSide::Over);
        OpenPopup::resolve(
            WidgetId::new(owner),
            Rect::new(rect.x, rect.y, 0.0, 0.0),
            popup,
            Rect::default(),
        )
    }

    /// A popup answers before a node pushed after its owner: popups sit
    /// above every node.
    #[test]
    fn hit_prefers_a_popup_over_any_node() {
        let mut tree = FrameTree::new();
        let (owner, owner_rect, owner_widget) = node("owner", Rect::new(0.0, 0.0, 10.0, 1.0));
        let (top, top_rect, top_widget) = node("top", Rect::new(0.0, 0.0, 20.0, 20.0));
        tree.push(owner, owner_rect, owner_widget);
        tree.push(top, top_rect, top_widget);
        tree.push_popup(open_popup("owner", Rect::new(0.0, 1.0, 10.0, 3.0), 0));

        match tree.hit(Point::new(2.0, 2.0)) {
            Some(FrameHit::Popup(p)) => assert_eq!(p.owner, owner),
            _ => panic!("the popup should win over the node beneath it"),
        }
        match tree.hit(Point::new(15.0, 2.0)) {
            Some(FrameHit::Node(n)) => assert_eq!(n.id, top),
            _ => panic!("outside the popup the topmost node answers"),
        }
        assert_eq!(tree.hit_test(Point::new(2.0, 2.0)).unwrap().id, top);
    }

    /// Among overlapping popups the highest `z` answers, whatever order
    /// they were pushed in; equal `z` falls back to push order.
    #[test]
    fn popup_at_prefers_the_highest_z_then_the_last_pushed() {
        let mut tree = FrameTree::new();
        let area = Rect::new(0.0, 0.0, 5.0, 5.0);
        tree.push_popup(open_popup("high", area, 2));
        tree.push_popup(open_popup("low", area, 0));
        tree.push_popup(open_popup("mid", area, 1));
        let at = |tree: &FrameTree| tree.popup_at(Point::new(1.0, 1.0)).unwrap().owner;
        assert_eq!(at(&tree), WidgetId::new("high"));
        let order: Vec<_> = tree.popups().iter().map(|p| p.owner).collect();
        assert_eq!(order, ["low", "mid", "high"].map(WidgetId::new).to_vec());

        tree.push_popup(open_popup("late", area, 2));
        assert_eq!(at(&tree), WidgetId::new("late"));
    }

    #[test]
    fn find_looks_up_by_id_regardless_of_position() {
        let mut tree = FrameTree::new();
        let (id, rect, widget) = node("only", Rect::new(100.0, 100.0, 5.0, 5.0));
        tree.push(id, rect, widget);

        assert!(tree.find(&id).is_some());
        assert!(tree.find(&WidgetId::new("missing")).is_none());
    }

    /// Proves a found node's widget is the real, callable [`DynWidget`]
    /// value, not just an id/rect pair — the entire reason [`FrameTree`]
    /// exists instead of reusing [`crate::frame::ScreenLayout::tab_stops`]'s
    /// shape.
    #[test]
    fn found_node_widget_is_handle_callable() {
        #[derive(Debug, Default)]
        struct Clicky;

        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        struct Activated;

        impl crate::interactive::WidgetAction for Activated {
            fn into_ui_event(self, id: WidgetId) -> crate::UiEvent {
                crate::UiEvent::FocusChanged(Some(id))
            }
        }

        impl A11yPrimitive for Clicky {
            fn a11y_info(&self) -> A11yInfo {
                A11yInfo::default()
            }
        }

        impl Primitive for Clicky {
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

        impl Interactive for Clicky {
            type State = ();
            type Action = Activated;

            fn focus(&self) -> FocusSpec {
                FocusSpec::Stop
            }

            fn handle(
                &self,
                _state: &mut (),
                _event: &RoutedEvent,
                _layout: &Rect,
                _cx: &mut EventCx,
            ) -> Handled<Activated> {
                Handled::Event(Activated)
            }
        }

        struct NoopHost;
        impl crate::interactive::EventHost for NoopHost {
            fn is_focused(
                &self,
                _id: &WidgetId,
                _sub: Option<crate::interactive::SubTarget>,
            ) -> bool {
                false
            }
            fn request_focus(
                &mut self,
                _id: &WidgetId,
                _sub: Option<crate::interactive::SubTarget>,
            ) {
            }
            fn capture_pointer(&mut self, _id: &WidgetId) {}
            fn release_pointer(&mut self) {}
            fn set_cursor(&mut self, _cursor: crate::backend::PointerShape) {}
            fn request_redraw(&mut self) {}
            fn emit(&mut self, _event: crate::UiEvent) {}
            fn units(&self) -> crate::primitive::Units {
                crate::primitive::Units::Cells
            }
        }

        let mut tree = FrameTree::new();
        let id = WidgetId::new("clicky");
        tree.push(id, Rect::new(0.0, 0.0, 10.0, 10.0), Box::new(Clicky));

        let found = tree.find(&id).expect("pushed node is found");
        let mut state = found.widget.new_state();
        let mut host = NoopHost;
        let mut cx = EventCx::new(found.id, &mut host);
        let outcome = found.widget.handle(
            state.as_mut(),
            &RoutedEvent::Key(crate::interactive::KeyEvent {
                key: crate::event::Key::Named(crate::event::NamedKey::Enter),
                modifiers: crate::types::Modifiers::default(),
                repeat: false,
            }),
            &*found
                .widget
                .layout(&LayoutCtx::new(1.0, 1.0, false), found.rect),
            &mut cx,
        );
        assert_eq!(
            outcome,
            Handled::Event(crate::UiEvent::FocusChanged(Some(id)))
        );
    }
}
