//! [`Minimap`] on the widget contract: [`crate::Primitive`] (layout, pixel
//! paint, cell paint) and [`crate::Interactive`] (click and drag to seek).
//!
//! The minimap owns no scroll position: a press or a captured drag on the
//! strip reports the fraction of the file under the pointer
//! ([`MinimapEvent::Seek`]) and the app scrolls its editor there.

use super::{Minimap, MinimapHit, MinimapLayout, MinimapSizing};
use crate::event::{Point, Rect};
use crate::interactive::PointerKind;
use crate::types::WidgetId;
use serde::{Deserialize, Serialize};

/// A gesture on a [`Minimap`], resolved against its layout.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum MinimapEvent {
    /// Scroll the editor to `fraction` (`0.0` top, `1.0` bottom) of the
    /// file: the pointer's height within the strip.
    Seek { fraction: f32 },
}

impl crate::interactive::WidgetAction for MinimapEvent {
    fn into_ui_event(self, id: WidgetId) -> crate::UiEvent {
        crate::UiEvent::Minimap(id, self)
    }
}

/// A [`Minimap`]'s per-widget gesture state: whether a left-button drag
/// that started on the strip is in progress.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct MinimapState {
    /// `true` from a left press on the strip until its release.
    pub dragging: bool,
}

impl MinimapLayout {
    /// The seek fraction at absolute `y`, clamped onto the strip.
    fn fraction_at(&self, y: f32) -> f32 {
        let b = &self.bounds;
        if b.height > 0.0 {
            ((y - b.y) / b.height).clamp(0.0, 1.0)
        } else {
            0.0
        }
    }
}

impl crate::primitive::Primitive for Minimap {
    type Layout = MinimapLayout;
    type Event = MinimapEvent;

    fn id(&self) -> Option<&WidgetId> {
        Some(&self.id)
    }

    /// The glyph atlas is sampled from the editor font, so the strip reads
    /// as a thumbnail of the text beside it.
    fn font_role(&self) -> crate::FontRole {
        crate::FontRole::Editor
    }

    fn capture(&self) -> crate::frame::OwnedContent {
        crate::frame::OwnedContent::Minimap(self.clone())
    }

    /// A minimap fills the strip it is given; the host sizes the strip
    /// (see [`MinimapSizing::resolve_width`]).
    fn measure(
        &self,
        _ctx: &crate::primitive::LayoutCtx,
        _avail: crate::primitive::Size,
    ) -> crate::NaturalSize {
        crate::NaturalSize::NONE
    }

    /// [`Minimap::cell_layout`] on a cell grid (one cell row per row
    /// unless [`Minimap::sizing`] says otherwise), [`Minimap::pixel_layout`]
    /// on a pixel surface.
    fn layout(&self, ctx: &crate::primitive::LayoutCtx, rect: Rect) -> MinimapLayout {
        match ctx.units() {
            crate::primitive::Units::Cells => {
                self.cell_layout(rect, self.sizing.unwrap_or(MinimapSizing::FixedPitch(1.0)))
            }
            _ => self.pixel_layout(rect, self.scale, self.sizing),
        }
    }

    /// The shared pixel painter at the surface's display scale, blitting
    /// glyph shapes when the surface can rasterise them.
    fn paint(
        &self,
        layout: &MinimapLayout,
        surface: &mut dyn crate::paint_surface::PaintSurface,
        env: &crate::PaintEnv,
    ) {
        if layout.visible_lines.is_empty() {
            return;
        }
        let dpi_scale = surface.surface_viewport().scale;
        super::paint_pixels::paint(
            self,
            layout,
            surface,
            env.theme(),
            self.scale,
            super::paint_pixels::Glyphs::Atlas { dpi_scale },
        );
    }

    #[cfg(feature = "tui")]
    fn paint_cells(
        &self,
        layout: &MinimapLayout,
        cells: &mut dyn crate::CellSurface,
        env: &crate::PaintEnv,
    ) {
        super::paint_cells::paint(self, layout, cells, env.theme());
    }

    /// The whole strip, so a structural observer can find the minimap
    /// even though it paints no text.
    fn paint_zone(&self, layout: &MinimapLayout, _units: crate::primitive::Units) -> Option<Rect> {
        Some(layout.bounds)
    }
}

impl crate::interactive::Interactive for Minimap {
    type State = MinimapState;
    type Action = MinimapEvent;

    /// The minimap is pointer-only: it never takes keyboard focus.
    fn focus(&self) -> crate::interactive::FocusSpec {
        crate::interactive::FocusSpec::None
    }

    /// - **Left press** on the strip captures the pointer, starts a drag
    ///   and reports [`MinimapEvent::Seek`] for the pressed point.
    /// - **Move** during the drag reports a seek for the pointer's height,
    ///   clamped onto the strip; a move with no drag is ignored.
    /// - **Left release** ends the drag and releases the capture.
    fn handle(
        &self,
        state: &mut MinimapState,
        event: &crate::interactive::RoutedEvent,
        layout: &MinimapLayout,
        cx: &mut crate::interactive::EventCx,
    ) -> crate::Handled<MinimapEvent> {
        use crate::interactive::RoutedEvent;
        use crate::Handled;

        let origin = Point::new(layout.bounds.x, layout.bounds.y);
        let absolute = |local: &Point| Point::new(origin.x + local.x, origin.y + local.y);

        match event {
            RoutedEvent::Pointer {
                kind: PointerKind::Down(crate::MouseButton::Left),
                local,
                ..
            } => {
                let p = absolute(local);
                match layout.hit_test(p.x, p.y) {
                    MinimapHit::Seek { fraction } => {
                        cx.capture_pointer();
                        state.dragging = true;
                        Handled::Event(MinimapEvent::Seek { fraction })
                    }
                    MinimapHit::None => Handled::Ignored,
                }
            }
            RoutedEvent::Pointer {
                kind: PointerKind::Move,
                local,
                ..
            } if state.dragging => Handled::Event(MinimapEvent::Seek {
                fraction: layout.fraction_at(absolute(local).y),
            }),
            RoutedEvent::Pointer {
                kind: PointerKind::Up(crate::MouseButton::Left),
                ..
            } if state.dragging => {
                state.dragging = false;
                cx.release_pointer();
                Handled::Consumed
            }
            RoutedEvent::FocusOut if state.dragging => {
                state.dragging = false;
                cx.release_pointer();
                Handled::Ignored
            }
            _ => Handled::Ignored,
        }
    }
}
