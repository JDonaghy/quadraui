//! The README's "Your own widget" sample, compiled and run. The block between
//! the markers is copied verbatim from README.md; `tools/test_doc_samples.py`
//! fails if the two drift apart.
#![cfg(feature = "tui")]

// readme:begin
use quadraui::prelude::*;
use quadraui::{
    A11yInfo, A11yPrimitive, CellSurface, CellSurfaceStyle, LayoutCtx, PaintEnv, PaintSurface,
    Primitive,
};

struct Swatch {
    id: WidgetId,
    color: Color,
}

impl A11yPrimitive for Swatch {
    fn a11y_info(&self) -> A11yInfo {
        A11yInfo::default()
    }
}

impl Primitive for Swatch {
    // The resolved geometry that paint, and any hit-test, read.
    type Layout = Rect;
    type Event = ();

    fn id(&self) -> Option<&WidgetId> {
        Some(&self.id)
    }

    fn layout(&self, _ctx: &LayoutCtx, rect: Rect) -> Rect {
        rect
    }

    // GTK, macOS and Windows.
    fn paint(&self, layout: &Rect, surface: &mut dyn PaintSurface, _env: &PaintEnv) {
        surface.surface_fill_rect(*layout, self.color);
    }

    // The terminal (needs the `tui` feature).
    fn paint_cells(&self, layout: &Rect, cells: &mut dyn CellSurface, _env: &PaintEnv) {
        let style = CellSurfaceStyle {
            bg: Some(self.color),
            ..Default::default()
        };
        cells.fill(*layout, style);
    }
}

quadraui::non_interactive!(Swatch);

fn paint_swatch(frame: &mut ScreenLayout, rect: Rect) {
    let swatch = Swatch {
        id: WidgetId::new("swatch"),
        color: Color::rgb(200, 60, 60),
    };
    frame.push(Surface::Widget {
        rect,
        widget: Box::new(swatch),
    });
}
// readme:end

use quadraui::tui::testing::{Color as TermColor, TuiDriver};
use quadraui::FrameTree;

struct Host;

impl AppLogic for Host {
    type AreaId = ();
    fn render(&self, backend: &mut dyn Backend, _: ()) {
        let mut frame = ScreenLayout::new();
        paint_swatch(&mut frame, Rect::new(0.0, 0.0, 4.0, 2.0));
        frame.draw(backend);
    }
    fn frame_tree(&self, _: ()) -> FrameTree {
        FrameTree::default()
    }
    fn handle(&mut self, _: UiEvent, _: &mut dyn Backend) -> Reaction {
        Reaction::Continue
    }
}

#[test]
fn the_readme_swatch_paints_its_colour_on_the_terminal() {
    let mut driver = TuiDriver::new(Host, 8, 3);
    driver.render();
    let swatch = TermColor::Rgb(200, 60, 60);
    for y in 0..2 {
        let row = driver.styled_row(y);
        assert!(
            row[..4].iter().all(|(_, s)| s.bg == swatch),
            "row {y}: {row:?}"
        );
        assert_ne!(row[4].1.bg, swatch, "row {y} painted past the rect");
    }
    assert_ne!(driver.styled_row(2)[0].1.bg, swatch);
}
