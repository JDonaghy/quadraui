# Changelog

All notable changes to the `quadraui` Python package (`quadraui-py`) are
documented in this file. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

This file starts now, not retroactively — `quadraui-py` has history
predating it (see `git log -- quadraui-py`); none of it is backfilled here.
`[Unreleased]` is where changelog discipline begins for this package.

## [Unreleased]

### Added

- `QuiAppActions.show_overlay(kind, descriptor, anchor=None)`/
  `.dismiss_overlay(id)` -- the same nine `kind`s and anchor semantics as
  `QuiApp.show_overlay`/`QuiApp.dismiss_overlay`, callable from inside a
  registered handler (where the `QuiApp` methods themselves raise
  `ReentrantCallError`). `App.confirm`/`App.alert` now queue through
  `show_overlay("message_dialog", ...)` instead of `show_message_dialog`.

- A `"cell_grid"` leaf's navigation/editing/virtualisation outcomes reach
  the registered handler as an ordinary `UiEvent` variant,
  `{"CellGrid": (id, event)}`, rather than the earlier
  `{"CellGridController": (id, event)}` shape: `CellGrid` carries no
  state of its own across frames, so the handler must apply
  `Navigated`/`CellEditStarted`/`CellEditChanged`/`CellEditCommitted`/
  `CellEditCancelled` back onto its own render state before the next
  frame. `CellsRequested` (the two-axis-virtualisation window request)
  still needs no new host call to answer -- re-render with that
  `"cell_grid"` leaf's `cells`/`loaded_row_start`/`loaded_col_start`
  covering the named block, same as any other leaf's per-frame data.
- A `"grid"` layout node: `{"grid": {"columns": [...], "rows": [...],
  "cells": [...]}}` lays its cells out row-major across two independent
  track lists, each track one of `{"Fixed": px}`/`{"Percent": 0.0-1.0}`/
  `{"Weight": fr}`/`{"Content": px}` (`quadraui.wire.GridTrack`), resolved
  through the same `Fixed`/`Content` -> `Percent` -> `Weight` pass order a
  row/column child's `"size"` key already uses. Takes the usual
  `"padding"`/`"margin"` siblings (no `"gap"`: a grid's tracks carry no
  gutter). More `"cells"` than `columns x rows` has room for is a decode
  error rather than a silently dropped tile. `wire.pyi` carries the
  generated `GridTrack`/`LayoutGridPayload`/`LayoutGridNode` types.
- `quadraui.app.widget.Widget`: a base class for a script-authored node
  with its own `measure`/`paint`/`on_event`/`a11y` -- for a widget that
  can't be built by composing existing nodes. Builds on the `Canvas`
  `DrawOp`/`CanvasRegion` display-list vocabulary (`paint` returns the
  same `{"ops": [...], "regions": [...]}` shape `quadraui.wire.CanvasDescriptor`
  documents) and routes through `quadraui_binding_core::ScriptedPrimitive`.
  See `quadraui-py/examples/rating.py` (a 5-star rating control) for a
  worked example.
- A `"toolbar"` extra's dict may now set `"overflow"` (`"Clip"`/`"Wrap"`/
  `"Menu"`) and `"overflow_priorities"` (a list of `{"id", "priority"}`
  entries) to opt into `ToolbarOverflow`'s wrap/menu-with-drop-priority
  layouts instead of today's Clip-only behaviour. Both keys are optional;
  a `"toolbar"` dict with neither renders exactly as before.
- A click on a `"tree_table"` extra's branch-row chevron now resolves to a
  typed `{"TreeTable": (id, {"ToggleExpanded": {"path", "expand"}})}`
  event instead of only a raw `MouseDown`; a click elsewhere in the row
  body still falls back to the raw `MouseDown`.
- A `"style"` dict (inline on a node, or passed to `define_style`) now
  accepts `"font_family"`/`"font_size"`/`"font_weight"` (a string, a
  number, and `"normal"`/`"bold"` respectively) and `"hover"`/`"pressed"`/
  `"focus"` -- each a nested `StyleValue` applied over the override's own
  base tokens while the node is in that interaction state. A state branch
  may itself carry colour/metric/font tokens but may not nest a further
  `"hover"`/`"pressed"`/`"focus"` branch. `wire.pyi`'s `StyleValue`
  carries the six new keys.

### Changed

