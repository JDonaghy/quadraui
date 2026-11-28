use quadraui::prelude::*;

/// The parity app: a draft field, a todo list, Add/Clear buttons and a
/// status bar -- the same app as `examples/parity/{python,node}`.
pub struct Todo {
    pub draft: String,
    pub todos: Vec<String>,
    pub selected: usize,
    /// Every update, in order: what the parity runner compares.
    pub log: Vec<String>,
}

impl Default for Todo {
    fn default() -> Self {
        let todos = vec!["Write the README".into(), "Ship 0.12".into()];
        Todo {
            draft: String::new(),
            todos,
            selected: 0,
            log: Vec::new(),
        }
    }
}

#[derive(Clone)]
pub enum Msg {
    Edit(String),
    Add,
    Select(usize),
    Clear,
}

fn segment(text: String) -> StatusBarSegment {
    let (fg, bg) = (Color::rgb(220, 220, 220), Color::rgb(40, 40, 60));
    StatusBarSegment {
        text: format!(" {text} "),
        fg,
        bg,
        bold: false,
        action_id: None,
    }
}

impl View for Todo {
    type Msg = Msg;
    fn view(&self) -> Node<Msg> {
        let draft = text_field("draft", &self.draft).label("New");
        column([
            form([draft.on_change(Msg::Edit).on_submit(|_| Msg::Add)])
                .id("form")
                .content(),
            list(&self.todos)
                .selected(self.selected)
                .id("todos")
                .on_select(Msg::Select),
            row([
                button("Add").id("add").on_click(Msg::Add),
                button("Clear").id("clear").on_click(Msg::Clear),
            ]),
            status_bar(
                [segment(format!("{} todos", self.todos.len()))],
                [segment(format!("selected {}", self.selected))],
            )
            .id("status"),
        ])
    }
    fn update(&mut self, msg: Msg) -> Command {
        match msg {
            Msg::Edit(text) => {
                self.log.push(format!("edit {text}"));
                self.draft = text;
            }
            Msg::Add => {
                self.log.push(format!("add {}", self.draft));
                if !self.draft.is_empty() {
                    self.todos.push(std::mem::take(&mut self.draft));
                }
            }
            Msg::Select(index) => {
                self.log.push(format!("select {index}"));
                self.selected = index;
            }
            Msg::Clear => {
                self.log.push("clear".into());
                self.todos.clear();
                self.selected = 0;
            }
        }
        Command::None
    }
}

#[allow(dead_code)]
fn main() -> Result<(), quadraui::RunError> {
    run_tui(Todo::default())
}
