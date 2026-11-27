//! A host binding built only from `quadraui` and `quadraui-binding-core`'s
//! public API, with no `quadraui-py`/`quadraui-node` code in the loop: the
//! shape a third binding (a Lua host binding `binding-core` directly)
//! takes. Each frame decodes a wire layout tree, resolves the engine's
//! stored widget state into it, paints it through a `TuiBackend`, and
//! routes input back through the engine to a host-side event sink.
//!
//! The tree mixes a built-in kind (`"button"`) with a kind the host
//! registers at runtime (`"host_toggle"`), so the same frame proves both
//! halves of the generic leaf: the static `widget_kinds!` table and the
//! runtime kind registry.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Once;

use quadraui::primitive::DynWidget;
use quadraui::tui::testing::TuiDriver;
use quadraui::{AppLogic, Backend, FormEvent, Reaction, Rect, UiEvent, WidgetId};
use quadraui_binding_core::{
    decode_layout_node, paint_layout_windowed, resolve_layout_node_windowed, widget_leaf_kinds,
    EventSink, HitDetail, InteractionEngine, LayoutNode,
};

const HOST_KIND: &str = "host_toggle";

/// Registers [`HOST_KIND`] once per test binary: a host-defined kind name
/// whose payload decodes to a core `Checkbox`.
fn register_host_kind() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        quadraui::register_widget_kind(HOST_KIND, |value| {
            serde_json::from_value::<quadraui::Checkbox>(value)
                .map(|checkbox| Box::new(checkbox) as Box<dyn DynWidget>)
        })
        .expect("host kind registers");
    });
}

/// The host's callback boundary: records every event the engine delivers.
#[derive(Default)]
struct HostSink {
    events: Vec<UiEvent>,
}

impl EventSink for HostSink {
    fn dispatch(&mut self, event: &UiEvent, _hit: Option<&HitDetail>) -> Reaction {
        self.events.push(event.clone());
        Reaction::Redraw
    }
}

/// A host app whose view is a wire tree, decoded fresh every frame the way
/// a scripting host hands one over from its own `view()` function.
struct HostApp {
    view: serde_json::Value,
    engine: InteractionEngine,
    sink: RefCell<HostSink>,
}

impl HostApp {
    fn new(view: serde_json::Value) -> Self {
        register_host_kind();
        Self {
            view,
            engine: InteractionEngine::new(),
            sink: RefCell::new(HostSink::default()),
        }
    }

    fn decode(&self) -> LayoutNode {
        decode_layout_node("view", self.view.clone()).expect("the host view decodes")
    }
}

impl AppLogic for HostApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let viewport = backend.viewport();
        let area = Rect::new(0.0, 0.0, viewport.width, viewport.height);
        let mut node = self.decode();
        let windows = HashMap::new();
        resolve_layout_node_windowed(
            self.engine.widget_state(),
            backend,
            area,
            &mut node,
            &windows,
        );
        let mut hits = Vec::new();
        paint_layout_windowed(backend, area, &node, &mut hits, &windows);
        self.engine.set_hits(hits);
    }

    fn extra_tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
        self.engine.tab_stops()
    }

    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
        self.engine
            .handle(event, backend, &mut *self.sink.borrow_mut())
    }
}

fn host_view() -> serde_json::Value {
    serde_json::json!({
        "column": [
            {"id": "save", "button": {"id": "save", "label": "Save"}},
            {"id": "opt", HOST_KIND: {"id": "opt", "value": false}},
        ]
    })
}

#[test]
fn a_runtime_registered_kind_is_a_generic_leaf_kind() {
    register_host_kind();
    assert!(widget_leaf_kinds().contains(&HOST_KIND));
    assert!(widget_leaf_kinds().contains(&"button"));
}

#[test]
fn a_host_view_decodes_a_runtime_kind_as_a_generic_widget_leaf() {
    let app = HostApp::new(host_view());
    let LayoutNode::Column(column) = app.decode() else {
        panic!("the view root is a column");
    };
    let kinds: Vec<&str> = column
        .children
        .iter()
        .map(|child| match &child.node {
            LayoutNode::Leaf(leaf) => match &leaf.descriptor {
                quadraui_binding_core::LeafDescriptor::Widget { kind, .. } => *kind,
                _ => "not a generic widget",
            },
            _ => "not a leaf",
        })
        .collect();
    assert_eq!(kinds, vec!["button", HOST_KIND]);
}

#[test]
fn a_host_paints_and_routes_both_kinds_through_binding_core_alone() {
    let mut driver = TuiDriver::new(HostApp::new(host_view()), 24, 8);
    driver.render();
    assert!(driver.screen_contains("Save"), "{}", driver.screen());

    let (save_x, save_y) = driver.find("Save").expect("the button paints its label");
    driver.click(save_x, save_y);

    let opt = driver
        .app()
        .engine
        .hit_map()
        .iter()
        .find_map(|hit| match hit {
            quadraui_binding_core::PaintedHit::Widget { id, area, .. }
                if *id == WidgetId::new("opt") =>
            {
                Some(*area)
            }
            _ => None,
        })
        .expect("the runtime kind leaf records a generic widget hit");
    driver.click(opt.x + 1.0, opt.y);

    let id = WidgetId::new("opt");
    assert_eq!(
        driver.app().sink.borrow().events,
        vec![
            UiEvent::FocusChanged(Some(WidgetId::new("save"))),
            UiEvent::ButtonActivated {
                widget: WidgetId::new("save")
            },
            UiEvent::FocusChanged(Some(id)),
            UiEvent::Form(id, FormEvent::ToggleChanged { id, value: true }),
        ]
    );
}

#[cfg(feature = "schema")]
#[test]
fn the_derived_wire_schema_lists_a_runtime_registered_kind() {
    register_host_kind();
    let schema = quadraui_binding_core::wire::schema::wire_schema();
    assert!(schema["kinds"].get(HOST_KIND).is_some());
    assert!(schema["kinds"].get("button").is_some());
}
