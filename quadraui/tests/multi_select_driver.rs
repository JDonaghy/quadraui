#![cfg(feature = "tui")]
//! Headless `TuiDriver` conformance test for multi-select gestures:
//! plain click (replace), Ctrl/Cmd-click (toggle), Shift-click (range),
//! and Shift+Down keyboard extension, all driving `SelectionMode::Multi`
//! through the real `event -> handle -> render` path on `ListView`,
//! `TreeView`, and `DataTable`.
//!
//! Per the `SelectionGesture` / `apply_selection_gesture` seam in
//! `src/types.rs`, these primitives stay purely declarative: the app
//! computes the gesture from the incoming click/key `Modifiers` via
//! `SelectionGesture::from_modifiers` + `apply_selection_gesture`, then
//! writes the resulting index set back into the primitive's own
//! selection field(s) and constructs the primitive's own `SelectionSet`
//! event itself. The three `*MultiApp`s below are that app layer's
//! reference implementation, driven headlessly against the real TUI
//! rasterisers (`draw_list` / `draw_tree` / `draw_data_table`).

use quadraui::tui::testing::TuiDriver;
use quadraui::{
    apply_selection_gesture, selected_paths_for_indices, AppLogic, Backend, Column, DataRow,
    DataTable, DataTableEvent, DataTableHit, Key, ListItem, ListView, ListViewEvent, ListViewHit,
    Modifiers, MouseButton, NamedKey, Reaction, Rect, SelectionGesture, SelectionMode, StyledText,
    TreeEvent, TreeRow, TreeView, TreeViewHit, UiEvent, WidgetId,
};

const VIEWPORT: Rect = Rect {
    x: 0.0,
    y: 0.0,
    width: 30.0,
    height: 10.0,
};

// ── ListView ─────────────────────────────────────────────────────────────

struct ListMultiApp {
    list: ListView,
    anchor: usize,
    focus: usize,
    last_event: Option<ListViewEvent>,
}

impl ListMultiApp {
    fn new() -> Self {
        let items = (0..6)
            .map(|i| ListItem {
                text: StyledText::plain(format!("item {i}")),
                icon: None,
                detail: None,
                decoration: Default::default(),
            })
            .collect();
        let list = ListView {
            items,
            has_focus: true,
            selection_mode: SelectionMode::Multi,
            ..Default::default()
        };
        Self {
            list,
            anchor: 0,
            focus: 0,
            last_event: None,
        }
    }

    /// Mirrors the app-layer seam: derive the next selection from the
    /// gesture, update the anchor only when the gesture isn't `Range` (so
    /// later Shift-clicks keep extending from the last non-extending
    /// target), and emit `SelectionSet` with the full resulting set.
    fn apply_gesture(&mut self, idx: usize, gesture: SelectionGesture) {
        let next = apply_selection_gesture(&self.list.selected_indices, self.anchor, idx, gesture);
        if !matches!(gesture, SelectionGesture::Range) {
            self.anchor = idx;
        }
        self.focus = idx;
        self.list.selected_indices = next.clone();
        self.last_event = Some(ListViewEvent::SelectionSet { indices: next });
    }
}

impl AppLogic for ListMultiApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        backend.draw_list(VIEWPORT, &self.list);
    }

    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
        match event {
            UiEvent::MouseDown {
                position,
                modifiers,
                ..
            } => {
                let layout = backend.list_layout(VIEWPORT, &self.list);
                match layout.hit_test(position.x, position.y) {
                    ListViewHit::Item(idx) => {
                        self.apply_gesture(idx, SelectionGesture::from_modifiers(modifiers));
                        Reaction::Redraw
                    }
                    _ => Reaction::Continue,
                }
            }
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Down),
                modifiers,
                ..
            } => {
                let next_focus = (self.focus + 1).min(self.list.items.len().saturating_sub(1));
                let gesture = if modifiers.shift {
                    SelectionGesture::Range
                } else {
                    SelectionGesture::Replace
                };
                self.apply_gesture(next_focus, gesture);
                Reaction::Redraw
            }
            _ => Reaction::Continue,
        }
    }
}

fn list_driver() -> TuiDriver<ListMultiApp> {
    let mut driver = TuiDriver::new(ListMultiApp::new(), 30, 10);
    // These tests click the same row twice in a row (to toggle it back
    // off) with no intent of ever producing a `DoubleClick` — disable
    // `TuiBackend`'s `DoubleClickDetector` folding so each `click_with`
    // call is delivered as its own independent `MouseDown`.
    driver.set_double_click_folding(false);
    driver
}

