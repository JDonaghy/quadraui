//! Direct2D / DirectWrite rasteriser for [`crate::DataTable`].
//!
//! Painting uses the shared [`crate::primitives::data_table::native_surface_paint::paint`].
//! See that function's documentation for the full per-backend divergence survey.
//! This module now uses the shared, translucent
//! [`crate::primitives::scrollbar::native_surface_paint::paint`] for scrollbars
//! and respects each footer span's foreground colour.
//! [`draw_data_table`] is a thin wrapper over the shared paint, using
//! [`crate::win::surface::D2dSurface`] as the `PaintSurface` adapter.
//!
//! [`win_data_table_layout`] is pure geometry generic over
//! [`crate::primitives::layout_metrics::TextMeasure`] — no Direct2D/
//! DirectWrite type in its signature — so it compiles and runs
//! everywhere. Only [`draw_data_table`] is `#[cfg(target_os = "windows")]`-gated.
//! See `win::status_bar`'s module doc for why colours come from `Theme::default()`
//! rather than a live `WinBackend` theme field.

#[cfg(target_os = "windows")]
use windows::Win32::Graphics::Direct2D::ID2D1RenderTarget;

use crate::event::Rect;
use crate::primitives::data_table::DataTable;
use crate::primitives::layout_metrics::{pixel_data_table_layout, TextMeasure};
#[cfg(target_os = "windows")]
use crate::theme::Theme;
use crate::DataTableLayout;

/// Compute a [`DataTable`]'s layout without painting — the DirectWrite
/// twin of [`draw_data_table`]'s internal layout call. Pure geometry over
/// [`TextMeasure`] — `measure` may be a live `&DWrite` (when
/// painting) or [`super::backend`]'s nominal measurer (no surface yet).
/// Shares its column-measurement path with `gtk_data_table_layout` /
/// `mac_data_table_layout` via [`pixel_data_table_layout`].
pub fn win_data_table_layout(
    measure: &dyn TextMeasure,
    rect: Rect,
    table: &DataTable,
    line_height: f32,
) -> DataTableLayout {
    pixel_data_table_layout(table, rect.width, rect.height, line_height, measure)
}

/// Draw a [`DataTable`] into `rect` (DIPs) on `target`. Returns the
/// resolved [`DataTableLayout`] for host click dispatch.
///
/// `hovered_idx` tints the hovered body row (skipped when it's also
/// the selected row). See
/// [`crate::primitives::data_table::native_surface_paint::paint`]'s doc
/// for the full visual contract this now shares with
/// `macos::data_table::draw_data_table`.
#[cfg(target_os = "windows")]
pub fn draw_data_table(
    target: &ID2D1RenderTarget,
    dwrite: &super::text::DWrite,
    rect: Rect,
    table: &DataTable,
    line_height: f32,
    hovered_idx: Option<usize>,
) -> DataTableLayout {
    let theme = Theme::default();
    let mut surface = super::surface::D2dSurface {
        target,
        dwrite: Some(dwrite),
    };
    crate::primitives::data_table::native_surface_paint::paint(
        table,
        &mut surface,
        &theme,
        rect,
        line_height,
        hovered_idx,
    )
}

