# Focus

One widget at a time has **keyboard focus**. Keys go to it first; keys it
does not use reach your app (`on_event` in Python and Node, `handle` in
Rust), which is how the counter's `q` works while a button has focus.

- **Tab** and **Shift+Tab** move focus through every focus stop on screen,
  in the order the widgets declare them.
- **Clicking** a focusable widget focuses it.
- A widget can have several stops: each row of a list, each field of a
  form, each cell of a grid.

Widgets declare their stops themselves (`Interactive::focus()` in Rust,
returning none, one stop, or one per child), so a screen gets a sensible
Tab order with no extra code. Plain text is not a stop: a `TextDisplay`
takes focus only when it shows a scrollbar (`show_scrollbar`), so the
keyboard can scroll it.

The focused widget shows a focus ring, the same in Rust, Python and Node.
In the terminal the ring never covers what the widget draws: a focused
button keeps `[ + ]` intact, its brackets turn the accent colour and the
label is underlined at its own width, not the width of its layout cell. A
taller widget, such as a list, gets a box drawn in the blank cells around
its text. A form shows its focused field with its own highlight instead.

## Setting focus

In Python and Node, pass `has_focus` / `hasFocus` to the widget that should
hold focus, as the task list does: `List(..., has_focus=True)`. In Rust a
widget asks for focus from its event handler through `EventCx::request_focus`.

A modal overlay, such as a dialog, holds focus until it closes; see
[Popups](popups.md).

## If you know...

- **The web:** focus stops are `tabindex`, already set on every widget.
- **iced:** like `focus_next()` / `text_input::focus(id)`, without managing
  ids for the Tab order.
- **Textual:** `can_focus` and `focus()`, with the same Tab and Shift+Tab
  behaviour.
