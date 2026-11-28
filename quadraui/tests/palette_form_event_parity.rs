#![cfg(feature = "tui")]
//! The `TuiDriver` leg of the `Palette` and `Form` event-parity checks.
//!
//! Each widget reaches the frame as a `Surface::Widget` registered in the
//! app's `FrameTree`, so a gesture routes through the shared
//! widget-contract engine (hit test, click-to-focus, `Interactive::handle`)
//! and comes back out as the `UiEvent` this app records. The
//! assistive-technology leg (`a11y::route_widget_action`, crate-private)
//! lives in each widget's own in-crate tests, and the binding-host leg
//! (`quadraui-binding-core`'s `InteractionEngine`) in that crate's engine
//! tests. All three resolve the same gesture to the same `UiEvent`:
//!
//! | widget    | gesture                          | event                                   |
//! |-----------|----------------------------------|-----------------------------------------|
//! | `Palette` | click the third row              | `Palette(pal, SelectionChanged { 2 })`  |
//! | `Palette` | Enter on the focused palette     | `Palette(pal, ItemConfirmed { 0 })`     |
//! | `Form`    | click the focused toggle row     | `Form(settings, ToggleChanged { .. })`  |

use quadraui::tui::testing::TuiDriver;
use quadraui::{
    AppLogic, Backend, FieldKind, Form, FormEvent, FormField, FrameTree, NamedKey, Palette,
    PaletteEvent, PaletteItem, Reaction, Rect, ScreenLayout, StyledText, Surface, UiEvent,
    WidgetId,
};

const COLS: u16 = 40;
const ROWS: u16 = 14;
const AREA: Rect = Rect::new(2.0, 1.0, 30.0, 10.0);

fn row(text: &str) -> PaletteItem {
    PaletteItem {
        text: StyledText::plain(text),
        detail: None,
        icon: None,
        match_positions: Vec::new(),
        depth: 0,
        expandable: false,
        expanded: false,
    }
}

fn palette() -> Palette {
    Palette {
        id: WidgetId::new("pal"),
        title: "Commands".to_string(),
        items: vec![row("alpha"), row("beta"), row("gamma")],
        total_count: 3,
        has_focus: true,
        ..Palette::default()
    }
}

