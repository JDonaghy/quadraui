# quadraui design: goals, non-goals and invariants

What quadraui is for, what it deliberately is not, and the architectural
invariants every change has to preserve. Individual decisions, with dates
and the reasoning behind each, are in
[`decisions/DECISIONS.md`](decisions/DECISIONS.md); the rules a single
widget follows are in [`PRIMITIVE_RULES.md`](PRIMITIVE_RULES.md).

---

## 1. Vision

A Rust UI toolkit for building rich, fast, keyboard-driven applications
that run **both in a terminal and as a native desktop window** from one
codebase.

Backends, each behind a Cargo feature:

- **TUI** (`tui`): ratatui + crossterm.
- **Linux** (`gtk`): GTK4 + Cairo + Pango.
- **macOS** (`macos`): Core Graphics + Core Text.
- **Windows** (`win`): Direct2D + DirectWrite.

**Ambition:** someone can write a SQL client, a Kubernetes dashboard, a
log viewer or a package-manager UI once and get all four backends. Apps
should **feel native** (platform fonts, accent colour, menus, keybinding
conventions) without **being native**: there is no `gtk::TreeView`, no
`NSOutlineView`, no Win32 common control. The app writes once; quadraui
draws everywhere.

**Stress test:** [vimcode](https://github.com/JDonaghy/vimcode), a
Vim-compatible editor with dozens of panels, complex text editing,
drag-to-split panes, trees, forms, modals, a command palette and dialogs.

---

## 2. Non-goals

- **Not a general-purpose GUI framework** like GTK, Qt, Iced, Slint or
  egui. quadraui is biased toward keyboard-driven productivity apps: IDEs,
  database tools, devops consoles. Apps built around heavy custom drawing
  (games, video editors, Figma) are out of scope.
- **Not a web-tech shim** like Tauri or Electron. No WebView, no
  HTML/CSS, no JavaScript runtime.
  **This rejects Electron's *implementation*, not its *capability bar*.**
  See the root [`GOAL.md`](../../GOAL.md). What an app can do from
  Electron's main process (dialogs, clipboard, notifications, menus, tray,
  custom title bar, drag-and-drop) is a fair benchmark for quadraui;
  "Electron exposes this and we don't" is on its own a valid basis for an
  issue, provided the capability isn't web-specific. It is the browser
  runtime and the web surfaces (`webContents`, `session`, cookies) we
  don't want.
- **Not a pixel-perfect platform-widget mimic.** Widgets look
  *consistent* with each platform's conventions (font, accent, spacing)
  but do not try to reproduce `NSButton` exactly.
- **Not a retained-mode DOM.** No virtual DOM and no diffing: the app
  describes its UI again every frame (§3.1).
- **Not a general animation framework.** Apps cannot tween arbitrary
  properties. Chrome transitions are in scope on GUI backends (hover and
  press fades, toast slide-in, smooth scrolling, caret blink), and the
  TUI snaps to the end state. See `GOAL.md` § The visual target.

---

## 3. Architecture

### 3.1 Rendering model: describe the UI every frame

Each frame:

1. The app builds plain-data **widget descriptors** from its own state
   (`StatusBar { … }`, `TabBar { … }`) and pushes them into a
   `ScreenLayout` as `Surface::Widget` entries.
2. quadraui lays each widget out and paints it on the active backend.
3. Input comes back as events. The runtime routes pointer and keyboard
   input to the widgets the app listed in its `FrameTree`; each widget
   turns what it handled into a semantic `UiEvent` for the app.
4. The app updates its state, and the next frame's descriptors show the
   result.

**Who owns which state.** Visible content and selection (which tab is
active, which rows exist, what text a field holds, whether a dropdown is
open) are descriptor fields the app owns and sets every frame. A widget
keeps only what it needs between events (hover, a pressed segment, an
in-progress drag, a keyboard cursor inside the widget) in its
`Interactive::State`, which the runtime stores per `WidgetId` in
`WidgetStateStore` so it survives the descriptor being rebuilt. A widget
whose id stops appearing loses that state.

**Why not retained mode with diffing (React, Relm)?** A simpler mental
model, no stale-widget or identity-mismatch bugs, and far less work per
backend. vimcode shows that rebuilding the description every frame
scales to dozens of panels.

### 3.2 The widget contract

A widget is **one Rust type with one implementation**, used unchanged on
every backend:

