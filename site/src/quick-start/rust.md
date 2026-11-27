# Quick start: Rust

You need a Rust toolchain ([rustup](https://rustup.rs)). On Linux, the
native window also needs GTK4's development files
(`sudo apt install libgtk-4-dev` on Debian and Ubuntu).

## 1. Make a project

```sh
cargo new counter && cd counter
cargo add quadraui --features tui
```

> Until 0.12 is on crates.io, add it from git instead:
> `cargo add quadraui --git https://github.com/JDonaghy/quadraui --branch develop --features tui`

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

## 3. Run it in the terminal

```sh
cargo run
```

Click `+` and `-`. `q` quits.

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

`TuiDriver` runs the app against an in-memory terminal: click what was
painted, then read the screen. This is `examples/hero/rust/tests/headless.rs`:

<!-- sample: examples/hero/rust/tests/headless.rs -->
```rust,noplayground
#[test]
fn plus_minus_plus_counts_to_one() {
    let mut driver = TuiDriver::new(hero::Counter::default(), 30, 5);
    driver.render();
    assert!(driver.screen_contains("Count: 0"), "{}", driver.screen());
```

`GtkDriver`, `MacDriver` and `WinDriver` do the same off screen in pixels.

## About the hand-placed layout

The Rust view places each widget in a `Rect` by hand, which is why it is
longer than the [Python](python.md) and [Node](node.md) versions. Those get
`Column` and `Row` from their app layer; a Rust equivalent is proposed in
[`examples/hero/README.md`](https://github.com/JDonaghy/quadraui/blob/develop/examples/hero/README.md)
but not built yet. For real apps today, `ShellApp` gives you a VS Code-style
frame with the regions already laid out, and the `layout` feature adds
flexbox and grid ([Layout](../concepts/layout.md)).

## Next

- [The shape of an app](../concepts/index.md), and how it maps onto Elm,
  iced, Textual and React.
- [Widget gallery](../gallery/index.md): every widget, with its code.
- [API reference on docs.rs](https://docs.rs/quadraui).
- The longer Rust guide:
  [`GUIDE.md`](https://github.com/JDonaghy/quadraui/blob/develop/quadraui/docs/GUIDE.md).
