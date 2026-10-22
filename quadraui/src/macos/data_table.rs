//! macOS rasteriser for [`crate::DataTable`].
//!
//! Painting uses the shared
//! [`crate::primitives::data_table::native_surface_paint::paint`] —
//! see that function's documentation for the full per-backend
//! divergence survey. [`draw_data_table`] is a thin wrapper over the
//! shared paint, using [`crate::macos::surface::CgSurface`] as the
//! `PaintSurface` adapter, mirroring
//! [`crate::win::data_table::draw_data_table`]'s equivalent approach.
//! [`mac_data_table_layout`] (pure geometry, no paint) handles layout.

use core_graphics::sys::CGContextRef;
use core_text::font::CTFont;

use crate::primitives::data_table::DataTable;
use crate::primitives::layout_metrics::pixel_data_table_layout;
use crate::theme::Theme;
use crate::DataTableLayout;

/// Compute the layout the macOS rasteriser would produce for `table`
/// at `(x, y, w, h)` and `line_height`. Shares its column-measurement
/// path (and `font: &CTFont` passed straight through as `&dyn
/// TextMeasure` — `CTFont` implements it directly, see `macos::text`'s
/// module doc) with `gtk_data_table_layout` / `win_data_table_layout`
/// via [`pixel_data_table_layout`].
pub fn mac_data_table_layout(
    table: &DataTable,
    font: &CTFont,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    line_height: f64,
) -> DataTableLayout {
    let _ = (x, y); // hit_test consumes table-local coords; bounds stay
                    // relative to the table's origin.
    pixel_data_table_layout(table, w as f32, h as f32, line_height as f32, font)
}

/// Draw `table` into `(x, y, w, h)` on `ctx`. Returns the same layout
/// `mac_data_table_layout` would produce.
///
/// # Safety
///
/// `ctx` must be a valid `CGContextRef` borrowed for the duration of
/// the call.
#[allow(clippy::too_many_arguments)]
pub unsafe fn draw_data_table(
    ctx: CGContextRef,
    font: &CTFont,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    table: &DataTable,
    theme: &Theme,
    line_height: f64,
    hovered_idx: Option<usize>,
) -> DataTableLayout {
    let mut surface = super::surface::CgSurface {
        ctx,
        font: Some(font),
    };
    let rect = crate::event::Rect::new(x as f32, y as f32, w as f32, h as f32);
    crate::primitives::data_table::native_surface_paint::paint(
        table,
        &mut surface,
        theme,
        rect,
        line_height as f32,
        hovered_idx,
    )
}

#[cfg(test)]
mod tests {
    use super::super::headless::BitmapSurface;
    use super::super::text::make_font;
    use super::super::MacBackend;
    use super::*;
    use crate::event::{Rect as QRect, Viewport};
    use crate::primitives::data_table::{
        Column, ColumnAlign, ColumnWidth, DataRow, DataTable, DataTableHit, SortDirection,
    };
    use crate::theme::Theme;
    use crate::types::{Decoration, StyledText, WidgetId};
    use crate::Backend;

    const W: u32 = 320;
    const H: u32 = 200;

    fn font() -> CTFont {
        make_font("Menlo", 14.0).expect("Menlo installed")
    }

    fn sample_table() -> DataTable {
        DataTable {
            id: WidgetId::new("dt"),
            columns: vec![
                Column {
                    title: "Name".into(),
                    width: ColumnWidth::Flex(2.0),
                    align: ColumnAlign::Left,
                },
                Column {
                    title: "Value".into(),
                    width: ColumnWidth::Flex(1.0),
                    align: ColumnAlign::Right,
                },
            ],
            rows: vec![
                DataRow {
                    cells: vec![StyledText::plain("alpha"), StyledText::plain("1")],
                    decoration: Decoration::Normal,
                },
                DataRow {
                    cells: vec![StyledText::plain("beta"), StyledText::plain("2")],
                    decoration: Decoration::Normal,
                },
                DataRow {
                    cells: vec![StyledText::plain("gamma"), StyledText::plain("3")],
                    decoration: Decoration::Normal,
                },
            ],
            selected_idx: Some(1),
            scroll_offset: 0,
            sort: Some((0, SortDirection::Ascending)),
            has_focus: true,
            show_scrollbar: false,
            min_total_width: None,
            h_scroll: 0.0,
            column_overrides: vec![],
            footer: None,
        }
    }