- **`Primitive`** ([`src/primitive.rs`](../src/primitive.rs)), the paint
  half. `layout(ctx, rect)` resolves every coordinate once into
  `Self::Layout`; `paint(layout, surface, env)` draws onto a pixel
  `PaintSurface`, and `paint_cells(layout, cells, env)` draws onto the
  TUI's `CellSurface`. `PaintEnv` carries the theme, style, row height,
  hover and press, and the widget's own persistent state. Optional hooks
  cover intrinsic size (`measure`), paint outside the rect (`paint_bleed`),
  the zone a structural test sees (`paint_zone`) and platform presentation
  (`native_presentation`, which installs a `MenuBar` as the macOS global
  menu instead of painting it).
- **`Interactive`** ([`src/interactive.rs`](../src/interactive.rs)), the
  behaviour half. `focus` declares the widget's focus stops,
  `sub_regions` names its addressable parts, `handle` reacts to a
  `RoutedEvent` against the same `Layout` paint used, `State` is what it
  remembers between events, and `popups` (with `OPENS_POPUPS`) declares
  dropdowns, calendars and submenus that paint and hit-test above the
  frame. A widget with no behaviour uses `non_interactive!`.

`DynWidget` is the type-erased view of any `Interactive` widget, provided
by a blanket impl. `Surface::Widget` and `DrawsPrimitives::draw_dyn`
paint it; `FrameTree` routes events to it. Because paint and hit-testing
read the same resolved `Layout`, a click cannot drift from what is on
screen.

A widget defined outside this crate implements the same two traits and
needs no change anywhere else.

### 3.3 Events

Input reaches a widget as a `RoutedEvent`: a pointer gesture in
widget-local coordinates with the `SubTarget` under it, a key, committed
text, an accessibility action, focus in or out, or an event on one of its
popups. A widget reports outcomes through its `Action` type, whose
`WidgetAction::into_ui_event` turns it into a `UiEvent` carrying the
widget's `WidgetId` (for example
`UiEvent::TabBar(id, TabBarEvent::TabActivated { index })`). The app
routes on that id.

**Compose controllers** (`quadraui::compose`) coordinate several widgets
or an app-level flow (an application shell, a menu system, a form
dialog) and also report semantic events. A single widget's own input
handling belongs in its `Interactive` impl, not in a controller.

### 3.4 Backends: platform services and rasterisers

A backend implements the `Backend` trait: frame lifecycle, measurement,
input translation, and platform services (clipboard, file and message
dialogs, notifications, native menus, tray, window control, secret
storage, OS file drop). It also supplies a drawing surface: a pixel
backend implements `PaintSurface` (fill, stroke, text runs and lines,
images, clips, shadows), and the TUI backend supplies a `CellSurface`
(glyphs, styles, borders, cursor, inline graphics).

**A widget never needs a `Backend` method.** Backends hold no
per-widget code, so adding a widget touches no backend and adding a
backend touches no widget.

