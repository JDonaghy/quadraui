//! `DatePicker` primitive: a month-grid calendar with optional time field.
//!
//! Distinct from a plain text `Editor`/`TextInput` bound to a date string:
//! `DatePicker` owns the calendar math (which day falls on which weekday,
//! how many days a month has, leap years) and keyboard navigation (arrow
//! keys move the focused day, `PageUp`/`PageDown` change month, `Enter`
//! confirms), so every backend renders the same grid and honours the same
//! keys without re-deriving the calendar itself.
//!
//! # Backend contract
//!
//! **Purely declarative**, like `ListView`/`TreeView`/`DataTable`: this
//! module owns calendar math and the pure keyboard-navigation state
//! machine, but does not mutate itself or touch any backend/surface type.
//! Call [`DatePicker::layout`] to get cell bounds for painting and
//! hit-testing, [`DatePickerLayout::hit_test`] for clicks, and
//! [`DatePicker::navigate`] for keyboard input; apply the resulting
//! [`DatePickerEvent`] to `focused` / `visible_month` / `selected` for the
//! next frame, the same way apps update `ListView::selected_idx`.
//!
//! No `chrono`/`time` dependency: calendar math is the well-known
//! proleptic-Gregorian day-count algorithm (Howard Hinnant's
//! `days_from_civil` / `civil_from_days`), good for any date a `u8` month
//! (1-12) and `u8` day (1-31) pairing with an `i32` year can express.

use crate::event::{Point, Rect};
use crate::types::WidgetId;
use serde::{Deserialize, Serialize};

/// Day of the week. `index()` maps `Sunday` to `0`, matching the
/// civil-calendar weekday formula used throughout this module.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Weekday {
    Sunday,
    Monday,
    Tuesday,
    Wednesday,
    Thursday,
    Friday,
    Saturday,
}

impl Weekday {
    /// `0` = Sunday .. `6` = Saturday.
    pub fn index(self) -> u8 {
        match self {
            Weekday::Sunday => 0,
            Weekday::Monday => 1,
            Weekday::Tuesday => 2,
            Weekday::Wednesday => 3,
            Weekday::Thursday => 4,
            Weekday::Friday => 5,
            Weekday::Saturday => 6,
        }
    }

    /// Inverse of [`Weekday::index`]. `n` is taken modulo 7.
    pub fn from_index(n: u8) -> Self {
        match n % 7 {
            0 => Weekday::Sunday,
            1 => Weekday::Monday,
            2 => Weekday::Tuesday,
            3 => Weekday::Wednesday,
            4 => Weekday::Thursday,
            5 => Weekday::Friday,
            _ => Weekday::Saturday,
        }
    }
}

/// How a [`CalendarDate`] renders as text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DateFormat {
    /// `YYYY-MM-DD`.
    Iso8601,
    /// `MM/DD/YYYY`.
    UsSlash,
    /// `DD/MM/YYYY`.
    EuSlash,
}

/// Locale rules a `DatePicker` renders and navigates by: which weekday
/// starts a week, and how a date formats as text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DateLocale {
    /// The weekday a week starts on (e.g. `Sunday` in the US, `Monday` in
    /// most of Europe); drives [`DatePicker::month_grid`]'s column order.
    pub first_weekday: Weekday,
    /// How [`CalendarDate::format`] renders a date.
    pub format: DateFormat,
}

impl Default for DateLocale {
    fn default() -> Self {
        DateLocale {
            first_weekday: Weekday::Sunday,
            format: DateFormat::Iso8601,
        }
    }
}

/// A calendar date in the proleptic Gregorian calendar — no timezone, no
/// clock. `month` is `1..=12`, `day` is `1..=31` (bounded by
/// [`CalendarDate::days_in_month`] for the given `year`/`month`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct CalendarDate {
    /// Proleptic Gregorian year; may be negative (there is no year 0).
    pub year: i32,
    /// `1..=12`.
    pub month: u8,
    /// `1..=31`, bounded by [`CalendarDate::days_in_month`] for `year`/`month`.
    pub day: u8,
}

