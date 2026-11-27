//! `OneWidgetApp`: a single contract widget drawn and routed as a
//! `Surface::Widget` through `TuiDriver`, recording every `UiEvent` the
//! routing engine hands back and applying it onto the widget with the
//! test's own `apply` function, the way a real host owns a controlled
//! widget's visible state.

#![allow(dead_code)]

use quadraui::interactive::Interactive;
use quadraui::tui::testing::TuiDriver;
use quadraui::{AppLogic, Backend, FrameTree, Reaction, Rect, ScreenLayout, Surface, UiEvent};

/// Applies one routed event onto the widget; returns whether it changed.
pub type Apply<W> = fn(&mut W, &UiEvent) -> bool;

pub struct OneWidgetApp<W> {
    pub widget: W,
    pub rect: Rect,
    pub events: Vec<UiEvent>,
    apply: Apply<W>,
}

impl<W: Interactive + Clone + 'static> AppLogic for OneWidgetApp<W> {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let mut frame = ScreenLayout::new();
        frame.push(Surface::Widget {
            rect: self.rect,
            widget: Box::new(self.widget.clone()),
        });
        frame.draw(backend);
    }

    fn frame_tree(&self, _area: ()) -> FrameTree {
        let mut tree = FrameTree::new();
        let id = *self.widget.id().expect("a contract widget names itself");
        tree.push(id, self.rect, Box::new(self.widget.clone()));
        tree
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        let changed = (self.apply)(&mut self.widget, &event);
        self.events.push(event);
        if changed {
            Reaction::Redraw
        } else {
            Reaction::Continue
        }
    }
}

/// A driver over `widget` painted at `rect` on a `cols`×`rows` grid.
pub fn driver<W: Interactive + Clone + 'static>(
    widget: W,
    rect: Rect,
    cols: u16,
    rows: u16,
    apply: Apply<W>,
) -> TuiDriver<OneWidgetApp<W>> {
    let mut driver = TuiDriver::new(
        OneWidgetApp {
            widget,
            rect,
            events: Vec::new(),
            apply,
        },
        cols,
        rows,
    );
    driver.set_double_click_folding(false);
    driver
}
