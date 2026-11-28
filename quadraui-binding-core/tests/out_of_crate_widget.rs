//! A widget defined outside both `quadraui` and `quadraui-binding-core`,
//! reached the way a Python or Node host reaches any widget: registered at
//! runtime with `quadraui::widget_kinds::register_widget_kind`, named by
//! kind in a wire layout leaf, decoded through binding-core's wire decode,
//! painted, and driven by input through the binding host's
//! `InteractionEngine`. Nothing here edits either crate, and no Python
//! build is needed to prove the binding path.

use std::cell::RefCell;
use std::sync::Once;

use quadraui::primitive::{DynWidget, LayoutCtx, Primitive};
use quadraui::tui::testing::TuiDriver;
use quadraui::{
    A11yInfo, A11yPrimitive, AppLogic, Backend, EventCx, FocusSpec, Handled, Interactive, Key,
    NamedKey, PaintSurface, PointerKind, Reaction, Rect, RoutedEvent, UiEvent, UserPayload,
    WidgetAction, WidgetId,
};
use quadraui_binding_core::{
    decode_layout_node, paint_layout, resolve_layout_node, EventSink, HitDetail, InteractionEngine,
    LayoutNode, PaintedHit,
};

const KIND: &str = "outside_counter";

/// What [`Counter`] emits inside a [`UiEvent::User`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CounterBumped {
    id: WidgetId,
    from: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Bumped {
    from: u32,
}

impl WidgetAction for Bumped {
    fn into_ui_event(self, id: WidgetId) -> UiEvent {
        UiEvent::User(UserPayload::new(CounterBumped {
            id,
            from: self.from,
        }))
    }
}

/// A focusable counter whose payload a host sends as
/// `{"id": .., "count": n}`.
#[derive(Debug, Clone, serde::Deserialize)]
struct Counter {
    id: WidgetId,
    count: u32,
}

impl A11yPrimitive for Counter {
    fn a11y_info(&self) -> A11yInfo {
        A11yInfo::default()
    }
}

impl Primitive for Counter {
    type Layout = Rect;
    type Event = Bumped;

    fn id(&self) -> Option<&WidgetId> {
        Some(&self.id)
    }

    fn layout(&self, _ctx: &LayoutCtx, rect: Rect) -> Rect {
        rect
    }

    fn paint(&self, layout: &Rect, surface: &mut dyn PaintSurface, env: &quadraui::PaintEnv) {
        surface.surface_draw_text_run(
            *layout,
            &format!("<{}>", self.count),
            env.theme().foreground,
        );
    }

    fn paint_cells(
        &self,
        layout: &Rect,
        cells: &mut dyn quadraui::CellSurface,
        _env: &quadraui::PaintEnv,
    ) {
        let label = format!("<{}>", self.count);
        let x = layout.x + ((layout.width - label.len() as f32) / 2.0).max(0.0).floor();
        let y = layout.y + (layout.height / 2.0).floor();
        cells.put_str_clipped(
            x as u16,
            y as u16,
            &label,
            layout.width as u16,
            quadraui::CellSurfaceStyle::default(),
        );
    }
}

impl Interactive for Counter {
    type State = ();
    type Action = Bumped;

    fn focus(&self) -> FocusSpec {
        FocusSpec::Stop
    }

    fn handle(
        &self,
        _state: &mut (),
        event: &RoutedEvent,
        _layout: &Rect,
        cx: &mut EventCx,
    ) -> Handled<Bumped> {
        let bump = match event {
            RoutedEvent::Pointer {
                kind: PointerKind::Down(quadraui::MouseButton::Left),
                ..
            } => {
                cx.request_focus(None);
                true
            }
            RoutedEvent::Key(key) => matches!(key.key, Key::Named(NamedKey::Enter)),
            _ => false,
        };
        if bump {
            Handled::Event(Bumped { from: self.count })
        } else {
            Handled::Ignored
        }
    }
}

/// Registers [`KIND`] once per test binary, straight into quadraui's
/// runtime kind registry.
fn register_kind() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        quadraui::widget_kinds::register_widget_kind(KIND, |_kind, value| {
            serde_json::from_value::<Counter>(value).map(|c| Box::new(c) as Box<dyn DynWidget>)
        })
        .expect("the outside kind registers");
    });
}

