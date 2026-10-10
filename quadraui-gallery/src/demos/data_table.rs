//! `DataTable` demo — adapted from
//! `quadraui/examples/common/data_table_app.rs` and
//! `dialog_table_demo.rs`.
//!
//! Two variants: a sortable, selectable pod list (click a header to
//! sort, click a row to select, `f` toggles the pinned footer), and a
//! static two-column keybindings reference (no sort, no selection) —
//! the same [`DataTable`] primitive at both ends of the interactivity
//! spectrum.

use quadraui::{
    Backend, BackendCaps, Color, Column, ColumnAlign, ColumnWidth, DataRow, DataTable,
    DataTableHit, InteractionState, Key, Reaction, Rect, SortDirection, StatusBar,
    StatusBarSegment, StyledText, UiEvent, WidgetId,
};

use crate::demo::{extract_region, Demo};

const SOURCE: &str = include_str!("data_table.rs");

// gallery:begin
pub struct DataTableDemo {
    columns: Vec<Column>,
    selected: Option<usize>,
    sort_col: Option<usize>,
    sort_asc: bool,
    show_footer: bool,
    hovered_idx: Option<usize>,
}

impl DataTableDemo {
    pub fn new() -> Self {
        Self {
            columns: vec![
                Column {
                    title: "Name".into(),
                    width: ColumnWidth::Flex(3.0),
                    align: ColumnAlign::Left,
                },
                Column {
                    title: "Status".into(),
                    width: ColumnWidth::Flex(1.5),
                    align: ColumnAlign::Left,
                },
                Column {
                    title: "Age".into(),
                    width: ColumnWidth::Flex(0.5),
                    align: ColumnAlign::Right,
                },
                Column {
                    title: "Restarts".into(),
                    width: ColumnWidth::Fixed(10.0),
                    align: ColumnAlign::Right,
                },
            ],
            selected: Some(0),
            sort_col: Some(0),
            sort_asc: true,
            show_footer: true,
            hovered_idx: None,
        }
    }

    fn pod_rows() -> Vec<DataRow> {
        let pods = [
            ("nginx-7d9b8c66b-x2j4k", "Running", "3d", "0"),
            ("redis-master-0", "Running", "5d", "1"),
            ("api-gateway-5f6c8d-9mn2q", "Running", "1d", "0"),
            ("worker-batch-7b9c4-kl3m8", "Pending", "2m", "0"),
            ("grafana-5f4c8d-mn2q7", "CrashLoopBackOff", "1h", "14"),
            ("prometheus-server-0", "Running", "3d", "0"),
        ];
        pods.iter()
            .map(|(name, status, age, restarts)| DataRow {
                cells: vec![
                    StyledText::plain(*name),
                    if status.starts_with("Running") {
                        StyledText::colored(*status, Color::rgb(80, 200, 80))
                    } else if status.starts_with("CrashLoopBackOff") {
                        StyledText::colored(*status, Color::rgb(220, 60, 60))
                    } else {
                        StyledText::colored(*status, Color::rgb(220, 180, 50))
                    },
                    StyledText::plain(*age),
                    StyledText::plain(*restarts),
                ],
                decoration: Default::default(),
            })
            .collect()
    }

    fn keybinding_rows() -> Vec<DataRow> {
        [
            ("Ctrl+Enter", "Stage hunk"),
            ("Ctrl+Shift+Enter", "Stage file"),
            ("Ctrl+Z", "Revert hunk"),
            ("[", "Previous change"),
            ("]", "Next change"),
            ("Esc", "Close"),
        ]
        .iter()
        .map(|(key, action)| DataRow {
            cells: vec![StyledText::plain(*key), StyledText::plain(*action)],
            decoration: Default::default(),
        })
        .collect()
    }

    fn footer_row(&self) -> DataRow {
        let rows = Self::pod_rows();
        let total_restarts: u32 = rows
            .iter()
            .filter_map(|r| r.cells.get(3))
            .filter_map(|c| {
                let text: String = c.spans.iter().map(|s| s.text.as_str()).collect();
                text.parse::<u32>().ok()
            })
            .sum();
        DataRow {
            cells: vec![
                StyledText::plain(format!("{} pods", rows.len())),
                StyledText::plain(""),
                StyledText::plain(""),
                StyledText::colored(total_restarts.to_string(), Color::rgb(220, 180, 50)),
            ],
            decoration: Default::default(),
        }
    }

