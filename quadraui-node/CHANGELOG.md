# Changelog

All notable changes to the `@quadraui/node` package (`quadraui-node`) are
documented in this file. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

This file starts now, not retroactively — `quadraui-node` has history
predating it (see `git log -- quadraui-node`); none of it is backfilled
here. `[Unreleased]` is where changelog discipline begins for this package.

Unlike the `quadraui` Rust crate's own `CHANGELOG.md`, entries here are
written directly under `[Unreleased]` — there is no `changelog.d/`
fragment step for this package (that mechanism, and
`tools/changelog_check.py`'s enforcement of it, only covers
`quadraui/src/**`).

## [Unreleased]

### Added

- `TextDisplay(lines, { showScrollbar: true })` paints a scrollbar and makes
  the pane a Tab stop the keyboard can scroll (#3108). Wire descriptors
  gain `Palette.show_mode_badge` (default true; false hides the `[L]`/`[I]`
  badge) and `FindReplacePanel.hidden_toggles` (toggles the panel does not
  offer: no paint, no click, no Alt shortcut) (#3082). A `cell_grid` leaf
  scrolls under the mouse wheel, and while a cell is being edited a click
  on it places the caret and a drag selects (#3082).

- `QuiAppActions.showOverlay(kind, descriptor, anchor)`/
  `.dismissOverlay(id)` -- the same nine `kind`s and anchor semantics as
  `QuiApp.showOverlay`/`QuiApp.dismissOverlay`, callable from inside a
  registered handler (where the `QuiApp` methods themselves raise
  `ReentrantCallError`). `App.confirm`/`App.alert` now queue through
  `showOverlay("message_dialog", ...)` instead of `showMessageDialog`.

- A `"cell_grid"` leaf's navigation/editing/virtualisation outcomes reach
  the registered handler as an ordinary `UiEvent` variant,
  `{"CellGrid": [id, event]}`, rather than the earlier
  `{"CellGridController": [id, event]}` shape: `CellGrid` carries no
  state of its own across frames, so the handler must apply
  `Navigated`/`CellEditStarted`/`CellEditChanged`/`CellEditCommitted`/
  `CellEditCancelled` back onto its own render state before the next
  frame. `CellsRequested` (the two-axis-virtualisation window request)
  still needs no new host call to answer -- re-render with that
  `"cell_grid"` leaf's `cells`/`loaded_row_start`/`loaded_col_start`
  covering the named block, same as any other leaf's per-frame data.
- A `"grid"` layout node: `{grid: {columns: [...], rows: [...], cells:
  [...]}}` lays its cells out row-major across two independent track
  lists, each track one of `{Fixed: px}`/`{Percent: 0.0-1.0}`/`{Weight:
  fr}`/`{Content: px}` (`GridTrack` in `wire.d.ts`), resolved through the
  same `Fixed`/`Content` -> `Percent` -> `Weight` pass order a row/column
  child's `size` key already uses. Takes the usual `padding`/`margin`
  siblings (no `gap`: a grid's tracks carry no gutter). More `cells` than
  `columns x rows` has room for is a decode error rather than a silently
  dropped tile. `wire.d.ts` carries the generated
  `GridTrack`/`LayoutGridPayload`/`LayoutGridNode` types.
- A `"toolbar"` extra's object may now set `overflow` (`"Clip"`/`"Wrap"`/
  `"Menu"`) and `overflow_priorities` (a `{id, priority}[]` list) to opt
  into `ToolbarOverflow`'s wrap/menu-with-drop-priority layouts instead of
  today's Clip-only behaviour. Both keys are optional; a `"toolbar"`
  object with neither renders exactly as before.
- A click on a `"tree_table"` extra's branch-row chevron now resolves to a
  typed `{TreeTable: [id, {ToggleExpanded: {path, expand}}]}` event
  instead of only a raw `MouseDown`; a click elsewhere in the row body
  still falls back to the raw `MouseDown`.
- A `"style"` object (inline on a node, or passed to `defineStyle`) now
  accepts `font_family`/`font_size`/`font_weight` (a string, a number,
  and `"normal"`/`"bold"` respectively) and `hover`/`pressed`/`focus` --
  each a nested `StyleValue` applied over the override's own base tokens
  while the node is in that interaction state. A state branch may itself
  carry colour/metric/font tokens but may not nest a further
  `hover`/`pressed`/`focus` branch. `wire.d.ts`'s `StyleValue` carries the
  six new keys.
- `App.confirm(title, body, buttons, severity?)` and `App.alert(title,
  body, options?)`: queue an in-canvas dialog through `App.actions`
  (never the direct, mid-dispatch-unsafe `QuiApp.showMessageDialog`) and
  resolve a `Promise<string>` with whichever button's id the user picked
  -- a click, or Enter/Escape activating the `is_default`/`is_cancel`
  button. `App.actions` is now typed as `QuiAppActions` (exported from
  both `js/index.ts` and `js/index.d.ts`) instead of `unknown`, so no
  `as any` cast is needed to call a method on it.
- `List(items, options)` (and the `<list>` JSX intrinsic): a typed node for
  a `ListViewDescriptor` leaf, routing `SelectionChanged` to `onSelect` and
  either `ItemDoubleClicked` or `ItemActivated` to `onActivate` -- no raw
  wire object or `"List"` branch in `onEvent` needed to build a selectable
  list.
- `Form(fields, options)` (and the `<form>` JSX intrinsic) plus
  `TextInputField`/`DropdownField`/`ToggleField`/`SliderField`/
  `PasswordField`/`TextAreaField`: typed `FormFieldDescriptor` builders,
  each with its own `onChange` (and `onSubmit` where the field supports a
  commit) registered under that field's own resolved id, so a detail form
  needs no raw wire object and no `"Form"` branch in `onEvent`.
- `Checkbox`/`Slider`/`Select`/`Password`/`TextArea` (and their matching
  `<checkbox>`/`<slider>`/`<select>`/`<password>`/`<textarea>` JSX
  intrinsics): standalone leaf siblings of the `Form` field kinds above,
  for the same wire kinds used outside a `Form`.
- `README.md` now documents install (npm vs. from source), build and run
  steps, the CommonJS-vs-ESM `.ts`/`require` constraint the examples and
  tests rely on, and this `CHANGELOG.md` itself.
- `defineWidget({measure, paint, onEvent, a11y})` (`js/app/widget.ts`) and
  the `<widget>` JSX intrinsic: script-authored widgets with their own
  measure/paint/hit-testing/keyboard handling and accessibility, built on
  `quadraui-binding-core`'s generic `ScriptedPrimitive`/`ScriptWidget`
  seam. Typed props flow through the same `.d.ts` generation as any other
  node; local state uses the same keyed-state mechanism `Component` does.
  `js/testing.ts`'s `Widget.press()` now drives a focused scripted
  widget's own key handling directly. See `examples/rating.ts` for a
  runnable 5-star rating control.

### Changed

- Focus renders the same as in Rust (#3108): a `TextDisplay` is not a Tab
  stop unless it shows a scrollbar; the TUI focus ring never covers a
  control's glyphs (a focused button keeps both brackets, underlined at its
  natural width, with accent-coloured brackets; a list's ring steps around
  its rows' text); and a `cell_grid` cursor on a cell filled close to the
  cursor colour paints inverse so it stays visible (#3082).

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
  - A key the bar has no use for still reaches the handler once, as the
    bar's `TabBar` `KeyPressed` event and not also as the raw `KeyPressed`.
  - A double-click on a tab reaches the handler as the raw `DoubleClick`
    (before, a second `TabActivated`).

- The four text-entry leaf kinds, `"text_input"`, `"text_field"`,
  `"password"` and `"textarea"`, are the core widgets painted and edited
  through their own state (#2977/#2978), with these visible changes:
  - `"text_input"` is the multi-line core `TextInput`: Enter inserts a line
    break (a `TextInputChanged` with the `\n`) instead of reporting
    `TextInputCommitted`. Use `"text_field"` or `"password"` for a field that
    commits on Enter.
  - A key that only moves the caret or the selection (arrows, Home/End,
    Shift-selection, Ctrl/Cmd+A) is claimed by a focused `"text_input"`,
    and no longer reaches the handler as a raw `KeyPressed`.
  - `"text_field"` now edits: typing, paste, click-to-caret, the clear
    button and Enter (a `TextInputCommitted`) all work, where it used to paint only.
  - `"textarea"` with no `"cursor"` edits from the end of its value
    (it used to start at column 0), and a click on a `"textarea"` or
    `"password"` leaf focuses it without moving the caret. Both now answer
    Ctrl/Cmd+A (select all) and Ctrl/Cmd+C/X (copy and cut, as
    `TextCopied`; a password swallows them), which used to pass through.
  - `"password"` accepts a `"placeholder"`, and a `"textarea"`'s
    `"placeholder"` is a string, no longer nullable; `wire.d.ts` follows.
  - A `"text_input"` leaf's natural height counts its border rows, so
    under a non-`stretch` `align` it sizes to the box it paints.
- `TextArea(value, { placeholder: null })` omits the placeholder
  instead of sending `null`.

- A `"menu_bar"` leaf is the whole application menu (`AppMenu`). A click
  or key that opens a menu or moves a highlight reports
  `{ AppMenu: [id, { Changed: view }] }`. An object that leaves every view
  key out (`open_item`, `focused_item`, `dropdown_selected`,
  `submenu_path`, `submenu_selected`) is uncontrolled: the menu keeps its
  own view, so it opens and closes with no app code, as before. An app
  that merges each `Changed` view back into the `menu_bar` object it
  renders next controls the view instead. An activation
  still reports `{ MenuActivated: itemId }`, now after the `Changed` view
  that closes the menu and with the `hit: { kind: "widget_body" }` every
  generic leaf's event carries. The dropdown is a popup routed to the
  menu: a press on a row activates it, and a press elsewhere or Escape
  closes the menu (as a `Changed` view). The bar is a focus stop, and its
  Alt mnemonics and arrow keys reach it while it holds focus, not
  globally. While a dialog is open the dropdown is hidden and takes no
  input; it is no longer closed by opening the dialog. `wire.d.ts`'s
  `MenuBar`/`MenuBarItem` types are now `AppMenu`/`MenuDef`.
- A `"form"` leaf routes through `Form`'s own widget contract: its events
  carry `hit: { kind: "widget_body" }` like every generic leaf's. Field
  focus the descriptor leaves unset is still the form's own to track; Tab
  inside a focused form walks its fields and moves on to the next stop
  past either end, Up/Down move between fields the focused one has no use
  for, and a wheel notch scrolls it. A form's slider field (`SliderField`)
  moves on a click or drag on its track and on the arrow keys and Home/End
  while focused, firing `onChange`, instead of only through an a11y
  Increment/Decrement. Any open widget popup (a `"combo_box"` list) now
  receives the press that lands on it, instead of the press being dropped.
- **Breaking:** a `"tree_table_controller"` leaf's `nodes` are content
  read every frame (#2978, 0.12 rec 14), no longer a seed read once on
  the leaf's first frame. The widget keeps only what the user does to the
  tree -- expanded branches, the selection, the scroll position and the
  header sort -- keyed by node id. So:
  - lazily loaded children reach the widget as that node's `children` in
    the next render, not through a method call;
  - `selected_id` selects that node whenever the app changes it (between
    changes the user's own selection stands);
  - the new `sort_override: true` says `nodes` already arrive in the
    app's own order for the current sort, so a header click reports
    `SortChanged` and moves the sort indicator but does not re-order the
    rows;
  - `has_focus` paints as sent; keyboard focus no longer forces the
    focused look on.

  Events keep their `{"TreeTableController": [id, event]}` shape and
  variants, and now reach the handler through the ordinary `UiEvent` path,
  so `pushEvent` accepts that shape too. See *Removed* for the migration.
- A generic widget leaf lays out and paints with the `Interactive` state
  its own events left (#2978): binding-core keeps that state in the same
  per-thread store the Rust runtime paints from. A widget whose paint
  reads its state (a caret, a text selection, an open section) now shows
  it in a `quadraui-node` app as it does in a Rust one.
- A `"scripted_widget"` leaf's payload now names the registered widget's
  id: `{ id, scripted_widget: { id, props } }` (#2978). The leaf decodes
  through core's runtime kind registry into a generic widget leaf, so the
  payload carries its id like every other widget kind's does.
  `defineWidget` builds this shape; a raw `registerScriptedWidget` caller
  must add `id` to the leaf it returns. A pointer that enters or leaves a
  script widget reports `MouseEntered`/`MouseLeft` with hit detail
  `{ kind: "widget_body" }` rather than `{ kind: "canvas_region", region }`;
  its `{ Canvas: [id, { RegionHovered }] }` events are unchanged. The
  payload's generated type is no longer named `ScriptedWidgetDescriptor`
  in `wire.d.ts`.

- A `"completions"` overlay (`showOverlay("completions", ...)`) now routes
  pointer input through core `Completions`' own event handling: a click
  on a row reports `{ List: [id, { SelectionChanged: { idx: n } }] }` and
  moves the highlight, and only a double-click reports
  `{ CompletionItemActivated: [id, n] }` and closes the popup (a single
  click used to activate). A press on the popup no longer moves focus to
  it, so the editor underneath keeps receiving keys.
- A wheel notch over an overflowing `"rich_text_popup"` overlay scrolls the
  popup and reports `{ ScrollOffsetChanged: { widget: id, new_offset: n } }`
  instead of reaching whatever is painted behind it.
- `wire.d.ts` (#2979) is generated from a JSON Schema derived from the
  Rust wire types themselves, so every descriptor type is now named
  after its Rust type: the `Descriptor` suffix is gone (`ButtonDescriptor`
  -> `Button`, `TreeViewDescriptor` -> `TreeView`, `ListViewDescriptor` ->
  `ListView`, `ColumnDescriptor` -> `Column`, `ListItemDescriptor` ->
  `ListItem`, `TreeRowDescriptor` -> `TreeRow`, `CanvasDescriptor` ->
  `Canvas`, `PasswordDescriptor` -> `PasswordInput`, `FloatDescriptor` ->
  `Float`, ...). `SplitTreeRecursiveNode` is `SplitTreeNode`,
  `EditorDiffLine` is `DiffLine`, `MsvAxis` is `Axis` and
  `DialogChromeDescriptor` is `DialogChromeWire`. `TreePath` is gone (a
  path is a plain `number[]`), as are `EditorStyle`, `IconGlyph` and
  `IconCodicon` (`Icon` is still the union of both shapes). Optionality
  now matches what the decoder accepts: an `Option<T>` field may be
  omitted (`field?: T | null`) rather than being a required
  nullable key, and field types are the real Rust types (an integer field
  is `number`, an enum field its literal set). A `list`/`tree`/`data_table`
  leaf body is `ListView | ListViewLeaf` (the leaf variant adds
  `scroll_px`/`items_window`), and `WidgetKind` names every leaf kind.
  Doc comments are the Rust types' own first paragraph. New definitions
  describe the leaf `a11y`/`draggable`/`drop_target` keys, every
  `run_shell` config shape and the drag/tree-table/hit wire events.

- `progress`, `spinner`, `scrollbar`, `command_line`, `message_list`,
  `multi_section_view` and `find_replace` leaves decode through the
  generic widget leaf (`quadraui::decode_widget`), like `button` or
  `switch`. A click on one now carries `widget: <id>` and
  `hit: { kind: "widget_body" }` (previously `progress`/`spinner`/
  `scrollbar`/`message_list`/`multi_section_view`/`command_line` attached
  no hit and `find_replace` attached `{ kind: "extra_body" }`). A click on
  `find_replace` is resolved by the panel itself: after the
  `FocusChanged`, the handler receives `{ FindReplace: [id, event] }`
  (e.g. `FieldEdited` placing the caret in the clicked field) instead of
  the raw `MouseDown`. `message_list` drag-select and Ctrl-C copy are
  unchanged.
- `number_input`, `combo_box` and `date_picker` leaves decode through the
  generic widget leaf too, so each widget's own `Interactive::handle`
  produces its events. `number_input` reports a step, an arrow key or a
  commit as `NumberChanged`/`NumberDraftChanged`/`NumberCommitted` with a numeric value, e.g.
  `{ Form: [id, { NumberChanged: { id, value: 43 } }] }` (previously re-shaped into a string-valued
  `{ Form: [id, { TextInputChanged: { id, value: "43" } }] }`). `date_picker` reports
  a click or arrow-key scrub as `{ Form: [id, { DateFocusMoved: ... }] }` (previously
  dropped). `combo_box` reports the first filtered option as
  `ComboBoxHighlightChanged` before each typed `ComboBoxTextChanged`, and
  `Enter` on a closed box opens the list rather than committing.
- `toolbar` decodes through the generic widget leaf (`quadraui::Toolbar`,
  which carries `overflow`/`overflow_priorities` itself). A click on a
  button now parks the bar's keyboard cursor there, so the handler
  receives `{ FocusChanged: "<bar id>" }` before the `Toolbar` event. The bar's
  keyboard cursor is the widget's own state: after Tab focuses the bar,
  the arrow keys move it (the first press parks it on the first button)
  and Enter/Space activate the button under it; the descriptor's
  `focused_index` no longer chooses what Enter activates. The
  `"extra_chrome"` hit kind is removed.
- `tree_table` decodes through the generic widget leaf, so
  `quadraui::TreeTable`'s own `Interactive::handle` resolves every click:
  a click on a row body focuses the table and reports
  `{ TreeTable: [id, { RowSelected: { idx, path } }] }` instead of reaching the
  handler as a raw `MouseDown`. The `"tree_table_chevron"` hit kind is
  removed; a chevron click still reports `ToggleExpanded`.
- A `tree` row mid-rename edits through core's single-line text editor,
  so the rename honours the same keys every text field does: Ctrl/Cmd+A
  selects the whole name, Shift+arrows extend the selection and
  Ctrl/Alt+arrows move by word, each reported as `RowRenameChanged`.
- A tooltip opened with `show_tooltip` is sized by the tooltip's own
  measure, so its box matches the one every other host paints (on a cell
  grid, one column narrower than before).

### Removed

- The `"extra_body"` hit detail (#2977). Every widget leaf is a generic
  widget now and reports `"widget_body"`; no leaf produced `"extra_body"`
  any more. The `_HitDetailExtraBody` type is gone from `wire.d.ts`.

- The `"canvas_region"` hit detail (#2978): a script widget reports
  `"widget_body"` like every other generic widget, and its region events
  arrive as `{ Canvas: [id, ...] }`.

- `QuiApp.listCharWidth()` and `QuiApp.scrollbarReserve()` (#2979, 0.12
  rec 14), part of the 0.12 binding API cleanup. Budget row text with
  `charWidth()` or `measureText()` instead.

- `QuiApp.setTreeTableChildren(id, nodeId, children)` and
  `QuiApp.setTreeTableSortOverride(id, nodes)` (#2978, 0.12 rec 14).
  Lazily loaded children and a host sort order are
  `"tree_table_controller"` descriptor content now: keep them in the app's
  own model and send them from the render callback. Before:

  ```ts
  app.setHandler((event) => {
    if ("TreeTableController" in event) {
      const [tableId, payload] = event.TreeTableController;
      if ("ChildrenRequested" in payload) pending.push([tableId, payload.ChildrenRequested.id]);
    }
  });
  app.run(1);
  for (const [tableId, nodeId] of pending) app.setTreeTableChildren(tableId, nodeId, load(nodeId));
  ```

  After -- the handler records what the app now knows, and the render
  callback builds the whole tree from it:

  ```ts
  const loaded = new Map<string, Node[]>(); // node id -> children the app has loaded
  let sort: [number, boolean] | null = null; // set when the app sorts rows itself

  app.setHandler((event) => {
    if (!("TreeTableController" in event)) return;
    const payload = event.TreeTableController[1];
    if ("ChildrenRequested" in payload) {
      const id = payload.ChildrenRequested.id;
      loaded.set(id, load(id));
    } else if ("SortChanged" in payload) {
      sort = [payload.SortChanged.col, payload.SortChanged.ascending];
    }
  });

  const wire = (n: Node): object => ({
    id: n.id,
    cells: n.cells,
    ...(n.isBranch ? { has_children: true } : {}),
    // Leave `children` out until loaded: that is a lazy branch.
    ...(loaded.has(n.id) ? { children: myOrder(loaded.get(n.id)!, sort).map(wire) } : {}),
  });

  app.setRender(() => ({
    row: [{ id: "files", tree_table_controller: {
      id: "files", columns, nodes: myOrder(roots, sort).map(wire),
      sort_override: sort !== null, // leave out to keep the built-in text sort
    } }],
  }));
  ```

  An app that is happy with the built-in sort (cell text, branches first
  with `folders_first`) leaves `sort_override` out and ignores
  `SortChanged`.

- `QuiApp.insertButton`/`insertTextDisplay`/`insertList`/`insertTree`/
  `insertDataTable`/`insertTabBar`/`insertProgress`/`insertSpinner`/
  `insertImage`/`insertExtra`/`clearExtra`/`slotClear`: the six fixed
  primitive slots and every additive primitive now reach the screen only
  through a `setRender` callback's descriptor object. `SlotOccupiedError`/
  `UnknownSlotError` are removed with them.
- The 13 legacy per-kind overlay methods on `QuiApp`, and their 5
  duplicates on `QuiAppActions`, now that `showOverlay`/`dismissOverlay`
  cover every kind each one forwarded onto:

  | Removed | `QuiApp` replacement | `QuiAppActions` replacement |
  | --- | --- | --- |
  | `showContextMenu(items, position)` | `showOverlay("context_menu", {items}, position)` | same, via `actions.showOverlay` |
  | `showMessageDialog(title, body, buttons, severity?)` | `showOverlay("message_dialog", {title, body, buttons, severity}, null)` | same, via `actions.showOverlay` |
  | `dismissMessageDialog()` | `dismissOverlay("message_dialog")` | n/a (`QuiAppActions` never had one) |
  | `showToast(toast, corner?)` | `showOverlay("toast", {...toast, corner}, null)` | same, via `actions.showOverlay` |
  | `dismissToast(id)` | `dismissOverlay(id)` | n/a (`QuiAppActions` never had one) |
  | `showTooltip(tooltip, anchor)` | `showOverlay("tooltip", tooltip, anchor)` | same, via `actions.showOverlay` |
  | `hideTooltip()` | `dismissOverlay("tooltip")` | `dismissOverlay("tooltip")` |
  | `showCompletions(completions, cursor)` | `showOverlay("completions", completions, cursor)` | n/a (`QuiAppActions` never had one) |
  | `dismissCompletions()` | `dismissOverlay("completions")` | n/a (`QuiAppActions` never had one) |
  | `showFloat(float, width, height)` | `showOverlay("float", float, {width, height})` | n/a (`QuiAppActions` never had one) |
  | `dismissFloat()` | `dismissOverlay("float")` | n/a (`QuiAppActions` never had one) |
  | `showRichTextPopup(popup, anchor)` | `showOverlay("rich_text_popup", popup, anchor)` | n/a (`QuiAppActions` never had one) |
  | `dismissRichTextPopup()` | `dismissOverlay("rich_text_popup")` | n/a (`QuiAppActions` never had one) |

### Fixed

- While a modal overlay (`showOverlay("message_dialog", ...)` and the
  other modal kinds) is open, a click, pointer move or key it does not use
  no longer reaches the widget painted beside or beneath it.
- Ctrl/Cmd+Z in a `form` text, textarea or password row undoes back past
  the last keystroke: the row keeps its undo history while it holds focus.
  Before, each keystroke started a fresh history, so undo had nothing to
  restore.
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

- A layout-tree render keeps the stored state of a subtree that is not
  painted this frame -- an unselected `"tabs"` entry, a `"scroll"` child
  outside the viewport -- instead of dropping it, so switching tabs away
  and back restores a tree's cursor, a nested tab's active index and a
  split's ratio.

- `showOverlay` decodes through binding-core's `decode_overlay`, shared
  with `quadraui-py`; kinds, descriptor shapes, anchors, returned ids and
  `InvalidDescriptorError` for a malformed call are unchanged. The
  `"context_menu"` and `"message_dialog"` descriptors are now typed in
  `wire.d.ts` (`ContextMenuOverlayDescriptor`,
  `MessageDialogOverlayDescriptor`).
