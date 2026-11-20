#![cfg(feature = "tui")]
//! Live-runner a11y tree built from `Backend::last_frame()`.
//!
//! A real `AppLogic` paints a `ScreenLayout` (a list plus a form) through
//! the TUI `Backend` with frame capture enabled. The resulting tree —
//! built from `Backend::last_frame()` + `Backend::focus_manager()` via
//! `backend_a11y_tree` — must match the `A11yScenario` snapshot of the
//! identical surfaces, proving the live path and the test-only path agree.

use quadraui::testing::a11y::A11yScenario;
use quadraui::testing::RecordingBackend;
use quadraui::tui::testing::TuiDriver;
use quadraui::{
    backend_a11y_tree, set_frame_capture, Backend, ControlWidth, Decoration, FieldKind, Form,
    FormField, LabelAlign, LabelPosition, ListItem, ListView, Reaction, Rect, ScreenLayout,
    StyledText, Surface, UiEvent, WidgetId,
};

fn list_surface() -> Surface {
    Surface::List {
        rect: Rect::new(0.0, 0.0, 30.0, 8.0),
        list: ListView {
            id: WidgetId::new("files"),
            title: Some(StyledText::plain("Files")),
            items: vec![
                ListItem {
                    text: StyledText::plain("lib.rs"),
                    icon: None,
                    detail: None,
                    decoration: Decoration::default(),
                },
                ListItem {
                    text: StyledText::plain("main.rs"),
                    icon: None,
                    detail: None,
                    decoration: Decoration::default(),
                },
            ],
            selected_idx: 0,
            scroll_offset: 0,
            has_focus: true,
            bordered: false,
            h_scroll: 0,
            max_content_width: None,
            show_v_scrollbar: false,
        },
    }
}

fn form_surface() -> Surface {
    Surface::Form {
        rect: Rect::new(0.0, 8.0, 30.0, 4.0),
        form: Form {
            id: WidgetId::new("settings"),
            fields: vec![FormField {
                id: WidgetId::new("settings:name"),
                label: StyledText::plain("Name"),
                kind: FieldKind::TextInput {
                    value: "Ada".to_string(),
                    placeholder: String::new(),
                    cursor: None,
                    selection_anchor: None,
                },
                hint: StyledText::plain(""),
                disabled: false,
                validation: None,
                label_position: None,
                label_align: None,
                label_width: None,
                tab_index: None,
                control_width: None,
                min_control_width: None,
                row_align: None,
            }],
            focused_field: None,
            scroll_offset: 0,
            has_focus: false,
            label_position: LabelPosition::default(),
            label_align: LabelAlign::default(),
            label_width: None,
            control_width: ControlWidth::default(),
            min_control_width: None,
            spacing: Default::default(),
        },
    }
}

/// Paints `list_surface`/`form_surface` through a real `Backend` via
/// `ScreenLayout::draw`, exactly the canonical path (D-006) frame
/// capture is documented to observe.
struct LiveApp;

impl quadraui::AppLogic for LiveApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let mut layout = ScreenLayout::new();
        layout.push(list_surface());
        layout.push(form_surface());
        layout.draw(backend);
    }

    fn handle(&mut self, _event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        Reaction::Continue
    }
}

#[test]
fn last_frame_builds_the_same_a11y_tree_a_test_scenario_would() {
    let mut driver = TuiDriver::new(LiveApp, 40, 20);
    {
        let (backend, _app) = driver.parts_mut();
        assert!(
            set_frame_capture(backend, true),
            "the TUI backend offers a frame-capture slot"
        );
        backend.set_focus(Some(WidgetId::new("files")));
    }
    driver.render();

    let live_tree =
        backend_a11y_tree(driver.backend()).expect("frame capture was enabled before render");

    let expected = A11yScenario::new()
        .with_surface(list_surface())
        .with_surface(form_surface())
        .with_focus(WidgetId::new("files"))
        .snapshot();

    assert_eq!(live_tree.to_snapshot(), expected.text());
}

#[test]
fn last_frame_is_none_until_capture_is_enabled() {
    let mut driver = TuiDriver::new(LiveApp, 40, 20);
    driver.render();

    assert!(
        driver.backend().last_frame().is_none(),
        "capture defaults to off, so a backend that never opted in must never retain a frame"
    );
    assert!(
        backend_a11y_tree(driver.backend()).is_none(),
        "capture defaults to off, so a backend that never opted in must never report a frame"
    );
}

/// A backend that doesn't override the capture accessors keeps the
/// trait's `None` defaults: opting in fails, and painting a real
/// `ScreenLayout` through it still reports no frame.
#[test]
fn a_backend_without_a_capture_slot_never_reports_a_frame() {
    let mut backend = RecordingBackend::new();
    assert!(
        !set_frame_capture(&mut backend, true),
        "a backend with no capture slot must report that capture is unavailable"
    );

    let mut layout = ScreenLayout::new();
    layout.push(list_surface());
    layout.draw(&mut backend);

    assert!(backend.last_frame().is_none());
    assert!(backend_a11y_tree(&backend).is_none());
}

#[test]
fn opting_back_out_drops_the_retained_frame() {
    let mut driver = TuiDriver::new(LiveApp, 40, 20);
    set_frame_capture(driver.parts_mut().0, true);
    driver.render();
    assert!(
        driver.backend().last_frame().is_some(),
        "a paint with capture enabled retains the frame"
    );

    set_frame_capture(driver.parts_mut().0, false);
    assert!(
        driver.backend().last_frame().is_none(),
        "opting out must drop the retained frame, not leave a stale screen readable"
    );

    driver.render();
    assert!(
        driver.backend().last_frame().is_none(),
        "painting after opting out must not retain anything again"
    );
}