- A tab bar (the `"tab_bar"` leaf and slot, and a `"tabs"` node's strip)
  is a generic widget routed through the core `TabBar`'s own input
  handling (#2977), with these visible changes:
  - A press on a tab bar carries the `"widget_body"` hit detail; the
    `"tab_bar_tab"`, `"tab_bar_tab_close"`, `"tab_bar_scroll_left"`,
    `"tab_bar_scroll_right"` and `"tab_bar_right_segment"` hit details are
    gone.
  - Keyboard focus walks: each Left/Right moves on from the tab the last
    press reached (before, a bare tab bar started from tab 0 on every
    press), and Enter/Space activate it. While the bar has not moved focus
    yet, a `"tabs"` node's `focused_index` is the starting tab.
  - A key the bar has no use for reaches the handler as the bar's
    `TabBar` `KeyPressed` event and then as the raw `KeyPressed` (before,
    only the former).
  - A double-click on a tab reaches the handler as the raw `DoubleClick`
    (before, a second `TabActivated`).

- The four text-entry leaf kinds, `"text_input"`, `"text_field"`,
  `"password"` and `"textarea"`, are the core widgets painted and edited
  through their own state (#2977/#2978), with these visible changes:
  - `"text_input"` is the multi-line core `TextInput`: Enter inserts a line
    break (a `TextInputChanged` with the `\n`) instead of reporting
    `{"TextInputCommitted": ...}`. Use `"text_field"` or `"password"` for a field that
    commits on Enter.
  - A key that only moves the caret or the selection (arrows, Home/End,
    Shift-selection, Ctrl/Cmd+A) is claimed by a focused `"text_input"`,
    and no longer reaches the handler as a raw `KeyPressed`.
  - `"text_field"` now edits: typing, paste, click-to-caret, the clear
    button and Enter (a `{"TextInputCommitted": ...}`) all work, where it used to paint only.
  - `"textarea"` with no `"cursor"` edits from the end of its value
    (it used to start at column 0), and a click on a `"textarea"` or
    `"password"` leaf focuses it without moving the caret. Both now answer
    Ctrl/Cmd+A (select all) and Ctrl/Cmd+C/X (copy and cut, as
    `TextCopied`; a password swallows them), which used to pass through.
  - `"password"` accepts a `"placeholder"`, and a `"textarea"`'s
    `"placeholder"` is a string, no longer nullable; `wire.pyi` follows.
  - A `"text_input"` leaf's natural height counts its border rows, so
    under a non-`stretch` `align` it sizes to the box it paints.

- A `"menu_bar"` leaf is the whole application menu (core `AppMenu`) and
  is controlled: a click or key that opens a menu or moves a
  highlight reports `{"AppMenu": (id, {"Changed": view})}`, and the app
  merges `view` (`open_item`, `focused_item`, `dropdown_selected`,
  `submenu_path`, `submenu_selected`) back into the `"menu_bar"` dict it
  renders next; a menu with nothing echoed never opens. An activation still
  reports `{"MenuActivated": item_id}`, now after the `Changed` view that
  closes the menu and with the `"hit": {"kind": "widget_body"}` every
  generic leaf's event carries. The dropdown is a popup routed to the
  menu: a press on a row activates it, and a press elsewhere or Escape
  closes the menu (as a `Changed` view). The bar is a focus stop, and its
  Alt mnemonics and arrow keys reach it while it holds focus, not
  globally. While a dialog is open the dropdown is hidden and takes no
  input; it is no longer closed by opening the dialog. `wire.pyi`'s
  `MenuBar`/`MenuBarItem` types are now `AppMenu`/`MenuDef`.
- A `"form"` leaf routes through core `Form`'s own widget contract: its
  events carry `"hit": {"kind": "widget_body"}` like every generic leaf's.
  Field focus the dict leaves unset is still the form's own to track; Tab
  inside a focused form walks its fields and moves on to the next stop
  past either end, Up/Down move between fields the focused one has no use
  for, and a wheel notch scrolls it. A form's slider field (`SliderField`)
  moves on a click or drag on its track and on the arrow keys and Home/End
  while focused, firing `on_change`, instead of only through an a11y
  Increment/Decrement. Any open widget popup (a `"combo_box"` list) now
  receives the press that lands on it, instead of the press being dropped.
- **Breaking:** a `"tree_table_controller"` leaf's `"nodes"` are content
  read every frame (#2978, 0.12 rec 14), no longer a seed read once on
  the leaf's first frame. The widget keeps only what the user does to the
  tree -- expanded branches, the selection, the scroll position and the
  header sort -- keyed by node id. So:
  - lazily loaded children reach the widget as that node's `"children"`
    in the next `render`, not through a method call;
  - `"selected_id"` selects that node whenever the app changes it
    (between changes the user's own selection stands);
  - the new `"sort_override": True` says `"nodes"` already arrive in the
    app's own order for the current sort, so a header click reports
    `SortChanged` and moves the sort indicator but does not re-order the
    rows;
  - `"has_focus"` paints as sent; keyboard focus no longer forces the
    focused look on.

  Events keep their `{"TreeTableController": (id, event)}` shape and
  variants, and now reach the handler through the ordinary `UiEvent` path,
  so `push_event` accepts that shape too. See *Removed* for the migration.
- A generic widget leaf lays out and paints with the `Interactive` state
  its own events left (#2978): binding-core keeps that state in the same
  per-thread store the Rust runtime paints from. A widget whose paint
  reads its state (a caret, a text selection, an open section) now shows
  it in a `quadraui-py` app as it does in a Rust one.
- A `quadraui.app.widget.Widget` is now an ordinary generic widget leaf
  (binding-core's `ScriptedPrimitive` implements the widget contract): a
  pointer that enters or leaves it reports `MouseEntered`/`MouseLeft` with
  hit detail `{"kind": "widget_body"}` rather than
  `{"kind": "canvas_region", "region": ..}`; the region's own
  `{"Canvas": (id, {"RegionHovered": ..})}` events are unchanged. The
  leaf payload's generated type is no longer named
  `ScriptedWidgetDescriptor` in `wire.pyi`.

- A `"completions"` overlay (`show_overlay("completions", ...)`) now routes
  pointer input through `quadraui.Completions`' own event handling: a click
  on a row reports `{"List": (id, {"SelectionChanged": {"idx": n}})}` and
  moves the highlight, and only a double-click reports
  `{"CompletionItemActivated": (id, n)}` and closes the popup (a single
  click used to activate). A press on the popup no longer moves focus to
  it, so the editor underneath keeps receiving keys.
- A wheel notch over an overflowing `"rich_text_popup"` overlay scrolls the
  popup and reports `{"ScrollOffsetChanged": {"widget": id, "new_offset":
  n}}` instead of reaching whatever is painted behind it.
- `quadraui.wire` (#2979) is generated from a JSON Schema derived from the
  Rust wire types themselves, so every descriptor `TypedDict` is now named
  after its Rust type: the `Descriptor` suffix is gone (`ButtonDescriptor`
  -> `Button`, `TreeViewDescriptor` -> `TreeView`, `ListViewDescriptor` ->
  `ListView`, `ColumnDescriptor` -> `Column`, `ListItemDescriptor` ->
  `ListItem`, `TreeRowDescriptor` -> `TreeRow`, `CanvasDescriptor` ->
  `Canvas`, `PasswordDescriptor` -> `PasswordInput`, `FloatDescriptor` ->
  `Float`, ...). `SplitTreeRecursiveNode` is `SplitTreeNode`,
  `EditorDiffLine` is `DiffLine`, `MsvAxis` is `Axis` and
  `DialogChromeDescriptor` is `DialogChromeWire`. `TreePath` is gone (a
  path is a plain `List[int]`), as are `EditorStyle`, `IconGlyph` and
  `IconCodicon` (`Icon` is still the union of both shapes). Optionality
  now matches what the decoder accepts: an `Option<T>` field may be
  omitted (`NotRequired[Optional[T]]`) rather than being a required
  nullable key, and field types are the real Rust types (an integer field
  is `int`, an enum field its literal set). A `list`/`tree`/`data_table`
  leaf body is `ListView | ListViewLeaf` (the leaf variant adds
  `scroll_px`/`items_window`), and `WidgetKind` names every leaf kind.
  Docstrings are the Rust types' own first paragraph. New definitions
  describe the leaf `a11y`/`draggable`/`drop_target` keys, every
  `run_shell` config shape and the drag/tree-table/hit wire events.

- `progress`, `spinner`, `scrollbar`, `command_line`, `message_list`,
  `multi_section_view` and `find_replace` leaves decode through the
  generic widget leaf (`quadraui::decode_widget`), like `button` or
  `switch`. A click on one now carries `"widget": <id>` and
  `"hit": {"kind": "widget_body"}` (previously `progress`/`spinner`/
  `scrollbar`/`message_list`/`multi_section_view`/`command_line` attached
  no hit and `find_replace` attached `{"kind": "extra_body"}`). A click on
  `find_replace` is resolved by the panel itself: after the
  `FocusChanged`, the handler receives `{"FindReplace": (id, event)}`
  (e.g. `FieldEdited` placing the caret in the clicked field) instead of
  the raw `MouseDown`. `message_list` drag-select and Ctrl-C copy are
  unchanged.
- `number_input`, `combo_box` and `date_picker` leaves decode through the
  generic widget leaf too, so each widget's own `Interactive::handle`
  produces its events. `number_input` reports a step, an arrow key or a
  commit as `NumberChanged`/`NumberDraftChanged`/`NumberCommitted` with a numeric value, e.g.
  `{"Form": (id, {"NumberChanged": {"id": id, "value": 43.0}})}` (previously re-shaped into a string-valued
  `{"Form": (id, {"TextInputChanged": {"id": id, "value": "43"}})}`). `date_picker` reports
  a click or arrow-key scrub as `{"Form": (id, {"DateFocusMoved": ...})}` (previously
  dropped). `combo_box` reports the first filtered option as
  `ComboBoxHighlightChanged` before each typed `ComboBoxTextChanged`, and
  `Enter` on a closed box opens the list rather than committing.
- `toolbar` decodes through the generic widget leaf (`quadraui::Toolbar`,
  which carries `overflow`/`overflow_priorities` itself). A click on a
  button now parks the bar's keyboard cursor there, so the handler
  receives `{"FocusChanged": "<bar id>"}` before the `Toolbar` event. The bar's
  keyboard cursor is the widget's own state: after Tab focuses the bar,
  the arrow keys move it (the first press parks it on the first button)
  and Enter/Space activate the button under it; the descriptor's
  `focused_index` no longer chooses what Enter activates. The
  `"extra_chrome"` hit kind is removed.
- `tree_table` decodes through the generic widget leaf, so
  `quadraui::TreeTable`'s own `Interactive::handle` resolves every click:
  a click on a row body focuses the table and reports
  `{"TreeTable": (id, {"RowSelected": {"idx": i, "path": [...]}})}` instead of reaching the
  handler as a raw `MouseDown`. The `"tree_table_chevron"` hit kind is
  removed; a chevron click still reports `ToggleExpanded`.
- A `tree` row mid-rename edits through core's single-line text editor,
  so the rename honours the same keys every text field does: Ctrl/Cmd+A
  selects the whole name, Shift+arrows extend the selection and
  Ctrl/Alt+arrows move by word, each reported as `RowRenameChanged`.
- A tooltip opened with `show_tooltip` is sized by the tooltip's own
  measure, so its box matches the one every other host paints (on a cell
  grid, one column narrower than before).
- `show_overlay("layout_dialog", ...)`'s `"dialog"` chrome no longer
  requires a `"buttons"` key: a content-bearing dialog may rely on its
  own subtree (a form's Cancel/Save row, a close icon) instead of the
  chrome's button row. `DialogChromeDescriptor.buttons` is now
  `NotRequired[List[DialogButtonDescriptor]]` in `wire.pyi`.

### Added

- `App.confirm(title, body, buttons, severity=None, *, on_resolve=None)` /
  `App.alert(title, body, *, id="ok", label="OK", severity=None,
  on_resolve=None)`: wrap `show_overlay("message_dialog", ...)` +
  `MessageDialogResolved` into a single call that reports the clicked
  button's id back to the caller -- through a sync or `async def`
  `on_resolve` callback, or, with `on_resolve` omitted, an `asyncio.Future`
  an `async def` handler can `await` directly under `quadraui.aio`. Plus
  `quadraui.aio.schedule_callback(waker, func, *args)`, factored out of
  `async_handler`'s own task-scheduling so `on_resolve` can share it.
- `quadraui.app.Form(fields=, id=, ...)` plus six typed field builders --
  `ToggleField`, `TextInputField`, `SliderField`, `PasswordField`,
  `TextAreaField`, `SelectField` -- each with its own `on_change` (and
  `on_submit` on `TextInputField`/`PasswordField`), routed by the field's
  own id rather than the wrapping `Form`'s. Plus `Checkbox`, `Slider`,
  `Select`, `Password`, `TextArea`: the same five controls as standalone,
  `Form`-less leaves. No raw wire dict or `"Form"` branch in `on_event`
  needed to build a settings-page-style field stack.
- `quadraui.app.List(items, selected=, title=, has_focus=, bordered=,
  on_select=, on_activate=, id=)`: a typed node for a `ListViewDescriptor`
  leaf, routing `SelectionChanged` to `on_select` and either
  `ItemDoubleClicked` or `ItemActivated` to `on_activate` -- no raw wire
  dict or `"List"` branch in `on_event` needed to build a selectable list.
- `quadraui.app.App`'s typed layer now stamps a `"wire_version"` sibling
  key onto every tree its `view()` renders (`quadraui.app.runtime.
  WIRE_VERSION`, mirroring `quadraui_binding_core::wire_schema::
  WIRE_VERSION`), unless `view()` already set one itself (the raw-dict
  escape hatch). The native extension's own version gate
  (`VersionMismatchError`) now applies to a typed-layer tree exactly the
  way it already did to a hand-built wire dict passed to `QuiApp.
  set_render` directly.
- `quadraui.app.App.run_async(fallback_interval=None)`: the app-layer
  `App` (`quadraui.app.App`) now has its own `run_async`, delegating to
  the wrapped `QuiApp.run_async`/`quadraui.aio.run` the same way
  `run_tui`/`run_native` already delegate — `await app.run_async()` on an
  app-layer `App`, as `quadraui/AGENTS.md`'s "Running it" section
  documents, previously raised `AttributeError`.
- Every `InvalidDescriptorError` subclass (`MissingFieldError`,
  `UnknownVariantError`, `TypeMismatchError`, `UnknownKeyError`,
  `VersionMismatchError`, `DecodeFailedError`) now carries a `path`
  instance attribute mirroring `DecodeError::path` verbatim (e.g.
  `"render › split#body › panel#chrome"`), alongside the existing `code`
  class attribute, so a caller can locate the offending node
  programmatically instead of parsing it back out of the exception
  message.

### Removed

- The `"extra_body"` hit detail (#2977). Every widget leaf is a generic
  widget now and reports `"widget_body"`; no leaf produced `"extra_body"`
  any more. The `_HitDetailExtraBody` type is gone from `quadraui.wire`.

- The `"canvas_region"` hit detail (#2978): a script-defined `Widget`
  reports `"widget_body"` like every other generic widget, and its region
  events arrive as `{"Canvas": (id, ..)}`.

- `QuiApp.list_char_width()` and `QuiApp.scrollbar_reserve()` (#2979,
  0.12 rec 14), part of the 0.12 binding API cleanup. Budget row text
  with `char_width()` or `measure_text()` instead.

- `QuiApp.set_tree_table_children(id, node_id, children)`,
  `QuiApp.set_tree_table_sort_override(id, nodes)` and their
  `QuiAppActions` twins (#2978, 0.12 rec 14). Lazily loaded children and
  a host sort order are `"tree_table_controller"` descriptor content now:
  keep them in the app's own model and send them from `render`. Before:

  ```python
  def on_event(event):
      if "TreeTableController" in event:
          table_id, payload = event["TreeTableController"]
          if "ChildrenRequested" in payload:
              node_id = payload["ChildrenRequested"]["id"]
              actions.set_tree_table_children(table_id, node_id, load(node_id))
          elif "SortChanged" in payload:
              col = payload["SortChanged"]["col"]
              asc = payload["SortChanged"]["ascending"]
              actions.set_tree_table_sort_override(table_id, my_sorted_nodes(col, asc))
  ```

  After -- the handler only records what the app now knows, and `render`
  builds the whole tree from it:

  ```python
  loaded = {}          # node id -> list of child nodes the app has loaded
  sort = None          # (col, ascending) when the app sorts rows itself

  def on_event(event):
      global sort
      if "TreeTableController" in event:
          _, payload = event["TreeTableController"]
          if "ChildrenRequested" in payload:
              node_id = payload["ChildrenRequested"]["id"]
              loaded[node_id] = load(node_id)
          elif "SortChanged" in payload:
              sort = (payload["SortChanged"]["col"], payload["SortChanged"]["ascending"])

  def node(n):
      out = {"id": n.id, "cells": n.cells}
      if n.is_branch:
          out["has_children"] = True
          if n.id in loaded:        # omit "children" until loaded: a lazy branch
              out["children"] = [node(c) for c in my_order(loaded[n.id], sort)]
      return out

  def render():
      return {"row": [{"id": "files", "tree_table_controller": {
          "id": "files", "columns": columns, "nodes": [node(r) for r in my_order(roots, sort)],
          "sort_override": sort is not None,   # drop to keep the built-in text sort
      }}]}
  ```

  An app that is happy with the built-in sort (cell text, branches first
  with `"folders_first"`) leaves `"sort_override"` out and ignores
  `SortChanged`. The handler can update the model directly, so neither
  answer needs `QuiApp.actions()` any more.

- `QuiApp.insert_button`/`insert_text_display`/`insert_list`/`insert_tree`/
  `insert_data_table`/`insert_tab_bar`/`insert_progress`/`insert_spinner`/
  `insert_image`/`insert_extra`/`clear_extra`/`slot_clear`, deprecated in
  0.11.0: the six fixed primitive slots and every additive primitive now
  reach the screen only through a `render` callback's descriptor dict.
  `SlotOccupiedError`/`UnknownSlotError` are removed with them.
- The 13 legacy per-kind overlay methods on `QuiApp`, and their 5
  duplicates on `QuiAppActions`, now that `show_overlay`/`dismiss_overlay`
  cover every kind each one forwarded onto:

  | Removed | `QuiApp` replacement | `QuiAppActions` replacement |
  | --- | --- | --- |
  | `show_context_menu(items, position)` | `show_overlay("context_menu", {"items": items}, (*position, 0.0, 0.0))` | same, via `actions.show_overlay` |
  | `show_message_dialog(title, body, buttons, severity=None)` | `show_overlay("message_dialog", {"title": title, "body": body, "buttons": buttons, "severity": severity})` | same, via `actions.show_overlay` |
  | `dismiss_message_dialog()` | `dismiss_overlay("message_dialog")` | n/a (`QuiAppActions` never had one) |
  | `show_toast(toast, corner=None)` | `show_overlay("toast", {**toast, "corner": corner})` | same, via `actions.show_overlay` |
  | `dismiss_toast(id)` | `dismiss_overlay(id)` | n/a (`QuiAppActions` never had one) |
  | `show_tooltip(tooltip, anchor)` | `show_overlay("tooltip", tooltip, anchor)` | same, via `actions.show_overlay` |
  | `hide_tooltip()` | `dismiss_overlay("tooltip")` | `dismiss_overlay("tooltip")` |
  | `show_completions(completions, cursor)` | `show_overlay("completions", completions, (*cursor, 0.0, 0.0))` | n/a (`QuiAppActions` never had one) |
  | `dismiss_completions()` | `dismiss_overlay("completions")` | n/a (`QuiAppActions` never had one) |
  | `show_float(float, width, height)` | `show_overlay("float", float, (width, height, 0.0, 0.0))` | n/a (`QuiAppActions` never had one) |
  | `dismiss_float()` | `dismiss_overlay("float")` | n/a (`QuiAppActions` never had one) |
  | `show_rich_text_popup(popup, anchor)` | `show_overlay("rich_text_popup", popup, (*anchor, 0.0, 0.0))` | n/a (`QuiAppActions` never had one) |
  | `dismiss_rich_text_popup()` | `dismiss_overlay("rich_text_popup")` | n/a (`QuiAppActions` never had one) |

### Fixed

- A focused `"text_input"` answers Ctrl+Left/Right (word jump),
  Ctrl+Backspace/Delete (word delete) and Ctrl+Home/End, which used to
  pass through unhandled, and Ctrl/Cmd+Z/Ctrl/Cmd+Shift+Z undo and redo in
  every text-entry leaf, not only `"text_input"`.
- The caret of a text-entry leaf, and of a `"command_line"` or
  `"palette"` query, paints where the widget's own editing put it, not
  where the descriptor's `cursor` says.
- A text-entry leaf inside a layout dialog or popover keeps editing:
  keys and typed text reach it through the same routing as the main tree.
- A double-click or a wheel notch on an open `"completions"` list or
  `"rich_text_popup"` that the overlay itself ignores no longer reaches the
  handler or the widget painted beneath it: an overlay swallows every
  press, double-click and wheel notch that lands on it (#2978). A press on
  a completions list outside its rows no longer dismisses it.

- A layout-tree render resolves stored cursor, scroll, split-ratio and
  active-tab state through binding-core's shared resolvers, the same ones
  `quadraui-node` uses: a `"tree"`/`"list"`/`"data_table"`/`"tab_bar"`
  leaf's state is keyed by the leaf's own `"id"` even when its descriptor
  carries a different `"id"` (a tree leaf in that shape used to snap its
  keyboard cursor back to the first row), and a leaf's first-frame scroll
  is sized against the rect it is about to paint into. State owned by a
  subtree that is not painted this frame -- an unselected `"tabs"` entry,
  a `"scroll"` child outside the viewport -- is kept, so switching tabs
  away and back restores a tree's cursor, a nested tab's active index and
  a split's ratio.

- `show_overlay` decodes through binding-core's `decode_overlay`, shared
  with `quadraui-node`; kinds, descriptor shapes, anchors, returned ids and
  `InvalidDescriptorError` for a malformed call are unchanged. The
  `"context_menu"` and `"message_dialog"` descriptors are now typed in
  `wire.pyi` (`ContextMenuOverlayDescriptor`,
  `MessageDialogOverlayDescriptor`).

- `QuiApp.show_overlay`/`dismiss_overlay`, called reentrantly from inside
  the registered event handler or render callback, now raise
  `ReentrantCallError` -- the same restricted-method contract every other
  `QuiApp` method with a `QuiAppActions` counterpart has -- instead of
  PyO3's raw `RuntimeError: Already borrowed`. The removed per-kind methods
  masked this: each ran the `ReentrantCallError` guard itself before
  forwarding to the shared overlay logic, so the gap was unreachable until
  `show_overlay`/`dismiss_overlay` became the only way in.

## [0.11.0] - 2026-11-14

### Added

- `QuiApp(..., strict=False, on_unknown_key=None)`: a render callback's
  returned descriptor carrying an unrecognised *optional* key (a misspelt
  `"cursr"`, `"focussed"`, ...) now reports it through
  `on_unknown_key(key, path)` — called at most once per distinct key for
  the app's whole lifetime — instead of silently dropping it.
  `strict=True` turns the same finding into an `UnknownKeyError` instead,
  raised from the render that found it. The default (`strict=False`, no
  `on_unknown_key`) keeps every existing app's behaviour unchanged.
- `QuiApp.run_shell(..., strict=False, on_unknown_key=None)`: the same
  `strict`/`on_unknown_key` policy, applied to `run_shell`'s own `config`
  argument. Since `config` decodes exactly once, synchronously, during
  `run_shell` itself, an `on_unknown_key` callback that raises surfaces as
  `run_shell`'s own raised exception rather than through a later `step`/
  `run` call.
- `ReentrantCallError`: raised instead of PyO3's raw `RuntimeError: Already
  borrowed` when a `QuiApp` method restricted to top-level script code
  (`show_message_dialog`, `set_title`, `apply_on_quit`, ... — every one
  with a matching `QuiAppActions` method) is called re-entrantly, from
  inside that same app's own registered event/tick/render callback. The
  message names the method and points at `actions()`.
- An asyncio-native runner: `quadraui.asyncio.run(app)` (equivalently,
  `await app.run_async()`) drives `QuiApp.step()`/render from a running
  asyncio event loop instead of a plain `while`/`for` loop, mirroring
  `quadraui-node`'s own `run()`. `QuiApp.arm_wake_notify(callback)`/
  `disarm_wake_notify()` are the new low-level hook it is built on: a
  no-argument callback fired every time any `QuiAppWaker` handle delivers
  a `post`/`wake`/`post_after`/`wake_after`, from whatever thread that
  happened on, so a background delivery wakes the asyncio loop the
  instant it fires rather than waiting on a fixed poll interval.
  `quadraui.asyncio.async_handler(app, handler)` adapts a `handler` that
  may be a plain callable or an `async def` into the synchronous shape
  `set_handler`/`set_render_content`/... themselves expect — an `async
  def` handler runs as its own `asyncio.Task` and must not block
  rendering, so a later event dispatches and renders while it is still
  awaiting. See `quadraui-py/examples/asyncio_demo.py`.
- A descriptor-decode failure now raises one of six new
  `InvalidDescriptorError` subclasses instead of the bare base class:
  `MissingFieldError`, `UnknownVariantError`, `TypeMismatchError`,
  `UnknownKeyError`, `VersionMismatchError`, `DecodeFailedError` — chosen
  by matching `quadraui_binding_core::DecodeError`'s own variant directly,
  never by parsing the exception's message. Each has a `code` class attribute
  (e.g. `"missing_field"`) mirroring `DecodeError::code`, and the
  exception's message now names the full path into the layout tree the
  problem was found at (e.g. `render: at split#body › panel#chrome:
  missing field "ratio"`), not just a bare field name — `DecodeError` no
  longer loses an ancestor node's id when a decode error happens several
  levels into a nested tree. Existing `except InvalidDescriptorError`/
  `pytest.raises(InvalidDescriptorError, ...)` call sites keep matching
  unchanged.
- `QuiApp.show_overlay(kind, descriptor, anchor)` / `QuiApp.dismiss_overlay(id)`:
  a generic entry point over the same seven in-canvas overlay kinds the
  existing `show_context_menu`/`show_message_dialog`/`show_toast`/
  `show_tooltip`/`show_completions`/`show_float`/`show_rich_text_popup`
  methods each open, naming the kind by string (`"context_menu"`,
  `"message_dialog"`, `"toast"`, `"tooltip"`, `"completions"`, `"float"`,
  `"rich_text_popup"`) rather than needing a dedicated method per kind.
  `show_overlay` returns an id `dismiss_overlay` can later close by — the
  descriptor's own `"id"` field for the five kinds that carry one, or a
  fixed `kind`-named sentinel for the two that don't (`"context_menu"`,
  `"message_dialog"`) — and `dismiss_overlay` can now also close a
  context menu, a capability none of the original 13 methods had (there
  was no `dismiss_context_menu`). All 13 existing methods are unchanged
  one-line forwards onto this pair, so no existing call site needs to
  change.
- `QuiApp.define_style(name, overrides)` registers a named style set — a
  reusable bundle of the same tokens a node's inline `"style"` dict
  accepts — and any layout-tree node may then name one or more of them
  through a new optional `"class"` key: a single name, a
  whitespace-separated string of names, or a list, the CSS `class=`
  analogue of `"style"`. Classes merge left to right (a later name wins a
  token an earlier one also named) and the node's own inline `"style"`
  beats all of them. A class name no set was defined under is ignored
  rather than raising — unlike an unknown key *inside* `"style"`, which
  is still an `InvalidDescriptorError`. Four theme-derived sets exist
  before any `define_style` call — `"primary"`, `"secondary"`,
  `"danger"`, `"subtle"`, each mapping onto the matching `Button` variant's
  own resting fill, and each following a later `set_theme` call; defining
  one of those names replaces it. Redefining a set repaints from the next
  frame on. `QuiApp.style_set_names()` lists what is registered.
  `QuiApp.actions().define_style(name, overrides)` is the counterpart a
  registered event handler calls instead of `QuiApp.define_style` directly
  — the same "already borrowed" restriction every other `actions()` method
  exists for — and is the path that makes a redefinition repaint a running
  `run_tui` loop immediately. See `docs/bindings-wire-format.md` and
  `quadraui-py/examples/style_classes_demo.py`.
- Any layout-tree node a render callback returns (`"row"`/`"column"`/
  `"split"`/`"tabs"`/`"panel"`/`"sidebar_panel"`/`"split_tree"`/`"scroll"`,
  or a leaf) may carry an optional `"style"` dict: `"bg"`/`"fg"`/
  `"border"`/`"accent"`/`"surface_bg"`/`"accent_bg"` (a `Color` or a
  `"#rrggbb"`/`"#rrggbbaa"` hex string) override `Theme` colour tokens,
  `"padding"`/`"corner_radius"`/`"border_width"` override `Style` metric
  tokens — applied as a derived theme/style scoped to that node's own
  subtree, so siblings are unaffected and a nested `"style"` only
  overrides the tokens it names. `"surface_bg"`/`"accent_bg"` are
  semantic aliases for the fill a `Button`/`List`/`Dialog`/... actually
  paints — a plain `"bg"` only ever reaches `Theme.background`, which a
  `Button`'s own fill never reads on any variant but `Icon`. See
  `docs/bindings-wire-format.md` for the full per-leaf token mapping.
  Only reachable today from a raw dict/object render tree — not yet
  wired into `quadraui.app.nodes`' own keyword/prop-list node builders.
- A render callback's returned `dict` may carry an optional top-level
  `"wire_version"` integer key. A tree with no `"wire_version"` key still
  decodes exactly as before (treated as version 1); a version the native
  extension doesn't understand yet raises `InvalidDescriptorError` naming
  both versions instead of silently misinterpreting it. No app needs to
  set this key today — it exists for a future wire-shape change to check
  against.
- A `"row"`/`"column"` child dict may now carry its own `"size"` key
  (`"EqualShare"`/`"Content"`, or `{"Fixed": n}`/`{"Percent": p}`/
  `{"Weight": w}` — the same vocabulary `MultiSectionView`'s own
  per-section `"size"` key already uses) plus optional `"min"`/`"max"`
  clamps, and the `"row"`/`"column"` node itself may carry a `"justify"`
  (`"start"`/`"center"`/`"end"`/`"space_between"`) and/or `"align"`
  (`"start"`/`"center"`/`"end"`/`"stretch"`) key. None of the five keys
  is required; a tree with none of them lays out exactly as before
  (every child an equal share, `Justify::Start`, `Align::Stretch`).
- **Backfill, `py-v0.8.0` through this file's creation:** this file starts
  now (see the note at the top), so none of the work below ever got its own
  entry; rather than leave a silent gap, it is summarized here as one
  block instead of the dozens of individual PRs it spans. `quadraui-py`'s
  interaction engine was extracted into the language-neutral
  `quadraui-binding-core` crate every binding now shares (hit testing,
  focus, semantic event synthesis, text-input/drag controllers). Every
  additive primitive went from paint-only to hit-testable and
  keyboard-focusable: `tree`/`data_table` gained scrollbar drag, wheel
  scroll, column resize, and focus-scoped arrow-key navigation;
  `split`/`panel`/`sidebar_panel` gained real divider drag and chrome
  hits; `toolbar`/`status_bar`/`activity_bar` gained click routing and
  `text_display` gained scroll; `form`/`combo_box`/`date_picker`/`switch`
  gained key/char/paste routing through `FormController`; and a `tabs`
  node gained inline tree-rename and its full field set. The menu bar and
  context menus were wired to real activation, and
  `QuiApp.show_message_dialog` added a modal overlay routed ahead of
  both. Text regions became selectable/copyable with richer clipboard
  access, `QuiApp.run_shell` added a `ShellApp`-backed app frame, and
  cross-thread event injection landed alongside a `WindowClose` veto
  default fix and a waker panic-safety hardening pass. See `git log
  py-v0.8.0.. -- quadraui-py quadraui-binding-core` for the full,
  PR-by-PR history.

### Changed

- `QuiApp.step`/`QuiShellApp.step`/`QuiApp.run` now render once whenever
  a step dispatches nothing (no event queued, no posted value, no tick
  that itself requested a redraw), not only when something explicitly
  forces one (`screen()`/`screen_contains()`, or an event/tick asking for
  a redraw). A `render` callback that decodes badly now raises from the
  very next idle `step()`/`run()` call instead of only the next explicit
  render.
- `_quadraui.pyi` (the compiled extension's type stub) now declares every
  `#[pymethods]`/`#[pyfunction]` the native extension exports — including
  `QuiAppActions`, which previously had no stub class at all — with real
  parameter/return types instead of falling back to `__getattr__ -> Any`.
  A `&Bound<'_, PyDict>`/`PyList` argument this binding decodes into one of
  `wire.pyi`'s own shapes (`UiEvent`, `Accelerator`, `ContextMenuItem`,
  `ImageSource`, the `insert_*` descriptor types, …) is typed as that
  `TypedDict`/`Sequence` instead of `Any`, so `mypy --strict`/`pyright`
  catches a misspelled key or wrong-shaped argument at the call site, the
  same way `wire.pyi` already does for a render callback's return value.
  Generated by `scripts/gen_wire_types.py`, which now also parses
  `quadraui-py/src/lib.rs` directly (there is no Rust-side JSON export for
  `#[pymethods]` the way `wire_schema.rs` provides one for wire data);
  CI's `wire-types-check` job fails if `_quadraui.pyi` drifts from `lib.rs`
  the same way it already fails for `wire.pyi`/`wire.d.ts` drifting from
  `wire_schema.rs`.
- A `"split"` node's `"first_min"`/`"second_min"` are now read in the same
  portable surface-native-pixel unit `"padding"`/`"margin"`/`"gap"` already
  use, not backend-native rows/columns. `"first_min": 10` previously
  reserved 10 TUI rows/columns outright; it now reserves `ceil(10 / 8)`
  TUI columns on a `Horizontal` split, or `ceil(10 / 16)` TUI rows on a
  `Vertical` one (the same `char_width`/`line_height` nominal-cell-px
  conversion `"padding"`/`"margin"`/`"gap"` use). An app pinning a
  `first_min`/`second_min` value sized for the old row/column reading
  needs to multiply it up (by roughly 8 for a `Horizontal` split, 16 for a
  `Vertical` one) to reserve the same TUI space as before.

### Deprecated

- `QuiApp.insert_button`/`insert_text_display`/`insert_list`/`insert_tree`/
  `insert_data_table`/`insert_tab_bar`/`insert_progress`/`insert_spinner`/
  `insert_image`/`insert_extra`/`clear_extra`/`slot_clear` -- every kind is
  reachable as an inline layout leaf from a `render` callback now, so the
  fixed-slot/additive-slot API is no longer the only way onto the screen.
  Each deprecated method still works exactly as before, but raises a
  `DeprecationWarning` naming the inline leaf kind that replaces it.
  Removal is a later breaking release.

## [0.10.1] - 2026-11-11

### Added

- `quadraui.testing` — the app-author test harness: a public API over the
  headless runner that queries by widget id/role/text (`harness(app)`,
  `find`/`find_all`/`query`/`exists`, and `Widget.find` for one subtree),
  synthesises the gestures a user performs (`press`, `type`, `click_text`,
  `Widget.click`/`press`/`focus`/`drag_divider`, `Row.click`/
  `double_click`/`activate`/`toggle`, `Tab.click`/`close`), and asserts on
  the rendered frame (`screen`, `contains`, `assert_text`,
  `assert_not_text`). Queries read the descriptors the app actually
  rendered and skip what is off screen (an inactive tab's content) unless
  asked for with `include_hidden=True`; a button click goes through the
  engine's own hit test against the label as it painted, so a test needs
  no coordinates. Wraps a `quadraui.app.App` directly, or a plain `QuiApp`
  given the same `render` callable it was built with.
  `tests/test_file_manager_app.py` is written entirely against it.
- A `pytest` plugin (`quadraui.testing.pytest_plugin`, registered by this
  distribution's own `pytest11` entry point) providing a
  `quadraui_harness` factory fixture: `quadraui_harness(app)` builds a
  `Harness` and `Harness.close()`s it when the test ends, so a test that
  enabled hot reload cannot leak its watcher thread into the rest of the
  session. No `conftest.py` needed in a consumer's own repo.
- Hot reload for `quadraui.app.App`: `enable_hot_reload(*modules,
  interval=0.5, on_reload=None)` watches the given modules' source files and
  re-imports them on change (a daemon thread wakes the event loop through
  `QuiApp.waker()`, so `run_tui()`/`run_native()` repaint with no input),
  plus `reload_if_changed()`, `reload()`, `disable_hot_reload()`,
  `reload_error` and `reload_count`. Component state, the state on a
  bound-method view's own object (its class is swapped for the reloaded
  one in place) and framework widget state (scroll, selection, focus) all
  survive. `Component.migrate_state(state)` runs once per surviving slot
  after a reload and by default merges in keys a new `initial_state()`
  added. A broken edit — import error or a view that raises on its first
  render — rolls back to the last good code and is reported on
  `reload_error`. Component state slots are now keyed by the class's
  qualified name rather than the class object. `examples/hot_reload_demo.py`
  demonstrates it.
- `QuiApp.widget_state(id)` — a read of a widget's current state in the
  engine's widget-state store: `selected_path`/`selected_idx`, `scroll_offset`,
  `ratio`, `active_tab` and `expanded`, whichever apply to the id's widget
  kind, or `None` if the store has nothing for it yet. Only callable from
  outside any active `render`/`handle` callback (it borrows the app the same
  way `step()`/`screen()` do). `examples/file_manager.py` relies on the store
  for its tree's scroll offset, its body split's ratio, and its details
  tabs' active index instead of hand-tracking any of the three; the outer
  chrome/status split's ratio is also now store-derived (previously fixed),
  so dragging that divider persists too — there is no "this split is fixed"
  opt-out yet.
- `QuiAppActions.apply_tab_activated(id, index)` — makes tab `index` of
  `TabBar`/`LayoutTabs` widget `id` active in the engine's own widget-state
  store directly, the same persisted change a real click on that tab makes,
  for a handler that needs to activate a tab programmatically (e.g.
  `examples/file_manager.py`'s "open this folder as a new tab and focus
  it") without pinning the descriptor's own active index, which would make
  every later render controlled and defeat click-driven switching.
- `quadraui.wire` — a `py.typed` `.pyi` stub (no runtime body; import names
  from it inside `if TYPE_CHECKING:`) with a `TypedDict`/`Literal` for every
  layout-tree node, the six core leaf descriptors, `status_bar`, the
  full `UiEvent` vocabulary (nested per-widget activations like `TreeEvent`/
  `DataTableEvent` included, each carrying the `hit: HitDetail` key the
  binding attaches whenever a hit test resolved one), and `Icon`'s two
  input shapes (a raw glyph, or a bundled codicon name), generated by
  `scripts/gen_wire_types.py` — `mypy --strict` now catches a misspelled
  descriptor/event key at type-check time instead of a runtime
  `InvalidDescriptorError`. `FormEvent`/`PaletteEvent` payloads and every
  additive primitive other than `status_bar` are typed `Any`/
  `Dict[str, Any]` for now; see that script's module doc.
- `quadraui._quadraui.pyi` — a hand-written companion stub for the compiled
  native extension `quadraui/__init__.py` re-exports from, so `py.typed`
  doesn't trade "no stub, skipped" for "has py.typed, can't resolve
  `_quadraui`" under a strict type checker. Only unblocks the import and
  types `QuiApp`'s constructor precisely; every other method on
  `QuiApp`/`QuiAppWaker`/`QuiShellApp` falls back to `Any` for now.
- `QuiAppActions.hide_tooltip()` — the deferred counterpart of
  `QuiApp.hide_tooltip`, for a handler that shows its own tooltip from a
  source with no `Button` to carry it (a toolbar action's own `tooltip`,
  or a tree/data-table row) and needs to dismiss it again from inside
  that same handler — typically on the hit-tested `MouseLeft` for the
  widget it showed the tooltip on `MouseEntered` for. Calling
  `app.hide_tooltip()` directly from there raises "already borrowed", the
  same wall `QuiAppActions.show_tooltip` already works around.
- `QuiApp.set_test_clock_ms(ms)`: a test-oriented override for the
  headless clock `drive_hover_tooltip` reads via
  `quadraui_binding_core::BindingHost::now_ms`. Pins it to an absolute
  millisecond value (`None` reverts to the real wall clock), so a test can
  drive the hover-tooltip delay and auto-dismiss timeout deterministically
  instead of a real `time.sleep`.
- `QuiApp` platform-services methods, each a thin wrapper over the matching
  `quadraui_binding_core::services` function: `check_for_update`/
  `download_and_stage`/`apply_on_quit` (auto-update), `begin_power_save_block`/
  `end_power_save_block` (the sleep/suspend inhibitor), `set_login_item`/
  `login_item_enabled` (launch at login), `set_as_default_protocol_client`/
  `is_default_protocol_client`/`remove_as_default_protocol_client` (OS
  protocol registration), `begin_drag_out` (OS-level drag-out of this app's
  window), and `add_recent_document`/`clear_recent_documents`/
  `set_dock_menu`/`set_jump_list` (recent documents, the Dock icon menu, and
  the Windows jump list). A new `UnsupportedError` exception (mirrors
  `quadraui-ffi`'s `QUI_ERR_UNSUPPORTED`) surfaces a backend with no
  implementation for one of these — the headless `TuiBackend` implements
  none of them for real — instead of
  a panic or a silent no-op. The void-returning methods among these
  (everything but `check_for_update`, `download_and_stage`,
  `begin_power_save_block`, `login_item_enabled`, and
  `is_default_protocol_client`) also gained a `QuiAppActions` mirror for use
  from inside a registered handler.
- Three module-level, app-less functions: `quadraui.app_path(kind)` resolves
  a standard per-app filesystem location (`"user_data"`, `"app_config"`,
  `"app_cache"`, `"logs"`, `"temp"`, `"downloads"`, `"documents"`, `"home"`);
  `quadraui.set_app_identifier(identifier)` sets the process-global
  identifier its app-scoped kinds resolve under;
  `quadraui.install_crash_handler(cfg)` installs a process-global Rust
  panic hook that writes a crash record on an uncaught panic.
- A `MouseMoved` event landing on (or leaving) any painted primitive now
  dispatches a hit-tested `MouseEntered`/`MouseLeft` to the registered
  handler ahead of the raw move itself, each carrying the primitive's own
  `widget` id and `hit` detail — the real hover signal a tree/details-row
  (or any other non-toolbar widget) hover tooltip needs, instead of a
  handler re-deriving hit-testing from the raw cursor position itself.
  Moving between two hits with the same widget id but a different detail
  (e.g. row 2 to row 3 within the same tree) still reads as a hover
  change. Suppressed while a divider/scrollbar/column drag is in
  progress.
- A `"tabs"` layout-node dict accepts an optional `"focused_index"` key
  naming the keyboard-focused tab. TUI paints it in reverse video;
  omitting the key (or passing `None`) keeps today's unfocused paint.
- `QuiAppActions.set_theme(overrides)` — the deferred counterpart of
  `QuiApp.set_theme`, so a handler reacting to `UiEvent::SystemThemeChanged`
  can repaint the live `run_tui`/`run_native` backend without hitting the
  "already borrowed" PyO3 error a direct `app.set_theme(...)` call raises
  from inside that same dispatch. Decodes and validates `overrides` against
  `Theme::default()`'s field set immediately (an unknown field or malformed
  colour still raises synchronously); `flush_deferred_actions` re-merges the
  stored overrides onto the real backend's current theme and applies them
  via `Backend::set_theme` right after the dispatch that queued it finishes.
- `QuiApp.actions()` returns a `QuiAppActions` handle a registered event
  handler can call `show_message_dialog`/`show_context_menu`/`show_toast`/
  `show_tooltip`/`send_notification`/`open_path`/`reveal_in_file_manager`/
  `move_to_trash`/`restore_from_trash` on without hitting the "already
  borrowed" PyO3 runtime error a direct call on `app` itself raises from
  inside a handler. Each method queues
  the already-decoded action instead of applying it immediately;
  `PyAppLogic::handle`/`tick` apply it right after the dispatch that
  queued it finishes, against whichever live backend that dispatch was
  given — the headless driver for `step`/`run`, the live terminal for
  `run_tui`, and the live window for `run_native`'s `gtk`/`macos`/`win`
  loops alike.
- `QuiApp.restore_from_trash(path)` restores `path` from the platform
  trash/recycle bin back to its original location — the inverse of
  `QuiApp.move_to_trash`, for an "Undo" affordance on a delete-confirm
  toast. Raises `InternalError` on a real filesystem failure, when no
  trashed item matches `path`, or when built for macOS (no programmatic
  trash listing/restore API there — see
  `quadraui::PlatformServices::restore_from_trash`'s doc).
- A click on a focused `text_input` extra now places the caret under the
  click (`EditOp::SetCursor`, resolved via `TextInput::layout`'s own
  metrics-aware hit-test) instead of only moving keyboard focus there,
  and the caret is now actually painted there too (`sync_text_input_carets`
  repaints the extra from the cached editor's live cursor position after
  the plain paint pass, since the descriptor's own `cursor_col` is
  normally `0` with nothing to echo it back).
- `Ctrl`/`Cmd`+`A`, `Ctrl`/`Cmd`+`Z`, and `Ctrl`/`Cmd`+`Shift`+`Z` on a
  focused `text_input` extra now resolve to `EditOp::SelectAll`/`Undo`/
  `Redo` — the per-widget cursor cache now persists the whole
  `TextEditor` session (including its undo history) across keystrokes
  instead of a fresh, empty one every call, so a selection extended with
  `Shift`+arrows, or an edit undone/redone, is actually reachable. The
  chord match is exact (`Ctrl+Shift+A`/`Ctrl+Alt+Z` no longer also
  trigger `SelectAll`/`Undo`), matching the canonical
  `KeyBinding::SelectAll`/`Undo`/`Redo` chords.
- `number_input`, `combo_box`, `date_picker`, and `radio_group` extras are
  now hit-testable and tab stops, with their own keyboard/`click`
  semantics routed into `Form` events instead of staying paint-only: a
  `NumberInput` stepper click or `Up`/`Down` key resolves to
  `FormEvent::TextInputChanged`/`Committed`; `RadioGroup`'s arrow keys to
  `FormEvent::SegmentedControlChanged`; a `ComboBox`'s typing/navigation/
  commit/cancel to `FormEvent::ComboBoxTextChanged`/`HighlightChanged`/
  `Committed`/`Cancelled`; and a `DatePicker`'s `Enter` to
  `FormEvent::DateChanged`. Resolving a click to a specific option/item/
  day cell (rather than just focusing the whole control) is not yet
  wired.
- A `"form"` extra is now hit-testable and a tab stop, and is routed
  through its own cached `FormController` instead of staying paint-only:
  a click on a `Toggle`/`Button`/text field, or a keystroke while a field
  inside it has focus, resolves through `FormController`'s own
  field-level dispatch and emits the matching `Form` event
  (`ToggleChanged`, `ButtonClicked`, `TextInputChanged`, `FocusChanged`,
  ...) — `FormEvent::ButtonClicked` was previously never emitted by any
  path. `Tab`/`Shift`+`Tab` field cycling inside a form is not yet
  reachable this way (the app-level tab order claims it first); a click
  still moves field focus directly. A descriptor's own `has_focus`/
  `scroll_offset` seed the controller (so a host painting them directly
  keeps working), `has_focus` additionally tracks engine-level focus in
  both directions every frame (the focused-row highlight now appears the
  render right after a click, and clears once focus moves elsewhere), and
  a wheel `Scroll`/scrollbar-thumb drag over the form now scrolls it
  (`MouseUp`/`MouseMoved`/`Scroll` are now routed into the cached
  `FormController`, not just `MouseDown`).
- `QuiApp.viewport()`, `line_height()`, `char_width()`,
  `list_char_width()`, `chrome_line_height()`, `scrollbar_reserve()`,
  `scales_text_rows()`, `snap_height(h)`, `measure_text(text, role)`, and
  `backend_caps()` expose the headless backend's real layout metrics and
  declared capabilities — `Backend::viewport`/`line_height`/`char_width`/
  `list_char_width`/`chrome_line_height`/`scrollbar_reserve`/
  `scales_text_rows`/`snap_height`/`measure_text`/`backend_caps` from the
  `quadraui` core crate — so a Python app can size and scroll content
  against measured values instead of a hardcoded guess.
- `InvalidFontRoleError`, raised by `measure_text` when its `role` argument
  isn't `"chrome"` or `"editor"`.
- `QuiApp.register_accelerator(descriptor)` / `unregister_accelerator(id)`
  expose `Backend::register_accelerator`/`unregister_accelerator` from the
  `quadraui` core crate, so a declared chord fires `UiEvent::Accelerator`
  instead of a plain `KeyPressed` — registered on this app's own live
  backend immediately, and replayed onto whichever real backend
  `run_tui`/`run_native` goes on to construct.
- `QuiApp.register_global_shortcut(descriptor)` /
  `unregister_global_shortcut(id)` / `unregister_all_global_shortcuts()`
  expose `quadraui::PlatformServices`'s OS-wide hotkey seam — headless-only
  for now, and raising `InternalError` until an in-tree backend implements
  a real OS hotkey call.
- `quadraui.app` — a thin app layer over `QuiApp`'s render/handle seam:
  typed node constructors (`Split`, `Column`, `Row`, `SidebarPanel`, `Tab`,
  `Tabs`, `Tree`, `Table`, `StatusBar`, `TextDisplay`, `Button`, `Toolbar`)
  that build the existing wire dict shapes and accept `on_*` callbacks
  directly (`Button("Save", on_click=self.save)`), a `Component` base
  class for view + local state bundled together and keyed by tree
  position (or an explicit `key=`) so it survives re-render, and an `App`
  that walks a `view()` tree once per render into an id → callback
  registry and dispatches events against it — no manual id switching, no
  scroll/selection bookkeeping (both stay framework-owned widget state).
  `examples/file_manager_app.py` ports `examples/file_manager.py` onto it.

### Fixed

- `QuiApp.widget_state(id)`'s `expanded` field is now actually reachable: a
  `Tree` leaf's render path now resolves its per-row expand/collapse set
  against the engine's own widget-state store (via
  `quadraui_binding_core::resolve_tree_expansion`) before painting, the
  same way its cursor/scroll already did. Previously nothing in this
  binding's render path ever called `resolve_tree_expansion`, so an
  uncontrolled tree's toggled rows never persisted (expand/collapse only
  worked when the descriptor pinned `is_expanded` itself every frame) and
  the store's `TreeExpandedState` entry was swept by the next frame's GC
  before `widget_state()` could ever read it back — `expanded` was always
  `None`.
- `QuiApp.run_native()` now swaps `QuiApp.waker()`'s shared cell onto the
  live `GtkRunner`/`MacRunner`/`WinRunner` backend's own waker for as long
  as the native window is open, restoring it once `run_native` returns —
  mirroring `run_tui`'s existing swap. Previously, a `QuiAppWaker.post`/
  `.wake`/`.post_after`/`.wake_after` call made while a native window was
  open queued into the dormant headless driver instead, where it was never
  observed: `run_native` moves the whole handler-carrying `PyAppLogic` into
  the native window, leaving nothing registered on the headless driver to
  read that queue back.

### Changed

- A `list` event dict can now carry an `"ItemDoubleClicked"` key (double
  click, distinct from a plain click's `"SelectionChanged"`) alongside the
  existing `"SelectionChanged"`/`"ItemActivated"`/`"KeyPressed"`/
  `"SelectionSet"` — mirrors the Rust-side `ListViewEvent::ItemDoubleClicked`
  through the same generic event-to-dict pythonization every other `list`
  event already goes through, no new binding code needed. A `data_table`
  event dict gained the equivalent `"RowDoubleClicked"` key alongside
  `"RowSelected"`/`"RowActivated"`/`"HeaderClicked"`/etc. A single click on
  a `list`/`data_table` row now only ever produces
  `"SelectionChanged"`/`"RowSelected"`, never `"ItemActivated"`/
  `"RowActivated"` — a Python app that was treating a plain click as
  "open" (reading `"ItemActivated"`/`"RowActivated"` from a single click)
  now needs to read `"ItemDoubleClicked"`/`"RowDoubleClicked"` instead, or
  listen for both to also catch Enter (see next entry).
- Pressing Enter on a focused `list` with a selected row now also produces
  an `"ItemActivated"` event dict (previously only a focused `data_table`'s
  Enter did; a focused list's Enter fell through as a raw `"KeyPressed"`
  dict instead). A Python app wiring up "open this row" from the keyboard
  listens for `"ItemActivated"` on a `list` event the same way it already
  would on a `data_table` one.
