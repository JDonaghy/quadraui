//! `HoverBoard`: one of each widget that tints under the pointer, painted
//! through `DrawsPrimitives::draw_dyn(.., &())`, plus the backend-neutral
//! probe that drives the pointer over each one and reads the rendered
//! frame back.
//!
//! The board's `AppLogic::handle` ignores every event, so nothing in the
//! frame changes except what the runtime's ambient pointer state paints:
//! a hovered or pressed sub-part, and nothing else. Every assertion is on
//! the rendered output — pixels on GTK, macOS and Win, cell styles on
//! TUI — never on a widget's own state.
//!
//! Each `tests/hover_channel_<backend>.rs` file wraps its driver in
//! [`HoverDriver`] and runs [`check`] once per [`Target`]. Each of
//! those files uses only its own half of [`BoardLayout`] (cells or
//! pixels), hence the module-wide `dead_code` allowance.

#![allow(dead_code)]

use quadraui::interaction::reset_ambient_interaction;
use quadraui::interactive::SubTarget;
use quadraui::primitive::{painted_hit, with_painted_widgets};
use quadraui::{
    ActivityBar, ActivityItem, AppLogic, Backend, Button, Color, ContextMenu, ContextMenuItem,
    ContextMenuPlacement, DrawsPrimitives, MenuBar, MenuBarItem, Palette, PaletteItem, Reaction,
    Rect, SidebarPanel, StatusBar, StatusBarSegment, StyledText, TabBar, TabItem, Theme, Toolbar,
    ToolbarButton, UiEvent, WidgetId,
};

/// Where each widget sits on the board, in the backend's own units.
#[derive(Debug, Clone, Copy)]
pub struct BoardLayout {
    pub menu_bar: Rect,
    pub toolbar: Rect,
    pub tab_bar: Rect,
    pub activity_bar: Rect,
    pub sidebar: Rect,
    pub button: Rect,
    pub context_menu: Rect,
    pub palette: Rect,
    pub status_bar: Rect,
    /// A point no widget covers: moving here clears hover.
    pub off: (f32, f32),
}

impl BoardLayout {
    /// The layout for a 100×40 cell grid.
    pub const CELL_COLS: u16 = 100;
    pub const CELL_ROWS: u16 = 40;

    pub fn cells() -> Self {
        Self {
            menu_bar: Rect::new(0.0, 0.0, 100.0, 1.0),
            toolbar: Rect::new(0.0, 2.0, 70.0, 1.0),
            tab_bar: Rect::new(0.0, 4.0, 80.0, 1.0),
            activity_bar: Rect::new(0.0, 6.0, 3.0, 15.0),
            sidebar: Rect::new(5.0, 6.0, 30.0, 12.0),
            button: Rect::new(38.0, 6.0, 14.0, 1.0),
            context_menu: Rect::new(56.0, 6.0, 24.0, 9.0),
            palette: Rect::new(38.0, 18.0, 44.0, 14.0),
            status_bar: Rect::new(0.0, 39.0, 100.0, 1.0),
            off: (95.5, 30.5),
        }
    }

    /// The layout for an 800×600 pixel surface.
    pub const PIXEL_WIDTH: u32 = 800;
    pub const PIXEL_HEIGHT: u32 = 600;

    pub fn pixels() -> Self {
        Self {
            menu_bar: Rect::new(0.0, 0.0, 800.0, 24.0),
            toolbar: Rect::new(0.0, 32.0, 520.0, 30.0),
            tab_bar: Rect::new(0.0, 70.0, 620.0, 34.0),
            activity_bar: Rect::new(0.0, 112.0, 48.0, 240.0),
            sidebar: Rect::new(60.0, 112.0, 240.0, 200.0),
            button: Rect::new(320.0, 112.0, 140.0, 28.0),
            context_menu: Rect::new(480.0, 112.0, 220.0, 180.0),
            palette: Rect::new(320.0, 320.0, 420.0, 220.0),
            status_bar: Rect::new(0.0, 576.0, 800.0, 24.0),
            off: (780.0, 420.0),
        }
    }
}

