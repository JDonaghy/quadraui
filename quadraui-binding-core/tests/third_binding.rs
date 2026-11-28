//! A host binding built only from `quadraui` and `quadraui-binding-core`'s
//! public API, with no `quadraui-py`/`quadraui-node` code in the loop: the
//! shape a third binding (a Lua host binding `binding-core` directly)
//! takes. Each frame decodes a wire layout tree, resolves the engine's
//! stored widget state into it, paints it through a `TuiBackend` with the
//! engine's per-widget state installed
//! (`InteractionEngine::paint_with_widget_state`), and routes input back
//! through the engine to a host-side event sink.
//!
//! Overlays take the same route: a `show_overlay(kind, descriptor, anchor)`
//! call decodes through `decode_overlay` and opens through
//! `apply_deferred_action`, and each frame paints them with
//! `render_overlays`.
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
    apply_deferred_action, decode_layout_node, decode_overlay, paint_layout_windowed,
    render_overlays, resolve_layout_node_windowed, widget_leaf_kinds, BindingHost, EventSink,
    HitDetail, InteractionEngine, LayoutNode,
};

const HOST_KIND: &str = "host_toggle";

/// Registers [`HOST_KIND`] once per test binary: a host-defined kind name
/// whose payload decodes to a core `Checkbox`.
fn register_host_kind() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        quadraui_binding_core::register_widget_kind(HOST_KIND, |_kind, value| {
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
    host: BindingHost,
    sink: RefCell<HostSink>,
}

impl HostApp {
    fn new(view: serde_json::Value) -> Self {
        register_host_kind();
        Self {
            view,
            engine: InteractionEngine::new(),
            host: BindingHost::default(),
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
        self.engine.paint_with_widget_state(|| {
            paint_layout_windowed(backend, area, &node, &mut hits, &windows)
        });
        render_overlays(&self.engine, backend, hits);
    }

    fn extra_tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
        self.engine.tab_stops()
    }

    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
        let seen = self.sink.borrow().events.len();
        let reaction = self
            .engine
            .handle(event, backend, &mut *self.sink.borrow_mut());
        let changes: Vec<_> = self.sink.borrow().events[seen..]
            .iter()
            .filter_map(|event| match event {
                UiEvent::AppMenu(_, quadraui::AppMenuEvent::Changed(view)) => Some(view.clone()),
                _ => None,
            })
            .collect();
        for view in changes {
            echo_menu_view(&mut self.view, &view);
        }
        reaction
    }
}

