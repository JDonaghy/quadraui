//! `DatePicker` primitive: a month-grid calendar with optional time
//! field, its keyboard model, and both rasterisers.
//!
//! The whole widget contract lives here: [`Primitive`] resolves the
//! layout and paints it ([`native_surface_paint`] on a pixel surface,
//! [`cell_paint`] on a cell grid), and [`Interactive`] answers pointer,
//! keyboard and assistive-technology input. A host reaches all of it
//! type-erased through [`crate::frame::Surface::Widget`].
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
//! # Dropdown presentation
//!
//! With [`DatePicker::dropdown`] set, the picker is a one-row date field
//! and the calendar is a widget-owned popup
//! ([`Interactive::popups`]) shown while the app's [`DatePicker::open`]
//! is set. The runtime paints it above the rest of the frame, hit-tests
//! it first and dismisses it on an outside press or Escape; every
//! outcome still arrives as a [`DatePickerEvent`] the app applies to the
//! descriptor, with [`DatePickerEvent::OpenChanged`] for the popup
//! itself.
//!
//! No `chrono`/`time` dependency: calendar math is the well-known
//! proleptic-Gregorian day-count algorithm (Howard Hinnant's
//! `days_from_civil` / `civil_from_days`), good for any date a `u8` month
//! (1-12) and `u8` day (1-31) pairing with an `i32` year can express.

use crate::a11y::A11yAction;
use crate::event::{Key, Point, Rect};
use crate::interactive::{
    EventCx, FocusSpec, Interactive, PointerKind, RoutedEvent, SubTarget, WidgetAction,
};
use crate::popup::{Popup, PopupKey};
use crate::primitive::{LayoutCtx, Primitive, Size, Units};
use crate::types::WidgetId;
use crate::Handled;
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
/// use quadraui::{CalendarDate, DatePicker, WidgetId};
///
/// let mut picker = DatePicker::new(WidgetId::new("due-date"), CalendarDate::new(2024, 3, 15));
/// picker.has_focus = true;
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
    /// Present the picker as a one-row date field whose calendar opens
    /// in a popup below it, instead of as the calendar itself.
    #[serde(default)]
    pub dropdown: bool,
    /// Whether a [`Self::dropdown`] picker's calendar popup is showing.
    /// The app owns it: [`DatePickerEvent::OpenChanged`] asks for a
    /// change, and [`DatePickerEvent::DateSelected`] closes it. Ignored
    /// when `dropdown` is `false`.
    #[serde(default)]
    pub open: bool,
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
            dropdown: false,
            open: false,
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
            Some(Rect::new(
                origin_x,
                origin_y,
                cell_width * 7.0,
                header_height,
            ))
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
            Some(Rect::new(
                origin_x,
                grid_bottom,
                cell_width * 7.0,
                time_height,
            ))
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
#[derive(Debug, Clone, Copy, PartialEq, Default)]
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

/// Fully-resolved date-picker layout.
#[derive(Debug, Clone, PartialEq, Default)]
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

    /// The smallest [`Rect`] enclosing every populated region (header,
    /// day grid, time field). Backends pass this to
    /// [`Backend::register_zone`](crate::Backend::register_zone) under
    /// the picker's single `WidgetId`, mirroring how `ComboBox`
    /// registers its one `field_bounds` zone — `DatePicker` has no
    /// per-cell `WidgetId`s, so hit-testing within the zone goes back
    /// through [`DatePickerLayout::hit_test`] rather than through
    /// multiple registered zones.
    pub fn bounds(&self) -> Rect {
        let mut min_x = f32::INFINITY;
        let mut min_y = f32::INFINITY;
        let mut max_x = f32::NEG_INFINITY;
        let mut max_y = f32::NEG_INFINITY;
        let mut grow = |r: Rect| {
            min_x = min_x.min(r.x);
            min_y = min_y.min(r.y);
            max_x = max_x.max(r.x + r.width);
            max_y = max_y.max(r.y + r.height);
        };
        if let Some(r) = self.header_bounds {
            grow(r);
        }
        for cell in &self.day_cells {
            grow(cell.bounds);
        }
        if let Some(r) = self.time_bounds {
            grow(r);
        }
        if !min_x.is_finite() {
            return Rect::default();
        }
        Rect::new(min_x, min_y, max_x - min_x, max_y - min_y)
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
    /// `Enter` or a click on a day confirmed that date as `selected`. A
    /// [`DatePicker::dropdown`] picker's calendar closes on it too: the
    /// app also clears `open`.
    DateSelected(CalendarDate),
    /// A [`DatePicker::dropdown`] picker asked for its calendar popup to
    /// open (`true`) or close (`false`); the app writes it onto
    /// [`DatePicker::open`].
    OpenChanged(bool),
}

impl WidgetAction for DatePickerEvent {
    /// Every outcome rides [`crate::UiEvent::Form`] under the picker's
    /// own id, on the [`crate::FormEvent`] date channels that name them:
    /// [`crate::FormEvent::DateChanged`] for a confirmed date,
    /// [`crate::FormEvent::DateFocusMoved`] for the arrow-key scrubbing
    /// that precedes it, and [`crate::FormEvent::DateOpenChanged`] for a
    /// dropdown picker's calendar popup. A standalone `DatePicker` therefore
    /// reaches an app through the same event vocabulary an embedded one
    /// does, with the same "scrubbing fires every keystroke, confirming
    /// fires once" split, and each channel's apply contract spells out
    /// which descriptor fields to write.
    fn into_ui_event(self, id: WidgetId) -> crate::UiEvent {
        let event = match self {
            DatePickerEvent::FocusMoved(value) => crate::FormEvent::DateFocusMoved { id, value },
            DatePickerEvent::DateSelected(value) => crate::FormEvent::DateChanged { id, value },
            DatePickerEvent::OpenChanged(open) => crate::FormEvent::DateOpenChanged { id, open },
        };
        crate::UiEvent::Form(id, event)
    }
}

/// Columns one day cell is wide on a cell grid: two digits framed by the
/// focus ring's own two columns, which [`cell_paint::paint`] insets the
/// digits inside so the ring never overwrites them.
const CELL_COLUMNS: f32 = 4.0;

/// [`DatePicker`]'s fully-resolved widget-contract layout: the absolute
/// `area` the calendar was laid out in, the cell metrics it was resolved
/// against, and the calendar-local [`DatePickerLayout`] itself.
///
/// `#[non_exhaustive]`: produced by [`Primitive::layout`], never built by
/// a consumer via struct literal — the same convention
/// [`DatePickerLayout`] itself follows.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct DatePickerPrimitiveLayout {
    /// The rect [`Primitive::layout`] was called with: the month header
    /// on top, six week rows under it, and the time field (when
    /// [`DatePicker::time`] is set) under those.
    pub area: Rect,
    /// One day cell's resolved size.
    pub cell_size: Size,
    /// The resolved calendar layout. Its rects are in the same absolute
    /// space as [`Self::area`]. Empty for a [`DatePicker::dropdown`]
    /// picker, whose calendar lives in its popup.
    pub layout: DatePickerLayout,
    /// A [`DatePicker::dropdown`] picker's date field, in the same
    /// absolute space as [`Self::area`]; `None` for an inline calendar.
    pub field: Option<Rect>,
    /// The size a [`DatePicker::dropdown`] picker's calendar popup asks
    /// for: the inline calendar's own measured size.
    pub popup_size: Size,
}

impl DatePickerPrimitiveLayout {
    /// Hit-test at a position relative to [`Self::area`]'s own origin —
    /// the space [`RoutedEvent::Pointer`]'s `local` field uses.
    pub fn hit_test_local(&self, x: f32, y: f32) -> DatePickerHit {
        self.layout.hit_test(self.area.x + x, self.area.y + y)
    }
}

impl Primitive for DatePicker {
    type Layout = DatePickerPrimitiveLayout;
    type Event = DatePickerEvent;

    fn id(&self) -> Option<&WidgetId> {
        Some(&self.id)
    }