#[test]
fn list_plain_click_replaces_selection() {
    let mut driver = list_driver();
    let (x, y) = driver.find("item 2").unwrap();
    driver.click_with(MouseButton::Left, Modifiers::default(), x, y);
    assert_eq!(driver.app().list.selected_indices, vec![2]);
    assert_eq!(
        driver.app().last_event,
        Some(ListViewEvent::SelectionSet { indices: vec![2] })
    );
}

#[test]
fn list_ctrl_click_toggles_membership() {
    let mut driver = list_driver();
    let (x2, y2) = driver.find("item 2").unwrap();
    driver.click_with(MouseButton::Left, Modifiers::default(), x2, y2);
    let ctrl = Modifiers {
        ctrl: true,
        ..Default::default()
    };
    let (x4, y4) = driver.find("item 4").unwrap();
    driver.click_with(MouseButton::Left, ctrl, x4, y4);
    assert_eq!(driver.app().list.selected_indices, vec![2, 4]);

    // Both non-adjacent rows must actually paint with the selected
    // background — not just the anchor row — which is the whole point
    // of the shared `is_row_selected` seam this test exercises.
    let (x0, y0) = driver.find("item 0").unwrap();
    let bg2 = driver.style_at(x2 as u16, y2 as u16).unwrap().bg;
    let bg4 = driver.style_at(x4 as u16, y4 as u16).unwrap().bg;
    let bg0 = driver.style_at(x0 as u16, y0 as u16).unwrap().bg;
    assert_eq!(bg2, bg4, "both selected rows should share the selected bg");
    assert_ne!(bg2, bg0, "unselected row should not paint the selected bg");

    // Ctrl-clicking the same row again removes it from the set.
    driver.click_with(MouseButton::Left, ctrl, x4, y4);
    assert_eq!(driver.app().list.selected_indices, vec![2]);
}

#[test]
fn list_shift_click_selects_contiguous_range() {
    let mut driver = list_driver();
    let (x1, y1) = driver.find("item 1").unwrap();
    driver.click_with(MouseButton::Left, Modifiers::default(), x1, y1);
    let shift = Modifiers {
        shift: true,
        ..Default::default()
    };
    let (x4, y4) = driver.find("item 4").unwrap();
    driver.click_with(MouseButton::Left, shift, x4, y4);
    assert_eq!(driver.app().list.selected_indices, vec![1, 2, 3, 4]);
    assert_eq!(
        driver.app().last_event,
        Some(ListViewEvent::SelectionSet {
            indices: vec![1, 2, 3, 4]
        })
    );
}

#[test]
fn list_shift_down_extends_selection_from_the_keyboard() {
    let mut driver = list_driver();
    let (x1, y1) = driver.find("item 1").unwrap();
    driver.click_with(MouseButton::Left, Modifiers::default(), x1, y1);
    let shift = Modifiers {
        shift: true,
        ..Default::default()
    };
    driver.dispatch(UiEvent::KeyPressed {
        key: Key::Named(NamedKey::Down),
        modifiers: shift,
        repeat: false,
    });
    driver.dispatch(UiEvent::KeyPressed {
        key: Key::Named(NamedKey::Down),
        modifiers: shift,
        repeat: false,
    });
    assert_eq!(driver.app().list.selected_indices, vec![1, 2, 3]);
}

// ── TreeView ─────────────────────────────────────────────────────────────

struct TreeMultiApp {
    tree: TreeView,
    anchor: usize,
    selected_indices: Vec<usize>,
    last_event: Option<TreeEvent>,
}

impl TreeMultiApp {
    fn new() -> Self {
        let rows = (0..6_u16)
            .map(|i| TreeRow {
                path: vec![i],
                indent: 0,
                icon: None,
                text: StyledText::plain(format!("node {i}")),
                badge: None,
                is_expanded: None,
                decoration: Default::default(),
                edit: None,
            })
            .collect();
        let tree = TreeView {
            id: WidgetId::new("test:tree"),
            rows,
            selection_mode: SelectionMode::Multi,
            selected_path: None,
            selected_paths: Vec::new(),
            scroll_offset: 0,
            style: Default::default(),
            has_focus: true,
        };
        Self {
            tree,
            anchor: 0,
            selected_indices: Vec::new(),
            last_event: None,
        }
    }

