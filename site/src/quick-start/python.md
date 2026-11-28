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
from quadraui.app import App, Button, Column, Row, Text


class Counter:
    def __init__(self):
        self.count = 0

    def view(self):
        return Column(
            Text(f"Count: {self.count}", id="count"),
            Row(Button("-", id="dec", on_click=self.dec), Button("+", id="inc", on_click=self.inc)),
        )

    def inc(self):
        self.count += 1

    def dec(self):
        self.count -= 1


if __name__ == "__main__":
    App(view=Counter().view).run_tui()  # q quits; or .run_native(title="Counter"): same Counter
```

The three parts:

- **State** is the `Counter` object's attributes.
- **View** is `view()`: it returns a tree of nodes (`Column`, `Row`,
  `Button`, ...) and runs again after every event, so a changed `count`
  shows up without any binding or redraw call.
- **Update** is the callbacks: `on_click=self.inc` on a node, and
  `on_event` for input no single widget owns, such as a global shortcut.

Three details the sample does not explain:

- **`Text("...")`** is one line of plain text, sized to its content. For
  colours, use a `TextDisplay`: it takes a list of lines, each a list of
  styled spans with optional `fg` and `bg`.
- **`App(...)`** takes an optional size in character cells, `App(30, 5,
  view=...)`, for a headless run such as the test in step 5; it defaults
  to 80x24. A live `run_tui()` fills the terminal and `run_native()` opens
  a window, and both ignore it.
- **Layout.** `Column` and `Row` split their space equally between the
  children that do not ask for a size. `Text` asks for its content size, so
  the label takes one row and the buttons the rest. Any node takes `size=`
  (pixels as an `int`, `"30%"`, `"2fr"` or `"content"`) and `min=`/`max=`
  ([Layout](../concepts/layout.md)).
- **Quitting.** Ctrl-C always quits, and `q` or Escape quit unless a
  focused widget used the key (a text field takes `q` as a letter). Call
  `app.quit()` from any callback to quit yourself, and pass
  `App(..., quit_keys=False)` to turn the keys off.

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
so `q` quits faster there.

## 4. Run it in a native window

Change the last line's `.run_tui()` to `.run_native(title="Counter")`.

- **macOS and Windows:** that is all. The wheels from PyPI include the
  native backend.
- **Linux (x86_64, glibc 2.39+: Ubuntu 24.04+, Fedora 40+, Debian 13+):**
  the PyPI wheel uses the system GTK 4 (4.10 or newer). Install it once:

  ```sh
  sudo apt install libgtk-4-1    # Ubuntu / Debian
  sudo dnf install gtk4          # Fedora
  ```

- **Other Linux hosts** (aarch64, which has no prebuilt wheel, or an older
  glibc such as Ubuntu 22.04) get a terminal-only build. Build from source with the GTK backend instead
  (needs `libgtk-4-dev` or `gtk4-devel`, GTK 4.10 or newer, and a Rust
  toolchain):

  ```sh
  MATURIN_PEP517_ARGS="--features gtk" pip install --no-binary quadraui quadraui
  ```

To check whether your installed build has a native backend:

```sh
python -c "import quadraui._quadraui as m; print(m._NATIVE_BACKEND)"
```

It prints `gtk`, `macos` or `win`, or `None` when there is none. Where there
is none, `run_native()` raises `quadraui.UnsupportedError` with the reason
and the fix, so an app can catch it and fall back to `run_tui()`.

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
    return {"text": {"spans": [{"text": text}]}}
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