<!-- TODO(0.12 final pass): after W9 (#2980, #2981, #2982), confirm no
widget-specific Backend draw_*/…_layout method survives
(backend_method_ratchet at its final count) and that the list of
service groups above matches the trait. -->

`BackendCaps` reports what the running backend can do. Where a terminal
cannot do something (a tray icon, a native dialog) the backend degrades
and says so rather than silently doing nothing.

### 3.5 Fully drawn, not native widgets

Every widget is drawn through the backend's 2D or cell API.

**Benefits:**

- The same behaviour on every platform, from one code path for selection,
  scrolling and keyboard navigation.
- Testable: paint and hit-testing are platform-agnostic, so the same
  scenario runs headlessly against every backend.

**Costs, and where they stand:**

- **Accessibility.** There is no free accessibility tree. Each widget
  reports accessibility information through `A11yPrimitive::a11y_info`
  (a supertrait of `Primitive`) and its supported actions through
  `Interactive::a11y_actions`, and the `a11y` feature converts these to
  AccessKit types. Accessibility comes from those trait methods, not
  from annotation fields on descriptors: no primitive has any such field
  (`a11y_role`, `a11y_label`). No backend yet publishes the tree to UI
  Automation, NSAccessibility or AT-SPI (quadraui#1309).
- **IME and composition.** CJK input needs platform IME integration in
  each backend. Not implemented; the design is
  [`IME_INPUT_PROPOSAL.md`](IME_INPUT_PROPOSAL.md), the backend work
  quadraui#900.
- **Right-to-left text.** East-Asian width is handled; RTL, bidi and
  complex shaping are out of scope.
- **Platform context menus, kinetic scrolling, drag-and-drop between
  apps.** quadraui reimplements these per backend, or uses the native
  service where one exists (`ContextMenuController` shows a native menu
  where the platform has one).

These costs are acceptable for keyboard-driven productivity tools. They
would be dealbreakers for a mass-market app framework.

### 3.6 Native feel without native widgets

- **Colours** come from `Theme`, never hardcoded hex values in a widget.
  `Theme::from_vscode_json` loads a VS Code theme file.
- **Sizes** come from `Style` tokens (row height, activity bar width,
  radii), so a backend or app can retune them in one place.
- **Accelerators** are declared once: `KeyBinding::Save` renders as `⌘S`
  on macOS and `Ctrl+S` elsewhere ([`src/accelerator.rs`](../src/accelerator.rs)).
- **Menus:** `MenuBar` becomes the macOS global menu through
  `native_presentation`, and paints in-window on the other backends.

---

## 4. Widget coverage against target apps

The widget set is checked against three kinds of app: each needs to be
expressible from shipped widgets alone. The full list of shipped widgets
is the root `README.md`'s "What's in the box".

| App | Widgets it needs |
|---|---|
| **vimcode** (editor) | `ShellView` (activity bar, sidebar, panels), `Editor` in `TabBar`/`SplitTree` panes, `TreeView` for explorer and source control, `Form` for settings, `Palette`, `Completions`, `Tooltip`, `FindReplace`, `Dialog`, `Terminal`, `StatusBar` |
| **SQL client** | `TreeView` of connections and schemas, `Editor` for queries over a `DataTable` of results in a `Split`, `Dialog` with a `Form` for connection settings, `ListView` for history, `ProgressBar` and `Toast` for long-running queries |
| **Kubernetes dashboard** | `TreeView` of clusters and namespaces, `DataTable` or `TreeTable` resource lists, `TextDisplay` for YAML and logs, `Terminal` for a shell, `Dialog` for confirmations, `Palette` for switching context |

Two pressures from this exercise shaped the design: a data grid with
sortable, resizable columns (`DataTable`, and `TreeTable` for
hierarchical rows), and content that streams in while on screen (logs,
command output), which the rebuild-every-frame model handles by having
the app own the buffer and describe only the visible window.

---

## 5. Invariants for plugin- and binding-driven UI

These hold for every widget and every public API. Each one is what makes
scripted UI (vimcode's Lua extensions) and the Python and Node.js
bindings possible; breaking one would break them.

1. **`WidgetId` is allocatable at runtime.** `WidgetId::new` takes any
   string, so a plugin or binding can mint ids; it serialises as its
   string label.
2. **Events are plain data.** A widget emits a `UiEvent` carrying its
   `WidgetId`; the app routes on the id. No closure crosses the API.
3. **Descriptors are serialisable.** Every widget struct under
   `src/primitives/` implements `Serialize` and `Deserialize`, so a descriptor can be built from JSON, a Lua table
   or a Python dict.
4. **Ids can be namespaced.** An app can reserve a prefix per plugin
   (`"plugin:my-ext:send"`) so plugin ids never collide with its own.
5. **No global handlers.** Every event names the widget it came from.
6. **Descriptors own their data.** A widget struct does not borrow from
   app state, so a plugin or binding can hand over owned data.
7. **One implementation per widget.** A widget paints and behaves through
   its `Primitive` and `Interactive` impls on every backend; no backend
   has widget-specific code (§3.4).
8. **Paint and hit-test share one layout.** `handle` and `sub_regions`
   read the same `Layout` that `paint` and `paint_cells` draw from.

<!-- TODO(0.12 final pass): after W8 (binding-core collapse, #2976-#2979),
check invariants 3 and 6 against the final py/node wire surface. -->

---

## 6. Decisions still in force

The full log is [`decisions/DECISIONS.md`](decisions/DECISIONS.md). The
ones that shape everything above:

- **Describe every frame, no diffing** (§3.1).
- **One widget, one implementation**, through `Primitive` + `Interactive`
  (§3.2); backends are platform services and rasterisers (§3.4).
- **The editor consumes a description.** `Editor` paints a viewport the
  app describes each frame. The text engine (rope, syntax highlighting,
  LSP) stays in the app; it is not part of quadraui.
- **Native menu bars:** global on macOS, in-window elsewhere (§3.6).
- **One crate, backends behind features:** `tui`, `gtk`, `macos`, `win`,
  plus optional `terminal`, `layout`, `a11y` and `control`.
- **Rust 2021.** `rust-version` in `quadraui/Cargo.toml` is the real
  dependency floor, not a fixed offset from the latest stable; see that
  field's own comment and D-019 rule 7 in `DECISIONS.md`.
- **License:** MIT OR Apache-2.0.

**Not yet built:** multiple windows per process (quadraui#1120), IME
(§3.5) and platform accessibility wiring (§3.5).