impl CalendarDate {
    pub fn new(year: i32, month: u8, day: u8) -> Self {
        CalendarDate { year, month, day }
    }

    /// `true` if `year` is a Gregorian leap year.
    pub fn is_leap_year(year: i32) -> bool {
        year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
    }

    /// Number of days in `month` (`1..=12`) of `year`. An out-of-range
    /// `month` (`0` or `>12`) is not a documented input; it falls back to
    /// `30` rather than panicking, so callers passing already-validated
    /// months never need to handle an error case.
    pub fn days_in_month(year: i32, month: u8) -> u8 {
        match month {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 => {
                if Self::is_leap_year(year) {
                    29
                } else {
                    28
                }
            }
            _ => 30,
        }
    }

    /// Days since the 1970-01-01 civil epoch (negative for earlier dates).
    /// Howard Hinnant's `days_from_civil` algorithm.
    pub fn days_from_civil(self) -> i64 {
        let (y, m, d) = (self.year as i64, self.month as i64, self.day as i64);
        let y = if m <= 2 { y - 1 } else { y };
        let era = if y >= 0 { y } else { y - 399 } / 400;
        let yoe = y - era * 400; // [0, 399]
        let mp = if m > 2 { m - 3 } else { m + 9 }; // [0, 11]
        let doy = (153 * mp + 2) / 5 + d - 1; // [0, 365]
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
        era * 146097 + doe - 719468
    }

    /// Inverse of [`CalendarDate::days_from_civil`].
    pub fn from_days_since_epoch(z: i64) -> Self {
        let z = z + 719468;
        let era = if z >= 0 { z } else { z - 146096 } / 146097;
        let doe = z - era * 146097; // [0, 146096]
        let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // [0, 399]
        let y = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
        let mp = (5 * doy + 2) / 153; // [0, 11]
        let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
        let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
        let y = if m <= 2 { y + 1 } else { y };
        CalendarDate {
            year: y as i32,
            month: m as u8,
            day: d as u8,
        }
    }

    /// Day of the week this date falls on.
    pub fn weekday(self) -> Weekday {
        let z = self.days_from_civil();
        Weekday::from_index(((z + 4).rem_euclid(7)) as u8)
    }

    /// Shifts by `delta` days (negative moves backward).
    pub fn add_days(self, delta: i64) -> Self {
        Self::from_days_since_epoch(self.days_from_civil() + delta)
    }

    /// Shifts by `delta` months (negative moves backward), clamping `day`
    /// to the target month's length (e.g. Jan 31 + 1 month -> Feb 28/29).
    pub fn add_months(self, delta: i32) -> Self {
        let total = self.year as i64 * 12 + (self.month as i64 - 1) + delta as i64;
        let year = total.div_euclid(12) as i32;
        let month = (total.rem_euclid(12) + 1) as u8;
        let day = self.day.min(Self::days_in_month(year, month));
        CalendarDate { year, month, day }
    }

    /// First day of this date's month.
    pub fn first_of_month(self) -> Self {
        CalendarDate {
            year: self.year,
            month: self.month,
            day: 1,
        }
    }

    /// Formats this date per `format`, zero-padded.
    ///
    /// `Iso8601`'s zero-padding applies to the magnitude only, so a
    /// negative `year` still reads as a sign followed by 4+ digits
    /// (`-0005-01-01`), not a sign eating into the pad width
    /// (`-005-01-01`).
    pub fn format(self, format: DateFormat) -> String {
        match format {
            DateFormat::Iso8601 => {
                let sign = if self.year < 0 { "-" } else { "" };
                format!(
                    "{sign}{:04}-{:02}-{:02}",
                    self.year.unsigned_abs(),
                    self.month,
                    self.day
                )
            }
            DateFormat::UsSlash => format!("{:02}/{:02}/{:04}", self.month, self.day, self.year),
            DateFormat::EuSlash => format!("{:02}/{:02}/{:04}", self.day, self.month, self.year),
        }
    }
}

