#![cfg(any(feature = "tui", feature = "gtk"))]
//! Black-box tests for a dropdown `DatePicker`'s calendar popup on the
//! widget contract, on the TUI and GTK backends.
//!
//! The picker is a one-row date field; while the app's `open` is set its
//! calendar is a widget-owned popup (`Interactive::popups`). The tests
//! drive the real runner through `TuiDriver`/`GtkDriver`: the popup is
//! painted above the rest of the frame by the runner, hit-tested ahead of
//! the content beneath it, and dismissed by an outside press or Escape —
//! and every outcome reaches the app as the `FormEvent` it applies onto
//! the descriptor, which the next frame paints.

use quadraui::{
    AppLogic, Backend, CalendarDate, Color, DateFormat, DatePicker, DrawsPrimitives, FormEvent,
    FrameTree, Reaction, Rect, StatusBar, StatusBarSegment, UiEvent, Weekday, WidgetId,
};

/// A dropdown picker over a status bar its calendar drops across, plus
/// the descriptor writes the app was asked to make.
struct PopupApp {
    picker: DatePicker,
    field: Rect,
    base: StatusBar,
    base_rect: Rect,
    applied: Vec<String>,
}

impl PopupApp {
    fn new(field: Rect, base_rect: Rect) -> Self {
        let mut picker = DatePicker::new(WidgetId::new("due"), CalendarDate::new(2024, 3, 15));
        picker.locale.first_weekday = Weekday::Monday;
        picker.locale.format = DateFormat::UsSlash;
        picker.dropdown = true;
        let base = StatusBar {
            id: WidgetId::new("base"),
            left_segments: vec![StatusBarSegment {
                text: "#".repeat(200),
                fg: Color::rgb(200, 200, 200),
                bg: Color::rgb(30, 30, 30),
                bold: false,
                action_id: None,
            }],
            right_segments: Vec::new(),
        };
        Self {
            picker,
            field,
            base,
            base_rect,
            applied: Vec::new(),
        }
    }
}

impl AppLogic for PopupApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        backend.draw_dyn(self.field, &self.picker, &());
        // Painted after the picker, so only the runner's popup pass can
        // put the calendar above it.
        backend.draw_dyn(self.base_rect, &self.base, &());
    }

    fn frame_tree(&self, _area: ()) -> FrameTree {
        let mut tree = FrameTree::new();
        tree.push(self.picker.id, self.field, Box::new(self.picker.clone()));
        tree.push(self.base.id, self.base_rect, Box::new(self.base.clone()));
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
            FormEvent::DateOpenChanged { open, .. } => {
                self.applied.push(format!("open({open})"));
                self.picker.open = open;
            }
            FormEvent::DateFocusMoved { value, .. } => {
                self.applied.push(format!("focus({value:?})"));
                self.picker.focused = value;
                if !self.picker.is_in_visible_month(value) {
                    self.picker.visible_month = value.first_of_month();
                }
            }
            // A dropdown picker's calendar closes on a confirmed date.
            FormEvent::DateChanged { value, .. } => {
                self.applied.push(format!("selected({value:?})"));
                self.picker.selected = Some(value);
                self.picker.open = false;
            }
            _ => return Reaction::Continue,
        }
        Reaction::Redraw
    }
}

#[cfg(feature = "tui")]
mod tui {
    use super::*;
    use quadraui::tui::testing::TuiDriver;
    use quadraui::NamedKey;

    const COLS: u16 = 40;
    const ROWS: u16 = 14;
    const FIELD: Rect = Rect::new(2.0, 1.0, 16.0, 1.0);
    /// A row the open calendar covers.
    const BASE: Rect = Rect::new(0.0, 4.0, 40.0, 1.0);

    fn driver() -> TuiDriver<PopupApp> {
        let mut driver = TuiDriver::new(PopupApp::new(FIELD, BASE), COLS, ROWS);
        driver.set_double_click_folding(false);
        driver
    }

    fn open(driver: &mut TuiDriver<PopupApp>) {
        let (x, y) = driver.find("03/15/2024").expect("the field shows its date");
        driver.click(x, y);
        assert!(driver.app().picker.open, "{:?}", driver.app().applied);
        assert!(driver.screen_contains("2024-03"), "{}", driver.screen());
    }

