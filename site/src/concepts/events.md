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
            Row(Button("-", id="dec", on_click=self.dec), Button("+", id="inc", on_click=self.inc)),
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

## Rust: one `handle`, matched by id

Rust apps get every event in one method and match on it, as in Elm's
`update`:

<!-- sample: examples/hero/rust/src/main.rs -->
```rust,noplayground
    fn handle(&mut self, event: UiEvent, _: &mut dyn Backend) -> Reaction {
        match event {
            UiEvent::ButtonActivated { widget } if widget.as_str() == "inc" => self.count += 1,
            UiEvent::ButtonActivated { widget } if widget.as_str() == "dec" => self.count -= 1,
```

`handle` returns a `Reaction`: `Redraw` after a change, `Continue` when
nothing changed, `Exit` to quit.

## Common events

What each Python / Node callback receives, and the Rust event it answers:

| Widget | Python / Node callback | Rust `UiEvent` |
|---|---|---|
| Button | `on_click()` / `onClick()` | `ButtonActivated { widget }` |
| List | `on_select(index)`, `on_activate(index)` | `List(id, ListViewEvent::SelectionChanged { idx })`, `ItemActivated { idx }` |
| Tree | `on_select(path)`, `on_activate(path)`, `on_row_click(path)`, `on_toggle_expand(path)`; a path is a list of indices | `Tree(id, TreeEvent::SelectionChanged { path })`, `RowActivated`, `RowClicked`, `RowToggleExpand` |
| Table | `on_select(row)`, `on_activate(row)`, `on_header_click(column)` | `DataTable(id, DataTableEvent::RowSelected { idx })`, `RowActivated`, `HeaderClicked { col }` |
| Tabs | `on_activate(index)`, `on_close(index)` | `TabBar(id, TabBarEvent::TabActivated { index })`, `TabClosed { index }` |
| Status bar | `on_segment_click(segment_id)` | `StatusBar(id, StatusBarEvent::SegmentClicked { id })` |
| Text field (in a `Form`) | `on_change(text)`, `on_submit(text)` | `Form(id, FormEvent::TextInputChanged { value, .. })`, `TextInputCommitted { .. }` |
| Any key | `on_event(event)` + `key_name` | `KeyPressed { key, modifiers, .. }` |

`on_change` gets the field's whole new value, not the key that changed it.
The full list of events and their payloads is generated from one Rust
schema into [the bindings' wire types](../bindings.md#reference); for Rust,
see `UiEvent` in the [API reference](https://docs.rs/quadraui).

## Two quick clicks are one double-click

Two clicks less than 400 ms and 1.5 cells apart fold into a double-click.
A button does not treat the second half of a double-click as another
press, so clicking `+` twice quickly counts once, in a live app and in the
test harnesses. In tests, click a different widget in between, press Enter
on the (now focused) button, or in Rust call
`TuiDriver::set_double_click_folding(false)`. Making a button activate on
every press is tracked as a gap in
[`examples/hero/README.md`](https://github.com/JDonaghy/quadraui/blob/develop/examples/hero/README.md#gaps-this-exposed).

## If you know...

- **React:** `onClick` and friends, but the handler mutates your state
  directly; there is no `setState`. The next render picks the change up.
- **Elm / iced:** Rust's `UiEvent` is your `Msg`, already defined: match it
  in `handle` instead of declaring your own message enum.
- **Textual:** like `on_button_pressed`, but attached to the node rather
  than found by a method name.
