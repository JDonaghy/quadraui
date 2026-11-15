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