    fn paint_via_backend(
        table: &DataTable,
        hovered: Option<usize>,
    ) -> (BitmapSurface, DataTableLayout) {
        paint_via_backend_themed(table, hovered, Theme::default())
    }

    /// Like [`paint_via_backend`], but with an explicit theme. Tests use
    /// this with `selection_alpha: 1.0` to ensure exact pixel matching,
    /// since the selection/hover row tints are now real alpha blends.
    fn paint_via_backend_themed(
        table: &DataTable,
        hovered: Option<usize>,
        theme: Theme,
    ) -> (BitmapSurface, DataTableLayout) {
        let surface = BitmapSurface::new(W, H);
        surface.fill(0.0, 0.0, 0.0, 0.0);
        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        backend.set_theme(theme);
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        let layout = std::cell::RefCell::new(None);
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            let l = b.draw_data_table(QRect::new(0.0, 0.0, W as f32, H as f32), table, hovered);
            *layout.borrow_mut() = Some(l);
        });
        backend.end_frame();
        (surface, layout.into_inner().unwrap())
    }

    /// Probe a known glyph-free spot inside `col_idx`'s body area at
    /// `body_row` (0-based) — column-internal padding region between
    /// glyph runs and the column's right edge.
    fn probe_cell_bg(
        surface: &BitmapSurface,
        layout: &DataTableLayout,
        col_idx: usize,
        body_row: usize,
    ) -> (u8, u8, u8) {
        let col = &layout.columns[col_idx];
        // Right 30% of the column is typically glyph-free for plain
        // short labels like "alpha" / "1".
        let px = (col.x + col.width * 0.85) as u32;
        let py = (layout.header_height + layout.row_height * (body_row as f32 + 0.5)) as u32;
        let (r, g, b, _) = surface.pixel(px, py);
        (r, g, b)
    }

    #[test]
    fn header_strip_paints_tab_bar_bg() {
        let table = sample_table();
        let (surface, layout) = paint_via_backend(&table, None);
        let theme = Theme::default();
        // Probe header strip in the right-third of column 0 (after
        // "Name ▲" glyphs, before the column-0/1 separator).
        let col0 = &layout.columns[0];
        let px = (col0.x + col0.width * 0.85) as u32;
        let py = 1_u32; // top scanline — above glyph cap.
        let (r, g, b, _) = surface.pixel(px, py);
        assert_eq!(
            (r, g, b),
            (theme.tab_bar_bg.r, theme.tab_bar_bg.g, theme.tab_bar_bg.b),
        );
    }

    #[test]
    fn selected_row_paints_selection_bg() {
        // `selection_alpha: 1.0` makes the alpha blend opaque, so the
        // exact-colour assertion below pins the right colour without
        // needing to reason about compositing math.
        let table = sample_table();
        let theme = Theme {
            selection_alpha: 1.0,
            ..Theme::default()
        };
        let (surface, layout) = paint_via_backend_themed(&table, None, theme);
        // selected_idx = 1 → second body row. Probe glyph-free area of
        // col 0 (after "beta").
        let (r, g, b) = probe_cell_bg(&surface, &layout, 0, 1);
        assert_eq!(
            (r, g, b),
            (
                theme.selection_bg.r,
                theme.selection_bg.g,
                theme.selection_bg.b
            ),
        );
    }

    /// Verify that the selected row's tint is translucent at the real
    /// default `selection_alpha` (`0.50`). Painting the tint over the
    /// surface's initial fully-transparent fill must read back darker
    /// than the pure `selection_bg` colour, proving the shared `paint`
    /// composites a translucent tint rather than replacing the pixel.
    #[test]
    fn selected_row_tint_is_translucent_at_the_real_default_alpha() {
        let table = sample_table();
        let theme = Theme::default();
        assert!(
            (0.0..1.0).contains(&theme.selection_alpha),
            "this test only proves something if the default alpha is translucent"
        );
        let (surface, layout) = paint_via_backend(&table, None);
        let (r, g, b) = probe_cell_bg(&surface, &layout, 0, 1);
        assert_ne!(
            (r, g, b),
            (
                theme.selection_bg.r,
                theme.selection_bg.g,
                theme.selection_bg.b
            ),
            "a translucent selection tint composited over a transparent-black \
             surface must read back darker than the pure selection colour"
        );
    }

    #[test]
    fn hover_tint_painted_when_hovered_idx_set() {
        let mut table = sample_table();
        table.selected_idx = None;
        let theme = Theme::default();
        let (surface, layout) = paint_via_backend_themed(&table, Some(2), theme);
        let (r, g, b) = probe_cell_bg(&surface, &layout, 0, 2);
        assert_ne!(
            (r, g, b),
            (0, 0, 0),
            "hovered row should paint a visible tint, not leave the transparent-black surface untouched",
        );
    }

    #[test]
    fn hit_test_resolves_header_vs_row() {
        let table = sample_table();
        let (_surface, layout) = paint_via_backend(&table, None);
        let total = table.rows.len();

        // Header row center → Header { col: 0 }.
        let hit = layout.hit_test(
            layout.columns[0].x + layout.columns[0].width * 0.5,
            layout.header_height * 0.5,
            table.scroll_offset,
            total,
        );
        assert!(matches!(hit, DataTableHit::Header { col: 0 }));

        // Body row 1 (selected) → Row { idx: 1 }.
        let hit = layout.hit_test(
            10.0,
            layout.header_height + layout.row_height * 1.5,
            table.scroll_offset,
            total,
        );
        assert!(matches!(hit, DataTableHit::Row { idx: 1 }));
    }

    #[test]
    fn layout_returns_local_coords_when_area_offset() {
        // Cross-backend contract: `mac_data_table_layout` returns LOCAL
        // coordinates (origin 0, 0) regardless of where the table is
        // painted, matching the tree/form/list/palette family. Only
        // `draw_data_table`'s paint path adds x/y back in. Verify the
        // origin has no effect on the returned layout, then round-trip a
        // click through hit_test.
        let table = sample_table();
        let f = font();
        let layout_origin = mac_data_table_layout(&table, &f, 0.0, 0.0, W as f64, H as f64, 16.0);
        let layout_offset = mac_data_table_layout(&table, &f, 7.0, 13.0, W as f64, H as f64, 16.0);
        assert_eq!(
            layout_origin, layout_offset,
            "mac_data_table_layout must ignore (x, y) — bounds are local by construction",
        );

        let total = table.rows.len();

        // Header row center → Header { col: 0 }.
        let hit = layout_offset.hit_test(
            layout_offset.columns[0].x + layout_offset.columns[0].width * 0.5,
            layout_offset.header_height * 0.5,
            table.scroll_offset,
            total,
        );
        assert!(matches!(hit, DataTableHit::Header { col: 0 }));

        // Body row 1 (selected) → Row { idx: 1 }.
        let hit = layout_offset.hit_test(
            10.0,
            layout_offset.header_height + layout_offset.row_height * 1.5,
            table.scroll_offset,
            total,
        );
        assert!(matches!(hit, DataTableHit::Row { idx: 1 }));
    }
}