impl Default for CalendarDate {
    /// The 1970-01-01 civil epoch.
    fn default() -> Self {
        CalendarDate::new(1970, 1, 1)
    }
}

/// Optional time-of-day field shown alongside the date grid.
/// `hour` is `0..=23`, `minute` is `0..=59`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeOfDay {
    /// `0..=23`.
    pub hour: u8,
    /// `0..=59`.
    pub minute: u8,
}

impl TimeOfDay {
    pub fn new(hour: u8, minute: u8) -> Self {
        TimeOfDay { hour, minute }
    }

    /// `HH:MM`, 24-hour, zero-padded.
    pub fn format(self) -> String {
        format!("{:02}:{:02}", self.hour, self.minute)
    }
}

impl Default for TimeOfDay {
    fn default() -> Self {
        TimeOfDay::new(0, 0)
    }
}

/// Declarative description of a `DatePicker` widget.
///
/// # Examples
///
/// ```
/// use quadraui::{CalendarDate, DatePicker, DateLocale, WidgetId};
///
/// let picker = DatePicker {
///     id: WidgetId::new("due-date"),
///     visible_month: CalendarDate::new(2024, 3, 1),
///     focused: CalendarDate::new(2024, 3, 15),
///     has_focus: true,
///     ..Default::default()
/// };
///
/// assert_eq!(picker.month_grid().len(), 42);
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct DatePicker {
    pub id: WidgetId,
    /// The month this picker displays. `day` is ignored for grid purposes
    /// — [`DatePicker::month_grid`] always starts from day 1.
    pub visible_month: CalendarDate,
    /// The confirmed date, if any (set by an `Enter` / click
    /// [`DatePickerEvent::DateSelected`]).
    #[serde(default)]
    pub selected: Option<CalendarDate>,
    /// The keyboard-navigable cursor within the grid. Independent of
    /// `selected` until confirmed, so arrow keys can preview a date
    /// before committing.
    pub focused: CalendarDate,
    pub locale: DateLocale,
    /// Optional time-of-day field rendered alongside the date grid.
    #[serde(default)]
    pub time: Option<TimeOfDay>,
    #[serde(default)]
    pub has_focus: bool,
}

impl Default for DatePicker {
    fn default() -> Self {
        DatePicker {
            id: WidgetId::new(""),
            visible_month: CalendarDate::default(),
            selected: None,
            focused: CalendarDate::default(),
            locale: DateLocale::default(),
            time: None,
            has_focus: false,
        }
    }
}

impl DatePicker {
    /// A new picker showing `focused`'s month, with nothing selected and
    /// no time field.
    pub fn new(id: WidgetId, focused: CalendarDate) -> Self {
        DatePicker {
            id,
            visible_month: focused.first_of_month(),
            focused,
            ..Default::default()
        }
    }

    /// A 42-cell (6 week rows x 7 days), always-complete month grid for
    /// `visible_month`, in visual row-major order starting from
    /// `locale.first_weekday`. Cells outside `visible_month` (the leading
    /// days of the prior month and the trailing days of the next) are
    /// included so every row is a full week — callers distinguish them via
    /// [`DatePicker::is_in_visible_month`].
    pub fn month_grid(&self) -> Vec<CalendarDate> {
        let first = self.visible_month.first_of_month();
        let leading = (first.weekday().index() + 7 - self.locale.first_weekday.index()) % 7;
        let grid_start = first.add_days(-(leading as i64));
        (0..42).map(|i| grid_start.add_days(i)).collect()
    }

    /// `true` if `date` falls within `visible_month` (same year and month).
    pub fn is_in_visible_month(&self, date: CalendarDate) -> bool {
        date.year == self.visible_month.year && date.month == self.visible_month.month
    }

