#![cfg(feature = "gtk")]
//! Headless `GtkDriver` checks that a widget painted through the widget
//! contract (`DrawsPrimitives::draw_dyn`) looks, lays out and hit-tests on
//! GTK the way the host's styling and the backend's native painters do:
//!
//! - a bordered `ListView` strokes its rounded frame and insets its rows;
//! - a `Scrollbar` honours the backend's overlay idle fade;
//! - a `DataTable` `Content` column is as wide as the text painted in it;
//! - a `TreeView` lays out with the host's live `Style`;
//! - a `StatusBar` measures the width its text fits in untruncated;
//! - a `FindReplacePanel` paints with the backend's `Style`, so a host
//!   resolves its click geometry from that same style.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;

use quadraui::gtk::testing::GtkDriver;
use quadraui::primitive::{LayoutCtx, Primitive, Size};
use quadraui::primitives::data_table::DataTablePrimitiveLayout;
use quadraui::primitives::tree::TreeViewPrimitiveLayout;
use quadraui::{
    AppLogic, Backend, Column, ColumnAlign, ColumnWidth, DataRow, DataTable, Decoration,
    DrawsPrimitives, FindReplaceClickTarget, FindReplaceHit, FindReplaceMetrics, FindReplacePanel,
    FontRole, ListItem, ListView, ListViewHit, ListViewPrimitiveLayout, Reaction, Rect, Scrollbar,
    SelectionMode, StatusBar, StatusBarSegment, Style, StyledText, TreeRow, TreeStyle, TreeView,
    UiEvent, WidgetId,
};

const W: i32 = 480;
const H: i32 = 320;

/// Paints through `paint` every frame, under `style`.
struct PaintApp<F> {
    style: Style,
    paint: F,
}

impl<F: Fn(&mut dyn Backend)> AppLogic for PaintApp<F> {
    type AreaId = ();

    fn setup(&mut self, backend: &mut dyn Backend) {
        backend.set_style(self.style);
    }

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        backend.set_style(self.style);
        (self.paint)(backend);
    }

    fn handle(&mut self, _event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        Reaction::Continue
    }
}

fn drive<F: Fn(&mut dyn Backend)>(style: Style, paint: F) -> GtkDriver<PaintApp<F>> {
    GtkDriver::new(PaintApp { style, paint }, W, H)
}

/// The rendered screen with the overlay fade pinned at `last_scroll` (None:
/// never scrolled, fully faded) and its clock at `now`.
fn screen_with_fade<F: Fn(&mut dyn Backend)>(
    driver: &mut GtkDriver<PaintApp<F>>,
    last_scroll: Option<Instant>,
    now: Instant,
) -> Vec<u8> {
    let (backend, _) = driver.parts_mut();
    backend.set_scroll_last_interaction(last_scroll);
    backend.set_fade_clock_override(Some(now));
    driver.render();
    driver.screen()
}

fn close(a: (u8, u8, u8), b: (u8, u8, u8)) -> bool {
    let d = |x: u8, y: u8| (x as i16 - y as i16).abs();
    d(a.0, b.0) <= 24 && d(a.1, b.1) <= 24 && d(a.2, b.2) <= 24
}

// ── ListView ────────────────────────────────────────────────────────────

const LIST_RECT: Rect = Rect::new(20.0, 20.0, 240.0, 160.0);

fn bordered_list() -> ListView {
    ListView {
        id: WidgetId::new("fidelity:list"),
        title: None,
        items: (0..4)
            .map(|i| ListItem {
                text: StyledText::plain(format!("entry {i}")),
                icon: None,
                detail: None,
                decoration: Decoration::default(),
            })
            .collect(),
        selected_idx: 1,
        scroll_offset: 0,
        has_focus: true,
        bordered: true,
        h_scroll: 0,
        max_content_width: None,
        show_v_scrollbar: false,
        striped: false,
    }
}