    fn table(&self, variant: usize) -> DataTable {
        if variant == 0 {
            let mut rows = Self::pod_rows();
            if let Some(col) = self.sort_col {
                rows.sort_by(|a, b| {
                    let a_text: String = a
                        .cells
                        .get(col)
                        .map(|c| c.spans.iter().map(|s| s.text.as_str()).collect())
                        .unwrap_or_default();
                    let b_text: String = b
                        .cells
                        .get(col)
                        .map(|c| c.spans.iter().map(|s| s.text.as_str()).collect())
                        .unwrap_or_default();
                    let cmp = a_text.cmp(&b_text);
                    if self.sort_asc {
                        cmp
                    } else {
                        cmp.reverse()
                    }
                });
            }
            DataTable {
                id: WidgetId::new("gallery:data-table:pods"),
                columns: self.columns.clone(),
                rows,
                selected_idx: self.selected,
                scroll_offset: 0,
                sort: self.sort_col.map(|c| {
                    (
                        c,
                        if self.sort_asc {
                            SortDirection::Ascending
                        } else {
                            SortDirection::Descending
                        },
                    )
                }),
                has_focus: true,
                show_scrollbar: true,
                min_total_width: None,
                h_scroll: 0.0,
                column_overrides: Vec::new(),
                footer: if self.show_footer {
                    Some(self.footer_row())
                } else {
                    None
                },
            }
        } else {
            DataTable {
                id: WidgetId::new("gallery:data-table:keybindings"),
                columns: vec![
                    Column {
                        title: "Key".into(),
                        width: ColumnWidth::Flex(1.0),
                        align: ColumnAlign::Left,
                    },
                    Column {
                        title: "Action".into(),
                        width: ColumnWidth::Flex(2.0),
                        align: ColumnAlign::Left,
                    },
                ],
                rows: Self::keybinding_rows(),
                selected_idx: None,
                scroll_offset: 0,
                sort: None,
                has_focus: false,
                show_scrollbar: false,
                min_total_width: None,
                h_scroll: 0.0,
                column_overrides: Vec::new(),
                footer: None,
            }
        }
    }

    fn status(&self, variant: usize) -> StatusBar {
        let text = if variant == 0 {
            let sort_text = match self.sort_col {
                Some(c) => format!(
                    "sort: {} {}",
                    self.columns[c].title,
                    if self.sort_asc { "asc" } else { "desc" }
                ),
                None => "sort: none".into(),
            };
            format!(" {sort_text} — click header to sort, f=footer ")
        } else {
            " static reference table — no sort, no selection ".into()
        };
        StatusBar {
            id: WidgetId::new("gallery:data-table:status"),
            left_segments: vec![StatusBarSegment {
                text,
                fg: Color::rgb(220, 220, 220),
                bg: Color::rgb(40, 40, 60),
                bold: false,
                action_id: None,
            }],
            right_segments: vec![],
        }
    }

    fn table_rect(area: Rect, backend: &dyn Backend) -> Rect {
        let lh = backend.line_height();
        Rect::new(area.x, area.y, area.width, (area.height - lh).max(0.0))
    }

    fn status_rect(area: Rect, backend: &dyn Backend) -> Rect {
        let lh = backend.line_height();
        Rect::new(area.x, area.y + area.height - lh, area.width, lh)
    }
}

impl Default for DataTableDemo {
    fn default() -> Self {
        Self::new()
    }
}