    /// Pure keyboard-navigation state machine. Returns the event the app
    /// should apply to `focused` / `visible_month` / `selected` for the
    /// next frame; returns `None` for unrecognised keys (the app may
    /// still interpret them, e.g. `Escape` to close).
    ///
    /// Recognised keys: `Left`/`Right`/`Up`/`Down` (move focus by a day /
    /// a week), `PageUp`/`PageDown` (move focus by a month, clamping the
    /// day), `Enter` (confirm the focused date).
    pub fn navigate(&self, key: &str) -> Option<DatePickerEvent> {
        match key {
            "Left" => Some(DatePickerEvent::FocusMoved(self.focused.add_days(-1))),
            "Right" => Some(DatePickerEvent::FocusMoved(self.focused.add_days(1))),
            "Up" => Some(DatePickerEvent::FocusMoved(self.focused.add_days(-7))),
            "Down" => Some(DatePickerEvent::FocusMoved(self.focused.add_days(7))),
            "PageUp" => Some(DatePickerEvent::FocusMoved(self.focused.add_months(-1))),
            "PageDown" => Some(DatePickerEvent::FocusMoved(self.focused.add_months(1))),
            "Enter" => Some(DatePickerEvent::DateSelected(self.focused)),
            _ => None,
        }
    }

    /// Computes the full rendering + hit-test layout for this picker.
    ///
    /// `cell_width` / `cell_height` size each day cell; `header_height`
    /// reserves a top row for the month/year label plus prev/next
    /// controls; `time_height` reserves a row below the grid for the time
    /// field (`0.0` when `self.time.is_none()`, matching other
    /// primitives' "pass `0.0` to omit" convention).
    pub fn layout(
        &self,
        origin_x: f32,
        origin_y: f32,
        cell_width: f32,
        cell_height: f32,
        header_height: f32,
        time_height: f32,
    ) -> DatePickerLayout {
        let grid = self.month_grid();
        let grid_top = origin_y + header_height;

        let header_bounds = if header_height > 0.0 {
            Some(Rect::new(origin_x, origin_y, cell_width * 7.0, header_height))
        } else {
            None
        };
        let (prev_bounds, next_bounds) = if header_height > 0.0 {
            (
                Some(Rect::new(origin_x, origin_y, cell_width, header_height)),
                Some(Rect::new(
                    origin_x + cell_width * 6.0,
                    origin_y,
                    cell_width,
                    header_height,
                )),
            )
        } else {
            (None, None)
        };

        let mut day_cells: Vec<DayCell> = Vec::with_capacity(grid.len());
        for (i, date) in grid.into_iter().enumerate() {
            let row = (i / 7) as f32;
            let col = (i % 7) as f32;
            let bounds = Rect::new(
                origin_x + col * cell_width,
                grid_top + row * cell_height,
                cell_width,
                cell_height,
            );
            day_cells.push(DayCell {
                date,
                in_visible_month: self.is_in_visible_month(date),
                is_selected: self.selected == Some(date),
                is_focused: self.focused == date,
                bounds,
            });
        }

        let grid_bottom = grid_top + cell_height * 6.0;
        let time_bounds = if time_height > 0.0 {
            Some(Rect::new(origin_x, grid_bottom, cell_width * 7.0, time_height))
        } else {
            None
        };

        DatePickerLayout {
            header_bounds,
            prev_bounds,
            next_bounds,
            day_cells,
            time_bounds,
        }
    }
}

/// One resolved day cell in a [`DatePickerLayout`].
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct DayCell {
    pub date: CalendarDate,
    /// `false` for the leading/trailing days of adjacent months that fill
    /// out a complete 6x7 grid.
    pub in_visible_month: bool,
    pub is_selected: bool,
    pub is_focused: bool,
    pub bounds: Rect,
}

impl Default for DayCell {
    fn default() -> Self {
        DayCell {
            date: CalendarDate::default(),
            in_visible_month: false,
            is_selected: false,
            is_focused: false,
            bounds: Rect::default(),
        }
    }
}