// Tests paint through a real `DWrite`/`HeadlessSurface`.
#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::*;
    use crate::primitives::data_table::{Column, ColumnAlign, ColumnWidth, DataRow, DataTableHit};
    use crate::types::{Decoration, StyledText, WidgetId};
    use crate::win::testing::HeadlessSurface;
    use crate::win::text::DWrite;

    const W: f32 = 300.0;
    const H: f32 = 100.0;
    const LINE_HEIGHT: f32 = 16.0;

    fn table(rows: Vec<DataRow>) -> DataTable {
        DataTable {
            id: WidgetId::new("table"),
            columns: vec![
                Column {
                    title: "Name".into(),
                    width: ColumnWidth::Flex(1.0),
                    align: ColumnAlign::Left,
                },
                Column {
                    title: "Size".into(),
                    width: ColumnWidth::Flex(1.0),
                    align: ColumnAlign::Right,
                },
            ],
            rows,
            selected_idx: Some(0),
            scroll_offset: 0,
            sort: None,
            has_focus: true,
            show_scrollbar: false,
            min_total_width: None,
            h_scroll: 0.0,
            column_overrides: Vec::new(),
            footer: None,
        }
    }

    fn row(name: &str, size: &str) -> DataRow {
        DataRow {
            cells: vec![
                StyledText::plain(name.to_string()),
                StyledText::plain(size.to_string()),
            ],
            decoration: Decoration::Normal,
        }
    }

    /// Paint↔click round trip: the selected row's blended background
    /// must be painted at its own bounds, and clicking each row/header
    /// resolves to the expected `DataTableHit`.
    #[test]
    fn paint_and_hit_test_round_trip() {
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let table = table(vec![
            row("a.txt", "1kb"),
            row("b.txt", "2kb"),
            row("c.txt", "3kb"),
        ]);
        let rect = Rect::new(0.0, 0.0, W, H);

        let layout = surface
            .paint(|target| {
                draw_data_table(target, &dwrite, rect, &table, LINE_HEIGHT, None);
            })
            .map(|_| win_data_table_layout(&dwrite, rect, &table, LINE_HEIGHT))
            .expect("paint data table");

        // Header click.
        let hit = layout.hit_test(1.0, 1.0, table.scroll_offset, table.rows.len());
        assert_eq!(hit, DataTableHit::Header { col: 0 });

        // Row click (row 1, below the header).
        let row_y = layout.header_height + line_height_mid(LINE_HEIGHT);
        let hit = layout.hit_test(1.0, row_y, table.scroll_offset, table.rows.len());
        assert_eq!(hit, DataTableHit::Row { idx: 0 });

        // Selected row (idx 0) painted a blended background distinct
        // from the plain body background.
        let theme = Theme::default();
        let sample_y = (layout.header_height + 2.0) as u32;
        let px = surface.pixel_at(2, sample_y);
        assert_ne!(
            (px.r, px.g, px.b),
            (theme.background.r, theme.background.g, theme.background.b),
            "selected row should paint a tinted background distinct from the plain body bg"
        );
    }

    fn line_height_mid(h: f32) -> f32 {
        h / 2.0
    }

    /// Scroll-offset round trip: with `scroll_offset = 1`, a click on
    /// the first body row resolves to row index 1.
    #[test]
    fn scroll_offset_hit_test_agrees() {
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let mut t = table(vec![row("a", "1"), row("b", "2"), row("c", "3")]);
        t.scroll_offset = 1;
        let rect = Rect::new(0.0, 0.0, W, H);
        let layout = win_data_table_layout(&dwrite, rect, &t, LINE_HEIGHT);
        let row_y = layout.header_height + line_height_mid(LINE_HEIGHT);
        let hit = layout.hit_test(1.0, row_y, t.scroll_offset, t.rows.len());
        assert_eq!(hit, DataTableHit::Row { idx: 1 });
    }

    /// No-paint layout must agree byte-for-byte with what
    /// `draw_data_table` painted.
    #[test]
    fn no_paint_layout_matches_paint_layout() {
        let table = table(vec![row("a.txt", "1kb"), row("b.txt", "2kb")]);
        let rect = Rect::new(0.0, 0.0, W, H);
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");

        let painted = surface
            .paint(|target| {
                draw_data_table(target, &dwrite, rect, &table, LINE_HEIGHT, None);
            })
            .map(|_| win_data_table_layout(&dwrite, rect, &table, LINE_HEIGHT))
            .expect("paint");
        let no_paint = win_data_table_layout(&dwrite, rect, &table, LINE_HEIGHT);
        assert_eq!(painted, no_paint);
    }

    /// Footer spans' foreground colours are respected, not discarded
    /// in favour of the theme's default foreground.
    #[test]
    fn footer_span_colour_overrides_the_default_foreground() {
        use crate::types::{Color, StyledSpan};

        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let mut t = table(vec![row("a.txt", "1kb")]);
        t.footer = Some(DataRow {
            cells: vec![
                StyledText {
                    spans: vec![StyledSpan {
                        text: "Total".into(),
                        fg: Some(Color::rgb(255, 0, 0)),
                        bg: None,
                        bold: false,
                        italic: false,
                        underline: false,
                    }],
                },
                StyledText::plain("3"),
            ],
            decoration: Decoration::Normal,
        });
        let rect = Rect::new(0.0, 0.0, W, H);
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
        surface
            .fill_rect(rect, Color::rgb(255, 255, 255))
            .expect("fill white bg");
        surface
            .paint(|target| {
                draw_data_table(target, &dwrite, rect, &t, LINE_HEIGHT, None);
            })
            .expect("paint data table");

        let layout = win_data_table_layout(&dwrite, rect, &t, LINE_HEIGHT);
        let footer_top = (H - layout.footer_height).max(0.0) as u32;

        let mut found_red = false;
        for y in footer_top..(H as u32) {
            for x in 0..(W as u32) {
                let px = surface.pixel_at(x, y);
                if px.r > 200 && px.g < 80 && px.b < 80 {
                    found_red = true;
                }
            }
        }
        assert!(
            found_red,
            "footer 'Total' span should paint in its own red fg, not \
             the pre-#1084 whole-cell `theme.foreground`"
        );
    }
}

