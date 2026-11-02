# Changelog

All notable changes to the `quadraui` Python package (`quadraui-py`) are
documented in this file. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

This file starts now, not retroactively — `quadraui-py` has history
predating it (see `git log -- quadraui-py`); none of it is backfilled here.
`[Unreleased]` is where changelog discipline begins for this package.

## [Unreleased]

### Added

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
