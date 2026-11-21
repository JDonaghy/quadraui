//! Op-recording parity tests for `ListView`/`TreeView`/`DataTable`/
//! `TreeTable`'s [`Primitive::paint_cells`] path.
//!
//! Each primitive's TUI rasteriser (`quadraui::tui::draw_list`/`draw_tree`/
//! `draw_data_table`/`draw_tree_table`) predates the `Primitive` trait and
//! stays the "legacy shared-painter call" every one of these widgets'
//! `Primitive::paint_cells` override delegates straight to (see each impl's
//! own doc comment in `quadraui/src/primitives/{list,tree,data_table,
//! tree_table}.rs`). This file is a separate compilation unit — a widget
//! driven purely through the public `Primitive::layout` + `Primitive::
//! paint_cells` seam outside the crate — so a byte-for-byte equal
//! `ratatui::buffer::Buffer` from the two call paths is real evidence the
//! seam paints exactly what the pre-`Primitive` rasteriser always did, not
//! evidence the in-crate pilot forgot to update one of two copies.
//!
//! Every fixture below is deliberately non-default — a mid-list selected
//! row, `has_focus: true` — rather than an all-defaults widget, so the
//! comparison exercises the selection/focus highlighting path, not just an
//! empty frame.
//!
//! ## Why this covers TUI only, not GTK/macOS/Win
//!
//! The GTK/macOS/Win pixel-paint equivalent of this comparison
//! (`Primitive::paint` against each primitive's `native_surface_paint::
//! paint`/`paint_with_state`) already exists, in-crate, as each primitive's
//! own `primitive_parity_tests` module — `native_surface_paint` is
//! `pub(crate)`, reachable from those in-crate tests but not from this
//! external integration-test crate. Duplicating that coverage here would
//! need `native_surface_paint` widened to `pub`, a separate, reviewable
//! public-API change this file does not make.
//!
//! Every one of these four `Primitive::paint`/`paint_cells` overrides also
//! hardcodes "no hover, no press, no focus ring" when it calls through to
//! the shared painter — `Primitive`'s fixed `fn paint(&self, layout,
//! surface, theme, style)` / `fn paint_cells(&self, layout, buf, theme)`
//! signatures (`quadraui/src/primitive.rs`) carry no channel for per-call
//! interaction state, so there is no hovered/pressed row for this seam to
//! paint today. The "non-default" state these tests exercise is therefore
//! the data each widget itself carries (`selected_idx`/`selected_path`,
//! `has_focus`) rather than ephemeral hover/press, which both call paths
//! agree is simply absent.

#![cfg(feature = "tui")]

use ratatui::buffer::Buffer;
use ratatui::layout::Rect as CellRect;

use quadraui::primitive::{LayoutCtx, Primitive};
use quadraui::{
    Column, ColumnAlign, ColumnWidth, DataRow, DataTable, Decoration, ListItem, ListView, Rect,
    SelectionMode, SortDirection, StyledText, Theme, TreeRow, TreeStyle, TreeTable, TreeTableNode,
    TreeView, WidgetId,
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
fn ctx() -> LayoutCtx {
    LayoutCtx {
        line_height: 1.0,
        char_width: 1.0,
        nerd_fonts_enabled: true,
    }
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
    }
}

fn sample_tree_table() -> TreeTable {
    let mut table = sample_table();
    table.id = WidgetId::new("files");
    let tree = (0..table.rows.len())
        .map(|i| TreeTableNode::new(vec![i as u16], 0))
        .collect();
    TreeTable { table, tree }
}

/// The acceptance criterion: `ListView`'s [`Primitive::layout`] +
/// [`Primitive::paint_cells`] must produce the exact same cell buffer as
/// calling [`quadraui::tui::draw_list`] directly with the equivalent
/// arguments.
#[test]
fn list_paint_cells_matches_draw_list() {
    let list = sample_list();
    let theme = Theme::default();
    let ctx = ctx();

    let layout = Primitive::layout(&list, &ctx, area());
    let mut via_primitive = Buffer::empty(cell_area());
    Primitive::paint_cells(&list, &layout, &mut via_primitive, &theme);

    let mut direct = Buffer::empty(cell_area());
    quadraui::tui::draw_list(
        &mut direct,
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
    assert_eq!(via_primitive, direct);
}

/// Same acceptance criterion as [`list_paint_cells_matches_draw_list`],
/// for `TreeView` against [`quadraui::tui::draw_tree`].
#[test]
fn tree_paint_cells_matches_draw_tree() {
    let tree = sample_tree();
    let theme = Theme::default();
    let ctx = ctx();

    let layout = Primitive::layout(&tree, &ctx, area());
    let mut via_primitive = Buffer::empty(cell_area());
    Primitive::paint_cells(&tree, &layout, &mut via_primitive, &theme);

    let mut direct = Buffer::empty(cell_area());
    quadraui::tui::draw_tree(
        &mut direct,
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
    assert_eq!(via_primitive, direct);
}

/// Same acceptance criterion as [`list_paint_cells_matches_draw_list`],
/// for `DataTable` against [`quadraui::tui::draw_data_table`] — called
/// with `hovered_idx: None`, matching exactly what
/// `DataTable`'s `Primitive::paint_cells` itself passes (see that impl's
/// own doc comment for why hover state never reaches it).
#[test]
fn data_table_paint_cells_matches_draw_data_table() {
    let table = sample_table();
    let theme = Theme::default();
    let ctx = ctx();

    let layout = Primitive::layout(&table, &ctx, area());
    let mut via_primitive = Buffer::empty(cell_area());
    Primitive::paint_cells(&table, &layout, &mut via_primitive, &theme);

    let mut direct = Buffer::empty(cell_area());
    quadraui::tui::draw_data_table(&mut direct, cell_area(), &table, &theme, None);

    assert_ne!(
        via_primitive,
        Buffer::empty(cell_area()),
        "the table must paint something for this comparison to be meaningful"
    );
    assert_eq!(via_primitive, direct);
}

/// Same acceptance criterion as [`list_paint_cells_matches_draw_list`],
/// for `TreeTable` against [`quadraui::tui::draw_tree_table`] — same
/// `hovered_idx: None` note as
/// [`data_table_paint_cells_matches_draw_data_table`].
#[test]
fn tree_table_paint_cells_matches_draw_tree_table() {
    let tt = sample_tree_table();
    let theme = Theme::default();
    let ctx = ctx();

    let layout = Primitive::layout(&tt, &ctx, area());
    let mut via_primitive = Buffer::empty(cell_area());
    Primitive::paint_cells(&tt, &layout, &mut via_primitive, &theme);

    let mut direct = Buffer::empty(cell_area());
    quadraui::tui::draw_tree_table(
        &mut direct,
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
    assert_eq!(via_primitive, direct);
}
