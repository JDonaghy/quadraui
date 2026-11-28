use quadraui::prelude::*;
use quadraui::{Button, FormEvent, FrameTree, ListViewEvent};

/// The parity app: a draft field, a todo list, Add/Clear buttons and a
/// status bar -- the same app as `examples/parity/{python,node}`.
pub struct Todo {
    pub draft: String,
    pub todos: Vec<String>,
    pub selected: usize,
    /// The form field holding keyboard focus, as the form last reported it.
    pub focused_field: Option<WidgetId>,
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
            focused_field: None,
            log: Vec::new(),
        }
    }
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

/// `n` equal shares of `0..len`, the split the Python/Node `Column`/`Row` make.
fn shares(len: f32, n: usize) -> Vec<(f32, f32)> {
    let edge = |i: usize| (len * i as f32 / n as f32).floor();
    (0..n).map(|i| (edge(i), edge(i + 1) - edge(i))).collect()
}

impl Todo {
    fn view(&self, width: f32, height: f32) -> ScreenLayout {
        let draft = FormField {
            id: WidgetId::new("draft"),
            label: StyledText::plain("New"),
            kind: FieldKind::TextInput {
                value: self.draft.clone(),
                placeholder: String::new(),
                cursor: None,
                selection_anchor: None,
            },
            hint: StyledText::default(),
            disabled: false,
            validation: None,
            label_position: None,
            label_align: None,
            label_width: None,
            tab_index: None,
            control_width: None,
            min_control_width: None,
            row_align: None,
        };
        let form = Form {
            id: WidgetId::new("form"),
            fields: vec![draft],
            focused_field: self.focused_field,
            scroll_offset: 0,
            has_focus: self.focused_field.is_some(),
            label_position: Default::default(),
            label_align: Default::default(),
            label_width: None,
            control_width: Default::default(),
            min_control_width: None,
            spacing: Default::default(),
        };
        let items = self.todos.iter().map(|t| ListItem {
            text: StyledText::plain(t),
            icon: None,
            detail: None,
            decoration: Default::default(),
        });
        let list = ListView {
            id: WidgetId::new("todos"),
            title: None,
            items: items.collect(),
            selected_idx: self.selected,
            scroll_offset: 0,
            items_window: None,
            scroll_px: None,
            has_focus: false,
            bordered: false,
            h_scroll: 0,
            max_content_width: None,
            show_v_scrollbar: false,
            striped: false,
        };
        let status = StatusBar {
            id: WidgetId::new("status"),
            left_segments: vec![segment(format!("{} todos", self.todos.len()))],
            right_segments: vec![segment(format!("selected {}", self.selected))],
        };
        let rows = shares(height, 4);
        let cols = shares(width, 2);
        let mut ui = ScreenLayout::new();
        let mut put =
            |x: (f32, f32), y: (f32, f32), widget: Box<dyn quadraui::primitive::DynWidget>| {
                ui.push(Surface::Widget {
                    rect: Rect::new(x.0, y.0, x.1, y.1),
                    widget,
                })
            };
        put((0.0, width), rows[0], Box::new(form));
        put((0.0, width), rows[1], Box::new(list));
        put(cols[0], rows[2], Box::new(Button::new("add".into(), "Add")));
        put(
            cols[1],
            rows[2],
            Box::new(Button::new("clear".into(), "Clear")),
        );
        put((0.0, width), rows[3], Box::new(status));
        ui
    }

    fn add(&mut self) {
        self.log.push(format!("add {}", self.draft));
        if !self.draft.is_empty() {
            self.todos.push(std::mem::take(&mut self.draft));
        }
    }
}

/// The parity runner drives this app on a 40x12 cell grid.
const SIZE: (f32, f32) = (40.0, 12.0);

impl AppLogic for Todo {
    type AreaId = ();
    fn render(&self, backend: &mut dyn Backend, _: ()) {
        let vp = backend.viewport();
        self.view(vp.width, vp.height).draw(backend);
    }
    fn frame_tree(&self, _: ()) -> FrameTree {
        self.view(SIZE.0, SIZE.1).into_frame_tree()
    }
    fn extra_tab_stops(&self, _: ()) -> Vec<(WidgetId, Rect)> {
        self.view(SIZE.0, SIZE.1).tab_stops()
    }
    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
        // The form's focused field is app state here, so drop it once focus has left the form.
        if !matches!(backend.focus_tree().focused(), Some((id, _)) if id.as_str() == "form") {
            self.focused_field = None;
        }
        match event {
            UiEvent::Form(_, FormEvent::TextInputChanged { value, .. }) => {
                self.log.push(format!("edit {value}"));
                self.draft = value;
            }
            UiEvent::Form(_, FormEvent::TextInputCommitted { .. }) => self.add(),
            UiEvent::Form(_, FormEvent::FocusChanged { id }) => self.focused_field = Some(id),
            UiEvent::List(_, ListViewEvent::SelectionChanged { idx }) => {
                self.log.push(format!("select {idx}"));
                self.selected = idx;
            }
            UiEvent::ButtonActivated { widget } if widget.as_str() == "add" => self.add(),
            UiEvent::ButtonActivated { widget } if widget.as_str() == "clear" => {
                self.log.push("clear".into());
                self.todos.clear();
                self.selected = 0;
            }
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Escape),
                ..
            } => return Reaction::Exit,
            _ => return Reaction::Continue,
        }
        Reaction::Redraw
    }
}

#[allow(dead_code)]
fn main() -> Result<(), quadraui::RunError> {
    quadraui::tui::run(Todo::default())
}
