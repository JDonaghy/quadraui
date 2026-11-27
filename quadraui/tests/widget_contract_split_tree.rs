#![cfg(feature = "tui")]
//! Black-box `TuiDriver` test for `SplitTree` on the widget contract: a
//! nested tree reaches the frame as a type-erased `Surface::Widget`,
//! paints every divider and the junction where they meet through
//! `Primitive::paint_cells`, lays its leaves out against the painted
//! divider cells, and a drag on any divider routes through
//! `Interactive::handle` back out as `UiEvent::SplitDividerDragged`
//! carrying that divider's pre-order index.

use quadraui::primitive::{LayoutCtx, Primitive, Units};
use quadraui::tui::testing::TuiDriver;
use quadraui::{
    AppLogic, Backend, FrameTree, Reaction, Rect, ScreenLayout, SplitDirection, SplitTree,
    Surface, UiEvent, WidgetId,
};

const COLS: u16 = 50;
const ROWS: u16 = 14;
const AREA: Rect = Rect::new(2.0, 1.0, 41.0, 11.0);

fn tree_id() -> WidgetId {
    WidgetId::new("split-tree:groups")
}

/// `Split(Horizontal, Split(Vertical, a, c), b)`: a side-by-side divider
/// with a stacked one in its left half.
fn nested(outer: f32, inner: f32) -> SplitTree {
    SplitTree::split(
        SplitDirection::Horizontal,
        outer,
        SplitTree::split(
            SplitDirection::Vertical,
            inner,
            SplitTree::leaf(WidgetId::new("a")),
            SplitTree::leaf(WidgetId::new("c")),
        ),
        SplitTree::leaf(WidgetId::new("b")),
    )
}

struct TreeApp {
    tree: SplitTree,
    drags: Vec<(usize, f32)>,
}

impl AppLogic for TreeApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let mut frame = ScreenLayout::new();
        frame.push(Surface::Widget {
            rect: AREA,
            widget: Box::new(self.tree.clone()),
        });
        frame.draw(backend);
    }

    fn frame_tree(&self, _area: ()) -> FrameTree {
        let mut tree = FrameTree::new();
        tree.push(tree_id(), AREA, Box::new(self.tree.clone()));
        tree
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        if let UiEvent::SplitDividerDragged {
            tree,
            split_index,
            new_ratio,
        } = event
        {
            if tree == tree_id() {
                self.drags.push((split_index, new_ratio));
                self.tree.set_ratio_at_index(split_index, new_ratio);
                return Reaction::Redraw;
            }
        }
        Reaction::Continue
    }
}

fn driver() -> TuiDriver<TreeApp> {
    TuiDriver::new(
        TreeApp {
            tree: nested(0.5, 0.5),
            drags: Vec::new(),
        },
        COLS,
        ROWS,
    )
}

fn cell(driver: &TuiDriver<TreeApp>, x: u16, y: u16) -> char {
    driver.styled_row(y)[x as usize].0
}

fn cells_ctx() -> LayoutCtx<'static> {
    let ctx = LayoutCtx::new(1.0, 1.0, false);
    assert_eq!(ctx.units(), Units::Cells);
    ctx
}

#[test]
fn surface_widget_tree_paints_both_dividers_and_their_junction() {
    let driver = driver();
    let outer_x = AREA.x as u16 + 20;
    // The stacked divider: 11 rows less one leaves 10, half is row 5.
    let inner_y = AREA.y as u16 + 5;
    assert_eq!(cell(&driver, outer_x, AREA.y as u16), '│', "{}", driver.screen());
    assert_eq!(cell(&driver, AREA.x as u16, inner_y), '─', "{}", driver.screen());
    assert_eq!(cell(&driver, outer_x, inner_y), '┤', "{}", driver.screen());
}

/// The cell layout the widget paints from is the one a host reads leaf
/// rects off: an odd-width left half puts its stacked divider at a
/// fractional position, and no leaf may cover any divider cell.
#[test]
fn leaf_rects_end_at_the_painted_divider_cells() {
    let tree = nested(0.37, 0.5);
    let layout = Primitive::layout(&tree, &cells_ctx(), AREA);
    for d in &layout.dividers {
        let band = d.band();
        for (id, leaf) in &layout.leaves {
            let overlap_w = (leaf.x + leaf.width).min(band.x + band.width) - leaf.x.max(band.x);
            let overlap_h = (leaf.y + leaf.height).min(band.y + band.height) - leaf.y.max(band.y);
            assert!(
                overlap_w <= 0.0 || overlap_h <= 0.0,
                "leaf {id:?} {leaf:?} covers divider {band:?}"
            );
        }
    }
    let (_, b) = layout.leaves[2];
    assert_eq!(b.x, layout.dividers[0].band().x + 1.0);
}

#[test]
fn dragging_the_inner_divider_reports_its_index_and_lands_under_the_pointer() {
    let mut driver = driver();
    let x = AREA.x + 4.0;
    driver.mouse_down(x, AREA.y + 5.0);
    driver.mouse_move(x, AREA.y + 8.0);
    driver.mouse_up(x, AREA.y + 8.0);
    assert_eq!(driver.app().drags.len(), 1);
    assert_eq!(driver.app().drags[0].0, 1);
    driver.render();
    assert_eq!(
        cell(&driver, AREA.x as u16, AREA.y as u16 + 8),
        '─',
        "{}",
        driver.screen()
    );
}

#[test]
fn dragging_the_outer_divider_reports_index_zero() {
    let mut driver = driver();
    let y = AREA.y + 2.0;
    driver.mouse_down(AREA.x + 20.0, y);
    driver.mouse_move(AREA.x + 10.0, y);
    driver.mouse_up(AREA.x + 10.0, y);
    assert_eq!(driver.app().drags.len(), 1);
    let (index, ratio) = driver.app().drags[0];
    assert_eq!(index, 0);
    assert!((ratio - 0.25).abs() < 1e-4, "ratio {ratio}");
}

#[test]
fn a_press_inside_a_leaf_does_not_drag() {
    let mut driver = driver();
    driver.mouse_down(AREA.x + 30.0, AREA.y + 3.0);
    driver.mouse_move(AREA.x + 10.0, AREA.y + 3.0);
    driver.mouse_up(AREA.x + 10.0, AREA.y + 3.0);
    assert!(driver.app().drags.is_empty());
}
