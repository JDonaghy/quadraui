#![cfg(feature = "tui")]
//! Black-box `TuiDriver` test for `ContextMenu` on the widget contract:
//! the popup reaches the frame as a type-erased `Surface::Widget`, is
//! painted through `DynWidget::paint` → `Primitive::paint_cells`, and is
//! registered in the app's `FrameTree` so input is routed through the
//! shared widget-contract engine — `UiEvent` into
//! `quadraui::runtime::preprocess_event`'s widget routing (hit test,
//! click-to-focus, `Interactive::handle`) and back out as the `UiEvent`
//! this test's own `AppLogic::handle` applies onto the owned
//! `ContextMenu`, which the next `render` paints.
//!
//! Never a direct call into `ContextMenu::handle_key`/`Primitive::layout`
//! that would skip the routing engine. Two arms the engine cannot deliver
//! are covered by the widget module's own unit tests instead:
//!
//! - **Hover.** `route_widget_hover` only synthesises
//!   `PointerKind::Enter`/`Leave` for a *change of hovered widget* and
//!   deliberately discards their outcome, so the popup's
//!   `PointerKind::Move` arm (highlight-follows-pointer, open-submenu-on-
//!   hover) is only reachable under a pointer capture a menu never takes.
//! - **Assistive-technology `Click`.** `RoutedEvent::Action` has no
//!   `UiEvent` an app can dispatch through the driver.

use quadraui::tui::ratatui_color;
use quadraui::tui::testing::TuiDriver;
use quadraui::{
    AppLogic, Backend, ContextMenu, ContextMenuGlyphs, ContextMenuItem, ContextMenuPlacement,
    FrameTree, Icon, ListViewEvent, NamedKey, Reaction, Rect, ScreenLayout, StyledText, Surface,
    Theme, UiEvent, WidgetId,
};

const COLS: u16 = 40;
const ROWS: u16 = 12;
/// The popup's whole box, border ring included: `Primitive::layout`
/// reserves one cell a side out of this rect for the box chrome.
const POPUP: Rect = Rect::new(2.0, 1.0, 24.0, 8.0);

/// One context-menu popup, drawn and routed as a `Surface::Widget`, plus
/// a record of what the host was told to do with it.
struct ContextMenuApp {
    menu: ContextMenu,
    activated: Vec<WidgetId>,
    submenus_opened: Vec<usize>,
    dismissed: bool,
}

fn leaf(id: &str, label: &str) -> ContextMenuItem {
    ContextMenuItem {
        id: Some(WidgetId::new(id)),
        label: StyledText::plain(label),
        ..Default::default()
    }
}

impl ContextMenuApp {
    fn new() -> Self {
        Self {
            menu: ContextMenu {
                id: WidgetId::new("context_menu:editor"),
                items: vec![
                    ContextMenuItem {
                        detail: Some(StyledText::plain("Ctrl+C")),
                        ..leaf("copy", "Copy")
                    },
                    leaf("paste", "Paste"),
                    // A separator: no id.
                    ContextMenuItem::default(),
                    ContextMenuItem {
                        disabled: true,
                        ..leaf("delete", "Delete")
                    },
                    ContextMenuItem {
                        submenu: Some(vec![leaf("email", "Email")]),
                        ..leaf("share", "Share")
                    },
                ],
                selected_idx: 0,
                bg: None,
                placement: ContextMenuPlacement::AnchorPoint,
                glyphs: Default::default(),
            },
            activated: Vec::new(),
            submenus_opened: Vec::new(),
            dismissed: false,
        }
    }
}

impl AppLogic for ContextMenuApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        if self.dismissed {
            return;
        }
        let mut frame = ScreenLayout::new();
        frame.push(Surface::Widget {
            rect: POPUP,
            widget: Box::new(self.menu.clone()),
        });
        frame.draw(backend);
    }

    fn frame_tree(&self, _area: ()) -> FrameTree {
        let mut tree = FrameTree::new();
        if !self.dismissed {
            tree.push(self.menu.id, POPUP, Box::new(self.menu.clone()));
        }
        tree
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        match event {
            UiEvent::ContextMenuItemActivated(item) => {
                self.activated.push(item);
                self.dismissed = true;
                Reaction::Redraw
            }
            UiEvent::ContextMenuDismissed => {
                self.dismissed = true;
                Reaction::Redraw
            }
            UiEvent::List(id, ListViewEvent::SelectionChanged { idx }) if id == self.menu.id => {
                self.menu.selected_idx = idx;
                Reaction::Redraw
            }
            // A submenu parent asked for its child popup. A real host
            // paints that child beside this one; here it is enough to
            // record which row asked, and keep the parent open.
            UiEvent::List(id, ListViewEvent::ItemActivated { idx }) if id == self.menu.id => {
                self.submenus_opened.push(idx);
                Reaction::Redraw
            }
            _ => Reaction::Continue,
        }
    }
}

fn driver() -> TuiDriver<ContextMenuApp> {
    TuiDriver::new(ContextMenuApp::new(), COLS, ROWS)
}

