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

- Bundled Microsoft's codicon icon font (CC-BY-4.0; license text ships at
  `quadraui/assets/CODICON_LICENSE`) and self-register it on every GUI
  backend with no app configuration required (issue #1377). Tree
  expand/collapse chevrons, tab dirty/close marks, the context-menu
  submenu arrow, and (GTK) data-table sort arrows now paint as codicon
  glyphs on GTK, macOS and Windows instead of plain text characters; TUI
  output is unchanged.

### Changed

- `GtkBackend`/`MacBackend`/`WinBackend` now default `nerd_fonts_enabled`
  to `true` (issue #1377) — the bundled codicon font removes the
  "Nerd Font not installed" risk that kept every backend defaulting to
  `false`, so an app's own `Icon::glyph` paints with no
  `Backend::set_nerd_fonts` call. TUI's default is unchanged (`false`).
  An app that needs the old default can call `set_nerd_fonts(false)`
  itself.

### Fixed

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
