#![cfg(feature = "tui")]
//! Black-box `TuiDriver` test for `DatePicker` on the widget contract:
//! the calendar reaches the frame as a type-erased `Surface::Widget`, is
//! painted through `DynWidget::paint` → `Primitive::paint_cells`, and is
//! registered in the app's `FrameTree` so input is routed through the
//! shared widget-contract engine — `UiEvent` into
//! `quadraui::runtime::preprocess_event`'s widget routing (hit test,
//! click-to-focus, `Interactive::handle`) and back out as the `UiEvent`
//! this test's own `AppLogic::handle` applies onto the owned
//! `DatePicker`, which the next `render` paints.
//!
//! Never a direct call into `DatePicker::navigate`/`Primitive::layout`
//! that would skip the routing engine; the direct-dispatch leg an
//! out-of-process binding host takes is
//! `primitives::date_picker::interactive_tests`.

use quadraui::tui::testing::TuiDriver;
use quadraui::{
    AppLogic, Backend, CalendarDate, DatePicker, FormEvent, FrameTree, NamedKey, Reaction, Rect,
    ScreenLayout, Surface, Theme, UiEvent, Weekday, WidgetId,
};

const COLS: u16 = 40;
const ROWS: u16 = 14;
/// Seven four-column days wide; one header row, six week rows and one
/// row for the optional time field tall.
const CALENDAR: Rect = Rect::new(2.0, 1.0, 28.0, 8.0);

/// One calendar, drawn and routed as a `Surface::Widget`, plus a record
/// of every event the host was told to apply.
struct CalendarApp {
    picker: DatePicker,
    applied: Vec<String>,
}

impl CalendarApp {
    fn new() -> Self {
        let mut picker = DatePicker::new(WidgetId::new("due-date"), CalendarDate::new(2024, 3, 15));
        picker.locale.first_weekday = Weekday::Monday;
        picker.has_focus = true;
        Self {
            picker,
            applied: Vec::new(),
        }
    }
}

impl AppLogic for CalendarApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let mut frame = ScreenLayout::new();
        frame.push(Surface::Widget {
            rect: CALENDAR,
            widget: Box::new(self.picker.clone()),
        });
        frame.draw(backend);
    }

    fn frame_tree(&self, _area: ()) -> FrameTree {
        let mut tree = FrameTree::new();
        tree.push(self.picker.id, CALENDAR, Box::new(self.picker.clone()));
        tree
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        let UiEvent::Form(id, form) = event else {
            return Reaction::Continue;
        };
        if id != self.picker.id {
            return Reaction::Continue;
        }
        match form {
            // `DateFocusMoved`'s apply contract: write the cursor, and the
            // month on screen when the cursor left it.
            FormEvent::DateFocusMoved { value, .. } => {
                self.applied.push(format!("focus({value:?})"));
                self.picker.focused = value;
                if !self.picker.is_in_visible_month(value) {
                    self.picker.visible_month = value.first_of_month();
                }
            }
            FormEvent::DateChanged { value, .. } => {
                self.applied.push(format!("selected({value:?})"));
                self.picker.selected = Some(value);
            }
            _ => return Reaction::Continue,
        }
        Reaction::Redraw
    }
}

fn driver() -> TuiDriver<CalendarApp> {
    let mut driver = TuiDriver::new(CalendarApp::new(), COLS, ROWS);
    // Two deliberate single clicks on the same header control back to
    // back would otherwise fold into one double-click.
    driver.set_double_click_folding(false);
    driver
}

/// Click the cursor's own day so the widget-contract click-to-focus step
/// runs — nothing routes a key to the calendar until then. Clicking the
/// focused day also confirms it, which is what a click on a day means.
fn focus(driver: &mut TuiDriver<CalendarApp>) {
    let (x, y) = driver.find("15").expect("the cursor's day is painted");
    driver.click(x, y);
    assert_eq!(
        driver.app().picker.selected,
        Some(CalendarDate::new(2024, 3, 15))
    );
}

/// The screen position of the header's previous/next month control.
fn header_controls() -> ((f32, f32), (f32, f32)) {
    let cell_w = CALENDAR.width / 7.0;
    (
        (CALENDAR.x + cell_w / 2.0, CALENDAR.y),
        (CALENDAR.x + cell_w * 6.5, CALENDAR.y),
    )
}

/// The day the focus ring is drawn around, read off the screen: the
/// digits immediately right of the ring's one top-left corner glyph,
/// which the painter insets the digits inside so the ring can never
/// overwrite them. Coordinate-free on purpose — the cursor's day is
/// wherever the calendar put it.
fn ringed_day(driver: &TuiDriver<CalendarApp>) -> (String, u16, u16) {
    let screen = driver.screen();
    for (y, line) in screen.lines().enumerate() {
        let cells: Vec<char> = line.chars().collect();
        if let Some(x) = cells.iter().position(|c| *c == '┌') {
            let digits: String = cells[x + 1..]
                .iter()
                .take_while(|c| c.is_ascii_digit())
                .collect();
            return (digits, x as u16, y as u16);
        }
    }
    panic!("no focus ring on screen:\n{screen}");
}