    /// Seven day columns wide, and a month header plus six week rows
    /// tall, plus the time field's own row when [`DatePicker::time`] is
    /// set — the grid is always six complete weeks (see
    /// [`DatePicker::month_grid`]), so this never depends on which month
    /// is on screen. A [`DatePicker::dropdown`] picker measures its
    /// one-row field instead.
    fn measure(&self, ctx: &LayoutCtx, _avail: Size) -> crate::NaturalSize {
        if self.dropdown {
            let width = match ctx.units() {
                Units::Cells => {
                    crate::text_util::display_width(&self.field_text()) as f32
                        + FIELD_CHROME_COLUMNS
                }
                _ => {
                    ctx.text_width(&self.field_text(), crate::FontRole::Chrome)
                        + FIELD_CHROME_COLUMNS * ctx.char_width
                }
            };
            return crate::NaturalSize::new(Some(width), Some(ctx.line_height));
        }
        let cell_width = match ctx.units() {
            Units::Cells => CELL_COLUMNS,
            _ => ctx.text_width("00", crate::FontRole::Chrome) + 2.0 * ctx.char_width,
        };
        let rows = 1.0 + 6.0 + if self.time.is_some() { 1.0 } else { 0.0 };
        crate::NaturalSize::new(Some(cell_width * 7.0), Some(rows * ctx.line_height))
    }

    /// Divides `rect` into the header row, the 7×6 day grid and the
    /// optional time row, then resolves the cells through
    /// [`DatePicker::layout`] — so paint, hit-testing and the legacy
    /// `Backend::draw_date_picker` path can never disagree about which
    /// day sits where.
    ///
    /// On a cell grid the day cells are floored to whole columns and
    /// rows (a terminal cannot paint a 3.5-column cell), with a
    /// one-cell floor so a box too small for the whole calendar still
    /// paints a readable, clipped one rather than nothing.
    ///
    /// A [`DatePicker::dropdown`] picker lays out only its field (the
    /// top row of `rect`) and records the calendar's measured size for
    /// its popup.
    fn layout(&self, ctx: &LayoutCtx, rect: Rect) -> DatePickerPrimitiveLayout {
        let row_height = if ctx.line_height > 0.0 {
            ctx.line_height
        } else {
            1.0
        };
        if self.dropdown {
            let calendar = self
                .calendar()
                .measure(ctx, Size::new(rect.width, rect.height));
            return DatePickerPrimitiveLayout {
                area: rect,
                cell_size: Size::new(0.0, 0.0),
                layout: DatePickerLayout::default(),
                field: Some(Rect::new(
                    rect.x,
                    rect.y,
                    rect.width,
                    row_height.min(rect.height),
                )),
                popup_size: Size::new(
                    calendar.width.unwrap_or(rect.width),
                    calendar.height.unwrap_or(row_height * 7.0),
                ),
            };
        }
        let cells = matches!(ctx.units(), Units::Cells);
        let header_height = row_height.min(rect.height);
        let time_height = if self.time.is_some() {
            row_height.min((rect.height - header_height).max(0.0))
        } else {
            0.0
        };
        let grid_height = (rect.height - header_height - time_height).max(0.0);
        let quantise = |raw: f32| -> f32 {
            if cells {
                raw.floor().max(1.0)
            } else {
                raw.max(1.0)
            }
        };
        let cell_width = quantise(rect.width / 7.0);
        let cell_height = quantise(grid_height / 6.0);
        DatePickerPrimitiveLayout {
            area: rect,
            cell_size: Size::new(cell_width, cell_height),
            layout: self.layout(
                rect.x,
                rect.y,
                cell_width,
                cell_height,
                header_height,
                time_height,
            ),
            field: None,
            popup_size: Size::new(0.0, 0.0),
        }
    }

    /// The shared pixel painter — the same
    /// [`native_surface_paint::paint`] every pixel backend's
    /// `draw_date_picker` wrapper calls.
    fn paint(
        &self,
        layout: &DatePickerPrimitiveLayout,
        surface: &mut dyn crate::paint_surface::PaintSurface,
        env: &crate::PaintEnv,
    ) {
        let theme = env.theme();
        #[cfg(any(
            feature = "gtk",
            feature = "win",
            all(feature = "macos", target_os = "macos")
        ))]
        {
            match layout.field {
                Some(field) => native_surface_paint::paint_field(self, field, surface, theme),
                None => native_surface_paint::paint(
                    self,
                    &layout.layout,
                    surface,
                    theme,
                    self.hovered_day(env),
                ),
            }
        }
        #[cfg(not(any(
            feature = "gtk",
            feature = "win",
            all(feature = "macos", target_os = "macos")
        )))]
        {
            let _ = (layout, surface, theme);
        }
    }

    /// [`cell_paint::paint`] over `buf`, reached through
    /// [`crate::tui::draw_date_picker`] because the `Buffer` →
    /// [`crate::CellSurface`] adapter is private to `crate::tui`.
    #[cfg(feature = "tui")]
    fn paint_cells(
        &self,
        layout: &DatePickerPrimitiveLayout,
        cells: &mut dyn crate::CellSurface,
        env: &crate::PaintEnv,
    ) {
        let theme = env.theme();
        match layout.field {
            Some(field) => cell_paint::paint_field(self, field, cells, theme),
            None => cell_paint::paint(self, &layout.layout, cells, theme, self.hovered_day(env)),
        }
    }
}

impl DatePicker {
    /// Resolve a keystroke through [`DatePicker::navigate`], the one
    /// place this widget's calendar keyboard model lives — so a
    /// standalone picker and a `Form` row holding one answer every key
    /// identically.
    fn key(&self, key: &Key) -> Handled<DatePickerEvent> {
        let Key::Named(named) = key else {
            return Handled::Ignored;
        };
        match self.navigate(&named.name()) {
            Some(event) => Handled::Event(event),
            None => Handled::Ignored,
        }
    }

    /// The date a [`Self::dropdown`] picker's field shows: the confirmed
    /// date, or the cursor's before any date is confirmed.
    fn field_text(&self) -> String {
        self.selected
            .unwrap_or(self.focused)
            .format(self.locale.format)
    }

    /// The inline calendar a [`Self::dropdown`] picker shows in its
    /// popup: this picker as a calendar, under the popup's own id so its
    /// painted day cells and hover are told apart from the field's.
    fn calendar(&self) -> DatePicker {
        DatePicker {
            id: WidgetId::new(format!("{}{CALENDAR_ID_SUFFIX}", self.id)),
            dropdown: false,
            open: false,
            ..self.clone()
        }
    }

    /// The index into [`DatePickerLayout::day_cells`] of the day the
    /// pointer is over, read from `env`'s hover state.
    #[cfg(any(
        feature = "tui",
        feature = "gtk",
        feature = "win",
        all(feature = "macos", target_os = "macos")
    ))]
    fn hovered_day(&self, env: &crate::PaintEnv) -> Option<usize> {
        env.hovered_sub(&self.id)
            .and_then(|sub| sub.0.checked_sub(SUB_DAY_BASE))
            .map(|i| i as usize)
            .filter(|i| *i < 42)
    }

    /// What a click on the calendar part named by `sub` means — the
    /// day, header control or time field [`Interactive::sub_regions`]
    /// tagged it with.
    fn hit_for_sub(&self, sub: SubTarget) -> DatePickerHit {
        match sub.0 {
            SUB_PREV_MONTH => DatePickerHit::PrevMonth,
            SUB_NEXT_MONTH => DatePickerHit::NextMonth,
            SUB_TIME => DatePickerHit::Time,
            n => n
                .checked_sub(SUB_DAY_BASE)
                .and_then(|i| self.month_grid().get(i as usize).copied())
                .map_or(DatePickerHit::Empty, DatePickerHit::Day),
        }
    }

    /// A press on the calendar part `hit`, inline or in the popup.
    fn click(&self, hit: DatePickerHit, cx: &mut EventCx) -> Handled<DatePickerEvent> {
        match hit {
            // The two header controls page the month the same amount
            // `PageUp`/`PageDown` do, so both affordances resolve
            // through one calendar model.
            DatePickerHit::PrevMonth => {
                cx.request_focus(None);
                Handled::Event(DatePickerEvent::FocusMoved(self.focused.add_months(-1)))
            }
            DatePickerHit::NextMonth => {
                cx.request_focus(None);
                Handled::Event(DatePickerEvent::FocusMoved(self.focused.add_months(1)))
            }
            DatePickerHit::Day(date) => {
                cx.request_focus(None);
                // A day in the month on screen is confirmed outright;
                // one of the leading/trailing days that fill out the
                // grid only moves the cursor (and, through
                // `FocusMoved`'s apply contract, the month on screen)
                // — the split [`DatePickerEvent::FocusMoved`]'s own
                // doc describes.
                if self.is_in_visible_month(date) {
                    Handled::Event(DatePickerEvent::DateSelected(date))
                } else {
                    Handled::Event(DatePickerEvent::FocusMoved(date))
                }
            }
            // The time field has no editing model of its own yet, but
            // the calendar still owns the click rather than letting it
            // fall through to whatever is painted behind it.
            DatePickerHit::Time => {
                cx.request_focus(None);
                Handled::Consumed
            }
            DatePickerHit::Empty => Handled::Ignored,
        }
    }

    /// A [`Self::dropdown`] picker's input: the field toggles the popup,
    /// the popup's calendar answers like an inline one, and the keyboard
    /// opens the popup before it navigates it.
    fn handle_dropdown(&self, event: &RoutedEvent, cx: &mut EventCx) -> Handled<DatePickerEvent> {
        match event {
            RoutedEvent::Pointer {
                kind: PointerKind::Down(crate::MouseButton::Left),
                ..
            } => {
                cx.request_focus(None);
                Handled::Event(DatePickerEvent::OpenChanged(!self.open))
            }
            RoutedEvent::PopupEvent { popup, event } if *popup == CALENDAR_POPUP => {
                match event.as_ref() {
                    RoutedEvent::Pointer {
                        kind: PointerKind::Down(crate::MouseButton::Left),
                        sub: Some(sub),
                        ..
                    } => self.click(self.hit_for_sub(*sub), cx),
                    // Hover inside the calendar is painted from the
                    // pointer position; a move only needs a repaint.
                    RoutedEvent::Pointer {
                        kind: PointerKind::Move,
                        ..
                    } => Handled::Consumed,
                    _ => Handled::Ignored,
                }
            }
            RoutedEvent::PopupDismissed { popup, .. } if *popup == CALENDAR_POPUP => {
                Handled::Event(DatePickerEvent::OpenChanged(false))
            }
            RoutedEvent::Key(key) if self.open => self.key(&key.key),
            RoutedEvent::Key(key) => match &key.key {
                Key::Named(crate::NamedKey::Enter | crate::NamedKey::Down) | Key::Char(' ') => {
                    Handled::Event(DatePickerEvent::OpenChanged(true))
                }
                _ => Handled::Ignored,
            },
            RoutedEvent::Action(A11yAction::Click) if self.open => {
                Handled::Event(DatePickerEvent::DateSelected(self.focused))
            }
            RoutedEvent::Action(A11yAction::Click) => {
                Handled::Event(DatePickerEvent::OpenChanged(true))
            }
            _ => Handled::Ignored,
        }
    }
}