pub const MENU_BAR: &str = "hover:menu_bar";
pub const TOOLBAR: &str = "hover:toolbar";
pub const TAB_BAR: &str = "hover:tab_bar";
pub const ACTIVITY_BAR: &str = "hover:activity_bar";
pub const SIDEBAR: &str = "hover:sidebar";
pub const SIDEBAR_TOOLBAR: &str = "hover:sidebar:toolbar";
pub const BUTTON: &str = "hover:button";
pub const CONTEXT_MENU: &str = "hover:context_menu";
pub const PALETTE: &str = "hover:palette";
pub const STATUS_BAR: &str = "hover:status_bar";

fn id(name: &str) -> WidgetId {
    WidgetId::new(name)
}

fn action(name: &str, label: &str) -> ToolbarButton {
    ToolbarButton::Action {
        id: id(name),
        label: label.to_string(),
        icon: None,
        key_hint: None,
        enabled: true,
        is_active: false,
        tooltip: String::new(),
        spec: None,
    }
}

fn segment(text: &str, action: Option<&str>) -> StatusBarSegment {
    StatusBarSegment {
        text: text.to_string(),
        fg: Color::rgb(230, 230, 230),
        bg: Color::rgb(40, 80, 120),
        bold: false,
        action_id: action.map(id),
    }
}

fn menu_item(name: &str, label: &str) -> ContextMenuItem {
    ContextMenuItem {
        id: Some(id(name)),
        label: StyledText::plain(label),
        ..Default::default()
    }
}

fn palette_item(text: &str) -> PaletteItem {
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

fn activity_item(name: &str, glyph: &str, active: bool) -> ActivityItem {
    ActivityItem {
        id: id(name),
        icon: glyph.into(),
        tooltip: String::new(),
        is_active: active,
        is_keyboard_selected: false,
    }
}

/// An activity bar built from its wire form, the way a binding host
/// builds one, so the fixture names only the fields it sets.
fn activity_bar() -> ActivityBar {
    let items = vec![
        activity_item("hover:act:files", "F", true),
        activity_item("hover:act:search", "S", false),
        activity_item("hover:act:git", "G", false),
    ];
    serde_json::from_value(serde_json::json!({
        "id": ACTIVITY_BAR,
        "top_items": items,
    }))
    .expect("activity bar wire form")
}

/// The board: every widget the pointer can tint, and no state beyond
/// the descriptors themselves.
pub struct HoverBoard {
    layout: BoardLayout,
    menu_bar: MenuBar,
    toolbar: Toolbar,
    tab_bar: TabBar,
    activity_bar: ActivityBar,
    sidebar: SidebarPanel,
    button: Button,
    context_menu: ContextMenu,
    palette: Palette,
    status_bar: StatusBar,
}

impl HoverBoard {
    pub fn new(layout: BoardLayout) -> Self {
        let menu_bar = MenuBar {
            id: id(MENU_BAR),
            items: [("file", "&File"), ("edit", "&Edit"), ("view", "&View")]
                .iter()
                .map(|(name, label)| MenuBarItem {
                    id: id(&format!("hover:menu:{name}")),
                    label: (*label).to_string(),
                    disabled: false,
                    submenu: None,
                })
                .collect(),
            open_item: None,
            focused_item: None,
        };
        let toolbar = Toolbar::new(id(TOOLBAR)).with_buttons(vec![
            action("hover:tb:build", "Build"),
            action("hover:tb:test", "Test"),
            ToolbarButton::Group {
                id: id("hover:tb:align"),
                items: vec![
                    action("hover:tb:left", "Left"),
                    action("hover:tb:right", "Right"),
                ],
                exclusive: false,
                label: None,
            },
            ToolbarButton::MenuButton {
                id: id("hover:tb:sort"),
                label: "Sort".to_string(),
                icon: None,
                menu: vec![menu_item("hover:tb:sort:name", "By name")],
            },
        ]);
        let tab_bar = TabBar {
            id: id(TAB_BAR),
            tabs: ["main.rs", "lib.rs", "mod.rs"]
                .iter()
                .enumerate()
                .map(|(i, label)| TabItem {
                    label: (*label).to_string(),
                    is_active: i == 0,
                    ..TabItem::default()
                })
                .collect(),
            ..TabBar::default()
        };
        let sidebar = SidebarPanel {
            id: id(SIDEBAR),
            toolbar: Some(Toolbar::new(id(SIDEBAR_TOOLBAR)).with_buttons(vec![
                action("hover:side:refresh", "Refresh"),
                action("hover:side:collapse", "Collapse"),
            ])),
            toolbar_height: None,
        };
        let context_menu = ContextMenu {
            id: id(CONTEXT_MENU),
            items: vec![
                menu_item("hover:cm:cut", "Cut"),
                menu_item("hover:cm:copy", "Copy"),
                menu_item("hover:cm:paste", "Paste"),
            ],
            selected_idx: 0,
            bg: None,
            placement: ContextMenuPlacement::AnchorPoint,
            glyphs: Default::default(),
        };
        let palette = Palette {
            id: id(PALETTE),
            title: "Pick".to_string(),
            items: vec![palette_item("alpha"), palette_item("beta")],
            total_count: 2,
            has_focus: false,
            create_label: Some("Create branch".to_string()),
            ..Palette::default()
        };
        let status_bar = StatusBar {
            id: id(STATUS_BAR),
            left_segments: vec![
                segment(" MAIN ", Some("hover:sb:main")),
                segment(" SYNC ", Some("hover:sb:sync")),
            ],
            right_segments: vec![segment(" UTF-8 ", None)],
        };
        Self {
            layout,
            menu_bar,
            toolbar,
            tab_bar,
            activity_bar: activity_bar(),
            sidebar,
            button: Button::new(id(BUTTON), "Deploy"),
            context_menu,
            palette,
            status_bar,
        }
    }
}

impl AppLogic for HoverBoard {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let l = self.layout;
        backend.draw_dyn(l.menu_bar, &self.menu_bar, &());
        backend.draw_dyn(l.toolbar, &self.toolbar, &());
        backend.draw_dyn(l.tab_bar, &self.tab_bar, &());
        backend.draw_dyn(l.activity_bar, &self.activity_bar, &());
        backend.draw_dyn(l.sidebar, &self.sidebar, &());
        backend.draw_dyn(l.button, &self.button, &());
        backend.draw_dyn(l.context_menu, &self.context_menu, &());
        backend.draw_dyn(l.palette, &self.palette, &());
        backend.draw_dyn(l.status_bar, &self.status_bar, &());
    }

    fn handle(&mut self, _event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        Reaction::Continue
    }
}

