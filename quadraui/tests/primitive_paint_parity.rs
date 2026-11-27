//! Parity between `ListView`/`TreeView`/`DataTable`/`TreeTable`'s
//! [`Primitive::paint_cells`] path and their TUI free-function
//! rasterisers (`quadraui::tui::draw_list`/`draw_tree`/`draw_data_table`/
//! `draw_tree_table`), driven entirely through public API from a
//! separate compilation unit (`tests/`, outside the crate).
//!
//! Each test below does two things:
//!
//! 1. Asserts that `Primitive::layout` + `Primitive::paint_cells`
//!    produces a byte-for-byte equal `ratatui::buffer::Buffer` to
//!    calling the widget's plain `draw_*` function directly, for a
//!    fixture that is deliberately non-default on the data each widget
//!    itself carries (a mid-list selected row, `has_focus: true`).
//! 2. Asserts the opposite for *interaction* state a widget does not
//!    carry on itself — pin down, with `assert_ne!`, exactly where that
//!    state is lost. See "Why interaction state diverges" below.
//!
//! ## Why interaction state diverges
//!
//! [`Primitive::paint`]/[`Primitive::paint_cells`]'s signatures
//! (`quadraui::primitive::Primitive`) take only `&self`, `Self::Layout`,
//! a paint target and a `Theme` — no parameter for per-call interaction
//! state. Every one of these four `paint_cells` overrides therefore
//! hardcodes a default [`crate::primitives::row_stack::RowPaintState`]
//! (aliased per widget as `ListPaintState`/`TreePaintState`/
//! `DataTablePaintState`) when it calls through to its own `draw_*`,
//! even though the TUI free functions below accept a real one:
//!
//! - [`quadraui::tui::draw_data_table_with_state`] /
//!   [`quadraui::tui::draw_tree_table`]'s `hovered_idx` tints a
//!   non-selected row's background —
//!   `data_table_paint_cells_matches_draw_data_table_then_diverges_under_hover`
//!   and
//!   `tree_table_paint_cells_matches_draw_tree_table_then_diverges_under_hover`
//!   below show the resulting buffers are not equal.
//! - [`quadraui::tui::draw_list_with_state`] /
//!   [`quadraui::tui::draw_tree_with_scroll_px`]'s `scroll_px` wins over
//!   `scroll_offset` and can land on a different first visible row —
//!   `list_paint_cells_matches_draw_list_then_diverges_under_scroll_px`
//!   and
//!   `tree_paint_cells_matches_draw_tree_then_diverges_under_scroll_px`
//!   below show the same thing for scroll position.
//!
//! `pressed_idx` is omitted from every divergence test: no TUI
//! rasteriser in this crate reads it today (confirmed by grep across
//! `src/tui/`), so passing it can't demonstrate anything
//! `Primitive`-specific — the gap there is backend-wide, not a seam
//! this file can isolate.
//!
//! A pixel-backend (GTK/macOS/Win) equivalent of these divergence tests
//! — comparing [`Primitive::paint`] against each primitive's own
//! `native_surface_paint::paint_with_state` — isn't constructible from
//! here: that function is `pub(crate)` for every one of these four
//! widgets, unreachable from this external `tests/` crate. The in-crate
//! `primitive_parity_tests` module next to each primitive's own
//! `native_surface_paint` covers that side instead.

#![cfg(feature = "tui")]

use ratatui::buffer::Buffer;
use ratatui::layout::Rect as CellRect;

use quadraui::primitive::{LayoutCtx, Primitive};
use quadraui::{
    Column, ColumnAlign, ColumnWidth, DataRow, DataTable, DataTablePaintState, Decoration,
    ListItem, ListPaintState, ListView, Rect, SelectionMode, SortDirection, StyledText, Theme,
    TreeRow, TreeStyle, TreeTable, TreeTableNode, TreeView, WidgetId,
};