/// Click the separator row so the widget-contract click-to-focus step
/// runs — `ContextMenu::focus()` is `FocusSpec::Stop`, so nothing routes
/// a key to the popup until then — without activating anything.
fn focus(driver: &mut TuiDriver<ContextMenuApp>) {
    let paste = driver.find_bounds("Paste").expect("row 1 painted");
    driver.click(paste.x, paste.y + 1.0);
    assert!(driver.app().activated.is_empty());
    assert!(!driver.app().dismissed);
}

fn row_chars(driver: &TuiDriver<ContextMenuApp>, y: f32) -> Vec<char> {
    driver
        .screen()
        .lines()
        .nth(y as usize)
        .map(|l| l.chars().collect())
        .unwrap_or_default()
}

/// The box frames exactly the rows it has: the items region is as tall
/// as the five rows themselves (a popup never stretches to fill a taller
/// box), and the border ring paints on the cells immediately outside it.
#[test]
fn a_surface_widget_popup_paints_its_box_border_around_the_rows() {
    let driver = driver();
    let share = driver.find_bounds("Share").expect("last row painted");
    let top = row_chars(&driver, POPUP.y);
    let bottom = row_chars(&driver, share.y + 1.0);
    let left = POPUP.x as usize;
    let right = (POPUP.x + POPUP.width) as usize - 1;

    assert_eq!(top[left], '┌', "{}", driver.screen());
    assert_eq!(top[right], '┐', "{}", driver.screen());
    assert_eq!(bottom[left], '└', "{}", driver.screen());
    assert_eq!(bottom[right], '┘', "{}", driver.screen());

    let first_row = row_chars(&driver, POPUP.y + 1.0);
    assert_eq!(first_row[left], '│', "{}", driver.screen());
    assert_eq!(first_row[right], '│', "{}", driver.screen());
}

#[test]
fn every_row_shape_paints_its_own_affordance() {
    let driver = driver();
    for label in ["Copy", "Paste", "Delete", "Share"] {
        assert!(
            driver.screen_contains(label),
            "{label} missing:\n{}",
            driver.screen()
        );
    }
    // The shortcut hint is right-aligned on its own row.
    assert!(driver.screen_contains("Ctrl+C"), "{}", driver.screen());
    let copy = driver.find_bounds("Copy").expect("row 0 painted");
    let hint = driver.find_bounds("Ctrl+C").expect("hint painted");
    assert_eq!(hint.y, copy.y, "{}", driver.screen());
    assert!(hint.x > copy.x, "{}", driver.screen());

    // The submenu parent shows the pull-right arrow in its last column.
    let share = driver.find_bounds("Share").expect("row 4 painted");
    let share_row = row_chars(&driver, share.y);
    assert_eq!(
        share_row[(POPUP.x + POPUP.width) as usize - 2],
        '▶',
        "{}",
        driver.screen()
    );

    // The separator row is a rule of dashes between the two halves.
    let separator = row_chars(&driver, copy.y + 2.0);
    assert_eq!(separator[POPUP.x as usize + 1], '─', "{}", driver.screen());
}

#[test]
fn the_highlighted_row_is_inverted_and_a_disabled_row_is_dimmed() {
    let driver = driver();
    let theme = Theme::default();

    let copy = driver.find_bounds("Copy").expect("row 0 painted");
    let selected = driver
        .style_at(copy.x as u16, copy.y as u16)
        .expect("cell inside the popup");
    assert_eq!(selected.bg, ratatui_color(theme.foreground));
    assert_eq!(selected.fg, ratatui_color(theme.tab_bar_bg));

    let delete = driver.find_bounds("Delete").expect("row 3 painted");
    let disabled = driver
        .style_at(delete.x as u16, delete.y as u16)
        .expect("cell inside the popup");
    assert_eq!(disabled.fg, ratatui_color(theme.muted_fg));
    assert_eq!(disabled.bg, ratatui_color(theme.tab_bar_bg));
}

#[test]
fn arrow_keys_move_the_highlight_through_the_real_routing_engine() {
    let mut driver = driver();
    focus(&mut driver);

    driver.press_named(NamedKey::Down);
    assert_eq!(driver.app().menu.selected_idx, 1);
    // Down again skips both the separator and the disabled row.
    driver.press_named(NamedKey::Down);
    assert_eq!(driver.app().menu.selected_idx, 4);
    driver.press_named(NamedKey::Up);
    assert_eq!(driver.app().menu.selected_idx, 1);
}

#[test]
fn home_and_end_reach_the_first_and_last_selectable_rows() {
    let mut driver = driver();
    focus(&mut driver);

    driver.press_named(NamedKey::End);
    assert_eq!(driver.app().menu.selected_idx, 4);
    driver.press_named(NamedKey::Home);
    assert_eq!(driver.app().menu.selected_idx, 0);
}

