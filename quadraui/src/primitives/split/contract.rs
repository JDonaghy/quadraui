//! [`Split`] and [`SplitTree`] on the widget contract: layout on both
//! unit systems, the pixel and cell painters, and the divider drag.
//!
//! The two widgets share everything but their descriptor: a `Split` is
//! the two-leaf [`SplitTree`] [`Split::as_tree`] builds, so both lay out
//! through one recursive pass, paint their cells through one painter
//! ([`paint_tree_cells`]), and turn a drag into a ratio through one
//! inverse ([`crate::SplitTreeDivider::ratio_at`]).
//!
//! Neither widget paints its panes or takes keyboard focus: the panes are
//! the host's, and a divider is structural chrome reached with the
//! pointer. The pointer counts as on the widget only over its dividers
//! ([`Interactive::HOVER_ON_SUB_REGIONS_ONLY`]), so a press inside a pane
//! reaches whatever the host painted there.

use super::{Split, SplitDirection, SplitLayout};
use crate::event::{Point, Rect};
use crate::interactive::{
    EventCx, FocusSpec, Interactive, PointerKind, RoutedEvent, SubTarget, WidgetAction,
};
use crate::primitive::{LayoutCtx, PaintEnv, Primitive, Units};
use crate::primitives::split_tree::{SplitTree, SplitTreeLayout};
use crate::types::WidgetId;
use crate::{Handled, MouseButton, PointerShape, ResizeEdge, UiEvent};

/// A divider drag moved divider `split_index` to `new_ratio` — what both
/// widgets report while a drag is live. It becomes
/// [`UiEvent::SplitDividerDragged`]; the host writes the ratio back onto
/// the descriptor ([`Split::ratio`], or
/// [`SplitTree::set_ratio_at_index`]) before the next frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SplitDividerDrag {
    /// Pre-order index of the dragged divider; always `0` for a [`Split`].
    pub split_index: usize,
    /// The ratio that puts the divider under the pointer, `0.0..=1.0`.
    pub new_ratio: f32,
}

impl WidgetAction for SplitDividerDrag {
    fn into_ui_event(self, id: WidgetId) -> UiEvent {
        UiEvent::SplitDividerDragged {
            tree: id,
            split_index: self.split_index,
            new_ratio: self.new_ratio,
        }
    }
}

/// [`Split`]'s gesture state: whether a divider drag is live.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SplitState {
    dragging: bool,
}

impl SplitState {
    /// Whether the divider is being dragged.
    pub fn is_dragging(&self) -> bool {
        self.dragging
    }
}

/// [`SplitTree`]'s gesture state: which divider, if any, is being dragged.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SplitTreeState {
    dragging: Option<usize>,
}

impl SplitTreeState {
    /// The pre-order index of the divider being dragged.
    pub fn dragging(&self) -> Option<usize> {
        self.dragging
    }
}

/// The resize pointer over a divider of a split along `direction`.
fn resize_cursor(direction: SplitDirection) -> PointerShape {
    match direction {
        SplitDirection::Horizontal => PointerShape::Resize(ResizeEdge::East),
        SplitDirection::Vertical => PointerShape::Resize(ResizeEdge::South),
    }
}

/// `local` (relative to the widget's rect) in the absolute frame every
/// split layout is expressed in.
fn absolute(local: Point, bounds: Rect) -> Point {
    Point::new(local.x + bounds.x, local.y + bounds.y)
}

/// `rect` relative to `origin`'s top-left — the space
/// [`Interactive::sub_regions`] answers in.
fn relative(rect: Rect, origin: Rect) -> Rect {
    Rect::new(
        rect.x - origin.x,
        rect.y - origin.y,
        rect.width,
        rect.height,
    )
}

/// The divider-drag gesture both widgets share. `hit` resolves a press to
/// the pressed divider's `(split_index, direction)`; `ratio` resolves a
/// live drag's pointer to the dragged divider's new ratio.
fn handle_drag(
    dragging: &mut Option<usize>,
    event: &RoutedEvent,
    bounds: Rect,
    hit: impl Fn(Point) -> Option<(usize, SplitDirection)>,
    ratio: impl Fn(usize, Point) -> Option<(f32, SplitDirection)>,
    cx: &mut EventCx,
) -> Handled<SplitDividerDrag> {
    match event {
        RoutedEvent::Pointer {
            kind: PointerKind::Down(MouseButton::Left),
            local,
            ..
        } => match hit(absolute(*local, bounds)) {
            Some((split_index, direction)) => {
                *dragging = Some(split_index);
                cx.capture_pointer();
                cx.set_cursor(resize_cursor(direction));
                Handled::Consumed
            }
            None => Handled::Ignored,
        },
        RoutedEvent::Pointer {
            kind: PointerKind::Move,
            local,
            ..
        } => {
            let point = absolute(*local, bounds);
            match *dragging {
                Some(split_index) => match ratio(split_index, point) {
                    Some((new_ratio, direction)) => {
                        cx.set_cursor(resize_cursor(direction));
                        Handled::Event(SplitDividerDrag {
                            split_index,
                            new_ratio,
                        })
                    }
                    None => Handled::Consumed,
                },
                None => {
                    if let Some((_, direction)) = hit(point) {
                        cx.set_cursor(resize_cursor(direction));
                    }
                    Handled::Ignored
                }
            }
        }
        RoutedEvent::Pointer {
            kind: PointerKind::Up(MouseButton::Left),
            ..
        } => {
            if dragging.take().is_some() {
                cx.release_pointer();
                Handled::Consumed
            } else {
                Handled::Ignored
            }
        }
        _ => Handled::Ignored,
    }
}