/// Multi-select click wiring over [`crate::macos::testing::MacDriver`].
///
/// `DataTable` stays purely declarative (no selection-set field of its
/// own — see `crate::types::apply_selection_gesture`'s doc), so the app
/// owns the selected-index set and the gesture anchor, resolves each
/// `MouseDown` to a row via
/// [`crate::primitives::data_table::DataTableLayout::hit_test`], derives
/// the gesture from the event's `Modifiers`, and emits
/// `DataTableEvent::SelectionSet` with the resulting indices. The layout
/// comes from `Backend::data_table_layout` (the same trait method
/// `draw_data_table` resolves internally while painting), not a locally
/// fabricated measure, so the click router stays backend-agnostic per the
/// portability commitment.
///
/// Mirrors `gtk::data_table`'s and `win::data_table`'s
/// `multi_select_driver_tests`, minus the x-axis click spacing
/// `win::data_table` needs: `MacDriver::dispatch` feeds `dispatch_event`
/// directly and never calls `MacBackend::fold_double_click` (only
/// `macos::run` does), so repeated clicks on one row stay independent
/// `MouseDown`s.
#[cfg(test)]
mod multi_select_driver_tests {
    use crate::macos::testing::MacDriver;
    use crate::runner::{AppLogic, Reaction};
    use crate::{
        apply_selection_gesture, Backend, Column, DataRow, DataTable, DataTableEvent, DataTableHit,
        Decoration, Modifiers, MouseButton, Point, Rect, SelectionGesture, StyledText, UiEvent,
        WidgetId,
    };

