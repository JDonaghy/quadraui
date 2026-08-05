//! Smoke test for [`DataTable`]: a Kubernetes-style pod list.
//!
//! Keys:
//! - `j` / `↓` — select next row
//! - `k` / `↑` — select previous row
//! - `s` — cycle sort column (Name → Status → Age → Restarts → none)
//! - `d` — toggle sort direction
//! - `f` — toggle the pinned footer/summary row (#432)
//! - `q` / `Esc` — quit

use quadraui::primitives::scrollbar::fit_thumb;
use quadraui::{
    AppLogic, Backend, Color, Column, ColumnAlign, ColumnWidth, DataRow, DataTable, DataTableHit,
    DataTableLayout, Key, NamedKey, Reaction, Rect, SortDirection, StatusBar, StatusBarSegment,
    StyledText, UiEvent, WidgetId,
};

pub struct DataTableApp {
    columns: Vec<Column>,
    selected: Option<usize>,
    scroll_offset: usize,
    sort_col: Option<usize>,
    sort_asc: bool,
    resize_col: Option<usize>,
    /// Per-column width overrides from divider drags, layered on top of
    /// `columns`' declared strategy (#516 defect 3) — mirrors how a real
    /// consumer app drives `DataTable`. Kept
    /// separate from `columns[i].width` deliberately: overriding a
    /// column must never rewrite its original declared strategy, only
    /// lay a resolved width on top of it — that distinction is exactly
    /// what `primitives::data_table::resolve_columns`'s pass 2 has to
    /// respect (and, before the fix, didn't) for a `Flex`-declared
    /// column.
    column_overrides: Vec<Option<f32>>,
    /// Vertical scrollbar thumb drag: (track_start_y, track_height, thumb_length, grab_offset)
    sb_drag: Option<(f32, f32, f32, f32)>,
    /// Horizontal scrollbar thumb drag: (track_start_x, track_width, thumb_length, grab_offset)
    h_sb_drag: Option<(f32, f32, f32, f32)>,
    h_scroll: f32,
    hovered_idx: Option<usize>,
    /// Toggle for the pinned footer/summary row demo (`f` key, #432).
    show_footer: bool,
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
            column_overrides: Vec::new(),
            sb_drag: None,
            h_sb_drag: None,
            h_scroll: 0.0,
            hovered_idx: None,
            show_footer: true,
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
            // Flex (not Fixed) so the divider immediately before the
            // last column (Restarts) sits between two Flex-declared
            // columns — the exact shape that reproduced #516 defect 3
            // ("divider before the last column resizes backward"): the
            // *dragged* column (Age) must itself be Flex-declared for
            // `resolve_columns`'s flex-redistribution pass to have a
            // chance of clobbering its override.
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
            // Status is the *middle* column (not last) and this value is
            // far wider than its resolved Flex(1.5) share — #516 defect
            // 1's regression case ("a table whose middle column contains
            // text far wider than its resolved width renders every other
            // column's value intact and readable"). Before the fix this
            // interleaved into Age/Restarts instead of clipping.
            (
                "grafana-5f4c8d-mn2q7",
                "CrashLoopBackOff: ImagePullBackOff waiting for registry retry backoff window",
                "1h",
                "14",
            ),
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

