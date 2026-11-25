#![cfg(feature = "tui")]
//! Black-box `TuiDriver` test for `Palette` on the widget contract: the
//! command palette reaches the frame as a type-erased `Surface::Widget`,
//! is painted through `DynWidget::paint` → `Primitive::paint_cells`, and
//! is registered in the app's `FrameTree` so input is routed through the
//! shared widget-contract engine — `UiEvent` into
//! `quadraui::runtime::preprocess_event`'s widget routing (hit test,
//! click-to-focus, `Interactive::handle`) and back out as the `UiEvent`
//! this test's own `AppLogic::handle` applies onto the owned `Palette`,
//! which the next `render` paints.
//!
//! Never a direct call into `Palette::handle`/`Primitive::layout` that
//! would skip the routing engine. The one arm the engine cannot deliver
//! is the wheel: `route_widget_event`'s `UiEvent` match has no `Scroll`
//! arm that reaches a widget as `RoutedEvent::Wheel`, and `Palette` has
//! no wheel arm of its own either — the item list scrolls by moving the
//! selection, which is covered below.

use quadraui::tui::ratatui_color;
use quadraui::tui::testing::TuiDriver;
use quadraui::{
    AppLogic, Backend, FrameTree, NamedKey, Palette, PaletteEvent, PaletteItem, Reaction, Rect,
    ScreenLayout, StyledText, Surface, Theme, UiEvent, WidgetId,
};

const COLS: u16 = 44;
const ROWS: u16 = 16;
/// Ten rows tall: one title row, two query rows (prompt + separator), six
/// item rows and the bottom border — so a seven-command source overflows
/// it and the list has to scroll.
const POPUP: Rect = Rect::new(2.0, 2.0, 32.0, 10.0);

/// The unfiltered command source the app filters against the live query.
const COMMANDS: [&str; 7] = [
    "save file",
    "save all",
    "open file",
    "close tab",
    "quit",
    "reload window",
    "toggle sidebar",
];

/// One command palette, drawn and routed as a `Surface::Widget`, plus a
/// record of what the host was told to do with it.
struct PaletteApp {
    palette: Palette,
    confirmed: Vec<String>,
    copied: Vec<String>,
    dismissed: bool,
}

/// The rows `query` matches, in source order — the filtering an app owns
/// (a `Palette` renders what it is given; see that type's own doc).
fn filter(query: &str) -> Vec<PaletteItem> {
    COMMANDS
        .iter()
        .filter(|cmd| cmd.contains(query))
        .map(|cmd| PaletteItem {
            text: StyledText::plain(*cmd),
            detail: None,
            icon: None,
            match_positions: Vec::new(),
            depth: 0,
            expandable: false,
            expanded: false,
        })
        .collect()
}

impl PaletteApp {
    fn new() -> Self {
        let palette = Palette {
            id: WidgetId::new("palette:commands"),
            title: "Commands".to_string(),
            items: filter(""),
            total_count: COMMANDS.len(),
            has_focus: true,
            ..Palette::default()
        };
        Self {
            palette,
            confirmed: Vec::new(),
            copied: Vec::new(),
            dismissed: false,
        }
    }

    /// The label of the row the palette currently highlights.
    fn selected_label(&self) -> String {
        self.palette.items[self.palette.selected_idx]
            .text
            .spans
            .iter()
            .map(|s| s.text.as_str())
            .collect()
    }
}

impl AppLogic for PaletteApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        if self.dismissed {
            return;
        }
        let mut frame = ScreenLayout::new();
        frame.push(Surface::Widget {
            rect: POPUP,
            widget: Box::new(self.palette.clone()),
        });
        frame.draw(backend);
    }

    fn frame_tree(&self, _area: ()) -> FrameTree {
        let mut tree = FrameTree::new();
        if !self.dismissed {
            tree.push(self.palette.id, POPUP, Box::new(self.palette.clone()));
        }
        tree
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        match event {
            UiEvent::Palette(ref id, ref palette_event) if *id == self.palette.id => {
                match palette_event {
                    PaletteEvent::QueryChanged { value } => {
                        self.palette.query = value.clone();
                        self.palette.query_cursor = value.len();
                        self.palette.items = filter(value);
                        self.palette.selected_idx = 0;
                        self.palette.scroll_offset = 0;
                    }
                    PaletteEvent::SelectionChanged { idx } => {
                        self.palette.selected_idx = *idx;
                    }
                    PaletteEvent::ItemConfirmed { idx } => {
                        self.confirmed.push(
                            self.palette.items[*idx]
                                .text
                                .spans
                                .iter()
                                .map(|s| s.text.as_str())
                                .collect(),
                        );
                        self.dismissed = true;
                    }
                    PaletteEvent::Closed => self.dismissed = true,
                    _ => return Reaction::Continue,
                }
                Reaction::Redraw
            }
            UiEvent::TextCopied(text) => {
                self.copied.push(text);
                Reaction::Redraw
            }
            _ => Reaction::Continue,
        }
    }
}