/// [`Interactive::popups`]'s key for a [`DatePicker::dropdown`] picker's
/// calendar.
const CALENDAR_POPUP: PopupKey = PopupKey(0);

/// Appended to a [`DatePicker::dropdown`] picker's id to name its
/// calendar popup's content.
const CALENDAR_ID_SUFFIX: &str = ":calendar";

/// Columns a [`DatePicker::dropdown`] picker's field adds around its
/// date: one pad either side and the trailing chevron.
const FIELD_CHROME_COLUMNS: f32 = 3.0;

/// A [`DatePicker::dropdown`] picker's popup content: the inline
/// calendar over an opaque backdrop, so the popup hides whatever the
/// frame painted beneath it. It only paints and names its day cells; the
/// owning picker answers every event on it.
struct CalendarPopup(DatePicker);

impl crate::a11y::A11yPrimitive for CalendarPopup {
    fn a11y_info(&self) -> crate::a11y::A11yInfo {
        crate::a11y::A11yPrimitive::a11y_info(&self.0)
    }
}

impl Primitive for CalendarPopup {
    type Layout = DatePickerPrimitiveLayout;
    type Event = DatePickerEvent;

    fn id(&self) -> Option<&WidgetId> {
        Some(&self.0.id)
    }

    fn measure(&self, ctx: &LayoutCtx, avail: Size) -> crate::NaturalSize {
        self.0.measure(ctx, avail)
    }

    fn layout(&self, ctx: &LayoutCtx, rect: Rect) -> DatePickerPrimitiveLayout {
        Primitive::layout(&self.0, ctx, rect)
    }

    fn paint(
        &self,
        layout: &DatePickerPrimitiveLayout,
        surface: &mut dyn crate::paint_surface::PaintSurface,
        env: &crate::PaintEnv,
    ) {
        surface.surface_fill_rect(layout.area, env.theme().surface_bg);
        surface.surface_stroke_rect(layout.area, env.theme().border_fg, 1.0);
        self.0.paint(layout, surface, env);
    }

    #[cfg(feature = "tui")]
    fn paint_cells(
        &self,
        layout: &DatePickerPrimitiveLayout,
        cells: &mut dyn crate::CellSurface,
        env: &crate::PaintEnv,
    ) {
        let theme = env.theme();
        cells.fill(
            layout.area,
            crate::cell_surface::solid_style(theme.surface_fg, theme.surface_bg),
        );
        self.0.paint_cells(layout, cells, env);
    }
}

impl Interactive for CalendarPopup {
    type State = ();
    type Action = DatePickerEvent;

    fn handle(
        &self,
        _state: &mut (),
        _event: &RoutedEvent,
        _layout: &DatePickerPrimitiveLayout,
        _cx: &mut EventCx,
    ) -> Handled<DatePickerEvent> {
        Handled::Ignored
    }

    fn sub_regions(&self, layout: &DatePickerPrimitiveLayout) -> Vec<(Rect, SubTarget)> {
        self.0.sub_regions(layout)
    }
}

impl Interactive for DatePicker {
    /// Nothing: the cursor ([`DatePicker::focused`]), the month on screen
    /// ([`DatePicker::visible_month`]), the confirmed date
    /// ([`DatePicker::selected`]) and whether the popup is open
    /// ([`DatePicker::open`]) all live on the descriptor, which the host
    /// updates from [`DatePickerEvent`]; the day under the pointer is
    /// painted from the frame's hover state. So this widget has no
    /// interaction state the descriptor doesn't already carry.
    type State = ();
    type Action = DatePickerEvent;

    const OPENS_POPUPS: bool = true;

    /// One focus stop as a whole: the calendar moves its own cursor
    /// through [`Self::handle`]'s key dispatch rather than declaring a
    /// stop per day. [`DatePicker::has_focus`] is not consulted — it is
    /// the host's record of what it already focused, and gating on it
    /// would leave a never-yet-focused picker unreachable by the
    /// click-to-focus step.
    fn focus(&self) -> FocusSpec {
        FocusSpec::Stop
    }

    fn handle(
        &self,
        _state: &mut (),
        event: &RoutedEvent,
        layout: &DatePickerPrimitiveLayout,
        cx: &mut EventCx,
    ) -> Handled<DatePickerEvent> {
        if self.dropdown {
            return self.handle_dropdown(event, cx);
        }
        match event {
            RoutedEvent::Pointer {
                kind: PointerKind::Down(crate::MouseButton::Left),
                local,
                ..
            } => self.click(layout.hit_test_local(local.x, local.y), cx),
            RoutedEvent::Key(key) => self.key(&key.key),
            // `A11yAction` names no day through this generic seam, so
            // `Click` confirms whichever day the cursor is on — the same
            // one Enter confirms, matching the assistive-technology
            // default-action convention.
            RoutedEvent::Action(A11yAction::Click) => {
                Handled::Event(DatePickerEvent::DateSelected(self.focused))
            }
            _ => Handled::Ignored,
        }
    }

    /// A [`DatePicker::dropdown`] picker's calendar, below its field,
    /// while [`DatePicker::open`] is set. An outside press or Escape
    /// dismisses it, which [`Self::handle`] reports as
    /// [`DatePickerEvent::OpenChanged`]`(false)`.
    fn popups(&self, _state: &(), layout: &DatePickerPrimitiveLayout) -> Vec<Popup> {
        let (true, true, Some(field)) = (self.dropdown, self.open, layout.field) else {
            return Vec::new();
        };
        let anchor = Rect::new(
            field.x - layout.area.x,
            field.y - layout.area.y,
            field.width,
            field.height,
        );
        vec![Popup::new(
            CALENDAR_POPUP,
            anchor,
            layout.popup_size,
            CalendarPopup(self.calendar()),
        )]
    }

    fn a11y_actions(&self, _state: &()) -> Vec<A11yAction> {
        vec![A11yAction::Click]
    }

    /// One sub-part per day cell, addressed by its index into
    /// [`DatePickerLayout::day_cells`] (row-major, top-left first), after
    /// the two header controls — most-specific first, so a header control
    /// wins over the header row it sits in.
    fn sub_regions(&self, layout: &DatePickerPrimitiveLayout) -> Vec<(Rect, SubTarget)> {
        let local =
            |r: Rect| Rect::new(r.x - layout.area.x, r.y - layout.area.y, r.width, r.height);
        if let Some(field) = layout.field {
            return vec![(local(field), SubTarget(SUB_FIELD))];
        }
        let mut regions: Vec<(Rect, SubTarget)> = Vec::new();
        if let Some(prev) = layout.layout.prev_bounds {
            regions.push((local(prev), SubTarget(SUB_PREV_MONTH)));
        }
        if let Some(next) = layout.layout.next_bounds {
            regions.push((local(next), SubTarget(SUB_NEXT_MONTH)));
        }
        regions.extend(
            layout
                .layout
                .day_cells
                .iter()
                .enumerate()
                .map(|(i, cell)| (local(cell.bounds), SubTarget(SUB_DAY_BASE + i as u64))),
        );
        if let Some(time) = layout.layout.time_bounds {
            regions.push((local(time), SubTarget(SUB_TIME)));
        }
        regions
    }
}

