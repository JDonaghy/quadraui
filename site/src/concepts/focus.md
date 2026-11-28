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
Tab order with no extra code. The focused widget paints a focus ring.

Two things you will notice in the terminal today, both open issues rather
than intended behaviour:

- A `TextDisplay` is a Tab stop. In the counter, Tab therefore visits
  the `Count:` label as well as the buttons, and while the label has
  focus its focus ring covers the text.
- In Python and Node, the focus ring outlines the button's whole share
  of the layout (often several rows tall, since `Column` and `Row` split
  space equally) and is drawn over the button's left bracket, so `[ + ]`
  shows as `│ + ]`. The Rust counter draws no ring.

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
