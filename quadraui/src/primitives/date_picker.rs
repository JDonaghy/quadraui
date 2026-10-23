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
    /// `Enter` or a click on a day confirmed that date as `selected`.
    DateSelected(CalendarDate),
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

        for cell in &layout.day_cells {
            if cell.bounds.width <= 0.0 || cell.bounds.height <= 0.0 {
                continue;
            }
            if cell.is_selected {
                surface.surface_fill_rect(cell.bounds, theme.selected_bg);
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
            paint(&picker, &layout, &mut surface, &theme);

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
            paint(&picker, &layout, &mut surface, &theme);

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
            paint(&picker, &layout, &mut surface, &theme);

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
            paint(&picker, &layout, &mut surface, &theme);

            let time_bounds = layout.time_bounds.expect("time field reserved");
            assert!(surface.strokes.contains(&(time_bounds, theme.border_fg)));
            assert!(surface.text_runs.iter().any(|(_, text, _)| text == "09:30"));

            let no_time_layout = picker.layout(0.0, 0.0, 10.0, 2.0, 1.0, 0.0);
            let mut surface2 = RecordingSurface::default();
            paint(&picker, &no_time_layout, &mut surface2, &theme);
            assert!(no_time_layout.time_bounds.is_none());
        }

        #[test]
        fn zero_size_header_and_time_paint_nothing_for_those_regions() {
            let picker = picker();
            let layout = picker.layout(0.0, 0.0, 10.0, 2.0, 0.0, 0.0);
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            paint(&picker, &layout, &mut surface, &theme);

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
    fn bounds_encloses_header_grid_and_time_field() {
        let picker = DatePicker {
            id: WidgetId::new("dp"),
            visible_month: CalendarDate::new(2024, 3, 1),
            selected: None,
            focused: CalendarDate::new(2024, 3, 15),
            locale: DateLocale::default(),
            time: Some(TimeOfDay::new(10, 30)),
            has_focus: true,
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
}