/// Multi-select click wiring over [`crate::win::testing::WinDriver`].
///
/// `DataTable` stays purely declarative (no selection-set field of its
/// own — see `crate::types::apply_selection_gesture`'s doc), so the app
/// owns the selected-index set and the gesture anchor, resolves each
/// `MouseDown` to a row via
/// [`crate::primitives::data_table::DataTableLayout::hit_test`], derives
/// the gesture from the event's `Modifiers`, and emits
/// `DataTableEvent::SelectionSet` with the resulting indices. Mirrors
/// `gtk::data_table`'s `multi_select_driver_tests`, except `WinDriver` has
/// no `set_double_click_folding` toggle, so consecutive clicks that must
/// stay independent (rather than fold into a `DoubleClick`) are spaced
/// more than `WIN_DOUBLE_CLICK_RADIUS` DIPs apart on the x axis — row hit
/// testing only depends on y, so this keeps each click landing on its
/// intended row without tripping the shared `DoubleClickDetector`.
#[cfg(all(test, target_os = "windows"))]
mod multi_select_driver_tests {
    use crate::primitives::data_table::ColumnMeasure;
    use crate::runner::{AppLogic, Reaction};
    use crate::win::testing::WinDriver;
    use crate::{
        apply_selection_gesture, Backend, Column, DataRow, DataTable, DataTableEvent, DataTableHit,
        Decoration, Modifiers, MouseButton, Point, Rect, SelectionGesture, StyledText, WidgetId,
    };

    const VIEWPORT_W: f32 = 40.0;
    const VIEWPORT_H: f32 = 10.0;
    const ROW_H: f32 = 1.0;
    const HEADER_H: f32 = 1.0;
    const SCROLLBAR_W: f32 = 1.0;

    fn row(label: &str) -> DataRow {
        DataRow {
            cells: vec![StyledText::plain(label.to_string())],
            decoration: Decoration::Normal,
        }
    }

    fn sample_table() -> DataTable {
        DataTable {
            id: WidgetId::new("dt"),
            columns: vec![Column {
                title: "Name".to_string(),
                width: Default::default(),
                align: Default::default(),
            }],
            rows: (0..5).map(|i| row(&format!("row {i}"))).collect(),
            selected_idx: None,
            scroll_offset: 0,
            sort: None,
            has_focus: true,
            show_scrollbar: false,
            min_total_width: None,
            h_scroll: 0.0,
            column_overrides: vec![],
            footer: None,
        }
    }

    struct TableApp {
        table: DataTable,
        selected: Vec<usize>,
        anchor: usize,
        events: Vec<DataTableEvent>,
    }

    impl TableApp {
        fn new() -> Self {
            Self {
                table: sample_table(),
                selected: Vec::new(),
                anchor: 0,
                events: Vec::new(),
            }
        }
    }

    impl AppLogic for TableApp {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            backend.draw_data_table(
                Rect::new(0.0, 0.0, VIEWPORT_W, VIEWPORT_H),
                &self.table,
                None,
            );
        }

