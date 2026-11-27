//! [`Chart`] on the widget contract: [`crate::Primitive`] (layout, pixel
//! paint, cell paint) and [`crate::Interactive`] (hover, click, and
//! keyboard stepping through the data points).
//!
//! Hover and crosshair are app-owned descriptor fields
//! ([`Chart::hovered_point`], [`Chart::crosshair_x`]): the chart reports
//! the point under the pointer as [`ChartEvent::DataPointHovered`] and
//! paints whatever the app writes back.

use super::{Chart, ChartEvent, ChartHit, ChartLayout};
use crate::behaviour::nav::{FlatNav, NavKey};
use crate::event::{Key, NamedKey, Point, Rect};
use crate::interactive::PointerKind;
use crate::types::WidgetId;

/// [`crate::Primitive::Layout`] for [`Chart`]: the resolved
/// [`ChartLayout`] plus how far from a data point the pointer may be and
/// still hover it.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct ChartPrimitiveLayout {
    /// The chart's resolved geometry and hit regions.
    pub chart: ChartLayout,
    /// The hover snap radius: two rows of the laying-out backend.
    pub snap_distance: f32,
}

/// A [`Chart`]'s per-widget gesture state. A chart has no gesture to
/// track — hover and crosshair are descriptor fields the app owns.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct ChartState {}

impl crate::interactive::WidgetAction for ChartEvent {
    fn into_ui_event(self, id: WidgetId) -> crate::UiEvent {
        crate::UiEvent::Chart(id, self)
    }
}

impl crate::primitive::Primitive for Chart {
    type Layout = ChartPrimitiveLayout;
    type Event = ChartEvent;

    fn id(&self) -> Option<&WidgetId> {
        Some(&self.id)
    }

    fn capture(&self) -> crate::frame::OwnedContent {
        crate::frame::OwnedContent::Chart(self.clone())
    }

    /// A chart fills the region it is given.
    fn measure(
        &self,
        _ctx: &crate::primitive::LayoutCtx,
        _avail: crate::primitive::Size,
    ) -> crate::NaturalSize {
        crate::NaturalSize::NONE
    }

    /// [`Chart::layout_in`] at the chrome font's digit advance and the
    /// row height (`1.0`/`1.0` on a cell grid): the axis labels paint in
    /// the chrome font, so the y-axis gutter is sized in its advance.
    fn layout(&self, ctx: &crate::primitive::LayoutCtx, rect: Rect) -> ChartPrimitiveLayout {
        let (char_width, line_height) = match ctx.units() {
            crate::primitive::Units::Cells => (1.0, 1.0),
            _ => (
                ctx.text_width("0", crate::FontRole::Chrome),
                ctx.line_height,
            ),
        };
        ChartPrimitiveLayout {
            chart: self.layout_in(rect, char_width, line_height),
            snap_distance: line_height * 2.0,
        }
    }

    fn paint(
        &self,
        layout: &ChartPrimitiveLayout,
        surface: &mut dyn crate::paint_surface::PaintSurface,
        env: &crate::PaintEnv,
    ) {
        super::paint_pixels::paint(
            self,
            &layout.chart,
            surface,
            env.theme(),
            self.hovered_point,
            self.crosshair_x,
        );
    }

    #[cfg(feature = "tui")]
    fn paint_cells(
        &self,
        layout: &ChartPrimitiveLayout,
        cells: &mut dyn crate::CellSurface,
        env: &crate::PaintEnv,
    ) {
        super::paint_cells::paint(
            self,
            &layout.chart,
            cells,
            env.theme(),
            self.hovered_point,
            self.crosshair_x,
        );
    }

    /// The whole chart, so a structural observer finds it even though
    /// its marks carry no text.
    fn paint_zone(
        &self,
        layout: &ChartPrimitiveLayout,
        _units: crate::primitive::Units,
    ) -> Option<Rect> {
        Some(layout.chart.bounds)
    }
}

impl Chart {
    /// The hover event for `point`, or `None` when it is already the
    /// painted [`Chart::hovered_point`].
    fn hover(&self, point: Option<(usize, usize)>) -> Option<ChartEvent> {
        (point != self.hovered_point).then_some(ChartEvent::DataPointHovered { id: self.id, point })
    }