/// Fully-resolved date-picker layout.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct DatePickerLayout {
    /// Present iff the caller reserved a non-zero `header_height`.
    pub header_bounds: Option<Rect>,
    /// Present iff the caller reserved a non-zero `header_height` (the
    /// prev/next controls live inside the header row, so there is nowhere
    /// to put them when it is absent).
    pub prev_bounds: Option<Rect>,
    /// See [`DatePickerLayout::prev_bounds`].
    pub next_bounds: Option<Rect>,
    /// Always 42 cells (6 complete weeks), row-major, top-left first.
    pub day_cells: Vec<DayCell>,
    /// Present iff the caller reserved a non-zero `time_height`.
    pub time_bounds: Option<Rect>,
}

impl Default for DatePickerLayout {
    fn default() -> Self {
        DatePickerLayout {
            header_bounds: None,
            prev_bounds: None,
            next_bounds: None,
            day_cells: Vec::new(),
            time_bounds: None,
        }
    }
}

/// Classification of a hit-test result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatePickerHit {
    PrevMonth,
    NextMonth,
    Day(CalendarDate),
    Time,
    /// Click landed outside every region above.
    Empty,
}

impl DatePickerLayout {
    /// Test which element (prev/next control, a day cell, the time field,
    /// or nothing) contains `(x, y)`.
    pub fn hit_test(&self, x: f32, y: f32) -> DatePickerHit {
        let p = Point::new(x, y);
        if self.prev_bounds.is_some_and(|r| r.contains(p)) {
            return DatePickerHit::PrevMonth;
        }
        if self.next_bounds.is_some_and(|r| r.contains(p)) {
            return DatePickerHit::NextMonth;
        }
        for cell in &self.day_cells {
            if cell.bounds.contains(p) {
                return DatePickerHit::Day(cell.date);
            }
        }
        if self.time_bounds.is_some_and(|r| r.contains(p)) {
            return DatePickerHit::Time;
        }
        DatePickerHit::Empty
    }
}