impl Primitive for Split {
    type Layout = SplitLayout;
    type Event = SplitDividerDrag;

    fn id(&self) -> Option<&WidgetId> {
        Some(&self.id)
    }

    /// [`Split::cell_layout`] on a cell grid; on pixels, the shared
    /// pixel-backend divider thickness.
    fn layout(&self, ctx: &LayoutCtx, rect: Rect) -> SplitLayout {
        match ctx.units() {
            Units::Cells => self.cell_layout(rect),
            _ => crate::primitives::layout_metrics::pixel_split_layout(self, rect),
        }
    }

    fn paint(
        &self,
        layout: &SplitLayout,
        surface: &mut dyn crate::paint_surface::PaintSurface,
        env: &PaintEnv,
    ) {
        #[cfg(any(
            feature = "gtk",
            feature = "win",
            all(feature = "macos", target_os = "macos")
        ))]
        super::native_surface_paint::paint(layout, surface, env.theme());
        #[cfg(not(any(
            feature = "gtk",
            feature = "win",
            all(feature = "macos", target_os = "macos")
        )))]
        let _ = (layout, surface, env);
    }

    /// The [`SplitTree`] cell painter over the equivalent two-leaf tree.
    #[cfg(feature = "tui")]
    fn paint_cells(
        &self,
        layout: &SplitLayout,
        cells: &mut dyn crate::CellSurface,
        env: &PaintEnv,
    ) {
        paint_tree_cells(
            &self.as_tree().cell_layout(layout.bounds),
            cells,
            env.theme(),
        );
    }
}

impl Interactive for Split {
    type State = SplitState;
    type Action = SplitDividerDrag;

    const HOVER_ON_SUB_REGIONS_ONLY: bool = true;

    /// A divider is structural chrome: never a keyboard tab stop.
    fn focus(&self) -> FocusSpec {
        FocusSpec::None
    }

    /// The divider's hit band — the one part of the split the pointer
    /// lands on.
    fn sub_regions(&self, layout: &SplitLayout) -> Vec<(Rect, SubTarget)> {
        vec![(
            relative(layout.divider_hit_bounds(), layout.bounds),
            SubTarget(0),
        )]
    }

    /// A left press on the divider captures the pointer and starts a drag;
    /// each captured move reports the ratio that puts the divider under
    /// the pointer ([`SplitLayout::ratio_at`]); the release ends it. A
    /// press inside either pane is not the split's.
    fn handle(
        &self,
        state: &mut SplitState,
        event: &RoutedEvent,
        layout: &SplitLayout,
        cx: &mut EventCx,
    ) -> Handled<SplitDividerDrag> {
        let direction = self.direction;
        let mut dragging = state.dragging.then_some(0);
        let handled = handle_drag(
            &mut dragging,
            event,
            layout.bounds,
            |point| {
                layout
                    .divider_hit_bounds()
                    .contains(point)
                    .then_some((0, direction))
            },
            |_, point| Some((layout.ratio_at(point), direction)),
            cx,
        );
        state.dragging = dragging.is_some();
        handled
    }
}

impl Primitive for SplitTree {
    type Layout = SplitTreeLayout;
    type Event = SplitDividerDrag;

    /// [`SplitTree::cell_layout`] on a cell grid; on pixels, the shared
    /// pixel-backend divider thickness. Every leaf's rect is in the
    /// returned layout's `leaves`, the one source of pane rects for both
    /// the host's paint and its hit-testing.
    fn layout(&self, ctx: &LayoutCtx, rect: Rect) -> SplitTreeLayout {
        match ctx.units() {
            Units::Cells => self.cell_layout(rect),
            _ => crate::primitives::layout_metrics::pixel_split_tree_layout(self, rect),
        }
    }

    fn paint(
        &self,
        layout: &SplitTreeLayout,
        surface: &mut dyn crate::paint_surface::PaintSurface,
        env: &PaintEnv,
    ) {
        #[cfg(any(
            feature = "gtk",
            feature = "win",
            all(feature = "macos", target_os = "macos")
        ))]
        crate::primitives::split_tree::native_surface_paint::paint(layout, surface, env.theme());
        #[cfg(not(any(
            feature = "gtk",
            feature = "win",
            all(feature = "macos", target_os = "macos")
        )))]
        let _ = (layout, surface, env);
    }

    #[cfg(feature = "tui")]
    fn paint_cells(
        &self,
        layout: &SplitTreeLayout,
        cells: &mut dyn crate::CellSurface,
        env: &PaintEnv,
    ) {
        paint_tree_cells(layout, cells, env.theme());
    }
}

impl Interactive for SplitTree {
    type State = SplitTreeState;
    type Action = SplitDividerDrag;

    const HOVER_ON_SUB_REGIONS_ONLY: bool = true;

    /// A divider is structural chrome: never a keyboard tab stop.
    fn focus(&self) -> FocusSpec {
        FocusSpec::None
    }

    /// Every divider's hit band, outermost first, named by its pre-order
    /// `split_index`.
    fn sub_regions(&self, layout: &SplitTreeLayout) -> Vec<(Rect, SubTarget)> {
        layout
            .dividers
            .iter()
            .map(|d| {
                (
                    relative(d.band(), layout.bounds),
                    SubTarget(d.split_index as u64),
                )
            })
            .collect()
    }