    fn row(driver: &TuiDriver<PopupApp>, y: usize) -> String {
        driver
            .screen()
            .lines()
            .nth(y)
            .unwrap_or_default()
            .to_string()
    }

    #[test]
    fn a_closed_picker_paints_only_its_field() {
        let driver = driver();
        assert!(driver.screen_contains("03/15/2024"), "{}", driver.screen());
        assert!(driver.screen_contains("\u{25BE}"), "{}", driver.screen());
        assert!(!driver.screen_contains("2024-03"), "{}", driver.screen());
        assert!(quadraui::popup::painted_popups().is_empty());
        assert!(!row(&driver, BASE.y as usize).contains(' '));
    }

    #[test]
    fn clicking_the_field_opens_the_calendar_above_later_content() {
        let mut driver = driver();
        open(&mut driver);

        let popups = quadraui::popup::painted_popups();
        assert_eq!(popups.len(), 1);
        assert_eq!(popups[0].owner, WidgetId::new("due"));
        // Directly below the field, left-aligned with it.
        assert_eq!(popups[0].rect.x, FIELD.x);
        assert_eq!(popups[0].rect.y, FIELD.y + 1.0);

        // The status bar painted after the picker is covered where the
        // calendar lies, and untouched to either side of it.
        let base = row(&driver, BASE.y as usize);
        let inside: String = base
            .chars()
            .skip(popups[0].rect.x as usize)
            .take(popups[0].rect.width as usize)
            .collect();
        assert!(!inside.contains('#'), "{}", driver.screen());
        assert!(base.starts_with("##"), "{}", driver.screen());
    }

    #[test]
    fn the_header_controls_and_page_keys_navigate_months_in_the_popup() {
        let mut driver = driver();
        open(&mut driver);

        let (x, y) = driver.find(">").expect("the next-month control");
        driver.click(x, y);
        assert_eq!(
            driver.app().picker.visible_month,
            CalendarDate::new(2024, 4, 1)
        );
        assert!(driver.app().picker.open, "navigating keeps the popup open");
        assert!(driver.screen_contains("2024-04"), "{}", driver.screen());

        driver.press_named(NamedKey::PageUp);
        driver.press_named(NamedKey::PageUp);
        assert_eq!(
            driver.app().picker.visible_month,
            CalendarDate::new(2024, 2, 1)
        );
        assert!(driver.screen_contains("2024-02"), "{}", driver.screen());
    }

    #[test]
    fn picking_a_day_commits_it_and_closes_the_popup() {
        let mut driver = driver();
        open(&mut driver);

        let (x, y) = driver.find("22").expect("a day in March");
        driver.click(x, y);
        assert_eq!(
            driver.app().picker.selected,
            Some(CalendarDate::new(2024, 3, 22))
        );
        assert!(!driver.app().picker.open);
        assert!(!driver.screen_contains("2024-03"), "{}", driver.screen());
        assert!(driver.screen_contains("03/22/2024"), "{}", driver.screen());
    }

    #[test]
    fn enter_on_the_open_calendar_commits_the_cursor_day() {
        let mut driver = driver();
        open(&mut driver);
        driver.press_named(NamedKey::Right);
        driver.press_named(NamedKey::Enter);
        assert_eq!(
            driver.app().picker.selected,
            Some(CalendarDate::new(2024, 3, 16))
        );
        assert!(!driver.app().picker.open);
    }

    #[test]
    fn escape_dismisses_the_popup() {
        let mut driver = driver();
        open(&mut driver);
        driver.press_named(NamedKey::Escape);
        assert!(!driver.app().picker.open);
        assert_eq!(driver.app().picker.selected, None);
        assert!(!driver.screen_contains("2024-03"), "{}", driver.screen());
        assert!(quadraui::popup::painted_popups().is_empty());
    }

    #[test]
    fn an_outside_click_dismisses_the_popup() {
        let mut driver = driver();
        open(&mut driver);
        driver.click(38.0, 12.0);
        assert!(!driver.app().picker.open);
        assert_eq!(
            driver.app().applied.last().map(String::as_str),
            Some("open(false)")
        );
        assert!(!driver.screen_contains("2024-03"), "{}", driver.screen());
    }

    #[test]
    fn a_second_click_on_the_field_closes_the_popup() {
        let mut driver = driver();
        open(&mut driver);
        let (x, y) = driver.find("03/15/2024").expect("the field");
        driver.click(x, y);
        assert!(!driver.app().picker.open);
    }

