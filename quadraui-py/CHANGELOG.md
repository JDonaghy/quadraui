# Changelog

All notable changes to the `quadraui` Python package (`quadraui-py`) are
documented in this file. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

This file starts now, not retroactively — `quadraui-py` has history
predating it (see `git log -- quadraui-py`); none of it is backfilled here.
`[Unreleased]` is where changelog discipline begins for this package.

## [Unreleased]

### Added

- A `"tabs"` layout-node dict accepts an optional `"focused_index"` key
  naming the keyboard-focused tab (B2d). TUI paints it in reverse video;
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
