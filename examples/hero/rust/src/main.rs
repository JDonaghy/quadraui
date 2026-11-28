use quadraui::prelude::*;
use quadraui::{primitive::DynWidget, Button, FrameTree, StyledSpan};
use std::cell::Cell;

#[derive(Default)]
pub struct Counter {
    pub count: i32,
    cell: Cell<(f32, f32)>,
}

impl Counter {
    fn view(&self) -> ScreenLayout {
        let (w, h) = self.cell.get(); // one character cell, in this backend's units
        let text = StyledSpan::plain(format!("Count: {}", self.count));
        let mut label = TextDisplay::new(WidgetId::new("count"));
        label.lines = vec![TextDisplayLine {
            spans: vec![text],
            decoration: Default::default(),
            timestamp: None,
        }];
        let mut ui = ScreenLayout::new();
        let mut put = |col: f32, row: f32, cols: f32, widget: Box<dyn DynWidget>| {
            let rect = Rect::new(col * w, row * h, cols * w, h);
            ui.push(Surface::Widget { rect, widget })
        };
        put(0.0, 0.0, 20.0, Box::new(label));
        put(0.0, 2.0, 5.0, Box::new(Button::new("dec".into(), "-")));
        put(6.0, 2.0, 5.0, Box::new(Button::new("inc".into(), "+")));
        ui
    }
}

impl AppLogic for Counter {
    type AreaId = ();
    fn render(&self, backend: &mut dyn Backend, _: ()) {
        self.cell.set((backend.char_width(), backend.line_height()));
        self.view().draw(backend);
    }
    fn frame_tree(&self, _: ()) -> FrameTree {
        self.view().into_frame_tree()
    }
    fn handle(&mut self, event: UiEvent, _: &mut dyn Backend) -> Reaction {
        match event {
            UiEvent::ButtonActivated { widget } if widget.as_str() == "inc" => self.count += 1,
            UiEvent::ButtonActivated { widget } if widget.as_str() == "dec" => self.count -= 1,
            UiEvent::KeyPressed {
                key: Key::Char('q'),
                ..
            } => return Reaction::Exit,
            _ => return Reaction::Continue,
        }
        Reaction::Redraw
    }
}

fn main() -> Result<(), quadraui::RunError> {
    quadraui::tui::run(Counter::default()) // or gtk::run, macos::run, win::run: same Counter
}
