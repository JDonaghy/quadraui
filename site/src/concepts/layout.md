# Layout

A view says **what** goes where in relative terms; quadraui works out the
rectangles for the current window or terminal size, in that backend's
units (pixels, or character cells in a terminal).

## Containers

`Column` stacks children top to bottom, `Row` left to right. `Split` makes
two resizable panes, `Tabs` a tabbed area, `Scroll` a scrolling viewport.
They nest. From `quadraui-py/examples/task_list_demo.py`:

<!-- sample: quadraui-py/examples/task_list_demo.py -->
```python
        detail = Column(Form(fields, id="tasks:form"), Button("Delete", id="tasks:delete", on_click=self._confirm_delete))
        return Column(
            Row(task_list, detail),
```

In JSX the same containers are `<column>`, `<row>`, `<split>` and so on.

`Column` and `Row` take `gap`, `padding` and `margin`. Their children share
the space equally unless a child asks for a size along the stack's main axis
(height in a `Column`, width in a `Row`). Every node takes one, except
`Slider`, whose `min`/`max` are its value range:

| `size=` (Python) / `size` (Node) | Takes |
|---|---|
| `32` | that many pixels; a terminal cell is 8 px wide and 16 px tall |
| `"30%"` | that share of the parent |
| `"2fr"` | a weight: what is left is shared out by weight |
| `"content"` | the child's natural size (a text's lines, a list's rows) |
| left out | an equal share of what is left |

`min` and `max` clamp the result, in the same pixels. `Text("...")` and
`<text>` default to `"content"`, so the counter's label takes one row; its
row of buttons asks for `"content"` too, and the two buttons share the
row's width:

<!-- sample: untested — a fragment; the sizes are tested in tests/test_app_ergonomics.py -->
```python
Column(Text("Title"), List(items, id="items"), Row(..., size=32))
```

<!-- sample: untested — a fragment; the sizes are tested in tests/app_ergonomics.test.ts -->
```tsx
<column><text>Title</text><list id="items" items={items} /><row size={32}>...</row></column>
```

The root of the tree fills the whole window or terminal. The optional size
you pass to `App(width, height, ...)` (80x24 by default) is used only by a
headless run (the test harness); a live `run_tui()` or `run_native()`
ignores it.

## Rust: the same containers

A Rust [`View`](../quick-start/rust.md) builds the same tree with
`column([...])` and `row([...])`. A child asks for its size with a method
instead of `size=`, in the same pixels:

| Rust | Python / Node equivalent |
|---|---|
| `.fixed(32)` | `32` |
| `.percent(0.3)` | `"30%"` |
| `.weight(2.0)` | `"2fr"` |
| `.content()` | `"content"` |
| left out | an equal share of what is left |

`.min(px)` and `.max(px)` clamp, and containers take `.gap(px)` and
`.padding(px)`. `text("...")` is content-sized, as in Python and Node:

<!-- sample: untested — a fragment; the sizes are tested in quadraui/tests/app_view_driver.rs -->
```rust
column([text("Title"), list(items).id("items"), row([...]).fixed(32)])
```

Below the app layer, `ShellApp` gives you a VS Code-style frame (activity
bar, sidebar, editor area, panel, status bar) with every region already
laid out; see
[`GUIDE.md`](https://github.com/JDonaghy/quadraui/blob/develop/quadraui/docs/GUIDE.md).
`ScreenLayout` takes widgets at rectangles you compute, and the `layout`
feature adds flexbox and grid (`quadraui::flex`, built on
[Taffy](https://github.com/DioxusLabs/taffy)) for computing them.

## Paint and click agree

Each widget resolves its geometry once, in `layout()`. Painting and
hit-testing both read that result, so a click always lands on what is drawn,
on every backend.

## If you know...

- **CSS flexbox / React Native:** `Column` and `Row` are flex containers
  with a fixed direction. Rust's `layout` feature is real flexbox (Taffy,
  the engine behind Dioxus and Bevy UI).
- **iced:** `column![]` and `row![]` are the same idea; Rust's
  `column([...])` and `row([...])` are functions rather than macros.
- **Textual:** `Vertical` and `Horizontal` containers; there is no CSS file.
