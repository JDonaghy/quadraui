# Quick start: Rust

You need a Rust toolchain ([rustup](https://rustup.rs)). On Linux, the
native window also needs GTK4's development files
(`sudo apt install libgtk-4-dev` on Debian and Ubuntu).

## 1. Make a project

```sh
cargo new counter && cd counter
cargo add quadraui --features tui
```

> **Versions.** These pages track `develop`, which becomes 0.12. The last
> crates.io release is 0.11, and some APIs used here are newer. Until 0.12
> is published, pin a `develop` commit rather than following its tip (the
> same advice the README gives):
> `cargo add quadraui --git https://github.com/JDonaghy/quadraui --rev <commit-sha> --features tui`

## 2. Write the app

Replace `src/main.rs` with this. It is the counter from the README, and CI
builds and clicks this exact file (`examples/hero/rust/src/main.rs`):

<!-- sample: examples/hero/rust/src/main.rs -->
```rust,noplayground
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
```

The three parts:

- **State** is the `Counter` struct. (`cell` remembers the backend's
  character size between `render` and `frame_tree`; see the note below.)
- **View** is `view()`: it builds a `ScreenLayout` of widgets, every frame.
  `render` paints it; `frame_tree` hands the same widgets to the runtime so
  clicks reach them.
- **Update** is `handle()`: match the event, change state, return
  `Reaction::Redraw` (or `Reaction::Exit`).

Rust has no `App`, `Column` or `Row` (the Python and Node versions do).
`put` places a widget at a column and row, `cols` cells wide and one row
(`h`) high. A widget that needs several rows, such as a list, needs a
height too: add a `rows` argument and use `rows * h`.

## 3. Run it in the terminal

```sh
cargo run
```

Click `+` and `-`. `q` quits.

Run it in a real terminal (tmux and ssh are fine). The terminal backend
asks the terminal for its cursor position at startup, so it fails when
input is piped or there is no terminal.

## 4. Run it in a native window

Swap the backend feature and the runner call. Nothing else changes:

| Platform | Feature | Last line of `main` |
|---|---|---|
| macOS | `cargo add quadraui --features macos` | `quadraui::macos::run(Counter::default())` |
| Linux | `cargo add quadraui --features gtk` | `quadraui::gtk::run(Counter::default())` |
| Windows | `cargo add quadraui --features win` | `quadraui::win::run(Counter::default())` |

Enable several features to choose at run time, for example from a
`--tui` flag.

**Windows:** a binary using the `win` feature must embed the Common-Controls
v6 manifest, or Windows refuses to start it (error 0xc0000139). Copy
[`examples/hero/rust/build.rs`](https://github.com/JDonaghy/quadraui/blob/develop/examples/hero/rust/build.rs)
into your project root.

## 5. Test it headless

`TuiDriver` runs the app against an in-memory terminal: find what was
painted, click it, then read the screen. This is
`examples/hero/rust/tests/headless.rs`:

<!-- sample: examples/hero/rust/tests/headless.rs -->
```rust,noplayground
#[allow(dead_code)]
#[path = "../src/main.rs"]
mod hero;

use quadraui::tui::testing::TuiDriver;

fn click_label(driver: &mut TuiDriver<hero::Counter>, label: &str) {
    let (x, y) = driver
        .find(label)
        .unwrap_or_else(|| panic!("{label:?} not on screen:\n{}", driver.screen()));
    driver.click(x, y);
}

#[test]
fn plus_minus_plus_counts_to_one() {
    let mut driver = TuiDriver::new(hero::Counter::default(), 30, 5);
    driver.render();
    assert!(driver.screen_contains("Count: 0"), "{}", driver.screen());
    // `+ - +`, not `+ + -`: two clicks on one spot this fast fold into a
    // DoubleClick, which a Button does not treat as a second activation
    // (see examples/hero/README.md, "Gaps").
    for label in ["+", "-", "+"] {
        click_label(&mut driver, label);
    }
```

- The `#[path]` line compiles `src/main.rs` into the test as module
  `hero`, so a binary crate can test its app. In your own project you can
  do the same, or move `Counter` into `src/lib.rs` and `use counter::Counter`.
- `TuiDriver::new(app, 30, 5)` is a 30x5-cell terminal. `click(x, y)`
  takes cell coordinates; `find(text)` gives the coordinates of painted
  text. `press(key)`, `type_char(c)`, `screen()` and `app()` do what their
  names say. Tab and bar parts have helpers by id, such as
  `tab_center(&bar_id, 2)`.
- **Two clicks on the same spot count once**: clicks less than 400 ms and
  1.5 cells apart fold into a double-click, which a button does not treat
  as a second press. A test that means two separate clicks calls
  `driver.set_double_click_folding(false)` first. A user clicking quickly
  in a live app hits the same limit today.

`GtkDriver`, `MacDriver` and `WinDriver` do the same off screen in pixels.

## 6. Next: a text field and a list

The counter uses two widgets. The
[parity app](https://github.com/JDonaghy/quadraui/tree/develop/examples/parity)
is a todo list CI runs in all three languages, with a text field, a list
and a status bar. Its Rust version, `examples/parity/rust/src/main.rs`,
shows the parts the counter does not. `ListView` and `ListItem` have no
`new()` or `Default` yet, so every field is spelled out:

<!-- sample: examples/parity/rust/src/main.rs -->
```rust,noplayground
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
```

A text field is a `FormField` with `kind: FieldKind::TextInput { .. }`
inside a `Form`; the file builds one in full. Their events, matched in
`handle`:

<!-- sample: examples/parity/rust/src/main.rs -->
```rust,noplayground
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
```

`TextInputChanged` carries the whole new value; `TextInputCommitted` is
Enter. The form's focused field is app state in Rust, which is why the app
tracks `FocusChanged`. `log` records each update for the parity check;
your app does not need it. The
[API reference](https://docs.rs/quadraui) lists every widget's fields and
every `UiEvent`.

## About the hand-placed layout

The Rust view places each widget in a `Rect` by hand, which is why it is
longer than the [Python](python.md) and [Node](node.md) versions. Those get
`Column` and `Row` from their app layer; a Rust equivalent is proposed in
[`examples/hero/README.md`](https://github.com/JDonaghy/quadraui/blob/develop/examples/hero/README.md)
but not built yet. For real apps today, `ShellApp` gives you a VS Code-style
frame with the regions already laid out: the smallest one is
[`quadraui/examples/hello.rs`](https://github.com/JDonaghy/quadraui/blob/develop/quadraui/examples/hello.rs)
(`cargo run --example hello --features tui` in a clone). The `layout`
feature adds flexbox and grid for computing rectangles
([Layout](../concepts/layout.md)); it has no worked example in these pages
yet, so see `quadraui::flex` in the [API reference](https://docs.rs/quadraui).

## More

- [The shape of an app](../concepts/index.md), and how it maps onto Elm,
  iced, Textual and React.
- [Widget gallery](../gallery/index.md): every widget, with its code.
- [API reference on docs.rs](https://docs.rs/quadraui).
- The longer Rust guide:
  [`GUIDE.md`](https://github.com/JDonaghy/quadraui/blob/develop/quadraui/docs/GUIDE.md).