fn driver() -> TuiDriver<PaletteApp> {
    TuiDriver::new(PaletteApp::new(), COLS, ROWS)
}

/// Click the popup's own title row once so the widget-contract
/// click-to-focus step runs — `Palette::focus()` is `FocusSpec::Stop`, so
/// nothing routes a key to it until then. The title row is inert, so this
/// changes no palette state.
fn focus(driver: &mut TuiDriver<PaletteApp>) {
    driver.click(POPUP.x + 6.0, POPUP.y);
    assert_eq!(driver.app().palette.selected_idx, 0);
    assert_eq!(driver.app().palette.query, "");
}

#[test]
fn a_surface_widget_palette_paints_its_box_title_and_rows() {
    let driver = driver();
    let screen = driver.screen();

    // Rounded box corners at the popup's own bounds.
    let rows: Vec<Vec<char>> = screen.lines().map(|l| l.chars().collect()).collect();
    let (x0, y0) = (POPUP.x as usize, POPUP.y as usize);
    let (x1, y1) = (
        (POPUP.x + POPUP.width) as usize - 1,
        (POPUP.y + POPUP.height) as usize - 1,
    );
    assert_eq!(rows[y0][x0], '╭', "{screen}");
    assert_eq!(rows[y0][x1], '╮', "{screen}");
    assert_eq!(rows[y1][x0], '╰', "{screen}");
    assert_eq!(rows[y1][x1], '╯', "{screen}");

    // Title, the "shown/total" count and the List-mode badge.
    assert!(driver.screen_contains("Commands"), "{screen}");
    assert!(driver.screen_contains("7/7"), "{screen}");
    assert!(driver.screen_contains("[L]"), "{screen}");

    // Six item rows fit, so the seventh command is scrolled out.
    for label in &COMMANDS[..6] {
        assert!(driver.screen_contains(label), "{label} missing:\n{screen}");
    }
    assert!(!driver.screen_contains("toggle sidebar"), "{screen}");
}

#[test]
fn the_highlighted_row_is_marked_and_tinted_through_the_painted_widget() {
    let driver = driver();
    let selected = driver.find_bounds("save file").expect("row 0 visible");
    let style = driver
        .style_at(selected.x as u16, selected.y as u16)
        .expect("cell inside the popup");
    assert_eq!(style.bg, ratatui_color(Theme::default().selected_bg));

    // The `▶ ` marker sits in the two columns ahead of the label.
    let row: Vec<char> = driver
        .screen()
        .lines()
        .nth(selected.y as usize)
        .map(|l| l.chars().collect())
        .unwrap_or_default();
    assert_eq!(row[selected.x as usize - 2], '▶', "{}", driver.screen());

    // A row that isn't highlighted keeps the popup's own backdrop.
    let other = driver.find_bounds("open file").expect("row 2 visible");
    let plain = driver
        .style_at(other.x as u16, other.y as u16)
        .expect("cell inside the popup");
    assert_eq!(plain.bg, ratatui_color(Theme::default().surface_bg));
}

#[test]
fn typing_edits_the_query_and_refilters_the_list() {
    let mut driver = driver();
    focus(&mut driver);

    driver.type_char('q');
    assert_eq!(driver.app().palette.query, "q");
    // "quit" is the only command containing a 'q'.
    assert_eq!(driver.app().palette.items.len(), 1);
    assert!(driver.screen_contains("> q"), "{}", driver.screen());
    assert!(driver.screen_contains("quit"), "{}", driver.screen());
    assert!(!driver.screen_contains("save file"), "{}", driver.screen());
    // The header count tracks the filtered list.
    assert!(driver.screen_contains("1/7"), "{}", driver.screen());
}

#[test]
fn backspace_deletes_the_character_left_of_the_caret() {
    let mut driver = driver();
    focus(&mut driver);

    driver.type_char('s');
    driver.type_char('a');
    assert_eq!(driver.app().palette.query, "sa");
    driver.press_named(NamedKey::Backspace);
    assert_eq!(driver.app().palette.query, "s");
    assert!(driver.screen_contains("> s"), "{}", driver.screen());
}