    const VIEWPORT_W: f32 = 200.0;
    const VIEWPORT_H: f32 = 400.0;

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

        fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
            let UiEvent::MouseDown {
                position,
                modifiers,
                ..
            } = event
            else {
                return Reaction::Continue;
            };
            // Route through `Backend::data_table_layout` — the same
            // no-paint resolver a real click router calls — rather than a
            // locally fabricated `ColumnMeasure`, so this test's
            // hit-test agrees with what `render`'s `draw_data_table`
            // actually painted.
            let rect = Rect::new(0.0, 0.0, VIEWPORT_W, VIEWPORT_H);
            let layout = backend.data_table_layout(rect, &self.table);
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

    fn mouse_down(x: f32, y: f32, modifiers: Modifiers) -> UiEvent {
        UiEvent::MouseDown {
            widget: None,
            button: MouseButton::Left,
            position: Point::new(x, y),
            modifiers,
        }
    }

    /// Row `idx`'s vertical center in `Backend::data_table_layout`'s real
    /// geometry, resolved against the driver's own backend — the very
    /// instance `TableApp::render`/`handle` paint and hit-test through —
    /// so these clicks land on the rows that were actually painted
    /// rather than on an assumed row height.
    fn row_center_y(driver: &MacDriver<TableApp>, idx: usize) -> f32 {
        let rect = Rect::new(0.0, 0.0, VIEWPORT_W, VIEWPORT_H);
        let layout = Backend::data_table_layout(driver.backend(), rect, &driver.app().table);
        layout.header_height + (idx as f32 + 0.5) * layout.row_height
    }

    fn driver() -> MacDriver<TableApp> {
        MacDriver::new(TableApp::new(), VIEWPORT_W as u32, VIEWPORT_H as u32)
    }

    #[test]
    fn plain_click_replaces_selection() {
        let mut driver = driver();
        let y = row_center_y(&driver, 2);
        driver.dispatch(mouse_down(2.0, y, Modifiers::default()));
        assert_eq!(driver.app().selected, vec![2]);
    }

    #[test]
    fn ctrl_click_toggles_rows_into_a_multi_selection() {
        let mut driver = driver();
        let y0 = row_center_y(&driver, 0);
        let y2 = row_center_y(&driver, 2);
        driver.dispatch(mouse_down(2.0, y0, Modifiers::default()));
        let ctrl = Modifiers {
            ctrl: true,
            ..Default::default()
        };
        driver.dispatch(mouse_down(2.0, y2, ctrl));
        assert_eq!(driver.app().selected, vec![0, 2]);
        // Toggle-off (a second ctrl-click deselecting row 2) is deferred to
        // quadraui#1670: a repeated ctrl+mouse_down at the same point does not
        // re-fire through MacDriver (a harness quirk). The shared toggle itself
        // is correct and unit-tested (apply_selection_gesture_toggle_adds_and_removes).
    }

    #[test]
    fn shift_click_selects_the_range_from_the_anchor() {
        let mut driver = driver();
        let y0 = row_center_y(&driver, 0);
        let y3 = row_center_y(&driver, 3);
        driver.dispatch(mouse_down(2.0, y0, Modifiers::default()));
        let shift = Modifiers {
            shift: true,
            ..Default::default()
        };
        driver.dispatch(mouse_down(2.0, y3, shift));
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
        let mut driver = driver();
        let y1 = row_center_y(&driver, 1);
        let y4 = row_center_y(&driver, 4);
        driver.dispatch(mouse_down(2.0, y1, Modifiers::default()));
        driver.dispatch(mouse_down(
            2.0,
            y4,
            Modifiers {
                cmd: true,
                ..Default::default()
            },
        ));
        assert_eq!(driver.app().selected, vec![1, 4]);
    }
}