#[test]
fn bordered_list_view_paints_its_frame_and_insets_its_rows() {
    let list = bordered_list();
    let laid_out: Rc<RefCell<Option<ListViewPrimitiveLayout>>> = Rc::default();
    let record = laid_out.clone();
    let mut contract = drive(Style::default(), move |b| {
        let layout = b.draw_dyn(LIST_RECT, &list, &());
        *record.borrow_mut() = layout
            .as_any()
            .downcast_ref::<ListViewPrimitiveLayout>()
            .cloned();
    });

    // The frame is a border_fg stroke along the list's edge.
    let border = contract.backend().theme().border_fg;
    let top_mid = contract.pixel(
        (LIST_RECT.x + LIST_RECT.width / 2.0) as i32,
        LIST_RECT.y as i32,
    );
    assert!(
        close(top_mid, (border.r, border.g, border.b)),
        "the frame's top edge paints border_fg {border:?}, got {top_mid:?}"
    );

    // Rows start inside the frame, and a click is resolved inside it too.
    let layout = laid_out.borrow().clone().expect("a ListView layout");
    assert_eq!(layout.border_inset, 1.0);
    let first = layout.layout.visible_items[0].bounds;
    let first_end = LIST_RECT.y + layout.border_inset + first.y + first.height;
    let x = LIST_RECT.x + 30.0;
    assert_eq!(
        layout.hit_test(x, first_end - 0.5),
        ListViewHit::Item(0),
        "the first row's last pixel row belongs to the first row"
    );
    assert_eq!(layout.hit_test(x, first_end + 0.5), ListViewHit::Item(1));
}

// ── Scrollbar ───────────────────────────────────────────────────────────

const TRACK: Rect = Rect::new(400.0, 20.0, 10.0, 240.0);

fn scrollbar() -> Scrollbar {
    Scrollbar::vertical(
        WidgetId::new("fidelity:sb"),
        TRACK,
        120.0,
        1000.0,
        240.0,
        8.0,
    )
}

#[test]
fn contract_scrollbar_follows_the_backend_idle_fade() {
    let t0 = Instant::now();
    let idle = t0 + std::time::Duration::from_secs(10);
    let mut blank = drive(Style::default(), |_| {});
    let mut contract = drive(Style::default(), |b| {
        let _ = b.draw_dyn(TRACK, &scrollbar(), &());
    });

    // Never scrolled: fully faded, nothing painted at all.
    let never = screen_with_fade(&mut contract, None, t0);
    assert!(
        never == screen_with_fade(&mut blank, None, t0),
        "a never-scrolled contract Scrollbar paints at opacity 0"
    );

    // Just scrolled: full opacity.
    let fresh = screen_with_fade(&mut contract, Some(t0), t0);
    assert!(
        fresh != screen_with_fade(&mut blank, Some(t0), t0),
        "a just-scrolled contract Scrollbar is visible"
    );

    // Long idle: faded out again.
    assert!(
        screen_with_fade(&mut contract, Some(t0), idle)
            == screen_with_fade(&mut blank, Some(t0), idle)
    );
}

// ── DataTable ───────────────────────────────────────────────────────────

const TABLE_RECT: Rect = Rect::new(10.0, 10.0, 460.0, 200.0);
/// A `Content` column's title, in glyphs far wider than the average glyph.
const WIDE: &str = "漢字漢字漢字";
/// The cell under it.
const CELL: &str = "日本";