/// Absolute area every fixture is laid out into, in both unit systems —
/// integer-valued so the pixel `Rect`/cell `CellRect` conversion is exact,
/// with no rounding to drift the two call paths apart.
fn area() -> Rect {
    Rect::new(2.0, 1.0, 30.0, 8.0)
}

fn cell_area() -> CellRect {
    let a = area();
    CellRect::new(a.x as u16, a.y as u16, a.width as u16, a.height as u16)
}

/// TUI's own [`LayoutCtx`] shape — `line_height`/`char_width` both `1.0`
/// cell, the runtime discriminator every `Primitive::layout` impl in this
/// crate uses to pick its TUI branch over a pixel-backend one (see e.g.
/// `ListView`'s own `Primitive::layout` doc).
fn ctx() -> LayoutCtx<'static> {
    LayoutCtx::new(1.0, 1.0, true)
}

fn sample_list() -> ListView {
    ListView {
        id: WidgetId::new("quickfix"),
        title: None,
        items: (0..5)
            .map(|i| ListItem {
                text: StyledText::plain(format!("row {i}")),
                icon: None,
                detail: None,
                decoration: Decoration::default(),
            })
            .collect(),
        selected_idx: 2,
        scroll_offset: 0,
        has_focus: true,
        bordered: false,
        h_scroll: 0,
        max_content_width: None,
        show_v_scrollbar: false,
        striped: false,
    }
}

fn tree_row(path: &[u16], label: &str) -> TreeRow {
    TreeRow {
        path: path.to_vec(),
        indent: 0,
        icon: None,
        text: StyledText::plain(label),
        badge: None,
        is_expanded: None,
        decoration: Decoration::default(),
        edit: None,
    }
}

fn sample_tree() -> TreeView {
    TreeView {
        id: WidgetId::new("explorer"),
        rows: vec![
            tree_row(&[0], "alpha"),
            tree_row(&[1], "beta"),
            tree_row(&[2], "gamma"),
        ],
        selection_mode: SelectionMode::Single,
        selected_path: Some(vec![2]),
        scroll_offset: 0,
        style: TreeStyle::default(),
        has_focus: true,
    }
}

