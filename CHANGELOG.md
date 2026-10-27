# Changelog

All notable changes to `quadraui` are documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project intends to follow [Semantic Versioning](https://semver.org/)
once it has a first tagged release — see the *Versioning* section below for
what that means in practice pre-1.0.

`quadraui` is a single-crate workspace by publication surface: `kubeui*` are
in-tree demo apps, never published, and are not covered by this file.
Everything here is about the `quadraui` crate itself (`quadraui/Cargo.toml`).

## Scope: this file starts now, not retroactively

quadraui has 1000+ commits of history predating this file (see `git log`);
none of it is backfilled here. `[Unreleased]` below is where changelog
discipline begins — every PR from this point forward that changes `quadraui`'s
public API, in the sense `quadraui/docs/PRIMITIVE_RULES.md` rule 8 defines,
adds an entry here in the same PR. A public-API PR that skips this file is
incomplete for the same reason rule 8 already treats a missing
`## Downstream impact` section as incomplete — this file's `Deprecated` /
`Removed` sections are rule 8's two-PR protocol made visible outside a diff:

- **PR 1 of a rule-8 change** (new shape added, old one kept behind
  `#[deprecated]`) adds an entry under `### Deprecated` naming the old item,
  its replacement, and the tracking issue for PR 2.
- **PR 2** (shim deleted) moves that same line to `### Removed` and closes the
  loop — same wording, one section down, so the deprecation's full lifecycle
  reads as two adjacent lines rather than being lost across two disconnected
  PRs.
- A non-breaking addition (new primitive, new optional field, new `Backend`
  method with no default per rule 7) goes under `### Added`.
- A behavior fix with no API shape change goes under `### Fixed`.

See `quadraui/docs/PRIMITIVE_RULES.md` rule 8 for the full lifecycle policy
(what counts as breaking, the deprecation-shim patterns, the two-PR split)
and `CLAUDE.md`'s *Downstream consumers* section for why any of this matters
to `coord-tui` and `vimcode`.

## Versioning

Pre-1.0, per Cargo's own semver convention: a `0.MINOR.PATCH` bump treats
`MINOR` the way `1.0`+ treats `MAJOR` (breaking), and `PATCH` the way `1.0`+
treats `MINOR`/`PATCH` combined (additive or fix). A rule-8 breaking change
bumps `MINOR`; everything else bumps `PATCH`.

A release is a reviewed, CI-green `develop` -> `main` promotion PR (see
`CLAUDE.md`'s *Branching + releases* section), assembled and merged by the
coordinator, not something an individual feature PR does — a feature PR
only ever adds its own entry under `[Unreleased]`. The promotion PR bumps
`quadraui/Cargo.toml`'s version and retitles `[Unreleased]` to
`[X.Y.Z] - YYYY-MM-DD` (adding a fresh empty `[Unreleased]` above it).
Merging that PR to `main` triggers `.github/workflows/release.yml`, which
publishes to crates.io via Trusted Publishing once a reviewer approves the
`release` environment, then creates the GitHub release from the `[X.Y.Z]`
section. A hand-pushed `vX.Y.Z` tag on `develop` no longer does anything —
only a push to `main` runs this workflow.

## [Unreleased]
### Added

- `WindowControl::is_fullscreen` — queries the window's current fullscreen
  state, alongside the existing `is_maximized`, so a backend-neutral caller
  can read both chrome flags without tracking its own shadow state.
- Windows event producers for `UiEvent::WindowStateChanged`,
  `UiEvent::SystemThemeChanged` and `UiEvent::DisplaysChanged` — previously
  declared variants with zero producers on any backend. `win::run`'s
  `wndproc` now dispatches `WindowStateChanged` off `WM_SIZE`'s
  `SIZE_MAXIMIZED`/`SIZE_MINIMIZED` transitions (deduplicated against the
  window's previous tri-state so an ordinary resize drag doesn't re-fire
  it), `SystemThemeChanged` off `WM_SETTINGCHANGE` filtered to the
  `ImmersiveColorSet` setting name, and `DisplaysChanged` off
  `WM_DISPLAYCHANGE`. GTK and macOS producers are tracked separately.
- `PlatformServices::update_service` — an auto-update seam. The new
  `UpdateService` trait (`check_for_update`, `download_and_stage`,
  `apply_on_quit`) plus its `UpdateFeed`/`UpdateInfo`/`StagedUpdate` types
  give every backend a contract to implement an updater against, without
  requiring one yet: the default `UpdateService` returns
  `BackendError::Unsupported` from every method. The TUI backend overrides
  `download_and_stage` to emit an `"update available: <version>"`
  diagnostic through `quadraui::diagnostics` before degrading to the same
  `Unsupported`, since a terminal app has no package format of its own to
  stage. `UpdateInfo::builder` offers a named-field alternative to `new`'s
  four same-typed positional arguments. Sparkle/WinSparkle/AppImage
  implementations are separate follow-ons.
- `PlatformServices::on_battery`/`thermal_state` and
  `begin_power_save_block`/`end_power_save_block` — a power-monitor and
  sleep-blocker seam, plus the new `UiEvent::PowerStateChanged` event (not
  yet emitted by any backend) and the `power` module's `PowerSaveKind`/
  `ThermalState`/`BlockerId`/`PowerEventKind` types. All four methods
  default to `BackendError::Unsupported`. The TUI backend overrides the
  blocker pair with an honest no-op implementation — it mints and tracks a
  real `BlockerId` so a begin/end lifecycle behaves consistently, it just
  never asks the OS to stay awake — while leaving `on_battery`/
  `thermal_state` at the default, since a terminal has no battery/thermal
  signal of its own. IOKit/Win32/`org.freedesktop.login1` implementations
  are separate, backend-specific follow-ons.
- `FieldKind::Dropdown` TUI popup: the TUI backend now paints an in-canvas
  option list under a focused `Dropdown` field, instead of leaving the
  selected value and the rest of the options invisible between Enter
  presses. No new fields on `FieldKind::Dropdown` — `focused` (derived from
  the existing `Form::focused_field`/`Form::has_focus`) is the sole
  visibility signal, matching the GUI backends' existing popup. `Up`/`Down`
  live-commit `selected_idx` on every keystroke via `FormEvent::DropdownChanged`,
  with no separate open/close step; `FormController::handle_dropdown_key`
  routes them, falling back to the generic Tab/BackTab handling other fields
  share once the first/last option is reached. Covered by `tui::form`'s
  paint tests, `compose::form_controller`'s routing tests, and a dedicated
  `examples/tui_form_dropdown.rs` demo (`examples/common/form_dropdown.rs`)
  with `TuiDriver` end-to-end tests.
- `FieldKind::ComboBox`: a searchable / type-to-filter combobox form field,
  wrapping the existing `ComboBox` primitive. `FormEvent::ComboBoxTextChanged`
  / `ComboBoxHighlightChanged` / `ComboBoxCommitted` / `ComboBoxCancelled`
  surface its edits through the same `FormController`/`FormEvent` dispatch
  every other field kind uses; `FormController::handle_combo_box_key` routes
  typed characters, arrow-key highlight movement, and commit/cancel to the
  focused field's embedded `ComboBox::handle_key`, falling back to the
  generic Tab/BackTab/Escape handling other fields already share. TUI paints
  the compact field row (live filter text + chevron) via a new
  `src/tui/form.rs` arm; GTK renders it through the existing
  backend-generic `primitives::form::paint` path with no GTK-specific code
  needed. Both backends are covered by `examples/common/form_all_fields.rs`
  and its cross-backend parity/driver tests. A dedicated
  `examples/tui_form_combo_box.rs` demo (`examples/common/form_combo_box.rs`)
  shows the full apply contract end to end — the field row paints only the
  compact filter text, the app itself paints the open dropdown overlay via
  `Backend::draw_combo_box`, mirroring `FieldKind::ColorPicker`'s popup
  contract — covered by `TuiDriver` tests exercising type-to-filter,
  highlight navigation, commit, cancel, and quit routing.
- `Form`/`FormField` label placement and explicit tab order: a new
  `LabelPosition` (`Left`/`Top`/`Inline`/`Hidden`) and `LabelAlign`
  (`Start`/`Center`/`End`) enum, settable per-form (`Form::label_position`,
  `Form::label_align`, `Form::label_width`) and overridden per-field
  (`FormField::label_position`, `FormField::label_align`,
  `FormField::label_width`), resolved via `Form::resolved_label_position` /
  `resolved_label_align` / `resolved_label_width`. `FormField::tab_index`
  lets a form declare a custom tab order, consulted by
  `FormController::move_focus`; fields that leave it unset keep their
  declaration order. The TUI backend renders all four label positions:
  `Left` aligns every such field's label into one shared column sized to
  the widest label (or an explicit `label_width` override), `Top` paints
  the label on its own row above the input, `Hidden` suppresses the label
  entirely, and `Inline` keeps each row's own per-label width unshared.
  All fields are additive with `serde` defaults, so a form that never sets
  them deserializes and reads exactly as before. The TUI paint path does
  shift: every field that resolves to the default `Left` position now
  shares one label column sized to the widest such label, so an existing
  multi-field form's inputs (including `ToggleGroup`/`ButtonRow`/
  `Toolbar`/`SegmentedControl` item columns) can move right to line up
  with that shared column. `GTK`/`macOS`/`Win` compile against the new
  fields but don't yet have their own rasteriser support for them.
- `quadraui::expr`: a serializable, binding-neutral expression AST (`Expr`,
  `Value`, `Context`) for form rules (conditional visibility, validation).
  Field references, string/number/bool/null literals, comparisons
  (`==`/`!=`/`<`/`<=`/`>`/`>=`), membership (`in`), `present`/`empty`,
  boolean combinators (`and`/`or`/`not`), and a regex `matches` test — no
  function calls, loops, or side effects. `Expr::eval`/`eval_bool` are
  total (never panic or error); a dangling field reference or malformed
  regex degrades to a safe default instead of failing. The `Expr`/`Value`
  enums serialize with serde's default representation unmodified, so that
  *is* the wire shape every language binding (Lua, Python, a future
  C-ABI) emits.
- `quadraui::validation`: declarative field/form validation rules —
  `Validator` built-ins (`Required`, `Pattern`, `Min`/`Max`,
  `MinLength`/`MaxLength`, `OneOf`) plus cross-field rules via
  `Validator::Expr(quadraui::expr::Expr)`, grouped per field into a
  serializable `FormRules` (`BTreeMap<String, Vec<Rule>>` keyed by
  `WidgetId::as_str()`) and applied with `validate(&mut Form, &FormRules)`.
  Same binding-neutral rationale as `quadraui::expr`: rules are data, so
  every language binding gets identical `Error`/`Warning`
  `FormField::validation` state with no host-side validation logic.
  `FormController::set_rules` attaches a `FormRules` that
  `FormController::set_form` evaluates automatically on every pushed
  `Form`; fields with no declared rules are left untouched, so the
  existing imperative `ValidationState` escape hatch still works unchanged.
- `quadraui::visibility`: declarative `visible_when`/`enabled_when`
  conditions per field, evaluated with `quadraui::expr` against the same
  `Context` `quadraui::validation` builds. `FieldVisibility` holds each
  field's optional `visible_when` (removes the field from the rendered
  `Form` when false) and `enabled_when` (drives `FormField::disabled`),
  grouped per field into a serializable `FormVisibility`
  (`BTreeMap<String, FieldVisibility>` keyed by `WidgetId::as_str()`) and
  applied with `apply_visibility(&mut Form, &FormVisibility)`. Binding-
  neutral for the same reason as `quadraui::expr`/`quadraui::validation`:
  conditions are plain serializable data, not a host callback.
  `FormController::set_visibility` attaches a `FormVisibility` that
  `FormController::set_form` evaluates automatically on every pushed
  `Form`, after validation and before the field list is rendered, so a
  field hides/shows or enables/disables declaratively with no per-keystroke
  host round-trip; fields with no declared rule are left untouched.

### Fixed

- `FormController::handle_cached`'s backend-free fallback layout (used when
  no live `Backend` is attached) now gives `FieldKind::Toolbar` fields the
  same per-button hit regions the pixel backends and TUI already compute: it
  was missing a `Toolbar` arm entirely, so every click anywhere in a toolbar
  row resolved to the field's own id (`FormEvent::FocusChanged`) instead of
  the specific button clicked (`FormEvent::ToolbarButtonClicked`).

## [0.6.1] - 2026-10-26
### Added

- `Style::backdrop_opacity` (default `1.0`, fully opaque) lets `Palette`
  render a translucent backdrop independent of the shared `Theme.surface_bg`
  every other overlay (`Toast`/`Tooltip`/`ContextMenu`/`Dialog`/`Spinner`/
  `ProgressBar`) reads — only `Palette`'s own paint path consults it, so no
  other overlay is affected. Pixel backends (GTK/macOS/Win) apply it via
  `Color::with_alpha` on the existing backdrop fill; the TUI backend has no
  compositing primitive, so it pre-blends
  `theme.background.blend(theme.surface_bg, backdrop_opacity)` on the CPU
  before writing opaque cells, the same technique already used for the
  minimap's translucent highlight tint.

### Fixed

- Pixel backends' translucent `Palette` backdrop (`backdrop_opacity < 1.0`)
  no longer double-composites: `native_surface_paint::paint` filled the
  backdrop twice (once square, once rounded) over the same area, so two
  sequential alpha blends at `a` compounded to an effective `1 - (1-a)²`
  instead of `a`. The square pre-fill is now skipped whenever the backdrop
  is translucent; it still runs at the opaque default so corner pixels
  outside the rounded fill stay covered.
- `tui::draw_palette`'s public signature stays style-less (D-020): the live
  `&Style` (including `backdrop_opacity`) now reaches the paint through a
  `pub(crate) draw_palette_with_style` sibling `Backend::draw_palette`
  calls, the same shim shape `win::dialog::draw_dialog_with_style` already
  uses, instead of widening the public free function's argument list.
- `tui::draw_solid_fill` now honours `color.a`: a translucent colour
  (`a < 255`) alpha-blends over each cell's existing background and leaves
  its glyph and foreground untouched, instead of blanking the cell to a
  solid opaque colour. This matches the pixel backends' `surface_fill_rect`,
  which already honour `a` — a translucent modal scrim (e.g. the fuzzy
  finder backdrop) now dims the TUI content behind it instead of blacking
  it out. `a == 255` keeps the prior blank-fill behaviour. A cell whose
  existing background isn't a literal RGB colour (e.g. a buffer cell this
  crate never painted) has no base to blend against, so it's left
  unblended.

## [0.6.0] - 2026-10-25
### Added

- `Backend::set_editor_right_reserve_w()`/`editor_right_reserve_w()` (both
  default-provided, no-op/`None`) let a host reserve trailing width in an
  `Editor` area — e.g. a minimap strip — without relocating the vertical
  scrollbar. `TuiBackend` overrides both: the background fill, text,
  selection overlays, cursor (block and secondary), AI ghost text and the
  horizontal-scrollbar track all stop short of the reserved column count,
  while the vertical scrollbar stays pinned to the area's true right edge.
- `tui::draw_editor_with_reserve()` — the paint-side counterpart to
  `Editor::layout_with_options_and_reserve`'s `right_reserve_w`, and what
  `TuiBackend` calls to honour `set_editor_right_reserve_w`. Lets a TUI host
  reserve a fixed-width strip (a minimap, say) between the editor's text and
  its vertical scrollbar without narrowing the whole `area` it passes to
  `draw_editor` — narrowing `area` drags the scrollbar off the viewport's
  true right edge along with the text, which is what this parameter exists
  to avoid. `tui::draw_editor()` keeps its existing signature and delegates
  with `right_reserve_w: 0`, mirroring how
  `layout_with_options_and_v_scrollbar_w()` delegates to
  `layout_with_options_and_reserve()`, so no existing call site changes.
- `MinimapSizing::RepeatToFill(f32)` — a new `Minimap` row-pitch sizing that
  repeat-paints a short sampled-line list across a whole-number multiple of
  the base pitch to visually stretch it across a taller strip, instead of
  `FixedPitch`'s top-align-with-blank-space-below behaviour. `Backend` gains
  `minimap_sizing()`/`set_minimap_sizing()` (both default-provided, mirroring
  `minimap_scale()`/`set_minimap_scale()`) so a host can opt a `Minimap` into
  it without changing any existing call site. `TuiBackend` is the first and
  currently only backend to honour it, via `tui::draw_minimap_with_sizing`/
  `tui::tui_minimap_layout_with_sizing`, since it is the one rasteriser that
  quantises row pitch to whole terminal cells and so is the one that needs a
  whole-number-multiple trick to avoid a fractional, unaddressable pitch.
- `quadraui-py`: `QuiApp.__new__` takes an optional `render` keyword argument,
  and a new `QuiApp.set_render(fn)` method sets one after construction. The
  callback takes no arguments, runs once per frame, and returns a `dict`
  mapping slot names to descriptor dicts (or `None` to clear a slot) — the
  per-frame way for a `set_handler` callback to drive what the next frame
  shows without re-entering `QuiApp` from inside the runner's borrow of it.
  Existing `insert_*`/`run_*` call sites are unaffected.

### Fixed