    /// Flattened-row-index gesture math, then bridged back to
    /// `TreePath`s via `selected_paths_for_indices` before writing into
    /// `TreeView`'s path-based selection fields — see this file's module
    /// doc and `TreeEvent::SelectionSet`'s doc for why `TreeView` needs
    /// this extra step that `ListView`/`DataTable` don't.
    fn apply_gesture(&mut self, idx: usize, gesture: SelectionGesture) {
        let next = apply_selection_gesture(&self.selected_indices, self.anchor, idx, gesture);
        if !matches!(gesture, SelectionGesture::Range) {
            self.anchor = idx;
        }
        self.selected_indices = next.clone();
        let paths = selected_paths_for_indices(&self.tree.rows, &next);
        self.tree.selected_path = paths.last().cloned();
        self.tree.selected_paths = paths.clone();
        self.last_event = Some(TreeEvent::SelectionSet { paths });
    }
}

impl AppLogic for TreeMultiApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        backend.draw_tree(VIEWPORT, &self.tree);
    }

    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
        match event {
            UiEvent::MouseDown {
                position,
                modifiers,
                ..
            } => {
                let layout = backend.tree_layout(VIEWPORT, &self.tree);
                match layout.hit_test(position.x, position.y) {
                    TreeViewHit::Row(idx) => {
                        self.apply_gesture(idx, SelectionGesture::from_modifiers(modifiers));
                        Reaction::Redraw
                    }
                    _ => Reaction::Continue,
                }
            }
            _ => Reaction::Continue,
        }
    }
}

fn tree_driver() -> TuiDriver<TreeMultiApp> {
    let mut driver = TuiDriver::new(TreeMultiApp::new(), 30, 10);
    driver.set_double_click_folding(false);
    driver
}

#[test]
fn tree_plain_click_replaces_selection() {
    let mut driver = tree_driver();
    let (x, y) = driver.find("node 2").unwrap();
    driver.click_with(MouseButton::Left, Modifiers::default(), x, y);
    assert_eq!(driver.app().tree.selected_paths, vec![vec![2]]);
    assert_eq!(
        driver.app().last_event,
        Some(TreeEvent::SelectionSet {
            paths: vec![vec![2]]
        })
    );
}

#[test]
fn tree_ctrl_click_toggles_membership() {
    let mut driver = tree_driver();
    let (x2, y2) = driver.find("node 2").unwrap();
    driver.click_with(MouseButton::Left, Modifiers::default(), x2, y2);
    let ctrl = Modifiers {
        ctrl: true,
        ..Default::default()
    };
    let (x4, y4) = driver.find("node 4").unwrap();
    driver.click_with(MouseButton::Left, ctrl, x4, y4);
    assert_eq!(driver.app().tree.selected_paths, vec![vec![2], vec![4]]);

    // Both non-adjacent rows must actually paint with the selected
    // background on the real TUI tree rasteriser — not just the anchor
    // row held in the now-legacy `selected_path` field.
    let (x0, y0) = driver.find("node 0").unwrap();
    let bg2 = driver.style_at(x2 as u16, y2 as u16).unwrap().bg;
    let bg4 = driver.style_at(x4 as u16, y4 as u16).unwrap().bg;
    let bg0 = driver.style_at(x0 as u16, y0 as u16).unwrap().bg;
    assert_eq!(bg2, bg4, "both selected rows should share the selected bg");
    assert_ne!(bg2, bg0, "unselected row should not paint the selected bg");

    driver.click_with(MouseButton::Left, ctrl, x4, y4);
    assert_eq!(driver.app().tree.selected_paths, vec![vec![2]]);
}

#[test]
fn tree_shift_click_selects_contiguous_range() {
    let mut driver = tree_driver();
    let (x1, y1) = driver.find("node 1").unwrap();
    driver.click_with(MouseButton::Left, Modifiers::default(), x1, y1);
    let shift = Modifiers {
        shift: true,
        ..Default::default()
    };
    let (x4, y4) = driver.find("node 4").unwrap();
    driver.click_with(MouseButton::Left, shift, x4, y4);
    assert_eq!(
        driver.app().tree.selected_paths,
        vec![vec![1], vec![2], vec![3], vec![4]]
    );
    assert_eq!(
        driver.app().last_event,
        Some(TreeEvent::SelectionSet {
            paths: vec![vec![1], vec![2], vec![3], vec![4]]
        })
    );
}

// ── DataTable ────────────────────────────────────────────────────────────

struct DataTableMultiApp {
    table: DataTable,
    anchor: usize,
    last_event: Option<DataTableEvent>,
}