    /// The data point a navigation key moves the hover to: Left/Right
    /// (and Home/End) step through the hovered series' points, Up/Down
    /// move to the previous/next series at the same index, clamped to
    /// its length. With nothing hovered the walk starts at series 0.
    fn stepped_point(&self, key: &Key) -> Option<(usize, usize)> {
        let (si, di) = self.hovered_point.unwrap_or((0, 0));
        let current = self.hovered_point.map(|(_, di)| di);
        let len_of = |si: usize| self.series.get(si).map_or(0, |s| s.data.len());
        let along =
            |nav: NavKey| FlatNav::mv(len_of(si), current, nav, false, |_| true).map(|di| (si, di));
        match key {
            Key::Named(NamedKey::Left) => along(NavKey::Up),
            Key::Named(NamedKey::Right) => along(NavKey::Down),
            Key::Named(NamedKey::Home) => along(NavKey::Home),
            Key::Named(NamedKey::End) => along(NavKey::End),
            Key::Named(k @ (NamedKey::Up | NamedKey::Down)) => {
                let nav = if *k == NamedKey::Up {
                    NavKey::Up
                } else {
                    NavKey::Down
                };
                let current_series = self.hovered_point.map(|(si, _)| si);
                let next = FlatNav::mv(self.series.len(), current_series, nav, false, |i| {
                    len_of(i) > 0
                })?;
                Some((next, di.min(len_of(next).saturating_sub(1))))
            }
            _ => None,
        }
    }
}

impl crate::interactive::Interactive for Chart {
    type State = ChartState;
    type Action = ChartEvent;

    /// Every move inside the widget reaches [`Self::handle`], so
    /// the hovered data point follows the pointer.
    const RECEIVES_HOVER_MOVES: bool = true;

    /// One tab stop: the arrow keys walk the hover through the points.
    fn focus(&self) -> crate::interactive::FocusSpec {
        crate::interactive::FocusSpec::Stop
    }

    /// - **Move** reports the data point within the snap radius of the
    ///   pointer ([`ChartLayout::nearest_point`]), or `None` past every
    ///   point; **Leave** reports `None`. Each is sent only when it
    ///   differs from [`Chart::hovered_point`].
    /// - **Left press** reports the data point under the pointer, else
    ///   the legend entry, else a click on the plot body.
    /// - **Keys**: Left/Right/Home/End step through the hovered series,
    ///   Up/Down change series; Enter clicks the hovered point.
    fn handle(
        &self,
        _state: &mut ChartState,
        event: &crate::interactive::RoutedEvent,
        layout: &ChartPrimitiveLayout,
        _cx: &mut crate::interactive::EventCx,
    ) -> crate::Handled<ChartEvent> {
        use crate::interactive::RoutedEvent;
        use crate::Handled;

        let resolved = |e: Option<ChartEvent>| match e {
            Some(e) => Handled::Event(e),
            None => Handled::Ignored,
        };
        let b = layout.chart.bounds;
        let absolute = |local: &Point| Point::new(b.x + local.x, b.y + local.y);
        let nearest = |p: Point| layout.chart.nearest_point(p.x, p.y, layout.snap_distance);

        match event {
            RoutedEvent::Pointer {
                kind: PointerKind::Move | PointerKind::Enter,
                local,
                ..
            } => resolved(self.hover(nearest(absolute(local)))),
            RoutedEvent::Pointer {
                kind: PointerKind::Leave,
                ..
            } => resolved(self.hover(None)),
            RoutedEvent::Pointer {
                kind: PointerKind::Down(crate::MouseButton::Left),
                local,
                ..
            } => {
                let p = absolute(local);
                if let Some((series_idx, data_idx)) = nearest(p) {
                    return Handled::Event(ChartEvent::DataPointClicked {
                        id: self.id,
                        series_idx,
                        data_idx,
                    });
                }
                match layout.chart.hit_test(p.x, p.y) {
                    ChartHit::Legend(id, series_idx) => {
                        Handled::Event(ChartEvent::LegendClicked { id, series_idx })
                    }
                    ChartHit::Body(id) => Handled::Event(ChartEvent::Clicked { id }),
                    ChartHit::DataPoint(id, series_idx, data_idx) => {
                        Handled::Event(ChartEvent::DataPointClicked {
                            id,
                            series_idx,
                            data_idx,
                        })
                    }
                    ChartHit::Empty => Handled::Ignored,
                }
            }
            RoutedEvent::Key(key) => match key.key {
                Key::Named(NamedKey::Enter) => match self.hovered_point {
                    Some((series_idx, data_idx)) => Handled::Event(ChartEvent::DataPointClicked {
                        id: self.id,
                        series_idx,
                        data_idx,
                    }),
                    None => Handled::Ignored,
                },
                ref k => resolved(self.stepped_point(k).and_then(|p| self.hover(Some(p)))),
            },
            _ => Handled::Ignored,
        }
    }
}