- `quadraui-py`: a Python app's descriptor bands now lay out correctly in a
  native window. `draw_bands` sized each band in character-cell counts while
  `Backend::viewport()` reports device pixels on the native backends, so every
  app collapsed into a ~17px strip at the top of its macOS/GTK/Win window. Band
  heights are now scaled by `Backend::line_height()` (1.0 on TUI, the font's
  pixel line height on native), so a band is N text lines tall on every
  backend; the TUI/headless path is byte-for-byte unchanged.
- `MinimapSizing::RepeatToFill` no longer leaves a blank gap below the last
  row when the strip height isn't an exact multiple of `base_pitch *
  row_count`: instead of every row sharing one floor-ed repeat count, the
  whole-unit remainder is now spread across the first few rows (one extra
  repeat each), so the painted rows sum to the strip's height exactly
  whenever a full-row-count display is possible at all.

## [0.5.1] - 2026-10-24
### Added

- `gtk::display_available()` — a pre-flight check (`gtk4::init().is_ok()`,
  with a macOS main-thread guard) that an embedder can call before
  `GtkRunner::new`/`new_with` to detect "no display reachable" up front,
  instead of GTK's own init path killing the whole process with
  `exit(1)` and no catchable error.
- `GtkRunner::request_exit()` — destroys every window the running
  `gio::Application` owns, letting an embedder (or a signal handler) tear
  the window down from outside the normal close-request path.
- `gtk::app_id_is_valid(&str) -> bool` — wraps `g_application_id_is_valid`
  so an embedder can check a caller-supplied `app_id` before handing it to
  `RunConfig`/`GtkRunner::new_with`, instead of an invalid id tripping an
  uncatchable `GLib-GIO-CRITICAL` once it reaches `Application::builder()`.
  `quadraui-py`'s `run_native()` now raises a catchable `InvalidAppIdError`
  for this case instead.
- `Editor::layout_with_options_and_reserve()` adds a `right_reserve_w`
  parameter that shrinks the text area (and the horizontal scrollbar it
  derives from) without moving the vertical scrollbar off the viewport's
  true right edge, so a host can dock a fixed-width panel — a minimap
  strip, say — between the buffer text and the scrollbar.
  `layout_with_options_and_v_scrollbar_w()` is unchanged and now delegates
  to it with `right_reserve_w: 0.0`.

### Fixed

- `run_native()`'s Ctrl-C path now runs one more non-blocking pump after
  `GtkRunner::request_exit()` before returning, so the window's `destroy()`
  actually gets processed instead of being left for an already-abandoned
  `MainContext`.

## [0.5.0] - 2026-10-23
### Added

- `FieldKind::DatePicker(DatePicker)` embeds the full `DatePicker` primitive
  as a `Form` field, and `FormEvent::DateChanged { id, value: CalendarDate }`
  fires on confirmation. `FormController` routes `Left`/`Right`/`Up`/`Down`/
  `PageUp`/`PageDown`/`Enter` into the embedded picker's own `navigate` while
  it has focus — `Tab`/`BackTab` still traverse focus normally — and caches
  the in-flight focused day/visible month the same way it already caches a
  text field's cursor, so scrubbing composes across keystrokes even when the
  host app never echoes the uncommitted position back. The TUI renderer
  shows the field's focused/selected date as a single right-aligned line.

- `quadraui-ffi`'s C ABI is hardened for a real consumer: every `extern "C"`
  entry point catches a Rust panic at the boundary (reported as
  `QUI_ERR_PANIC`, after which the handle is poisoned and every call but
  `qui_app_free` returns `QUI_ERR_POISONED`); a documented 13-code error
  taxonomy (`QUI_ERR_NULL_HANDLE` … `QUI_ERR_INTERNAL`) replaces the flat
  `-1`, with `qui_last_error_code` and `qui_error_name` alongside
  `qui_last_error`; the header now exports every code and `QUI_SLOT_*` id
  as a `#define`, inserts return the slot id they filled, and
  `qui_slot_clear` empties one again. `qui_screen` hands out a freeable
  frame dump, and the ownership rule for every pointer the ABI passes is
  documented on the header. `smoke/smoke.c` is a worked C consumer that
  builds a six-primitive screen, drives the key/mouse/resize paths through
  a registered vtable, walks every error path and frees every
  callee-allocated string.

- The macOS native painter honors the chrome-glyph override registries at
  all four sites that have one: `MacBackend::draw_tab_bar_with_chrome_and_glyphs`
  (close/dirty marks) and `MacBackend::draw_context_menu_with_glyphs`
  (checked mark, submenu pull-right arrow) now paint the app-supplied
  `Icon` in place of the default codicon when one is set, and fall back
  to today's codicon glyph when `glyphs` is `None`. `draw_data_table_with_glyphs`
  (sort asc/desc) and the tree expand/collapse chevron already honored
  their overrides through the shared `native_surface_paint`/`Backend`
  paths, so those two sites needed only a confirming test. Each of the
  four sites has a black-box test, using the crate's text-run recording
  harness (`MacBackend::set_painted_text_recording`/`text_runs`), proving
  the override glyph paints and `None` reproduces the current default
  exactly.

### Fixed

- `DatePicker::layout()`'s `header_bounds` now spans only the middle 5 of 7
  columns instead of the full header row, so the month/year label no longer
  shares pixels with `prev_bounds`/`next_bounds`. The overlap previously
  corrupted the label's leading character wherever a rasteriser painted the
  header and the prev/next controls in sequence (`tui::date_picker` and the
  shared `primitives::date_picker::native_surface_paint::paint` fallback used
  by GTK/Win/macOS).


## [0.4.0] - 2026-10-22
### Added

- `Backend::draw_date_picker`/`date_picker_layout` (no default body, per
  `PRIMITIVE_RULES.md` rule 7) paint and measure `DatePicker`'s month grid
  and time field. Implemented on every backend (`TuiBackend`, `GtkBackend`,
  `WinBackend`, plus the crate's test-only `MockBackend`/`RecordingBackend`
  mocks) via the shared `primitives::date_picker::native_surface_paint::paint`
  helper or a dedicated `tui::date_picker` rasteriser; `MacBackend` is tracked
  separately under #1453.
- `Backend::draw_date_picker(&mut self, date_picker: &DatePicker, layout: &DatePickerLayout)`
  rasterises `DatePicker`'s month grid (header, prev/next controls, 42-day
  grid with focus/selection styling) and optional time field. TUI
  (`tui::date_picker`) has its own cell-grid rasteriser; GTK, macOS and
  Win-GUI all paint through the shared
  `primitives::date_picker::native_surface_paint::paint`, each reached
  through its own `Backend::draw_date_picker` impl. New trait method
  with no default, per `PRIMITIVE_RULES.md` rule 7 — no hits in either
  downstream consumer's source tree, so this is additive, not breaking.
- `SelectionMode::Multi` is wired into GTK's `List`, `Tree` and
  `DataTable` event handling: a plain click replaces the selection,
  Ctrl/Cmd-click toggles the clicked row, and Shift-click selects the
  range from the last anchor, via the shared
  `types::apply_selection_gesture` seam. Covered by `GtkDriver`-based
  tests in each primitive's own module.
- Windows rasteriser for `DatePicker` (`quadraui::win::date_picker`): same
  shape as the GTK rasteriser above — `Backend::draw_date_picker` delegates
  entirely to the shared `primitives::date_picker::native_surface_paint::paint`,
  so this module carries no production code of its own; keyboard
  navigation (arrow keys, page up/down, enter-to-select) is covered by a
  `WinDriver` test. `SelectionMode::Multi`'s Ctrl-click/Shift-range wiring,
  already exercised on GTK above, is additionally covered by new
  `WinDriver`-based tests in `win::list`, `win::tree` and `win::data_table`,
  confirming the same `types::apply_selection_gesture` seam round-trips on
  Windows. The gallery demo is deferred to the `DatePicker` gallery-page
  issue.
- TUI delivery of `DatePicker` keyboard navigation and `SelectionMode::Multi`:
  the `tui::date_picker` cell-grid rasteriser is driven keyboard-only (arrow
  keys move within the month grid, page up/down change month, enter selects),
  and List/Tree/DataTable gain keyboard multi-select since TUI has no mouse —
  a plain Up/Down only moves the cursor, Shift+Up/Down extends a range from
  the anchor via `SelectionGesture::from_modifiers`, and Space toggles the
  cursor row without moving it, all through the shared
  `types::apply_selection_gesture` seam. Verified by `TuiDriver`-based tests
  in `tui::list`, `tui::tree` and `tui::data_table`.
- macOS rasteriser module for `DatePicker` (`quadraui::macos::date_picker`):
  the macOS twin of the Win-GUI module above, carrying no production code of
  its own — `MacBackend`'s `Backend::draw_date_picker` delegates entirely to
  the shared `primitives::date_picker::native_surface_paint::paint`. Keyboard
  navigation (arrow keys, page up/down, enter-to-select), the painted
  header/prev/next/day glyphs and the `theme.selected_bg` fill on the
  selected day's cell are covered by `MacDriver` tests. `SelectionMode::Multi`'s
  plain-click/Ctrl-or-Cmd-click/Shift-range wiring is additionally covered by
  new `MacDriver`-based tests in `macos::list`, `macos::tree` and
  `macos::data_table`, confirming the same `types::apply_selection_gesture`
  seam round-trips on macOS. The gallery demo is deferred to the `DatePicker`
  gallery-page issue.
- `ScreenLayout::tree_frame` returns each pushed `Surface::Tree`'s geometry
  as `(Handle<TreeView>, Rect)` pairs, backed internally by
  `DescriptorStore<TreeView>`. On a retained `ScreenLayout` kept across
  frames, a tree whose `WidgetId` was already seen (e.g. replaced in place
  via `surfaces_mut`) reuses its existing handle and store slot instead of
  being re-inserted, and an id dropped from one call to the next has its
  handle and slot evicted rather than leaked. A fresh `ScreenLayout::new()`
  built every frame still inserts every tree from an empty store each call.
  Internal and additive — `Surface`'s shape and `ScreenLayout::push`'s
  signature are unchanged, and no in-crate caller consumes this yet.
- App-facing chrome-glyph override hook for context menus, tab bars and
  data tables: three `#[non_exhaustive]` + `Default` registries
  (`primitives::context_menu::ContextMenuGlyphs`,
  `primitives::tab_bar::TabBarGlyphs`,
  `primitives::data_table::DataTableGlyphs`) each hold optional `Icon`
  overrides, and three new default-delegating `Backend` trait methods
  (`draw_context_menu_with_glyphs`, `draw_tab_bar_with_chrome_and_glyphs`,
  `draw_data_table_with_glyphs`) accept them, falling back to the
  existing codicon-based method when an app passes the registry's
  `Default`. TUI rasterisers resolve each override via `Icon::resolve`
  and paint its first `char`, except `DataTableGlyphs`'s sort-direction
  glyphs, which splice the whole resolved string into the column-header
  title; `Icon::color` is resolved but never painted by any of the three
  TUI paths. Covered by a black-box paint/screen-text test per site plus
  a `c0` conformance entry per new trait method. Additive — no hits for
  any of the six new symbol names in either downstream consumer's source
  tree.
- GTK native painter honours the chrome-glyph overrides above:
  `GtkBackend` overrides `draw_tab_bar_with_chrome_and_glyphs`,
  `draw_data_table_with_glyphs` and `draw_context_menu_with_glyphs`
  instead of inheriting the glyph-ignoring trait defaults, so a
  `TabBarGlyphs` close/dirty override, a `DataTableGlyphs` sort
  ascending/descending override and a `ContextMenuGlyphs` checked /
  submenu-arrow override all reach the Cairo paint. Each override
  `Icon` is resolved through `Icon::resolve` against
  `Backend::nerd_fonts_enabled`, and `None` (or a `Default` registry
  with no field set) paints byte-identical to today's codicon
  defaults. Three new free functions carry the extra arguments
  alongside the existing ones rather than changing them:
  `gtk::draw_data_table_with_glyphs`,
  `gtk::draw_tab_bar_with_chrome_and_glyphs` and
  `gtk::draw_tab_bar_icons_with_chrome_and_glyphs`. Covered by a
  Cairo `ImageSurface` pixel-comparison test per site, asserting an
  empty override is byte-identical to `None` and a populated one is
  not. Additive — no hits
  for any of the three new function names in either downstream
  consumer's source tree.
- GTK scroll translation (`gtk::events::gdk_scroll_to_uievent` and
  `gdk_scroll_to_uievent_with_direction`) documents and regression-pins
  real wheel/trackpad delta magnitude as a guarantee: GDK's `dx`/`dy`
  reach `ScrollDelta` with only the natural-scroll sign convention and
  an `f64` → `f32` cast applied, never reduced to ±1, so a sub-unit
  smooth trackpad delta stays sub-row and a multi-line wheel notch keeps
  its full magnitude. No behavior change — the pass-through was already
  what the GTK event path did; it is now stated in the public docs and
  covered by tests for both the discrete and smooth paths rather than
  holding only by absence of a bug.
- The Windows native painter honors the three chrome-glyph override
  registries: `Backend::draw_data_table_with_glyphs` (sort asc/desc),
  `Backend::draw_tab_bar_with_chrome_and_glyphs` (close/dirty marks) and
  `Backend::draw_context_menu_with_glyphs` (checked mark, submenu
  pull-right arrow) now paint the app-supplied `Icon` in place of the
  default codicon when one is set, and fall back to today's codicon glyph
  when `glyphs` is `None`. The tree expand/collapse chevron already honors
  a `TreeStyle` override glyph through the shared
  `primitives::tree::native_surface_paint::paint` path, so that site needed
  no production change. Each registry has a black-box test confirming the
  override glyph paints and `None` leaves the default codicon unchanged,
  driven through `WinBackend`'s `Backend` trait method; data table and tab
  bar each also keep a module-level paint-function test covering the same
  override.

### Changed

- macOS `scrollWheel:` handling distinguishes trackpad ("precise") deltas
  from discrete mouse-wheel notches via
  `NSEvent::hasPreciseScrollingDeltas` (the routing decision is a pure,
  unit-tested `route_wheel_event` in `macos::events`). A trackpad gesture
  still dispatches AppKit's own decaying `scrollWheel:` stream straight
  through (already proportional and sub-row). A discrete wheel notch,
  which AppKit delivers as a single call with no native momentum, glides
  out over ~220ms via `Transition` and a cubic ease-out instead of
  landing as one lump, so external-mouse scrolling animates smoothly; a
  fast run of notches folds each new one into the in-flight glide rather
  than stuttering or queuing. Each tick dispatches only the whole-unit
  increment since the previous tick (the cumulative eased target is
  rounded before diffing), so the glide's total delivered delta always
  sums to exactly one un-split notch — a row-index consumer like
  `primitives::scrollbar::wheel_row_delta`, which rounds any nonzero
  fractional delta up to at least one row, no longer sees ~14 small
  dispatches inflate a single notch into ~14 rows of over-scroll.
### Fixed

- `DatePicker`'s GTK/native rasteriser
  (`primitives::date_picker::native_surface_paint::paint`) centers the
  `YYYY-MM` header label between the prev/next controls instead of
  left-aligning it at the header's origin, where it collided with the
  `<` glyph. `tui::date_picker`'s day-cell focus ring and time-field
  border no longer overwrite the digits/text they decorate: both now go
  through `fill_text_with_ring`, which insets the text by one column so
  the ring's corner glyphs land on blank cells, falling back to reverse
  video when a cell is too narrow to spare the inset columns.
- Win-GUI `WM_MOUSEWHEEL`/`WM_MOUSEHWHEEL` handling forwards every
  message's delta immediately and in full, with no per-axis accumulation
  or gating on a full `WHEEL_DELTA` notch: a high-resolution wheel or
  trackpad message smaller than one notch now dispatches its own
  proportional sub-row `UiEvent::Scroll` rather than being withheld until
  several messages sum to a full notch. This matches the GTK
  (`gdk_scroll_to_uievent`/`gdk_scroll_to_uievent_with_direction`, the
  entry above) and macOS (`macos::events::ns_scroll`) translators, which
  likewise forward the native delta with only a sign/precision cast, so a
  sub-unit smooth-trackpad delta stays sub-row on every backend rather
  than being notch-quantised on Windows alone. Rounding a nonzero delta up
  to a full row for row-based consumers (`primitives::scrollbar`'s
  `wheel_row_delta`) is a decision the shared layer makes once for every
  backend; this translator no longer pre-empts it by withholding events.

## [0.3.0] - 2026-10-20
### Changed

- `Surface` no longer has a lifetime parameter: every variant carries its
  primitive descriptor by value (`Surface::StatusBar { bar: StatusBar, .. }`
  rather than `bar: &'a StatusBar`), including the six overlay variants'
  companion `*Layout` values, and `hovered`/`pressed` take
  `Option<WidgetId>` instead of `Option<&WidgetId>`. `ScreenLayout` loses
  its lifetime parameter accordingly, so a `ScreenLayout` can be stored in
  a struct, returned from a function, or built well ahead of the frame that
  draws it, rather than being pinned to the scope holding the borrowed
  descriptors.

  **Breaking, authorized for the v0.2 batch.** A lifetime removal admits
  no deprecation shim — `Surface<'a>` and `Surface` cannot coexist under
  one name — so the two-PR deprecation protocol (`CLAUDE.md` *Downstream
  consumers* rule 3, `docs/PRIMITIVE_RULES.md` rule 8) does not apply
  here. The caller
  migration is mechanical: drop the `&` on each `Surface` field, cloning
  the primitive only where the caller still needs it after the push, and
  pass `Option<WidgetId>` instead of `Option<&WidgetId>` for `hovered` and
  `pressed`. Every in-tree call site (`examples/hello.rs`, the shared
  `examples/common/` demos, `templates/app`) was a move, not a clone.