fn form() -> Form {
    Form {
        id: WidgetId::new("settings"),
        fields: vec![FormField {
            id: WidgetId::new("notify"),
            label: StyledText::plain("Notify"),
            kind: FieldKind::Toggle { value: false },
            hint: StyledText::default(),
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
        focused_field: Some(WidgetId::new("notify")),
        scroll_offset: 0,
        has_focus: true,
        label_position: Default::default(),
        label_align: Default::default(),
        label_width: None,
        control_width: Default::default(),
        min_control_width: None,
        spacing: Default::default(),
    }
}

/// One widget drawn and routed as a `Surface::Widget`; `events` records
/// every `UiEvent` the routing engine hands the app.
struct WidgetApp<W> {
    id: WidgetId,
    widget: W,
    events: Vec<UiEvent>,
}

impl<W> AppLogic for WidgetApp<W>
where
    W: quadraui::primitive::DynWidget + Clone + 'static,
{
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let mut frame = ScreenLayout::new();
        frame.push(Surface::Widget {
            rect: AREA,
            widget: Box::new(self.widget.clone()),
        });
        frame.draw(backend);
    }

    fn frame_tree(&self, _area: ()) -> FrameTree {
        let mut tree = FrameTree::new();
        tree.push(self.id, AREA, Box::new(self.widget.clone()));
        tree
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        self.events.push(event);
        Reaction::Continue
    }
}

fn driver<W>(id: &str, widget: W) -> TuiDriver<WidgetApp<W>>
where
    W: quadraui::primitive::DynWidget + Clone + 'static,
{
    TuiDriver::new(
        WidgetApp {
            id: WidgetId::new(id),
            widget,
            events: Vec::new(),
        },
        COLS,
        ROWS,
    )
}

fn palette_events(events: &[UiEvent]) -> Vec<UiEvent> {
    events
        .iter()
        .filter(|e| matches!(e, UiEvent::Palette(..)))
        .cloned()
        .collect()
}

#[test]
fn a_click_on_the_third_palette_row_matches_the_parity_event() {
    let mut driver = driver("pal", palette());
    let (x, y) = driver.find("gamma").expect("the third row is painted");
    driver.click(x, y);
    assert_eq!(
        palette_events(&driver.app().events),
        vec![UiEvent::Palette(
            WidgetId::new("pal"),
            PaletteEvent::SelectionChanged { idx: 2 }
        )]
    );
}

#[test]
fn enter_on_the_focused_palette_matches_the_parity_event() {
    let mut driver = driver("pal", palette());
    // The title row is inert: clicking it only focuses the palette.
    driver.click(AREA.x + 4.0, AREA.y);
    assert!(palette_events(&driver.app().events).is_empty());
    driver.press_named(NamedKey::Enter);
    assert_eq!(
        palette_events(&driver.app().events),
        vec![UiEvent::Palette(
            WidgetId::new("pal"),
            PaletteEvent::ItemConfirmed { idx: 0 }
        )]
    );
}

#[test]
fn a_click_on_the_focused_toggle_row_matches_the_parity_event() {
    let mut driver = driver("settings", form());
    let (x, y) = driver.find("Notify").expect("the toggle row is painted");
    driver.click(x, y);
    let toggles: Vec<UiEvent> = driver
        .app()
        .events
        .iter()
        .filter(|e| matches!(e, UiEvent::Form(_, FormEvent::ToggleChanged { .. })))
        .cloned()
        .collect();
    assert_eq!(
        toggles,
        vec![UiEvent::Form(
            WidgetId::new("settings"),
            FormEvent::ToggleChanged {
                id: WidgetId::new("notify"),
                value: true
            }
        )]
    );
}

/// A text field row for [`uncontrolled_form`].
fn text_field(id: &str) -> FormField {
    FormField {
        id: WidgetId::new(id),
        label: StyledText::plain(id),
        kind: FieldKind::TextInput {
            value: String::new(),
            placeholder: String::new(),
            cursor: None,
            selection_anchor: None,
        },
        hint: StyledText::default(),
        disabled: false,
        validation: None,
        label_position: None,
        label_align: None,
        label_width: None,
        tab_index: None,
        control_width: None,
        min_control_width: None,
        row_align: None,
    }
}

/// A form whose app never sets `focused_field` and never echoes a
/// `FormEvent` back: every frame repaints the same descriptor.
fn uncontrolled_form(ids: &[&str]) -> Form {
    Form {
        id: WidgetId::new("settings"),
        fields: ids.iter().map(|id| text_field(id)).collect(),
        focused_field: None,
        ..form()
    }
}

fn form_events(events: &[UiEvent]) -> Vec<FormEvent> {
    events
        .iter()
        .filter_map(|e| match e {
            UiEvent::Form(_, event) => Some(event.clone()),
            _ => None,
        })
        .collect()
}

/// Field focus a descriptor leaves unset is the form's own to track:
/// after a click on "b", typing edits "b" with nothing echoed back.
#[test]
fn an_uncontrolled_form_edits_the_field_its_own_click_focused() {
    let mut driver = driver("settings", uncontrolled_form(&["a", "b"]));
    let (x, y) = driver.find("b").expect("the second row is painted");
    driver.click(x, y);
    driver.press(quadraui::Key::Char('x'));
    assert_eq!(
        form_events(&driver.app().events),
        vec![
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

/// Inside a focused form, Down moves past a field that has no use for it
/// (a toggle), and Tab past the last field wraps to the first; each
/// move reports the field as the form's focused child, so a host's focus
/// traversal knows where inside the form focus sits.
#[test]
fn a_form_moves_field_focus_on_down_and_reports_the_focused_child() {
    let toggle = |id: &str| FormField {
        id: WidgetId::new(id),
        label: StyledText::plain(id),
        ..form().fields[0].clone()
    };
    let mut driver = driver(
        "settings",
        Form {
            fields: vec![toggle("a"), toggle("b")],
            ..uncontrolled_form(&[])
        },
    );
    // A click on a toggle flips it and focuses the form, but no field;
    // the first Down enters the first field, the second moves past it.
    let (x, y) = driver.find("a").expect("the first row is painted");
    driver.click(x, y);
    driver.press_named(NamedKey::Down);
    driver.press_named(NamedKey::Down);
    {
        let (backend, _) = driver.parts_mut();
        assert_eq!(
            backend.focus_tree().focused().copied(),
            Some((
                WidgetId::new("settings"),
                Some(quadraui::interactive::SubTarget(1))
            ))
        );
    }
    driver.press_named(NamedKey::Tab);
    let focus_moves: Vec<FormEvent> = form_events(&driver.app().events)
        .into_iter()
        .filter(|e| matches!(e, FormEvent::FocusChanged { .. }))
        .collect();
    assert_eq!(
        focus_moves,
        vec![
            FormEvent::FocusChanged {
                id: WidgetId::new("a")
            },
            FormEvent::FocusChanged {
                id: WidgetId::new("b")
            },
            FormEvent::FocusChanged {
                id: WidgetId::new("a")
            },
        ]
    );
}

/// A wheel notch over a form scrolls what it paints: the offset lives in
/// the form's own state, which its layout reads.
#[test]
fn a_wheel_notch_scrolls_the_painted_form() {
    let ids: Vec<String> = (0..20).map(|i| format!("Field{i:02}")).collect();
    let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
    let mut driver = driver("settings", uncontrolled_form(&ids));
    assert!(driver.screen_contains("Field00"), "{}", driver.screen());
    driver.dispatch(UiEvent::Scroll {
        widget: None,
        delta: quadraui::ScrollDelta { x: 0.0, y: -1.0 },
        position: quadraui::Point::new(AREA.x + 1.0, AREA.y + 1.0),
    });
    driver.render();
    assert!(
        !driver.screen_contains("Field00") && driver.screen_contains("Field01"),
        "{}",
        driver.screen()
    );
}