impl DataTableMultiApp {
    fn new() -> Self {
        let rows = (0..6)
            .map(|i| DataRow {
                cells: vec![StyledText::plain(format!("row {i}"))],
                decoration: Default::default(),
            })
            .collect();
        let table = DataTable {
            id: WidgetId::new("test:table"),
            columns: vec![Column {
                title: "Name".to_string(),
                width: Default::default(),
                align: Default::default(),
            }],
            rows,
            selected_idx: None,
            scroll_offset: 0,
            sort: None,
            has_focus: true,
            show_scrollbar: false,
            min_total_width: None,
            h_scroll: 0.0,
            column_overrides: vec![],
            footer: None,
            selection_mode: SelectionMode::Multi,
            selected_indices: Vec::new(),
        };
        Self {
            table,
            anchor: 0,
            last_event: None,
        }
    }

    fn apply_gesture(&mut self, idx: usize, gesture: SelectionGesture) {
        let next = apply_selection_gesture(&self.table.selected_indices, self.anchor, idx, gesture);
        if !matches!(gesture, SelectionGesture::Range) {
            self.anchor = idx;
        }
        self.table.selected_indices = next.clone();
        self.last_event = Some(DataTableEvent::SelectionSet { indices: next });
    }
}

impl AppLogic for DataTableMultiApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        backend.draw_data_table(VIEWPORT, &self.table, None);
    }

    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
        match event {
            UiEvent::MouseDown {
                position,
                modifiers,
                ..
            } => {
                let layout = backend.data_table_layout(VIEWPORT, &self.table);
                match layout.hit_test(
                    position.x,
                    position.y,
                    self.table.scroll_offset,
                    self.table.rows.len(),
                ) {
                    DataTableHit::Row { idx } => {
                        self.apply_gesture(idx, SelectionGesture::from_modifiers(modifiers));
                        Reaction::Redraw
                    }
                    _ => Reaction::Continue,
                }
            }
            _ => Reaction::Continue,
        }
    }
}

fn data_table_driver() -> TuiDriver<DataTableMultiApp> {
    let mut driver = TuiDriver::new(DataTableMultiApp::new(), 30, 10);
    driver.set_double_click_folding(false);
    driver
}

#[test]
fn data_table_plain_click_replaces_selection() {
    let mut driver = data_table_driver();
    let (x, y) = driver.find("row 2").unwrap();
    driver.click_with(MouseButton::Left, Modifiers::default(), x, y);
    assert_eq!(driver.app().table.selected_indices, vec![2]);
    assert_eq!(
        driver.app().last_event,
        Some(DataTableEvent::SelectionSet { indices: vec![2] })
    );
}

#[test]
fn data_table_ctrl_click_toggles_membership() {
    let mut driver = data_table_driver();
    let (x2, y2) = driver.find("row 2").unwrap();
    driver.click_with(MouseButton::Left, Modifiers::default(), x2, y2);
    let ctrl = Modifiers {
        ctrl: true,
        ..Default::default()
    };
    let (x4, y4) = driver.find("row 4").unwrap();
    driver.click_with(MouseButton::Left, ctrl, x4, y4);
    assert_eq!(driver.app().table.selected_indices, vec![2, 4]);

    // Both non-adjacent rows must actually paint with the selected
    // background — not just the anchor row.
    let (x0, y0) = driver.find("row 0").unwrap();
    let bg2 = driver.style_at(x2 as u16, y2 as u16).unwrap().bg;
    let bg4 = driver.style_at(x4 as u16, y4 as u16).unwrap().bg;
    let bg0 = driver.style_at(x0 as u16, y0 as u16).unwrap().bg;
    assert_eq!(bg2, bg4, "both selected rows should share the selected bg");
    assert_ne!(bg2, bg0, "unselected row should not paint the selected bg");

    driver.click_with(MouseButton::Left, ctrl, x4, y4);
    assert_eq!(driver.app().table.selected_indices, vec![2]);
}

#[test]
fn data_table_shift_click_selects_contiguous_range() {
    let mut driver = data_table_driver();
    let (x1, y1) = driver.find("row 1").unwrap();
    driver.click_with(MouseButton::Left, Modifiers::default(), x1, y1);
    let shift = Modifiers {
        shift: true,
        ..Default::default()
    };
    let (x4, y4) = driver.find("row 4").unwrap();
    driver.click_with(MouseButton::Left, shift, x4, y4);
    assert_eq!(driver.app().table.selected_indices, vec![1, 2, 3, 4]);
    assert_eq!(
        driver.app().last_event,
        Some(DataTableEvent::SelectionSet {
            indices: vec![1, 2, 3, 4]
        })
    );
}