/// How a probe finds the sub-part it hovers.
#[derive(Debug, Clone, Copy)]
pub enum Locate {
    /// The center of this painted text; the sub-part is whatever the
    /// painted-layout record says lies under it.
    Text(&'static str),
    /// This widget's `SubTarget`, for a sub-part with no text of its own.
    Sub(&'static str, u64),
}

/// The colour a tint paints, given the theme and the resting colour
/// under the same pixel.
pub type Tint = fn(&Theme, Color) -> Color;

/// What pressing a sub-part looks like.
#[derive(Debug, Clone, Copy)]
pub enum PressLook {
    /// A press paints its own look, different from hover; on a pixel
    /// backend, `Some` names its colour.
    Distinct(Option<Tint>),
    /// The widget has no press look; a press shows the hover tint.
    SameAsHover,
}

/// One sub-part the pointer tints.
#[derive(Debug, Clone, Copy)]
pub struct Target {
    pub widget: &'static str,
    pub at: Locate,
    /// A neighbouring sub-part that must stay at rest while `at` is
    /// hovered or pressed.
    pub sibling: Option<Locate>,
    /// On a pixel backend, the hover colour.
    pub hover: Option<Tint>,
    pub press: PressLook,
}

fn menu_hover(t: &Theme, base: Color) -> Color {
    t.menu_item_hover_bg(base)
}

fn menu_pressed(t: &Theme, base: Color) -> Color {
    t.menu_item_pressed_bg(base)
}

fn toolbar_hover(t: &Theme, _: Color) -> Color {
    t.hover_bg
}

fn toolbar_pressed(t: &Theme, _: Color) -> Color {
    t.selected_bg
}

pub fn status_bar() -> Target {
    Target {
        widget: STATUS_BAR,
        at: Locate::Text("MAIN"),
        sibling: Some(Locate::Text("SYNC")),
        hover: Some(|_, base| base.lighten(0.05)),
        press: PressLook::Distinct(Some(|_, base| base.darken(0.05))),
    }
}

pub fn toolbar_action() -> Target {
    Target {
        widget: TOOLBAR,
        at: Locate::Text("Build"),
        sibling: Some(Locate::Text("Test")),
        hover: Some(toolbar_hover),
        press: PressLook::Distinct(Some(toolbar_pressed)),
    }
}

pub fn toolbar_group_child() -> Target {
    Target {
        widget: TOOLBAR,
        at: Locate::Text("Left"),
        sibling: Some(Locate::Text("Right")),
        hover: Some(toolbar_hover),
        press: PressLook::Distinct(Some(toolbar_pressed)),
    }
}

pub fn toolbar_menu_button() -> Target {
    Target {
        widget: TOOLBAR,
        at: Locate::Text("Sort"),
        sibling: Some(Locate::Text("Test")),
        hover: Some(toolbar_hover),
        press: PressLook::Distinct(None),
    }
}

pub fn sidebar_header() -> Target {
    Target {
        widget: SIDEBAR,
        at: Locate::Text("Refresh"),
        sibling: Some(Locate::Text("Collapse")),
        hover: Some(toolbar_hover),
        press: PressLook::Distinct(Some(toolbar_pressed)),
    }
}

pub fn menu_bar() -> Target {
    Target {
        widget: MENU_BAR,
        at: Locate::Text("Edit"),
        sibling: Some(Locate::Text("View")),
        hover: Some(menu_hover),
        press: PressLook::Distinct(Some(menu_pressed)),
    }
}

pub fn context_menu() -> Target {
    Target {
        widget: CONTEXT_MENU,
        at: Locate::Text("Copy"),
        sibling: Some(Locate::Text("Paste")),
        hover: Some(menu_hover),
        press: PressLook::Distinct(Some(menu_pressed)),
    }
}

pub fn palette_create_row() -> Target {
    Target {
        widget: PALETTE,
        at: Locate::Text("Create branch"),
        sibling: Some(Locate::Text("beta")),
        hover: Some(menu_hover),
        press: PressLook::Distinct(Some(menu_pressed)),
    }
}

pub fn activity_bar_item() -> Target {
    Target {
        widget: ACTIVITY_BAR,
        at: Locate::Sub(ACTIVITY_BAR, 1),
        sibling: Some(Locate::Sub(ACTIVITY_BAR, 2)),
        hover: Some(|t, _| t.tab_bar_bg.lighten(0.10)),
        press: PressLook::SameAsHover,
    }
}

pub fn button() -> Target {
    Target {
        widget: BUTTON,
        at: Locate::Text("Deploy"),
        sibling: None,
        hover: Some(|t, base| t.button_hover_bg(base)),
        press: PressLook::Distinct(Some(|t, base| t.button_pressed_bg(base))),
    }
}

pub fn tab_bar() -> Target {
    Target {
        widget: TAB_BAR,
        at: Locate::Text("lib.rs"),
        sibling: Some(Locate::Text("mod.rs")),
        hover: Some(|t, base| t.list_hover_bg(base)),
        press: PressLook::Distinct(Some(|t, base| t.list_pressed_bg(base))),
    }
}

/// A whole rendered frame, one sample per pixel or cell.
pub struct Grab<S> {
    pub width: usize,
    pub height: usize,
    pub samples: Vec<S>,
}

impl<S: Copy> Grab<S> {
    fn at(&self, x: usize, y: usize) -> S {
        self.samples[y * self.width + x]
    }
}

/// The per-backend half of a hover probe: pointer input and frame
/// readback through the backend's own test driver.
pub trait HoverDriver {
    /// One pixel (RGB) or one cell (its full style).
    type Sample: Copy + PartialEq + std::fmt::Debug;

    /// How far a probe stays inside a sub-part's edges. Pixel backends
    /// skip the antialiased border; a cell grid has none.
    const INSET: f32;

    fn move_to(&mut self, x: f32, y: f32);
    fn press(&mut self, x: f32, y: f32);
    fn release(&mut self, x: f32, y: f32);
    fn text_bounds(&mut self, needle: &str) -> Option<Rect>;
    fn theme(&self) -> Theme;
    fn grab(&mut self) -> Grab<Self::Sample>;
    /// Whether two samples differ beyond readback noise.
    fn differs(a: Self::Sample, b: Self::Sample) -> bool;
    /// A sample's colour, on a backend whose tints are theme colours.
    fn rgb(sample: Self::Sample) -> Option<Color>;
}

/// The point the pointer moves to and the region whose samples change.
fn resolve<D: HoverDriver>(driver: &mut D, widget: &str, at: Locate) -> ((f32, f32), Rect) {
    let widget = id(widget);
    match at {
        Locate::Text(needle) => {
            let b = driver
                .text_bounds(needle)
                .unwrap_or_else(|| panic!("{needle:?} is painted"));
            let point = (b.x + b.width / 2.0, b.y + b.height / 2.0);
            let (hit, sub) = painted_hit(point.0, point.1)
                .unwrap_or_else(|| panic!("{needle:?} lies inside a painted widget"));
            assert_eq!(hit, widget, "{needle:?} is painted by {widget:?}");
            let region = with_painted_widgets(|p| match sub {
                Some(sub) => p.painted_sub_rect(&hit, sub),
                None => p.painted_layout(&hit).map(|(r, _)| r),
            })
            .expect("the hit widget is in the painted record");
            (point, region)
        }
        Locate::Sub(owner, n) => {
            let owner = id(owner);
            assert_eq!(owner, widget, "a sub-part of {widget:?}");
            let region = with_painted_widgets(|p| p.painted_sub_rect(&owner, SubTarget(n)))
                .unwrap_or_else(|| panic!("{owner:?} painted sub-part {n}"));
            let point = (
                region.x + region.width / 2.0,
                region.y + region.height / 2.0,
            );
            (point, region)
        }
    }
}

fn region_points(region: Rect, inset: f32, width: usize, height: usize) -> Vec<(usize, usize)> {
    let x0 = (region.x + inset).ceil().max(0.0) as usize;
    let y0 = (region.y + inset).ceil().max(0.0) as usize;
    let x1 = ((region.x + region.width - inset).floor().max(0.0) as usize).min(width);
    let y1 = ((region.y + region.height - inset).floor().max(0.0) as usize).min(height);
    let mut points = Vec::new();
    for y in y0..y1 {
        for x in x0..x1 {
            points.push((x, y));
        }
    }
    assert!(!points.is_empty(), "empty probe region {region:?}");
    points
}

fn distance(a: Color, b: Color) -> i32 {
    (a.r as i32 - b.r as i32)
        .abs()
        .max((a.g as i32 - b.g as i32).abs())
        .max((a.b as i32 - b.b as i32).abs())
}

/// Whether two pixels differ beyond a one-level rounding difference.
pub fn pixels_differ(a: Color, b: Color) -> bool {
    distance(a, b) > 1
}

/// Within this many levels per channel, a pixel shows the tint colour.
const TINT_TOLERANCE: i32 = 6;

/// `after` shows a tint over `region`: at least a fifth of its samples
/// changed from `before`, and, when `tint` names the colour on a pixel
/// backend, at least a tenth of them now show it.
fn assert_tinted<D: HoverDriver>(
    what: &str,
    theme: &Theme,
    before: &Grab<D::Sample>,
    after: &Grab<D::Sample>,
    points: &[(usize, usize)],
    tint: Option<Tint>,
) {
    let changed = points
        .iter()
        .filter(|&&(x, y)| D::differs(before.at(x, y), after.at(x, y)))
        .count();
    assert!(
        changed * 5 >= points.len(),
        "{what}: only {changed} of {} samples changed; first sample {:?} -> {:?}",
        points.len(),
        before.at(points[0].0, points[0].1),
        after.at(points[0].0, points[0].1),
    );
    let Some(tint) = tint else { return };
    let mut matched = 0;
    let mut first_miss = None;
    for &(x, y) in points {
        let (Some(rest), Some(now)) = (D::rgb(before.at(x, y)), D::rgb(after.at(x, y))) else {
            return;
        };
        let expected = tint(theme, rest);
        if D::differs(before.at(x, y), after.at(x, y)) && distance(now, expected) <= TINT_TOLERANCE
        {
            matched += 1;
        } else if first_miss.is_none() {
            first_miss = Some((rest, now, expected));
        }
    }
    assert!(
        matched * 10 >= points.len(),
        "{what}: only {matched} of {} samples show the tint colour; first miss \
         (rest, painted, expected) = {first_miss:?}",
        points.len(),
    );
}

fn assert_same<D: HoverDriver>(
    what: &str,
    a: &Grab<D::Sample>,
    b: &Grab<D::Sample>,
    points: &[(usize, usize)],
) {
    let diff = points
        .iter()
        .find(|&&(x, y)| D::differs(a.at(x, y), b.at(x, y)));
    assert!(
        diff.is_none(),
        "{what}: sample at {diff:?} changed: {:?} -> {:?}",
        diff.map(|&(x, y)| a.at(x, y)),
        diff.map(|&(x, y)| b.at(x, y)),
    );
}

fn assert_differs<D: HoverDriver>(
    what: &str,
    a: &Grab<D::Sample>,
    b: &Grab<D::Sample>,
    points: &[(usize, usize)],
) {
    assert!(
        points
            .iter()
            .any(|&(x, y)| D::differs(a.at(x, y), b.at(x, y))),
        "{what}: no sample differs"
    );
}

fn whole_frame<S>(grab: &Grab<S>) -> Vec<(usize, usize)> {
    region_points(
        Rect::new(0.0, 0.0, grab.width as f32, grab.height as f32),
        0.0,
        grab.width,
        grab.height,
    )
}

/// Clear the pointer state this thread carries over from an earlier
/// test, before a driver paints its first frame.
pub fn fresh_pointer() {
    reset_ambient_interaction();
}

/// Drive the pointer over `target` and assert what each step renders:
///
/// 1. Hovering tints the sub-part, and only that sub-part.
/// 2. Pressing shows the press look (or the hover tint, for a widget
///    with none).
/// 3. Dragging off with the button held shows the resting look; dragging
///    back shows the press look again.
/// 4. Releasing returns to the hover tint.
/// 5. Moving onto empty space restores the whole frame to rest.
pub fn check<D: HoverDriver>(driver: &mut D, off: (f32, f32), target: Target) {
    let name = target.widget;
    assert!(
        painted_hit(off.0, off.1).is_none(),
        "the off point {off:?} lies outside every widget"
    );
    driver.move_to(off.0, off.1);
    let theme = driver.theme();
    let (point, region) = resolve(driver, target.widget, target.at);
    let sibling = target.sibling.map(|s| resolve(driver, target.widget, s).1);

    let rest = driver.grab();
    let (w, h) = (rest.width, rest.height);
    let points = region_points(region, D::INSET, w, h);
    let sibling_points = sibling.map(|r| region_points(r, D::INSET, w, h));
    let all = whole_frame(&rest);

    driver.move_to(point.0, point.1);
    let hover = driver.grab();
    assert_tinted::<D>(
        &format!("{name}: hover"),
        &theme,
        &rest,
        &hover,
        &points,
        target.hover,
    );
    if let Some(sp) = &sibling_points {
        assert_same::<D>(&format!("{name}: sibling while hovered"), &rest, &hover, sp);
    }

    driver.press(point.0, point.1);
    let pressed = driver.grab();
    match target.press {
        PressLook::Distinct(tint) => {
            assert_tinted::<D>(
                &format!("{name}: pressed"),
                &theme,
                &rest,
                &pressed,
                &points,
                tint,
            );
            assert_differs::<D>(
                &format!("{name}: pressed vs hovered"),
                &hover,
                &pressed,
                &points,
            );
        }
        PressLook::SameAsHover => {
            assert_same::<D>(
                &format!("{name}: pressed shows hover"),
                &hover,
                &pressed,
                &points,
            );
        }
    }
    if let Some(sp) = &sibling_points {
        assert_same::<D>(
            &format!("{name}: sibling while pressed"),
            &rest,
            &pressed,
            sp,
        );
    }

    driver.move_to(off.0, off.1);
    let dragged_off = driver.grab();
    assert_same::<D>(
        &format!("{name}: pressed then dragged off"),
        &rest,
        &dragged_off,
        &all,
    );
    driver.move_to(point.0, point.1);
    let dragged_back = driver.grab();
    assert_same::<D>(
        &format!("{name}: dragged back while pressed"),
        &pressed,
        &dragged_back,
        &points,
    );

    driver.release(point.0, point.1);
    let released = driver.grab();
    assert_same::<D>(
        &format!("{name}: released returns to hover"),
        &hover,
        &released,
        &points,
    );

    driver.move_to(off.0, off.1);
    let left = driver.grab();
    assert_same::<D>(&format!("{name}: moved off"), &rest, &left, &all);
}
