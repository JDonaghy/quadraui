#![cfg(feature = "tui")]
//! Black-box `TuiDriver` tests for `MultiSectionView` on the widget
//! contract: the view reaches the frame as a type-erased `Surface::Widget`,
//! paints its chrome and each section body through that body's own widget,
//! and is registered in the app's `FrameTree`, so pointer, wheel and key
//! input route through the shared engine into `Interactive::handle`.
//!
//! The view is controlled: this app applies every `MultiSectionViewEvent`
//! (collapse, scroll, resize, aux input) and every forwarded body event
//! back onto the descriptor it owns, and the next frame paints the result.

use quadraui::tui::testing::TuiDriver;
use quadraui::{
    AppLogic, Backend, EmptyBody, FieldKind, Form, FormEvent, FormField, FrameTree, InlineInput,
    MsvAxis, MultiSectionView, MultiSectionViewEvent, NamedKey, Point, Reaction, Rect,
    ScreenLayout, ScrollDelta, ScrollMode, Section, SectionAux, SectionBody, SectionHeader,
    SectionSize, SelectionMode, StyledText, Surface, TreeEvent, TreeRow, TreeView, UiEvent,
    WidgetId,
};

const COLS: u16 = 40;
const ROWS: u16 = 16;
/// A non-zero origin, so the view-local and body-local translations of
/// every pointer event are exercised.
const AREA: Rect = Rect::new(2.0, 1.0, 30.0, 14.0);

fn view_id() -> WidgetId {
    WidgetId::new("msv:sidebar")
}

fn tree_id() -> WidgetId {
    WidgetId::new("msv:files")
}

fn form_id() -> WidgetId {
    WidgetId::new("msv:options")
}

fn header(title: &str) -> SectionHeader {
    SectionHeader {
        title: StyledText::plain(title),
        ..Default::default()
    }
}

fn section(id: &str, title: &str, body: SectionBody, size: SectionSize) -> Section {
    Section {
        id: id.into(),
        header: header(title),
        body,
        aux: None,
        size,
        collapsed: false,
        min_size: None,
        max_size: None,
    }
}

fn file_rows(n: usize) -> Vec<TreeRow> {
    (0..n)
        .map(|i| TreeRow {
            path: vec![i as u16],
            indent: 0,
            icon: None,
            text: StyledText::plain(format!("file{i:02}")),
            badge: None,
            is_expanded: None,
            decoration: Default::default(),
            edit: None,
        })
        .collect()
}