- `A11yInfo`'s role is now typed instead of a free string: a new
  `#[non_exhaustive]` `A11yRole` enum covers window, toolbar, button,
  tab/tab list, menu/menu item, tree/tree item, list/list item,
  table/row/cell, text input, document/editor, terminal, dialog, status,
  progress, image, group, and presentation/none, each mapping 1:1 onto
  `accesskit::Role`. `A11yInfo` also gained typed state builders:
  `with_selected`, `with_expanded`, `with_checked` (new tri-state
  `A11yChecked`), `with_disabled`, `with_read_only`, `with_value`,
  `with_value_range` (new `A11yValueRange`), `with_level`,
  `with_position_in_set`, `with_set_size`, and `with_live` (new
  `A11yLive`, mapping onto `accesskit::Live`). The data model
  (`A11yRole`, `A11yInfo`, and friends) is always compiled; a new
  optional `a11y` feature only gates the `accesskit`-facing `From`
  conversions, so a descriptor embedding `A11yInfo` never changes shape
  depending on which backend features a consumer enables. This is a
  breaking change to `A11yInfo::with_role`, which now takes `A11yRole`
  instead of `impl Into<String>`; no in-tree or downstream consumer used
  it. `A11yInfo` also drops its `Eq` derive (kept only `PartialEq`),
  forced by `A11yValueRange`'s `f64` fields; its sole in-tree holder
  (`OwnedNode.a11y` in `frame.rs`) never derived `Eq`/`Hash` either, so
  this has no in-tree effect, but a downstream consumer that relied on
  `A11yInfo: Eq` would need to drop that bound. `A11yRole`, `A11yChecked`
  and `A11yLive` serialise as snake_case strings (e.g. `"text_input"`)
  via `#[serde(rename_all = "snake_case")]` rather than serde's PascalCase
  default.
- Compose controllers consume the signed magnitude of `ScrollDelta`
  instead of reducing a wheel event to its sign:
  `TreeController`, `FormController` and `SidebarSystem` move
  `wheel_row_delta(delta.y, …)` rows per event, and `ChatController`
  moves three rows per notch scaled by the delta. A sub-row trackpad
  gesture still moves at least one row rather than being rounded away.
  `SidebarSystem`'s `ScrollMode::WholePanel` path scrolls by
  `delta.y * line_height` pixels, so sub-line trackpad deltas produce
  sub-line panel offsets.
### Added

- Continuous-scroll helpers in `primitives::scrollbar`, re-exported at
  the crate root: `scroll_rows` (round an `f32` offset to the whole row
  a row-indexing backend must paint from, clamping negatives and
  non-finite values to `0`), `scroll_by_clamped_f32` and
  `clamp_scroll_offset_f32` (the `f32` counterparts of the existing
  `usize` clamps), `wheel_delta` (a signed `ScrollDelta::y` scaled into
  native units) and `wheel_row_delta` (the same scaled into whole rows,
  never swallowing a sub-row gesture).

- `DatePicker` primitive (`primitives/date_picker.rs`) — a declarative
  month-grid calendar: `CalendarDate`/`Weekday`/`DateFormat`/`DateLocale`
  for locale-aware first-weekday and ISO-8601/US/EU date formatting, an
  optional `TimeOfDay` field, `navigate` for arrow/page/enter keyboard
  handling, and `layout`/`hit_test` for pointer routing. Pure primitive
  logic only — no `Backend::draw_date_picker` method yet; that is
  follow-on, per-backend work.
- `SelectionMode::Multi` support: `SelectionGesture`
  (`Replace`/`Toggle`/`Range`) and `apply_selection_gesture` in
  `types.rs` give `List`, `Tree` and `DataTable` a shared, pure
  multi-selection helper driven by Ctrl/Cmd-click, Shift-range and
  keyboard extension. Each of `ListViewEvent`, `TreeEvent` and
  `DataTableEvent` gains a new `SelectionSet` variant carrying the
  resulting selection; existing `Single`-mode call sites are unaffected.
- `FindReplacePanel`'s GUI toggle buttons (match case, whole word, regex,
  preserve case) now paint codicon glyphs instead of literal `Aa`/`ab`/
  `.*`/`AB` text on every pixel backend (GTK, macOS, Win-GUI) that goes
  through `native_surface_paint`. TUI (`tui::find_replace`) is unaffected
  and keeps painting the plain-text pairs. Additive — no public shape
  changed.

- Accessibility tree builder (`a11y::tree`, re-exported at the crate
  root): `frame_to_tree` turns a captured `OwnedFrame` plus a
  `FocusManager` into an `A11yTree` of `A11yTreeNode`s, with
  `A11yNodeId`s derived from `WidgetId` so a node keeps its id across
  frames and position changes, bounds in logical pixels, and focus
  descending into the focused primitive's own sub-node. `A11yTreeBuilder`
  makes that incremental — the first `update` is the full tree, later
  ones carry only changed nodes as an `A11yUpdate`. Per-primitive
  role/state/children come from the `A11yPrimitive` trait (`A11yChild`);
  `List` and `Toolbar` are mapped fully and every other primitive falls
  back to a labelled group. Behind the optional `a11y` cargo feature the
  same types emit `accesskit` values (`A11yUpdate::to_tree_update`,
  `A11yTreeBuilder::tree_update`). The TUI backend deliberately emits no
  tree — a terminal screen reader reads the terminal itself.
- `testing::a11y::{A11yScenario, A11ySnapshot}` — a headless seam that
  builds a frame from pushed surfaces, applies focus, and returns the
  accessibility tree as a stable assertable snapshot (one
  `role "label" [state]` line per node, bounds deliberately omitted). No
  backend, no window and no `a11y` feature required.
- `OwnedFrame::nodes_mut` and `ScreenLayout::{surfaces, surfaces_mut}` —
  in-place accessors for a captured frame's nodes and a layout's pushed
  surfaces, since `Surface` is neither `Clone` nor `Debug`.
- `Canvas` v2: six new `DrawOp` variants — `Polygon` (even-odd fill),
  `Ellipse`, `Arc`, `Bezier` (cubic, SVG `C`-command contract),
  `DashedLine` and `StrokeRect` — plus `DrawOp::StyledTextRun`, a
  `TextRun` with a per-op `role: CanvasFontRole` (a serde-capable
  mirror of `FontRole`), `italic` flag and `#[serde(default)] size:
  Option<f32>`. `Color::with_alpha` already covers per-op translucency
  for every op that carries a `Color`, so there is no separate alpha
  field. Every new op degrades to the existing sub-cell braille grid on
  TUI (`crate::tui::canvas`), never panics or reports `Unsupported`; the
  new `tui_canvas_shapes` / `gtk_canvas_shapes` example (backed by one
  shared `CanvasShapesApp`, with a `TuiDriver` test in
  `tests/tui_example_driver.rs`) renders a chart-style gauge from the
  full new op set on every backend. `DrawOp` stays `#[non_exhaustive]`.
- `Surface::Canvas` — a pushed `Canvas` primitive now flows through
  `ScreenLayout`/`OwnedFrame` like every other primitive, instead of
  only being paintable via a direct `Backend::draw_canvas` call.
- `macos::MacRunner` and re-exported `macos::StepOutcome` — a non-blocking
  `step()`/`pump(timeout)` entry point for the AppKit backend, mirroring
  the existing TUI/GTK runners: `step()` drains at most one already-queued
  AppKit event without blocking, `pump(timeout)` blocks up to `timeout`
  waiting for the next one, and both return `StepOutcome::Continue` or
  `StepOutcome::Exited` so a host loop (not `NSApplication::run()`) can
  drive the app a slice at a time. The outcome and the redraw sweep are
  scoped to the single window `MacRunner` created, never a host's own
  AppKit windows, and the outcome tracks that window *closing* — latched
  from `windowWillClose:` — rather than its onscreen-ness, so hiding it
  (`WindowControl::hide`, Cmd-H) or minimizing it
  (`WindowControl::minimize`, the yellow traffic light, Cmd-M) keeps
  returning `Continue` and keeps the host's loop pumping the events that
  bring the window back. `MacRunner::new`/`new_with` send
  `[NSApp finishLaunching]` only when the process's `NSApplication` is not
  already running, so an embedding host does not get a second
  `NSApplicationWillFinishLaunching`/`DidFinishLaunching` pair.
  `macos::run`/`run_with` are unchanged convenience
  wrappers around `MacRunner` for the all-owning case. New example:
  `examples/macos_embedded.rs`. Note: `StepOutcome::Exited` is not
  reachable from a quadraui-initiated quit today — both `Reaction::Exit`
  and the native last-window-close path route to `[NSApp terminate:]`,
  which calls `exit()` and ends the host process; see `MacRunner`'s doc
  comment for the caveat.
- `win::WinRunner` and re-exported `win::StepOutcome` — a non-blocking
  `step()`/`pump(timeout)` entry point for the Win32 backend, mirroring
  the existing TUI/GTK/AppKit runners: `step()` dispatches at most one
  already-queued Win32 message without blocking, `pump(timeout)` blocks
  up to `timeout` waiting for the next one, and both return
  `StepOutcome::Continue` or `StepOutcome::Exited` so a host loop (not the
  built-in `run`/`run_with` message pump) can drive the app a slice at a
  time. `WinRunner` is `!Send`/`!Sync` and must be constructed and
  stepped on one thread, since Win32 windows are thread-affine; a
  background thread still reaches a stepped loop through
  `WinBackend::waker`, which nudges the runner's thread without
  busy-polling. `win::run`/`run_with`/`RunConfig` are unchanged
  convenience wrappers around `WinRunner` for the all-owning case.
  `WinRunner` is `#[cfg(target_os = "windows")]`; `StepOutcome`,
  `RunConfig`, `run` and `run_with` compile on every host.
- `win::testing::HeadlessSurface::{stride, buffer}` and
  `win::testing::WinDriver::{screen, stride}` — a whole-buffer readback
  seam for the headless Direct2D DC render target, mirroring
  `gtk::testing::GtkDriver::{screen, stride}`: `screen()` returns the
  full rendered buffer as `[B, G, R, A]` bytes and `stride()` the row
  byte width (`width * 4` for this DIB format), letting a consumer (e.g.
  `quadraui-gallery`'s headless capture tool) PNG-encode a Windows
  capture the same way it already does for GTK.

### Fixed

- `tree_layout_measured` (the shared pixel-layout math `gtk_tree_layout_measured` /
  `win_tree_layout_measured` / `mac_tree_layout` all call) now measures the same
  chevron glyph the GUI paint path actually draws. Both now go through a new
  shared `primitives::tree::tree_chevron_glyph` helper, which substitutes the
  codicon `CHEVRON_DOWN`/`CHEVRON_RIGHT` glyph only when `TreeStyle`'s
  `chevron_expanded`/`chevron_collapsed` still hold their defaults (`▾`/`▸`),
  honouring an app-supplied override verbatim otherwise. Previously layout
  measured the configured string literally even when paint substituted the
  codicon glyph for the default style, so the chevron hit-region's end-x could
  disagree with where the glyph paint drew.
- `FindReplacePanel::hit_test` now compares the click point against each
  region's pixel rect directly instead of flooring the click to a cell
  first. Behaviour-preserving for every `char_width`/`line_height` pair a
  backend actually passes.
- `Canvas` `DrawOp::Polygon` and `DrawOp::Ellipse` fills now render at the
  requested opacity on every pixel backend (GTK, win, macOS — all of which
  paint `Canvas` through the shared scanline fill). The fill's scanline rows
  are anchored to multiples of the row height from `0` rather than starting
  at the shape's un-aligned minimum `y`, so each row lands on a device pixel
  row boundary for the common integer-pixel canvas offset. Rows that
  straddled the same pixel row at a shared fractional offset previously had
  two independently-antialiased row fills `OVER`-composite onto that pixel,
  blending in ~21% of the background and making an opaque fill's interior
  look translucent.

## [0.2.2] - 2026-10-17
### Fixed

- GTK's `MultiSectionView` aux-row text input caret now blinks, matching
  macOS's `caret_blink` behaviour. `GtkBackend` owns a
  `caret_blink::CaretBlinkState`, ticked every `CaretBlinkState::period`
  by a recurring `glib::timeout_add_local` installed in
  `gtk::run::setup_caret_blink`, with `KeyPressed`/`CharTyped` events
  holding the caret solid through a typing pause via
  `GtkBackend::note_caret_activity`. Previously the caret was always
  painted solid (`caret_visible: true` was hardcoded).
- Windows' `MultiSectionView` aux-row text input caret now blinks, matching
  macOS/GTK's `caret_blink` behaviour. `WinBackend` owns a
  `caret_blink::CaretBlinkState`, ticked every `CaretBlinkState::period`
  by a dedicated, always-repeating `SetTimer` (`CARET_BLINK_TIMER_ID`)
  armed once at startup, with `KeyPressed`/`CharTyped` events holding the
  caret solid through a typing pause via `WinBackend::note_caret_activity`.
  Previously the caret was always painted solid whenever focused.
- GTK `TreeView` chevron hit-region width now comes from a real Pango
  glyph measurement of `tree.style`'s configured chevron character,
  instead of the `line_height * 0.65` estimate `gtk_tree_layout` uses.
  Both `draw_tree`'s paint path and `GtkBackend::tree_layout`'s
  hit-test path go through the new `gtk_tree_layout_measured` (backed
  by a fresh Pango layout from `GtkBackend::chrome_measure_layout` when
  no live layout is already in hand), so chevron hit-testing agrees
  with what's actually painted — mirroring the macOS backend's
  `mac_tree_layout`.
- Windows `TreeView` chevron hit-region width now comes from a real
  DirectWrite glyph measurement of `tree.style`'s configured chevron
  character, instead of the `line_height * 0.65` estimate
  `win_tree_layout` uses. Both `draw_tree`'s paint path and
  `WinBackend::tree_layout`'s hit-test path go through the new
  `win_tree_layout_measured` (backed by the live `DWrite` handle), so
  chevron hit-testing agrees with what's actually painted — mirroring
  the GTK and macOS backends' `gtk_tree_layout_measured` /
  `mac_tree_layout`.

## [0.2.1] - 2026-10-16
### Added

- `AppShell::with_active_accent`/`set_active_accent` and
  `AppShell::with_selection_bg`/`set_selection_bg` — builder and runtime
  setters that feed `AppShell::build_activity_bar`'s previously-hardcoded
  `ActivityBar::active_accent`/`selection_bg` fields. A host app sources
  colours from its own `Theme` and passes them through these before
  `AppShell::render`, so the activity bar paints an active-panel accent
  line and a keyboard-selection highlight. Unset (the default) still
  paints neither, matching the pre-existing behaviour.

### Fixed

- `Minimap::layout_with_sizing` no longer paints an oversized
  `viewport_highlight` band across the whole strip when the editor's
  viewport already spans every row of the file (an empty or very short
  buffer that fits entirely on-screen). The highlight is now zero-height
  in that case, matching the "nothing to scroll, nothing to indicate"
  convention `Minimap::scroll_thumb` already follows. Affects every
  backend that renders the minimap (GTK, TUI, macOS, Win-GUI), since they
  all consume the same shared layout.

## [0.2.0] - 2026-10-16

### Added

- `descriptor_store` module (`DescriptorStore<T>`, `Handle<T>`) — a
  Rust-owned retained store addressed by handle instead of by position:
  `insert(value) -> Handle<T>`, `update(handle, |t| …)` for in-place
  mutation, `remove`, `get`/`get_mut`, and `iter`. Lets a host turn a
  per-frame `Vec<(Handle<T>, Rect)>` into pure geometry, with the
  descriptor itself mutated in place rather than rebuilt from scratch each
  frame. Generation-checked per slot and store-id-tagged per `Handle`, so a
  stale or foreign handle is rejected by every lookup rather than
  silently resolving to the wrong value. Backend-neutral, no feature gate;
  no primitive is wired through it yet — that is per-primitive follow-on
  work.
- `OwnedFrame`/`OwnedNode`/`OwnedContent` — a lifetime-free, owned sibling
  of `Surface`/`ScreenLayout`: `OwnedFrame::from_layout` clones a
  `ScreenLayout<'_>`'s surfaces into a `Vec<OwnedNode>` (each carrying its
  own `Rect`, optional `WidgetId`, optional `A11yInfo` and an owned
  `OwnedContent` payload), fully `Serialize`/`Deserialize`. `OwnedFrame::
  draw` replays the captured tree through any `&mut dyn Backend`
  (returning a `FrameHitMap`); `OwnedFrame::hit_map` derives the same hit
  map without a backend at all. Six overlay-companion primitives
  (`Tooltip`, `ContextMenu`, `Dialog`, `Float`, `Completions`,
  `RichTextPopup`) and the two non-serializable `DropOverlay`/
  `FindReplacePanel` primitives are captured as zone-only, no-op-on-draw
  variants since their `*Layout` companions and overlay handles aren't
  serializable. Purely additive: `Surface<'a>`/`ScreenLayout<'a>` are
  unchanged, so this is a sibling type rather than a breaking rewrite.
