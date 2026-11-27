//! [`Canvas`] on the widget contract: [`crate::Primitive`] (layout, pixel
//! paint through the shared op painter, cell paint through the braille
//! degrade) and [`crate::Interactive`] (declared-region hover, click and
//! double-click, with each region's pointer shape).

use super::{Canvas, CanvasHit, CanvasLayout};
use crate::event::{CanvasEvent, Point, Rect};
use crate::interactive::PointerKind;
use crate::types::WidgetId;

/// A [`Canvas`]'s per-widget gesture state: the declared region the
/// pointer is over, so a hover change is reported once.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct CanvasState {
    /// The region under the pointer, `None` over bare canvas or outside.
    pub hovered: Option<WidgetId>,
}

impl crate::interactive::WidgetAction for CanvasEvent {
    fn into_ui_event(self, id: WidgetId) -> crate::UiEvent {
        crate::UiEvent::Canvas(id, self)
    }
}

impl Canvas {
    /// The declared region at absolute `p`: cell-quantised on a cell
    /// grid, exact on pixels.
    fn region_at(
        &self,
        layout: &CanvasLayout,
        p: Point,
        units: crate::primitive::Units,
    ) -> Option<WidgetId> {
        let hit = match units {
            #[cfg(feature = "tui")]
            crate::primitive::Units::Cells => {
                super::paint_cells::region_hit_test(self, layout, p.x, p.y)
            }
            _ => layout.region_hit_test(self, p.x, p.y),
        };
        match hit {
            CanvasHit::Region { id } => Some(id),
            _ => None,
        }
    }
}

impl crate::primitive::Primitive for Canvas {
    type Layout = CanvasLayout;
    type Event = CanvasEvent;

    fn id(&self) -> Option<&WidgetId> {
        Some(&self.id)
    }

    fn capture(&self) -> crate::frame::OwnedContent {
        crate::frame::OwnedContent::Canvas(self.clone())
    }

    /// A canvas fills the region it is given; its ops are drawn in that
    /// region's own coordinates.
    fn measure(
        &self,
        _ctx: &crate::primitive::LayoutCtx,
        _avail: crate::primitive::Size,
    ) -> crate::NaturalSize {
        crate::NaturalSize::NONE
    }

    fn layout(&self, _ctx: &crate::primitive::LayoutCtx, rect: Rect) -> CanvasLayout {
        Canvas::layout(self, rect)
    }

    fn paint(
        &self,
        layout: &CanvasLayout,
        surface: &mut dyn crate::paint_surface::PaintSurface,
        _env: &crate::PaintEnv,
    ) {
        super::native_surface_paint::paint(self, layout, surface);
    }

    #[cfg(feature = "tui")]
    fn paint_cells(
        &self,
        layout: &CanvasLayout,
        cells: &mut dyn crate::CellSurface,
        env: &crate::PaintEnv,
    ) {
        super::paint_cells::paint(self, layout, cells, env.theme());
    }

    /// The whole canvas, so a structural observer finds it even when its
    /// ops paint no text.
    fn paint_zone(&self, layout: &CanvasLayout, _units: crate::primitive::Units) -> Option<Rect> {
        Some(layout.bounds)
    }
}

impl crate::interactive::Interactive for Canvas {
    type State = CanvasState;
    type Action = CanvasEvent;

    /// One tab stop when [`Canvas::focusable`] is set; otherwise none.
    fn focus(&self) -> crate::interactive::FocusSpec {
        if self.focusable {
            crate::interactive::FocusSpec::Stop
        } else {
            crate::interactive::FocusSpec::None
        }
    }

    /// - **Move/Enter** resolves the declared region under the pointer;
    ///   when it changes, the pointer takes the new region's
    ///   [`super::CanvasRegion::cursor`] (the default shape when it has
    ///   none) and [`CanvasEvent::RegionHovered`] reports it. **Leave**
    ///   reports `None` if a region was hovered.
    /// - **Press** / **double-click** on a region reports
    ///   [`CanvasEvent::RegionClicked`] /
    ///   [`CanvasEvent::RegionDoubleClicked`] with the canvas-local
    ///   point. A press on bare canvas is ignored, so it reaches the app
    ///   as the plain click it is.
    fn handle(
        &self,
        state: &mut CanvasState,
        event: &crate::interactive::RoutedEvent,
        layout: &CanvasLayout,
        cx: &mut crate::interactive::EventCx,
    ) -> crate::Handled<CanvasEvent> {
        use crate::interactive::RoutedEvent;
        use crate::Handled;

        let b = layout.bounds;
        let units = cx.units();
        let region_at =
            |local: &Point| self.region_at(layout, Point::new(b.x + local.x, b.y + local.y), units);
        let mut hover = |region: Option<WidgetId>, cx: &mut crate::interactive::EventCx| {
            if region == state.hovered {
                return Handled::Ignored;
            }
            state.hovered = region;
            let shape = region
                .and_then(|id| self.regions.iter().rev().find(|r| r.id == id))
                .and_then(|r| r.cursor)
                .map(super::CanvasCursor::to_pointer_shape)
                .unwrap_or(crate::PointerShape::Default);
            cx.set_cursor(shape);
            Handled::Event(CanvasEvent::RegionHovered {
                region: region.map(|id| id.as_str().to_string()),
            })
        };

        match event {
            RoutedEvent::Pointer {
                kind: PointerKind::Move | PointerKind::Enter,
                local,
                ..
            } => hover(region_at(local), cx),
            RoutedEvent::Pointer {
                kind: PointerKind::Leave,
                ..
            } => hover(None, cx),
            RoutedEvent::Pointer {
                kind: PointerKind::Down(button),
                local,
                mods,
                ..
            } => match region_at(local) {
                Some(region) => Handled::Event(CanvasEvent::RegionClicked {
                    region,
                    button: *button,
                    modifiers: *mods,
                    x: local.x,
                    y: local.y,
                }),
                None => Handled::Ignored,
            },
            RoutedEvent::Pointer {
                kind: PointerKind::DoubleClick(_),
                local,
                ..
            } => match region_at(local) {
                Some(region) => Handled::Event(CanvasEvent::RegionDoubleClicked {
                    region,
                    x: local.x,
                    y: local.y,
                }),
                None => Handled::Ignored,
            },
            _ => Handled::Ignored,
        }
    }
}