/// Writes an application menu's reported view back onto every
/// `"menu_bar"` descriptor in `view`, the way a scripting host's own state
/// update would: the open menu and its highlights are the app's to keep.
fn echo_menu_view(view: &mut serde_json::Value, menu: &quadraui::AppMenuView) {
    match view {
        serde_json::Value::Object(map) => {
            if let Some(serde_json::Value::Object(bar)) = map.get_mut("menu_bar") {
                let serde_json::Value::Object(fields) =
                    serde_json::to_value(menu).expect("the view serialises")
                else {
                    unreachable!("a view serialises to an object");
                };
                bar.extend(fields);
            }
            for child in map.values_mut() {
                echo_menu_view(child, menu);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                echo_menu_view(item, menu);
            }
        }
        _ => {}
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

/// A runtime kind named after a layout container, a fixed leaf kind, an
/// extra primitive or a node sibling key would be shadowed during decode,
/// so binding-core refuses to register it at all.
#[test]
fn registering_a_reserved_node_key_as_a_kind_is_refused() {
    fn decode(
        _kind: &str,
        value: serde_json::Value,
    ) -> Result<Box<dyn DynWidget>, serde_json::Error> {
        serde_json::from_value::<quadraui::Checkbox>(value)
            .map(|checkbox| Box::new(checkbox) as Box<dyn DynWidget>)
    }
    for reserved in [
        "grid",
        "tabs",
        "text_input",
        "text_display",
        "scripted_widget",
        "style",
        "a11y",
        "id",
    ] {
        assert!(
            matches!(
                quadraui_binding_core::register_widget_kind(reserved, decode),
                Err(quadraui::widget_kinds::KindError::Duplicate(_))
            ),
            "{reserved:?} must be refused"
        );
        assert!(!quadraui::widget_kinds::runtime_widget_kinds().contains(&reserved));
    }
}

/// A host's `show_overlay("context_menu", ...)` decodes, opens, paints and
/// delivers its activation through binding-core alone.
#[test]
fn a_host_opens_a_context_menu_and_receives_its_activation() {
    let mut driver = TuiDriver::new(HostApp::new(host_view()), 30, 10);
    driver.render();
    let descriptor = serde_json::json!({
        "items": [{"id": "copy", "label": {"spans": [{"text": "Copy", "fg": null, "bg": null}]}}]
    });
    let (action, id) = decode_overlay(
        "show_overlay",
        "context_menu",
        &descriptor,
        Some([2.0, 2.0, 0.0, 0.0]),
    )
    .expect("the context menu decodes");
    assert_eq!(id, "context_menu");
    {
        let (backend, app) = driver.parts_mut();
        apply_deferred_action(
            action,
            backend,
            &mut app.engine,
            &mut app.host,
            "show_overlay",
        )
        .expect("the context menu opens");
    }
    driver.render();
    let (copy_x, copy_y) = driver
        .find("Copy")
        .unwrap_or_else(|| panic!("{}", driver.screen()));
    driver.click(copy_x, copy_y);
    assert!(
        driver
            .app()
            .sink
            .borrow()
            .events
            .contains(&UiEvent::ContextMenuItemActivated(WidgetId::new("copy"))),
        "{:?}",
        driver.app().sink.borrow().events
    );
}

/// A text input's caret is the widget's own state: a host that never
/// echoes a cursor back still sees the caret painted where `End` moved it,
/// and typing lands there, with nothing text-specific in the host.
#[test]
fn a_host_text_input_paints_its_caret_from_the_engines_widget_state() {
    let view = serde_json::json!({
        "column": [
            {"id": "name", "text_input": {"id": "name", "lines": ["hello"], "has_focus": true}},
        ]
    });
    let mut driver = TuiDriver::new(HostApp::new(view), 24, 8);
    driver.render();
    let (x, y) = driver.find("hello").expect("the input paints its value");
    let (x, y) = (x as u16, y as u16);
    let caret_at = |driver: &TuiDriver<HostApp>, col: u16| {
        driver.style_at(x + col, y) != driver.style_at(x + col + 1, y)
            && driver.style_at(x + col, y) != driver.style_at(x + col - 1, y)
    };
    driver.click(f32::from(x), f32::from(y));
    driver.press_named(quadraui::NamedKey::End);
    driver.render();
    assert!(caret_at(&driver, 5), "the caret paints after the last char");
    assert!(
        !caret_at(&driver, 0),
        "the descriptor's column-0 caret is not painted"
    );

    driver.type_char('!');
    assert!(
        driver
            .app()
            .sink
            .borrow()
            .events
            .iter()
            .any(|event| matches!(
                event,
                UiEvent::Form(_, FormEvent::TextInputChanged { value, .. }) if value == "hello!"
            )),
        "{:?}",
        driver.app().sink.borrow().events
    );
}

/// The engine's widget state is installed only for the paint pass: the
/// thread's own store is back in place once it returns.
#[test]
fn paint_with_widget_state_restores_the_threads_own_store() {
    let id = WidgetId::new("outside");
    quadraui::with_widget_states(|states| {
        states.state_for(id, &quadraui::Checkbox::new(id));
    });
    let engine = InteractionEngine::new();
    let seen_inside = engine
        .paint_with_widget_state(|| quadraui::with_widget_states(|states| states.contains(&id)));
    assert!(
        !seen_inside,
        "the engine's own store is the one painted from"
    );
    assert!(quadraui::with_widget_states(|states| states.contains(&id)));
    quadraui::with_widget_states(|states| states.remove(&id));
}

/// A widget's popups route through the engine with no host code of their
/// own: a click on the menu bar reports the opened view, the host echoes
/// it, and a click on a row of the dropdown — a popup the menu owns —
/// activates it.
#[test]
fn a_host_menu_bar_opens_its_dropdown_popup_and_activates_a_row() {
    let view = serde_json::json!({
        "column": [
            {"id": "bar", "menu_bar": {"id": "bar", "items": [
                {"id": "menu:file", "label": "&File", "submenu": [
                    {"id": "menu:file:save", "label": {"spans": [{"text": "Save", "fg": null, "bg": null}]}},
                ]},
            ]}},
            {"id": "save", "button": {"id": "save", "label": "Go"}},
        ]
    });
    let mut driver = TuiDriver::new(HostApp::new(view), 30, 12);
    driver.render();
    let (x, y) = driver.find("File").expect("the bar paints its items");
    driver.click(x, y);
    driver.render();
    let (x, y) = driver
        .find("Save")
        .unwrap_or_else(|| panic!("the dropdown paints:\n{}", driver.screen()));
    driver.click(x, y);

    let events = &driver.app().sink.borrow().events;
    assert!(
        events.iter().any(
            |e| matches!(e, UiEvent::MenuActivated(id) if *id == WidgetId::new("menu:file:save"))
        ),
        "{events:?}"
    );
}

/// A form's field focus, caret and scroll are its own stored state: a host
/// that never echoes a form event back still has Tab walk the fields and
/// typing land in the focused one.
#[test]
fn a_host_form_walks_its_fields_and_edits_with_nothing_echoed() {
    let field = |id: &str| {
        serde_json::json!({
            "id": id,
            "label": {"spans": [{"text": id, "fg": null, "bg": null}]},
            "kind": {"TextInput": {"value": ""}},
        })
    };
    let view = serde_json::json!({
        "column": [
            {"id": "settings", "form": {"id": "settings", "fields": [field("a"), field("b")]}},
        ]
    });
    let mut driver = TuiDriver::new(HostApp::new(view), 30, 8);
    driver.render();
    for _ in 0..3 {
        driver.press_named(quadraui::NamedKey::Tab);
        driver.render();
    }
    driver.press(quadraui::Key::Char('x'));

    let id = WidgetId::new("settings");
    let form_events: Vec<FormEvent> = driver
        .app()
        .sink
        .borrow()
        .events
        .iter()
        .filter_map(|e| match e {
            UiEvent::Form(form, event) if *form == id => Some(event.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        form_events,
        vec![
            FormEvent::FocusChanged {
                id: WidgetId::new("a")
            },
            FormEvent::FocusChanged {
                id: WidgetId::new("b")
            },
            FormEvent::TextInputChanged {
                id: WidgetId::new("b"),
                value: "x".into()
            },
        ]
    );
}
