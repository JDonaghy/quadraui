# Session History

## Session 2026-05-01 — Cross-backend portability arc

**Agent:** Claude Opus 4.7 (1M context)

### Issues closed (9)

| # | Title | Path | Key deliverable |
|---|---|---|---|
| 1 | MSV Debug-sidebar consumer pattern | B (PR #10) | 5 consumer-state round-trip tests, `examples/msv_multi_tree.rs`, CLAUDE.md Consumer patterns |
| 9 | MSV per-section scrollbar paint + track-page hit regions | A | `thumb_bounds` on `SectionLayout`, `fit_thumb`-based thumb position, 3 painted-indicator tests, natural-max clamp |
| 2 | MSV SC-panel consumer pattern | A | `examples/msv_sc_panel.rs`, 6 SC consumer-state tests (input keystroke, chevron toggle, collapse semantics) |
| 3 | GTK MSV paint↔click round-trip harness | A | `ImageSurface` + pixel-scan harness pattern, `gtk_msv_layout` rename, 4 GTK MSV tests |
| 4 | GTK TreeView paint↔click round-trip harness | A | `gtk_tree_layout` extracted, 4 GTK tree tests (mixed-decoration row pitch) |
| 5 | Promote 'harness is the gate' rule in CLAUDE.md | closed as superseded | Covered by Rule #6, Coverage taxonomy, Backend testability requirement |
| 12 | GTK twin of msv_multi_tree | A | `examples/gtk_multi_tree.rs`, proportional drag, active-section indicator |
| 13 | Backend trait coverage gap | B (PR #15) | 9 new trait methods (7 draw + msv_layout + tree_layout), `EditorPaintResult`, `Backend::line_height` |
| 14 | Shared AppLogic refactor | A | `examples/common/multi_tree.rs`, thin runner shells (~24 lines each), `EventControllerMotion` in `gtk::run` |

### Issues filed (4)

| # | Title | Status |
|---|---|---|
| 9 | MSV scrollbar paint + track-page | closed in-session |
| 11 | SC sidebar focus-restore after Esc | open |
| 12 | gtk_multi_tree example | closed in-session |
| 13 | Backend trait coverage gap | closed in-session |
| 14 | Shared AppLogic refactor | closed in-session |

### CLAUDE.md sections added/updated

- **Cross-backend portability commitment** (new) — 6 sub-rules for "Windows/macOS for free" goal.
- **Primitive Authoring Rule #6** — test state-derived paint geometry.
- **Primitive Authoring Rule #7** — every primitive on Backend trait.
- **Coverage taxonomy** (new) — 3 bug classes, 3 test shapes, where they live.
- **Backend testability requirement** (new) — headless paint-to-memory per backend.
- **Consumer patterns: Debug-sidebar** — updated with scrollbar routing (TrackBefore/Thumb/TrackAfter), natural-max clamp, shared AppLogic pointer.
- **Consumer patterns: SC panel** (new) — aux=Input, collapsible sections, keystroke routing, collapsed semantics.
- **Development Workflow** (new) — branch-from-develop, Path A/B, smoke test, issue creation discipline.
- **Lessons captured** — shared AppLogic unit-mismatch bug, runner event-variant coverage gap.

### Test count progression

| Checkpoint | Lib tests |
|---|---|
| Session start | 307 |
| After #1 | 311 |
| After #9 | 311 |
| After #2 | 317 |
| After #3 | 338 |
| After #4 | 342 |
| Session end | 342 |

### Architectural decisions made

1. **Option (i) for scrollbar scroll_offset**: primitive introspects `SectionBody::Tree(t).scroll_offset` directly; no host-supplied measure threading.
2. **Backend::line_height()** on the trait: TUI returns 1.0 (cell), GTK returns Pango-resolved pixels. Portable sizing for shared AppLogic code.
3. **EditorPaintResult on the trait**: TUI populates cursor_position; GTK returns default. Trait shape is symmetric; data asymmetry is documented.
4. **Interior-pixel-scan** for GTK harness: skip 1px AA boundary on each edge; assert at interior pixels only. Documented in test comments.

### Open queue for next session

*Resolved in session 2026-05-01b below.*

## Session 2026-05-01b — MenuBar + Split rasterisers, primitive audit

**Agent:** Claude Opus 4.6 (1M context)

### Issues closed (5)

| # | Title | Path | Key deliverable |
|---|---|---|---|
| 11 | SC sidebar focus-restore after Esc | A | `previous_active` field, 2 consumer-state tests (mutation-verified) |
| 8 | Audit primitives without rasterisers | A | All 5 kept (real descriptors, not stubs). CLAUDE.md "Primitive maturity levels" rule added |
| 6 | TUI + GTK rasterisers for MenuBar | A | `draw_menu_bar` + `tui_menu_bar_layout` / `gtk_menu_bar_layout`, Backend `draw_menu_bar` + `menu_bar_layout`, 5 TUI tests (mutation-verified), shared `MenuBarApp` + runner shells |
| 18 | MenuBar: complete menu experience | A | Dropdown via ContextMenu composition, hover-to-switch, keyboard nav (Alt+key, arrows, Enter, Esc), realistic File/Edit/View menus |
| 17 | MenuBar: hover + dropdown (premature split) | closed | Superseded by #18 |

### Issues filed (3)

| # | Title | Status |
|---|---|---|
| 16 | Rasterisers for descriptor-only primitives (umbrella) | open (split done, 4 remain) |
| 17 | MenuBar hover + dropdown | closed (superseded by #18) |
| 18 | MenuBar complete menu experience | closed in-session |

### Split primitive shipped (first of #16's 5)

- TUI rasteriser: `draw_split` + `tui_split_layout`, 4 paint↔click tests (mutation-verified)
- GTK rasteriser: `draw_split` + `gtk_split_layout` via Cairo
- Backend trait: `draw_split` + `split_layout`
- Shared `SplitApp` + `tui_split` / `gtk_split` runner shells

### CLAUDE.md sections added/updated

- **Primitive maturity levels** (new) — descriptors vs shipped; don't delete descriptors, prioritise rasterisers for vimcode adoption.
- **Lessons: Backend `_layout` methods must work outside GTK frame scope** (new) — use stored metrics, not pango handles.
- **Lessons: Real apps need layout caching, not layout re-derivation** (new) — cache the layout paint produced on host state; read it in click. Distinguishes from Cell-smuggling anti-pattern.
- **The band-aid trap** (updated) — now documents two safe patterns (re-derivation when inputs match vs layout caching when they don't).
- **What NOT to do** (rewritten) — replaces overly broad Cell-smuggling warning with precise distinction: caching layout *outputs* is safe, caching *inputs* to bridge two independent derivations is not.
- **Consumer patterns: MSV click routing** (updated) — references both safe patterns.

### Test count progression

| Checkpoint | Lib tests |
|---|---|
| Session start | 342 |
| After #11 | 344 |
| After #6 (MenuBar) | 349 |
| After Split | 353 |
| Session end | 353 |

### Bugs found + fixed during session

1. **GTK menu bar click drift**: example's `handle()` used a hand-rolled char-count measurer for click routing that didn't match GTK's Pango pixel-width measurer in paint. Fixed by adding `Backend::menu_bar_layout` so click handlers use the same measurer as the painter. Lesson captured in CLAUDE.md.
2. **GTK `menu_bar_layout` panic**: `current_frame_refs()` called from `handle()` which runs outside GTK's draw callback. Fixed by using `current_char_width` instead of pango. Lesson captured.
3. **TUI dropdown border clipping**: ContextMenu's 1-cell border extended above/left of `layout.bounds`, overwriting the menu bar row and clipping at x=0. Fixed by padding the anchor rect by `line_height`.
4. **GTK tree row bleed into next section header**: no per-body Cairo clip in GTK MSV rasteriser. Fixed by adding `cr.save()/clip()/restore()` around each body paint.
5. **GTK compressed last row**: tree layout clips last row to fractional height; rasteriser painted compressed background band. Fixed by skipping rows whose height < full row height.
6. **GTK sub-pixel text blur**: text y-positions in tree rasteriser and MSV header were fractional pixels, causing anti-aliasing smear. Fixed by `.round()` on all `cr.move_to` y-coordinates.
7. **GTK header text descender clipping**: "CALL STACK" header text bottom-clipped. Fixed by increasing tree header row from 1.0x to 1.2x line_height and biasing MSV header text centering to 0.4 (giving descenders 60% of slack).
8. **TUI `q_to_tui_rect` rounding**: `round()` on body height could give rasteriser more cells than available. Fixed by switching to `floor()` (defensive — cell_quantum already snaps to integers).

### Open queue for next session

*Resolved in session 2026-05-03/04 below.*

## Session 2026-05-03/04 — Primitive completion + vimcode migration support

**Agent:** Claude Opus 4.6 (1M context)

### Issues closed (8)

| # | Title | Path | Key deliverable |
|---|---|---|---|
| 16 | Rasterisers for descriptor-only primitives (umbrella) | A | Panel, toast, progress, spinner — all shipped with TUI+GTK rasterisers, Backend trait methods, paint↔click harnesses, paired examples. CLAUDE.md "Primitive maturity levels" updated: zero descriptors-only remaining. |
| 7 | SearchPanel primitive spike | A | MSV+TreeView composition validated — no new primitive needed. `tui_search_panel`/`gtk_search_panel` examples with aux=Search input, file-grouped TreeView results, click-to-jump routing. |
| 46 | ScrollableLog primitive | A | Extended `TextDisplay` with `show_scrollbar: bool` instead of new primitive. Layout reserves scrollbar gutter, computes thumb via `fit_thumb`, emits ScrollbarThumb/TrackBefore/TrackAfter hit regions. 4 new tests. |
| 47 | Backend-independent scroll dispatch | A | `dispatch_scroll()` — modal-aware wheel event routing through registered `ScrollSurface` entries. 6 tests. |
| 48 | Scrollbar click-to-page and thumb-drag | A | `dispatch_click()` — supersedes `dispatch_mouse_down` for consumers with scroll surfaces. Auto-starts `DragTarget::ScrollbarY` on thumb click, emits `ScrollOffsetChanged` on track click. 6 tests. |
| 49 | GTK tab bar height hardcoded | A | Added `row_height: f64` parameter to GTK `draw_tab_bar`, `GtkBackend` forwards `rect.height`. |
| 50 | GTK tab bar compact mode | A | Added `compact: bool` to `TabBar` — 2px padding + 0px gap (vs 14px + 1px) for compact chrome. |
| 51 | CommandCenter primitive | A | New primitive: back/forward nav arrows + search box. TUI+GTK rasterisers, Backend trait methods, 5 paint↔click tests. |

### Issues filed (26)

Windows Backend milestone (#3): #19–#31 (13 issues)
macOS Backend milestone (#4): #32–#44 (13 issues)

### Primitives shipped (6 new, 1 extended)

| Primitive | Type | Consumer lines eliminated |
|---|---|---|
| Panel | New rasterisers (#16) | vimcode terminal panels, sidebar subsections |
| Toast (ToastStack) | New rasterisers (#16) | vimcode notification toasts |
| Progress (ProgressBar) | New rasterisers (#16) | Mason install progress, git operations |
| Spinner | New rasterisers (#16) | LSP boot, git clone spinners |
| TextDisplay + scrollbar | Extended (#46) | vimcode debug output panel |
| CommandCenter | New primitive (#51) | vimcode menu bar nav arrows + search box (~184 lines) |

### Infrastructure shipped

| Feature | Scope |
|---|---|
| `dispatch_scroll` | Modal-aware wheel event routing through `ScrollSurface` entries |
| `dispatch_click` | Extends click dispatch with scrollbar thumb-drag + track-page |
| `ScrollSurface` + `SurfaceScrollbar` | Paint-time surface registration for dispatch |
| TabBar `compact` mode | GTK compact spacing for toolbar tabs |
| TabBar `row_height` | GTK respects caller's rect height |

### Test count progression

| Checkpoint | Lib tests |
|---|---|
| Session start | 353 |
| After Panel (#16) | 360 |
| After Toast (#16) | 367 |
| After Progress+Spinner (#16) | 376 |
| After TextDisplay scrollbar (#46) | 380 |
| After dispatch_scroll (#47) | 397 |
| After dispatch_click (#48) | 403 |
| After TabBar compact (#50) | 403 |
| After CommandCenter (#51) | 408 |

### Bugs found + fixed

1. **Indicators cancel button advancing indeterminate pulse**: `spinner_frame += 1` ran unconditionally on every MouseDown in the indicators example. Fixed by only incrementing on body clicks, and making cancel exit indeterminate mode.
2. **TextDisplay scrollbar theme colours**: initial impl used `muted_fg`/`foreground` for track/thumb. Fixed to use `scrollbar_track`/`scrollbar_thumb` theme fields.

### CLAUDE.md updates

- **Primitive maturity levels**: updated to reflect zero descriptors-only remaining (all 5 shipped).

### Open queue for next session

*Resolved in session 2026-05-06c below.*

## Session 2026-05-06c — SidebarSystem selection nav + GTK menu fixes

**Agent:** Claude Opus 4.6 (1M context)

### Issues closed (3)

| # | Title | Path | Key deliverable |
|---|---|---|---|
| 68 | SidebarSystem: selection navigation mode | A | `NavigationMode::Selection` — Up/Down/j/k move `selected_path` with scroll-to-follow; Home/End/PageUp/PageDown; Enter → `RowActivated`. 17 new tests. |
| 69 | GtkBackend viewport 0x0 on menu open | A | `MenuOverlay` click/motion handlers call `begin_frame` with DA allocation before `menu_system.handle()`. |
| 70 | Menu dropdown descender clipping on highlight change | A | Split `draw_context_menu` into bg pass (separators + highlights) then text pass (labels + detail). |

### Test count progression

| Checkpoint | Lib tests |
|---|---|
| Session start | 452 |
| After #68 | 466 |
| After #69 | 466 |
| After #70 | 466 |

### Bugs found + fixed

1. **Viewport 0x0 on menu open** (#69): `GtkBackend.viewport` was never set outside the draw_func, so `MenuSystem::open_menu()` computed dropdown bounds against a zero-height viewport. Fixed by calling `begin_frame` in click/motion handlers.
2. **Context menu descender clipping** (#70): interleaved bg+text draw loop let a later item's highlight rectangle overwrite the previous item's text descenders. Fixed by two-pass rendering.

### Worktree tooling failure noted

Claude Code worktrees created from a 77-commit-stale `develop` instead of the tip. Wasted implementation on a non-existent codebase state. Discarded and reimplemented directly on real `develop`. Not a code bug — a tooling issue with worktree creation.

### Open queue for next session

*Resolved in session 2026-05-07 below.*

## Session 2026-05-07 — TreeController + FocusGroup compose helpers

**Agent:** Claude Opus 4.6 (1M context)

### Context

Reviewed the SidebarSystem abstraction (#63, #68) and what vimcode's agent did with it. Identified that the keyboard-navigation + scroll-to-follow state machine inside SidebarSystem was TreeView-generic — methods like `move_selection_by`, `scroll_to_visible`, `jump_to_edge`, `activate_selection` didn't use any MSV-specific state. Extracted these into reusable compose helpers.

### Issues closed (2)

| # | Title | Path | Key deliverable |
|---|---|---|---|
| 71 | TreeController: compose helper for single-TreeView keyboard navigation | B (PR #75) | Standalone compose controller for a single keyboard-navigable TreeView with scrollbar. Keyboard nav (Up/Down/j/k, Home/End, PageUp/PageDown, Enter), click hit-testing, scrollbar thumb drag, scroll wheel. `vim_keys` flag (default true) to disable j/k. SidebarSystem refactored to use `Vec<TreeController>` internally, replacing 3 parallel vectors. |
| 72 | FocusGroup: tiny helper for Tab-cycling between focusable regions | B (PR #76) | Index-based Tab/Shift+Tab cycling with wrap-around. Starts unfocused (None), supports dynamic count with clamping. SidebarSystem refactored to use `FocusGroup` replacing hand-rolled `active_section` + `cycle_active`. |

### Issues filed (2)

| # | Title | Status |
|---|---|---|
| 71 | TreeController | closed (PR #75) |
| 72 | FocusGroup | closed (PR #76) |

### Compose helpers shipped (2)

| Helper | File | Lines | Consumer lines eliminated |
|---|---|---|---|
| `TreeController` | `compose/tree_controller.rs` | 667 | ~300 per standalone tree consumer (file picker, search results, vimcode explorer panel) |
| `FocusGroup` | `compose/focus_group.rs` | 155 | ~15 per Tab-cycling consumer (panel layouts, dialog tab order) |

### SidebarSystem refactoring

SidebarSystem was refactored in two stages:
1. **PR #75:** Replaced 3 parallel vectors (`rows`, `scroll_offsets`, `selected_paths`) with `Vec<TreeController>`. Navigation logic delegates to TreeController's pub primitives.
2. **PR #76:** Replaced `active_section: Option<usize>` + `cycle_active()` with `FocusGroup` field.

Net effect: SidebarSystem is now a thin MSV-level orchestrator over N TreeControllers + a FocusGroup, plus Tab cycling, collapse state, two-layer click dispatch, and `build_view()`.

### Test count progression

| Checkpoint | Lib tests |
|---|---|
| Session start | 466 |
| After #71 (TreeController) | 486 |
| After #72 (FocusGroup) | 497 |

### Design decisions

1. **`TreeControllerEvent` naming** (not `TreeEvent`): `TreeEvent` already exists in `primitives::tree` for raw tree events. The compose helper's semantic event needed a distinct name.
2. **`vim_keys` flag** (default true): j/k bindings are opt-out rather than opt-in, matching SidebarSystem's existing behavior. Consumers wanting fully custom key dispatch call the pub navigation primitives directly.
3. **Scrollbar track width** = `backend.line_height()`: 1 cell on TUI (matches MSV's 1-cell scrollbar), proportional on GTK. Simplest portable heuristic without adding a new Backend method.
4. **FocusGroup vs FocusRing**: FocusRing is WidgetId-based and always starts focused. FocusGroup is index-based, starts unfocused (None), and supports None→first/last on first cycle. Different use cases, no overlap.

### Open queue for next session

*Resolved in session 2026-05-09/10 below.*

## Session 2026-05-09/10 — SidebarSystem Form sections + Form field kinds

**Agent:** Claude Opus 4.6 (1M context)

### Issues closed (7)

| # | Title | Path | Key deliverable |
|---|---|---|---|
| 105 | SidebarSystem: support Form sections | B (PR #106) | `SectionKind` enum, `FormController`, `SidebarEvent::FormEvent`, `build_view` branches on kind, mixed Form+Tree sidebar. 13 unit tests + 2 TUI round-trip smoke tests. |
| 107 | Form click emits ButtonClicked for all field types | B (PR #108) | `form_click_event()` inspects `FieldKind` to emit correct event (Toggle→ToggleChanged, Button→ButtonClicked, etc.). 6 tests. |
| 109 | GTK form: no cursor drawn on empty TextInput | B (PR #111) | Removed `!value.is_empty()` guard on cursor drawing. |
| 110 | Header row click selects first child | B (PR #111, #113) | GTK `body_measure` used 1.0× line_height for headers and unrounded item_h. Fixed to `(line_height * 1.2).round()` and `(line_height * 1.4).round()`, matching `gtk_tree_layout`. GTK round-trip test with mixed Header/Normal rows. |
| 112 | Form click generic measurement — items not individually clickable | B (PR #114) | `form_field_measure()` populates per-item hit regions for ToggleGroup/ButtonRow. `handle()` uses `backend.form_layout()` for pixel-accurate GTK hit-test. `sidebar_search` manual smoke test example. |
| 116 | handle_cached Form hit region drift | B (PR #117) | `cache_form_layouts()` pre-computes form layouts using backend measurement. `handle_cached()` checks cache before fallback estimate. |
| 53 | Form: additional field kinds | B (PR #119) | `FieldKind::TextArea`, `PasswordInput`, `SegmentedControl` + `ValidationState`. TUI+GTK rasterisers, layout measurement, SidebarSystem click dispatch. 7 new TUI tests. |

### Issues filed (4)

| # | Title | Status |
|---|---|---|
| 115 | quadraui-lua: reusable Lua app bridge crate | open (tracking) |
| 118 | quadraui-ipc: language-agnostic JSON bridge | open (tracking) |
| 120 | sidebar_search example: cursor movement + clipboard | open |

### New primitives/features shipped

| Feature | Files | Description |
|---|---|---|
| `SectionKind::Form` | compose/sidebar_system.rs, compose/form_controller.rs | SidebarSystem manages Form sections alongside Tree sections |
| `FieldKind::TextArea` | primitives/form.rs, tui/form.rs, gtk/form.rs | Multi-line text editing with `visible_rows` height hint |
| `FieldKind::PasswordInput` | primitives/form.rs, tui/form.rs, gtk/form.rs | Masked single-line input with configurable `mask_char` |
| `FieldKind::SegmentedControl` | primitives/form.rs, tui/form.rs, gtk/form.rs | Horizontal exclusive-choice selector `[opt1\|opt2\|opt3]` |
| `ValidationState` | primitives/form.rs | Error/Warning indicator on any FormField |
| `FormEvent::SegmentedControlChanged` | primitives/form.rs | New event for segmented control selection |
| `SidebarSystem::cache_form_layouts` | compose/sidebar_system.rs | Pre-compute form layouts for handle_cached path |

### New example

- `sidebar_search` (`tui_sidebar_search` / `gtk_sidebar_search`) — SidebarSystem with Form section (TextInput, ToggleGroup, SegmentedControl, PasswordInput, ValidationState) above a Tree section with Header-decorated rows. Manual smoke test for #105, #110, #112, #53.

### Bugs found + fixed

1. **Form click always emits ButtonClicked** (#107): `form_click_event` didn't inspect `FieldKind`. Fixed with per-kind dispatch.
2. **GTK empty TextInput cursor** (#109): `!value.is_empty()` guard prevented cursor drawing. Removed.
3. **GTK header row click drift** (#110): `body_measure` used 1.0× for headers (should be 1.2×) and unrounded item heights (19.6 vs 20.0). Both fixed to match `gtk_tree_layout`.
4. **ToggleGroup/ButtonRow items not individually clickable** (#112): Generic `FormFieldMeasure::new()` produced no per-item hit regions. Added `form_field_measure()` with item-level measurement, plus `backend.form_layout()` for GTK accuracy.
5. **GTK SegmentedControl not clickable**: GTK `form_layout()` fell through to generic measure for SegmentedControl. Added Pango-measured item regions.
6. **GTK text input in sidebar_search**: GTK emits `KeyPressed { Key::Char(ch) }` not `CharTyped`. Example now handles both.

### Test count progression

| Checkpoint | Lib tests |
|---|---|
| Session start | 543 |
| After #105 (Form sections) | 545 |
| After #107 (click dispatch) | 551 |
| After #110 (header click) | 553 |
| After #112 (item click) | 557 |
| After #53 (field kinds) | 565 |

### Design decisions

1. **`SectionKind` at construction time**: Explicit in `SidebarSectionDef`, not inferred from setter calls. Cleaner for Lua extensions declaring `kind = "form"`.
2. **`SidebarEvent::FormEvent { section, event }`**: Forward full `FormEvent` variant instead of collapsing to opaque `FormFieldActivated`. Enables typed Lua callbacks.
3. **`backend.form_layout()` threading**: `handle()` passes `Some(&*backend)` through `handle_inner` → `click()` for pixel-accurate GTK form hit-test. `handle_cached()` passes `None`, uses cached or estimated layout.
4. **SegmentedControl synthetic IDs**: `{field_id}__seg_{idx}` pattern parsed by `form_click_event` to emit `SegmentedControlChanged`.
5. **quadraui-lua vs quadraui-ipc**: Complementary crates — in-process Lua bridge vs out-of-process JSON/stdio bridge. Both tracked as future work.

### Open queue for next session

*Resolved in session 2026-05-10b below.*

## Session 2026-05-10b — Text field editing + clipboard + selection contrast

**Agent:** Claude Opus 4.6 (1M context)

### Issues closed (1)

| # | Title | Path | Key deliverable |
|---|---|---|---|
| 120 | sidebar_search: cursor movement + clipboard for text fields | A | `TextFieldState` editing state machine, arboard system clipboard for TUI+GTK, GTK Ctrl+key event fix, selection contrast fix in both backends. |

### Changes shipped

| Area | Files | Description |
|---|---|---|
| TextFieldState | examples/common/sidebar_search.rs | Consumer-side text editing: cursor positioning, char boundary navigation, selection range tracking, insert/delete at position, select all |
| Keyboard handling | examples/common/sidebar_search.rs | Left/Right/Home/End move cursor; Shift+variants extend selection; Ctrl+A select all; Ctrl+C copy (query only, not password); Ctrl+V paste; Delete key; ClipboardPaste event |
| arboard clipboard | Cargo.toml, tui/services.rs, gtk/services.rs | Replaced TUI no-op stub and GTK async-limited stub with `arboard` crate. `RefCell<Option<arboard::Clipboard>>` kept alive to avoid Linux clipboard serving thread teardown. |
| GTK Ctrl+key fix | gtk/events.rs | `gdk_key_to_quadraui_key` recovered base letter from keysym name when `to_unicode()` returns a control character (Ctrl+C → '\x03' → recover 'c') |
| TUI selection contrast | tui/form.rs | Text selection swaps fg/bg (inverse video) instead of using `selected_bg` for both row highlight and text selection |
| GTK selection contrast | gtk/form.rs | Text selection rendered in three segments: prefix (normal fg), selected (foreground on `selection_bg` rect), suffix (normal fg). Previous single-block paint made selection invisible when `selected_bg ≈ selection_bg`. |

### Bugs found + fixed

1. **TUI/GTK clipboard no-op**: `TuiClipboard` was a stub (read→None, write→no-op). `GtkClipboard::read_text` returned None (GTK read is async, trait is sync). Fixed by wiring `arboard` for both.
2. **arboard clipboard dropped too quickly**: Short-lived `arboard::Clipboard` handles logged "clipboard dropped in 0ms" on Linux — clipboard serving thread killed before managers could read. Fixed by storing handle in `RefCell` on the services struct.
3. **GTK Ctrl+key events silently dropped**: `gdk_key_to_quadraui_key` checked `!c.is_control()` and fell through to named-key lookup, which returned None for single-letter names like "c". Control characters Ctrl+A through Ctrl+Z never reached the app. Fixed with keysym name recovery.
4. **Text selection invisible on focused row (TUI)**: `selected_bg` used for both row-focus highlight and text-selection highlight — identical colors. Fixed with fg/bg swap for selected text.
5. **Text selection invisible on focused row (GTK)**: Same root cause — `sel` color used for both. Fixed with three-segment text rendering and `selection_bg` background rect.
6. **Ctrl+A case mismatch (GTK)**: GTK may deliver uppercase keyval depending on keyboard state. `handle_ctrl_key` matched lowercase only. Fixed with `ch.to_ascii_lowercase()`.

### Test count progression

| Checkpoint | Lib tests |
|---|---|
| Session start | 565 |
| Session end | 565 |

No new tests — this was consumer-side example logic + backend service wiring. Existing 565 tests pass.

### Dependencies added

| Crate | Version | Features pulling it in | Why |
|---|---|---|---|
| arboard | 3 | tui, gtk | System clipboard access (replaces no-op stubs) |

### Open queue for next session

*Resolved in session 2026-05-11 below.*

## Session 2026-05-11 — Vimcode dedup primitives + dispatch extensions + TUI terminal rasteriser

**Agent:** Claude Opus 4.6 (1M context)

### Issues closed (5)

| # | Title | Path | Key deliverable |
|---|---|---|---|
| 124 | DragTarget::ScrollbarY inverted flag | B (PR #125) | `inverted: bool` on ScrollbarY/X + SurfaceScrollbar. dispatch_mouse_drag flips ratio; dispatch_click flips track-click page direction. 4 new tests. |
| 123 | Terminal split-pane layout helper | B (PR #126) | `TerminalSplitLayout::new(area, left_cols, cell_width)` — left/right pane rects, divider position, hit_test. 5 new tests. |
| 121 | TabBar drop-zone computation + overlay | B (PR #127) | `compute_drop_zone` (Center/Split/TabReorder), `drop_zone_overlay` (highlight rect + insertion bar + ghost position), `DropEdge`, `DropGroupRect`. Edge-zone detection (20% clamped), tab midpoint reorder. 13 new tests. |
| 122 | Palette preview pane + tree-indented items | B (PR #128) | `PalettePreview` struct, `preview: Option` on Palette (40/60 split layout), `depth`/`expandable`/`expanded` on PaletteItem, `ExpandToggle`/`Preview` hit variants, `ExpandToggled` event. 4 new tests. |
| 129 | Terminal scrollbar rendering | B (PR #130) | `TerminalScrollbar` on Terminal primitive. TUI `draw_terminal` rasteriser (was `unimplemented!`) with full cell rendering + themed scrollbar. GTK `draw_terminal` draws themed scrollbar. `draw_terminal_divider` for both backends. 1 new test. |

### New primitives/types shipped

| Type | File | Description |
|---|---|---|
| `TerminalSplitLayout` | primitives/terminal.rs | Split-pane geometry + hit_test (Left/Divider/Right/Outside) |
| `TerminalScrollbar` | primitives/terminal.rs | Scroll state (total/visible/offset) for themed scrollbar rendering |
| `DropGroupRect` | primitives/drop_zone.rs | Group bounds + tab slot positions for drop-zone computation |
| `DropZone` / `DropZoneKind` | primitives/drop_zone.rs | Center/Split(DropEdge)/TabReorder result |
| `DropOverlay` | primitives/drop_zone.rs | Highlight rect + insertion bar + ghost position |
| `PalettePreview` | primitives/palette.rs | Styled lines, title, scroll offset, highlight line |

### New rasterisers

| Rasteriser | File | Description |
|---|---|---|
| TUI `draw_terminal` | tui/terminal.rs | Full cell grid rendering with overlay colors (cursor/selection/find) + themed scrollbar. Replaces `unimplemented!`. |
| TUI `draw_terminal_divider` | tui/terminal.rs | `│` characters for split-pane dividers using `theme.separator` |
| GTK `draw_terminal_divider` | gtk/terminal.rs | 1px Cairo line for split-pane dividers using `theme.separator` |

### Dispatch extensions

| Extension | File | Description |
|---|---|---|
| `DragTarget::ScrollbarY::inverted` | dispatch.rs | Flips scroll ratio for scrollback-style scrollbars |
| `SurfaceScrollbar::inverted` | dispatch.rs | Propagated to DragTarget on thumb click; flips track-click direction |

### Test count progression

| Checkpoint | Lib tests |
|---|---|
| Session start | 565 |
| After #124 (inverted scrollbar) | 569 |
| After #123 (terminal split) | 574 |
| After #121 (drop zone) | 587 |
| After #122 (palette preview) | 591 |
| After #129 (terminal scrollbar) | 592 |

### Open queue for next session

*Resolved in session 2026-05-11b/12 below.*

## Session 2026-05-11b/12 — Terminal scrollbar fixes + editor selection wrap + drag dispatch rework

**Agent:** Claude Opus 4.6 (1M context)

### Issues closed (4)

| # | Title | Path | Key deliverable |
|---|---|---|---|
| 131 | Backend::draw_terminal scrollbar: support inverted mode + configurable width | B (PR #132) | `TerminalScrollbar::inverted` + `width: Option<u16>` + `effective_scroll_offset()` method. GTK scrollbar width 8px default (was line_height ~18px). 8 new tests (5 unit + 3 TUI paint round-trip). |
| 133 | GTK draw_editor: char selection not painted on wrap-continuation rows | A | Replaced `line_to_view` HashMap lookup (skipped continuations) with direct visual-row iteration for Char and Block selection. Column ranges adjusted by `segment_col_offset` per segment. Removed unused `HashMap` import. |
| 134 | DragTarget::ScrollbarX/Y should respect minimum thumb length from rasteriser | B (PR #135) | Full Option B rework: replaced `visible_rows`/`total_items`/`visible_cols`/`total_cols` with `thumb_length: f32` + `max_scroll: usize`. Dispatcher does zero recomputation — maps cursor position directly using painted geometry. 6 round-trip tests proving `fit_thumb` ↔ `dispatch_mouse_drag` agreement. |
| 136 | Horizontal scroll surface: register h-scrollbar as ScrollSurface for automatic dispatch | B (PR #137) | `axis: ScrollAxis` on `SurfaceScrollbar`. `dispatch_click` branches on axis: vertical uses y + `ScrollbarY`, horizontal uses x + `ScrollbarX`. Track-click paging uses left/right for horizontal. 4 new tests. |

### API changes

| Change | Scope | Migration |
|---|---|---|
| `TerminalScrollbar::inverted` | New field, `#[serde(default)]` | Backward compatible; set `true` for scrollback-style terminals |
| `TerminalScrollbar::width` | New field, `Option<u16>` | Backward compatible; `None` uses default (8px GTK, 1 cell TUI) |
| `TerminalScrollbar::effective_scroll_offset()` | New method | Both rasterisers use it; consumers don't call directly |
| `DragTarget::ScrollbarY` | Breaking: `visible_rows`/`total_items` → `thumb_length`/`max_scroll` | Pass `Scrollbar.thumb_len` + actual scroll range |
| `DragTarget::ScrollbarX` | Breaking: `visible_cols`/`total_cols` → `thumb_length`/`max_scroll` | Same pattern |
| `SurfaceScrollbar::axis` | New required field | Existing vertical scrollbars add `axis: ScrollAxis::Vertical`; new h-scrollbars use `Horizontal` |

### Bug investigation: h-scrollbar drag range

Iterated three times on #134 before landing the correct fix:
1. **`min_thumb_length` approach** — added a floor to the recomputed thumb. Still wrong: the recomputation used `visible/total` ratios which could be in different units (chars vs pixels) than the rasteriser.
2. **`thumb_length` passthrough** — caller passes painted thumb size, no recomputation. Still wrong: dispatcher computed `max_scroll = total - visible` internally, which could use character counts while the actual scroll range was pixel-based.
3. **Full Option B** — `thumb_length` + `max_scroll` both caller-supplied. Dispatcher does only `effective_track = track_length - thumb_length` and linear interpolation over `[0, max_scroll]`. Round-trip tests prove correctness. Root cause confirmed as unit mismatch on vimcode side.

### Test count progression

| Checkpoint | Lib tests |
|---|---|
| Session start | 592 |
| After #131 (terminal scrollbar) | 600 |
| After #133 (selection wrap) | 600 |
| After #134 (drag dispatch) | 607 |
| After #136 (h-scroll surface) | 611 |
| After PR #139 (char_width + smoke test) | 611 |

### Additional PRs (no issue number)

| PR | Title | Key deliverable |
|---|---|---|
| #139 | Backend::char_width() + monospace default + Pango-measured width | `Backend::char_width()` on trait (TUI=1.0, GTK=Pango-measured). GTK runner default font `Sans 11` → `Monospace 11`. `approximate_char_width()` → `layout.pixel_size()` measurement. Paired `tui_hscroll`/`gtk_hscroll` smoke test (500-char line, $ jumps to end). |

### Bugs found + fixed

1. **GTK runner default font was proportional**: `Sans 11` caused `char_width` to be systematically too narrow — digits rendered wider than the average. `draw_editor`'s `scroll_left * char_width` formula assumes monospace. Fixed by changing default to `Monospace 11`.
2. **GTK char_width used approximate measurement**: `metrics.approximate_char_width()` doesn't account for font hinting. Over 500 chars the error was ~9 characters. Fixed by measuring via `layout.set_text("0"); layout.pixel_size()`.
3. **No `char_width()` on Backend trait**: `AppLogic` couldn't compute viewport_cols portably — only `line_height()` was exposed. Added `char_width()` to the trait.

### Open queue for next session

*Continued in session 2026-05-12b/13 below.*

## Session 2026-05-12b/13 — StatusBarLayout, DataTable, h-scroll, double-click, Lens assessment

**Agent:** Claude Opus 4.6 (1M context)

### Issues closed (5)

| # | Title | Path | Key deliverable |
|---|---|---|---|
| 140 | Backend::draw_status_bar returns StatusBarLayout | B (PR #141) | `StatusBarLayout` replaces `Vec<StatusBarHitRegion>`. `StatusBarInteraction` uses `StatusBarLayout::hit_test()`. Smoke-tested via multi_tree Run/Stop buttons. |
| 142 | DataTable primitive: multi-column sortable table | B (PR #145, worktree) | New primitive: `DataTable`, `Column`, `ColumnWidth` (Fixed/Flex/Content), `DataRow`, `SortDirection`. TUI+GTK rasterisers, Backend trait methods, `resolve_columns()` shared layout. 15 tests. Paired `tui_data_table`/`gtk_data_table` k8s pod list example. |
| 146 | DataTable v2: separators, scrollbar interaction, h-scroll, cell colors | B (PR #148, worktree) | Column header separators (TUI `│`, GTK 1px line). Per-cell colored text (per-span fg). V-scrollbar thumb drag + track page. H-scroll via `min_total_width` + `h_scroll` fields, half-height h-scrollbar (GTK), TUI i32 coordinate clipping. |
| 147 | TUI backend: synthesize DoubleClick from repeated MouseDown | B (PR #149) | `DoubleClickDetector` on `TuiBackend` (400ms, ±1.5 cell radius). GTK runner `connect_pressed` fixed to emit DoubleClick on `n_press == 2`. `SidebarSystem` forwards DoubleClick → `RowActivated`. Multi_tree example distinguishes sel/dblclick. |
| 138 | GTK draw_editor h_scroll_offset drift | closed by vimcode | Root cause was vimcode-side viewport_cols, not quadraui. |

### Issues filed (3)

| # | Title | Status |
|---|---|---|
| 142 | DataTable primitive | closed (PR #145) |
| 143 | Chart primitive: sparkline / line / area | open |
| 144 | TabGroup compose helper | open |

### Additional PRs

| PR | Title | Key deliverable |
|---|---|---|
| #139 | Backend::char_width() + monospace default + Pango-measured width | `Backend::char_width()` on trait. GTK runner `Sans 11` → `Monospace 11`. `approximate_char_width()` → `layout.pixel_size()`. Paired hscroll smoke test. |

### New primitive shipped

| Primitive | Files | Features |
|---|---|---|
| DataTable | primitives/data_table.rs, tui/data_table.rs, gtk/data_table.rs | Sortable column headers with ▲/▼ indicators + separator lines, Fixed/Flex/Content column sizing, per-cell colored text (StyledText spans), row selection, vertical scrollbar (thumb drag + track page), horizontal scroll with min_total_width + half-height h-scrollbar, header divider hit-test for column resize |

### Lens assessment

Audited quadraui's 29 primitives against Lens (Kubernetes IDE) features. Found ~90% UI coverage. Key gaps:
- **DataTable** — shipped this session (#142, #146)
- **Chart** — filed #143 (sparkline/line/area for metrics)
- **TabGroup** — filed #144 (compose helper for tabbed split panes)

### Bugs found + fixed

1. **GTK runner default font proportional** (PR #139): `Sans 11` → `Monospace 11`. `approximate_char_width()` → `layout.pixel_size()` for Pango-accurate measurement.
2. **GTK column text bleed** (#142): no per-column Cairo clip. Fixed with `cr.save()/clip()/restore()` per cell.
3. **TUI h-scroll columns anchored** (#146): header/body column x-positions used `u16` which couldn't go negative. Fixed with `i32` coordinate math and `cx >= area.x` guard.
4. **GTK h-scrollbar full row height** (#146): used `row_height` for h-scrollbar. Fixed to `row_height * 0.5` when `row_height > 1.5` (GTK).
5. **SidebarSystem dropped DoubleClick** (#147): only matched `MouseDown`. Added `double_click()` method forwarding to TreeController → `RowActivated`.
6. **GTK runner ignored n_press** (#147): `connect_pressed` always emitted `MouseDown`. Fixed to emit `DoubleClick` when `n_press == 2`.

### Test count progression

| Checkpoint | Lib tests |
|---|---|
| Session start (from previous) | 611 |
| After #140 (StatusBarLayout) | 611 |
| After #142 (DataTable v1) | 626 |
| After #146 (DataTable v2) | 626 |
| After PR #139 (char_width) | 626 |
| After #147 (double-click) | 629 |

### Open queue for next session

*Resolved in session 2026-05-13 below.*

## Session 2026-05-13 — Chart primitive + Chart/DataTable interactivity arc

**Agent:** Claude Opus 4.6 (1M context)

### Issues closed (6)

| # | Title | Path | Key deliverable |
|---|---|---|---|
| 143 | Chart primitive: sparkline / line / area charts | A | New primitive: `Chart`, `ChartKind` (Sparkline/Line/Bar), `Series` with `fill` for area charts. TUI rasteriser (sparkline=block chars, line=braille dots, bar=block fills). GTK rasteriser (Cairo polylines/rectangles). Backend trait `draw_chart`/`chart_layout`. `SectionBody::Chart` MSV integration. Paired `tui_chart`/`gtk_chart` examples. 15 tests. |
| 150 | Chart + DataTable: hover state and tooltip integration | A | `ChartHit::DataPoint` variant. `ChartLayout::data_point_positions` + `nearest_point()` helper. `draw_chart` gains `hovered_point` per-frame param. `draw_data_table` gains `hovered_idx` per-frame param. TUI hover marker (`●`), GTK filled circle. DataTable row tint on hover. |
| 153 | Chart: axis tick marks, value labels, and grid lines | A | `y_ticks`, `x_ticks`, `show_grid` fields on Chart. `ChartLayout::y_tick_positions`/`x_tick_positions`. Dynamic y-label gutter width from tick label widths. TUI tick labels + `┄` grid lines. GTK Pango tick labels + translucent grid lines. |
| 152 | Chart: crosshair cursor line with value readout | A | `draw_chart` gains `crosshair_x` per-frame param. `ChartLayout::screen_to_data_x`/`data_to_screen_x` helpers. TUI dim `│` crosshair. GTK dashed line + per-series value labels. |
| 151 | Chart: click-to-drill event with data point identity | A | `ChartEvent::DataPointClicked`/`LegendClicked` variants. `UiEvent::Chart(WidgetId, ChartEvent)` + `UiEvent::DataTable(WidgetId, DataTableEvent)` dispatch pipeline. |
| 154 | DataTable: column resize drag interaction | A | `column_overrides: Vec<Option<f32>>` on DataTable. `resolve_columns` applies overrides as `Fixed(w)`. `DataTableEvent::ColumnResized` variant. |

### Issues filed (5)

| # | Title | Status |
|---|---|---|
| 150 | Chart + DataTable hover state | closed in-session |
| 151 | Chart click-to-drill events | closed in-session |
| 152 | Chart crosshair cursor | closed in-session |
| 153 | Chart axis ticks + grid lines | closed in-session |
| 154 | DataTable column resize | closed in-session |

### New primitive shipped

| Primitive | Files | Features |
|---|---|---|
| Chart | primitives/chart.rs, tui/chart.rs, gtk/chart.rs | Sparkline (Unicode block chars TUI, Cairo polyline GTK), Line (braille dots TUI, Cairo paths GTK), Bar (block fills TUI, Cairo rects GTK). Per-series fill for area charts. Hover marker, crosshair line, axis ticks, grid lines, data point positions for nearest-point resolution. |

### Backend trait changes

| Method | Change |
|---|---|
| `draw_chart` | New (from #143), then gained `hovered_point` (#150) + `crosshair_x` (#152) |
| `chart_layout` | New (#143) |
| `draw_data_table` | Gained `hovered_idx` parameter (#150) |

### Interactivity features shipped

| Feature | Scope |
|---|---|
| Chart hover | Per-frame `hovered_point` param, `data_point_positions` + `nearest_point()` on layout, TUI `●` marker with braille-aligned coordinates, GTK filled circle with glow |
| DataTable hover | Per-frame `hovered_idx` param, row background tint (TUI `tab_bar_bg`, GTK 50% alpha) |
| Chart crosshair | Per-frame `crosshair_x` param, `screen_to_data_x`/`data_to_screen_x` helpers, TUI dim `│` column, GTK dashed line + Pango value labels per series |
| Chart axis ticks | `y_ticks`/`x_ticks`/`show_grid` fields, `y_tick_positions`/`x_tick_positions` on layout, `format_tick_value` helper, dynamic gutter width |
| Chart click-to-drill | `ChartEvent::DataPointClicked`/`LegendClicked`, `UiEvent::Chart`/`UiEvent::DataTable` |
| DataTable column resize | `column_overrides` field, `ColumnResized` event, `resolve_columns` override application |

### Bugs found + fixed

1. **TUI sparkline 1-char-per-point**: sparkline rendered data points as 1 char each instead of stretching across the full width. Fixed with linear interpolation across all available columns.
2. **TUI hover marker offset**: `●` painted ~2 rows below the braille line. Root cause: primitive's `data_point_positions` use full plot_area coordinates, but braille renderer offsets by 1 col (left axis) + 1 row (bottom axis). Fixed by computing marker position directly from data values using braille-grid math (dot_w/dot_h → cell_col/cell_row).
3. **DataTableEvent Eq derive with f32**: `ColumnResized { width: f32 }` broke `#[derive(Eq)]`. Fixed by dropping `Eq` (keeping `PartialEq`).

### Example updates

- `chart_app.rs`: Wired `MouseMoved` → `nearest_point` → `hovered_point` and `screen_to_data_x` → `crosshair_x` for interactive demos across all chart kinds.
- `data_table_app.rs`: Wired `MouseMoved` → `hit_test` → `hovered_idx` for row highlight.
- Line chart example enables `y_ticks: Some(5)` + `show_grid: true` to demonstrate axis features.

### Disk management

Worktree `target/` directories (6.5 GB each) and main repo `target/` (22 GB → 26.6 GB) consumed disk. Cleaned `~/.claude` caches (session logs, file-history, telemetry, stale worktree project dirs) and main `target/` to recover ~37 GB.

### Test count progression

| Checkpoint | Lib tests |
|---|---|
| Session start | 629 |
| After #143 (Chart primitive) | 644 |
| Session end (all interactivity) | 644 |

### Open queue for next session

- #65 — SplitDragController compose helper (deferred)
- #115 — quadraui-lua bridge crate (future)
- #118 — quadraui-ipc JSON bridge (future)
- #144 — TabGroup compose helper
- Windows milestone (#19–#31)
- macOS milestone (#32–#44)

---

## Session 2026-05-13 — FormController scroll + scrollbar support

**Agent:** Claude Opus 4.6 (1M context)

### Issues closed (1)

| # | Title | Path | Key deliverable |
|---|---|---|---|
| 155 | Form primitive: built-in scroll + scrollbar support | B (PR #156) | FormController enriched with scroll state, scrollbar rendering, scroll wheel / thumb-drag / track-click handling |

### What shipped

**FormController enrichment** (`compose/form_controller.rs`): Mirrored TreeController's scroll architecture onto FormController. Previously a thin storage wrapper; now owns `scroll_offset`, `scroll_drag`, `has_focus` and exposes `render()` + `handle()`. Apps call `set_form()` per frame, then `render()` draws the form + scrollbar when content overflows, and `handle()` processes scroll wheel, scrollbar thumb-drag, track-click page up/down, and form body clicks.

**FormControllerEvent**: `FormAction(FormEvent)` | `ScrollChanged` | `Consumed` | `Ignored`.

**form_click_event refactor**: Moved from `sidebar_system.rs` into `form_controller.rs` as `pub(crate)` — canonical location shared by both FormController and SidebarSystem.

**Exports**: `FormController` + `FormControllerEvent` now exported from crate root.

**Example pair**: `tui_form_scroll` / `gtk_form_scroll` — 20-toggle settings panel exercising FormController scroll. Status bar shows live scroll offset.

### Bugs fixed during session

1. **GTK scrollbar too wide**: `scrollbar_track_width` used `backend.line_height()` (~20px on GTK). Fixed with `(lh * 0.4).max(1.0).round()` — yields 1 cell on TUI, ~8px on GTK, matching MSV's `scrollbar_size: 8.0` convention.

### Cross-backend portability note

FormController is fully backend-agnostic — calls `Backend::draw_form`, `draw_scrollbar`, `form_layout`, `line_height`. Future macOS/Windows backends get FormController scroll support automatically by implementing the trait.

### Test count progression

| Checkpoint | Lib tests |
|---|---|
| Session start | 663 |
| Session end | 663 (16 new FormController tests, net zero because form_click_event tests moved from sidebar_system) |

### Open queue for next session

- #65 — SplitDragController compose helper (deferred)
- #115 — quadraui-lua bridge crate (future)
- #118 — quadraui-ipc JSON bridge (future)
- #144 — TabGroup compose helper
- Windows milestone (#19–#31)
- macOS milestone (#32–#44)
