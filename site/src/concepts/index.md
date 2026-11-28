# The shape of an app

Every quadraui app, in every language, has the same three parts. This is
MVU, the Elm architecture, which React, iced, Textual, SwiftUI and Flutter
also use.

| Part | What it is | quadraui |
|---|---|---|
| **State** | Your data. Plain fields on a plain object. | a struct / object you own |
| **View** | A function from state to a widget tree. Runs every frame. | `view()` returning nodes |
| **Update** | Code that changes state when an event arrives. | callbacks on nodes (Python, Node), or messages applied by one `update` (Rust) |

There is no data binding, no observable, no "mark dirty" call. After an
event is handled, the view runs again and quadraui repaints.

## If you know...

| Framework | State | View | Update |
|---|---|---|---|
| **Elm** | `Model` | `view : Model -> Html Msg` | `update : Msg -> Model -> Model`. Rust's `View::update` is this: nodes map events to your `Msg`, and one match applies it. |
| **iced** | your `struct` | `fn view(&self) -> Element<Message>` | `fn update(&mut self, Message)`. The same as Rust's `View`: `button("+").on_click(Msg::Inc)`, `column([...])`, `row([...])`. |
| **React** | `useState` / fields | a component returning JSX | `onClick` props. The Node layer is this, with JSX and `Component` for local state, but the whole view re-renders every event; there is no diffing for you to manage. |
| **Textual** | widget attributes | `compose()` | `on_button_pressed` handlers. The Python layer puts the handler on the node instead: `Button("+", on_click=self.inc)`. Layout comes from `Column`/`Row`, not CSS. |

## What runs where

One Rust core does layout, painting, hit-testing, focus and popups. A
**backend** turns that into pixels or terminal cells: `tui` (a terminal),
`gtk` (Linux), `macos` (Core Graphics) or `win` (Direct2D). The Python and
Node packages are thin layers over the same core, so a widget behaves the
same in all three languages.

The pages that follow take one idea each:

- [Widgets](widgets.md): what a widget is, and how to write one.
- [Events](events.md): how input reaches your code.
- [Layout](layout.md): how widgets get their rectangles.
- [State](state.md): what you own and what a widget keeps for itself.
- [Focus](focus.md): keyboard focus and Tab order.
- [Popups](popups.md): dropdowns, menus and dialogs.
- [Theming](theming.md): colours and styles.