/// Typeahead is a debounced *buffer*, not one jump per keystroke: two
/// characters typed in quick succession search for the pair, so "p" then
/// "a" stays on "Paste" rather than jumping to whatever "a" alone would
/// match.
#[test]
fn typing_jumps_the_highlight_to_the_matching_row() {
    let mut driver = driver();
    focus(&mut driver);

    driver.type_char('p');
    assert_eq!(driver.app().menu.selected_idx, 1);
    driver.type_char('a');
    assert_eq!(driver.app().menu.selected_idx, 1);
}

#[test]
fn enter_activates_the_highlighted_row() {
    let mut driver = driver();
    focus(&mut driver);

    driver.press_named(NamedKey::Down);
    driver.press_named(NamedKey::Enter);
    assert_eq!(driver.app().activated, vec![WidgetId::new("paste")]);
    assert!(driver.app().dismissed);
}

#[test]
fn a_click_activates_the_row_under_the_pointer() {
    let mut driver = driver();
    let (x, y) = driver.find("Paste").expect("row 1 painted");
    driver.click(x, y);
    assert_eq!(driver.app().activated, vec![WidgetId::new("paste")]);
    assert!(!driver.screen_contains("Paste"), "{}", driver.screen());
}

#[test]
fn a_click_on_a_disabled_row_activates_nothing() {
    let mut driver = driver();
    let (x, y) = driver.find("Delete").expect("row 3 painted");
    driver.click(x, y);
    assert!(driver.app().activated.is_empty());
    assert!(!driver.app().dismissed, "{}", driver.screen());
}

#[test]
fn a_click_on_a_submenu_parent_asks_the_host_for_the_child_popup() {
    let mut driver = driver();
    let (x, y) = driver.find("Share").expect("row 4 painted");
    driver.click(x, y);
    assert_eq!(driver.app().submenus_opened, vec![4]);
    assert!(driver.app().activated.is_empty());
    assert!(!driver.app().dismissed);
}

#[test]
fn right_opens_the_highlighted_submenu_parent_and_left_closes_the_popup() {
    let mut driver = driver();
    focus(&mut driver);

    driver.press_named(NamedKey::End);
    driver.press_named(NamedKey::Right);
    assert_eq!(driver.app().submenus_opened, vec![4]);

    driver.press_named(NamedKey::Left);
    assert!(driver.app().dismissed);
}

/// `Primitive::paint_cells` takes no glyph argument, so the only route an
/// app-supplied check mark / submenu arrow has onto the screen is the
/// descriptor's own `ContextMenu::glyphs`.
#[test]
fn descriptor_glyphs_replace_the_default_marks_on_screen() {
    let mut app = ContextMenuApp::new();
    app.menu.items[0] = ContextMenuItem {
        checked: Some(true),
        ..leaf("copy", "Copy")
    };
    app.menu.glyphs = ContextMenuGlyphs::new()
        .with_checked_glyph("V")
        .with_submenu_arrow_glyph(">");
    let driver = TuiDriver::new(app, COLS, ROWS);

    let copy = driver.find_bounds("Copy").expect("row 0 painted");
    let checked_row = row_chars(&driver, copy.y);
    assert_eq!(
        checked_row[POPUP.x as usize + 1],
        'V',
        "{}",
        driver.screen()
    );

    let share = driver.find_bounds("Share").expect("row 4 painted");
    let share_row = row_chars(&driver, share.y);
    assert_eq!(
        share_row[(POPUP.x + POPUP.width) as usize - 2],
        '>',
        "{}",
        driver.screen()
    );
    assert!(!driver.screen_contains("▶"), "{}", driver.screen());
    assert!(!driver.screen_contains("✓"), "{}", driver.screen());
}

/// The Nerd Font half of an item's own icon: the headless driver's
/// backend reports `nerd_fonts_enabled == false`, so the cell path takes
/// the ASCII fallback — and it takes it from the live `LayoutCtx` flag the
/// resolved layout carries, not from a constant baked into the paint
/// entry point.
#[test]
fn an_item_icon_resolves_against_the_hosts_own_nerd_font_flag() {
    let mut app = ContextMenuApp::new();
    app.menu.items[0] = ContextMenuItem {
        icon: Some(Icon::new("G", "F")),
        ..leaf("copy", "Copy")
    };
    let driver = TuiDriver::new(app, COLS, ROWS);
    assert!(
        !driver.backend().nerd_fonts_enabled(),
        "the headless driver has no Nerd Font face registered"
    );

    let copy = driver.find_bounds("Copy").expect("row 0 painted");
    let row = row_chars(&driver, copy.y);
    assert_eq!(row[POPUP.x as usize + 1], 'F', "{}", driver.screen());
}

#[test]
fn escape_dismisses_the_popup_without_activating_anything() {
    let mut driver = driver();
    focus(&mut driver);

    driver.press_named(NamedKey::Escape);
    assert!(driver.app().dismissed);
    assert!(driver.app().activated.is_empty());
    assert!(!driver.screen_contains("Copy"), "{}", driver.screen());
}
