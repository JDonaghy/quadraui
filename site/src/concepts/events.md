# Events

Input arrives as an **event**: a key, a click, a scroll, or a semantic
action a widget produced from one ("button `inc` was activated", "row 3 of
`files` was selected"). The runtime does the hit-testing. Your code never
sees a mouse coordinate unless it wants one.

## Python and Node: callbacks on nodes

Attach a handler to the node that should react. The same names exist in
both languages, `snake_case` in Python and `camelCase` in Node:

<!-- sample: examples/hero/python/counter.py -->
```python
                Button("-", id="dec", on_click=self.dec),
                Button("+", id="inc", on_click=self.inc),
```

<!-- sample: examples/hero/node/counter.tsx -->
```tsx
          <button id="dec" label="-" onClick={() => this.count--} />
          <button id="inc" label="+" onClick={() => this.count++} />
```

For input that belongs to no widget, such as a global shortcut, pass
`on_event` / `onEvent` to `App`. `key_name(event)` / `keyName(event)` turns
a key event into a name:

- a printable character is itself: `"q"`, `"A"`, `" "` (space);
- other keys have names: `"Enter"`, `"Escape"`, `"Backspace"`, `"Delete"`,
  `"Up"`, `"Down"`, `"Left"`, `"Right"`, `"Home"`, `"End"`, `"PageUp"`,
  `"PageDown"`, `"F1"` to `"F12"`;
- anything that is not a key event gives `None` / `null`.

The focused widget sees a key first, so `on_event` gets only the keys it
did not use: Enter on a focused button presses the button, and Tab moves
focus ([Focus](focus.md)).

## Rust: messages and one `update`

A Rust node maps its events to a message value you define, and one
`update` applies it, as in Elm and iced:

<!-- sample: examples/hero/rust/src/main.rs -->
```rust,noplayground
                button("-").id("dec").on_click(Msg::Dec),
                button("+").id("inc").on_click(Msg::Inc),
```

<!-- sample: examples/hero/rust/src/main.rs -->
```rust,noplayground
    fn update(&mut self, msg: Msg) -> Command {
        match msg {
            Msg::Inc => self.count += 1,
            Msg::Dec => self.count -= 1,
        }
        Command::None
    }
```

Every message redraws; `update` returns `Command::Exit` to quit. Events no
widget took go to the view's optional `on_event(&self, &UiEvent)`, which
may return a message, and then to `quits_on`: by default `q`, Escape and
Ctrl-C quit. `.on(|id, event| ...)` on any node maps any `UiEvent` the
widget emits.

## Common events

What each Python / Node callback receives, and the Rust event it answers:

| Widget | Python / Node callback | Rust node handler, and the `UiEvent` it answers |
|---|---|---|
| Button | `on_click()` / `onClick()` | `.on_click(msg)`: `ButtonActivated { widget }` |
| List | `on_select(index)`, `on_activate(index)`, `on_selection(indices)`, `on_context_menu(index, position)`, `on_click(index, button)` | `.on_select(Msg::Select)`: `List(id, ListViewEvent::SelectionChanged { idx })`, `ItemActivated { idx }`, `SelectionSet { indices }`, `ContextMenuRequested { idx, position }`, `ItemClicked { idx, button, .. }` |
| Tree | `on_select(path)`, `on_activate(path)`, `on_row_click(path)`, `on_toggle_expand(path)`, `on_selection(paths)`, `on_context_menu(path, position)`; a path is a list of indices | `Tree(id, TreeEvent::SelectionChanged { path })`, `RowActivated`, `RowClicked { path, button, .. }`, `RowToggleExpand`, `SelectionSet { paths }`, `ContextMenuRequested { path, position }` |
| Table | `on_select(row)`, `on_activate(row)`, `on_header_click(column)`, `on_selection(rows)`, `on_context_menu(row, position)`, `on_click(row, button)` | `DataTable(id, DataTableEvent::RowSelected { idx })`, `RowActivated`, `HeaderClicked { col }`, `SelectionSet { indices }`, `ContextMenuRequested { idx, position }`, `RowClicked { idx, button, .. }` |
| Tabs | `on_activate(index)`, `on_close(index)` | `TabBar(id, TabBarEvent::TabActivated { index })`, `TabClosed { index }` |
| Status bar | `on_segment_click(segment_id)` | `StatusBar(id, StatusBarEvent::SegmentClicked { id })` |
| Text field (in a `Form`) | `on_change(text)`, `on_submit(text)` | `text_field(..).on_change(Msg::Edit)`, `.on_submit(..)`: `Form(id, FormEvent::TextInputChanged { value, .. })`, `TextInputCommitted { .. }` |
| Any key | `on_event(event)` + `key_name` | `View::on_event`: `KeyPressed { key, modifiers, .. }` |

`on_change` gets the field's whole new value, not the key that changed it.
The full list of events and their payloads is generated from one Rust
schema into [the bindings' wire types](../bindings.md#reference); for Rust,
see `UiEvent` in the [API reference](https://docs.rs/quadraui).

## Double-clicks and other buttons

Two left clicks less than 400 ms and 1.5 cells apart fold into a
double-click (`UiEvent::DoubleClick { button: Left, .. }`). A list, table
or tree row treats it as "open" (`on_activate`); a button, or any widget
with no double-click meaning, sees both presses, so clicking `+` twice
quickly counts twice, live and in the test harnesses.

Only the left button folds. On a row, a right-click asks for a context menu
(`on_context_menu`), and a middle or back/forward click reports its button
(`on_click`), so two quick right-clicks are two context-menu requests.

## Multi-select

A `Table` with `selection_mode="Multi"` (Node: `selectionMode: "Multi"`)
grows a selection with Ctrl/Cmd-click (toggle), Shift-click (range),
Shift+Up/Down, Ctrl/Cmd+A (all) and Ctrl+Space or Insert (toggle the cursor
row), and reports the whole set to `on_selection`. Pass it back as
`selected_indices` to paint it. A `List` always takes these gestures, and a
`Tree` takes the clicks with `selection_mode="Multi"`. In a terminal, Cmd
never reaches the app and most terminals keep Shift-click for their own
text selection, so the keyboard chords are the dependable path there.

## If you know...

- **React:** `onClick` and friends, but the handler mutates your state
  directly; there is no `setState`. The next render picks the change up.
- **Elm / iced:** Rust's `UiEvent` is your `Msg`, already defined: match it
  in `handle` instead of declaring your own message enum.
- **Textual:** like `on_button_pressed`, but attached to the node rather
  than found by a method name.