#[derive(Default)]
struct Sink {
    events: Vec<UiEvent>,
}

impl EventSink for Sink {
    fn dispatch(&mut self, event: &UiEvent, _hit: Option<&HitDetail>) -> Reaction {
        self.events.push(event.clone());
        Reaction::Redraw
    }
}

/// A host whose view is a wire tree holding one outside counter, with
/// the count kept host-side the way a Python app keeps its model.
struct Host {
    count: RefCell<u32>,
    engine: InteractionEngine,
    sink: RefCell<Sink>,
}

impl Host {
    fn new() -> Self {
        register_kind();
        Self {
            count: RefCell::new(0),
            engine: InteractionEngine::new(),
            sink: RefCell::new(Sink::default()),
        }
    }

    fn view(&self) -> LayoutNode {
        let wire = serde_json::json!({
            "column": [
                {"id": "c", KIND: {"id": "c", "count": *self.count.borrow()}},
            ]
        });
        decode_layout_node("view", wire).expect("the host view decodes")
    }

    fn bumps(&self) -> Vec<CounterBumped> {
        self.sink
            .borrow()
            .events
            .iter()
            .filter_map(|event| match event {
                UiEvent::User(payload) => payload.downcast_ref::<CounterBumped>().copied(),
                _ => None,
            })
            .collect()
    }
}

impl AppLogic for Host {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let viewport = backend.viewport();
        let area = Rect::new(0.0, 0.0, viewport.width, viewport.height);
        let mut node = self.view();
        resolve_layout_node(&self.engine, backend, area, &mut node);
        let mut hits = Vec::new();
        paint_layout(backend, area, &node, &mut hits);
        quadraui_binding_core::render_overlays(&self.engine, backend, hits);
    }

    fn extra_tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
        self.engine.tab_stops()
    }

    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
        let reaction = self
            .engine
            .handle(event, backend, &mut *self.sink.borrow_mut());
        let bumps = self.bumps().len() as u32;
        *self.count.borrow_mut() = bumps;
        reaction
    }
}

#[test]
fn an_outside_kind_decodes_as_a_generic_widget_leaf() {
    let host = Host::new();
    let LayoutNode::Column(column) = host.view() else {
        panic!("the view root is a column");
    };
    let LayoutNode::Leaf(leaf) = &column.children[0].node else {
        panic!("the counter is a leaf");
    };
    let quadraui_binding_core::LeafDescriptor::Widget { kind, widget } = &leaf.descriptor;
    assert_eq!(*kind, KIND);
    assert_eq!(widget.id(), Some(&WidgetId::new("c")));
    assert_eq!(widget.focus(), FocusSpec::Stop);
}

#[test]
fn an_outside_kind_paints_and_takes_a_click_through_the_binding_host() {
    let mut driver = TuiDriver::new(Host::new(), 20, 4);
    driver.render();
    assert!(driver.screen_contains("<0>"), "{}", driver.screen());

    let area = driver
        .app()
        .engine
        .hit_map()
        .iter()
        .find_map(|hit| match hit {
            PaintedHit::Widget { id, area, .. } if *id == WidgetId::new("c") => Some(*area),
            _ => None,
        })
        .expect("the outside leaf records a generic widget hit");
    driver.click(area.x + 1.0, area.y);

    let id = WidgetId::new("c");
    assert_eq!(driver.app().bumps(), vec![CounterBumped { id, from: 0 }]);
    assert!(
        driver
            .app()
            .sink
            .borrow()
            .events
            .contains(&UiEvent::FocusChanged(Some(id))),
        "the click focuses the outside widget: {:?}",
        driver.app().sink.borrow().events
    );
    driver.render();
    assert!(driver.screen_contains("<1>"), "{}", driver.screen());
}

#[test]
fn tab_and_enter_reach_an_outside_kind_through_the_binding_host() {
    let mut driver = TuiDriver::new(Host::new(), 20, 4);
    driver.render();

    driver.press_named(NamedKey::Tab);
    driver.press_named(NamedKey::Enter);

    assert_eq!(
        driver.app().bumps(),
        vec![CounterBumped {
            id: WidgetId::new("c"),
            from: 0
        }]
    );
}