/// [`Interactive::sub_regions`]'s tag for the header's previous-month
/// control.
const SUB_PREV_MONTH: u64 = 0;
/// [`Interactive::sub_regions`]'s tag for the header's next-month
/// control.
const SUB_NEXT_MONTH: u64 = 1;
/// [`Interactive::sub_regions`]'s tag for the time field.
const SUB_TIME: u64 = 2;
/// [`Interactive::sub_regions`]'s tag for day cell `n`, offset past the
/// three fixed tags above.
const SUB_DAY_BASE: u64 = 3;
/// [`Interactive::sub_regions`]'s tag for a [`DatePicker::dropdown`]
/// picker's field, past the 42 day-cell tags.
const SUB_FIELD: u64 = SUB_DAY_BASE + 42;

/// The cell-grid painter for [`DatePicker`], written against
/// [`crate::CellSurface`].
///
/// The month/year header with its `<`/`>` controls, then the 42-cell day
/// grid (a focus ring on `is_focused`, a selection wash on
/// `is_selected`, dimmed digits for the leading/trailing days outside
/// `in_visible_month`), then the time field when
/// [`DatePickerLayout::time_bounds`] is `Some`. The same content
/// [`native_surface_paint::paint`] puts on a pixel surface, degraded onto
/// the cell grid (see [`crate::paint_surface`]'s module doc, "Why TUI
/// stays out").
#[cfg(feature = "tui")]
pub(crate) mod cell_paint {
    use super::{DatePicker, DatePickerLayout};
    use crate::cell_surface::{solid_style, CellStyle, CellSurface};
    use crate::theme::Theme;
    use crate::types::Color;
    use crate::Rect;

    /// Fill `rect` and draw `text` on its first row, either left-aligned
    /// or centred. The header label is centred so it does not collide
    /// with the `<`/`>` glyphs painted into the header's edge columns.
    fn fill_text_aligned(
        surface: &mut dyn CellSurface,
        rect: Rect,
        text: &str,
        style: CellStyle,
        centered: bool,
    ) {
        surface.fill(rect, style);
        let x0 = rect.x.round() as u16;
        let y0 = rect.y.round() as u16;
        let w = rect.width.round() as u16;
        let len = crate::text_util::display_width(text) as u16;
        let start = if centered && len < w {
            (w - len) / 2
        } else {
            0
        };
        surface.put_str_clipped(
            x0.saturating_add(start),
            y0,
            text,
            w.saturating_sub(start),
            style,
        );
    }

    /// The four corner glyphs of a box around `rect`, and nothing else —
    /// the focus ring a day cell wears, which leaves the cell's own
    /// edges unpainted so neighbouring cells stay legible.
    fn stroke_corners(surface: &mut dyn CellSurface, rect: Rect, style: CellStyle) {
        let x0 = rect.x.round() as u16;
        let y0 = rect.y.round() as u16;
        let w = rect.width.round() as u16;
        let h = rect.height.round() as u16;
        if w == 0 || h == 0 {
            return;
        }
        let x1 = x0.saturating_add(w.saturating_sub(1));
        let y1 = y0.saturating_add(h.saturating_sub(1));
        surface.put_char(x0, y0, '┌', style);
        if x1 != x0 {
            surface.put_char(x1, y0, '┐', style);
        }
        if y1 != y0 {
            surface.put_char(x0, y1, '└', style);
            if x1 != x0 {
                surface.put_char(x1, y1, '┘', style);
            }
        }
    }

    /// Fill `rect` and draw `text` left-aligned on its first row, then —
    /// when `ring` is `Some` — decorate it.
    ///
    /// `bg` of `None` leaves every cell on the terminal's own default
    /// background instead of naming a colour, which is what an
    /// unselected day cell wants.
    ///
    /// The ring's corner glyphs must never land on a digit, so the text
    /// is inset one column either side and the ring drawn through
    /// [`stroke_corners`]. When `rect` is too narrow to spare those
    /// columns, the ring degrades to reverse video over the text rather
    /// than destroying it.
    fn fill_text_with_ring(
        surface: &mut dyn CellSurface,
        rect: Rect,
        text: &str,
        fg: Color,
        bg: Option<Color>,
        ring: Option<Color>,
    ) {
        let style = CellStyle {
            fg: Some(fg),
            bg,
            ..CellStyle::default()
        };
        surface.fill(rect, style);
        let x0 = rect.x.round() as u16;
        let y0 = rect.y.round() as u16;
        let w = rect.width.round() as u16;
        let len = crate::text_util::display_width(text) as u16;
        let has_inset_ring = ring.is_some() && w >= 3 && w >= len.saturating_add(2);
        let text_start = u16::from(has_inset_ring);
        let text_w = if has_inset_ring { w - 2 } else { w };
        surface.put_str_clipped(x0.saturating_add(text_start), y0, text, text_w, style);
        let Some(ring_fg) = ring else {
            return;
        };
        if has_inset_ring {
            stroke_corners(
                surface,
                rect,
                CellStyle {
                    fg: Some(ring_fg),
                    bg,
                    ..CellStyle::default()
                },
            );
        } else if w > 0 {
            surface.put_str_clipped(
                x0,
                y0,
                text,
                w,
                CellStyle {
                    fg: bg,
                    bg: Some(ring_fg),
                    ..CellStyle::default()
                },
            );
        }
    }

    /// Paint a dropdown `date_picker`'s one-row `field`: its date, then
    /// a chevron in the last column.
    pub(crate) fn paint_field(
        date_picker: &DatePicker,
        field: Rect,
        surface: &mut dyn CellSurface,
        theme: &Theme,
    ) {
        let style = solid_style(theme.surface_fg, theme.input_bg);
        surface.fill(field, style);
        let x0 = field.x.round() as u16;
        let y0 = field.y.round() as u16;
        let w = field.width.round() as u16;
        if w < 3 {
            return;
        }
        surface.put_str_clipped(x0 + 1, y0, &date_picker.field_text(), w - 3, style);
        surface.put_char(
            x0 + w - 1,
            y0,
            '\u{25BE}',
            solid_style(theme.muted_fg, theme.input_bg),
        );
    }

    /// Paint `date_picker` at its resolved `layout` onto `surface`, with
    /// the day cell at index `hovered` washed as hovered.
    pub(crate) fn paint(
        date_picker: &DatePicker,
        layout: &DatePickerLayout,
        surface: &mut dyn CellSurface,
        theme: &Theme,
        hovered: Option<usize>,
    ) {
        let header_style = solid_style(theme.header_fg, theme.header_bg);

        if let Some(header) = layout.header_bounds {
            if header.width > 0.0 && header.height > 0.0 {
                let label = format!(
                    "{:04}-{:02}",
                    date_picker.visible_month.year, date_picker.visible_month.month
                );
                fill_text_aligned(surface, header, &label, header_style, true);
            }
        }
        for (bounds, glyph) in [(layout.prev_bounds, "<"), (layout.next_bounds, ">")] {
            if let Some(bounds) = bounds {
                if bounds.width > 0.0 && bounds.height > 0.0 {
                    fill_text_aligned(surface, bounds, glyph, header_style, false);
                }
            }
        }

        for (i, cell) in layout.day_cells.iter().enumerate() {
            if cell.bounds.width <= 0.0 || cell.bounds.height <= 0.0 {
                continue;
            }
            // An unselected, unhovered cell keeps the terminal's own
            // background (`CellStyle`'s `None`), so the calendar sits on
            // whatever the host painted behind it.
            let cell_bg = if cell.is_selected {
                Some(theme.selected_bg)
            } else if hovered == Some(i) {
                Some(theme.inactive_selected_bg)
            } else {
                None
            };
            let fg = if cell.in_visible_month {
                theme.surface_fg
            } else {
                theme.muted_fg
            };
            fill_text_with_ring(
                surface,
                cell.bounds,
                &cell.date.day.to_string(),
                fg,
                cell_bg,
                cell.is_focused.then_some(theme.accent_fg),
            );
        }

        if let Some(time) = layout.time_bounds {
            if time.width > 0.0 && time.height > 0.0 {
                let text = date_picker.time.map(|t| t.format()).unwrap_or_default();
                fill_text_with_ring(
                    surface,
                    time,
                    &text,
                    theme.surface_fg,
                    Some(theme.input_bg),
                    Some(theme.border_fg),
                );
            }
        }
    }
}

