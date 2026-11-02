//! Language-neutral `event -> handle -> render` smoke test, with no
//! host-language FFI layer in scope -- proves this crate is usable
//! standalone, the shape a further native binding can mirror.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use quadraui::tui::testing::TuiDriver;
use quadraui::{
    AppLogic, Backend, Button, ButtonHit, Modifiers, MouseButton, Point, Reaction, UiEvent,
    WidgetId,
};

use crate::hit::{hit_test_painted, PaintedHit};
use crate::render::{draw_bands, RenderRefs};

/// A minimal [`AppLogic`] built only from this crate's engine: one
/// `Button` slot, painted with [`draw_bands`] and hit-tested with
/// [`hit_test_painted`] -- the same two calls a binding's own `AppLogic`
/// makes, with no host-language FFI type anywhere in this file.
struct MinimalLogic {
    button: Button,
    hit_map: RefCell<Vec<PaintedHit>>,
    clicks: Cell<usize>,
}

impl AppLogic for MinimalLogic {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let mut hits = Vec::new();
        let extra = HashMap::new();
        draw_bands(
            backend,
            RenderRefs {
                button: Some(&self.button),
                text_display: None,
                list: None,
                tree: None,
                data_table: None,
                tab_bar: None,
                extra: &extra,
            },
            &mut hits,
        );
        *self.hit_map.borrow_mut() = hits;
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        if let UiEvent::MouseDown { position, .. } = event {
            if hit_test_painted(&self.hit_map.borrow(), position).is_some() {
                self.clicks.set(self.clicks.get() + 1);
            }
        }
        Reaction::Continue
    }
}

#[test]
fn draws_a_button_and_hit_tests_a_click_with_no_host_binding_in_scope() {
    let logic = MinimalLogic {
        button: Button::new(WidgetId::new("go"), "Go"),
        hit_map: RefCell::new(Vec::new()),
        clicks: Cell::new(0),
    };
    let mut driver = TuiDriver::new(logic, 20, 5);

    {
        let hits = driver.app().hit_map.borrow();
        let button_layout = hits.iter().find_map(|hit| match hit {
            PaintedHit::Button { layout, .. } => Some(*layout),
            _ => None,
        });
        let layout = button_layout.expect("button should have painted a hit");
        assert_eq!(layout.hit_test(Point::new(1.0, 1.0)), ButtonHit::Body);
    }

    driver.dispatch(UiEvent::MouseDown {
        widget: None,
        button: MouseButton::Left,
        position: Point::new(1.0, 1.0),
        modifiers: Modifiers::default(),
    });

    assert_eq!(driver.app().clicks.get(), 1);
}