        fn handle(&mut self, event: crate::UiEvent, _backend: &mut dyn Backend) -> Reaction {
            let crate::UiEvent::MouseDown {
                position,
                modifiers,
                ..
            } = event
            else {
                return Reaction::Continue;
            };
            let layout =
                self.table
                    .layout(VIEWPORT_W, VIEWPORT_H, ROW_H, HEADER_H, SCROLLBAR_W, |_| {
                        ColumnMeasure::new(20.0)
                    });
            let hit = layout.hit_test(
                position.x,
                position.y,
                self.table.scroll_offset,
                self.table.rows.len(),
            );
            let idx = match hit {
                DataTableHit::Row { idx } => idx,
                _ => return Reaction::Continue,
            };
            let gesture = SelectionGesture::from_modifiers(modifiers);
            let indices = apply_selection_gesture(&self.selected, self.anchor, idx, gesture);
            self.selected = indices.clone();
            if !matches!(gesture, SelectionGesture::Range) {
                self.anchor = idx;
            }
            self.events.push(DataTableEvent::SelectionSet { indices });
            Reaction::Redraw
        }
    }

    fn mouse_down(x: f32, y: f32, modifiers: Modifiers) -> crate::UiEvent {
        crate::UiEvent::MouseDown {
            widget: None,
            button: MouseButton::Left,
            position: Point::new(x, y),
            modifiers,
        }
    }

    #[test]
    fn plain_click_replaces_selection() {
        let mut driver = WinDriver::new(TableApp::new(), 80, 40);
        driver.dispatch(mouse_down(
            2.0,
            HEADER_H + 2.0 * ROW_H,
            Modifiers::default(),
        ));
        assert_eq!(driver.app().selected, vec![2]);
    }

    #[test]
    fn ctrl_click_toggles_rows_into_a_multi_selection() {
        let mut driver = WinDriver::new(TableApp::new(), 80, 40);
        driver.dispatch(mouse_down(2.0, HEADER_H, Modifiers::default()));
        let ctrl = Modifiers {
            ctrl: true,
            ..Default::default()
        };
        // x is spaced > WIN_DOUBLE_CLICK_RADIUS DIPs from the previous
        // click so the shared `DoubleClickDetector` doesn't fold these
        // back-to-back single clicks into a `DoubleClick`; y alone picks
        // the row.
        driver.dispatch(mouse_down(10.0, HEADER_H + 2.0 * ROW_H, ctrl));
        assert_eq!(driver.app().selected, vec![0, 2]);
        // Ctrl-click the same row again toggles it back off.
        driver.dispatch(mouse_down(18.0, HEADER_H + 2.0 * ROW_H, ctrl));
        assert_eq!(driver.app().selected, vec![0]);
        assert_eq!(
            driver.app().events.last(),
            Some(&DataTableEvent::SelectionSet { indices: vec![0] })
        );
    }

    #[test]
    fn shift_click_selects_the_range_from_the_anchor() {
        let mut driver = WinDriver::new(TableApp::new(), 80, 40);
        driver.dispatch(mouse_down(2.0, HEADER_H, Modifiers::default()));
        let shift = Modifiers {
            shift: true,
            ..Default::default()
        };
        driver.dispatch(mouse_down(10.0, HEADER_H + 3.0 * ROW_H, shift));
        assert_eq!(driver.app().selected, vec![0, 1, 2, 3]);
        assert_eq!(
            driver.app().events.last(),
            Some(&DataTableEvent::SelectionSet {
                indices: vec![0, 1, 2, 3]
            })
        );
    }

    #[test]
    fn cmd_click_toggles_the_same_as_ctrl_click() {
        let mut driver = WinDriver::new(TableApp::new(), 80, 40);
        driver.dispatch(mouse_down(2.0, HEADER_H + ROW_H, Modifiers::default()));
        driver.dispatch(mouse_down(
            10.0,
            HEADER_H + 4.0 * ROW_H,
            Modifiers {
                cmd: true,
                ..Default::default()
            },
        ));
        assert_eq!(driver.app().selected, vec![1, 4]);
    }
}