/// Shared-paint executor for [`DatePicker`] — a rasteriser calls into
/// this (never re-deriving geometry itself) instead of walking the
/// layout fields directly, matching every other primitive's
/// `native_surface_paint` module (see `docs/PRIMITIVE_RULES.md`'s
/// "Primitive-first" rule). Gated to the backends that actually paint
/// pixels — the TUI backend renders `DatePicker` through its own
/// cell-grid path instead.
#[cfg(any(
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
#[allow(dead_code)]
pub(crate) mod native_surface_paint {
    use super::{DatePicker, DatePickerLayout};
    use crate::event::Rect;
    use crate::paint_surface::PaintSurface;
    use crate::theme::Theme;

    /// Paint a dropdown [`DatePicker`]'s one-row `field`: a bordered input
    /// box holding its date, with a chevron at the right edge.
    pub(crate) fn paint_field(
        date_picker: &DatePicker,
        field: Rect,
        surface: &mut dyn PaintSurface,
        theme: &Theme,
    ) {
        surface.surface_fill_rect(field, theme.input_bg);
        surface.surface_stroke_rect(field, theme.border_fg, 1.0);
        let pad = surface.surface_char_width();
        let chevron = "\u{25BE}";
        let (chevron_w, _) = surface.surface_measure_text(chevron);
        let text_w = (field.width - 3.0 * pad - chevron_w).max(0.0);
        surface.surface_draw_text_run(
            Rect::new(field.x + pad, field.y, text_w, field.height),
            &date_picker.field_text(),
            theme.surface_fg,
        );
        surface.surface_draw_text_run(
            Rect::new(
                field.x + field.width - pad - chevron_w,
                field.y,
                chevron_w,
                field.height,
            ),
            chevron,
            theme.muted_fg,
        );
    }

    /// Paint a [`DatePicker`] onto `surface`: the month/year header with
    /// prev/next controls, the 42-cell day grid (focus ring on
    /// `is_focused`, fill on `is_selected`, dimmed text for cells outside
    /// `in_visible_month`), and — when `layout.time_bounds` is `Some` —
    /// the time field.
    pub(crate) fn paint(
        date_picker: &DatePicker,
        layout: &DatePickerLayout,
        surface: &mut dyn PaintSurface,
        theme: &Theme,
        hovered: Option<usize>,
    ) {
        if let Some(header) = layout.header_bounds {
            if header.width > 0.0 && header.height > 0.0 {
                surface.surface_fill_rect(header, theme.header_bg);
                let label = format!(
                    "{:04}-{:02}",
                    date_picker.visible_month.year, date_picker.visible_month.month
                );
                // Centre the label between the prev/next controls so it
                // never overlaps the glyphs drawn into the header's edge
                // columns (mirrors `tui::date_picker`'s `fill_text_aligned`
                // centring, which solves the identical collision).
                let left = layout
                    .prev_bounds
                    .map(|r| r.x + r.width)
                    .unwrap_or(header.x);
                let right = layout
                    .next_bounds
                    .map(|r| r.x)
                    .unwrap_or(header.x + header.width);
                let available = (right - left).max(0.0);
                let (label_w, _) = surface.surface_measure_text(&label);
                let label_x = left + ((available - label_w) / 2.0).max(0.0);
                let label_rect =
                    Rect::new(label_x, header.y, label_w.min(available), header.height);
                surface.surface_draw_text_run(label_rect, &label, theme.header_fg);
            }
        }
        if let Some(prev) = layout.prev_bounds {
            if prev.width > 0.0 && prev.height > 0.0 {
                surface.surface_draw_text_run(prev, "<", theme.header_fg);
            }
        }
        if let Some(next) = layout.next_bounds {
            if next.width > 0.0 && next.height > 0.0 {
                surface.surface_draw_text_run(next, ">", theme.header_fg);
            }
        }

        for (i, cell) in layout.day_cells.iter().enumerate() {
            if cell.bounds.width <= 0.0 || cell.bounds.height <= 0.0 {
                continue;
            }
            if cell.is_selected {
                surface.surface_fill_rect(cell.bounds, theme.selected_bg);
            } else if hovered == Some(i) {
                surface.surface_fill_rect(cell.bounds, theme.inactive_selected_bg);
            }
            if cell.is_focused {
                surface.surface_stroke_rect(cell.bounds, theme.accent_fg, 1.0);
            }
            let fg = if cell.in_visible_month {
                theme.surface_fg
            } else {
                theme.muted_fg
            };
            surface.surface_draw_text_run(cell.bounds, &cell.date.day.to_string(), fg);
        }

        if let Some(time) = layout.time_bounds {
            if time.width > 0.0 && time.height > 0.0 {
                surface.surface_fill_rect(time, theme.input_bg);
                surface.surface_stroke_rect(time, theme.border_fg, 1.0);
                if let Some(t) = date_picker.time {
                    surface.surface_draw_text_run(time, &t.format(), theme.surface_fg);
                }
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::backend::ImagePaintResult;
        use crate::event::{Point, Rect, Viewport};
        use crate::primitives::date_picker::{CalendarDate, TimeOfDay, Weekday};
        use crate::types::{Color, WidgetId};
        use crate::Image;

        /// Records every surface verb `DatePicker`'s paint uses — mirrors
        /// `primitives::combo_box`'s identical test double, so this test
        /// runs on any host without Cairo/Core Graphics/Direct2D.
        #[derive(Default)]
        struct RecordingSurface {
            fills: Vec<(Rect, Color)>,
            strokes: Vec<(Rect, Color)>,
            text_runs: Vec<(Rect, String, Color)>,
        }

        impl PaintSurface for RecordingSurface {
            fn surface_begin_frame(&mut self, _viewport: Viewport) {}
            fn surface_end_frame(&mut self) {}
            fn surface_viewport(&self) -> Viewport {
                Viewport::new(200.0, 100.0, 1.0)
            }
            fn surface_line_height(&self) -> f32 {
                16.0
            }
            fn surface_char_width(&self) -> f32 {
                8.0
            }
            fn surface_measure_text(&self, text: &str) -> (f32, f32) {
                (text.chars().count() as f32 * 8.0, 16.0)
            }
            fn surface_fill_rect(&mut self, rect: Rect, color: Color) {
                self.fills.push((rect, color));
            }
            fn surface_fill_rounded_rect(&mut self, rect: Rect, _radius: f32, color: Color) {
                self.fills.push((rect, color));
            }
            fn surface_stroke_rect(&mut self, rect: Rect, color: Color, _stroke_width: f32) {
                self.strokes.push((rect, color));
            }
            fn surface_stroke_rounded_rect(
                &mut self,
                _rect: Rect,
                _radius: f32,
                _color: Color,
                _stroke_width: f32,
            ) {
            }
            fn surface_draw_text_run(&mut self, rect: Rect, text: &str, color: Color) {
                self.text_runs.push((rect, text.to_string(), color));
            }
            fn surface_draw_line(
                &mut self,
                _from: Point,
                _to: Point,
                _color: Color,
                _stroke_width: f32,
            ) {
            }
            fn surface_push_clip(&mut self, _rect: Rect) {}
            fn surface_pop_clip(&mut self) {}
            fn surface_draw_image(&mut self, _rect: Rect, _image: &Image) -> ImagePaintResult {
                ImagePaintResult::Unsupported
            }
        }

        fn picker() -> DatePicker {
            let mut picker =
                DatePicker::new(WidgetId::new("test:date"), CalendarDate::new(2024, 3, 15));
            picker.locale.first_weekday = Weekday::Monday;
            picker
        }

        #[test]
        fn paints_header_label_and_prev_next_controls() {
            let picker = picker();
            let layout = picker.layout(0.0, 0.0, 10.0, 2.0, 1.0, 0.0);
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            paint(&picker, &layout, &mut surface, &theme, None);

            assert!(surface
                .fills
                .contains(&(layout.header_bounds.unwrap(), theme.header_bg)));
            let labels: Vec<&str> = surface
                .text_runs
                .iter()
                .map(|(_, text, _)| text.as_str())
                .collect();
            assert!(labels.contains(&"2024-03"));
            assert!(labels.contains(&"<"));
            assert!(labels.contains(&">"));

            // The label's run must not overlap the prev/next controls: it
            // starts no earlier than where `prev_bounds` ends and finishes
            // no later than where `next_bounds` begins.
            let (label_rect, _, _) = surface
                .text_runs
                .iter()
                .find(|(_, text, _)| text == "2024-03")
                .expect("label run is emitted");
            let prev_bounds = layout.prev_bounds.unwrap();
            let next_bounds = layout.next_bounds.unwrap();
            assert!(
                label_rect.x >= prev_bounds.x + prev_bounds.width,
                "label rect {label_rect:?} overlaps prev_bounds {prev_bounds:?}"
            );
            assert!(
                label_rect.x + label_rect.width <= next_bounds.x,
                "label rect {label_rect:?} overlaps next_bounds {next_bounds:?}"
            );
        }

        #[test]
        fn selected_day_gets_a_fill_and_focused_day_gets_a_stroke() {
            let mut picker = picker();
            picker.selected = Some(CalendarDate::new(2024, 3, 10));
            let layout = picker.layout(0.0, 0.0, 10.0, 2.0, 1.0, 0.0);
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            paint(&picker, &layout, &mut surface, &theme, None);

            let selected_bounds = layout
                .day_cells
                .iter()
                .find(|c| c.is_selected)
                .unwrap()
                .bounds;
            assert!(surface
                .fills
                .contains(&(selected_bounds, theme.selected_bg)));

            let focused_bounds = layout
                .day_cells
                .iter()
                .find(|c| c.is_focused)
                .unwrap()
                .bounds;
            assert!(surface.strokes.contains(&(focused_bounds, theme.accent_fg)));
        }

        #[test]
        fn days_outside_the_visible_month_paint_with_muted_text() {
            let picker = picker();
            let layout = picker.layout(0.0, 0.0, 10.0, 2.0, 1.0, 0.0);
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            paint(&picker, &layout, &mut surface, &theme, None);

            let leading_cell = layout.day_cells[0];
            assert!(!leading_cell.in_visible_month);
            assert!(surface.text_runs.contains(&(
                leading_cell.bounds,
                leading_cell.date.day.to_string(),
                theme.muted_fg
            )));
        }

        #[test]
        fn time_field_paints_only_when_reserved_and_set() {
            let mut picker = picker();
            picker.time = Some(TimeOfDay::new(9, 30));
            let layout = picker.layout(0.0, 0.0, 10.0, 2.0, 1.0, 2.0);
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            paint(&picker, &layout, &mut surface, &theme, None);

            let time_bounds = layout.time_bounds.expect("time field reserved");
            assert!(surface.strokes.contains(&(time_bounds, theme.border_fg)));
            assert!(surface.text_runs.iter().any(|(_, text, _)| text == "09:30"));

            let no_time_layout = picker.layout(0.0, 0.0, 10.0, 2.0, 1.0, 0.0);
            let mut surface2 = RecordingSurface::default();
            paint(&picker, &no_time_layout, &mut surface2, &theme, None);
            assert!(no_time_layout.time_bounds.is_none());
        }

        #[test]
        fn zero_size_header_and_time_paint_nothing_for_those_regions() {
            let picker = picker();
            let layout = picker.layout(0.0, 0.0, 10.0, 2.0, 0.0, 0.0);
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            paint(&picker, &layout, &mut surface, &theme, None);

            assert!(layout.header_bounds.is_none());
            assert!(layout.time_bounds.is_none());
            assert!(!surface.fills.contains(&(Rect::default(), theme.header_bg)));
        }
    }
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
            assert_eq!(
                CalendarDate::from_days_since_epoch(z),
                d,
                "round-trip for {d:?}"
            );
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
            ..Default::default()
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
            ..Default::default()
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
            ..Default::default()
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
            ..Default::default()
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
            ..Default::default()
        };
        assert_eq!(
            picker.navigate("Enter"),
            Some(DatePickerEvent::DateSelected(CalendarDate::new(
                2024, 3, 15
            )))
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
            ..Default::default()
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
            ..Default::default()
        };
        let layout = picker.layout(0.0, 0.0, 10.0, 2.0, 0.0, 0.0);
        assert_eq!(layout.header_bounds, None);
        assert_eq!(layout.time_bounds, None);
        assert_eq!(layout.prev_bounds, None);
        assert_eq!(layout.next_bounds, None);
    }