- `transition` module (`Transition`, `lerp`, `lerp_rect`) — a
  backend-agnostic time-progress helper for chrome motion (fades, slides)
  that computes `0.0..=1.0` progress from a start `Instant` and `Duration`,
  clamped before the start and after the end. It does not touch
  `InvalidationLog` itself; a caller advancing a `Transition` each frame is
  expected to call `Backend::invalidate_rect` for whatever rect changed.
- `Backend::invalidate_rect`/`Backend::invalidation_log` and the backing
  `InvalidationLog` type — a bookkeeping-only pair (required methods, no
  default, per rule 7) that records dirty rects without triggering any
  native partial-repaint call, mirroring `request_full_repaint`'s
  "latch now, act later" shape. Implemented by all four real backends via
  `BackendCore::record_invalidation`/`BackendCore::invalidation_log`, plus
  `testing::RecordingBackend` and both `MockBackend`s. Wiring the actual
  native partial-repaint call (GTK `queue_draw_area`, AppKit
  `setNeedsDisplay(_:in:)`, Win32 `InvalidateRect`) is deferred follow-on
  work.
- `Backend` trait methods `draw_button`/`button_layout`,
  `draw_radio_group`/`radio_group_layout` and `draw_switch`/`switch_layout`,
  with a real TUI rasteriser, a shared `PaintSurface`-based minimal paint
  (`native_surface_paint`, in each of `primitives::{button, radio_group,
  switch}`) wired into GTK's `draw_*` methods, and stub implementations
  for the Windows backend and both `MockBackend`s.
  `examples/tui_controls.rs` (`AppLogic` in `examples/common/controls.rs`)
  demos all three primitives together; a real native GTK painter (hover/
  pressed chrome, cairo/pango text) and demo follow in a later,
  backend-scoped issue.
- macOS painter for `Button`, `RadioGroup` and `Switch`: `MacBackend::
  draw_button`/`draw_radio_group`/`draw_switch` now paint through the
  shared `native_surface_paint` CoreGraphics path (primary/secondary
  variants, disabled dimming) instead of returning a bare layout, and
  register a click zone per widget (per option, for `RadioGroup`).
  Covered by a `MacDriver` conformance test driving focus, keyboard and
  mouse activation across all three (`tests/
  macos_standalone_controls_driver.rs`).
- Windows painter for `Button`, `RadioGroup` and `Switch`: `WinBackend::
  draw_button`/`draw_radio_group`/`draw_switch` now paint through the
  shared `native_surface_paint` Direct2D/DirectWrite path (primary/
  secondary variants, disabled dimming), with layout-only twins
  `win_button_layout`/`win_radio_group_layout`/`win_switch_layout` for
  hit-testing. Covered by a `WinDriver` conformance test driving focus,
  keyboard and mouse activation across all three (`tests/
  win_standalone_controls_driver.rs`) and a `win_controls` gallery
  example pairing the existing `ControlsApp`.
- `primitives::layout_metrics::tree_layout_measured` — a non-breaking
  sibling of `tree_layout` taking `&dyn TextMeasure`, which measures
  `TreeStyle::chevron_expanded`/`chevron_collapsed` for real instead of
  estimating a chevron's advance as `line_height * 0.65`. `tree_layout`
  keeps its existing signature and behavior.
- `Dialog::measure_generic_measured` — a non-breaking sibling of
  `measure_generic` taking `&dyn TextMeasure`, which sizes
  `DialogMeasure::button_width` from each `DialogButton::label`'s real
  measured width (floored at `measure_generic`'s flat `char_width * 8.0`
  estimate, so short labels never render narrower than they do today).
  `measure_generic` keeps its existing signature and behavior.
- macOS toast overlay now slides a new toast in from its stack's off-screen
  edge via `Transition`, instead of appearing at full position on its first
  painted frame. Entrance-only: toasts are removed from the overlay (not
  animated out) on dismissal, so there is no exit slide.
- macOS toolbar items now fade their hover highlight in over 120ms via
  `Transition` instead of snapping straight to `theme.hover_bg` on the
  first hovered frame. Scoped to hover only; the pressed/active highlight
  is unchanged.
- `prelude` module now also re-exports `AppShellLayout`, `ScreenLayout`,
  `Surface`, `ListView`, `Form`/`FormField`/`FieldKind`,
  `FormController`/`FormControllerEvent`, `PanelDefinition` and
  `TextDisplayLine`, alongside the primitives it already carried, so a
  README-style hello-world app can build against `quadraui::prelude::*`
  alone. Previously only `TextDisplayLine` of that list was re-exported.
- `Backend::context_menu_border_inset` (default `0.0`; `TuiBackend`
  returns `1.0`) and the provided `Backend::context_menu_viewport` built
  on it — the surface a context menu or dropdown should be laid out in,
  the viewport inset on all four sides by whatever chrome the backend
  paints *outside* the menu's item region. Callers pass it to
  `ContextMenu::layout`/`layout_at` instead of the raw viewport, so an
  edge-anchored menu keeps its border without the rasteriser and
  `hit_test` disagreeing about where the first item sits. Both are
  defaulted, so existing `Backend` implementors are unaffected.
- `quadraui::tui::CONTEXT_MENU_BORDER_INSET` — the TUI box border's width
  in cells, for callers that drive `tui::draw_context_menu` directly
  against a ratatui buffer and so have no `Backend` handle to ask.

### Changed

- `Default` + `#[non_exhaustive]` on eight primitives (`Button`, `ComboBox`,
  `DropZone`, `Float`, `NumberInput`, `RadioGroup`, `ScrollView`, `Switch`):
  each now implements `Default` and is `#[non_exhaustive]`, so a future
  field addition to any of them stops being a breaking change for a
  downstream consumer. **Migration note:** `#[non_exhaustive]` rejects
  every struct-literal form from outside this crate, including a literal
  naming every field with a trailing `..Default::default()` — that
  pattern still fails to compile. The construction patterns that keep
  working are a constructor (e.g. `Button::new(id)`) or
  `Default::default()` followed by direct field assignment (`let mut b =
  Button::default(); b.id = ...;`), since every field stays `pub`.
  `quadraui/tests/downstream_struct_literals.rs` exercises the second
  pattern against all eight types from an external-crate-compiled test,
  and `Button`'s own doc comment carries a `compile_fail,E0639` doctest
  proving the rejected literal actually fails to compile with the
  expected error. `Palette`, `Terminal`, `CommandCenter` and `Completions`
  also gain `Default` here but are **not** `#[non_exhaustive]`: `vimcode`
  builds each of them with exhaustive struct literals today (`Palette`
  has three live call sites; `Terminal`, `CommandCenter` and `Completions`
  one apiece), so marking any of them `#[non_exhaustive]` would break
  that published consumer; they stay plain, the same reason `Tooltip` is
  excluded below. `Scrollbar` keeps its existing `Default` impl but is
  intentionally excluded from `#[non_exhaustive]` because a sealed oracle
  acceptance fixture (`tests/acceptance/ms-11/c0_paint_smoke.rs`)
  constructs it as an exhaustive literal and that file cannot be edited
  by this change. `Tooltip` is intentionally excluded for the same
  published-consumer reason as `Palette`.
- `WidgetId` is now `Copy` and backed by a process-global string interner
  instead of a `pub String` field: constructing one still takes any
  `impl Into<String>` (`WidgetId::new`, `.into()` from `&str`/`String`),
  and `as_str()` still returns the label, now as `&'static str`. Breaking
  only for code that matched the old tuple-struct shape directly
  (`WidgetId(s)` / `widget_id.0`) or relied on `Clone`-not-`Copy`
  semantics; every in-repo call site used `WidgetId::new`/`as_str`/
  `Display`/`From`, which are unchanged, so no widget-ID construction or
  formatting call site needed touching. `Serialize`/`Deserialize` keep
  the plain-string wire form. The 50+ in-repo `.clone()` calls on what is
  now a `Copy` type were mechanically replaced with a plain copy
  (`clippy::clone_on_copy`); none of those sites changed meaning.
- `macos::mac_tree_layout` takes a fourth `font: &CTFont` argument, so it
  can measure chevron glyphs for real (mirrors `mac_form_layout`'s existing
  `font` argument). `Backend::tree_layout` is unchanged — `MacBackend`
  passes its own `chrome_font`. Breaking for direct callers of the
  `quadraui::macos::mac_tree_layout` re-export only; neither published
  consumer (`vimcode`, `coord-tui`) calls it — both reach macOS tree
  layout through `Backend::tree_layout`.
- `ShellConfig`'s px chrome fields are now primary on GUI backends, with
  VS Code's own defaults, instead of deriving from `line_height`:
  `activity_bar_width_px` defaults to `Some(48.0)` (was `None`),
  `title_bar_min_px` (tab bar) defaults to `Some(35.0)` (was `None`), and
  the new `status_bar_height_px` field defaults to `Some(22.0)` with a
  `with_status_bar_height_px` builder to override it. `AppShell::
  compute_layout` only applies these three px overrides when the active
  backend's `line_height` is above `1.0` (a real pixel metric); a cell-grid
  backend (`line_height <= 1.0`, true of `TuiBackend` only) still derives
  every chrome dimension from `line_height` as before, so **TUI layout is
  unaffected**. `Backend::snap_height` is identity on GUI and `.round()` on
  TUI, so GUI chrome no longer snaps to a line-height multiple at all.
  `TreeStyle::row_height` defaults to `Some(22)` (was `None`), matching VS
  Code's tree row pitch independently of font size; TUI ignores this field
  entirely and always renders one row per cell. **Migration note** (for
  `vimcode`, `coord-tui`, `kubeui`): a GUI app that relied on the old
  line-height-derived chrome sizing (activity bar / tab bar / status bar
  width or height, or tree row pitch) must now set the affected
  `ShellConfig`/`TreeStyle` field explicitly — `activity_bar_width_px:
  None`, `title_bar_min_px: None`, `status_bar_height_px: None`, or
  `TreeStyle { row_height: None, .. }` — to restore the previous
  line-height-relative behavior; a default-constructed `ShellConfig` run
  through `quadraui::gtk::run` now measures 48px/35px/22px chrome with no
  app configuration at all. No published consumer constructs
  `ShellConfig`/`TreeStyle` with `#[non_exhaustive]`-breaking exhaustive
  literals that this would reject — both fields were already `pub
  Option<f32>`/`Option<u16>` before this change, so existing explicit
  overrides keep compiling unchanged.

### Fixed

- Editor cursorline/diff/DAP-stopped row backgrounds (GTK, macOS, Win, TUI)
  no longer fill the full viewport width before a vertical scrollbar
  column is painted on top — each rasteriser now narrows the row fill to
  `EditorLayout::text_bounds`, the same bound the text clip already
  respects, so the highlight stops at the scrollbar track instead of
  bleeding under it.
- TUI context menu: the top (and left) border no longer collides with the
  first item's row when the menu is anchored flush against the viewport
  edge (e.g. right-clicking a tab bar rendered at row 0). The fix lives in
  geometry, not the rasteriser: a caller lays the menu out in
  `Backend::context_menu_viewport`, which reserves the chrome cell the
  rasteriser paints outside the item region, so `hit_test` and
  `draw_context_menu` agree on where the first item sits. Every in-tree
  caller (`compose::ContextMenuController`, `compose::MenuSystem`, the
  `submenu_app` example and the `kubeui` shells) goes through it.
- `tui::draw_context_menu` and `tui::draw_context_menu_with_submenus` no
  longer panic (debug) or wrap to `u16::MAX` (release) on a layout whose
  `bounds.x` or `bounds.y` rounds to `0` — a menu laid out in a viewport
  with no room reserved for the border. Such a layout degrades
  cosmetically instead: the edge with no cell to paint on loses its
  border, every item still renders on the row and column its own bounds
  report, and nested levels are laid out in a reserved child viewport so
  their own chrome stays on-screen.
