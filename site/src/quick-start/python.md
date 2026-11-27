# Quick start: Python

You need Python 3.8 or newer. Nothing else: the wheel carries the Rust
renderer.

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

`text()` builds the one-line styled text a `TextDisplay` takes.

## 3. Run it in the terminal

```sh
python counter.py
```

Click `+` and `-`, or Tab to a button and press Enter. `q` quits.

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

`hasattr(QuiApp, "run_native")` tells you whether your installed build has
a native backend.

## 5. Test it headless

`quadraui.testing.harness` renders the app off screen and clicks what was
painted, by its text. This is `examples/hero/python/run_headless.py`:

<!-- sample: examples/hero/python/run_headless.py -->
```python
counter = Counter()
h = harness(App(30, 5, view=counter.view))
assert h.contains("Count: 0"), h.screen()
```

The package also ships a pytest fixture, `quadraui_harness`, and an
`AGENTS.md` guide for writing apps with a coding agent.

## Next

- [The shape of an app](../concepts/index.md), and how it maps onto
  Textual, React, Elm and iced.
- A larger app: `quadraui-py/examples/task_list_demo.py` (list, form,
  confirm dialog, status bar) and `file_manager_app.py`.
- [Hot reload](https://github.com/JDonaghy/quadraui/tree/develop/quadraui-py/examples/hot_reload):
  `App.enable_hot_reload(module)` reloads your view code and keeps state.
- [The same app in Rust and TypeScript](../bindings.md).
