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

- A `"cell_grid"` extra's `CellGridController` events now reach the
  registered handler as `{"CellGridController": [id, event]}`, mirroring
  `{"TreeTableController": [id, event]}`. `CellsRequested` (the
  two-axis-virtualisation window request) needs no new host call to
  answer -- re-render with that `"cell_grid"` extra's `cells`/
  `loaded_row_start`/`loaded_col_start` covering the named block, same as
  any other extra's per-frame data.
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
