# Quick start: Python

You need Python 3.8 or newer. Nothing else: the wheel carries the Rust
renderer.

> **Versions.** These pages track `develop`, which becomes quadraui 0.12.
> The `quadraui` wheel on PyPI is the last release (0.11 today); it has
> the `quadraui.app` layer used here, but a detail on these pages can be
> newer. To run exactly what they describe, build from a clone:
> `cd quadraui/quadraui-py && maturin develop --release`.

## 1. Install

```sh
python3 -m venv .venv && . .venv/bin/activate
pip install quadraui
```

## 2. Write the app

Save this as `counter.py`. It is the counter from the README, and CI runs
this exact file (`examples/hero/python/counter.py`):

<!-- sample: examples/hero/python/counter.py -->
```python
from quadraui.app import App, Button, Column, Row, TextDisplay, key_name


def text(s):
    return [{"spans": [{"text": s, "fg": None, "bg": None}]}]


class Counter:
    def __init__(self):
        self.count = 0

    def view(self):
        return Column(
            TextDisplay(text(f"Count: {self.count}"), id="count"),
            Row(Button("-", id="dec", on_click=self.dec), Button("+", id="inc", on_click=self.inc)),
        )

    def inc(self):
        self.count += 1

    def dec(self):
        self.count -= 1

    def on_key(self, event):
        if key_name(event) == "q":
            raise SystemExit


if __name__ == "__main__":
    counter = Counter()
    App(30, 5, view=counter.view, on_event=counter.on_key).run_tui()  # or .run_native(title="Counter"): same Counter
```

The three parts:

- **State** is the `Counter` object's attributes.
- **View** is `view()`: it returns a tree of nodes (`Column`, `Row`,
  `Button`, ...) and runs again after every event, so a changed `count`
  shows up without any binding or redraw call.
- **Update** is the callbacks: `on_click=self.inc` on a node, and
  `on_event` for input no single widget owns, such as the `q` key.

Three details the sample does not explain:

- **`text()`.** A `TextDisplay` takes a list of lines, and each line is a
  list of styled spans. `fg` and `bg` set a span's colours; `None` (or
  leaving the key out) means the theme's colours. There is no plain-string
  shortcut yet, so the sample defines `text()` for one unstyled line.
- **`App(30, 5, ...)`** is the size in character cells (columns, rows)
  for a headless run, such as the test in step 5. A live `run_tui()` fills
  the terminal and `run_native()` opens a window, and both ignore it.
- **Layout.** `Column` and `Row` split their space equally between their
  children, so in a full-size terminal the label takes the top half and
  each button half the width. Per-child sizes are not exposed yet
  ([Layout](../concepts/layout.md)).

## 3. Run it in the terminal

```sh
python counter.py
```

Click `+` and `-`, or Tab to a button and press Enter. `q` quits.

Run it in a real terminal (tmux and ssh are fine). The terminal backend
asks the terminal for its cursor position at startup, so it fails with
"The cursor position could not be read" when input is piped or comes from
`script`, and with "No such device or address" when there is no terminal
at all. Under tmux an Escape key arrives after tmux's `escape-time` delay,
so bind quitting to a letter, as the sample does.

## 4. Run it in a native window

Change the last line's `.run_tui()` to `.run_native(title="Counter")`.

- **macOS and Windows:** that is all. The wheels from PyPI include the
  native backend.
- **Linux:** the PyPI wheel is terminal-only, because GTK cannot be bundled
  into a portable wheel. Build the package from source with the GTK backend
  (needs `libgtk-4-dev` and a Rust toolchain):

  ```sh
  pip install maturin
  git clone https://github.com/JDonaghy/quadraui
  cd quadraui/quadraui-py && maturin develop --release --features gtk
  ```

To check whether your installed build has a native backend:

```sh
python -c "import quadraui; print(hasattr(quadraui.QuiApp, 'run_native'))"
```

(`QuiApp` is the lower-level app object `App` runs on.)

## 5. Test it headless

