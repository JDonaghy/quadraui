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
a key event into a name like `"q"`, `"Enter"` or `"Down"`.

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

| Widget | Python / Node callback | Rust `UiEvent` |
|---|---|---|
| Button | `on_click` / `onClick` | `ButtonActivated { widget }` |
| List | `on_select`, `on_activate` | `List(id, ...)` |
| Tree | `on_select`, `on_activate`, `on_row_click`, `on_toggle_expand` | `Tree(id, ...)` |
| Table | `on_select`, `on_activate`, `on_header_click` | `DataTable(id, ...)` |
| Tabs | `on_activate`, `on_close` | `TabBar(id, ...)` |
| Status bar | `on_segment_click` | `StatusBar(id, ...)` |
| Form fields | `on_change`, `on_submit` | `Form(id, ...)` |
| Any key | `on_event` + `key_name` | `KeyPressed { key, modifiers, .. }` |

The full list of events and their payloads is generated from one Rust
schema into [the bindings' wire types](../bindings.md#reference).

## If you know...

- **React:** `onClick` and friends, but the handler mutates your state
  directly; there is no `setState`. The next render picks the change up.
- **Elm / iced:** Rust's `UiEvent` is your `Msg`, already defined: match it
  in `handle` instead of declaring your own message enum.
- **Textual:** like `on_button_pressed`, but attached to the node rather
  than found by a method name.
