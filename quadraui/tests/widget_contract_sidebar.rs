#![cfg(feature = "tui")]
//! Black-box `TuiDriver` tests for `Sidebar` on the widget contract: the
//! sidebar reaches the frame as a type-erased `Surface::Widget`, is laid
//! out and painted as a `MultiSectionView` with each body painted through
//! its own `TreeView` or `Form`, and is registered in the app's
//! `FrameTree`, so pointer and key input route through the shared engine
//! into `Interactive::handle` — and from there into each body widget's
//! own handling.
//!
//! The sidebar is controlled: this app applies every `SidebarAction`
//! (with `Sidebar::apply`) and every form event onto the descriptor it
//! owns, and the next frame paints the result.

use quadraui::primitives::number_input::NumberInput;
use quadraui::tui::testing::TuiDriver;
use quadraui::{
    AppLogic, Backend, FieldKind, Form, FormEvent, FormField, FrameTree, NamedKey, Point, Reaction,
    Rect, ScreenLayout, SectionSize, SelectionMode, Sidebar, SidebarAction, SidebarBody,
    SidebarSection, SidebarSectionDef, StyledText, Surface, TreeRow, TreeView, UiEvent, WidgetId,
};

const COLS: u16 = 40;
const ROWS: u16 = 22;
/// A non-zero origin, so the sidebar-local and body-local translations
/// of every pointer event are exercised.
const AREA: Rect = Rect::new(2.0, 1.0, 30.0, 20.0);

fn sidebar_id() -> WidgetId {
    WidgetId::new("sidebar")
}

fn row(path: u16, text: &str, indent: u16, expanded: Option<bool>) -> TreeRow {
    TreeRow {
        path: vec![path],
        indent,
        icon: None,
        text: StyledText::plain(text),
        badge: None,
        is_expanded: expanded,
        decoration: Default::default(),
        edit: None,
    }
}