fn toggle(id: &str, label: &str, value: bool) -> FormField {
    FormField {
        id: WidgetId::new(id),
        label: StyledText::plain(label),
        kind: FieldKind::Toggle { value },
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

/// Files (a 12-row tree in a 5-row section), Options (a one-toggle
/// form) and Search (an empty body under a search input).
fn sidebar() -> MultiSectionView {
    let tree = TreeView {
        id: tree_id(),
        rows: file_rows(12),
        selection_mode: SelectionMode::Single,
        selected_path: None,
        scroll_offset: 0,
        style: Default::default(),
        has_focus: false,
    };
    let form = Form {
        id: form_id(),
        fields: vec![toggle("wrap", "Wrap", false)],
        focused_field: None,
        scroll_offset: 0,
        has_focus: false,
        label_position: Default::default(),
        label_align: Default::default(),
        label_width: None,
        control_width: Default::default(),
        min_control_width: None,
        spacing: Default::default(),
    };
    let mut search = section(
        "search",
        "SEARCH",
        SectionBody::Empty(EmptyBody {
            text: StyledText::plain("No results"),
            ..Default::default()
        }),
        SectionSize::EqualShare,
    );
    search.aux = Some(SectionAux::Search(InlineInput {
        id: WidgetId::new("msv:query"),
        text: String::new(),
        caret: 0,
        placeholder: Some("Search".into()),
        has_focus: false,
    }));
    MultiSectionView {
        id: view_id(),
        sections: vec![
            section(
                "files",
                "FILES",
                SectionBody::Tree(tree),
                SectionSize::Fixed(6),
            ),
            section(
                "options",
                "OPTIONS",
                SectionBody::Form(form),
                SectionSize::Fixed(3),
            ),
            search,
        ],
        active_section: None,
        axis: MsvAxis::Vertical,
        allow_resize: true,
        allow_collapse: true,
        scroll_mode: ScrollMode::PerSection,
        has_focus: false,
        panel_scroll: 0.0,
    }
}

/// The app that owns the view and applies what it reports.
struct SidebarApp {
    view: MultiSectionView,
    events: Vec<MultiSectionViewEvent>,
    raw: Vec<UiEvent>,
}

impl SidebarApp {
    fn new() -> Self {
        Self {
            view: sidebar(),
            events: Vec::new(),
            raw: Vec::new(),
        }
    }

    fn tree(&self) -> &TreeView {
        match &self.view.sections[0].body {
            SectionBody::Tree(t) => t,
            other => panic!("section 0 is a tree, got {other:?}"),
        }
    }

    fn apply(&mut self, event: &MultiSectionViewEvent) {
        match event {
            MultiSectionViewEvent::CollapseChanged { section, collapsed } => {
                self.view.sections[*section].collapsed = *collapsed;
            }
            MultiSectionViewEvent::ScrollChanged {
                section: Some(i),
                offset,
            } => {
                if let SectionBody::Tree(t) = &mut self.view.sections[*i].body {
                    t.scroll_offset = *offset as usize;
                }
            }
            MultiSectionViewEvent::SectionResized {
                above,
                above_size,
                below,
                below_size,
            } => {
                self.view.sections[*above].size = SectionSize::Fixed(*above_size);
                self.view.sections[*below].size = SectionSize::Fixed(*below_size);
            }
            MultiSectionViewEvent::AuxInputChanged {
                section,
                text,
                caret,
            } => {
                if let Some(SectionAux::Search(input)) = &mut self.view.sections[*section].aux {
                    input.text = text.clone();
                    input.caret = *caret;
                    input.has_focus = true;
                }
            }
            MultiSectionViewEvent::Body(0, inner) => {
                if let UiEvent::Tree(_, event) = inner.as_ref() {
                    let tree = match &mut self.view.sections[0].body {
                        SectionBody::Tree(t) => t,
                        _ => return,
                    };
                    match event {
                        TreeEvent::SelectionChanged { path }
                        | TreeEvent::RowClicked { path, .. } => {
                            tree.selected_path = Some(path.clone());
                        }
                        TreeEvent::ScrollChanged { scroll_offset } => {
                            tree.scroll_offset = *scroll_offset;
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
}

impl AppLogic for SidebarApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let mut frame = ScreenLayout::new();
        frame.push(Surface::Widget {
            rect: AREA,
            widget: Box::new(self.view.clone()),
        });
        frame.draw(backend);
    }

    fn frame_tree(&self, _area: ()) -> FrameTree {
        let mut tree = FrameTree::new();
        tree.push(self.view.id, AREA, Box::new(self.view.clone()));
        tree
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        if let UiEvent::MultiSectionView(id, event) = &event {
            assert_eq!(*id, view_id());
            self.apply(event);
            self.events.push(event.clone());
        }
        self.raw.push(event);
        Reaction::Redraw
    }
}

fn driver() -> TuiDriver<SidebarApp> {
    TuiDriver::new(SidebarApp::new(), COLS, ROWS)
}

fn at(driver: &TuiDriver<SidebarApp>, needle: &str) -> (f32, f32) {
    let b = driver
        .find_bounds(needle)
        .unwrap_or_else(|| panic!("{needle} on screen:\n{}", driver.screen()));
    (b.x + 0.5, b.y)
}

#[test]
fn headers_and_every_body_paint_inside_the_view() {
    let d = driver();
    for title in ["FILES", "OPTIONS", "SEARCH"] {
        let (x, y) = at(&d, title);
        assert!(AREA.contains(Point::new(x, y)), "{title} inside the view");
    }
    // The tree body paints through `TreeView`'s own cell painter: its
    // first five rows fill the five body rows under the header.
    assert_eq!(at(&d, "file00").1, AREA.y + 1.0);
    assert_eq!(at(&d, "file04").1, AREA.y + 5.0);
    assert!(!d.screen_contains("file05"), "{}", d.screen());
    // The form body paints its toggle label, the search body its
    // placeholder and empty-state message.
    assert!(d.screen_contains("Wrap"), "{}", d.screen());
    assert!(d.screen_contains("Search"), "{}", d.screen());
    assert!(d.screen_contains("No results"), "{}", d.screen());
}

#[test]
fn the_section_gutter_is_the_only_scrollbar_beside_an_overflowing_tree() {
    let d = driver();
    let gutter_x = (AREA.x + AREA.width - 1.0) as u16;
    let thumb_and_track: Vec<char> = (1..6)
        .map(|dy| d.styled_row(AREA.y as u16 + dy)[gutter_x as usize].0)
        .collect();
    assert!(
        thumb_and_track.iter().all(|c| matches!(c, '█' | '░')),
        "the gutter column holds the section scrollbar: {thumb_and_track:?}\n{}",
        d.screen()
    );
    // The tree paints its rows right up to the gutter, with no second
    // scrollbar column of its own beside it.
    let inner: Vec<char> = (1..6)
        .map(|dy| d.styled_row(AREA.y as u16 + dy)[gutter_x as usize - 1].0)
        .collect();
    assert!(
        inner.iter().all(|c| !matches!(c, '█' | '░' | '▐' | '▌')),
        "no second scrollbar inside the tree body: {inner:?}\n{}",
        d.screen()
    );
}

#[test]
fn clicking_a_header_activates_it_and_collapses_its_section() {
    let mut d = driver();
    let (x, y) = at(&d, "OPTIONS");
    d.click(x, y);

    assert!(d
        .app()
        .events
        .contains(&MultiSectionViewEvent::HeaderActivated {
            section: 1,
            hit: quadraui::HeaderHit::TitleArea,
        }));
    assert!(d
        .app()
        .events
        .contains(&MultiSectionViewEvent::CollapseChanged {
            section: 1,
            collapsed: true,
        }));
    assert!(d.app().view.sections[1].collapsed);
    assert!(
        !d.screen_contains("Wrap"),
        "collapsed body is gone:\n{}",
        d.screen()
    );
}

#[test]
fn a_click_on_a_tree_row_routes_through_the_tree_widget() {
    let mut d = driver();
    let (x, y) = at(&d, "file02");
    d.click(x, y);

    assert!(
        d.app().events.iter().any(|e| matches!(
            e,
            MultiSectionViewEvent::Body(0, inner)
                if matches!(inner.as_ref(), UiEvent::Tree(id, _) if *id == tree_id())
        )),
        "the tree reports the click under its own id: {:?}",
        d.app().events
    );
    assert_eq!(d.app().tree().selected_path, Some(vec![2]));
}

#[test]
fn a_click_on_a_form_toggle_routes_through_the_form_widget() {
    let mut d = driver();
    let (x, y) = at(&d, "Wrap");
    d.click(x, y);

    assert!(
        d.app().events.iter().any(|e| matches!(
            e,
            MultiSectionViewEvent::Body(1, inner)
                if matches!(
                    inner.as_ref(),
                    UiEvent::Form(id, FormEvent::ToggleChanged { value: true, .. })
                        if *id == form_id()
                )
        )),
        "the form reports the toggle under its own id: {:?}",
        d.app().events
    );
}

#[test]
fn the_wheel_over_the_tree_scrolls_it_through_the_tree_widget() {
    let mut d = driver();
    let (x, y) = at(&d, "file01");
    d.dispatch(UiEvent::Scroll {
        widget: None,
        delta: ScrollDelta::new(0.0, -1.0),
        position: Point::new(x, y),
    });

    assert!(d.app().tree().scroll_offset > 0, "{:?}", d.app().events);
    assert!(!d.screen_contains("file00"), "{}", d.screen());
}

#[test]
fn a_track_click_below_the_thumb_pages_the_section() {
    let mut d = driver();
    let gutter_x = AREA.x + AREA.width - 1.0 + 0.5;
    d.click(gutter_x, AREA.y + 5.0);

    assert!(
        d.app()
            .events
            .contains(&MultiSectionViewEvent::ScrollChanged {
                section: Some(0),
                offset: 5.0,
            }),
        "{:?}",
        d.app().events
    );
    assert!(d.screen_contains("file05"), "{}", d.screen());
}

#[test]
fn dragging_a_divider_resizes_the_sections_either_side() {
    let mut d = driver();
    // The divider under the 6-row files section.
    let divider_y = AREA.y + 6.0;
    let x = AREA.x + 4.0;
    d.drag(x, divider_y, x, divider_y + 2.0);

    assert!(
        d.app().events.iter().any(|e| matches!(
            e,
            MultiSectionViewEvent::SectionResized {
                above: 0,
                above_size: 8,
                below: 1,
                below_size: 1,
            }
        )),
        "{:?}",
        d.app().events
    );
}

#[test]
fn typing_into_the_search_input_reports_its_new_text() {
    let mut d = driver();
    let (x, y) = at(&d, "Search");
    d.click(x, y);
    d.type_char('r');
    d.type_char('s');

    let last = d
        .app()
        .events
        .iter()
        .rev()
        .find(|e| matches!(e, MultiSectionViewEvent::AuxInputChanged { .. }));
    assert_eq!(
        last,
        Some(&MultiSectionViewEvent::AuxInputChanged {
            section: 2,
            text: "rs".into(),
            caret: 2,
        }),
        "{:?}",
        d.app().events
    );

    d.press_named(NamedKey::Enter);
    assert_eq!(
        d.app().events.last(),
        Some(&MultiSectionViewEvent::AuxInputSubmitted {
            section: 2,
            text: "rs".into(),
        })
    );
}
