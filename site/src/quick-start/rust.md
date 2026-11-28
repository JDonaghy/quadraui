# Quick start: Rust

You need a Rust toolchain ([rustup](https://rustup.rs)). On Linux, the
native window also needs GTK4's development files
(`sudo apt install libgtk-4-dev` on Debian and Ubuntu).

## 1. Make a project

```sh
cargo new counter && cd counter
cargo add quadraui --git https://github.com/JDonaghy/quadraui --branch develop --features tui
```

> **Versions.** These pages track `develop`, which becomes 0.12. The last
> crates.io release is 0.11, which lacks the app layer this page uses, so
> until 0.12 is published take the crate from git as above.
> `cargo add` records the commit it fetched in `Cargo.lock`, so the build
> stays on it until you run `cargo update`. After 0.12 it is
> `cargo add quadraui --features tui`.

## 2. Write the app

Replace `src/main.rs` with this. It is the counter from the README, and CI
builds and clicks this exact file (`examples/hero/rust/src/main.rs`):

<!-- sample: examples/hero/rust/src/main.rs -->
```rust,noplayground
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
            ])
            .content(),
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
```

The three parts:

- **State** is the `Counter` struct.
- **View** is `view()`: it returns a tree of nodes, every frame. `column`
  and `row` stack their children; `text` and `button` are widgets, and
  `.id(..)` names one (tests find it by that name, and focus and widget
  state are kept under it).
- **Update** is `update()`: `.on_click(Msg::Inc)` turns a click on that
  button into the message `Msg::Inc`, and `update` applies it. Every
  message redraws; return `Command::Exit` to quit.

Children share their container equally unless they ask otherwise, exactly
as in Python and Node. Only `text` sizes itself to its lines by default, so
the label takes one row; every other node, `form` and `list` included,
takes an equal share. `.content()` gives the row of buttons its natural
height, and the two buttons share its width; a one-line `form` wants
`.content()` too. `.fixed(px)`, `.weight(w)`, `.percent(p)`,
`.min(px)` and `.max(px)` do what they say, and `.gap(px)` spaces a
container's children. Sizes are surface pixels; the terminal rounds them up
to whole cells of 8×16 pixels, so `.gap(16.0)` between rows is one line.

`q` and Escape quit unless the focused widget takes the key (a text field
types `q`, a list keeps it), and Ctrl-C quits unless a focused text field
takes it: the same rule as Python and Node. A view that wants those keys
overrides `quits_on`; one that wants other keys overrides `on_event`.

## 3. Run it in the terminal

```sh
cargo run
```

Click `+` and `-`. `q` quits.

Run it in a real terminal (tmux and ssh are fine). The terminal backend
asks the terminal for its cursor position at startup, so it fails when
input is piped or there is no terminal; a scripted run goes through
`TuiDriver` instead (step 5).

## 4. Run it in a native window

Enable a native backend feature and call `run_native` instead of
`run_tui`. Nothing else changes:

| Platform | Feature |
|---|---|
| macOS | `cargo add quadraui --features macos` |
| Linux | `cargo add quadraui --features gtk` |
| Windows | `cargo add quadraui --features win` |

`run_native(Counter::default())` opens a window on the native backend this
build enables, and returns `RunError::Backend(BackendError::Unsupported)`
when there is none, so an app can fall back to `run_tui`.
`run_native_with(Counter::default(), NativeOptions::new("Counter"))` also
titles the window, as Python's `run_native(title=...)` and Node's
`runNative(title)` do.

Enable `tui` and a native feature together to choose at run time, for
example from a `--tui` flag.

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

use quadraui::app::Run;
use quadraui::tui::testing::TuiDriver;

fn click_label(driver: &mut TuiDriver<Run<hero::Counter>>, label: &str) {
    let (x, y) = driver
        .find(label)
        .unwrap_or_else(|| panic!("{label:?} not on screen:\n{}", driver.screen()));
    driver.click(x, y);
}

#[test]
fn plus_plus_minus_counts_to_one() {
    let mut driver = TuiDriver::new(Run::new(hero::Counter::default()), 30, 5);
    driver.render();
    assert!(driver.screen_contains("Count: 0"), "{}", driver.screen());
    for label in ["+", "+"] {
        click_label(&mut driver, label);
    }
    assert!(driver.screen_contains("Count: 2"), "{}", driver.screen());
    click_label(&mut driver, "-");
    let screen = driver.screen();
    println!("{screen}");
    assert!(screen.contains("Count: 1"), "{screen}");
    assert_eq!(driver.app().count, 1);
}
```

- The `#[path]` line compiles `src/main.rs` into the test as module
  `hero`, so a binary crate can test its app. In your own project you can
  do the same, or move `Counter` into `src/lib.rs` and `use counter::Counter`.
- `TuiDriver::new(Run::new(app), 30, 5)` is a 30x5-cell terminal. `Run`
  is the adapter `run_tui` uses: it makes a `View` an `AppLogic`, the
  trait every runner and test driver takes. It derefs to your view, so
  `driver.app().count` reads the state. `click(x, y)`
  takes cell coordinates; `find(text)` gives the coordinates of painted
  text. `press(key)` takes a `Key`: `Key::Named(NamedKey::Enter)` or
  `Key::Char('a')`. `type_char(c)`, `ctrl_char(c)`, `screen()` and `app()`
  do what their names say. Tab and bar parts have helpers by id, such as
  `tab_center(&bar_id, 2)`.
- **Two quick clicks are two clicks**: clicks less than 400 ms and 1.5
  cells apart fold into a left `UiEvent::DoubleClick`, which a list, table
  or tree row treats as "open"; a button, or any widget with no
  double-click meaning, sees both presses. Only the left button folds. A
  test that needs two plain `MouseDown`s on a row calls
  `driver.set_double_click_folding(false)` first.

`GtkDriver`, `MacDriver` and `WinDriver` do the same off screen in pixels.

## 6. Next: a text field and a list

The counter uses two widgets. The
[parity app](https://github.com/JDonaghy/quadraui/tree/develop/examples/parity)
is a todo list CI runs in all three languages, with a text field, a list
and a status bar. Its Rust version, `examples/parity/rust/src/main.rs`,
shows the parts the counter does not: a form with a text field, a list
and a status bar, each mapping its events to a message. The state and the
messages:

<!-- sample: examples/parity/rust/src/main.rs -->
```rust,noplayground
pub struct Todo {
    pub draft: String,
    pub todos: Vec<String>,
    pub selected: usize,
    /// Every update, in order: what the parity runner compares.
    pub log: Vec<String>,
}
```

<!-- sample: examples/parity/rust/src/main.rs -->
```rust,noplayground
#[derive(Clone)]
pub enum Msg {
    Edit(String),
    Add,
    Select(usize),
    Clear,
}
```

and the start of the view:

<!-- sample: examples/parity/rust/src/main.rs -->
```rust,noplayground
        let draft = text_field("draft", &self.draft).label("New");
        column([
            form([draft.on_change(Msg::Edit).on_submit(|_| Msg::Add)])
                .id("form")
                .content(),
            list(&self.todos)
                .selected(self.selected)
                .id("todos")
                .on_select(Msg::Select),
```

`on_change` gets the field's whole new text as a `String` and `on_submit`
fires on Enter. `list(items)` makes one row per item (anything
`Into<String>`); `.selected(i)` takes the `usize` row the app shows
selected, and `.on_select` reports a new one as a `usize`. Leave the form's
focused field alone: the runner keeps it while the form has focus, as the
Python and Node engines do. Tab leaves the form after its last field, and
clicking a button moves focus to the button, so type into the field again
by clicking it. `log` records each update for the parity check; your app
does not need it.

Any widget can be a node: `widget(descriptor)` takes one built in full
(it implements `Leaf`), and `.with(|w: &mut ListView| ...)` edits a field
the constructor does not take. The [API reference](https://docs.rs/quadraui)
lists every widget's fields and every `UiEvent`. `.on(|id, event| ...)`
maps any event a widget emits.

## Beyond `View`

`View` is the app layer. Underneath it, `AppLogic` is the trait every
runner takes (`render`, `handle`, `frame_tree`), for an app that paints
and routes by hand. `ShellApp` gives you a VS Code-style frame with the
regions already laid out: the smallest one is
[`quadraui/examples/hello.rs`](https://github.com/JDonaghy/quadraui/blob/develop/quadraui/examples/hello.rs)
(`cargo run --example hello --features tui` in a clone). The `layout`
feature adds flexbox and grid for computing rectangles
([Layout](../concepts/layout.md)).

## More

- [The shape of an app](../concepts/index.md), and how it maps onto Elm,
  iced, Textual and React.
- [Widget gallery](../gallery/index.md): every widget, with its code.
- [API reference on docs.rs](https://docs.rs/quadraui).
- The longer Rust guide:
  [`GUIDE.md`](https://github.com/JDonaghy/quadraui/blob/develop/quadraui/docs/GUIDE.md).
