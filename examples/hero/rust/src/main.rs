use quadraui::prelude::*;

#[derive(Default)]
pub struct Counter {
    pub count: i32,
}

#[derive(Clone)]
pub enum Msg {
    Inc,
    Dec,
}

impl View for Counter {
    type Msg = Msg;
    fn view(&self) -> Node<Msg> {
        column([
            text(format!("Count: {}", self.count)).id("count"),
            row([
                button("-").id("dec").on_click(Msg::Dec),
                button("+").id("inc").on_click(Msg::Inc),
            ]),
        ])
    }
    fn update(&mut self, msg: Msg) -> Command {
        match msg {
            Msg::Inc => self.count += 1,
            Msg::Dec => self.count -= 1,
        }
        Command::None
    }
}

fn main() -> Result<(), quadraui::RunError> {
    run_tui(Counter::default()) // or run_native: the same Counter in a window
}