    /// Column-aligned totals row: pod count under Name, total restarts
    /// under Restarts — demonstrates #432 (pinned footer/summary row).
    fn footer_row(&self) -> DataRow {
        let rows = Self::rows();
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
            min_total_width: None,
            h_scroll: self.h_scroll,
            column_overrides: self.column_overrides.clone(),
            footer: if self.show_footer {
                Some(self.footer_row())
            } else {
                None
            },
        }
    }

    /// Resolved column widths for the current viewport — a test-only
    /// accessor exercising the exact same `DataTable::layout` (and thus
    /// the shared `primitives::data_table::resolve_columns`) any real
    /// backend paints through, so a driver test can assert on resize
    /// direction without hardcoding per-backend layout math (#516
    /// defect 3).
    pub fn resolved_column_widths(&self, backend: &dyn Backend) -> Vec<f32> {
        self.table_layout(backend)
            .columns
            .iter()
            .map(|c| c.width)
            .collect()
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
        // Delegate to the real layout (rather than re-deriving header/
        // footer math here) so the footer row's reserved height is
        // accounted for exactly once, in `DataTable::layout`.
        self.table_layout(backend).visible_rows
    }

    /// `pub` (not just an internal helper) so driver tests can locate
    /// real hit-test targets (e.g. a column divider) instead of
    /// hardcoding per-backend coordinates — see
    /// `resolved_column_widths`'s docs for why this matters for #516
    /// defect 3 coverage specifically.
    pub fn table_layout(&self, backend: &dyn Backend) -> DataTableLayout {
        let vp = backend.viewport();
        let lh = backend.line_height();
        let cw = backend.char_width();
        let bar_h = if lh > 1.5 { lh * 1.5 } else { lh };
        let table_rect = Rect::new(0.0, 0.0, vp.width, vp.height - bar_h);
        let mut table = self.build_table();
        table.min_total_width = Some(80.0 * cw);
        backend.data_table_layout(table_rect, &table)
    }

    fn scrollbar_geometry(&self, backend: &dyn Backend) -> Option<(f32, f32, f32, f32, f32)> {
        let total = Self::rows().len();
        let layout = self.table_layout(backend);
        if !self.build_table().show_scrollbar
            || total <= layout.visible_rows
            || layout.scrollbar_width <= 0.0
        {
            return None;
        }
        let vp = backend.viewport();
        let lh = backend.line_height();
        let sb_x = vp.width - layout.scrollbar_width;
        let track_y = layout.header_height;
        let track_h =
            (vp.height - lh.max(1.0) * 1.5 - layout.header_height - layout.footer_height).max(1.0);
        let (thumb_start, thumb_len) = fit_thumb(
            self.scroll_offset as f32,
            total as f32,
            layout.visible_rows as f32,
            track_h,
            if lh > 1.5 { lh } else { 1.0 },
        );
        Some((sb_x, track_y, track_h, thumb_start, thumb_len))
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
        let cw = backend.char_width();
        let bar_h = if lh > 1.5 { lh * 1.5 } else { lh };
        let table_rect = Rect::new(0.0, 0.0, vp.width, vp.height - bar_h);
        let mut table = self.build_table();
        table.min_total_width = Some(80.0 * cw);
        let _layout = backend.draw_data_table(table_rect, &table, self.hovered_idx);

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
                    Key::Char('f') => {
                        self.show_footer = !self.show_footer;
                    }
                    Key::Named(NamedKey::Home) => {
                        self.selected = Some(0);
                    }
                    Key::Named(NamedKey::End) => {
                        self.selected = Some(total.saturating_sub(1));
                    }
                    Key::Char('H') | Key::Named(NamedKey::Left) => {
                        self.h_scroll = (self.h_scroll - 5.0).max(0.0);
                    }
                    Key::Char('L') | Key::Named(NamedKey::Right) => {
                        let layout = self.table_layout(backend);
                        let max_h = (layout.content_width - layout.viewport_width
                            + layout.scrollbar_width)
                            .max(0.0);
                        self.h_scroll = (self.h_scroll + 5.0).min(max_h);
                    }
                    _ => return Reaction::Continue,
                }
                self.ensure_visible(backend);
                Reaction::Redraw
            }
            UiEvent::MouseDown { position, .. } => {
                // Check h-scrollbar first.
                let layout = self.table_layout(backend);
                if layout.h_scrollbar_height > 0.0 && layout.content_width > 0.0 {
                    let vp = backend.viewport();
                    let lh = backend.line_height();
                    let bar_h = if lh > 1.5 { lh * 1.5 } else { lh };
                    let table_h = vp.height - bar_h;
                    let hsb_y = table_h - layout.footer_height - layout.h_scrollbar_height;
                    if position.y >= hsb_y && position.y < table_h {
                        let track_w = (vp.width - layout.scrollbar_width).max(1.0);
                        let visible_w = track_w;
                        let max_h_scroll = (layout.content_width - visible_w).max(0.0);
                        let (thumb_start, thumb_len) = fit_thumb(
                            self.h_scroll,
                            layout.content_width,
                            visible_w,
                            track_w,
                            if lh > 1.5 { lh } else { 1.0 },
                        );
                        let local_x = position.x;
                        if local_x >= thumb_start && local_x < thumb_start + thumb_len {
                            self.h_sb_drag = Some((0.0, track_w, thumb_len, local_x - thumb_start));
                            return Reaction::Continue;
                        }
                        // Track click — page left/right.
                        let page = visible_w;
                        if local_x < thumb_start {
                            self.h_scroll = (self.h_scroll - page).max(0.0);
                        } else {
                            self.h_scroll = (self.h_scroll + page).min(max_h_scroll);
                        }
                        return Reaction::Redraw;
                    }
                }
                // Check v-scrollbar.
                if let Some((sb_x, track_y, track_h, thumb_start, thumb_len)) =
                    self.scrollbar_geometry(backend)
                {
                    let vis = self.visible_rows(backend);
                    let max_scroll = total.saturating_sub(vis);
                    if position.x >= sb_x {
                        let local_y = position.y - track_y;
                        if local_y >= thumb_start && local_y < thumb_start + thumb_len {
                            self.sb_drag =
                                Some((track_y, track_h, thumb_len, local_y - thumb_start));
                            return Reaction::Continue;
                        }
                        // Track click — page up/down.
                        if local_y < thumb_start {
                            self.scroll_offset = self.scroll_offset.saturating_sub(vis);
                        } else {
                            self.scroll_offset = (self.scroll_offset + vis).min(max_scroll);
                        }
                        return Reaction::Redraw;
                    }
                }
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
                        // Just remember which divider is being dragged;
                        // `MouseMoved` below does the actual pair-resize
                        // math against the *current* layout each move
                        // (#521 defect 1) rather than snapshotting once
                        // here, so a drag that never moves still leaves
                        // `column_overrides` untouched.
                        self.resize_col = Some(col);
                        Reaction::Continue
                    }
                    DataTableHit::Row { idx } => {
                        self.selected = Some(idx);
                        Reaction::Redraw
                    }
                    // Pinned footer isn't selectable — same as empty space.
                    DataTableHit::Footer | DataTableHit::Empty => Reaction::Continue,
                }
            }
            UiEvent::MouseMoved { position, .. } => {
                if let Some((track_x, track_w, thumb_len, grab_off)) = self.h_sb_drag {
                    let lay = self.table_layout(backend);
                    let visible_w = (lay.viewport_width - lay.scrollbar_width).max(1.0);
                    let max_h_scroll = (lay.content_width - visible_w).max(0.0);
                    let effective = (track_w - thumb_len).max(1.0);
                    let rel = ((position.x - track_x - grab_off) / effective).clamp(0.0, 1.0);
                    self.h_scroll = (rel * max_h_scroll).round().min(max_h_scroll);
                    return Reaction::Redraw;
                }
                if let Some((track_y, track_h, thumb_len, grab_off)) = self.sb_drag {
                    let vis = self.visible_rows(backend);
                    let max_scroll = total.saturating_sub(vis);
                    let effective = (track_h - thumb_len).max(1.0);
                    let rel = ((position.y - track_y - grab_off) / effective).clamp(0.0, 1.0);
                    self.scroll_offset = (rel * max_scroll as f32).round() as usize;
                    return Reaction::Redraw;
                }
                if let Some(col) = self.resize_col {
                    let layout = self.table_layout(backend);
                    // Pair-resize (#521 defect 1): a divider drag moves
                    // width between `col` and `col + 1` only, combined
                    // width held constant, every other column frozen at
                    // its currently-resolved width. Layered on top of
                    // `columns` via `column_overrides` — the real API
                    // surface a consumer drags through — rather than
                    // rewriting the columns' declared strategies directly.
                    // A small absolute floor (not the old single-column
                    // `.max(20.0)`): this table's own `Restarts` column
                    // is declared `Fixed(10.0)` — the same literal value
                    // in both cell (TUI) and pixel (GTK) units — so a
                    // pair-conserving floor bigger than that would make
                    // the divider immediately before it refuse to widen
                    // at all, contradicting the #516 regression that the
                    // same divider must resize in the drag's direction.
                    self.column_overrides =
                        layout.drag_divider(&self.column_overrides, col, position.x, 4.0);
                    return Reaction::Redraw;
                }
                let layout = self.table_layout(backend);
                let total = Self::rows().len();
                let old = self.hovered_idx;
                match layout.hit_test(position.x, position.y, self.scroll_offset, total) {
                    DataTableHit::Row { idx } => self.hovered_idx = Some(idx),
                    _ => self.hovered_idx = None,
                }
                if self.hovered_idx != old {
                    return Reaction::Redraw;
                }
                Reaction::Continue
            }
            UiEvent::MouseUp { .. } => {
                let had_drag = self.resize_col.take().is_some()
                    || self.sb_drag.take().is_some()
                    || self.h_sb_drag.take().is_some();
                if had_drag {
                    return Reaction::Redraw;
                }
                Reaction::Continue
            }
            UiEvent::Scroll { delta, .. } => {
                if delta.x.abs() > 0.01 {
                    let layout = self.table_layout(backend);
                    let max_h = (layout.content_width - layout.viewport_width
                        + layout.scrollbar_width)
                        .max(0.0);
                    self.h_scroll = (self.h_scroll - delta.x * 10.0).clamp(0.0, max_h);
                }
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