#[test]
fn arrow_keys_move_the_highlight_through_the_real_routing_engine() {
    let mut driver = driver();
    focus(&mut driver);

    driver.press_named(NamedKey::Down);
    assert_eq!(driver.app().selected_label(), "save all");
    driver.press_named(NamedKey::Down);
    assert_eq!(driver.app().selected_label(), "open file");
    driver.press_named(NamedKey::Up);
    assert_eq!(driver.app().selected_label(), "save all");
    // At the top edge the palette owns the key but reports nothing.
    driver.press_named(NamedKey::Up);
    driver.press_named(NamedKey::Up);
    assert_eq!(driver.app().selected_label(), "save file");
}

/// `Home`/`End` belong to the query caret while the query row is live, so
/// they must not jump the item highlight.
#[test]
fn home_and_end_stay_with_the_query_caret() {
    let mut driver = driver();
    focus(&mut driver);
    driver.press_named(NamedKey::Down);

    driver.press_named(NamedKey::End);
    assert_eq!(driver.app().selected_label(), "save all");
    driver.press_named(NamedKey::Home);
    assert_eq!(driver.app().selected_label(), "save all");
}

/// Paging past the window's edge scrolls the highlighted row back into
/// view — the popup derives that offset itself, so the host never writes a
/// scroll field.
#[test]
fn paging_past_the_window_scrolls_the_row_into_view() {
    let mut driver = driver();
    focus(&mut driver);

    driver.press_named(NamedKey::PageDown);
    assert_eq!(driver.app().selected_label(), "toggle sidebar");
    assert_eq!(driver.app().palette.scroll_offset, 0, "host set no scroll");
    assert!(
        driver.screen_contains("toggle sidebar"),
        "{}",
        driver.screen()
    );
    assert!(!driver.screen_contains("save file"), "{}", driver.screen());
}

#[test]
fn a_click_selects_the_row_under_the_pointer() {
    let mut driver = driver();
    let (x, y) = driver.find("close tab").expect("row 3 visible");
    driver.click(x, y);
    assert_eq!(driver.app().selected_label(), "close tab");
    assert!(driver.app().confirmed.is_empty(), "a click only selects");
}

#[test]
fn a_double_click_confirms_the_row_under_the_pointer() {
    let mut driver = driver();
    let (x, y) = driver.find("open file").expect("row 2 visible");
    driver.double_click(x, y);
    assert_eq!(driver.app().confirmed, vec!["open file".to_string()]);
    assert!(driver.app().dismissed);
}

#[test]
fn enter_confirms_the_highlighted_row() {
    let mut driver = driver();
    focus(&mut driver);
    driver.press_named(NamedKey::Down);
    driver.press_named(NamedKey::Enter);
    assert_eq!(driver.app().confirmed, vec!["save all".to_string()]);
    assert!(driver.app().dismissed);
}

#[test]
fn escape_dismisses_the_palette_without_confirming_anything() {
    let mut driver = driver();
    focus(&mut driver);
    driver.press_named(NamedKey::Escape);
    assert!(driver.app().dismissed);
    assert!(driver.app().confirmed.is_empty());
    assert!(!driver.screen_contains("Commands"), "{}", driver.screen());
}

/// A press inside the popup that lands on no row is swallowed rather than
/// falling through to whatever the overlay covers.
#[test]
fn a_press_on_the_popups_own_chrome_is_swallowed() {
    let mut driver = driver();
    // The separator row between the query and the first item.
    driver.click(POPUP.x + 5.0, POPUP.y + 2.0);
    assert!(!driver.app().dismissed, "{}", driver.screen());
    assert_eq!(driver.app().palette.selected_idx, 0);
    assert!(driver.app().confirmed.is_empty());
}

/// Ctrl-A then Ctrl-C puts the query selection on the clipboard as
/// `UiEvent::TextCopied` — never `ClipboardPaste`.
#[test]
fn ctrl_c_copies_the_selected_query_text() {
    let mut driver = driver();
    focus(&mut driver);
    driver.type_char('s');
    driver.type_char('a');
    driver.ctrl_char('a');
    driver.ctrl_char('c');
    assert_eq!(driver.app().copied, vec!["sa".to_string()]);
    assert_eq!(driver.app().palette.query, "sa", "a copy changes nothing");
}