    #[test]
    fn bounds_encloses_header_grid_and_time_field() {
        let picker = DatePicker {
            id: WidgetId::new("dp"),
            visible_month: CalendarDate::new(2024, 3, 1),
            selected: None,
            focused: CalendarDate::new(2024, 3, 15),
            locale: DateLocale::default(),
            time: Some(TimeOfDay::new(10, 30)),
            has_focus: true,
            ..Default::default()
        };
        let layout = picker.layout(5.0, 5.0, 10.0, 2.0, 1.0, 2.0);

        let bounds = layout.bounds();
        assert_eq!(bounds.x, 5.0);
        assert_eq!(bounds.y, 5.0);
        assert_eq!(bounds.width, 70.0);
        // header (1.0) + 6 grid rows (12.0) + time field (2.0).
        assert_eq!(bounds.height, 15.0);
    }

    #[test]
    fn bounds_covers_just_the_grid_when_header_and_time_are_absent() {
        let picker = DatePicker::new(WidgetId::new("dp"), CalendarDate::new(2024, 3, 1));
        let layout = picker.layout(0.0, 0.0, 10.0, 2.0, 0.0, 0.0);

        let bounds = layout.bounds();
        assert_eq!(bounds, Rect::new(0.0, 0.0, 70.0, 12.0));
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

    // ── Cell-path paint (`cell_paint::paint`) ───────────────────────────

    #[cfg(feature = "tui")]
    mod cells {
        use super::super::cell_test_paint::draw_date_picker;
        use super::*;
        use crate::theme::Theme;
        use ratatui::buffer::Buffer;
        use ratatui::layout::Rect as RRect;

        fn cell_char(buf: &Buffer, x: u16, y: u16) -> char {
            buf[(x, y)].symbol().chars().next().unwrap_or(' ')
        }

        fn mk_date_picker() -> DatePicker {
            let mut picker = DatePicker::new(WidgetId::new("date"), CalendarDate::new(2024, 3, 15));
            picker.has_focus = true;
            picker
        }

        fn paint_and_click_round_trip_at(origin_x: u16, origin_y: u16) {
            let date_picker = mk_date_picker();
            let layout = date_picker.layout(origin_x as f32, origin_y as f32, 4.0, 1.0, 1.0, 0.0);

            let rect = RRect::new(0, 0, 80, 24);
            let mut buf = Buffer::empty(rect);
            draw_date_picker(&mut buf, &date_picker, &layout, &Theme::default());

            // Header label is centered in the header row, clear of the
            // prev/next glyphs painted into the header's edge columns.
            let header = layout.header_bounds.expect("header is reserved");
            let label_len = 7u16; // "2024-03"
            let label_x = header.x as u16 + (header.width as u16 - label_len) / 2;
            assert_eq!(cell_char(&buf, label_x, origin_y), '2');
            // Prev/next glyphs still own the header's edge columns.
            assert_eq!(cell_char(&buf, origin_x, origin_y), '<');

            let focused_cell = layout
                .day_cells
                .iter()
                .find(|c| c.is_focused)
                .expect("focused day is in the grid");
            let fx = focused_cell.bounds.x as u16;
            let fy = focused_cell.bounds.y as u16;
            // Focused day's top-left corner gets the focus-ring glyph,
            // but the digits it decorates stay readable: they are inset
            // by one column so the ring glyph never overwrites them.
            assert_eq!(cell_char(&buf, fx, fy), '┌');
            for (i, ch) in focused_cell.date.day.to_string().chars().enumerate() {
                assert_eq!(cell_char(&buf, fx + 1 + i as u16, fy), ch);
            }

            assert_eq!(
                layout.hit_test(focused_cell.bounds.x + 1.0, focused_cell.bounds.y),
                DatePickerHit::Day(focused_cell.date)
            );
        }

        #[test]
        fn paint_and_click_round_trip() {
            paint_and_click_round_trip_at(0, 0);
        }

        /// Paint and hit-test must work correctly when the control is not
        /// at the screen origin.
        #[test]
        fn paint_and_click_round_trip_at_nonzero_origin() {
            paint_and_click_round_trip_at(7, 3);
        }

        #[test]
        fn days_outside_the_visible_month_paint_muted() {
            let picker = mk_date_picker();
            let layout = picker.layout(0.0, 0.0, 4.0, 1.0, 1.0, 0.0);
            let mut buf = Buffer::empty(RRect::new(0, 0, 80, 24));
            draw_date_picker(&mut buf, &picker, &layout, &Theme::default());

            let outside = layout
                .day_cells
                .iter()
                .find(|c| !c.in_visible_month)
                .expect("a complete grid has leading or trailing days");
            let cell = &buf[(outside.bounds.x as u16, outside.bounds.y as u16)];
            assert_eq!(
                cell.fg,
                crate::tui::ratatui_color(Theme::default().muted_fg),
                "{:?} should be muted",
                outside.date
            );
        }

        #[test]
        fn the_time_field_paints_its_clock_when_one_is_set() {
            let mut picker = mk_date_picker();
            picker.time = Some(TimeOfDay::new(9, 30));
            let layout = picker.layout(0.0, 0.0, 4.0, 1.0, 1.0, 1.0);
            let mut buf = Buffer::empty(RRect::new(0, 0, 80, 24));
            draw_date_picker(&mut buf, &picker, &layout, &Theme::default());

            let time = layout.time_bounds.expect("time row is reserved");
            let painted: String = (0..5)
                .map(|i| cell_char(&buf, time.x as u16 + 1 + i, time.y as u16))
                .collect();
            assert_eq!(painted, "09:30");
        }
    }
}

/// [`Interactive`] dispatch, driven directly the way an out-of-process
/// binding host reaches a widget — the in-runtime leg is
/// `tests/widget_contract_date_picker.rs`.
#[cfg(test)]
mod interactive_tests {
    use super::*;
    use crate::event::NamedKey;
    use crate::interactive::{EventHost, KeyEvent};
    use crate::types::Modifiers;
    use crate::PointerShape;

