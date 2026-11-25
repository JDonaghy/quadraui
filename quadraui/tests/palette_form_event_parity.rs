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