impl Demo for DataTableDemo {
    fn name(&self) -> &'static str {
        "Data Table"
    }

    fn group(&self) -> &'static str {
        "Data"
    }

    fn variants(&self) -> &'static [&'static str] {
        &["Pods (sortable)", "Keybindings (static)"]
    }

    fn render(&self, variant: usize, backend: &mut dyn Backend, area: Rect) {
        let table_rect = Self::table_rect(area, backend);
        let hovered = if variant == 0 { self.hovered_idx } else { None };
        let _ = backend.draw_data_table(table_rect, &self.table(variant), hovered);

        let status_rect = Self::status_rect(area, backend);
        let _ = backend.draw_status_bar_interactive(
            status_rect,
            &self.status(variant),
            &InteractionState::new(),
        );
    }

    fn handle(
        &mut self,
        variant: usize,
        event: &UiEvent,
        backend: &mut dyn Backend,
        area: Rect,
    ) -> Reaction {
        if variant != 0 {
            return Reaction::Continue;
        }
        let table_rect = Self::table_rect(area, backend);
        let total = Self::pod_rows().len();
        match event {
            UiEvent::KeyPressed { key, .. } => match key {
                Key::Char('j') => {
                    let cur = self.selected.unwrap_or(0);
                    if cur + 1 < total {
                        self.selected = Some(cur + 1);
                    }
                    Reaction::Redraw
                }
                Key::Char('k') => {
                    let cur = self.selected.unwrap_or(0);
                    self.selected = Some(cur.saturating_sub(1));
                    Reaction::Redraw
                }
                Key::Char('s') => {
                    self.sort_col = match self.sort_col {
                        None => Some(0),
                        Some(c) if c + 1 < self.columns.len() => Some(c + 1),
                        Some(_) => None,
                    };
                    Reaction::Redraw
                }
                Key::Char('d') => {
                    self.sort_asc = !self.sort_asc;
                    Reaction::Redraw
                }
                Key::Char('f') => {
                    self.show_footer = !self.show_footer;
                    Reaction::Redraw
                }
                _ => Reaction::Continue,
            },
            UiEvent::MouseDown { position, .. } => {
                let table = self.table(variant);
                let layout = backend.data_table_layout(table_rect, &table);
                // `DataTableLayout::hit_test` is viewport-relative (measured
                // from the table's own left/top edge), not absolute like
                // most other primitives' layouts — localize before testing.
                let (lx, ly) = (position.x - table_rect.x, position.y - table_rect.y);
                match layout.hit_test(lx, ly, 0, total) {
                    DataTableHit::Header { col } => {
                        if self.sort_col == Some(col) {
                            self.sort_asc = !self.sort_asc;
                        } else {
                            self.sort_col = Some(col);
                            self.sort_asc = true;
                        }
                        Reaction::Redraw
                    }
                    DataTableHit::Row { idx } => {
                        self.selected = Some(idx);
                        Reaction::Redraw
                    }
                    _ => Reaction::Continue,
                }
            }
            UiEvent::MouseMoved { position, .. } => {
                let table = self.table(variant);
                let layout = backend.data_table_layout(table_rect, &table);
                let (lx, ly) = (position.x - table_rect.x, position.y - table_rect.y);
                let old = self.hovered_idx;
                self.hovered_idx = match layout.hit_test(lx, ly, 0, total) {
                    DataTableHit::Row { idx } => Some(idx),
                    _ => None,
                };
                if self.hovered_idx != old {
                    Reaction::Redraw
                } else {
                    Reaction::Continue
                }
            }
            _ => Reaction::Continue,
        }
    }

    fn source(&self) -> &'static str {
        extract_region(SOURCE)
    }

    fn data(&self, variant: usize) -> serde_json::Value {
        serde_json::to_value(self.table(variant)).unwrap_or(serde_json::Value::Null)
    }

    fn caps_note(&self, _variant: usize, _caps: &BackendCaps) -> Option<String> {
        None
    }
}
// gallery:end

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_starts_sorted_by_name_with_the_footer_shown() {
        let demo = DataTableDemo::new();
        assert_eq!(demo.sort_col, Some(0));
        assert!(demo.show_footer);
    }

    #[test]
    fn keybindings_variant_has_no_selection_and_no_sort() {
        let demo = DataTableDemo::new();
        let table = demo.table(1);
        assert_eq!(table.selected_idx, None);
        assert_eq!(table.sort, None);
        assert!(!table.has_focus);
    }

    #[test]
    fn f_key_toggles_the_footer() {
        let mut demo = DataTableDemo::new();
        let mut backend = quadraui::testing::RecordingBackend::new();
        let area = Rect::new(0.0, 0.0, 80.0, 20.0);
        let event = UiEvent::KeyPressed {
            key: Key::Char('f'),
            modifiers: Default::default(),
            repeat: false,
        };
        let reaction = demo.handle(0, &event, &mut backend, area);
        assert!(matches!(reaction, Reaction::Redraw));
        assert!(!demo.show_footer);
    }
}

#[cfg(all(test, feature = "tui"))]
mod tui_tests {
    use super::*;

    #[test]
    fn clicking_the_status_header_at_a_nonzero_area_offset_sorts_by_it() {
        let mut demo = DataTableDemo::new();
        let mut backend = quadraui::tui::TuiBackend::new();
        // A non-zero origin, mirroring the real gallery shell's content
        // area (offset past the activity bar + sidebar).
        let area = Rect::new(20.0, 0.0, 60.0, 20.0);
        let table_rect = DataTableDemo::table_rect(area, &backend);
        let layout = backend.data_table_layout(table_rect, &demo.table(0));
        let col = 1; // "Status".
                     // Click well inside the column, not near either edge — the
                     // header-divider grab zone extends 3 cells either side of a
                     // boundary (`DIVIDER_GRAB_PX`), and a click within it resolves
                     // to `HeaderDivider`, not `Header`.
        let resolved = &layout.columns[col];
        let header_x = table_rect.x + resolved.x + resolved.width / 2.0;
        let header_y = table_rect.y;
        let event = UiEvent::MouseDown {
            widget: None,
            button: quadraui::MouseButton::Left,
            position: quadraui::Point::new(header_x, header_y),
            modifiers: Default::default(),
        };
        let reaction = demo.handle(0, &event, &mut backend, area);
        assert!(matches!(reaction, Reaction::Redraw));
        assert_eq!(
            demo.sort_col,
            Some(1),
            "clicking column 1's header should sort by it"
        );
    }
}