    /// Minimal [`EventHost`] mock: records every call so a test can
    /// assert on what [`Interactive::handle`] asked for.
    #[derive(Debug, Default)]
    struct MockHost {
        focus_requests: Vec<Option<SubTarget>>,
    }

    impl EventHost for MockHost {
        fn is_focused(&self, _id: &WidgetId, _sub: Option<SubTarget>) -> bool {
            false
        }
        fn request_focus(&mut self, _id: &WidgetId, sub: Option<SubTarget>) {
            self.focus_requests.push(sub);
        }
        fn capture_pointer(&mut self, _id: &WidgetId) {}
        fn release_pointer(&mut self) {}
        fn set_cursor(&mut self, _cursor: PointerShape) {}
        fn request_redraw(&mut self) {}
        fn emit(&mut self, _event: crate::UiEvent) {}
        fn units(&self) -> Units {
            Units::Cells
        }
    }

    const AREA: Rect = Rect::new(3.0, 2.0, 28.0, 7.0);

    fn picker() -> DatePicker {
        let mut p = DatePicker::new(WidgetId::new("due-date"), CalendarDate::new(2024, 3, 15));
        p.has_focus = true;
        p
    }

    fn layout_for(p: &DatePicker) -> DatePickerPrimitiveLayout {
        let ctx = LayoutCtx::new(1.0, 1.0, false);
        Primitive::layout(p, &ctx, AREA)
    }

    fn dispatch(p: &DatePicker, event: &RoutedEvent) -> Handled<DatePickerEvent> {
        let layout = layout_for(p);
        let mut host = MockHost::default();
        let mut cx = EventCx::new(p.id, &mut host);
        p.handle(&mut (), event, &layout, &mut cx)
    }

    fn key(named: NamedKey) -> RoutedEvent {
        RoutedEvent::Key(KeyEvent {
            key: Key::Named(named),
            modifiers: Modifiers::default(),
            repeat: false,
        })
    }

    fn pointer_down(rect: Rect, layout: &DatePickerPrimitiveLayout) -> RoutedEvent {
        RoutedEvent::Pointer {
            kind: PointerKind::Down(crate::MouseButton::Left),
            sub: None,
            local: Point::new(
                rect.x + rect.width / 2.0 - layout.area.x,
                rect.y + rect.height / 2.0 - layout.area.y,
            ),
            mods: Modifiers::default(),
        }
    }

    #[test]
    fn the_calendar_is_one_focus_stop() {
        assert_eq!(picker().focus(), FocusSpec::Stop);
    }

    #[test]
    fn layout_quantises_day_cells_to_whole_cells_on_a_cell_grid() {
        let layout = layout_for(&picker());
        // 28 columns / 7 days = 4 columns; 7 rows minus the header row
        // leaves 6, one per week row.
        assert_eq!(layout.cell_size, Size::new(4.0, 1.0));
        assert_eq!(layout.layout.day_cells.len(), 42);
        assert_eq!(
            layout.layout.header_bounds,
            Some(Rect::new(3.0, 2.0, 28.0, 1.0))
        );
        assert_eq!(layout.layout.time_bounds, None, "no time field by default");
    }

    #[test]
    fn a_time_field_reserves_its_own_row_under_the_grid() {
        let mut p = picker();
        p.time = Some(TimeOfDay::new(7, 5));
        let layout = layout_for(&p);
        let time = layout.layout.time_bounds.expect("time row is reserved");
        assert_eq!(time.height, 1.0);
        // Six week rows of a now-shorter cell height sit above it.
        assert_eq!(layout.cell_size.height, 1.0);
    }

    #[test]
    fn arrow_keys_move_the_cursor_by_a_day_and_a_week() {
        let p = picker();
        for (named, expected) in [
            (NamedKey::Left, CalendarDate::new(2024, 3, 14)),
            (NamedKey::Right, CalendarDate::new(2024, 3, 16)),
            (NamedKey::Up, CalendarDate::new(2024, 3, 8)),
            (NamedKey::Down, CalendarDate::new(2024, 3, 22)),
        ] {
            assert_eq!(
                dispatch(&p, &key(named)),
                Handled::Event(DatePickerEvent::FocusMoved(expected))
            );
        }
    }

    #[test]
    fn page_keys_move_the_cursor_by_a_month_and_enter_confirms_it() {
        let p = picker();
        assert_eq!(
            dispatch(&p, &key(NamedKey::PageDown)),
            Handled::Event(DatePickerEvent::FocusMoved(CalendarDate::new(2024, 4, 15)))
        );
        assert_eq!(
            dispatch(&p, &key(NamedKey::PageUp)),
            Handled::Event(DatePickerEvent::FocusMoved(CalendarDate::new(2024, 2, 15)))
        );
        assert_eq!(
            dispatch(&p, &key(NamedKey::Enter)),
            Handled::Event(DatePickerEvent::DateSelected(CalendarDate::new(
                2024, 3, 15
            )))
        );
        // An unrecognised key stays unhandled, so a host's own binding
        // still sees it.
        assert_eq!(dispatch(&p, &key(NamedKey::Escape)), Handled::Ignored);
    }

    #[test]
    fn the_header_controls_page_the_month_the_same_amount_the_page_keys_do() {
        let p = picker();
        let layout = layout_for(&p);
        let prev = layout.layout.prev_bounds.expect("header is reserved");
        let next = layout.layout.next_bounds.expect("header is reserved");
        assert_eq!(
            dispatch(&p, &pointer_down(prev, &layout)),
            Handled::Event(DatePickerEvent::FocusMoved(CalendarDate::new(2024, 2, 15)))
        );
        assert_eq!(
            dispatch(&p, &pointer_down(next, &layout)),
            Handled::Event(DatePickerEvent::FocusMoved(CalendarDate::new(2024, 4, 15)))
        );
    }

    #[test]
    fn clicking_a_day_confirms_it_but_an_adjacent_month_day_only_moves_the_cursor() {
        let p = picker();
        let layout = layout_for(&p);
        let inside = layout
            .layout
            .day_cells
            .iter()
            .find(|c| c.in_visible_month)
            .expect("the month on screen has days");
        assert_eq!(
            dispatch(&p, &pointer_down(inside.bounds, &layout)),
            Handled::Event(DatePickerEvent::DateSelected(inside.date))
        );

        let outside = layout
            .layout
            .day_cells
            .iter()
            .find(|c| !c.in_visible_month)
            .expect("a complete grid has leading or trailing days");
        assert_eq!(
            dispatch(&p, &pointer_down(outside.bounds, &layout)),
            Handled::Event(DatePickerEvent::FocusMoved(outside.date))
        );
    }

    #[test]
    fn a_day_click_also_takes_keyboard_focus() {
        let p = picker();
        let layout = layout_for(&p);
        let cell = layout.layout.day_cells[10].bounds;
        let mut host = MockHost::default();
        let mut cx = EventCx::new(p.id, &mut host);
        let _ = p.handle(&mut (), &pointer_down(cell, &layout), &layout, &mut cx);
        assert_eq!(host.focus_requests, vec![None]);
    }