fn field(id: &str, label: &str, kind: FieldKind) -> FormField {
    FormField {
        id: WidgetId::new(id),
        label: StyledText::plain(label),
        kind,
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

/// FILES (a tree with a branch and two leaves) and OPTIONS (a number
/// input and a slider).
fn sidebar() -> Sidebar {
    let tree = TreeView {
        id: WidgetId::new("files"),
        rows: vec![
            row(0, "src", 0, Some(false)),
            row(1, "alpha.rs", 0, None),
            row(2, "beta.rs", 0, None),
        ],
        selection_mode: SelectionMode::Single,
        selected_path: None,
        scroll_offset: 0,
        items_window: None,
        scroll_px: None,
        style: Default::default(),
        has_focus: false,
    };
    let mut count = NumberInput::new(WidgetId::new("count"), 0.0, 10.0);
    count.value = 5.0;
    let form = Form {
        id: WidgetId::new("options"),
        fields: vec![
            field("count", "Count", FieldKind::NumberInput(count)),
            field(
                "zoom",
                "Zoom",
                FieldKind::Slider {
                    value: 0.0,
                    min: 0.0,
                    max: 100.0,
                    step: 1.0,
                },
            ),
        ],
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
    let mut files = SidebarSection::new(
        &SidebarSectionDef::new("files", "FILES"),
        SidebarBody::Tree(tree),
    );
    files.size = SectionSize::Fixed(6);
    let mut options = SidebarSection::new(
        &SidebarSectionDef::form("options", "OPTIONS"),
        SidebarBody::Form(form),
    );
    options.size = SectionSize::Fixed(10);
    let mut s = Sidebar::new(sidebar_id());
    s.sections = vec![files, options];
    s
}

/// The app that owns the sidebar and applies what it reports.
struct SidebarApp {
    sidebar: Sidebar,
    actions: Vec<SidebarAction>,
}

impl SidebarApp {
    fn apply_form(&mut self, event: &FormEvent) {
        let Some(SidebarBody::Form(form)) = self.sidebar.sections.get_mut(1).map(|s| &mut s.body)
        else {
            return;
        };
        for f in &mut form.fields {
            match (&mut f.kind, event) {
                (FieldKind::NumberInput(n), FormEvent::NumberChanged { id, value })
                    if *id == f.id =>
                {
                    n.value = *value;
                }
                (FieldKind::Slider { value, .. }, FormEvent::SliderChanged { id, value: v })
                    if *id == f.id =>
                {
                    *value = *v;
                }
                _ => {}
            }
        }
    }
}

impl AppLogic for SidebarApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let mut frame = ScreenLayout::new();
        frame.push(Surface::Widget {
            rect: AREA,
            widget: Box::new(self.sidebar.clone()),
        });
        frame.draw(backend);
    }

    fn frame_tree(&self, _area: ()) -> FrameTree {
        let mut tree = FrameTree::new();
        tree.push(self.sidebar.id, AREA, Box::new(self.sidebar.clone()));
        tree
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        if let UiEvent::Sidebar(id, action) = &event {
            assert_eq!(*id, sidebar_id());
            self.sidebar.apply(action);
            if let SidebarAction::Form { event, .. } = action {
                self.apply_form(event);
            }
            self.actions.push(action.clone());
        }
        Reaction::Redraw
    }
}

fn driver() -> TuiDriver<SidebarApp> {
    TuiDriver::new(
        SidebarApp {
            sidebar: sidebar(),
            actions: Vec::new(),
        },
        COLS,
        ROWS,
    )
}

fn at(driver: &TuiDriver<SidebarApp>, needle: &str) -> (f32, f32) {
    let b = driver
        .find_bounds(needle)
        .unwrap_or_else(|| panic!("{needle} on screen:\n{}", driver.screen()));
    (b.x, b.y)
}

fn number_value(d: &TuiDriver<SidebarApp>) -> f64 {
    match &d.app().sidebar.sections[1].body {
        SidebarBody::Form(f) => match &f.fields[0].kind {
            FieldKind::NumberInput(n) => n.value,
            other => panic!("number input, got {other:?}"),
        },
        _ => panic!("form section"),
    }
}

fn slider_value(d: &TuiDriver<SidebarApp>) -> f32 {
    match &d.app().sidebar.sections[1].body {
        SidebarBody::Form(f) => match &f.fields[1].kind {
            FieldKind::Slider { value, .. } => *value,
            other => panic!("slider, got {other:?}"),
        },
        _ => panic!("form section"),
    }
}

#[test]
fn headers_rows_and_fields_paint_inside_the_sidebar() {
    let d = driver();
    for needle in [
        "FILES", "OPTIONS", "src", "alpha.rs", "beta.rs", "Count", "Zoom",
    ] {
        let (x, y) = at(&d, needle);
        assert!(
            AREA.contains(Point::new(x, y)),
            "{needle} inside the sidebar"
        );
    }
}

#[test]
fn clicking_a_row_selects_it_through_the_tree() {
    let mut d = driver();
    let (x, y) = at(&d, "beta.rs");
    d.click(x + 1.0, y);
    assert!(d.app().actions.contains(&SidebarAction::RowSelected {
        section: 0,
        path: vec![2],
    }));
    let tree = d.app().sidebar.tree(0).unwrap();
    assert_eq!(tree.selected_path, Some(vec![2]));
    assert_eq!(d.app().sidebar.active_section, Some(0));
}

/// A click on a branch row's title selects it; only the chevron cells
/// before the title expand it.
#[test]
fn clicking_a_branch_title_selects_it_rather_than_expanding_it() {
    let mut d = driver();
    let (x, y) = at(&d, "src");
    d.click(x + 1.0, y);
    assert_eq!(
        d.app().actions.last(),
        Some(&SidebarAction::RowSelected {
            section: 0,
            path: vec![0],
        })
    );
    d.click(AREA.x, y);
    assert_eq!(
        d.app().actions.last(),
        Some(&SidebarAction::RowToggleExpand {
            section: 0,
            path: vec![0],
        })
    );
}

/// A press on a form section's number-input stepper steps the value
/// through the form's own handling.
#[test]
fn clicking_a_number_input_stepper_in_a_sidebar_steps_it() {
    let mut d = driver();
    let (_, y) = at(&d, "Count");
    let row = d.styled_row(y as u16);
    let plus = row
        .iter()
        .position(|(c, _)| *c == '+')
        .unwrap_or_else(|| panic!("increment stepper on the Count row:\n{}", d.screen()));
    d.click(plus as f32, y);
    assert_eq!(number_value(&d), 6.0, "{:?}", d.app().actions);
    assert!(d
        .app()
        .actions
        .iter()
        .any(|a| matches!(a, SidebarAction::Form { section: 1, .. })));
}

/// A press on a form section's slider track sets the value under the
/// pointer.
#[test]
fn clicking_a_slider_track_in_a_sidebar_sets_its_value() {
    let mut d = driver();
    let (_, y) = at(&d, "Zoom");
    let row = d.styled_row(y as u16);
    let close = row
        .iter()
        .rposition(|(c, _)| *c == ']')
        .unwrap_or_else(|| panic!("slider track on the Zoom row:\n{}", d.screen()));
    d.click(close as f32 - 1.0, y);
    assert!(
        slider_value(&d) > 50.0,
        "a press near the track's end sets a high value: {} {:?}",
        slider_value(&d),
        d.app().actions
    );
}

#[test]
fn right_clicking_empty_tree_space_requests_a_menu_only_when_opted_in() {
    let mut d = driver();
    // Two presses on the same cell are two clicks, not a double-click.
    d.set_double_click_folding(false);
    let (x, y) = at(&d, "beta.rs");
    let empty = (x, y + 2.0);
    d.right_click(empty.0, empty.1);
    assert!(
        !d.app()
            .actions
            .iter()
            .any(|a| matches!(a, SidebarAction::ContextMenuRequested { .. })),
        "{:?}",
        d.app().actions
    );

    d.app_mut().sidebar.sections[0].empty_space_context_menu = true;
    d.render();
    d.right_click(empty.0, empty.1);
    assert!(
        d.app().actions.iter().any(|a| matches!(
            a,
            SidebarAction::ContextMenuRequested { section: 0, path, .. } if path.is_empty()
        )),
        "{:?}",
        d.app().actions
    );
}

#[test]
fn tab_moves_the_active_section_marker() {
    let mut d = driver();
    let (x, y) = at(&d, "alpha.rs");
    d.click(x + 1.0, y);
    d.press_named(NamedKey::Tab);
    assert_eq!(d.app().sidebar.active_section, Some(1));
    let (_, marker_y) = at(&d, "▶");
    assert_eq!(marker_y, at(&d, "OPTIONS").1);
}
