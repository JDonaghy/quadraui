//! Smoke test for [`DataTable`]: a Kubernetes-style pod list.
//!
//! Keys:
//! - `j` / `↓` — select next row
//! - `k` / `↑` — select previous row
//! - `s` — cycle sort column (Name → Status → Age → Restarts → none)
//! - `d` — toggle sort direction
//! - `q` / `Esc` — quit

use quadraui::{
    AppLogic, Backend, Color, Column, ColumnAlign, ColumnWidth, DataRow, DataTable, DataTableEvent,
    DataTableHit, DataTableLayout, Key, NamedKey, Reaction, Rect, SortDirection, StatusBar,
    StatusBarSegment, StyledText, UiEvent, WidgetId,
};

pub struct DataTableApp {
    columns: Vec<Column>,
    selected: Option<usize>,
    scroll_offset: usize,
    sort_col: Option<usize>,
    sort_asc: bool,
    resize_col: Option<usize>,
}

impl DataTableApp {
    pub fn new() -> Self {
        Self {
            columns: Self::default_columns(),
            selected: Some(0),
            scroll_offset: 0,
            sort_col: Some(0),
            sort_asc: true,
            resize_col: None,
        }
    }

    fn default_columns() -> Vec<Column> {
        vec![
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
                width: ColumnWidth::Fixed(8.0),
                align: ColumnAlign::Right,
            },
            Column {
                title: "Restarts".into(),
                width: ColumnWidth::Fixed(10.0),
                align: ColumnAlign::Right,
            },
        ]
    }

    fn rows() -> Vec<DataRow> {
        let pods = [
            ("nginx-7d9b8c66b-x2j4k", "Running", "3d", "0"),
            ("redis-master-0", "Running", "5d", "1"),
            ("api-gateway-5f6c8d-9mn2q", "Running", "1d", "0"),
            ("postgres-0", "Running", "5d", "0"),
            ("worker-batch-7b9c4-kl3m8", "Pending", "2m", "0"),
            ("cert-manager-cainjector-6d4", "Running", "12d", "3"),
            ("coredns-5d78c9869d-abc12", "Running", "12d", "0"),
            ("etcd-controlplane", "Running", "12d", "0"),
            ("kube-apiserver-cp", "Running", "12d", "2"),
            ("kube-scheduler-cp", "Running", "12d", "0"),
            ("ingress-nginx-controller-xyz", "Running", "7d", "1"),
            ("metrics-server-6d94bc", "Running", "7d", "0"),
            ("fluentd-daemonset-abc", "Running", "7d", "0"),
            ("prometheus-server-0", "Running", "3d", "0"),
            ("grafana-5f4c8d-mn2q7", "CrashLoopBackOff", "1h", "14"),
            ("loki-0", "Running", "3d", "0"),
            ("argocd-server-6b8c9d-k3m8", "Running", "10d", "0"),
            ("vault-0", "Pending", "5m", "0"),
            ("consul-server-0", "Running", "10d", "1"),
            ("traefik-7d9b8c66b-x2j4k", "Running", "7d", "0"),
        ];
        pods.iter()
            .map(|(name, status, age, restarts)| DataRow {
                cells: vec![
                    StyledText::plain(*name),
                    if *status == "Running" {
                        StyledText::colored(*status, Color::rgb(80, 200, 80))
                    } else if *status == "CrashLoopBackOff" {
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

    fn build_table(&self) -> DataTable {
        let mut rows = Self::rows();
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
            id: WidgetId::new("pods"),
            columns: self.columns.clone(),
            rows,
            selected_idx: self.selected,
            scroll_offset: self.scroll_offset,
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
        }
    }

    fn status_bar(&self) -> StatusBar {
        let sort_text = match self.sort_col {
            Some(c) => {
                let dir = if self.sort_asc { "asc" } else { "desc" };
                let col_name = self.columns[c].title.clone();
                format!(" sort: {col_name} {dir} ")
            }
            None => " sort: none ".into(),
        };
        let sel_text = match self.selected {
            Some(i) => format!(" row {} / {} ", i + 1, Self::rows().len()),
            None => " no selection ".into(),
        };
        let fg = Color::rgb(220, 220, 220);
        let bg = Color::rgb(40, 40, 60);
        StatusBar {
            id: WidgetId::new("status"),
            left_segments: vec![StatusBarSegment {
                text: " k8s pods — DataTable smoke test ".into(),
                fg: Color::rgb(255, 255, 255),
                bg: Color::rgb(40, 80, 120),
                bold: true,
                action_id: None,
            }],
            right_segments: vec![
                StatusBarSegment {
                    text: sort_text,
                    fg,
                    bg,
                    bold: false,
                    action_id: None,
                },
                StatusBarSegment {
                    text: sel_text,
                    fg,
                    bg,
                    bold: false,
                    action_id: None,
                },
            ],
        }
    }

    fn visible_rows(&self, backend: &dyn Backend) -> usize {
        let vp = backend.viewport();
        let lh = backend.line_height();
        let bar_h = if lh > 1.5 { lh * 1.5 } else { lh };
        let table_h = vp.height - bar_h;
        let header_h = if lh > 1.5 { (lh * 1.2).round() } else { lh };
        let body_h = (table_h - header_h).max(0.0);
        if lh > 0.0 {
            (body_h / lh).floor() as usize
        } else {
            0
        }
    }

    fn table_layout(&self, backend: &dyn Backend) -> DataTableLayout {
        let vp = backend.viewport();
        let lh = backend.line_height();
        let bar_h = if lh > 1.5 { lh * 1.5 } else { lh };
        let table_rect = Rect::new(0.0, 0.0, vp.width, vp.height - bar_h);
        let table = self.build_table();
        backend.data_table_layout(table_rect, &table)
    }

    fn ensure_visible(&mut self, backend: &dyn Backend) {
        let vis = self.visible_rows(backend);
        if let Some(sel) = self.selected {
            if sel < self.scroll_offset {
                self.scroll_offset = sel;
            } else if vis > 0 && sel >= self.scroll_offset + vis {
                self.scroll_offset = sel + 1 - vis;
            }
        }
    }
}

impl Default for DataTableApp {
    fn default() -> Self {
        Self::new()
    }
}

impl AppLogic for DataTableApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let vp = backend.viewport();
        let lh = backend.line_height();
        let bar_h = if lh > 1.5 { lh * 1.5 } else { lh };
        let table_rect = Rect::new(0.0, 0.0, vp.width, vp.height - bar_h);
        let table = self.build_table();
        let _layout = backend.draw_data_table(table_rect, &table);

        let bar_rect = Rect::new(0.0, vp.height - bar_h, vp.width, bar_h);
        let _ = backend.draw_status_bar(bar_rect, &self.status_bar(), None, None);
    }

    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
        let total = Self::rows().len();
        match event {
            UiEvent::KeyPressed { key, .. } => {
                match key {
                    Key::Char('q') | Key::Named(NamedKey::Escape) => return Reaction::Exit,
                    Key::Char('j') | Key::Named(NamedKey::Down) => {
                        let cur = self.selected.unwrap_or(0);
                        if cur + 1 < total {
                            self.selected = Some(cur + 1);
                        }
                    }
                    Key::Char('k') | Key::Named(NamedKey::Up) => {
                        let cur = self.selected.unwrap_or(0);
                        self.selected = Some(cur.saturating_sub(1));
                    }
                    Key::Char('s') => {
                        self.sort_col = match self.sort_col {
                            None => Some(0),
                            Some(c) if c + 1 < self.columns.len() => Some(c + 1),
                            Some(_) => None,
                        };
                    }
                    Key::Char('d') => {
                        self.sort_asc = !self.sort_asc;
                    }
                    Key::Named(NamedKey::Home) => {
                        self.selected = Some(0);
                    }
                    Key::Named(NamedKey::End) => {
                        self.selected = Some(total.saturating_sub(1));
                    }
                    _ => return Reaction::Continue,
                }
                self.ensure_visible(backend);
                Reaction::Redraw
            }
            UiEvent::MouseDown { position, .. } => {
                let layout = self.table_layout(backend);
                match layout.hit_test(position.x, position.y, self.scroll_offset, total) {
                    DataTableHit::Header { col } => {
                        if self.sort_col == Some(col) {
                            self.sort_asc = !self.sort_asc;
                        } else {
                            self.sort_col = Some(col);
                            self.sort_asc = true;
                        }
                        Reaction::Redraw
                    }
                    DataTableHit::HeaderDivider { col } => {
                        self.resize_col = Some(col);
                        Reaction::Continue
                    }
                    DataTableHit::Row { idx } => {
                        self.selected = Some(idx);
                        Reaction::Redraw
                    }
                    DataTableHit::Empty => Reaction::Continue,
                }
            }
            UiEvent::MouseMoved { position, .. } => {
                if let Some(col) = self.resize_col {
                    let layout = self.table_layout(backend);
                    if col < layout.columns.len() {
                        let col_x = layout.columns[col].x;
                        let new_w = (position.x - col_x).max(20.0);
                        self.columns[col].width = ColumnWidth::Fixed(new_w);
                    }
                    return Reaction::Redraw;
                }
                Reaction::Continue
            }
            UiEvent::MouseUp { .. } => {
                if self.resize_col.take().is_some() {
                    return Reaction::Redraw;
                }
                Reaction::Continue
            }
            UiEvent::Scroll { delta, .. } => {
                let vis = self.visible_rows(backend);
                if delta.y < 0.0 {
                    self.scroll_offset = self
                        .scroll_offset
                        .saturating_add(3)
                        .min(total.saturating_sub(vis));
                } else if delta.y > 0.0 {
                    self.scroll_offset = self.scroll_offset.saturating_sub(3);
                }
                Reaction::Redraw
            }
            UiEvent::WindowResized { .. } => {
                self.ensure_visible(backend);
                Reaction::Redraw
            }
            _ => Reaction::Continue,
        }
    }
}