fn sample_table() -> DataTable {
    DataTable {
        id: WidgetId::new("pods"),
        columns: vec![
            Column {
                title: "Name".to_string(),
                width: ColumnWidth::Flex(2.0),
                align: ColumnAlign::Left,
            },
            Column {
                title: "Age".to_string(),
                width: ColumnWidth::FixedCells(5.0),
                align: ColumnAlign::Right,
            },
        ],
        rows: (0..5)
            .map(|i| DataRow {
                cells: vec![
                    StyledText::plain(format!("pod-{i}")),
                    StyledText::plain("3d"),
                ],
                decoration: Decoration::default(),
            })
            .collect(),
        selected_idx: Some(2),
        scroll_offset: 0,
        sort: Some((0, SortDirection::Ascending)),
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

fn sample_tree_table() -> TreeTable {
    let mut table = sample_table();
    table.id = WidgetId::new("files");
    let tree = (0..table.rows.len())
        .map(|i| TreeTableNode::new(vec![i as u16], 0))
        .collect();
    TreeTable::new(table, tree)
}

/// A theme whose `tab_bar_bg` (the colour `DataTable`'s hover tint
/// paints with) differs from `background` — [`Theme::default`]'s own
/// `tab_bar_bg` is set equal to `background`, which would make a
/// hovered, non-selected row paint pixel-identically to an un-hovered
/// one and defeat the divergence this file pins down.
fn theme_with_distinct_hover_bg() -> Theme {
    Theme {
        tab_bar_bg: quadraui::Color::rgb(90, 95, 110),
        ..Theme::default()
    }
}

/// `ListView`: [`Primitive::layout`] + [`Primitive::paint_cells`] match
/// [`quadraui::tui::draw_list`] exactly at the default interaction
/// state, and diverge from [`quadraui::tui::draw_list_with_state`] once
/// `scroll_px` names a different first visible row than `scroll_offset`
/// does — a state `paint_cells` has no parameter to carry.
#[test]
fn list_paint_cells_matches_draw_list_then_diverges_under_scroll_px() {
    let list = sample_list();
    let theme = Theme::default();
    let ctx = ctx();

    let layout = Primitive::layout(&list, &ctx, area());
    let mut via_primitive = Buffer::empty(cell_area());
    Primitive::paint_cells(
        &list,
        &layout,
        &mut quadraui::tui::BufferCellSurface::new(&mut via_primitive),
        &quadraui::PaintEnv::new(&theme, &quadraui::Style::default()),
    );

    let mut direct_default = Buffer::empty(cell_area());
    quadraui::tui::draw_list(
        &mut direct_default,
        cell_area(),
        &list,
        &theme,
        ctx.nerd_fonts_enabled,
    );

    assert_ne!(
        via_primitive,
        Buffer::empty(cell_area()),
        "the list must paint something for this comparison to be meaningful"
    );
    assert_eq!(
        via_primitive, direct_default,
        "Primitive::paint_cells must match quadraui::tui::draw_list at the default state"
    );

    let state = ListPaintState::default().with_scroll_px(Some(1.0));
    let mut direct_scrolled = Buffer::empty(cell_area());
    quadraui::tui::draw_list_with_state(
        &mut direct_scrolled,
        cell_area(),
        &list,
        &theme,
        ctx.nerd_fonts_enabled,
        &state,
    );
    assert_ne!(
        via_primitive, direct_scrolled,
        "Primitive::paint_cells has no scroll_px parameter, so it can never match \
         draw_list_with_state once scroll_px names a different first visible row"
    );
}

/// Same shape as [`list_paint_cells_matches_draw_list_then_diverges_under_scroll_px`],
/// for `TreeView` against [`quadraui::tui::draw_tree`] /
/// [`quadraui::tui::draw_tree_with_scroll_px`].
#[test]
fn tree_paint_cells_matches_draw_tree_then_diverges_under_scroll_px() {
    let tree = sample_tree();
    let theme = Theme::default();
    let ctx = ctx();

    let layout = Primitive::layout(&tree, &ctx, area());
    let mut via_primitive = Buffer::empty(cell_area());
    Primitive::paint_cells(
        &tree,
        &layout,
        &mut quadraui::tui::BufferCellSurface::new(&mut via_primitive),
        &quadraui::PaintEnv::new(&theme, &quadraui::Style::default()),
    );

    let mut direct_default = Buffer::empty(cell_area());
    quadraui::tui::draw_tree(
        &mut direct_default,
        cell_area(),
        &tree,
        &theme,
        ctx.nerd_fonts_enabled,
    );

    assert_ne!(
        via_primitive,
        Buffer::empty(cell_area()),
        "the tree must paint something for this comparison to be meaningful"
    );
    assert_eq!(
        via_primitive, direct_default,
        "Primitive::paint_cells must match quadraui::tui::draw_tree at the default state"
    );

    let mut direct_scrolled = Buffer::empty(cell_area());
    quadraui::tui::draw_tree_with_scroll_px(
        &mut direct_scrolled,
        cell_area(),
        &tree,
        &theme,
        ctx.nerd_fonts_enabled,
        Some(1.0),
    );
    assert_ne!(
        via_primitive, direct_scrolled,
        "Primitive::paint_cells has no scroll_px parameter, so it can never match \
         draw_tree_with_scroll_px once scroll_px names a different first visible row"
    );
}

/// `DataTable`: [`Primitive::layout`] + [`Primitive::paint_cells`] match
/// [`quadraui::tui::draw_data_table`] exactly at `hovered_idx: None`
/// (what `DataTable`'s `Primitive::paint_cells` itself passes), and
/// diverge from [`quadraui::tui::draw_data_table_with_state`] once a
/// non-selected row is hovered — a state `paint_cells` has no parameter
/// to carry.
#[test]
fn data_table_paint_cells_matches_draw_data_table_then_diverges_under_hover() {
    let table = sample_table();
    let theme = theme_with_distinct_hover_bg();
    let ctx = ctx();

    let layout = Primitive::layout(&table, &ctx, area());
    let mut via_primitive = Buffer::empty(cell_area());
    Primitive::paint_cells(
        &table,
        &layout,
        &mut quadraui::tui::BufferCellSurface::new(&mut via_primitive),
        &quadraui::PaintEnv::new(&theme, &quadraui::Style::default()),
    );

    let mut direct_default = Buffer::empty(cell_area());
    quadraui::tui::draw_data_table(&mut direct_default, cell_area(), &table, &theme, None);

    assert_ne!(
        via_primitive,
        Buffer::empty(cell_area()),
        "the table must paint something for this comparison to be meaningful"
    );
    assert_eq!(
        via_primitive, direct_default,
        "Primitive::paint_cells must match quadraui::tui::draw_data_table at hovered_idx: None"
    );

    // Row 1, not row 2 (`table.selected_idx`) — hover tinting is
    // suppressed on the selected row, so hovering a different row is
    // what actually exercises it.
    let state = DataTablePaintState::default().with_hovered_idx(Some(1));
    let mut direct_hovered = Buffer::empty(cell_area());
    quadraui::tui::draw_data_table_with_state(
        &mut direct_hovered,
        cell_area(),
        &table,
        &theme,
        &state,
    );
    assert_ne!(
        via_primitive, direct_hovered,
        "Primitive::paint_cells has no hovered_idx parameter, so it can never match \
         draw_data_table_with_state once a non-selected row is hovered"
    );
}

/// Same shape as
/// [`data_table_paint_cells_matches_draw_data_table_then_diverges_under_hover`],
/// for `TreeTable` against [`quadraui::tui::draw_tree_table`]'s own
/// `hovered_idx` parameter (`TreeTable` has no `_with_state` free
/// function of its own — `hovered_idx` is positional here, same as
/// `draw_data_table`'s).
#[test]
fn tree_table_paint_cells_matches_draw_tree_table_then_diverges_under_hover() {
    let tt = sample_tree_table();
    let theme = theme_with_distinct_hover_bg();
    let ctx = ctx();

    let layout = Primitive::layout(&tt, &ctx, area());
    let mut via_primitive = Buffer::empty(cell_area());
    Primitive::paint_cells(
        &tt,
        &layout,
        &mut quadraui::tui::BufferCellSurface::new(&mut via_primitive),
        &quadraui::PaintEnv::new(&theme, &quadraui::Style::default()),
    );

    let mut direct_default = Buffer::empty(cell_area());
    quadraui::tui::draw_tree_table(
        &mut direct_default,
        cell_area(),
        &tt,
        &theme,
        None,
        ctx.nerd_fonts_enabled,
    );

    assert_ne!(
        via_primitive,
        Buffer::empty(cell_area()),
        "the tree table must paint something for this comparison to be meaningful"
    );
    assert_eq!(
        via_primitive, direct_default,
        "Primitive::paint_cells must match quadraui::tui::draw_tree_table at hovered_idx: None"
    );

    // Row 1, not row 2 (`tt.table.selected_idx`) — same reasoning as
    // the `DataTable` test above.
    let mut direct_hovered = Buffer::empty(cell_area());
    quadraui::tui::draw_tree_table(
        &mut direct_hovered,
        cell_area(),
        &tt,
        &theme,
        Some(1),
        ctx.nerd_fonts_enabled,
    );
    assert_ne!(
        via_primitive, direct_hovered,
        "Primitive::paint_cells has no hovered_idx parameter, so it can never match \
         draw_tree_table once a non-selected row is hovered"
    );
}
