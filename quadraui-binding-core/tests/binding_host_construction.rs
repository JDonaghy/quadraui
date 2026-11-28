//! `BindingHost`'s construction contract, exercised from outside the crate
//! the way every binding leaf (`quadraui-py`, `quadraui-node`,
//! `quadraui-ffi`) builds one: a struct literal naming the few fields it
//! cares about, finished with `..BindingHost::default()`. That functional
//! record update only compiles while every field of the struct is visible
//! here, so this test is what keeps the host's fields reachable from a
//! binding crate rather than only from inside `quadraui-binding-core`.
//!
//! Also pins the paint consequence of the host carrying no additive
//! primitives of its own: the `RenderRefs` it hands `draw_bands` borrows an
//! empty extras map, so only the fixed bands reach the backend.

use std::collections::HashMap;
use std::rc::Rc;

use quadraui::primitive::DynWidget;
use quadraui::tui::testing::TuiDriver;
use quadraui::{
    Accelerator, AcceleratorId, AcceleratorScope, AppLogic, Backend, Button, KeyBinding, Reaction,
    StyledSpan, TextDisplay, TextDisplayLine, UiEvent, WidgetId,
};
use quadraui_binding_core::{draw_bands, BindingHost};

/// A [`BindingHost::widgets`] map holding just `button` under its `"button"`
/// key — the single generic-widget slot this test's own hosts carry.
fn button_widgets(button: Button) -> HashMap<String, Rc<dyn DynWidget>> {
    let mut widgets = HashMap::new();
    widgets.insert("button".to_string(), Rc::new(button) as Rc<dyn DynWidget>);
    widgets
}

/// Paints whatever fixed descriptors its host holds, through the exact
/// `BindingHost::render_refs` -> `draw_bands` path a binding's own
/// `AppLogic::render` drives.
struct HostApp {
    host: BindingHost,
}

impl AppLogic for HostApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let mut hits = Vec::new();
        draw_bands(backend, self.host.render_refs(), &mut hits);
    }

    fn handle(&mut self, _event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        Reaction::Continue
    }
}

/// A binding builds a host by naming only the fields it populates and
/// deferring the rest to `Default` — the fixed descriptor slots and the
/// backend-settings replay store are all reachable from another crate.
#[test]
fn a_binding_builds_a_host_by_functional_record_update() {
    let accelerator = Accelerator {
        id: AcceleratorId::new("save"),
        binding: KeyBinding::Save,
        scope: AcceleratorScope::Global,
        label: None,
    };
    let mut accelerators = std::collections::HashMap::new();
    accelerators.insert(accelerator.id.clone(), accelerator);

    let host = BindingHost {
        widgets: button_widgets(Button::new(WidgetId::new("save"), "Save")),
        accelerators,
        ..BindingHost::default()
    };

    assert!(host.widgets.contains_key("button"));
    assert_eq!(host.accelerators.len(), 1);
    assert!(host.list.is_none());
}

/// The fixed slots a host does hold paint top-to-bottom.
#[test]
fn a_host_paints_its_fixed_slots() {
    let mut status = TextDisplay::new(WidgetId::new("status"));
    status.append_line(TextDisplayLine {
        spans: vec![StyledSpan::plain("Ready")],
        decoration: Default::default(),
        timestamp: None,
    });

    let host = BindingHost {
        widgets: button_widgets(Button::new(WidgetId::new("save"), "Save")),
        text_display: Some(status),
        ..BindingHost::default()
    };

    let mut driver = TuiDriver::new(HostApp { host }, 24, 8);
    driver.render();
    assert!(
        driver.screen_contains("Save"),
        "the button band must paint:\n{}",
        driver.screen()
    );
    assert!(
        driver.screen_contains("Ready"),
        "the text-display band must paint above it:\n{}",
        driver.screen()
    );
}