    #[test]
    fn a_focused_closed_picker_opens_from_the_keyboard() {
        let mut driver = driver();
        open(&mut driver);
        driver.press_named(NamedKey::Escape);
        driver.press_named(NamedKey::Down);
        assert!(driver.app().picker.open, "{:?}", driver.app().applied);
    }

    #[test]
    fn the_day_under_the_pointer_is_washed_as_hovered() {
        let mut driver = driver();
        open(&mut driver);
        let (x, y) = driver.find("22").expect("a day in March");
        driver.mouse_move(x, y);
        driver.render();
        let style = driver
            .style_at(x as u16, y as u16)
            .expect("a cell inside the hovered day");
        assert_eq!(
            style.bg,
            quadraui::tui::ratatui_color(quadraui::Theme::default().inactive_selected_bg)
        );
    }
}

#[cfg(feature = "gtk")]
mod gtk {
    use super::*;
    use quadraui::gtk::testing::GtkDriver;
    use quadraui::NamedKey;

    const FIELD: Rect = Rect::new(20.0, 10.0, 160.0, 24.0);
    const BASE: Rect = Rect::new(0.0, 60.0, 400.0, 24.0);

    fn driver() -> GtkDriver<PopupApp> {
        let mut driver = GtkDriver::new(PopupApp::new(FIELD, BASE), 400, 320);
        driver.set_double_click_folding(false);
        driver
    }

    fn open(driver: &mut GtkDriver<PopupApp>) {
        let (x, y) = driver.find("03/15/2024").expect("the field shows its date");
        driver.click(x, y);
        assert!(driver.app().picker.open, "{:?}", driver.app().applied);
        assert!(
            driver.screen_contains("2024-03"),
            "{:?}",
            driver.painted_texts()
        );
    }

    #[test]
    fn the_calendar_opens_below_the_field_and_paints_above_later_content() {
        let mut driver = driver();
        assert!(!driver.screen_contains("2024-03"));
        open(&mut driver);

        let popups = quadraui::popup::painted_popups();
        assert_eq!(popups.len(), 1);
        let rect = popups[0].rect;
        assert_eq!(rect.x, FIELD.x);
        // The field is one text row tall at the top of its rect; the
        // calendar drops directly below that row.
        assert!(
            rect.y > FIELD.y && rect.y <= FIELD.y + FIELD.height,
            "{rect:?}"
        );
        assert!(rect.y < BASE.y && rect.y + rect.height > BASE.y + BASE.height);

        // Inside the popup, over the status bar's row, the pixel is the
        // calendar's backdrop rather than the bar painted after it.
        let (r, g, b) = driver.pixel(
            (rect.x + rect.width - 3.0) as i32,
            (BASE.y + BASE.height / 2.0) as i32,
        );
        let bar = (30u8, 30u8, 30u8);
        assert_ne!((r, g, b), bar, "the status bar shows through the popup");
    }

    #[test]
    fn navigate_months_then_pick_a_day() {
        let mut driver = driver();
        open(&mut driver);

        let (x, y) = driver.find(">").expect("the next-month control");
        driver.click(x, y);
        assert_eq!(
            driver.app().picker.visible_month,
            CalendarDate::new(2024, 4, 1)
        );
        assert!(driver.screen_contains("2024-04"));

        let (x, y) = driver.find("<").expect("the previous-month control");
        driver.click(x, y);
        assert!(driver.screen_contains("2024-03"));

        let (x, y) = driver.find("22").expect("a day in March");
        driver.click(x, y);
        assert_eq!(
            driver.app().picker.selected,
            Some(CalendarDate::new(2024, 3, 22))
        );
        assert!(!driver.app().picker.open);
        assert!(!driver.screen_contains("2024-03"));
        assert!(driver.screen_contains("03/22/2024"));
    }

    #[test]
    fn escape_dismisses_the_popup() {
        let mut driver = driver();
        open(&mut driver);
        driver.press_named(NamedKey::Escape);
        assert!(!driver.app().picker.open);
        assert!(!driver.screen_contains("2024-03"));
    }

    #[test]
    fn an_outside_click_dismisses_the_popup() {
        let mut driver = driver();
        open(&mut driver);
        driver.click(390.0, 310.0);
        assert!(!driver.app().picker.open);
        assert!(!driver.screen_contains("2024-03"));
    }
}