/// Events a `DatePicker` emits back to the app. The app applies these to
/// `focused` / `visible_month` / `selected` for the next frame, the same
/// way it updates `ListView::selected_idx` from `ListViewEvent`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DatePickerEvent {
    /// Keyboard navigation (or a click on a day in an adjacent month)
    /// moved the focus cursor. If the target date falls outside
    /// `visible_month`, the app should also update `visible_month`.
    FocusMoved(CalendarDate),
    /// `Enter` or a click on a day confirmed that date as `selected`.
    DateSelected(CalendarDate),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn days_from_civil_round_trips_through_from_days_since_epoch() {
        let dates = [
            CalendarDate::new(1970, 1, 1),
            CalendarDate::new(2000, 2, 29), // leap day
            CalendarDate::new(1900, 2, 28), // not a leap year
            CalendarDate::new(2024, 12, 31),
            CalendarDate::new(1969, 12, 31), // just before epoch
            CalendarDate::new(1, 1, 1),
        ];
        for d in dates {
            let z = d.days_from_civil();
            assert_eq!(CalendarDate::from_days_since_epoch(z), d, "round-trip for {d:?}");
        }
    }

    #[test]
    fn epoch_weekday_is_thursday() {
        assert_eq!(CalendarDate::new(1970, 1, 1).weekday(), Weekday::Thursday);
    }

    #[test]
    fn known_weekday_y2k() {
        // 2000-01-01 was a Saturday.
        assert_eq!(CalendarDate::new(2000, 1, 1).weekday(), Weekday::Saturday);
    }

    #[test]
    fn days_in_month_handles_leap_years() {
        assert_eq!(CalendarDate::days_in_month(2000, 2), 29);
        assert_eq!(CalendarDate::days_in_month(1900, 2), 28);
        assert_eq!(CalendarDate::days_in_month(2024, 2), 29);
        assert_eq!(CalendarDate::days_in_month(2023, 2), 28);
        assert_eq!(CalendarDate::days_in_month(2023, 4), 30);
    }

    #[test]
    fn add_months_clamps_day_to_target_month_length() {
        let jan31 = CalendarDate::new(2023, 1, 31);
        assert_eq!(jan31.add_months(1), CalendarDate::new(2023, 2, 28));
        let jan31_leap = CalendarDate::new(2024, 1, 31);
        assert_eq!(jan31_leap.add_months(1), CalendarDate::new(2024, 2, 29));
    }

    #[test]
    fn add_months_wraps_year_boundary() {
        let nov = CalendarDate::new(2023, 11, 15);
        assert_eq!(nov.add_months(3), CalendarDate::new(2024, 2, 15));
        assert_eq!(nov.add_months(-12), CalendarDate::new(2022, 11, 15));
    }

    #[test]
    fn month_grid_is_always_42_complete_weeks_starting_on_locale_weekday() {
        let picker = DatePicker {
            id: WidgetId::new("dp"),
            visible_month: CalendarDate::new(2024, 3, 1),
            selected: None,
            focused: CalendarDate::new(2024, 3, 1),
            locale: DateLocale {
                first_weekday: Weekday::Monday,
                format: DateFormat::Iso8601,
            },
            time: None,
            has_focus: true,
        };
        let grid = picker.month_grid();
        assert_eq!(grid.len(), 42);
        assert_eq!(grid[0].weekday(), Weekday::Monday);
        // March 2024 has 31 days starting on a Friday; with Monday as the
        // first weekday the grid's first cell is Feb 26, 2024.
        assert_eq!(grid[0], CalendarDate::new(2024, 2, 26));
        assert!(grid.contains(&CalendarDate::new(2024, 3, 31)));
    }

    #[test]
    fn month_grid_respects_sunday_first_locale() {
        let picker = DatePicker {
            id: WidgetId::new("dp"),
            visible_month: CalendarDate::new(2024, 3, 1),
            selected: None,
            focused: CalendarDate::new(2024, 3, 1),
            locale: DateLocale::default(),
            time: None,
            has_focus: true,
        };
        let grid = picker.month_grid();
        assert_eq!(grid[0].weekday(), Weekday::Sunday);
    }

    #[test]
    fn navigate_arrow_keys_move_focus_by_day_and_week() {
        let picker = DatePicker {
            id: WidgetId::new("dp"),
            visible_month: CalendarDate::new(2024, 3, 1),
            selected: None,
            focused: CalendarDate::new(2024, 3, 15),
            locale: DateLocale::default(),
            time: None,
            has_focus: true,
        };
        assert_eq!(
            picker.navigate("Right"),
            Some(DatePickerEvent::FocusMoved(CalendarDate::new(2024, 3, 16)))
        );
        assert_eq!(
            picker.navigate("Left"),
            Some(DatePickerEvent::FocusMoved(CalendarDate::new(2024, 3, 14)))
        );
        assert_eq!(
            picker.navigate("Down"),
            Some(DatePickerEvent::FocusMoved(CalendarDate::new(2024, 3, 22)))
        );
        assert_eq!(
            picker.navigate("Up"),
            Some(DatePickerEvent::FocusMoved(CalendarDate::new(2024, 3, 8)))
        );
        assert_eq!(picker.navigate("Tab"), None);
    }

    #[test]
    fn navigate_page_keys_move_focus_by_month() {
        let picker = DatePicker {
            id: WidgetId::new("dp"),
            visible_month: CalendarDate::new(2024, 1, 1),
            selected: None,
            focused: CalendarDate::new(2024, 1, 31),
            locale: DateLocale::default(),
            time: None,
            has_focus: true,
        };
        assert_eq!(
            picker.navigate("PageDown"),
            Some(DatePickerEvent::FocusMoved(CalendarDate::new(2024, 2, 29)))
        );
        assert_eq!(
            picker.navigate("PageUp"),
            Some(DatePickerEvent::FocusMoved(CalendarDate::new(2023, 12, 31)))
        );
    }

    #[test]
    fn navigate_enter_confirms_focused_date() {
        let picker = DatePicker {
            id: WidgetId::new("dp"),
            visible_month: CalendarDate::new(2024, 3, 1),
            selected: None,
            focused: CalendarDate::new(2024, 3, 15),
            locale: DateLocale::default(),
            time: None,
            has_focus: true,
        };
        assert_eq!(
            picker.navigate("Enter"),
            Some(DatePickerEvent::DateSelected(CalendarDate::new(2024, 3, 15)))
        );
    }

    #[test]
    fn format_iso8601_us_and_eu() {
        let d = CalendarDate::new(2024, 3, 5);
        assert_eq!(d.format(DateFormat::Iso8601), "2024-03-05");
        assert_eq!(d.format(DateFormat::UsSlash), "03/05/2024");
        assert_eq!(d.format(DateFormat::EuSlash), "05/03/2024");
    }

    #[test]
    fn time_of_day_formats_zero_padded() {
        assert_eq!(TimeOfDay::new(9, 5).format(), "09:05");
        assert_eq!(TimeOfDay::new(23, 59).format(), "23:59");
    }

    #[test]
    fn layout_hit_test_resolves_prev_next_day_and_time() {
        let picker = DatePicker {
            id: WidgetId::new("dp"),
            visible_month: CalendarDate::new(2024, 3, 1),
            selected: None,
            focused: CalendarDate::new(2024, 3, 15),
            locale: DateLocale::default(),
            time: Some(TimeOfDay::new(10, 30)),
            has_focus: true,
        };
        let layout = picker.layout(0.0, 0.0, 10.0, 2.0, 1.0, 2.0);

        assert_eq!(layout.hit_test(0.5, 0.5), DatePickerHit::PrevMonth);
        assert_eq!(layout.hit_test(65.0, 0.5), DatePickerHit::NextMonth);

        // First grid cell (top-left day) sits right below the header.
        let first_cell = layout.day_cells[0];
        let cx = first_cell.bounds.x + 1.0;
        let cy = first_cell.bounds.y + 1.0;
        assert_eq!(layout.hit_test(cx, cy), DatePickerHit::Day(first_cell.date));

        let time_bounds = layout.time_bounds.expect("time field reserved");
        assert_eq!(
            layout.hit_test(time_bounds.x + 1.0, time_bounds.y + 1.0),
            DatePickerHit::Time
        );

        assert_eq!(layout.hit_test(-5.0, -5.0), DatePickerHit::Empty);
    }

    #[test]
    fn layout_omits_header_and_time_bounds_when_heights_are_zero() {
        let picker = DatePicker {
            id: WidgetId::new("dp"),
            visible_month: CalendarDate::new(2024, 3, 1),
            selected: None,
            focused: CalendarDate::new(2024, 3, 1),
            locale: DateLocale::default(),
            time: None,
            has_focus: true,
        };
        let layout = picker.layout(0.0, 0.0, 10.0, 2.0, 0.0, 0.0);
        assert_eq!(layout.header_bounds, None);
        assert_eq!(layout.time_bounds, None);
        assert_eq!(layout.prev_bounds, None);
        assert_eq!(layout.next_bounds, None);
    }

    #[test]
    fn hit_test_resolves_top_row_edge_cells_to_day_when_header_height_is_zero() {
        // Regression test: with no header row reserved, there is no phantom
        // prev/next band at the grid's top edge, so clicks on the first
        // row's leftmost and rightmost cells must resolve to `Day(..)`, not
        // be misrouted to `PrevMonth`/`NextMonth`.
        let picker = DatePicker::new(WidgetId::new("dp"), CalendarDate::new(2024, 3, 1));
        let layout = picker.layout(0.0, 0.0, 10.0, 2.0, 0.0, 0.0);

        let top_left = layout.day_cells[0];
        assert_eq!(
            layout.hit_test(top_left.bounds.x + 1.0, top_left.bounds.y + 1.0),
            DatePickerHit::Day(top_left.date)
        );

        let top_right = layout.day_cells[6];
        assert_eq!(
            layout.hit_test(top_right.bounds.x + 1.0, top_right.bounds.y + 1.0),
            DatePickerHit::Day(top_right.date)
        );
    }
}