- Win-GUI activity bar text runs are now recorded in surface-absolute
  coordinates (matching GTK's `cr.user_to_device()` adjustment), so
  `WinDriver::find` returns positions in the same coordinate space the
  hit-test path expects — text painted inside a `with_translation`
  scope (e.g. the activity bar's bar-relative rasteriser) no longer
  drifts from its on-screen position when chrome dimensions change.
- `AppShell::layout` now derives its own px-override-suppression and
  rounding rule from `line_height` alone (identity above `1.0`, `.round()`
  at or below it) instead of always rounding, so the hit-test-only path
  agrees with `AppShell::render`/`AppShell::handle` (which pass
  `backend.snap_height`) on the same `area`/`line_height` — paint and
  hit-test no longer drift on a GUI backend.
- `MacBackend::invalidate_rect` now calls AppKit's `setNeedsDisplayInRect:`
  on the recorded rect instead of only bookkeeping it in the shared
  `InvalidationLog`, completing the native partial-repaint wiring this
  type was added for.
- `SidebarSystem`'s tree layout now honors `TreeStyle::row_height`:
  `compute_tree_layout` delegates to the shared
  `layout_metrics::tree_layout` instead of re-deriving row pitch, indent
  and chevron geometry from `line_height` in a hand-inlined copy that had
  drifted from it.
- `tree_row_pitch` now suppresses `TreeStyle::row_height` (a px value) on
  cell-grid backends (TUI, where `line_height` ≤ 1.0), falling back to the
  `line_height`-derived default. Without this guard, the default `Some(22)`
  made each tree row 22 cells tall, breaking click hit-testing on TUI.

- `Scrollbar`'s shared pixel-backend rasteriser (GTK/Win/macOS): thumb
  resting alpha raised from 0.50 to 0.65 (hover 0.70→0.80, drag 0.85→0.90)
  and the thumb now paints as a rounded pill via `surface_fill_rounded_rect`
  instead of a sharp-cornered fill, so a 14px-wide track reads with
  VS-Code-like visual weight instead of as too-translucent/too-narrow.

- `mac_tree_layout` now measures each branch row's chevron glyph
  (`TreeStyle::chevron_expanded`/`chevron_collapsed`) with the real
  `CTFont` via `layout_metrics::tree_layout_measured`, instead of the
  `line_height * 0.65` estimate `layout_metrics::tree_layout` falls back
  to, so a chevron hit region and the label column that follows it line up
  with the glyph the macOS rasteriser actually paints. `MacBackend`'s
  `Backend::tree_layout` and `macos::draw_tree` both route through it with
  the chrome `CTFont`.

- TUI tab bar: a tab with no close button (`TabBar::show_tab_close: false`,
  or a non-closable tab) now reserves and paints one trailing separator
  column (`tui::TAB_SEPARATOR_CHAR`, `│`) between it and the next tab,
  instead of zero inter-tab padding. Previously a bar like Problems/Output/
  Terminal painted as one concatenated run of text with only bold-vs-not
  distinguishing the active tab. `TuiBackend`'s eight tab-bar measurer call
  sites in `backend.rs` now add `tui::TAB_SEPARATOR_COLS` to a tab's
  measured width whenever it has no close-button reservation; tabs with a
  close button, and bracket-framed active tabs, are unaffected.

## [0.1.8] - 2026-10-14

### Added

- `ShellApp::pre_render` hook: called before shell layout reads
  `chrome_line_height`, so apps can synchronise per-frame backend state
  (font metrics, scrollbar widths) before layout runs, eliminating the
  one-frame convergence lag that caused hit-band mismatches.

## [0.1.7] - 2026-10-13
### Added

- Standalone `Button`, `RadioGroup`, and `Switch` primitives
  (`quadraui::primitives::{button, radio_group, switch}`): layout,
  hit-test, keyboard focus/activation, and their events, usable outside
  `Form`/`Dialog`. `Button` ships `ButtonVariant`, `ButtonLayout`,
  `ButtonHit`, `ButtonEvent`; `RadioGroup` ships `RadioOption`,
  `RadioOptionMeasure`, `RadioGroupLayout`, `RadioGroupHit`,
  `RadioGroupEvent`; `Switch` ships `SwitchLayout`, `SwitchHit`,
  `SwitchEvent`. Shared logic only — no backend painter yet, so these
  have no TUI/GTK rasteriser or demo; a headless `TuiDriver`
  conformance test (`tests/standalone_controls_driver.rs`) covers focus
  traversal, keyboard and pointer activation, and events on all three.
  `Form`/`Dialog`'s own field kinds are untouched.
- `ScrollView` primitive (`quadraui::primitives::scroll_view`): generic
  scroll-offset state, wheel/keyboard handling (`handle_wheel`,
  `handle_key`), visible-range clipping math (`visible_range`,
  `is_visible`), and scrollbar geometry (`scrollbar`) for a host that
  scrolls arbitrary child content it lays out itself. Ships
  `ScrollViewEvent`. Offsets are `f32` from the start, so it needs no
  further API change for the planned crate-wide move to continuous
  scroll offsets. Additive only — `List`/`TreeView`/`DataTable` keep
  their own row-indexed scroll state unchanged. Shared logic only — no
  backend painter yet; a headless `TuiDriver` conformance test
  (`tests/scroll_view_driver.rs`) covers wheel/keyboard scroll and
  clipping on arbitrary content.
- GTK rasteriser for `ScrollView`
  (`quadraui::gtk::scroll_view::{paint_clipped, draw_scrollbar_for}`):
  clips child painting to the viewport and draws the scrollbar chrome
  via the existing `native_surface_paint` scrollbar painter. Scroll-
  offset/clipping math stays in `quadraui::primitives::scroll_view` —
  this module only paints. Ships the `gtk_scroll_view` example (wheel
  scroll + scrollbar-thumb drag over a tall row list), covered by a
  `GtkDriver` test (`tests/gtk_example_driver.rs`) asserting both input
  paths clip and scroll the painted content correctly.
- Win-GUI rasteriser for `ScrollView`
  (`quadraui::win::scroll_view::{paint_clipped, draw_scrollbar_for}`):
  same shape as the GTK rasteriser above — clips child painting to the
  viewport via `Backend::paint_surface`'s clip push/pop and draws the
  scrollbar chrome via `Backend::draw_scrollbar`. Scroll-offset/clipping
  math stays in `quadraui::primitives::scroll_view`; this module only
  paints, and adds no new `Backend` trait method. Ships the
  `win_scroll_view` example, reusing the same `ScrollViewDemo`
  `AppLogic` as `gtk_scroll_view`, covered by a `WinDriver` test
  (`tests/win_example_driver.rs`) asserting both wheel scroll and
  scrollbar-thumb drag clip and scroll the painted content correctly.
- `NumberInput` and editable `ComboBox` primitives
  (`quadraui::primitives::{number_input, combo_box}`): `NumberInput`
  covers min/max/step/precision bounds, stepper-button and
  arrow-key/wheel increment, and commit/validation; `ComboBox` covers
  free-text editing with a filtered dropdown (reusing `Completions`'
  placement layout), type-ahead, keyboard navigation, and commit/cancel
  semantics. `NumberInput` ships `NumberInputLayout`, `NumberInputHit`,
  `NumberInputEvent`; `ComboBox` ships `ComboBoxLayout`, `ComboBoxHit`,
  `ComboBoxEvent`. Shared logic only; a headless `TuiDriver` conformance
  test (`tests/number_input_combo_box_driver.rs`) covers validation,
  commit, and cancel for both widgets. `TextInput` is untouched.
- GTK and TUI rasterisers for `NumberInput` and `ComboBox`
  (`quadraui::gtk::{number_input, combo_box}`,
  `quadraui::tui::{number_input, combo_box}`): stepper-button and
  mouse-wheel chrome for `NumberInput`, dropdown and type-ahead
  highlight chrome for `ComboBox`, via two new required `Backend`
  methods (`draw_number_input`, `draw_combo_box`, no default body per
  rule 7) implemented by every backend. The GTK rasterisers delegate to
  shared `native_surface_paint::paint` functions added to
  `quadraui::primitives::{number_input, combo_box}`; the TUI rasterisers
  mirror the same fill/stroke/text layering directly on the `ratatui`
  buffer — layout and validation/commit/cancel logic is unchanged and
  stays backend-generic. Covered by `GtkDriver` tests in each new GTK
  module (stepper click/wheel increment for `NumberInput`; type-ahead
  filter, keyboard navigation, commit, cancel, and dropdown-item click
  for `ComboBox`) and by TUI paint/click round-trip tests plus two new
  `tests/conformance/c0.rs` cases. The gallery demo is deferred to the
  `ComboBox`/`NumberInput` gallery-page issue.
- Windows rasteriser for `NumberInput` and `ComboBox`
  (`quadraui::win::{number_input, combo_box}`): same shape as the GTK
  rasterisers above — `Backend::draw_number_input` and
  `Backend::draw_combo_box` delegate entirely to the shared
  `native_surface_paint::paint` functions in
  `quadraui::primitives::{number_input, combo_box}`, so these modules
  carry no production code of their own. Layout and
  validation/commit/cancel logic is unchanged and stays backend-generic.
  Covered by `WinDriver` tests in each new module (stepper click/wheel
  increment for `NumberInput`; type-ahead filter, keyboard navigation,
  commit, cancel, and dropdown-item click for `ComboBox`). The gallery
  demo is deferred to the `ComboBox`/`NumberInput` gallery-page issue.

### Fixed

- `MacBackend::draw_tree`, `draw_list`, `tree_layout`, `tree_vscrollbar`,
  `list_layout`, and `list_vscrollbar` now use `chrome_line_height` instead
  of `current_line_height` — the same class of fix as the MSV metric
  correction in 7dfbad5e. When the editor font and system-UI font produce
  different line heights (e.g. CI default SF Pro 11pt vs Menlo 14pt), the
  mismatch made painted row bounds diverge from hit-test bands, causing
  `sweep_hit_band_integrity` failures in downstream tree tests.

## [0.1.6] - 2026-10-12
### Fixed

- `ShellAdapter::handle` now passes `chrome_line_height()` to
  `AppShell::layout`, matching `AppShell::render` which already uses it
  since #1517 — fixes a one-frame sidebar layout shift that broke
  explorer hit-band tests (the event routing computed a different
  sidebar rect than what was painted).
- `MacBackend::msv_metrics` now uses `chrome_line_height` instead of
  `current_line_height`, consistent with `msv_layout` and
  `draw_multi_section_view` (both already use it since 7dfbad5e).

## [0.1.5] - 2026-10-12
### Added

- `ShellConfig::status_bar_under_main_column` field and
  `ShellConfig::with_status_bar_under_main_column()` builder, threaded
  through `build_shell_adapter()` — lets ShellConfig-based consumers
  opt into the full-height activity bar layout without reaching past
  `AppShell`'s private fields (refs #1497).
- `Backend::chrome_line_height()` — line height of the chrome / UI font,
  defaulting to `line_height()` so TUI and single-font backends work
  unchanged. `MacBackend` overrides it to return the system UI font's
  line height. GTK and Win backends fall through to the default for now
  (issue #1483).

### Fixed

- Tab strip, app shell, sidebar, menus, bottom panel, help overlay,
  and context menu now size against `chrome_line_height()` instead of
  `line_height()`, matching VS Code's tab row height on macOS where the
  chrome and editor fonts have different metrics (issue #1483).
- MacBackend `draw_multi_section_view` and `msv_layout` now use
  `chrome_line_height` / `chrome_char_width` instead of the editor font
  metrics, fixing paint-vs-hit misalignment in the explorer sidebar
  (conformance regression from #1483 / #1518).

## [0.1.5] - 2026-10-12
### Added

- `CaretBlinkState` (issue #1460) — backend-neutral software caret-blink
  state machine: a `visible`/`paused_until` toggle driven entirely by
  caller-supplied `Instant`s, with `on_activity` to hold the caret solid
  while typing and `next_wake_delay` for a backend that schedules its next
  wake via `Backend::request_frame_in` instead of a raw repeating timer.
  This is the seam half of GTK/Win caret blink: it extracts the timer-free
  logic already proven by `macos::caret_blink`'s `NSTimer`-driven
  implementation so a GTK or Win backend can drive the same convention
  through its own native timer mechanism in a follow-up, per-backend issue.
  TUI is unaffected — it relies on the terminal's own hardware cursor
  blink, not a software state machine.
- `AppShell::with_status_bar_under_main_column(self) -> Self` (issue #1497) —
  opt-in layout mode where the activity bar and sidebar run the full height
  of the area below the title bar, and the status bar + command line are
  carved from the main column's x/width only, still flush against the
  bottom edge, instead of spanning the full window width. Off by default,
  which keeps the original full-width carve byte-identical to before this
  method existed.

### Fixed

- File-picker / palette overlay now paints an opaque background fill
  covering its full rectangular bounds before drawing the rounded-rect
  silhouette (issue #1509). Previously the rounded fill left the four
  corner triangles between each arc and the clipping rectangle unpainted,
  allowing content rendered earlier in the same frame (e.g. sidebar tree
  chevrons) to bleed through. Affects all pixel backends (macOS, GTK,
  Win) that share `native_surface_paint::paint`.
- `TreeView::vscrollbar`'s track is now a fixed 10px wide on pixel backends
  (GTK, macOS, Win), independent of the tree's row pitch, matching VS Code's
  standard list/tree scrollbar width (issue #1495). It previously reused
  `row_height` as both the track width and the minimum thumb length, so a
  pixel backend's ≈20px row pitch produced a ≈20px-wide scrollbar track
  instead of 10px. `TreeView::vscrollbar` now takes an explicit
  `track_width` parameter (still a separate `row_height` for the minimum
  thumb length); every pixel backend's `Backend::tree_vscrollbar` passes
  the new `layout_metrics::pixel::TREE_VSCROLLBAR_WIDTH` constant. TUI is
  unaffected — its track stays one cell wide.

### Fixed

- macOS Explorer header toolbar icons are now vertically centred in the
  sidebar header band instead of being placed low and clipped (issue #1494).
- macOS editor vertical scrollbar now renders at the configured width
  (14px) instead of collapsing to a 1px column at the window edge
  (issue #1496).
- macOS title-bar command centre now reads like VS Code's rather than a
  high-contrast form control (issue #1482): the search box is a rounded
  rect with a low-alpha tint of `Theme::separator` and a thin, low-alpha
  border of the same colour (instead of a full-opacity hard-edged
  rectangle outline), a small dim codicon magnifier sits ahead of the
  label, and the back/forward glyphs are codicon chevrons instead of
  full-weight Unicode triangles. `macos::command_center::mac_command_center_layout`
  measures the magnifier and its trailing pad as part of the search box's
  width, so a label long enough to beat the 280px minimum no longer paints
  past the box's right edge; the whole strip stays centred in its bounds.
  Other backends are unaffected — GTK, TUI and Win keep their own styling.
- macOS editor tab labels are now vertically centred against the chrome
  font's own line height instead of the editor font's (issue #1483).
  `MacBackend::draw_tab_bar_icons`/`draw_tab_bar_icons_layout` paint tab
  labels through `chrome_font` but centred them using
  `current_line_height` (the editor metric), so a tab bar whose chrome
  font differs in size from the editor font centred its label against a
  line height that didn't describe the font actually painting it. Both
  methods now use `chrome_line_height`. Other backends are unaffected.
- macOS active-tab chip background now fully encloses the tab label
  (issue #1474). The rounded chip's top/bottom inset was a fixed
  constant, so when the row height was tight (e.g. reusing the editor
  line height), the chip was shorter than the label's own line height
  and descenders or ascenders painted outside the chip. The inset now
  shrinks adaptively so the chip is always at least as tall as the
  label. Other backends are unaffected.

## [0.1.4] - 2026-10-11

### Added

- `Backend::tree_paints_own_scrollbar() -> bool` (issue #1476) — a provided
  (default `false`) trait method a tree-hosting consumer (e.g.
  `compose::TreeController`) calls to decide whether it must still reserve a
  column and paint a scrollbar itself, or whether the backend's `draw_tree`
  already paints an overlay scrollbar over the tree's own content. GTK,
  macOS and Win override it to `true`; TUI keeps the default `false`.
- `FlexLayout::without_rounding(self) -> Self` (issue #1428) — opts a
  `FlexLayout` out of its default pixel-snapping so a caller can compare
  raw `taffy` output bit-for-bit against a hand-rolled layout in tests.
  `AppShell`'s chrome-band carve, `Form`'s field stacking, and `Toolbar`'s
  item packing now route their 1D position arithmetic through
  `crate::flex::pack_1d` when the `layout` feature is enabled, falling
  back to the original hand-rolled arithmetic when it isn't; both paths
  are verified byte-identical by each primitive's own exact-value tests
  plus three `TuiDriver` end-to-end tests, all run under both
  configurations. `layout` stays its own opt-in feature (not
  bundled into `tui`/`gtk`/`win`/`macos`, per this crate's "no feature
  pulls in another opt-in feature" policy) — see its doc comment in
  `quadraui/Cargo.toml` for how to enable it alongside a backend feature.

### Changed

- macOS idiom pass (issue #1383): the menu bar installs the native
  `NSMenu` whenever the effective menu style resolves to `Native` (the macOS
  default), falling back to the in-window painted strip only after
  `set_menu_style(MenuStyle::Custom)`; the painted strip no longer draws
  Alt-mnemonic underlines on macOS; the active tab paints as an inset,
  rounded chip matching GTK's instead of an edge-to-edge fill.

### Fixed

- `compose::TreeController` no longer double-paints the tree's vertical
  scrollbar on GTK, macOS and Win (issue #1476): it previously always
  reserved a column and called `Backend::draw_scrollbar` even on backends
  whose `draw_tree` already paints an overlay scrollbar, producing two
  scrollbars (one of them an extra-wide reserved column) beside the tree.
  It now checks the new `Backend::tree_paints_own_scrollbar()` and, when
  `true`, paints nothing itself — the existing `Backend::tree_vscrollbar`
  geometry is still used for hit-testing and dragging. TUI's behavior is
  unchanged.
- Over-long tree row labels on GTK, macOS and Win are truncated with a
  trailing `…` (keeping the badge reserve) instead of being hard-clipped
  mid-glyph, matching TUI (issue #1475).
- macOS `SidebarPanel` header toolbar rows are sized off the chrome font's
  line height rather than the editor's, so header icons are no longer
  clipped at the top when the chrome font is taller (issue #1470).

## [0.1.3] - 2026-10-10

### Added

- Bundled Microsoft's codicon icon font (CC-BY-4.0; license text ships at
  `quadraui/assets/CODICON_LICENSE`) and self-register it on every GUI
  backend with no app configuration required (issue #1377). Tree
  expand/collapse chevrons, tab dirty/close marks, the context-menu
  submenu arrow, and (GTK) data-table sort arrows now paint as codicon
  glyphs on GTK, macOS and Windows instead of plain text characters; TUI
  output is unchanged.
- `Backend::set_editor_v_scrollbar_width(Option<f32>)` (issue #1411) — a
  provided (default no-op) trait method a GUI host calls once to size the
  editor's vertical scrollbar column in pixels, e.g. `Some(14.0)` to match
  VS Code's fixed 14px editor scrollbar independent of font size. Honoured
  by the GTK, macOS and Win backends in `draw_editor` and in
  `Backend::editor_layout` (so hit-testing agrees with the paint); TUI
  keeps its one-cell column. `Backend::editor_v_scrollbar_width()` reads
  it back. `None` (the default) keeps the existing one-`char_width`
  column.
- `Editor::layout_with_options_and_v_scrollbar_w(viewport, cell_width,
  line_height, options, v_scrollbar_w)` — the layout behind the above:
  `Some(px)` sizes `v_scrollbar_bounds` (and the text-area narrowing it
  causes) independently of `cell_width`. `Editor::layout_with_options` is
  unchanged and delegates with `None`. Purely additive:
  `EditorPaintOptions` is untouched.
- `Style` grows five new tokens (issue #1378): `padding`, `corner_radius`,
  `border_width`, `shadow_elevation` (`0..=3`), `control_height`, each with
  its own `with_*` builder and a default chosen to match VS Code's own
  chrome metrics. `PaintSurface` grows `surface_stroke_rounded_rect`
  (required; implemented on GTK/macOS/Win), `surface_draw_shadow` and
  `surface_draw_path` (both defaulted, composed out of existing verbs) plus
  the new `PathVerb` enum. `Toast`, `Tooltip` (its `Full`-border chrome),
  `ContextMenu`, `Palette` and `Dialog` now paint a rounded, bordered,
  shadowed box on every GUI backend instead of a square, borderless (for
  `ContextMenu`) or unshadowed one; `ProgressBar`'s track/fill paint with
  `corner_radius`. TUI is unchanged. See
  `quadraui/docs/decisions/DECISIONS.md` D-020 for the full design.

### Changed

- `GtkBackend`/`MacBackend`/`WinBackend` now default `nerd_fonts_enabled`
  to `true` (issue #1377) — the bundled codicon font removes the
  "Nerd Font not installed" risk that kept every backend defaulting to
  `false`, so an app's own `Icon::glyph` paints with no
  `Backend::set_nerd_fonts` call. TUI's default is unchanged (`false`).
  An app that needs the old default can call `set_nerd_fonts(false)`
  itself.

### Fixed

- **macOS, GTK, and Win native: editor body text was top-anchored in its
  row instead of centred** (issue #1412). A host sets the editor's row
  pitch (`line_height`) independently of the font's own natural
  ascent/descent, and each backend positioned text glyphs straight at a
  row's raw top edge: on macOS the gutter number centred itself but the
  body text and block-cursor glyph repaint did not, putting them on
  different baselines; on Win the gutter number centred itself
  per-glyph while body text and the block-cursor glyph repaint stayed
  top-anchored; on GTK every text paint (gutter, body, ghost
  continuation, the after-cursor AI ghost suggestion, inline annotation,
  the code-action lightbulb) was top-anchored. All three backends now
  derive one `text_v_offset` from the font's natural line height vs. the
  host-set row pitch and add it
  to every text (not row-fill) paint in a row, so gutter and body share
  a baseline and rows read centred like VS Code. Row-sized fills
  (cursor rect, selection, cursorline, diagnostic/spell underlines) are
  unaffected — they stay anchored to the full row, matching VS Code.
- **macOS native: the editor painted no vertical scrollbar at all**
  (issue #1411), even when the buffer overflowed the viewport and VS
  Code showed a 14px one on the same screen. `Editor::layout` already
  reserved the column (`v_scrollbar_bounds`), but no macOS rasteriser
  call ever filled it in. `macos::editor::draw_editor` now paints it
  through the same shared scrollbar paint GTK already uses, narrowing
  the text clip to match so no glyph paints under the reserved column.
  The Win backend's `draw_editor` likewise now paints the column through
  `Backend::draw_scrollbar`.
- **macOS native: editor text looked heavy and blocky next to VS Code**
  (issue #1405). The "Lines + gutter" paint loop drew every line's
  `raw_text` once in the default foreground colour, then re-drew each
  `line.spans` slice *on top of it* in its own colour — Core Text
  anti-aliases glyph edges, and compositing the same glyph twice turns
  edge alpha `a` into `1-(1-a)²`, saturating soft edges and making every
  stroke look thicker and stair-stepped. The line is now painted as
  contiguous, non-overlapping runs (default-coloured gaps + each span's
  own colour), so every glyph is painted exactly once — matching GTK
  (single Pango layout + `AttrList`) and Windows (coalesced
  non-overlapping `DrawText` runs), which never had this bug.

## [0.1.2] - 2026-10-09

### Fixed

- **GTK on macOS: Nerd Font icons rendered as tofu boxes** with their hex
  codes (issue #1367). `register_font_from_memory` registered app fonts
  only with Fontconfig, but GTK4 on macOS uses Pango's Core Text font
  map; fonts are now also registered with Core Text there.
- **TUI in macOS Terminal.app: runs of dimmed text** after every
  underline-coloured span (issue #1366). Terminal.app misparses
  crossterm's semicolon-form SGR 58 like ConPTY does (trailing `2` →
  faint); underline colour is now stripped when
  `TERM_PROGRAM=Apple_Terminal`. iTerm2 keeps coloured underlines.
- Published-crate hygiene: internal-only docs are excluded from the
  package, and `docs/decisions/` ships (issue #1351).

### Added

- A `# Examples` doctest on every public primitive, with a lint ratchet
  (issue #1352), and `examples/hello.rs` rewritten on the canonical
  `ShellApp` path (issue #1342).

### Changed

- `rust-version` in `quadraui/Cargo.toml` lowered from 1.97.1 (which only
  matched `rust-toolchain.toml`'s CI pin, for no technical reason) to
  1.92.0 — the real floor, held up by the `gtk` feature's glib-rs 0.22 /
  gtk4 0.11 dependency family. `ci.yml`'s new `msrv` job builds every
  backend feature, plus a zero-feature leg, at exactly that version on
  every PR.

## [0.1.1] - 2026-10-08

### Fixed

- **macOS native: Nerd Font icon glyphs painted as Apple's `?` placeholder**
  for some codepoints (e.g. Explorer, Source Control, Run & Debug) even
  though the registered icon font contains them (issue #1337, #1329). The
  Core Text cascade list now pins the registered font's own descriptor
  instead of looking the family up by name, which a different installed
  font could win. Confirmed on a real macOS 26 screen.
- **Windows native: the editor minimap painted as a dark, illegible strip**
  (issue #1354). `Characters`-mode rows were shaped with the editor-size
  DirectWrite format and clipped to a 2–4 DIP row. They now use a
  same-family format at `minimap_font_px`, cached by rounded size, falling
  back to column blocks below the legibility pitch. Confirmed on a real
  Windows desktop.
- `src/lib.rs`'s `## Status` section (docs.rs's front page) still said
  "prepared for its first publish to crates.io but not yet published"
  and claimed most Windows rasterisers were `todo!()` stubs, both wrong
  since v0.1.0 shipped (issue #1355). Note this only reaches docs.rs on
  the *next* published release, 0.1.1 — docs.rs renders whatever
  `src/lib.rs` looked like at the version it built, not `develop`'s tip.

## [0.1.0] - 2026-10-07

First tagged release, and the first published to crates.io.

### Added

- `Backend::draw_rich_text_popup_with_font_role` (issue #1322) — lets a
  consumer paint one `RichTextPopup` instance in
  `FontRole::Editor` (the editor's own monospace font/metrics) instead
  of the chrome (UI) font `Backend::draw_rich_text_popup` always uses,
  for editor-content hovers (code, signatures, diagnostics) where link
  hit-testing needs to match the glyphs actually painted
  (`vimcode#220`/`vimcode#504`'s supply side). Added as a new trait
  method with a default body delegating to `draw_rich_text_popup`
  (i.e. `FontRole::Chrome`, today's behaviour unchanged) rather than a
  new field on `RichTextPopup` itself — mirrors
  `Backend::draw_tooltip_with_chrome`'s `TooltipChrome` precedent, so
  no downstream consumer's exhaustive `RichTextPopup { .. }` literal
  breaks. GTK, macOS and Win-GUI all override it and honour
  `FontRole::Editor` with real per-backend ink/metrics tests; TUI keeps
  the default (one font per cell). `examples/common/markdown_demo.rs`
  gains an `f` key toggling between both roles, covered by a
  `TuiDriver` round-trip test.
- `Float { id, anchor, focusable, border, bg }` (issue #1321) — an
  anchored overlay surface above the main layout, the supply-side
  primitive `vimcode#1804` ("Extension UI Phase 3: overlays/floats for
  Lua extensions") needs so vimcode never builds floats itself.
  Positioned with the existing `Anchor` (`crate::layout`, now
  `Serialize`/`Deserialize` itself so a plugin-declared float survives
  a JSON round-trip), stacked via the existing `ModalStack` — which
  gains `push_focusable`/`top_focusable` (issue #1321) to carry and
  read back the "a hint popup must not steal keys; an interactive float
  must" distinction per entry, while `push` keeps its old
  focusable-by-default behaviour unchanged. `Float` is pure chrome
  (background fill + optional border), same contract as `Panel` — the
  host paints its own content into `FloatLayout::content_bounds`.
  Ships on TUI, GTK, macOS, and Win (`Backend::draw_float`, no default
  per rule 7; GTK/macOS/Win share one `PaintSurface`-routed
  implementation with zero backend-specific policy, same pattern as
  `Canvas`/`Panel`), with a `tui_float`/`gtk_float` demo and the
  `tui_float` demo's `TuiDriver` black-box tests.
- New optional `layout` feature and `quadraui::flex` module (issue #1103)
  — a flex/grid layout engine over [Taffy](https://github.com/DioxusLabs/taffy),
  for the app-level `Rect` arithmetic (`AppLogic::render` stacking/tiling
  panels by hand) every consumer previously wrote itself. Unit-agnostic —
  plain `f32` in, plain `f32` out, same as `event::Rect` — so it pairs with
  any backend feature, or none. `FlexLayout::add_leaf`/`add_container`
  build a tree of re-exported `taffy::Style`s; `FlexLayout::compute`
  returns a `NodeId` → `Rect` map in the same ABSOLUTE coordinate
  convention `crate::layout` already documents. No existing primitive's
  own `layout()` changes — see `src/flex.rs`'s module doc for the "what
  this is not" scope note.
- `TuiBackend::input_gone()` (issue #1295) — `pub`, read by a host embedding
  `TuiRunner` directly via `step`/`pump` to tell "the app chose to exit"
  apart from "its pty's input fd disappeared out from under it." Latched by
  the new dead-pty busy-loop guard described under `### Fixed` below; see
  that entry for the behavior it exposes.
- `Canvas { id, ops: Vec<DrawOp> }` (issue #1102) — the app-defined
  drawing escape hatch the framework audit's "closed widget catalogue"
  finding asked for. `DrawOp` covers rect, rounded rect, line, path,
  text run, image, and push/pop clip; GTK/macOS/Win paint it through the
  existing public `PaintSurface` seam (`Backend::paint_surface`, #1101)
  with no backend-specific policy at all. TUI's story is **degrade**,
  not `Unsupported` (issue #1097/D-014): shape ops rasterise into the
  same sub-cell braille dot grid `Chart`'s line charts already use;
  `TextRun`/`Image` snap to the nearest whole cell; `PushClip`/`PopClip`
  quantise outward to whole cells — see `primitives::canvas`'s module
  doc for the full per-op degrade table and the TUI rasteriser's own doc
  for its documented shapes-under-text z-order caveat. `Backend::canvas_layout`
  is a defaulted, backend-independent pure function of `rect` alone
  (`tests/conformance/caps.rs`'s `ACCEPTED_DEFAULTS`); `Backend::draw_canvas`
  has no default, per rule 7.
- `TextInput::new`/`with_*`, `Toolbar::new`/`with_*`, `Editor::new`/`with_*`,
  and a `Default` impl for all three (issue #1108, phase 1 of #1251) — every
  field a consumer previously had to set via an exhaustive struct literal
  (`tests/downstream_struct_literals.rs`'s guards) now has a chainable
  builder too, so `vimcode`/`coord-tui` can migrate off literal construction
  before the attribute below starts actually enforcing it. New
  off-by-default `strict-descriptors` feature: applies `#[non_exhaustive]`
  to these three descriptors via `cfg_attr`, purely so a consumer can prove
  its own migration is complete by building with the feature on — it
  changes nothing for anyone who doesn't opt in, and `tests/downstream_struct_literals.rs`'s
  literal-construction guards keep passing under the default feature set.
  #1251 is the follow-up that makes the attribute unconditional (the actual
  breaking change) once both consumers have migrated.
- `Backend::draw_status_bar_interactive_scaled` (issue #1045 item 2) — a
  per-call chrome-font-size override for one `StatusBar` paint, without
  perturbing the ambient `set_ui_font` state every other status bar (or
  chrome primitive) draws in the same frame. `font_scale` multiplies the
  backend's current UI font's point size for this call only; `1.0`
  behaves identically to `draw_status_bar_interactive`. Real per-call
  scaling ships on GTK (Pango `FontDescription` size swap) and macOS
  (`CTFont::clone_with_font_size`); the trait default (ignore `font_scale`,
  forward unscaled) covers TUI's fixed-cell grid and Win-GUI's existing
  status-bar font gap (tracked separately, unchanged by this issue).
- `ShellConfig::with_title_bar_min_px` / `AppShell::with_title_bar_min_px`
  (issue #1045 item 3) — a fixed-pixel floor under the title bar's
  line-height-derived height, mirroring `with_activity_bar_width_px`'s
  pattern (#657) but as a floor rather than a hard override: the resolved
  height is `max(title_bar_height_lh * line_height, title_bar_min_px)`, so
  the band keeps scaling with the editor font above the floor and only
  stops shrinking once a user-set font size would otherwise take it
  below some real platform constraint (e.g. macOS's traffic-light
  cluster height).
- `primitives::toast::{Toast, ToastOverlay, ToastButton}` (issue #1185) —
  the VS Code-style actionable-notification shapes that supersede
  `ToastItem` / `ToastStack` / `ToastAction`. A `Toast` carries an ordered
  `actions: Vec<ToastButton>` (zero or more, one of which may set
  `ToastButton::primary` for accent styling) instead of at most one
  `Option<ToastAction>`; a `ToastOverlay` adds `focus: Option<ToastFocus>`.
  Every backend rasteriser now wraps title/body across the box's full
  width and lays the actions out on their own row at the bottom-right
  (never inline with the title), with the dismiss `×` alone near the
  top-right. `Backend::draw_toast_overlay` is the new entry point.
  **The old names keep working** — see *Deprecated* below.
- `compose::ToastStackController` (issue #1185) — VS Code-style keyboard
  focus for the non-modal toast overlay: Tab/Shift+Tab/Left/Right
  cycle a focused toast's buttons (dismiss `×` included), Up/Down move
  between toasts, Enter activates the focused button, Escape dismisses the
  focused toast and returns focus to the app. Follows
  `MessageDialogController`'s pattern but never owns or blocks input on the
  stack — the app decides when to give it focus (e.g. its own "focus
  notifications" command) and attaches `ToastStackController::focus()` to
  the new `ToastOverlay::focus` field before painting so every backend draws
  a `theme.link_fg` ring around the focused control. `examples/tui_toast_actions.rs`
  / `examples/gtk_toast_actions.rs` (paired with `examples/common/toast_actions_app.rs`)
  demonstrate a two-action toast driven by both keyboard and mouse, with a
  `TuiDriver` black-box test in `tests/tui_example_driver.rs`.
- `Backend::default_fonts() -> PlatformFontDefaults` (issue #1156) — each
  backend's platform-native font defaults for the editor and UI (chrome)
  roles (`Menlo 12` / system-UI 13pt on macOS, `Consolas 14` / `Segoe UI`
  13pt on Windows, `Monospace 14` / `Sans` 13pt on GTK, an all-sentinel
  empty-family/`0.0`-size value on TUI, matching its no-op
  `set_editor_font`/`set_ui_font`), so a consumer can seed its own font
  settings from the running platform's convention instead of hardcoding
  one OS's defaults everywhere and branching on `cfg!(target_os)` to fix
  it up. `default_fonts()` always reports the static platform default,
  unaffected by any prior `set_editor_font`/`set_ui_font` call, so a
  consumer can compare its own setting against it to tell "still the
  platform default" from "user override" across a future backend change.
  New required `Backend` trait method with no default body (rule 7: safe
  because `Backend` is sealed, so this cannot break either downstream
  consumer) — all six in-tree `impl Backend` blocks (Tui, Gtk, Mac, Win,
  `RecordingBackend`, both `MockBackend` test doubles) implement it.
- `SidebarPanelChrome::Search { .. }` and `SidebarPanelChrome::StatusBars(Vec<StatusBar>)`
  (issue #1061) — two chrome shapes `draw_settings_chrome` couldn't
  express: a search-only row with no header above it (vimcode's
  Extensions sidebar, whose title now comes from `AppShell`'s own
  sidebar-header row since vimcode#1343 — reusing `draw_settings_chrome`
  would repaint the double-header bug #1256, since its row 0 is
  unconditionally header-styled), and one row per `StatusBar` (vimcode's
  Debug sidebar: a title bar + a Run/Stop action bar) painted via
  `Backend::draw_status_bar_interactive`, with every bar's hit regions
  translated into panel-space and surfaced on the new
  `SidebarPanelBodyLayout::status_bar_hit_regions` field so a host
  routes clicks against the same geometry it painted. `layout()`
  reserves the matching row count for both; `render()`/`render_with()`
  paint them in the existing background → chrome → body order.

  **Breaking, both admitted up front.** `SidebarPanelChrome` is now
  `#[non_exhaustive]` — adding these variants breaks any downstream
  exhaustive `match` with no wildcard arm regardless of the attribute
  (vimcode's `render::paint_sidebar_panel_chrome` has exactly such a
  match, confirmed against `origin/develop`), so marking it
  `#[non_exhaustive]` in the same breaking PR is rule 8's preferred
  shape so the *next* addition is additive instead. `SidebarPanelBodyLayout`
  also loses `Copy` (the new `status_bar_hit_regions: Vec<(Rect,
  StatusBarHit)>` field isn't `Copy`) while gaining that field — see
  `## Downstream impact` in the PR body for both consumer call sites
  that need to move.
- `Icon::color: Option<Color>` + `Icon::with_color(..)` builder (issue
  #1057) — a tree row's icon can now carry an identity colour, matching
  `TabIcon::color`'s tab-bar behaviour (#620). `None` (the default from
  `Icon::new`/`From<&str>`/`From<String>`) paints byte-identical to
  pre-#1057 rendering; `Some` is honoured by every backend's `TreeView`
  rasteriser (`tui::tree::draw_tree`, `gtk::tree::draw_tree`,
  `macos::tree::draw_tree`, `win::tree::draw_tree`), painting both the
  Nerd Font glyph and its ASCII fallback in that colour. New field on a
  public struct, so technically rule-8 territory — chosen over a new
  `TreeRow.icon_color` field (would break ~21 `TreeRow { .. }` literals
  in vimcode alone) and over a `TabIcon`-style sidecar (awkward for a
  tree controller). No consumer hits: `grep -rn 'Icon {' ~/src/coord-tui/src
  ~/src/vimcode/src` returns nothing — both consumers build `Icon` only via
  `Icon::new`/`From`, never an exhaustive struct literal.
- `PlatformServices::secret_store()` + `SecretStore` trait (issue #958,
  `ELECTRON_PARITY_AUDIT.md` §1.2 G16, ranked #9b) — get/set/delete access
  to the OS credential store (macOS Keychain, Windows Credential Manager,
  Linux Secret Service over D-Bus), keyed by `service` + `account`,
  backed by the cross-platform `keyring` crate. Unlike every other
  capability in the audit, this one is backend-independent: the same
  default body serves `tui`/`gtk`/`macos`/`win` with no per-backend
  override, and needs no window, display server, or desktop session — so
  it is **full support on TUI**, not a degrade. `get` returns `Ok(None)`
  for an entry that was never set (the same "nothing here, not a
  failure" idiom `Clipboard::read_text` already uses); `delete` on a
  nonexistent entry is a reported `Err`, matching
  `PlatformServices::move_to_trash`'s identical stance for a nonexistent
  path. Pulled in by the same four features (`tui`/`gtk`/`macos`/`win`)
  that already pull in `trash`; a build with none of them enabled gets a
  `SecretStore` whose every method honestly returns
  `BackendError::Unsupported` instead of failing to compile.
- `gtk::draw_editor` now paints both editor scrollbars itself, the way
  `tui::editor::draw_editor` already does (issue #968) — it used to defer
  to a host path (vimcode's `draw_window_scrollbars`) that vimcode#731
  deleted, so the capability had fallen through the gap between the two
  repos. The content clip is narrowed by the reserved column *before*
  painting text (mirroring TUI's `viewport_cols` narrowing), rather than
  letting the scrollbar's translucent track paint over glyphs. New
  `EditorPaintOptions` (currently just `suppress_v_scrollbar`, for a host
  running a `Minimap` in scrollbar mode, vimcode#723) plus
  `Editor::layout_with_options` / `gtk::draw_editor_with_options` are
  additive alongside the unchanged `Editor::layout` / `gtk::draw_editor` —
  deliberately *not* a new field on `Editor` itself, since both known
  consumers build it with an exhaustive struct literal (see
  `quadraui/tests/downstream_struct_literals.rs`'s new
  `editor_exhaustive_struct_literal_still_compiles`).
- `Backend::register_font_from_memory` + `Backend::set_nerd_font_fallback`
  (issue #929) — macOS and Win-GUI had no way to resolve Nerd-Font (or
  other PUA-codepoint) icon glyphs at all: unlike GTK, which cascades to
  a system-installed `Symbols Nerd Font` automatically
  (`crate::gtk::NERD_FONT_FALLBACK_FAMILY`), a `CTFont`/`IDWriteTextFormat`
  resolves every character against exactly one family, so an icon
  codepoint that family doesn't cover painted as a last-resort tofu box
  on both backends. `register_font_from_memory` hands the backend raw
  TTF/OTF bytes (e.g. an `include_bytes!`-embedded subset) to register
  with the platform font manager for the process lifetime — no
  filesystem write, no `fc-cache` — and returns the family name(s) it
  registered under; `set_nerd_font_fallback` names the family every
  subsequent `draw_*` call should consult for characters the primary
  font can't cover. Both default to a no-op (`None`/nothing) so every
  existing backend impl keeps compiling unchanged (rule 7).

  **Per backend:** macOS registers via `CTFontManagerRegisterGraphicsFont`
  and applies the fallback via a `kCTFontCascadeListAttribute` cascade
  list (`CTFontCreateCopyWithAttributes`) on the shared `current_font`,
  re-applied automatically regardless of whether `set_current_font` or
  `set_nerd_font_fallback` is called first. Win-GUI registers via
  `IDWriteFactory5::CreateInMemoryFontFileLoader` into a private
  `IDWriteFontCollection1` and builds a `IDWriteFontFallback` (via
  `IDWriteFontFallbackBuilder`, layered on top of — not replacing — the
  system's own fallback chain) applied to both `IDWriteTextFormat`s at
  the next `attach_surface`/`attach_headless`. GTK overrides
  `set_nerd_font_fallback` too, replacing the hardcoded
  `NERD_FONT_FALLBACK_FAMILY` constant with a process-wide settable
  value, so an app that targets this portable API gets the same effect
  on every backend rather than needing a GTK-specific escape hatch;
  `register_font_from_memory` stays a no-op there since fontconfig
  already resolves a system-installed Nerd Font. TUI takes both
  defaults — a fixed-cell backend has no font concept.

  New `BackendCaps::app_font_registration` field (declared by GTK,
  macOS, and Win-GUI; not TUI) lets a caller check either capability is
  wired before relying on it.
- `ToolbarIcons` + `Backend::nerd_fonts_enabled()` (issue #913) — a
  Nerd-Font glyph + ASCII fallback path for `ToolbarButton::Action`
  icons, which previously had none: `icon` is a single `Option<String>`,
  so a bar built with a Nerd glyph painted mojibake on a terminal
  without the font and a bar built with ASCII never showed the glyph on
  one with it. Hosts register `Icon` pairs by button id in a
  `ToolbarIcons` table and call `ToolbarIcons::apply(&bar, nerd)` to bake
  the resolved half into the `Toolbar` they hand to any existing
  `Backend::draw_toolbar*` / `SidebarPanel` / `FieldKind::Toolbar` /
  `Dialog` path; `Backend::nerd_fonts_enabled()` reads back whatever the
  host last gave `set_nerd_fonts`, so the flag doesn't have to be
  mirrored app-side. Because resolution happens *before* layout, the
  measured and painted widths of a wide glyph cannot disagree.

  **A side table, not a `Toolbar` field, deliberately.** Both consumers
  build `Toolbar` with exhaustive struct literals (four sites in
  `coord-tui`, two in `vimcode`), so a new field breaks them with
  `E0063` and `#[non_exhaustive]` breaks them with `E0639` — this change
  is non-breaking for every existing call site, and the new
  `Toolbar` cases in `quadraui/tests/downstream_struct_literals.rs`
  fail this repo's CI if a future change re-breaks that literal. Same
  escape hatch `TextEditor` uses for `TextInput` (issue #833).

  Also fixes the `cell_measure()` test helper in
  `primitives::toolbar`'s own suite, which measured icon/label/hint
  width with `chars().count()` — undercounting every East-Asian-Wide
  glyph by half a cell and diverging from the real
  `tui::toolbar::tui_item_width` — so a wide-icon layout bug could hide
  behind a green suite. The `tui_toolbar` demo grows an `n` key that
  toggles Nerd-Font glyphs live, covered end-to-end in
  `tests/tui_example_driver.rs`.
- `rust-version` in `quadraui/Cargo.toml`, pinned to match
  `rust-toolchain.toml` (1.97.1) — an older `cargo` now reports the version
  requirement directly instead of an opaque parse/feature error.
- `[package.metadata.docs.rs]` in `quadraui/Cargo.toml`, building every
  optional backend feature (`tui`, `gtk`, `win`, `terminal`, `macos`) across
  the Linux/macOS/Windows targets, plus `#[cfg_attr(docsrs, doc(cfg(...)))]`
  annotations on each feature-gated public module — a published crate now
  renders all four backends on docs.rs instead of only the always-on core.
- CI gates: `cargo package -p quadraui --locked` (full verification build,
  no vendored patches, proves the manifest + lockfile resolve and build
  standalone off crates.io) and `cargo doc --no-deps -D warnings` (the doc
  build itself is held to the same warnings-as-errors bar as the rest of
  CI; see that job's comment in `.github/workflows/ci.yml` for the narrow,
  named exception carved out for pre-existing intra-doc-link debt).
- This file.
- `quadraui::prelude` — a curated subset of the crate's ~500 flat
  crate-root exports (the two runner traits, event/geometry types, and a
  representative sample of primitives) for a first `AppLogic` app.
  `quadraui/docs/GUIDE.md` (new) walks through building a two-pane app
  with it, including a "which runner do I want?" (`AppLogic` vs
  `ShellApp`) comparison.
- `quadraui::layout` — shared foundation for the `layout()`/`hit_test()`
  convergence (issue #816): `Anchor` + `Side`/`ResolvedSide` (the
  preferred-side/overflow-flip/pin-to-edge resolution `Tooltip`,
  `Completions`, `ContextMenu` and `RichTextPopup` each reimplement via
  their own placement enum today) and `visible_range_walk` +
  `VisibleItem` (the scroll-offset visible-range walk all 19
  `Visible*{idx, bounds}` structs hand-roll today). Purely additive —
  no existing primitive consumes it yet; each primitive converges onto
  it in its own PR behind a `#[deprecated]` shim per
  `quadraui/docs/PRIMITIVE_RULES.md` rule 8.
- `quadraui::testing::RecordingBackend` — a public, fully-implemented
  `Backend` for unit tests that need *some* backend to hand a
  controller (e.g. `TreeController`, `ChatController`), not a specific
  backend's real rendering. Replaces three private, near-identical
  `MockBackend` copies previously duplicated across
  `compose::{tree_controller, chat_controller, app_shell}`'s own test
  modules.
- `quadraui/examples/hello.rs` — a standalone, under-60-line "hello
  world" example with no `examples/common/` dependency (`cargo run
  --example hello --features tui`), plus its `TuiDriver` end-to-end
  test in `quadraui/tests/tui_example_driver.rs`.
- `CONTRIBUTING.md` — human-oriented contributor guide (project layout,
  local setup, the quality-gate commands, PR expectations).
- `TerminalCell::dim` — carries SGR 2 (faint) from vt100's `Cell::dim()`
  (tracked since vt100 0.16; this crate's pinned floor). Additive field,
  `#[serde(default)]`, no consumer struct-literal hits in `coord-tui` or
  `vimcode` (grep in the PR body).
- `Reaction::RedrawAfter(Duration)` and `Backend::request_frame_in` (issue
  #832) — a precise scheduled wake, replacing the pre-#832 pattern of an
  app relying on a backend's fixed-cadence idle poll (TUI 16ms, GTK 33ms,
  neither on macOS/Windows at all) to eventually re-check time-driven
  state. TUI/GTK's idle poll is now a coarse fallback ceiling (250ms,
  `crate::runtime::IDLE_POLL_CEILING`) rather than the primary mechanism;
  macOS/Windows gained their first `AppLogic::tick` invocations ever,
  fired only when something requests one. `Reaction` is now
  `#[non_exhaustive]` — verified non-breaking for both downstream
  consumers today (neither `coord-tui` nor `vimcode` exhaustively matches
  a `Reaction` value; grep in the PR body), guarding against a future
  variant addition being one.
- `tui::testing::TuiDriver::tick` (issue #832) — runs one
  `AppLogic::tick` and applies its `Reaction` exactly as the live TUI
  loop does. Without it a driver test could only reach an app's
  event-driven half, so time-driven state (spinner frame, caret blink,
  countdown, background-job poll) was untestable headlessly.
- `tui::TuiBackend::frame_requests` / `TuiBackend::pending_frame_delay`
  (issue #832) — test-facing observers of `Backend::request_frame_in`:
  how many wakes an app asked for, and how long until the next one. An
  app that chains its own frames and one that free-rides on a fixed idle
  poll paint identical screens, so this is the only way a `TuiDriver`
  test can tell them apart. All three additions are purely additive —
  no consumer hits for either symbol in `coord-tui` or `vimcode` (grep in
  the PR body).
- `quadraui::undo::UndoStack<T>` (issue #833) — a generic, snapshot-based
  undo/redo stack, reusable by any primitive (not just `TextInput`).
- `TextEditor` + `TextEditor::apply(EditOp)` (issue #833) — the
  `TextInput` primitive's first real editing behaviour: insert, delete,
  cursor movement, and shift-to-select selection, plus
  `EditOp::Undo`/`EditOp::Redo` backed by a bounded `UndoStack` (200
  steps). `TextEditor::selection_range`/`selected_text`/
  `selection_anchor`/`set_selection_anchor` read and drive the selection.
  `EditOp::from_key` maps a plain keypress to the op it means;
  `EditOp::from_key_binding` wires the long-declared
  `KeyBinding::Undo`/`Redo`/`SelectAll` accelerator names (previously
  unconsumed) to real behaviour. Before this, every consumer (including
  this crate's own `examples/common/text_input_demo.rs`) hand-rolled
  insert/backspace/cursor-movement logic itself.

  **`TextInput`'s own field list is unchanged, deliberately** — this is
  fully additive for downstream consumers, with no migration required.
  `TextEditor` is a wrapper (`Deref`/`DerefMut` to the wrapped
  `TextInput`, plus a public `input` field) precisely so that the two
  pieces of live editing state, the selection anchor and the undo
  history, do *not* become new `TextInput` fields. Adding **any** field
  to `TextInput` — `pub` or private — breaks external exhaustive
  `TextInput { .. }` struct literals (`E0063` / `E0451` respectively;
  `..Default::default()` does not rescue the private case), and
  `vimcode`'s `src/render.rs::sc_commit_message_to_text_input()` is a
  live such call site. See `## Downstream impact` in the PR body for the
  grep, and the new `quadraui/tests/downstream_struct_literals.rs`, which
  fails this repo's own CI if a future change re-breaks that literal.
- `Surface::Board`/`CommandCenter`/`DiffView`/`DropOverlay`/`Image`/
  `MessageList`/`Minimap`/`PipelineView`/`Progress`/`SidebarPanel`/
  `Spinner`/`SplitTree`/`TextInput`/`Toolbar` and the matching
  `FrameZone` variants (issue #1099, partial — see below) — the 14
  primitives that had a `Backend::draw_*` method but no
  `Surface`/`FrameZone` twin (tracked since the #456 audit,
  `docs/decisions/DECISIONS.md` D-006) now have one, closing that gap.
  `DropOverlay::bounds()` — new, additive — gives its `Surface`/
  `FrameZone` pair a zone rect to use, since (unlike the other
  transient-overlay variants) it has no `rect` field or sibling
  `*Layout.bounds` to borrow. `FrameHitMap::zones`/`FrameHitMap::from_zones`
  and a `Serialize`/`Deserialize` derive on `FrameZone` and
  `FrameHitMap` make the existing, already-owned `FrameHitMap` readable
  and wire-transportable — a step toward #1099's "owned `Frame`" half,
  not that half itself: a `FrameHitMap` zone is still only a
  `(Rect, FrameZone)` pair (a rect plus a bare tag and a frame-local
  index), with no primitive content, role, label or value, so it
  cannot yet feed an AccessKit `TreeUpdate`, be re-painted by a web
  backend, or replay a frame on its own. `Surface<'a>`/`ScreenLayout<'a>`
  still borrow. Building an owned `Frame` tree with that content is
  tracked as a follow-up to #1099 and is not part of this change. See
  `quadraui/src/frame.rs`'s module doc for the full rationale, and
  `docs/audits/FRAMEWORK_AUDIT_2026-09-26.md`'s "Owned `Frame`/`Surface`
  for bindings" row, which stays **Open** on this same evidence.
  Purely additive to the type signatures (no existing `Surface`/
  `FrameZone` variant, field, or method changed shape). Both enums
  also gain `#[non_exhaustive]` here: adding 14 variants to a
  non-`#[non_exhaustive]` enum is a potential `E0004` (non-exhaustive
  match) break downstream, so this closes that risk for *future*
  variant additions too, per `CLAUDE.md` rule 2. Verified non-breaking
  for both downstream consumers today: `~/src/coord-tui` has zero
  `FrameZone`/`Surface::` hits, and vimcode's only `FrameZone` match
  (`~/src/vimcode/src/click.rs:331-355`) ends in a wildcard `_ => {}`
  arm (grep in the PR body).

### Changed

- `ToastMeasure` (issue #1185) carries toast-local `dismiss_rect:
  Option<Rect>` / `action_rects: Vec<Rect>` instead of the single
  `dismiss_width` / `action_width` widths, and `VisibleToast` gained
  `action_rects: Vec<Rect>` (one entry per action). Both are outputs
  computed by backends/`ToastOverlay::layout`, never struct-literal inputs
  downstream — the blast-radius grep below found no consumer that builds
  either. `VisibleToast::action_bounds` survives (deprecated) for the
  consumers that *read* it.
- `publish = false` removed from `quadraui/Cargo.toml` — `quadraui` is now
  publishable to crates.io. (The actual `v0.1.0` tag and `cargo publish` are
  a separate, coordinator-run release step — see `quadraui#797`.)
- `quadraui/docs/DECISIONS.md` and `quadraui/docs/BACKEND_TRAIT_PROPOSAL.md`
  moved to `quadraui/docs/decisions/` — archived design-history documents,
  separated from the "read on demand" reference docs `CLAUDE.md` points
  contributors at (`ARCHITECTURE.md`, `PRIMITIVE_RULES.md`,
  `CONSUMER_PATTERNS.md`, `TESTING.md`, `LESSONS.md`). All internal links
  updated; no content changed.
- `TextInput`, `Toolbar`, `Editor` are `#[non_exhaustive]` unconditionally
  (issue #1251, phase 2 of the v0.1.0 breaking batch) — the off-by-default
  `strict-descriptors` feature #1108 added to let a consumer opt into
  proving its own migration early is gone; there's nothing left to opt
  into. coord-tui#119 and vimcode#1652 both migrated off exhaustive
  struct literals ahead of this landing; `quadraui/tests/downstream_struct_literals.rs`'s
  guards are inverted (assert the `new`/`with_*` builders cover every field
  each consumer's literal used to set) rather than deleted, since the
  external-literal-still-compiles shape they guarded is now impossible by
  construction.
- `gtk::draw_status_bar`'s `#[deprecated]` attribute (issue #1109) is
  dropped — `vimcode`'s `src/gtk/mod.rs` test helper, its one external
  caller, migrated off it, but `kubeui-gtk/src/main.rs`'s `draw` closure
  (a bare `cr`/`layout` with no `GtkBackend` in scope) has no alternative
  route to the shared paint (`CairoSurface` is `pub(crate)`), so this one
  stays public and un-deprecated rather than following its siblings below
  into `Removed`.

### Fixed

- Example (`file_dialog`, both backends): the `FileDialogDemo` status bar
  no longer loses the confirmed path's filename — or the whole outcome
  message — when the picked path is longer than the bar is wide. The demo
  painted the full absolute path as a *right* `StatusBarSegment`;
  `StatusBar::layout` right-aligns the right group and paints it after
  the left segments, so an over-long right segment is clipped from its
  *tail* (dropping the filename) and, once wider than the bar itself,
  lands at column 0 and blanks the message. Since the path is rooted at
  the backend's `current_dir()` by default, whether that happened
  depended purely on how deep the checkout sat — the demo's own driver
  tests passed in a short checkout and failed in a deep one. The full
  message now lives in a left segment (clipping harmlessly from its
  tail) and only the path's final component goes in the short right
  segment, matching the identical fix already carried by
  `FolderPickerApp::status_bar`. `FileDialogDemo::with_initial_dir` is
  new (mirroring `FolderPickerApp::with_root`) so the regression test can
  root the dialogs in a directory it controls rather than inheriting the
  checkout's location. Examples only — no library behaviour changes, and
  no public quadraui item added, removed or changed, so there is no
  downstream impact.
- TUI: a process whose pty is closed out from under it no longer busy-spins
  forever at ~100% CPU (issue #1295) — crossterm 0.29's unix event source has
  a TTY read loop with no break arm for a bare `Ok(0)` (EOF) read, so once a
  pty's master side closes, every `read()` on the slave returns `Ok(0)`
  forever with no way for the caller to interrupt it once that call is
  entered. `TuiBackend::{poll_events,wait_events}` now run their own
  non-blocking `poll(2)` on stdin first and refuse to delegate into
  crossterm at all once `POLLHUP`/`POLLERR`/`POLLNVAL` is observed;
  `TuiRunner::run_one` turns that into a clean exit via the new
  `input_gone()` (`### Added` above) the same way `Reaction::Exit` already
  does. The same guard also covers `caps::probe_kitty_keyboard`'s own,
  separate crossterm entry point and the nested dialog event loop behind
  `show_file_open_dialog`/`show_file_save_dialog`/`show_message_dialog`.
  This narrowed, rather than eliminated, a real race for a hangup that lands
  while a wait is already in flight — closed by issue #1301 below.
- TUI: the narrower race issue #1295 left open — a pty master closing
  *while* `TuiBackend::wait_events` is already blocked inside a delegated
  crossterm call, rather than strictly before one begins — no longer hangs
  forever either (issue #1301). `wait_events` no longer delegates any
  blocking call to crossterm at all: it blocks on this crate's own
  `poll(2)` call against stdin for the caller's real timeout via the new
  `wait_for_stdin_ready`, and only ever hands crossterm a guaranteed
  non-blocking `Duration::ZERO` call once that call has already confirmed
  readiness with no hangup bit set. `poll(2)` reports `POLLHUP` on a
  hung-up fd the instant it happens, even while already parked waiting on
  it, so the hangup itself is now the wakeup rather than something a
  periodic re-check has to race an already in-flight crossterm call to
  notice. The `STDIN_HANGUP_POLL_SLICE`-sized (20ms) slicing loop this
  replaces is gone entirely. `TuiPlatformServices::next_dialog_events`'s
  nested dialog loop was converted to the same `wait_for_stdin_ready`
  guard for the same reason. No public API change.
- Win-GUI (`WinBackend`) now paints 11 more `ChromePrimitive` rasterisers
  — `Tree`, `List`, `MenuBar`, `ContextMenu`, `CommandCenter`,
  `MultiSectionView`, `SidebarPanel`, `StatusBar`, `ActivityBar`,
  `Toolbar`, `TabBar` — through the chrome (UI) font (`chrome_dwrite`,
  falling back to the editor `dwrite` handle if no live chrome one
  exists yet) instead of the editor font, matching GTK's `ui_font` swap
  (#624) and macOS's `chrome_font` (#1003) (issue #1266). Only `Dialog`
  and `RichTextPopup` (#1077) were wired previously; `chrome_dwrite` had
  sat built-but-unused since #724. A new private `ChromeSurface`
  `NativeSurface` adapter (mirroring `MacBackend::ChromeSurface`) covers
  the two primitives (`StatusBar`, `SidebarPanel`) that paint through the
  shared `native_surface_paint` module rather than a per-backend
  `D2dSurface`. No public API change.
- `terminal_engine::TerminalSession::resize` no longer panics when a
  **width shrink** lands exactly between the two halves of a double-width
  (CJK/emoji) glyph (issue #1130). vt100 0.16.2's `Screen::set_size`
  narrows rows with a plain `Vec` truncation, which drops a wide glyph's
  continuation half while leaving its first half behind; the orphan is a
  landmine that panics upstream both when written over
  (`screen.rs:870`) and when *erased* (`Row::clear_wide`), so it cannot
  be repaired afterwards — including by the `ED`/`EL` that replaying a
  `contents_formatted` dump begins with, which is why the internal
  `reflow_screen` snapshot→resize→replay path was hit as well as the
  plain height-only / alternate-screen one. quadraui now erases such a
  glyph *before* the shrink, while both halves are still addressable.
  Found by this crate's own new vt100 property tests; no public API
  change.
- `TreeController::right_click` on empty tree space below the last row
  (issue #1045 item 4) now emits `TreeControllerEvent::ContextMenuRequested`
  with an empty `path` instead of swallowing the click as plain
  `Consumed` — a host had no portable way to offer a context menu on the
  blank area below the last row at all before this (`vimcode` carried its
  own `route_tree_empty_space_context_menu` shared workaround for exactly
  this gap). A consumer distinguishes the two cases via `path.is_empty()`
  and falls back to a container-level menu (e.g. the tree's root/cwd).
  Additive, not breaking: `TreeControllerEvent`'s variant set is
  unchanged, and every existing row-targeted `ContextMenuRequested`
  handler that resolves via `path.first()` (or equivalent) keeps working
  unchanged, since an empty `path` simply resolves to nothing there.
- `macos::multi_section_view::draw_multi_section_view`'s rustdoc regained
  its `# Safety` section, dropped while its doc comment was being
  rewritten during issue #913. The
  omission is denied by `clippy::missing_safety_doc` under CI's
  `-D warnings`, but `lib.rs` gates `mod macos` on
  `target_os = "macos"`, so only the `macos-latest` runner ever compiles
  it — a blind spot now covered on every leg and every OS by the new
  text-level `quadraui/tests/macos_safety_docs.rs` guard, alongside the
  existing `macos_appkit_features.rs`.
- `Reaction`/`EventOutcome` batch-dispatch merging (issue #832 review
  follow-up): `macos::run::dispatch_event`'s drag-dispatch loops,
  `win::run`'s `route_mouse_down`/`route_mouse_move`/`route_mouse_up`,
  and the `GtkDriver`/`TuiDriver`/`TuiVtDriver` test harnesses' batch
  dispatch used to keep the *first* non-`Continue` outcome seen when
  folding several synthesized events into one result, so a shorter,
  more urgent `RedrawAfter` arriving after a longer one in the same
  batch was silently dropped — a wake later than the app asked for,
  narrower than `Reaction::RedrawAfter`'s own doc promise that the
  backend "never" wakes later. `Reaction::merge`/`EventOutcome::merge`
  now coalesce to the *earliest* deadline instead, mirroring
  `runtime::FrameScheduler::request`; covered by new unit tests
  (`runner::reaction_merge_tests`, `runtime::event_outcome_merge_tests`).
- TUI: a real Escape keypress landing in the same terminal read as an
  adjacent SGR mouse report (motion, click, or drag) could leak the
  report's tail as literal characters into whatever had focus — e.g.
  `[<35;10;5M` typed into a focused `TextInput` — because crossterm's
  reader can split `ESC [ < Cb ; Cx ; Cy (M|m)` right after the leading
  `ESC` and then decode the rest byte-by-byte as ordinary text (#293).
  `TuiBackend::poll_events`/`wait_events` now reconstitute any such
  leaked report back into the mouse event it should have decoded as
  before dispatch ever sees it; the standalone `Escape` is preserved.
- Embedded terminal: faint/dim text (SGR 2, e.g. claude's autosuggestion
  ghost-text) now renders visually distinct instead of at full
  brightness (#345). `terminal_engine::TerminalSession::to_terminal`
  reads vt100's `Cell::dim()` into the new `TerminalCell::dim` field;
  `terminal_style::resolve_cell_style` — the one place every backend
  (tui/gtk/macos/win) resolves a cell's paint colours — blends a dim
  cell's foreground 50% toward its resolved background. No new
  `Backend`/`NativeSurface` surface area: faint has no font-weight
  equivalent, so it's folded into colour resolution rather than added
  as a fourth per-backend text-run attribute alongside bold/italic/
  underline.
- TUI: `draw_split`/`draw_split_tree` painted plain `'│'`/`'─'` runs with
  no glyph where two dividers met or crossed (#1067). Both now read a
  run's perpendicular neighbour cells back out of the buffer (new
  private `tui::split_junction` module, shared by both rasterisers) and
  upgrade the run's ends — and any cell where it crosses an
  already-painted perpendicular divider — to the matching box-drawing
  junction glyph (`┼ ├ ┤ ┬ ┴`), so a cell-grid consumer no longer needs
  its own divider rasteriser to get junctions right (vimcode's own
  ~350-line `render_impl.rs` divider pass did this by hand). No
  descriptor/signature change — `SplitLayout`/`SplitTreeLayout` and
  hit-testing are unaffected; `draw_split_tree` paints every divider's
  plain glyph first and upgrades junctions in a second pass, since its
  pre-order traversal always visits the full-extent divider before the
  shorter ones it crosses. The read-back clips exactly where `set_cell`
  does: a caller's rounded divider geometry can put a run's last cell
  one column/row outside the buffer area, and those cells are skipped
  rather than indexed (which panics) — the last *visible* cell of a
  clipped run keeps its plain axis glyph.

### Deprecated

- `Backend::draw_status_bar` (issue #819) — both known consumers migrated
  off this positional hover/pressed shim ahead of issue #1251 (#1109's
  "Remaining deprecated items" list), and every one of its #1251 siblings
  (`draw_toolbar`, `draw_sidebar_panel`, `ShellApp::on_shell_event`) was
  removed outright once that was confirmed. This one stays deprecated
  instead, because the sealed milestone acceptance slices
  `tests/acceptance/ms-11/{structural_parity,c0_paint_smoke}.rs` still
  call it positionally and no PR may edit anything under
  `tests/acceptance/` (see that method's own doc). Removal target:
  whichever Gate A pass migrates those two slices off the positional
  call, not before v0.2.0.
- `primitives::tab_bar::TabBarHits` — the f64-tuple pre-D6 hit struct still
  returned by `Backend::draw_tab_bar` / `draw_tab_bar_icons` /
  `draw_tab_bar_with_chrome` / `tab_bar_layout` / `tab_bar_layout_icons` /
  `tab_bar_layout_with_chrome`. Issue #1251 confirmed both known consumers
  have zero remaining code-level uses of this type, but did not remove it
  (unlike every other item on #1109's "Remaining deprecated items" list):
  its removal is blocked on this crate's own unstarted six-method/four-
  backend signature swap to `TabBarLayout` (already real, already
  `Rect`/`TabBarHit`-based, already what every in-tree rasteriser computes
  before narrowing to `TabBarHits`), not a lagging consumer migration —
  see `primitives/tab_bar.rs`'s `TabBarHits` doc for why (two of the four
  backends construct it with no intermediate `TabBarLayout`). Tracked in
  #823. Removal target: the PR that lands that swap, not before v0.2.0.

### Removed

- `primitives::status_bar::StatusBar::{hit_regions, hit_regions_fit_chars,
  resolve_click_fit_chars}` — pre-D6 char-column hit-testing helpers
  (#823). Replacement: `StatusBar::layout()` + `StatusBarLayout::hit_test()`,
  which already applies the same priority-drop policy and returns the
  crate's `Rect` + `Hit`-enum convention instead of raw `u16` columns.
  Issue #1109 found zero uses of all three in coord-tui's `main` or
  vimcode's `develop`; `StatusBar::resolve_click` (not itself deprecated)
  kept its public signature, with the `hit_regions` logic it depended on
  inlined as a private helper.
- `primitives::multi_section_view::LayoutMetrics` — the pre-#822
  `#[deprecated]` `pub type` alias for `MsvLayoutMetrics` (the crate-root
  export was already using the new name). Issue #1109 found zero uses in
  coord-tui's `main` or vimcode's `develop`.
- `primitives::tooltip::Tooltip::{with_styled_lines, with_placement,
  with_bg, with_fg}` and `primitives::tooltip::TooltipChrome::{with_border,
  with_title}` — `with_*` builder sprawl (#824): every one of these fields
  is already `pub`, so the builder was sugar around a field write, not
  something guarding an invariant. Replacement: set the field directly.
  Issue #1109 found zero uses of any of the six in coord-tui's `main` or
  vimcode's `develop`.
- `VisibleToast::action_bounds` (issue #1185's single-action mirror field)
  — zero uses in coord-tui's `main` or vimcode's `develop` (issue #1109);
  the `src/gtk/testing.rs` read this file's #1185 entry once attributed to
  vimcode no longer exists there. Replacement: `VisibleToast::action_rects`.
- `primitives::minimap::Minimap::layout` and `primitives::minimap::
  sample_lines` — the pre-#667 two-argument `layout()` shim (defaulted to
  `MinimapSizing::Fill`) and the pre-#1012 point-sampler `sample_lines`.
  Issue #1109 found zero uses of either in coord-tui's `main` or vimcode's
  `develop` — the doc comments claiming a live `vimcode::src/render.rs::
  minimap_click_line` dependency on the 2-arg `layout()` were stale; that
  function now calls `layout_with_sizing` exclusively. Replacements:
  `Minimap::layout_with_sizing`, `Minimap::sample_blocks`.
- `Backend::tab_bar_layout_to_hits` and `EditorPaintResult::cursor_position`
  — issue #1109 found zero uses of either in coord-tui's `main` or
  vimcode's `develop`: the former's only vimcode hit was a doc comment
  (`src/gtk/testing.rs`), and the latter's `src/tui_main/render_impl.rs`
  call site this field was kept populated for no longer exists. Renamed
  to `tab_bar_hits_from_layout` (#504); replacement:
  `EditorPaintResult::cursor_position_native` (#504).
- Every per-backend free-function `draw_*` paint shim deprecated across
  issues #808/#811/#859/#860/#861/#862/#863/#864/#865/#866/#1072/#1085
  (one per primitive per backend, over the shared `CgSurface`/
  `D2dSurface`/`CairoSurface` adapters) — `macos::{draw_board, draw_panel,
  draw_split, draw_split_tree, draw_pipeline_view, draw_toast_stack,
  draw_scrollbar, draw_progress, draw_drop_overlay, draw_form,
  draw_diff_view, draw_status_bar}`, the identical `win::` set, and the
  identical `gtk::` set **except** `gtk::draw_status_bar` (issue #1109):
  that one has a real consumer (vimcode's `src/gtk/mod.rs` calls
  `quadraui::gtk::draw_status_bar` directly in a test helper), so it
  stayed deprecated pending that migration; every other free function in
  the list had zero uses in coord-tui's `main` and vimcode's `develop`.
  Replacement for all of them: the corresponding `Backend::draw_*` trait
  method, which every in-tree call site already used. (`gtk::draw_status_bar`
  itself: once that migration landed, issue #1251 dropped its
  `#[deprecated]` attribute rather than removing it too — see the
  `Changed` entry above for why `kubeui-gtk` keeps it un-deprecated and
  public.)
- `compose::key_map::{KeyMap, KeyContext}` (#473) — the "one convention
  #10" adopt-or-demote pass (#825) found zero constructors anywhere: no
  hit in this crate's own examples or tests beyond its own unit-test
  module, and the issue's own audit already recorded zero adopters in
  `coord-tui`/`vimcode`. Per `docs/PRIMITIVE_RULES.md` rule 8 ("zero hits
  in both plus no in-tree use ⇒ remove it outright"), no
  `#[deprecated]` shim was needed — this is a straight removal, not a
  two-PR deprecation. The type, its docs, and its full test suite move
  unchanged to `examples/common/key_map.rs` as a copy-paste recipe; see
  that file's module doc to promote it back if a consumer appears. The
  other six #825 candidates (`FocusRing`, `Spinner`, `ProgressBar`,
  `FolderPickerController`, `BottomPanelController`,
  `TabGroupController`) each keep their public API — see the PR
  description for each one's recorded disposition.
- `primitives::toast::{ToastStack, ToastItem, ToastAction}` and
  `Backend::draw_toast_stack` (issue #1251, PR 2 of the #1185
  deprecate-then-remove pair the `Deprecated` section above used to list)
  — coord-tui#119 migrated its last call sites (`src/app/mod.rs`'s two
  `ToastItem` construction sites, `src/app/render.rs`'s
  `draw_toast_stack` call) onto `Toast`/`ToastOverlay`/`ToastButton`/
  `Backend::draw_toast_overlay`; `vimcode` had zero uses already.
  Replacements unchanged: `Toast`, `ToastOverlay`, `ToastButton`,
  `Backend::draw_toast_overlay`.
- `primitives::editor::StyledSpan` (renamed to `EditorStyledSpan`, issue
  #822) and `primitives::minimap::SyntaxSpan` (merged into `MinimapSpan`,
  issue #822) — PR 2 shim removals; both consumers had zero remaining
  uses by the time coord-tui#119 / vimcode#1652 merged.
- `ShellApp::on_shell_event` (the ctx-less panel-switch hook, issue #617)
  — `on_shell_event_ctx`'s default implementation forwarded to this;
  coord-tui#119 was its last override (moved the routing logic to a
  plain `CoordApp::route_panel_changed` method `on_shell_event_ctx` now
  calls directly), so the default is a plain no-op now. Replacement:
  implement `ShellApp::on_shell_event_ctx` instead.
- `Backend::draw_toolbar` and `Backend::draw_sidebar_panel` (the
  positional hover/pressed shims, issue #819) — both coord-tui#119 and
  vimcode#1652 moved to `draw_toolbar_interactive`/
  `draw_toolbar_with_options` and `draw_sidebar_panel_interactive`.
  `Backend::draw_status_bar`, the third sibling in this #819 family,
  stays — see the `Deprecated` section above for why.
- `strict-descriptors` cargo feature (issue #1251) — see the `Changed`
  entry above; `TextInput`/`Toolbar`/`Editor` are `#[non_exhaustive]`
  unconditionally now, so there is nothing left for the feature to gate.
