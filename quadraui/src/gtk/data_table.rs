//! GTK click wiring for [`crate::DataTable`].
//!
//! Painting goes through [`crate::primitives::data_table::native_surface_paint`]
//! — the same shared pixel painter macOS/Win-GUI's `TreeTable` forwarding and
//! `Primitive::paint` already use — via
//! [`crate::gtk::GtkBackend::with_chrome_surface`]
//! (see [`crate::gtk::GtkBackend::draw_data_table_with_glyphs`] /
//! [`crate::gtk::GtkBackend::draw_data_table_with_state`]). GTK has no
//! bespoke Cairo+Pango `DataTable` rasteriser of its own left; this module
//! now holds only the multi-select click-routing coverage below.

/// Multi-select click wiring over [`crate::gtk::testing::GtkDriver`].
///
/// `DataTable` stays purely declarative (no selection-set field of its
/// own — see `crate::types::apply_selection_gesture`'s doc), so the app
/// owns the selected-index set and the gesture anchor, resolves each
/// `MouseDown` to a row via [`crate::primitives::data_table::DataTableLayout::hit_test`],
/// derives the gesture from the event's `Modifiers`, and emits
/// `DataTableEvent::SelectionSet` with the resulting indices. Mirrors
/// [`super::list::multi_select_driver_tests`] and
/// [`super::tree::multi_select_driver_tests`]'s shape.
#[cfg(test)]
mod multi_select_driver_tests {
    use crate::gtk::testing::GtkDriver;
    use crate::primitives::data_table::ColumnMeasure;
    use crate::runner::{AppLogic, Reaction};
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
            bordered: false,
            striped: false,
            glyphs: None,
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

        #[allow(deprecated)]
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
        let mut driver = GtkDriver::new(TableApp::new(), 80, 40);
        driver.dispatch(mouse_down(
            2.0,
            HEADER_H + 2.0 * ROW_H,
            Modifiers::default(),
        ));
        assert_eq!(driver.app().selected, vec![2]);
    }

    #[test]
    fn ctrl_click_toggles_rows_into_a_multi_selection() {
        let mut driver = GtkDriver::new(TableApp::new(), 80, 40);
        driver.set_double_click_folding(false);
        driver.dispatch(mouse_down(2.0, HEADER_H, Modifiers::default()));
        let ctrl = Modifiers {
            ctrl: true,
            ..Default::default()
        };
        driver.dispatch(mouse_down(2.0, HEADER_H + 2.0 * ROW_H, ctrl));
        assert_eq!(driver.app().selected, vec![0, 2]);
        driver.dispatch(mouse_down(2.0, HEADER_H + 2.0 * ROW_H, ctrl));
        assert_eq!(driver.app().selected, vec![0]);
        assert_eq!(
            driver.app().events.last(),
            Some(&DataTableEvent::SelectionSet { indices: vec![0] })
        );
    }

    #[test]
    fn shift_click_selects_the_range_from_the_anchor() {
        let mut driver = GtkDriver::new(TableApp::new(), 80, 40);
        driver.set_double_click_folding(false);
        driver.dispatch(mouse_down(2.0, HEADER_H, Modifiers::default()));
        let shift = Modifiers {
            shift: true,
            ..Default::default()
        };
        driver.dispatch(mouse_down(2.0, HEADER_H + 3.0 * ROW_H, shift));
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
        let mut driver = GtkDriver::new(TableApp::new(), 80, 40);
        driver.set_double_click_folding(false);
        driver.dispatch(mouse_down(2.0, HEADER_H + ROW_H, Modifiers::default()));
        driver.dispatch(mouse_down(
            2.0,
            HEADER_H + 4.0 * ROW_H,
            Modifiers {
                cmd: true,
                ..Default::default()
            },
        ));
        assert_eq!(driver.app().selected, vec![1, 4]);
    }
}