`quadraui.testing.harness(app)` renders the app off screen. You click what
was painted, by its text, and read the frame back. This is
`examples/hero/python/run_headless.py`:

<!-- sample: examples/hero/python/run_headless.py -->
```python
from counter import Counter  # noqa: E402
from quadraui.app import App  # noqa: E402
from quadraui.testing import harness  # noqa: E402

counter = Counter()
h = harness(App(30, 5, view=counter.view))
assert h.contains("Count: 0"), h.screen()
# `+ - +`, not `+ + -`: two clicks on one spot this fast fold into a
# DoubleClick, which a Button does not treat as a second activation (see
# examples/hero/README.md, "Gaps").
for label in ("+", "-", "+"):
    h.click_text(label)
print(h.screen())
assert h.contains("Count: 1"), h.screen()
assert counter.count == 1
```

The harness methods you will use most:

| Method | Does |
|---|---|
| `click_text(text, nth=0)` | Clicks the `nth` place `text` is painted. |
| `click_at(x, y)` | Clicks a cell. |
| `press(key, shift=False, ctrl=False, ...)` | Presses a named key (`"Enter"`, `"Escape"`, `"Down"`) or one character. |
| `type(text)` | Types each character in turn. |
| `contains(text)`, `assert_text(*texts)` | Checks the frame. |
| `screen()`, `screen_lines()` | The frame as text. |
| `find(id=..., role=..., text=...)` | A widget, to read (`.text`, `.state`) or act on (`.click()`, `.focus()`, `.row(...)`). |

**Two clicks on the same spot count once.** Clicks less than 400 ms and
1.5 cells apart fold into a double-click, and a button ignores the second
half of one. This is why the test clicks `+ - +` rather than `+ + -`. To
press the same button twice, click it once (which also focuses it) and
then `h.press("Enter")`. A user clicking quickly in a live app hits the
same limit today.

The package also ships a pytest fixture, `quadraui_harness`, and an
`AGENTS.md` guide for writing apps with a coding agent.

## 6. Next: a text field and a list

The counter uses two widgets. The
[parity app](https://github.com/JDonaghy/quadraui/tree/develop/examples/parity),
a todo list CI runs in all three languages, adds a text field, a list and
a status bar. List items are one line each (`{"text": {"spans": [...]}}`),
unlike a `TextDisplay`, which takes a list of lines:

<!-- sample: examples/parity/python/todo.py -->
```python
def item(text):
    return {"text": {"spans": [{"text": text, "fg": None, "bg": None}]}}
```

<!-- sample: examples/parity/python/todo.py -->
```python
    def view(self):
        return Column(
            Form([TextInputField(self.draft, id="draft", label="New", on_change=self.edit, on_submit=self.add)], id="form"),
            List([item(t) for t in self.todos], id="todos", on_select=self.select),
            Row(Button("Add", id="add", on_click=self.add), Button("Clear", id="clear", on_click=self.clear)),
            StatusBar([segment(f"{len(self.todos)} todos")], [segment(f"selected {self.selected}")], id="status"),
        )
```

- A text field lives in a `Form`. `on_change(text)` gets the whole new
  value after each keystroke; `on_submit(text)` fires on Enter.
- `on_select(index)` gets the selected row's index. Pass `selected=` to
  pin the selection yourself ([State](../concepts/state.md)).
- `self.log` in that file records each update for the parity check; your
  app does not need it.

## More

- [The shape of an app](../concepts/index.md), and how it maps onto
  Textual, React, Elm and iced.
- [Events](../concepts/events.md): every callback, and the key names
  `key_name` returns.
- A larger app: `quadraui-py/examples/task_list_demo.py` (list, form,
  confirm dialog, status bar) and `file_manager_app.py`.
- [Hot reload](https://github.com/JDonaghy/quadraui/tree/develop/quadraui-py/examples/hot_reload):
  `App.enable_hot_reload(module)` reloads your view code and keeps state.
- [The same app in Rust and TypeScript](../bindings.md).