    /// A left press on a divider captures the pointer and starts dragging
    /// it (the outermost divider wins where two meet); each captured move
    /// reports that divider's ratio under the pointer
    /// ([`crate::SplitTreeDivider::ratio_at`]); the release ends it.
    fn handle(
        &self,
        state: &mut SplitTreeState,
        event: &RoutedEvent,
        layout: &SplitTreeLayout,
        cx: &mut EventCx,
    ) -> Handled<SplitDividerDrag> {
        handle_drag(
            &mut state.dragging,
            event,
            layout.bounds,
            |point| {
                layout
                    .dividers
                    .iter()
                    .find(|d| d.band().contains(point))
                    .map(|d| (d.split_index, d.direction))
            },
            |split_index, point| {
                let d = layout
                    .dividers
                    .iter()
                    .find(|d| d.split_index == split_index)?;
                Some((d.ratio_at(point)?, d.direction))
            },
            cx,
        )
    }
}

/// The one cell painter for split dividers: a `│` column per
/// side-by-side divider and a `─` row per stacked one, each at the whole
/// cell [`crate::SplitTreeDivider::cell_position`] names (the cell
/// [`SplitTreeLayout::hit_test_divider_cell`] compares against), then
/// every run's junctions upgraded (`┼ ├ ┤ ┬ ┴`).
///
/// Two phases, not one pass per divider: an outer divider, whose run
/// spans the full cross axis and so holds the shared boundary cell, is
/// visited before its shorter inner dividers in pre-order. Painting every
/// plain run first and upgrading junctions over the complete grid second
/// makes the result independent of that order.
#[cfg(feature = "tui")]
pub(crate) fn paint_tree_cells(
    layout: &SplitTreeLayout,
    cells: &mut dyn crate::CellSurface,
    theme: &crate::Theme,
) {
    if layout.bounds.width < 1.0 || layout.bounds.height < 1.0 {
        return;
    }
    let style = crate::cell_surface::solid_style(theme.separator, theme.background);
    let mut runs: Vec<(Vec<(u16, u16)>, bool)> = Vec::with_capacity(layout.dividers.len());
    for div in &layout.dividers {
        let axis_cell = div.cell_position();
        let cross_start = div.cross_start.round() as u16;
        let cross_len = div.cross_size.round() as u16;
        let (run, vertical, glyph): (Vec<(u16, u16)>, bool, char) = match div.direction {
            SplitDirection::Horizontal => (
                (0..cross_len)
                    .map(|d| (axis_cell, cross_start + d))
                    .collect(),
                true,
                '│',
            ),
            SplitDirection::Vertical => (
                (0..cross_len)
                    .map(|d| (cross_start + d, axis_cell))
                    .collect(),
                false,
                '─',
            ),
        };
        for &(x, y) in &run {
            cells.put_char(x, y, glyph, style);
        }
        runs.push((run, vertical));
    }
    for (run, vertical) in &runs {
        super::junction::upgrade_junctions(cells, run, *vertical, style);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::split_tree::SplitTreeMeasure;

    fn hsplit(ratio: f32, thickness_cells: bool) -> (Split, SplitLayout) {
        let split = Split {
            id: WidgetId::new("s"),
            direction: SplitDirection::Horizontal,
            ratio,
            first_min: 0.0,
            second_min: 0.0,
        };
        let bounds = Rect::new(10.0, 3.0, 81.0, 20.0);
        let layout = if thickness_cells {
            split.cell_layout(bounds)
        } else {
            split.layout(bounds, super::super::SplitMeasure::new(4.0))
        };
        (split, layout)
    }

    #[test]
    fn split_ratio_at_inverts_layout_at_every_thickness() {
        for cells in [true, false] {
            let (split, layout) = hsplit(0.5, cells);
            for target in [12.0_f32, 30.0, 47.0, 80.0] {
                let ratio = layout.ratio_at(Point::new(target, 5.0));
                let moved = Split {
                    ratio,
                    ..split.clone()
                };
                let after = if cells {
                    moved.cell_layout(layout.bounds)
                } else {
                    moved.layout(layout.bounds, super::super::SplitMeasure::new(4.0))
                };
                assert!(
                    (after.divider_hit_bounds().x - target).abs() < 1e-3,
                    "cells={cells}: dragging to {target} landed at {:?}",
                    after.divider_hit_bounds()
                );
            }
        }
    }

    #[test]
    fn split_tree_ratio_at_inverts_layout_for_a_thick_divider() {
        let tree = SplitTree::split(
            SplitDirection::Vertical,
            0.5,
            SplitTree::leaf(WidgetId::new("a")),
            SplitTree::leaf(WidgetId::new("b")),
        );
        let bounds = Rect::new(0.0, 100.0, 50.0, 204.0);
        let layout = tree.layout(bounds, SplitTreeMeasure::new(4.0));
        let target = 250.0;
        let ratio = layout.dividers[0]
            .ratio_at(Point::new(0.0, target))
            .expect("room to drag");
        let mut moved = tree.clone();
        moved.set_ratio_at_index(0, ratio);
        let after = moved.layout(bounds, SplitTreeMeasure::new(4.0));
        assert!(
            (after.dividers[0].position - target).abs() < 1e-3,
            "divider landed at {} instead of {target}",
            after.dividers[0].position
        );
    }

    #[test]
    fn ratio_at_on_a_collapsed_parent_has_no_room() {
        let tree = SplitTree::split(
            SplitDirection::Horizontal,
            0.5,
            SplitTree::leaf(WidgetId::new("a")),
            SplitTree::leaf(WidgetId::new("b")),
        );
        let layout = tree.layout(Rect::new(0.0, 0.0, 1.0, 5.0), SplitTreeMeasure::new(1.0));
        assert_eq!(layout.dividers[0].ratio_at(Point::new(0.0, 0.0)), None);
    }

    /// A nested tree on a cell grid: every leaf ends at the painted
    /// divider cell beside it, so a leaf rect never covers a divider.
    #[test]
    fn cell_layout_leaves_never_cover_a_divider_cell() {
        let tree = SplitTree::split(
            SplitDirection::Horizontal,
            0.5,
            SplitTree::split(
                SplitDirection::Vertical,
                0.5,
                SplitTree::leaf(WidgetId::new("a")),
                SplitTree::leaf(WidgetId::new("c")),
            ),
            SplitTree::leaf(WidgetId::new("b")),
        );
        let layout = tree.cell_layout(Rect::new(3.0, 2.0, 80.0, 24.0));
        for d in &layout.dividers {
            let band = d.band();
            assert_eq!(band.x.fract(), 0.0);
            assert_eq!(band.y.fract(), 0.0);
            for (id, leaf) in &layout.leaves {
                let overlap_w = (leaf.x + leaf.width).min(band.x + band.width) - leaf.x.max(band.x);
                let overlap_h =
                    (leaf.y + leaf.height).min(band.y + band.height) - leaf.y.max(band.y);
                assert!(
                    overlap_w <= 0.0 || overlap_h <= 0.0,
                    "leaf {id:?} {leaf:?} covers divider {band:?}"
                );
            }
        }
    }

    /// The same nested tree laid out at a non-zero origin is the
    /// zero-origin layout translated, on cells and on pixels.
    #[test]
    fn layout_at_a_non_zero_origin_is_a_translation() {
        let tree = SplitTree::split(
            SplitDirection::Horizontal,
            0.37,
            SplitTree::split(
                SplitDirection::Vertical,
                0.61,
                SplitTree::leaf(WidgetId::new("a")),
                SplitTree::leaf(WidgetId::new("c")),
            ),
            SplitTree::leaf(WidgetId::new("b")),
        );
        let (ox, oy) = (7.0, 13.0);
        for cells in [true, false] {
            let at = |x: f32, y: f32| {
                let r = Rect::new(x, y, 81.0, 25.0);
                if cells {
                    tree.cell_layout(r)
                } else {
                    tree.layout(r, SplitTreeMeasure::new(4.0))
                }
            };
            let zero = at(0.0, 0.0);
            let moved = at(ox, oy);
            let translated = |a: Rect, b: Rect| {
                (a.x + ox - b.x).abs() < 1e-3
                    && (a.y + oy - b.y).abs() < 1e-3
                    && (a.width - b.width).abs() < 1e-3
                    && (a.height - b.height).abs() < 1e-3
            };
            for ((_, a), (_, b)) in zero.leaves.iter().zip(&moved.leaves) {
                assert!(translated(*a, *b), "cells={cells}: {a:?} vs {b:?}");
            }
            for (a, b) in zero.dividers.iter().zip(&moved.dividers) {
                assert!(
                    translated(a.band(), b.band()),
                    "cells={cells}: {a:?} vs {b:?}"
                );
            }
        }
    }
}

/// The cell painters, driven through `paint_cells` over a real buffer.
#[cfg(all(test, feature = "tui"))]
mod split_cell_tests {

    use super::*;
    use crate::primitives::split::SplitHit;
    use crate::style::Style;
    use crate::theme::Theme;
    use crate::tui::BufferCellSurface;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect as RRect;

    fn cell_rect(area: RRect) -> Rect {
        Rect::new(
            area.x as f32,
            area.y as f32,
            area.width as f32,
            area.height as f32,
        )
    }

    fn paint<W: Primitive>(w: &W, layout: &W::Layout, buf: &mut Buffer, theme: &Theme) {
        let style = Style::default();
        w.paint_cells(
            layout,
            &mut BufferCellSurface::new(buf),
            &PaintEnv::new(theme, &style),
        );
    }

    fn draw_split(buf: &mut Buffer, area: RRect, split: &Split, theme: &Theme) -> SplitLayout {
        let layout = split.cell_layout(cell_rect(area));
        paint(split, &layout, buf, theme);
        layout
    }

    fn cell_char(buf: &Buffer, x: u16, y: u16) -> char {
        buf[(x, y)].symbol().chars().next().unwrap_or(' ')
    }

    fn hsplit(ratio: f32) -> Split {
        Split {
            id: WidgetId::new("split"),
            direction: SplitDirection::Horizontal,
            ratio,
            first_min: 0.0,
            second_min: 0.0,
        }
    }

    fn vsplit(ratio: f32) -> Split {
        Split {
            id: WidgetId::new("split"),
            direction: SplitDirection::Vertical,
            ratio,
            first_min: 0.0,
            second_min: 0.0,
        }
    }

    // Parametrized over the area's origin — LESSONS.md "Layout helpers
    // must return coords in the same frame across backends".
    // `tui_split_layout` bakes `area.x`/`area.y`
    // straight into `split.layout`'s returned bounds (absolute frame),
    // so the regression guard is a full paint+hit_test round trip
    // re-run at a non-zero origin.

    fn horizontal_round_trip_at(origin_x: u16, origin_y: u16) {
        // Use 41 cols so available=40, 0.5*40=20 → divider at x=20, integer.
        let area = RRect::new(origin_x, origin_y, 41, 10);
        let mut buf = Buffer::empty(area);
        let split = hsplit(0.5);
        let layout = draw_split(&mut buf, area, &split, &Theme::default());

        let div_x = layout.divider_bounds.x.round() as u16;
        assert_eq!(cell_char(&buf, div_x, origin_y), '│');

        let mid_y = origin_y as f32 + 5.0;
        let hit = layout.hit_test(layout.divider_bounds.x + 0.5, mid_y);
        assert_eq!(hit, SplitHit::Divider(WidgetId::new("split")));

        let hit_first = layout.hit_test(origin_x as f32 + 1.0, mid_y);
        assert_eq!(hit_first, SplitHit::FirstPane(WidgetId::new("split")));

        let hit_second = layout.hit_test(layout.divider_bounds.x + 1.5, mid_y);
        assert_eq!(hit_second, SplitHit::SecondPane(WidgetId::new("split")));
    }

    #[test]
    fn horizontal_paint_and_click_round_trip() {
        horizontal_round_trip_at(0, 0);
    }

    /// Regression guard: non-zero-origin cases must be handled correctly.
    #[test]
    fn horizontal_paint_and_click_round_trip_at_nonzero_origin() {
        horizontal_round_trip_at(7, 13);
    }

    fn vertical_round_trip_at(origin_x: u16, origin_y: u16) {
        // Use 21 rows so available=20, 0.5*20=10 → divider at y=10, integer.
        let area = RRect::new(origin_x, origin_y, 40, 21);
        let mut buf = Buffer::empty(area);
        let split = vsplit(0.5);
        let layout = draw_split(&mut buf, area, &split, &Theme::default());

        let div_y = layout.divider_bounds.y.round() as u16;
        assert_eq!(cell_char(&buf, origin_x, div_y), '─');

        let mid_x = origin_x as f32 + 20.0;
        let hit = layout.hit_test(mid_x, layout.divider_bounds.y + 0.5);
        assert_eq!(hit, SplitHit::Divider(WidgetId::new("split")));

        let hit_first = layout.hit_test(mid_x, origin_y as f32 + 1.0);
        assert_eq!(hit_first, SplitHit::FirstPane(WidgetId::new("split")));

        let hit_second = layout.hit_test(mid_x, layout.divider_bounds.y + 1.5);
        assert_eq!(hit_second, SplitHit::SecondPane(WidgetId::new("split")));
    }

    #[test]
    fn vertical_paint_and_click_round_trip() {
        vertical_round_trip_at(0, 0);
    }

    /// Regression guard: non-zero-origin cases must be handled correctly.
    #[test]
    fn vertical_paint_and_click_round_trip_at_nonzero_origin() {
        vertical_round_trip_at(7, 13);
    }

    #[test]
    fn zero_size_is_a_no_op() {
        let buf_area = RRect::new(0, 0, 10, 10);
        let mut buf = Buffer::empty(buf_area);
        let area = RRect::new(0, 0, 0, 0);
        let split = hsplit(0.5);
        let _layout = draw_split(&mut buf, area, &split, &Theme::default());
        assert_eq!(cell_char(&buf, 0, 0), ' ');
    }

    /// An app that manually nests two independent
    /// `draw_split` calls (rather than a `SplitTree`) still gets a
    /// junction glyph where the second, full-extent divider crosses the
    /// first, shorter one already sitting in the buffer — this is the
    /// "read the four neighbours back out of the buffer" trick the
    /// issue describes, exercised end-to-end rather than as a synthetic
    /// buffer in `split_junction`'s own unit tests.
    ///
    /// Order matters for a *single*-pass-per-call API like this one: the
    /// divider whose own run spans the crossing (the "long" one) must be
    /// painted *after* the one it crosses, so its own upgrade pass can
    /// read the already-painted perpendicular glyph back. `SplitTree`
    /// (see `split_tree.rs`) sidesteps the ordering requirement with a
    /// two-phase paint; plain `Split` has no such batching, so this is
    /// the contract host code nesting `Split`s by hand must follow.
    #[test]
    fn manually_nested_splits_form_a_junction_when_the_spanning_one_paints_last() {
        let area = RRect::new(0, 0, 21, 11);
        let mut buf = Buffer::empty(area);
        let theme = Theme::default();

        // Inner: a Vertical (stacked) split confined to the left third
        // of the area — its '─' row lands at y=5, x=0..9.
        let inner_area = RRect::new(0, 0, 10, 11);
        let inner = vsplit(0.5);
        let inner_layout = draw_split(&mut buf, inner_area, &inner, &theme);
        let inner_row = inner_layout.divider_bounds.y.round() as u16;
        assert_eq!(cell_char(&buf, 4, inner_row), '─');

        // Outer: a Horizontal (side-by-side) split over the FULL area,
        // ratio chosen so its '│' column (x=5) crosses the inner row —
        // painted *last*, so it can see the inner run already there.
        let outer = hsplit(0.25);
        let outer_layout = draw_split(&mut buf, area, &outer, &theme);
        let outer_col = outer_layout.divider_bounds.x.round() as u16;
        assert_eq!(outer_col, 5);

        assert_eq!(cell_char(&buf, outer_col, inner_row), '┼');
        // Away from the crossing row, the outer column is still plain.
        assert_eq!(cell_char(&buf, outer_col, inner_row + 2), '│');
    }

    #[test]
    fn divider_position_matches_ratio() {
        let area = RRect::new(0, 0, 41, 10);
        let mut buf = Buffer::empty(area);
        let split = hsplit(0.3);
        let layout = draw_split(&mut buf, area, &split, &Theme::default());

        let div_x = layout.divider_bounds.x.round() as u16;
        // 41 cols, 1-cell divider → 40 available. 0.3 * 40 = 12.
        assert_eq!(div_x, 12);
        assert_eq!(cell_char(&buf, div_x, 0), '│');
        assert_eq!(cell_char(&buf, div_x - 1, 0), ' ');
        assert_eq!(cell_char(&buf, div_x + 1, 0), ' ');
    }

    /// Pins the TUI divider float->cell conversion to truncation, not
    /// rounding — the same rule `tui::split_tree`'s
    /// `fractional_ratio_round_trip_at` pins for `SplitTree`. A ratio
    /// chosen so the divider position is NOT cell-aligned (20.7, not
    /// 20.0 or 21.0) makes a round-vs-truncate regression visible: with
    /// `.round()` this would paint at column 21, not 20.
    #[test]
    fn divider_position_truncates_for_a_fractional_ratio() {
        let area = RRect::new(0, 0, 101, 10); // available = 100
        let mut buf = Buffer::empty(area);
        let split = hsplit(0.207); // position = 20.7
        let layout = draw_split(&mut buf, area, &split, &Theme::default());
        assert!((layout.divider_bounds.x - 20.7).abs() < 0.01);

        assert_eq!(cell_char(&buf, 20, 0), '│');
        assert_eq!(cell_char(&buf, 21, 0), ' ');
    }

    /// A fractional divider position that lands exactly on a half-cell
    /// boundary (`0.5` ratio over an odd `available`, e.g. 79) must still
    /// be clickable at the cell it actually paints at — the integer
    /// mouse coordinate a real TUI backend reports: the painted cell
    /// (`39`, from truncating `39.5`) lies outside the continuous
    /// `[39.5, 40.5)` band, so the hit region must be the snapped one.
    #[test]
    fn divider_is_clickable_at_the_cell_it_paints_at_for_a_half_cell_ratio() {
        let area = RRect::new(0, 0, 80, 10); // available = 79, position = 39.5
        let mut buf = Buffer::empty(area);
        let split = hsplit(0.5);
        let layout = draw_split(&mut buf, area, &split, &Theme::default());
        assert!((layout.divider_bounds.x - 39.5).abs() < 0.01);

        let painted_cell = 39u16;
        assert_eq!(cell_char(&buf, painted_cell, 5), '│');
        assert_eq!(
            layout.hit_test(painted_cell as f32, 5.0),
            SplitHit::Divider(split.id),
            "clicking the exact painted cell must register as the divider"
        );
        assert_eq!(
            layout.hit_test(40.0, 5.0),
            SplitHit::SecondPane(split.id),
            "the cell just past the divider is the second pane, not a dead zone"
        );
    }

    /// Pane bounds abut the painted divider cell exactly, so content
    /// painted into `first_bounds` can never cover the divider glyph.
    #[test]
    fn pane_bounds_abut_the_painted_divider_cell_for_a_half_cell_ratio() {
        let area = RRect::new(0, 0, 20, 4); // available = 19, position = 9.5
        let mut buf = Buffer::empty(area);
        let layout = draw_split(&mut buf, area, &hsplit(0.5), &Theme::default());
        assert_eq!(cell_char(&buf, 9, 0), '│');
        assert_eq!(layout.first_bounds.x, 0.0);
        assert_eq!(layout.first_bounds.width, 9.0);
        assert_eq!(layout.second_bounds.x, 10.0);
        assert_eq!(layout.second_bounds.width, 10.0);
    }
}

#[cfg(all(test, feature = "tui"))]
mod split_tree_cell_tests {

    use super::*;
    use crate::style::Style;
    use crate::theme::Theme;
    use crate::tui::BufferCellSurface;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect as RRect;

    fn cell_rect(area: RRect) -> Rect {
        Rect::new(
            area.x as f32,
            area.y as f32,
            area.width as f32,
            area.height as f32,
        )
    }

    fn paint<W: Primitive>(w: &W, layout: &W::Layout, buf: &mut Buffer, theme: &Theme) {
        let style = Style::default();
        w.paint_cells(
            layout,
            &mut BufferCellSurface::new(buf),
            &PaintEnv::new(theme, &style),
        );
    }

    fn draw_split_tree(
        buf: &mut Buffer,
        area: RRect,
        tree: &SplitTree,
        theme: &Theme,
    ) -> SplitTreeLayout {
        let layout = tree.cell_layout(cell_rect(area));
        paint(tree, &layout, buf, theme);
        layout
    }

    fn cell_char(buf: &Buffer, x: u16, y: u16) -> char {
        buf[(x, y)].symbol().chars().next().unwrap_or(' ')
    }

    fn wid(s: &str) -> WidgetId {
        WidgetId::new(s)
    }

    fn two_pane(direction: SplitDirection, ratio: f32) -> SplitTree {
        SplitTree::split(
            direction,
            ratio,
            SplitTree::leaf(wid("a")),
            SplitTree::leaf(wid("b")),
        )
    }

    // Parametrized over the area's origin — LESSONS.md "Layout helpers
    // must return coords in the same frame across backends".
    // `tui_split_tree_layout` bakes `area.x`/`area.y`
    // straight into `tree.layout`'s returned bounds (absolute frame),
    // so the regression guard is a full paint+hit_test round trip
    // re-run at a non-zero origin.
    fn horizontal_round_trip_at(origin_x: u16, origin_y: u16) {
        // 41 cols so available=40, 0.5*40=20 -> divider at cell x=origin_x+20.
        let area = RRect::new(origin_x, origin_y, 41, 10);
        let mut buf = Buffer::empty(area);
        let tree = two_pane(SplitDirection::Horizontal, 0.5);
        let layout = draw_split_tree(&mut buf, area, &tree, &Theme::default());

        let d = &layout.dividers[0];
        let cell = d.cell_position();
        assert_eq!(cell_char(&buf, cell, origin_y), '│');

        let cross_cell = origin_y + 5;
        // hit_test_divider_cell at the exact painted cell resolves the
        // same split_index the paint loop used.
        assert_eq!(
            layout.hit_test_divider_cell(cell, cross_cell),
            Some(d.split_index)
        );

        // Leaf hit-test resolves panes on either side of the divider.
        assert_eq!(
            layout.hit_test_leaf(Point {
                x: origin_x as f32 + 1.0,
                y: cross_cell as f32
            }),
            Some(&wid("a"))
        );
        assert_eq!(
            layout.hit_test_leaf(Point {
                x: cell as f32 + 2.0,
                y: cross_cell as f32
            }),
            Some(&wid("b"))
        );
    }

    #[test]
    fn horizontal_paint_and_click_round_trip() {
        horizontal_round_trip_at(0, 0);
    }

    /// Non-zero-origin regression guard (LESSONS.md).
    #[test]
    fn horizontal_paint_and_click_round_trip_at_nonzero_origin() {
        horizontal_round_trip_at(7, 13);
    }

    #[test]
    fn vertical_paint_and_click_round_trip() {
        // 21 rows so available=20, 0.5*20=10 -> divider at cell y=10.
        let area = RRect::new(0, 0, 40, 21);
        let mut buf = Buffer::empty(area);
        let tree = two_pane(SplitDirection::Vertical, 0.5);
        let layout = draw_split_tree(&mut buf, area, &tree, &Theme::default());

        let d = &layout.dividers[0];
        let cell = d.cell_position();
        assert_eq!(cell_char(&buf, 0, cell), '─');
        assert_eq!(layout.hit_test_divider_cell(cell, 5), Some(d.split_index));

        assert_eq!(
            layout.hit_test_leaf(Point { x: 5.0, y: 1.0 }),
            Some(&wid("a"))
        );
        assert_eq!(
            layout.hit_test_leaf(Point {
                x: 5.0,
                y: cell as f32 + 2.0
            }),
            Some(&wid("b"))
        );
    }

    #[test]
    fn nested_tree_paints_all_dividers_and_round_trips() {
        // Split(H, Split(V, a, c), b) inside a 61x21 area.
        let area = RRect::new(0, 0, 61, 21);
        let mut buf = Buffer::empty(area);
        let tree = SplitTree::split(
            SplitDirection::Horizontal,
            0.5,
            SplitTree::split(
                SplitDirection::Vertical,
                0.5,
                SplitTree::leaf(wid("a")),
                SplitTree::leaf(wid("c")),
            ),
            SplitTree::leaf(wid("b")),
        );
        let layout = draw_split_tree(&mut buf, area, &tree, &Theme::default());
        assert_eq!(layout.dividers.len(), 2);

        for div in &layout.dividers {
            let axis_cell = div.cell_position();
            let cross_mid = (div.cross_start + div.cross_size / 2.0).round() as u16;
            let expected = match div.direction {
                SplitDirection::Horizontal => cell_char(&buf, axis_cell, cross_mid),
                SplitDirection::Vertical => cell_char(&buf, cross_mid, axis_cell),
            };
            let want = match div.direction {
                SplitDirection::Horizontal => '│',
                SplitDirection::Vertical => '─',
            };
            assert_eq!(expected, want, "divider {} paint mismatch", div.split_index);

            // hit_test_divider_cell must resolve every painted divider
            // back to its own split_index — the round-trip guarantee.
            let cross_cell = match div.direction {
                SplitDirection::Horizontal => cross_mid,
                SplitDirection::Vertical => cross_mid,
            };
            assert_eq!(
                layout.hit_test_divider_cell(axis_cell, cross_cell),
                Some(div.split_index)
            );
        }

        assert_eq!(layout.leaves.len(), 3);
    }

    /// A `:vsplit` inside a `:split` — the
    /// nested-tree geometry from `nested_tree_paints_all_dividers_and_
    /// round_trips` above — paints a junction glyph, not a dangling
    /// `'│'`, where the inner divider's row meets the outer divider's
    /// column. `SplitTreeMeasure`'s exact-adjacency layout (each inner
    /// pane's rect stops one cell short of the divider that bounds it)
    /// means the inner run's endpoint always lands as an *interior*
    /// cell of the outer (full cross-axis) run, never overlapping it, so
    /// the junction always resolves onto the outer divider's own cell.
    #[test]
    fn nested_split_paints_a_junction_where_dividers_meet() {
        let area = RRect::new(0, 0, 61, 21);
        let mut buf = Buffer::empty(area);
        let tree = SplitTree::split(
            SplitDirection::Horizontal,
            0.5,
            SplitTree::split(
                SplitDirection::Vertical,
                0.5,
                SplitTree::leaf(wid("a")),
                SplitTree::leaf(wid("c")),
            ),
            SplitTree::leaf(wid("b")),
        );
        let layout = draw_split_tree(&mut buf, area, &tree, &Theme::default());

        let outer = &layout.dividers[0]; // Horizontal (│), full height
        let inner = &layout.dividers[1]; // Vertical (─), left pane only
        assert_eq!(outer.direction, SplitDirection::Horizontal);
        assert_eq!(inner.direction, SplitDirection::Vertical);

        let outer_col = outer.cell_position();
        let inner_row = inner.cell_position();

        // The junction cell is the outer divider's own column at the
        // inner divider's row — a T meeting from the left only (no pane
        // to the right of the outer column's own run), so '┤'.
        assert_eq!(cell_char(&buf, outer_col, inner_row), '┤');

        // A row away from the junction, the outer divider is still a
        // plain '│' — the upgrade only touches the meeting cell.
        assert_eq!(cell_char(&buf, outer_col, inner_row.saturating_sub(3)), '│');

        // hit_test_divider_cell still resolves the junction cell back
        // to the outer divider's split_index — junction painting must
        // not disturb hit-testing.
        assert_eq!(
            layout.hit_test_divider_cell(outer_col, inner_row),
            Some(outer.split_index)
        );
    }

    /// A genuine 4-way crossing (`┼`) — two inner
    /// `Horizontal` (side-by-side) splits stacked by an outer
    /// `Vertical` split, with matching ratios so both inner dividers
    /// land on the same column. The outer divider's row then has an
    /// inner vertical divider immediately above *and* below it in that
    /// same column, plus its own left/right run — all four directions.
    #[test]
    fn stacked_matching_splits_form_a_plus_junction() {
        let area = RRect::new(0, 0, 41, 21);
        let mut buf = Buffer::empty(area);
        let tree = SplitTree::split(
            SplitDirection::Vertical,
            0.5,
            SplitTree::split(
                SplitDirection::Horizontal,
                0.5,
                SplitTree::leaf(wid("a")),
                SplitTree::leaf(wid("b")),
            ),
            SplitTree::split(
                SplitDirection::Horizontal,
                0.5,
                SplitTree::leaf(wid("c")),
                SplitTree::leaf(wid("d")),
            ),
        );
        let layout = draw_split_tree(&mut buf, area, &tree, &Theme::default());
        assert_eq!(layout.dividers.len(), 3);

        let outer = &layout.dividers[0]; // Vertical (─), full width
        assert_eq!(outer.direction, SplitDirection::Vertical);
        let outer_row = outer.cell_position();

        let top_inner = &layout.dividers[1]; // Horizontal (│), top pane
        let bottom_inner = &layout.dividers[2]; // Horizontal (│), bottom pane
        assert_eq!(top_inner.cell_position(), bottom_inner.cell_position());
        let shared_col = top_inner.cell_position();

        assert_eq!(cell_char(&buf, shared_col, outer_row), '┼');
    }

    // Parametrized over the area's origin — LESSONS.md "Layout helpers
    // must return coords in the same frame across backends".
    // This is a genuine paint→hit_test round trip (it
    // reads the actually-painted column back out of the buffer), so it
    // gets the same non-zero-origin treatment as the other round trips
    // in this file.
    fn fractional_ratio_round_trip_at(origin_x: u16, origin_y: u16) {
        // A ratio chosen so the divider position is NOT cell-aligned — paint must truncate
        // (`as u16`) the same way `SplitTreeDivider::cell_position()` does, not round —
        // otherwise a click on the visually-painted divider misses.
        let area = RRect::new(origin_x, origin_y, 101, 10); // available = 100
        let mut buf = Buffer::empty(area);
        // Unrounded position origin_x + 20.7; the cell layout places the
        // divider on the whole cell it truncates to.
        let tree = two_pane(SplitDirection::Horizontal, 0.207);
        let layout = draw_split_tree(&mut buf, area, &tree, &Theme::default());
        assert_eq!(layout.dividers[0].position, origin_x as f32 + 20.0);

        let cross_row = origin_y + 5;
        // Find the column the divider glyph actually painted at —
        // don't assume a formula, read the buffer.
        let painted_col = (area.x..area.x + area.width)
            .find(|&x| cell_char(&buf, x, cross_row) == '│')
            .expect("divider glyph should have painted somewhere");

        // hit_test_divider_cell at the ACTUAL painted column must
        // resolve to this divider — proving paint and hit-test agree
        // on the same float -> cell conversion.
        assert_eq!(
            layout.hit_test_divider_cell(painted_col, cross_row),
            Some(layout.dividers[0].split_index),
            "painted at column {painted_col} but hit_test_divider_cell disagrees \
             (paint and click used different float->cell conversions)"
        );
    }

    #[test]
    fn hit_test_agrees_with_actually_painted_cell_for_fractional_ratio() {
        fractional_ratio_round_trip_at(0, 0);
    }

    /// Non-zero-origin regression guard (LESSONS.md): the paint/hit-test
    /// agreement must also hold when the tree
    /// isn't painted at the screen origin.
    #[test]
    fn hit_test_agrees_with_actually_painted_cell_for_fractional_ratio_at_nonzero_origin() {
        fractional_ratio_round_trip_at(7, 13);
    }

    #[test]
    fn zero_size_is_a_no_op() {
        let buf_area = RRect::new(0, 0, 10, 10);
        let mut buf = Buffer::empty(buf_area);
        let area = RRect::new(0, 0, 0, 0);
        let tree = two_pane(SplitDirection::Horizontal, 0.5);
        let _layout = draw_split_tree(&mut buf, area, &tree, &Theme::default());
        assert_eq!(cell_char(&buf, 0, 0), ' ');
    }

    #[test]
    fn divider_position_matches_ratio() {
        let area = RRect::new(0, 0, 41, 10);
        let mut buf = Buffer::empty(area);
        let tree = two_pane(SplitDirection::Horizontal, 0.3);
        let layout = draw_split_tree(&mut buf, area, &tree, &Theme::default());

        let cell = layout.dividers[0].cell_position();
        // 41 cols, 1-cell divider -> 40 available. 0.3 * 40 = 12.
        assert_eq!(cell, 12);
        assert_eq!(cell_char(&buf, cell, 0), '│');
        assert_eq!(cell_char(&buf, cell - 1, 0), ' ');
        assert_eq!(cell_char(&buf, cell + 1, 0), ' ');
    }
}