#[test]
fn a_surface_widget_calendar_paints_its_month_header_and_day_grid() {
    let driver = driver();
    assert!(driver.screen_contains("2024-03"), "{}", driver.screen());

    let rows: Vec<Vec<char>> = driver
        .screen()
        .lines()
        .map(|l| l.chars().collect())
        .collect();
    // The prev/next controls own the header's outer day columns.
    let cell_w = (CALENDAR.width / 7.0) as usize;
    assert_eq!(rows[CALENDAR.y as usize][CALENDAR.x as usize], '<');
    assert_eq!(
        rows[CALENDAR.y as usize][CALENDAR.x as usize + cell_w * 6],
        '>'
    );

    // Every day of March is on screen, and the cursor's own day wears a
    // focus ring whose corner glyph never lands on a digit.
    for day in ["1", "15", "31"] {
        assert!(driver.screen_contains(day), "{day}:\n{}", driver.screen());
    }
    assert_eq!(ringed_day(&driver).0, "15", "{}", driver.screen());
}

#[test]
fn arrow_keys_move_the_cursor_by_a_day_and_a_week() {
    let mut driver = driver();
    focus(&mut driver);

    driver.press_named(NamedKey::Right);
    assert_eq!(driver.app().picker.focused, CalendarDate::new(2024, 3, 16));
    driver.press_named(NamedKey::Left);
    driver.press_named(NamedKey::Left);
    assert_eq!(driver.app().picker.focused, CalendarDate::new(2024, 3, 14));

    driver.press_named(NamedKey::Down);
    assert_eq!(driver.app().picker.focused, CalendarDate::new(2024, 3, 21));
    driver.press_named(NamedKey::Up);
    driver.press_named(NamedKey::Up);
    assert_eq!(driver.app().picker.focused, CalendarDate::new(2024, 3, 7));

    // The ring follows the cursor on screen, not just in the descriptor.
    let (day, ring_x, ring_y) = ringed_day(&driver);
    assert_eq!(day, "7", "{}", driver.screen());
    let style = driver
        .style_at(ring_x, ring_y)
        .expect("the ring's own cell");
    assert_eq!(
        style.fg,
        quadraui::tui::ratatui_color(Theme::default().accent_fg)
    );
}

#[test]
fn page_keys_move_the_cursor_by_a_month_and_repaint_the_grid() {
    let mut driver = driver();
    focus(&mut driver);

    driver.press_named(NamedKey::PageDown);
    assert_eq!(driver.app().picker.focused, CalendarDate::new(2024, 4, 15));
    assert_eq!(
        driver.app().picker.visible_month,
        CalendarDate::new(2024, 4, 1)
    );
    assert!(driver.screen_contains("2024-04"), "{}", driver.screen());

    driver.press_named(NamedKey::PageUp);
    assert_eq!(driver.app().picker.focused, CalendarDate::new(2024, 3, 15));
    assert!(driver.screen_contains("2024-03"), "{}", driver.screen());
}

#[test]
fn enter_commits_the_day_under_the_cursor() {
    let mut driver = driver();
    focus(&mut driver);
    driver.press_named(NamedKey::Right);
    driver.press_named(NamedKey::Enter);
    assert_eq!(
        driver.app().picker.selected,
        Some(CalendarDate::new(2024, 3, 16))
    );
    // The selection wash follows it on screen.
    let selected = driver.find_bounds("16").expect("the committed day");
    let style = driver
        .style_at(selected.x as u16, selected.y as u16)
        .expect("a cell inside the committed day");
    assert_eq!(
        style.bg,
        quadraui::tui::ratatui_color(Theme::default().selected_bg)
    );
}

#[test]
fn clicking_a_day_in_the_month_on_screen_commits_it() {
    let mut driver = driver();
    let (x, y) = driver.find("22").expect("a day in March");
    driver.click(x, y);
    assert_eq!(
        driver.app().picker.selected,
        Some(CalendarDate::new(2024, 3, 22))
    );
}

#[test]
fn clicking_the_header_controls_pages_the_month() {
    let mut driver = driver();
    let ((prev_x, prev_y), (next_x, next_y)) = header_controls();

    driver.click(next_x, next_y);
    assert_eq!(
        driver.app().picker.visible_month,
        CalendarDate::new(2024, 4, 1)
    );
    assert!(driver.screen_contains("2024-04"), "{}", driver.screen());

    driver.click(prev_x, prev_y);
    driver.click(prev_x, prev_y);
    assert_eq!(
        driver.app().picker.visible_month,
        CalendarDate::new(2024, 2, 1)
    );
    assert!(driver.screen_contains("2024-02"), "{}", driver.screen());
}

#[test]
fn a_time_field_paints_its_clock_under_the_grid() {
    let mut driver = driver();
    driver.app_mut().picker.time = Some(quadraui::TimeOfDay::new(9, 30));
    driver.render();
    assert!(driver.screen_contains("09:30"), "{}", driver.screen());
}
