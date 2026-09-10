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

Releases are tagged `vX.Y.Z` against `main`. Tagging is a coordinator release
action (see `quadraui#797`), not something an individual PR does — a PR adds
its entry under `[Unreleased]`, and the coordinator retitles that section to
`[X.Y.Z] - YYYY-MM-DD` (adding a fresh empty `[Unreleased]` above it) at
release time.

## [Unreleased]

### Added

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

### Changed

- `publish = false` removed from `quadraui/Cargo.toml` — `quadraui` is now
  publishable to crates.io. (The actual `v0.1.0` tag and `cargo publish` are
  a separate, coordinator-run release step — see `quadraui#797`.)
- `quadraui/docs/DECISIONS.md` and `quadraui/docs/BACKEND_TRAIT_PROPOSAL.md`
  moved to `quadraui/docs/decisions/` — archived design-history documents,
  separated from the "read on demand" reference docs `CLAUDE.md` points
  contributors at (`ARCHITECTURE.md`, `PRIMITIVE_RULES.md`,
  `CONSUMER_PATTERNS.md`, `TESTING.md`, `LESSONS.md`). All internal links
  updated; no content changed.

### Fixed

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

### Deprecated

- `primitives::status_bar::StatusBar::hit_regions` and
  `hit_regions_fit_chars` — pre-D6 char-column hit-testing helpers.
  Replacement: `StatusBar::layout()` + `StatusBarLayout::hit_test()`, which
  already applies the same priority-drop policy and returns the crate's
  `Rect` + `Hit`-enum convention instead of raw `u16` columns. Tracked in
  #823.
- `primitives::status_bar::StatusBar::resolve_click_fit_chars` — same
  replacement as above (`StatusBar::layout()` + `StatusBarLayout::hit_test()`).
  Tracked in #823.
- `primitives::tab_bar::TabBarHits` — the f64-tuple pre-D6 hit struct still
  returned by `Backend::draw_tab_bar` / `draw_tab_bar_icons` /
  `draw_tab_bar_with_chrome` / `tab_bar_layout` / `tab_bar_layout_icons` /
  `tab_bar_layout_with_chrome`. This PR is the shim step only — it marks the
  struct `#[deprecated]` without changing any of those six methods'
  signatures; the eventual replacement is `TabBarLayout` (already real,
  already `Rect`/`TabBarHit`-based, already what every in-tree rasteriser
  computes before narrowing to `TabBarHits`). The six-method/four-backend
  signature swap is separate, larger follow-up work — see
  `primitives/tab_bar.rs`'s `TabBarHits` doc for why (two of the four
  backends construct it with no intermediate `TabBarLayout`). Tracked in
  #823; matching `vimcode` consumer-migration issue to be filed alongside
  this PR.
- `primitives::editor::StyledSpan` — renamed to `EditorStyledSpan` (the
  crate-root export was already using this name) to resolve a bare-name
  clash with the unrelated `types::StyledSpan`. Old name kept as a
  `#[deprecated]` `pub type` alias in `editor.rs`. PR 2 (shim removal),
  tracked in #822.
- `primitives::minimap::SyntaxSpan` — merged into the byte-identical
  `MinimapSpan` (both were the same four-field struct, distinguished only
  by which side of `aggregate_spans` produced them). Old name kept as a
  `#[deprecated]` `pub type` alias, still re-exported at the crate root
  behind `#[allow(deprecated)]`. PR 2 (shim removal), tracked in #822.
- `primitives::multi_section_view::LayoutMetrics` — renamed to
  `MsvLayoutMetrics` (the crate-root export was already using this name)
  to resolve a bare-name clash that read as though it belonged to the
  unrelated `primitives::layout_metrics` module. Old name kept as a
  `#[deprecated]` `pub type` alias. PR 2 (shim removal), tracked in #822.
- `primitives::tooltip::Tooltip::{with_styled_lines, with_placement,
  with_bg, with_fg}` and `primitives::tooltip::TooltipChrome::{with_border,
  with_title}` — `with_*` builder sprawl (#824): every one of these fields
  is already `pub`, so the builder was sugar around a field write, not
  something guarding an invariant. Replacement: set the field directly.
  `TooltipChrome` is already `#[non_exhaustive]` + `Default`, so it already
  *is* the options-struct shape #824 asks for; `Tooltip` stays a plain,
  non-`#[non_exhaustive]` struct (per its module doc's exhaustive-literal
  reasoning), so its fields are set the same way. Old methods kept behind
  `#[deprecated]`, unchanged in behaviour. PR 2 (shim removal), tracked in
  #824.

### Removed

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