    #[test]
    fn the_a11y_click_action_confirms_the_day_under_the_cursor() {
        let p = picker();
        assert_eq!(
            dispatch(&p, &RoutedEvent::Action(A11yAction::Click)),
            Handled::Event(DatePickerEvent::DateSelected(CalendarDate::new(
                2024, 3, 15
            )))
        );
        assert_eq!(p.a11y_actions(&()), vec![A11yAction::Click]);
    }

    #[test]
    fn sub_regions_name_the_header_controls_then_every_day_cell() {
        let p = picker();
        let layout = layout_for(&p);
        let regions = p.sub_regions(&layout);
        assert_eq!(regions.len(), 2 + 42);
        assert_eq!(regions[0].1, SubTarget(SUB_PREV_MONTH));
        assert_eq!(regions[1].1, SubTarget(SUB_NEXT_MONTH));
        assert_eq!(regions[2].1, SubTarget(SUB_DAY_BASE));
        for (rect, _) in &regions {
            assert!(rect.x >= 0.0 && rect.y >= 0.0, "{rect:?} is not local");
        }
    }

    #[test]
    fn both_outcomes_ride_the_two_form_date_channels_under_the_pickers_id() {
        let id = WidgetId::new("due-date");
        let date = CalendarDate::new(2024, 3, 16);
        assert_eq!(
            DatePickerEvent::DateSelected(date).into_ui_event(id),
            crate::UiEvent::Form(id, crate::FormEvent::DateChanged { id, value: date })
        );
        assert_eq!(
            DatePickerEvent::FocusMoved(date).into_ui_event(id),
            crate::UiEvent::Form(id, crate::FormEvent::DateFocusMoved { id, value: date })
        );
    }

    #[test]
    fn measure_reports_seven_day_columns_and_a_header_plus_six_week_rows() {
        let ctx = LayoutCtx::new(1.0, 1.0, false);
        let size = Primitive::measure(&picker(), &ctx, Size::new(80.0, 24.0));
        assert_eq!(size.width, Some(CELL_COLUMNS * 7.0));
        assert_eq!(size.height, Some(7.0));

        let mut timed = picker();
        timed.time = Some(TimeOfDay::new(0, 0));
        let size = Primitive::measure(&timed, &ctx, Size::new(80.0, 24.0));
        assert_eq!(size.height, Some(8.0), "the time field adds a row");
    }
    fn dropdown(open: bool) -> DatePicker {
        let mut p = picker();
        p.dropdown = true;
        p.open = open;
        p
    }

    fn popup_press(sub: u64) -> RoutedEvent {
        RoutedEvent::PopupEvent {
            popup: CALENDAR_POPUP,
            event: Box::new(RoutedEvent::Pointer {
                kind: PointerKind::Down(crate::MouseButton::Left),
                sub: Some(SubTarget(sub)),
                local: Point::new(0.0, 0.0),
                mods: Modifiers::default(),
            }),
        }
    }

    #[test]
    fn a_dropdown_lays_out_one_field_row_and_names_it_as_its_only_part() {
        let p = dropdown(false);
        let layout = layout_for(&p);
        assert_eq!(layout.field, Some(Rect::new(3.0, 2.0, 28.0, 1.0)));
        assert!(layout.layout.day_cells.is_empty());
        assert_eq!(
            p.sub_regions(&layout),
            vec![(Rect::new(0.0, 0.0, 28.0, 1.0), SubTarget(SUB_FIELD))]
        );
        // The popup asks for the inline calendar's own measured size.
        assert_eq!(layout.popup_size, Size::new(28.0, 7.0));
    }

    #[test]
    fn a_dropdown_owns_a_calendar_popup_only_while_open() {
        let closed = dropdown(false);
        assert!(closed.popups(&(), &layout_for(&closed)).is_empty());
        let inline = picker();
        assert!(inline.popups(&(), &layout_for(&inline)).is_empty());

        let open = dropdown(true);
        let popups = open.popups(&(), &layout_for(&open));
        assert_eq!(popups.len(), 1);
        assert_eq!(popups[0].key, CALENDAR_POPUP);
        assert_eq!(popups[0].anchor, Rect::new(0.0, 0.0, 28.0, 1.0));
        assert_eq!(
            popups[0].content.id(),
            Some(&WidgetId::new("due-date:calendar"))
        );
    }

    #[test]
    fn clicking_the_field_toggles_the_popup() {
        let event = pointer_down(
            Rect::new(3.0, 2.0, 28.0, 1.0),
            &layout_for(&dropdown(false)),
        );
        assert_eq!(
            dispatch(&dropdown(false), &event),
            Handled::Event(DatePickerEvent::OpenChanged(true))
        );
        assert_eq!(
            dispatch(&dropdown(true), &event),
            Handled::Event(DatePickerEvent::OpenChanged(false))
        );
    }

    #[test]
    fn a_dismissed_popup_asks_the_app_to_close_it() {
        for reason in [
            crate::popup::DismissReason::Escape,
            crate::popup::DismissReason::OutsideClick,
        ] {
            let event = RoutedEvent::PopupDismissed {
                popup: CALENDAR_POPUP,
                reason,
            };
            assert_eq!(
                dispatch(&dropdown(true), &event),
                Handled::Event(DatePickerEvent::OpenChanged(false))
            );
        }
    }

    #[test]
    fn presses_on_the_popup_calendar_answer_like_the_inline_one() {
        let p = dropdown(true);
        assert_eq!(
            dispatch(&p, &popup_press(SUB_NEXT_MONTH)),
            Handled::Event(DatePickerEvent::FocusMoved(CalendarDate::new(2024, 4, 15)))
        );
        assert_eq!(
            dispatch(&p, &popup_press(SUB_PREV_MONTH)),
            Handled::Event(DatePickerEvent::FocusMoved(CalendarDate::new(2024, 2, 15)))
        );
        // March 2024 starts on a Friday; with Sunday first, the grid
        // opens on Sunday 25 February, so 1 March is day cell 5.
        assert_eq!(
            dispatch(&p, &popup_press(SUB_DAY_BASE + 5)),
            Handled::Event(DatePickerEvent::DateSelected(CalendarDate::new(2024, 3, 1)))
        );
        assert_eq!(
            dispatch(&p, &popup_press(SUB_DAY_BASE)),
            Handled::Event(DatePickerEvent::FocusMoved(CalendarDate::new(2024, 2, 25)))
        );
    }

    #[test]
    fn the_keyboard_opens_a_closed_dropdown_then_navigates_it() {
        assert_eq!(
            dispatch(&dropdown(false), &key(NamedKey::Down)),
            Handled::Event(DatePickerEvent::OpenChanged(true))
        );
        assert_eq!(
            dispatch(&dropdown(false), &key(NamedKey::Left)),
            Handled::Ignored
        );
        assert_eq!(
            dispatch(&dropdown(true), &key(NamedKey::Down)),
            Handled::Event(DatePickerEvent::FocusMoved(CalendarDate::new(2024, 3, 22)))
        );
    }

    #[test]
    fn the_open_event_rides_the_date_open_channel() {
        let id = WidgetId::new("due-date");
        assert_eq!(
            DatePickerEvent::OpenChanged(true).into_ui_event(id),
            crate::UiEvent::Form(id, crate::FormEvent::DateOpenChanged { id, open: true })
        );
    }
}

/// Cell-painting entry points used by this module's own `#[cfg(test)]`
/// rasteriser tests: they build the `Buffer` -> [`crate::CellSurface`]
/// adapter (private to `crate::tui`) the same way
/// [`crate::primitive::Primitive::paint_cells`] does, so a test can
/// paint into a bare ratatui buffer without a backend.
#[cfg(all(test, feature = "tui"))]
mod cell_test_paint {
    use super::*;
    use crate::theme::Theme;
    use ratatui::buffer::Buffer;

    fn surface<'a>(
        buf: &'a mut Buffer,
        sink: &'a mut Option<(u16, u16, crate::cell_surface::CursorShape)>,
    ) -> impl crate::CellSurface + 'a {
        crate::tui::cell_surface::TuiCellSurface::over(
            buf,
            crate::backend::ColorDepth::TrueColor,
            sink,
        )
    }

    pub fn draw_date_picker(
        buf: &mut Buffer,
        date_picker: &DatePicker,
        layout: &DatePickerLayout,
        theme: &Theme,
    ) {
        let mut sink = None;
        cell_paint::paint(
            date_picker,
            layout,
            &mut surface(buf, &mut sink),
            theme,
            None,
        );
    }
}
