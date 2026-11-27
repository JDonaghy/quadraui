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

- `QuiApp.listCharWidth()` and `QuiApp.scrollbarReserve()` (#2979, 0.12
  rec 14), part of the 0.12 binding API cleanup. Budget row text with
  `charWidth()` or `measureText()` instead.

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