fn wide_table() -> DataTable {
    DataTable {
        selection_mode: Default::default(),
        selected_indices: Vec::new(),
        id: WidgetId::new("fidelity:table"),
        columns: vec![
            Column {
                title: WIDE.into(),
                width: ColumnWidth::Content {
                    min: 0.0,
                    max: 400.0,
                },
                align: ColumnAlign::Left,
            },
            Column {
                title: "Tail".into(),
                width: ColumnWidth::Flex(1.0),
                align: ColumnAlign::Left,
            },
        ],
        rows: vec![DataRow {
            cells: vec![StyledText::plain(CELL), StyledText::plain("tail")],
            decoration: Default::default(),
        }],
        selected_idx: None,
        scroll_offset: 0,
        sort: None,
        has_focus: false,
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

#[test]
fn data_table_content_column_spans_the_wide_text_painted_in_it() {
    let laid_out: Rc<RefCell<Option<DataTablePrimitiveLayout>>> = Rc::default();
    let record = laid_out.clone();
    let driver = drive(Style::default(), move |b| {
        let layout = b.draw_dyn(TABLE_RECT, &wide_table(), &());
        *record.borrow_mut() = layout
            .as_any()
            .downcast_ref::<DataTablePrimitiveLayout>()
            .cloned();
    });
    let layout = laid_out.borrow().clone().expect("a DataTable layout");
    let col = &layout.layout.columns[0];
    let col_x0 = layout.area.x + col.x;
    let col_x1 = col_x0 + col.width;

    // The title the column sized to paints in full, inside the column:
    // the column was measured in the font the header paints in.
    let title = driver
        .find_bounds(WIDE)
        .unwrap_or_else(|| panic!("{WIDE:?} painted untruncated: {:?}", driver.painted_texts()));
    assert!(
        title.x >= col_x0 - 0.5 && title.x + title.width <= col_x1 + 0.5,
        "painted {WIDE:?} at {title:?} must lie inside column 0 [{col_x0}, {col_x1})"
    );
    let tail = driver.find_bounds("Tail").expect("Tail painted");
    assert!(
        tail.x >= layout.area.x + layout.layout.columns[1].x - 0.5,
        "column 1's title starts inside column 1: {tail:?}"
    );

    // A click on the painted cell lands on its row, inside column 0.
    let cell = driver.find_bounds(CELL).expect("cell painted");
    let (cx, cy) = (cell.x + cell.width / 2.0, cell.y + cell.height / 2.0);
    assert_eq!(
        layout.hit_test(cx, cy),
        quadraui::DataTableHit::Row { idx: 0 }
    );
    assert!(
        cx >= col_x0 && cx < col_x1,
        "{cell:?} in [{col_x0}, {col_x1})"
    );
}

// ── TreeView ────────────────────────────────────────────────────────────

const TREE_RECT: Rect = Rect::new(10.0, 10.0, 300.0, 120.0);

fn themed_style() -> Style {
    Style::default()
        .with_row_padding_x(28.0)
        .with_scrollbar_width(18.0)
}

fn tall_tree() -> TreeView {
    TreeView {
        id: WidgetId::new("fidelity:tree"),
        rows: (0..20)
            .map(|i| TreeRow {
                path: vec![i],
                indent: 0,
                icon: None,
                text: StyledText::plain(format!("node {i}")),
                badge: None,
                is_expanded: Some(i % 2 == 0),
                decoration: Decoration::Normal,
                edit: None,
            })
            .collect(),
        selection_mode: SelectionMode::Single,
        selected_path: None,
        scroll_offset: 0,
        style: TreeStyle::default(),
        has_focus: false,
    }
}

/// Paints `tall_tree` through `draw_dyn` under `style`, returning the
/// driver and the layout `draw_dyn` resolved.
fn drive_tree(style: Style) -> (GtkDriver<impl AppLogic>, TreeViewPrimitiveLayout) {
    let laid_out: Rc<RefCell<Option<TreeViewPrimitiveLayout>>> = Rc::default();
    let record = laid_out.clone();
    let driver = drive(style, move |b: &mut dyn Backend| {
        let layout = b.draw_dyn(TREE_RECT, &tall_tree(), &());
        *record.borrow_mut() = layout
            .as_any()
            .downcast_ref::<TreeViewPrimitiveLayout>()
            .cloned();
    });
    let layout = laid_out.borrow().clone().expect("a TreeView layout");
    (driver, layout)
}

#[test]
fn tree_view_lays_out_and_paints_with_the_live_style() {
    let (themed, layout) = drive_tree(themed_style());
    let (plain, _) = drive_tree(Style::default());
    let shift = themed_style().row_padding_x - Style::default().row_padding_x;

    for (i, label) in ["node 0", "node 1", "node 3"].into_iter().enumerate() {
        let painted = themed
            .find_bounds(label)
            .unwrap_or_else(|| panic!("{label:?} painted: {:?}", themed.painted_texts()));
        let default_painted = plain.find_bounds(label).expect("painted under default");
        assert!(
            (painted.x - default_painted.x - shift).abs() < 0.5,
            "{label:?} moves by the live Style's extra row padding: {painted:?} vs {default_painted:?}"
        );
        let row = [0usize, 1, 3][i];
        assert_eq!(
            layout.hit_test(
                painted.x + painted.width / 2.0,
                painted.y + painted.height / 2.0
            ),
            quadraui::TreeViewHit::Row(row),
            "a click on painted {label:?} lands on its row"
        );
    }
    let sb = layout.vscrollbar.expect("20 rows overflow 120px");
    assert_eq!(sb.track.width, 18.0, "the track is Style::scrollbar_width");
}

// ── StatusBar ───────────────────────────────────────────────────────────

const STRIP_TEXT: &str = "Close tab (Ctrl+W)";

fn strip() -> StatusBar {
    StatusBar {
        id: WidgetId::new("fidelity:strip"),
        left_segments: vec![StatusBarSegment {
            text: STRIP_TEXT.into(),
            fg: quadraui::Color::rgb(230, 230, 230),
            bg: quadraui::Color::rgb(40, 40, 40),
            bold: false,
            action_id: None,
        }],
        right_segments: vec![],
    }
}

/// The natural width `StatusBar::measure` reports on `b`.
fn natural_width(b: &dyn Backend) -> f32 {
    let style = b.style();
    let measurer = |text: &str, role: FontRole| b.measure_text(text, role).0;
    let ctx = LayoutCtx::new(b.line_height(), b.char_width(), false)
        .with_measurer(&measurer)
        .with_style(&style);
    Primitive::measure(&strip(), &ctx, Size::new(0.0, 0.0))
        .width
        .expect("a status bar reports a natural width")
}

#[test]
fn status_bar_sized_to_its_natural_width_paints_its_text_in_full() {
    let fits = drive(Style::default(), |b| {
        let w = natural_width(b);
        let _ = b.draw_dyn(Rect::new(10.0, 10.0, w, 24.0), &strip(), &());
    });
    assert!(
        fits.painted_texts().contains(&STRIP_TEXT),
        "a strip at its natural width paints {STRIP_TEXT:?} untruncated: {:?}",
        fits.painted_texts()
    );

    let narrower = drive(Style::default(), |b| {
        let w = natural_width(b) - 12.0;
        let _ = b.draw_dyn(Rect::new(10.0, 10.0, w, 24.0), &strip(), &());
    });
    assert!(
        !narrower.painted_texts().contains(&STRIP_TEXT),
        "the natural width is the narrowest that fits: {:?}",
        narrower.painted_texts()
    );
}

// ── FindReplacePanel ────────────────────────────────────────────────────

fn panel() -> FindReplacePanel {
    let (hit_regions, _) = quadraui::compute_find_replace_hit_regions(50, true, "1 of 3", 2, 2);
    FindReplacePanel {
        id: WidgetId::new("find_replace"),
        query: "needle".into(),
        replacement: "thread".into(),
        show_replace: true,
        focus: 0,
        cursor: 6,
        sel_anchor: None,
        match_info: "1 of 3".into(),
        case_sensitive: false,
        whole_word: false,
        use_regex: false,
        preserve_case: false,
        in_selection: false,
        group_bounds: Rect::new(0.0, 0.0, W as f32, H as f32),
        panel_width: 50,
        replace_one_glyph: "R1".into(),
        replace_all_glyph: "R*".into(),
        hit_regions,
        hidden_toggles: Vec::new(),
    }
}

#[test]
fn find_replace_paints_with_the_backend_style_its_host_hit_tests_with() {
    let style = Style::default()
        .with_control_height(44.0)
        .with_padding(14.0);
    let bounds = Rect::new(0.0, 0.0, W as f32, H as f32);
    let driver = drive(style, |b| {
        let rect = panel().anchored_rect(b, bounds);
        let _ = b.draw_dyn(rect, &panel(), &());
    });
    let backend = driver.backend();
    let cw = backend.char_width().max(1.0);
    let popup = panel().anchored_rect(backend, bounds);
    let metrics =
        FindReplaceMetrics::for_popup(popup, &backend.style(), backend.chrome_line_height());
    assert_eq!(metrics.row_height, 44.0);

    for (text, want) in [("needle", 0usize), ("thread", 1usize)] {
        let (x, y) = driver
            .find(text)
            .unwrap_or_else(|| panic!("{text:?} painted: {:?}", driver.painted_texts()));
        let hit = panel().hit_test(x, y, metrics.content_origin, cw, metrics.row_height);
        let landed = match hit {
            FindReplaceHit::Target(FindReplaceClickTarget::FindInput(_)) => Some(0),
            FindReplaceHit::Target(FindReplaceClickTarget::ReplaceInput(_)) => Some(1),
            _ => None,
        };
        assert_eq!(
            landed,
            Some(want),
            "a click on painted {text:?} at ({x}, {y}) resolves, with the backend's style, \
             to the input it is painted in; got {hit:?}"
        );
    }
}
