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

## Session 2026-05-13/14 — FormController + vimcode primitive gap batch

**Agent:** Claude Opus 4.6 (1M context)

### Issues closed (8)

| # | Title | Path | Key deliverable |
|---|---|---|---|
| 155 | Form scroll + scrollbar support | B (PR #156) | FormController enriched: render() + handle(), scroll wheel/thumb-drag/track-click, scrollbar rendering |
| 157 | FormController handle_cached() | B (PR #158) | Backend-free event handler using cached line_height; refactored internals to pure functions |
| 159 | TreeView unfocused selection highlight | A (direct) | `inactive_selected_bg` theme color; dimmed selection when `has_focus=false` |
| 161 | Palette preview pane rendering | B (PR #168) | TUI + GTK rasterisers paint preview content in 40/60 split with separator, scroll, highlight_line |
| 162 | Palette popup variant (show_query) | B (PR #169) | `show_query: bool` hides query row + separator for tab-switcher shape |
| 163 | Form ButtonRowItem icon support | B (PR #170) | `icon: Option<Icon>` on ButtonRowItem; TUI + GTK render icon before label |
| 164 | Palette create_label action | B (PR #171) | Pinned `+ <label>` row below scrollable items with accent styling; PaletteHit::CreateAction |
| 165 | Dialog multi-line body | B (PR #172) | `body: Vec<StyledText>` replaces single StyledText; per-line styled spans |

### What shipped

**FormController arc (#155, #157):** Full scroll support mirroring TreeController — owned scroll_offset, scrollbar rendering, scroll wheel / thumb-drag / track-click. `handle_cached()` enables backend-free event handling via cached metrics. GTK scrollbar width convention: `(lh * 0.4).max(1.0).round()` (~8px on GTK, 1 cell on TUI). `form_click_event` moved from sidebar_system to form_controller as canonical location. Example pair `tui_form_scroll` / `gtk_form_scroll` exercises both paths.

**Unfocused selection (#159):** `inactive_selected_bg` theme color (rgb(35,40,58)) — midway between tab_bar_bg and selected_bg. Both tree rasterisers show dimmed highlight when `selected_path` is set but `has_focus` is false.

**Primitive gap batch (#161–#165):** Five vimcode-filed issues closing gaps that forced bespoke rendering. Palette gained preview pane (40/60 split), popup mode (hidden query), and pinned create-action row. ButtonRowItem gained icon support. Dialog.body became Vec<StyledText> for multi-line content. Together these eliminate ~1400+ lines of bespoke rendering across vimcode's TUI and GTK backends.

### Bugs fixed during session

1. **GTK scrollbar too wide** (#155): `scrollbar_track_width` used `line_height()` (~20px). Fixed with `(lh * 0.4).max(1.0).round()` matching MSV's `scrollbar_size: 8.0`.
2. **Palette bottom border overwrites ┴ junction** (#161): Bottom border loop drawn after preview junction, overwriting `┴`. Fixed by incorporating junction into the border loop.

### Test count progression

| Checkpoint | Lib tests |
|---|---|
| Session start | 663 |
| After #155 (FormController scroll) | 663 (16 new, form_click_event moved) |
| After #157 (handle_cached) | 670 (+7) |
| After #161 (palette preview) | 672 (+2) |
| After #162 (show_query) | 673 (+1) |
| After #164 (create_label) | 674 (+1) |
| Session end | 674 |

### Open queue for next session

- #166 — Folder picker primitive
- #65 — SplitDragController compose helper (deferred)
- #115 — quadraui-lua bridge crate (future)
- #118 — quadraui-ipc JSON bridge (future)
- #144 — TabGroup compose helper
- Windows milestone (#19–#31)
- macOS milestone (#32–#44)

## Session 2026-05-14 — macOS headless test surface (#37)

**Agent:** Claude Opus 4.7 (1M context)

**Issue closed (1):**

| # | Title | Path | Key deliverable |
|---|---|---|---|
| 37 | macOS: headless test surface (CGBitmapContext) | A (direct, `3b5ce8a`) | `quadraui::macos::headless::BitmapSurface` — `CGBitmapContextCreate` wrapper, top-left origin via CTM flip, RGBA byte order, `pixel(x,y)` readback. Integrates with `MacBackend::enter_frame_scope`. |

### What shipped

`quadraui/src/macos/headless.rs` (~250 lines impl + 6 tests). Gated
`#[cfg(test)] pub mod headless` so rasteriser tests in #38–#43 can
reach it from sibling files while it stays out of the public API
surface until the rasteriser contract is shaken out.

Key invariants:

- **Top-left origin**: constructor applies `translate(0, H)` + `scale(1, -1)` so callers paint in the same coord frame as the live `QuadraView` (which sets `isFlipped: YES`). Buffer scanlines are top-down in memory, so the flipped drawing space aligns directly with row indexing — `pixel()` reads `y` as a memory row with no inversion.
- **RGBA byte order** via `kCGImageAlphaPremultipliedLast` (matches the `core-graphics` crate's own `create_bitmap_context_test` byte assertions).
- **Raw FFI throughout** rather than the `core_graphics::context::CGContext` wrapper — the wrapper exposes its raw pointer only via the `foreign_types::ForeignType` trait, which isn't a direct dep. Same style as `macos::text` which already speaks raw CG FFI for `drawRect:`'s borrowed context.

Tests:

- `new_initialises_transparent_black` — zero-fill.
- `dimensions_reported` — buffer length `W·H·4`.
- `fill_paints_expected_colour` — RGBA byte order.
- `top_left_origin_for_partial_fill` — CTM flip + memory layout consistency.
- `integrates_with_mac_backend_frame_scope` — end-to-end: `BitmapSurface::context_ptr()` → `MacBackend::enter_frame_scope` → CG FFI inside closure → readback. Documents the exact shape #38–#43 rasteriser harnesses will use.
- `dump_smoke_ppm` (`#[ignore]`) — paints four-corner colour grid + Core Text label, writes `/tmp/quadraui_headless.ppm`, opens in Preview via `open` for visual confirmation.

### Bugs caught during development

1. **Buffer-layout y-inversion confusion**: initial `pixel()` inverted `y` on the assumption that CG bitmap rows ran bottom-up in memory. They don't — scanlines are top-down (the standard image-file convention); only the CG *coordinate* system is bottom-up by default. With the CTM flip applied, top-left coord y directly indexes memory row. Caught by a diagnostic test that dumped raw byte rows after a partial fill.

### Test count progression

| Checkpoint | Lib tests |
|---|---|
| Session start | 674 |
| After #37 | 679 (+5 normal, +1 ignored) |

### Open queue for next session

*Resolved in session 2026-05-15 below for #38.*

## Session 2026-05-15 — macOS chrome rasterisers (#38)

**Agent:** Claude Opus 4.7 (1M context)

**Issue closed (1):**

| # | Title | Path | Key deliverable |
|---|---|---|---|
| 38 | macOS: chrome rasterisers (status_bar, tab_bar, activity_bar, command_center, menu_bar) | B (PR #176) | Five `quadraui::macos::<chrome>::draw_*` rasterisers + matching `mac_*_layout` helpers; `MacBackend` chrome trait methods replace the `mac_unimpl!` stubs; `macos_app` + `macos_demo` examples wired through shared `AppLogic`; `BitmapSurface::write_ppm_and_open` helper. |

### What shipped

Five rasteriser modules (`quadraui/src/macos/{status_bar,tab_bar,activity_bar,command_center,menu_bar}.rs`), wired into `MacBackend` via the `chrome` trait methods and re-exported from `macos::mod`. Each rasteriser is unsafe-CG-FFI inside, exposes a safe layout helper, and ships with mutation-verified paint↔click round-trip tests via `BitmapSurface`.

Backend integration pattern (`backend.rs`):

```rust
fn draw_menu_bar(&mut self, bounds: Rect, bar: &MenuBar) -> MenuBarLayout {
    let ctx = self.current_cg();
    let font = self.current_font.as_ref().expect("font installed in setup");
    unsafe { super::menu_bar::draw_menu_bar(ctx, font, bounds.x as f64, bounds.y as f64,
        bounds.width as f64, bounds.height as f64, bar, &self.theme) }
}
```

Examples (paired runners against shared `AppLogic`):

- `examples/macos_app.rs` (new) — minimal app using `common::MiniApp` (one `StatusBar`).
- `examples/macos_demo.rs` (rewritten) — full demo using `common::AppState` (TabBar + StatusBar with focus cycling).

`run.rs` install of `Menlo 14pt` font happens before `AppLogic::setup` so primitives can rely on `font_metrics()` from setup onwards. The previous `drawRect:` Menlo smoke label was removed — its descenders peeked below the 28pt tab bar.

### Tests

17 new rasteriser unit tests in `macos::{status_bar,tab_bar,activity_bar,command_center,menu_bar}::tests` — all use the `BitmapSurface::pixel(x,y)` probe + `Layout::hit_test` round-trip pattern. Notable test-quality moves:

- **Sentinel-segment trick** (status_bar): sample StatusBar prepended with one extra clickable segment whose bg differs from both the bar bg and other segments — mutation of the layout fails the probe instead of trivially passing.
- **PPM smoke dumps** for the three rasterisers without paired example apps: `#[ignore]`d tests in `activity_bar.rs`, `command_center.rs`, `menu_bar.rs` write `/tmp/quadraui_<name>.ppm` and shell out to `open` for visual confirmation. Helper `BitmapSurface::write_ppm_and_open(path)` extracted from headless.rs's existing dump_smoke_ppm.

Visual confirmation done:

- `cargo run -p quadraui --example macos_app --features macos` — StatusBar
- `cargo run -p quadraui --example macos_demo --features macos` — TabBar + StatusBar with focus cycle
- `cargo test -p quadraui --features macos --lib -- --ignored macos::{activity_bar,command_center,menu_bar}::tests::dump_smoke_ppm` — three PPM dumps

### Scope omissions (deferred to a unified text-attribute pass)

These all require threading `CTAttributedString`/`kCTUnderlineStyleAttributeName` through the `macos::text` boundary, which currently only renders plain Menlo. Tracked as a single follow-up:

- **status_bar**: bold first segment.
- **tab_bar**: italic preview tabs, rounded close-button hover bg.
- **menu_bar**: Alt-key underline on the marked character.
- **command_center**: rounded search-box border (needs CG path API, not text attrs — separate concern).

### Bugs caught during development

1. **`as_ptr()` not on `CGContext`**: `foreign_types_shared::ForeignType` is a transitive dep, not direct. Fixed once at the start of #37; resolved before #38 by using raw FFI throughout.
2. **Weak status_bar round-trip**: bar bg == segment bg meant mutating geometry didn't fail the probe. Fixed with the sentinel segment.
3. **`menu_bar::hit_test` for disabled items**: disabled items hit `Bar` not `Item` per the primitive contract — test had to expect `Bar` for the View item.
4. **Pre-existing clippy errors on develop** (`unnecessary_min_or_max`, `redundant_guards`) surfaced when running quality gate. Unrelated to #38; left for a separate cleanup pass (no CI configured on this repo).

### Test count progression

| Checkpoint | macos:: tests |
|---|---|
| Session start (after #37) | 12 (+1 ignored) |
| After #38 | 66 (+4 ignored) |

### Open queue for next session

*Resolved in session 2026-05-16 below — #39, #40, #41, #42 all landed.*

## Session 2026-05-16 — macOS rasteriser arc: content, MSV, containers, overlays

**Agent:** Claude Opus 4.7 (1M context)

Four macOS rasteriser milestones closed in one session, completing 5 of 7 macOS milestones in total (rasterisers ✓; #36 platform services + #43 terminal/text_display/message_list still open).

### Issues closed (4)

| # | Title | Path | Key deliverable |
|---|---|---|---|
| 39 | macOS: content rasterisers (tree, list, form, editor, data_table, chart) | A (rebased + FF-merged) | Six rasterisers + `mac_*_layout` helpers + 3 paired examples (`macos_data_table`, `macos_chart`, `macos_form_groups`). CG path API bindings landed (`CGContextMoveToPoint`, `AddLineToPoint`, `ClosePath`, `StrokePath`, `FillPath`). |
| 40 | macOS: MSV + scrollbar rasterisers | A | `macos::multi_section_view` (full chrome — header / aux / body dispatch / per-section + panel scrollbar / dividers) + `macos::scrollbar` (overlay alpha-blend matching GTK). `macos_multi_tree` paired example. |
| 42 | macOS: container + indicator rasterisers (panel, split, toast, progress, spinner) | A | Five rasterisers + 4 paired examples (`macos_split`, `macos_panel`, `macos_toast`, `macos_indicators`). |
| 41 | macOS: overlay rasterisers (tooltip, context_menu, dialog, palette, completions, find_replace, rich_text_popup) | A | Seven rasterisers covering all in-window overlay primitives. No paired example — overlays are triggered from existing apps. |

### What shipped per ticket

#### #39 — Content rasterisers

- `tree` (5 tests): header pitch `line_height * 1.2`, leaf pitch `line_height * 1.4`, chevron + icon + text + badge, mutation-verified scroll_offset.
- `list` (5 tests): title strip, decoration-driven fg colour, right-aligned detail.
- `form` (3 tests): Label / Toggle / TextInput / Button / ReadOnly. Rich field kinds (Slider / ColorPicker / Dropdown / etc.) render label-only for now.
- `editor` (4 tests): bg + per-line text + line-number gutter + cursorline + per-span fg/bg + Block/Bar/Underline cursor. Selections / diagnostics / indent guides / multi-cursors deferred.
- `data_table` (4 tests): header sort glyphs, hover/select tints, column separators, vertical + horizontal scrollbars.
- `chart` (4 tests): Sparkline / Line / Bar via CG path API, legend swatches, axis tick labels, hover marker, crosshair.

Visual fix-up commit (`56e81cd`):
- data_table cells collapsed all StyledText spans into one fg run → per-span colour painting + per-column clipping (header + body) so titles in narrow `Fixed(8.0)` columns truncate instead of overflowing into neighbours.
- chart sparkline used the primitive's 1-pixel-per-data-point layout (TUI-cell convention bleeding through) → stretches across full plot width.
- chart line `fill=true` painted as a fan of vertical bars → proper closed polygon via `CGContextFillPath`.
- chart bar positioning centred bars on tick marks (first bar's left edge ended up left of plot_x, covering axis labels) → slot-based positioning with 15% gap, matching GTK.

#### #40 — MSV + scrollbar

- `scrollbar` (5 tests): overlay alpha cadence (track 0.20 → 0.35 on hover; thumb 0.50 → 0.70 → 0.85 on hover → drag). Both `ScrollAxis` variants share one impl.
- `multi_section_view` (5 tests): full MSV chrome including per-section headers (chevron / title / badge / right-aligned actions), aux row (Input / Search / Toolbar / Custom), per-section bodies dispatched to existing macos rasterisers (Tree / List / Form / Chart) with body clip, per-section scrollbar gutters with thumb, dividers, panel-level scrollbar. `mac_msv_metrics` + `mac_msv_layout` deliver the shared paint/click layout.
- `macos_multi_tree` paired example wires shared `common::DebugSidebar` (`SidebarSystem` compose helper) through `quadraui::macos::run`.

Shift+Tab fix (`d8e5740`): AppKit reports Shift+Tab as the Tab keycode (0x30) + shift modifier, not a separate back-tab keycode the way GTK (`ISO_Left_Tab`) and crossterm (`KeyCode::BackTab`) do. `ns_key_to_uievent` now promotes `Tab + shift` → `NamedKey::BackTab` so backend-neutral consumers (`SidebarSystem`, `FocusRing`) match the same variant on every backend. Caught visually in `macos_multi_tree` — Shift+Tab cycled forward instead of backward.

#### #42 — Containers + indicators

- `split` (5 tests): 4-pt divider matching GTK; both `SplitDirection` variants. Pane content stays the host's responsibility.
- `panel` (5 tests): title bar (bg + title + right-aligned action buttons), content region exposed via `PanelLayout::content_bounds`.
- `toast` (5 tests): `ToastStack` corner-positioned boxes, severity tint (Info / Success / Warning / Error), dismiss `×`, optional action button.
- `progress` (4 tests): track + determinate fill / indeterminate pulse + label + optional cancel `×`.
- `spinner` (4 tests): braille animation glyph + optional label. Same frame table as TUI/GTK.

#### #41 — Overlays

- `tooltip` (4 tests): bordered rect + plain or styled-line text.
- `context_menu` (3 tests): rows + separators + selection highlight + detail text. Returns `Vec<(Rect, WidgetId)>`.
- `dialog` (3 tests): title + body + optional input + button row. Returns `Vec<Rect>`.
- `completions` (3 tests): autocomplete popup with selected-row highlight.
- `palette` (3 tests): modal fuzzy picker — title / query+cursor / separator / scrollable items / pinned create row / optional preview pane / scrollbar. `mac_palette_layout` helper.
- `find_replace` (2 tests): panel anchored top-right; walks `hit_regions` for chevron / inputs / toggles / nav glyphs / dismiss.
- `rich_text_popup` (2 tests): bordered popup with per-line per-span text. `has_focus` swaps border to `theme.link_fg`.

### Issues filed for follow-up macOS native-feel work (2)

| # | Title | Why |
|---|---|---|
| 184 | macOS: native menu bar via NSMenu (and shared MenuBar shape upgrade) | Apps need menus in the system menu bar at top of screen, not in-window. Adds `Backend::install_menu_bar(&MenuBar)` + shared `MenuItem` shape with `key_equivalent` + `checked` + `submenu` fields. The shape upgrade benefits TUI/GTK too (structured shortcuts, checkbox menu items). |
| 185 | macOS: native right-click context menus via NSMenu.popUpContextMenu | User-triggered right-click should use native `NSMenu::popUpContextMenu`; app-driven dropdowns continue painting via existing `draw_context_menu`. Adds `Backend::show_context_menu(menu, anchor)`. Depends on #184 (shares NSMenu builder + selector bridge). |

The convention to document when #185 lands: `MouseDown { button: Right }` → `backend.show_context_menu(...)` (native on Mac, painted elsewhere); left-click on a UI affordance opening a menu-like dropdown → `draw_context_menu` (painted on all backends).

### Cross-cutting concerns surfaced during the session

- **Unified text-attribute pass remains the largest deferred work.** Bold span attrs, italic preview tabs, Alt-key underline, match-position highlighting, focused-link underline, selection bg + inverted fg in inputs, per-line font scale for markdown headings — all gated on threading `CTAttributedString` attributes through `macos::text::draw_text`. Touches every rasteriser that uses `draw_text`; high leverage for finishing-polish across all 5 macOS milestones.
- **`Fixed(N)` ColumnWidth in DataTable** carries no unit info — the primitive lets the value mean "cells" or "pixels" depending on backend. Example apps using `Fixed(8.0)` (TUI-sized) get tiny columns on GUI backends. Clip-on-truncate behaviour is now correct on macOS, but the example data needs updating to use `ColumnWidth::Content { min, max }` for cross-backend portability. Tracked implicitly — not yet filed.

### Test count progression

| Checkpoint | macos:: tests | Full lib tests |
|---|---|---|
| Session start (after #38) | 66 (+4 ignored) | — |
| After #39 | 91 (+4 ignored) | — |
| After #40 | 101 (+4 ignored) | — |
| After #42 | 126 (+4 ignored) | — |
| After #41 | 146 (+4 ignored) | 633 |

### macOS milestone status at end of session

| # | Ticket | Status |
|---|---|---|
| 37 | headless test surface | ✅ |
| 38 | chrome (status_bar / tab_bar / activity_bar / command_center / menu_bar) | ✅ |
| 39 | content (tree / list / form / editor / data_table / chart) | ✅ |
| 40 | MSV + scrollbar | ✅ |
| 42 | container + indicator (panel / split / toast / progress / spinner) | ✅ |
| 41 | overlay (tooltip / context_menu / dialog / palette / completions / find_replace / rich_text_popup) | ✅ |
| 43 | terminal + text_display + message_list | open *(gated on text-attribute pass)* |
| 36 | platform services (clipboard / dialogs / notifications / URL open) | open *(independent)* |
| 44 | port all paired examples | open *(depends on #43)* |

**27 rasterisers, ~150 round-trip tests, 8 paired examples, 5 milestones closed.** macOS apps using `quadraui::macos::run` render every primitive needed for kubeui-class consumers.

### Open queue for next session

- **#36** — macOS platform services. Pure FFI work (`NSPasteboard`, `NSOpenPanel`, `NSSavePanel`, `NSUserNotificationCenter`, `NSWorkspace`). Different muscle group from rasterisers. Natural lead-in to #184 / #185 since both use the same objc2 patterns.
- **Text-attribute pass** (not yet filed). Threads `CTAttributedString` attributes through `macos::text::draw_text` to unlock bold / italic / underline / per-character fg across every rasteriser. High leverage — completes the polish across all 5 closed milestones. Also unblocks #43.
- **#43** — terminal + text_display + message_list. Don't pick before the text-attribute pass; terminal cells need bold + per-char colour.
- **#184** — install_menu_bar via NSMenu. Best after #36 (same FFI shape).
- **#185** — show_context_menu via popUpContextMenu. Depends on #184.
- **#44** — port all paired examples. Depends on #43.
- **Win-GUI milestone (#19–#31)** — nothing started; #19 is the entry point.
- **#177** — TUI editor block cursor fg/bg swap → `theme.cursor`. ~5-line fix.
- **#180** — GTK palette scrollbar width 6 → 10 px. One-line constant change.
- **Future / deferred:** #166, #144, #65, #118, #115.

## Session 2026-05-17 — macOS terminal + text_display + message_list rasterisers (#43)

**Agent:** Claude Opus 4.7 (1M context)

Closes the macOS in-window rasteriser surface — last three primitives landed without waiting for the text-attribute pass (previous session expected #43 to be gated on it; instead we shipped attribute-less and filed the deferral inline).

### Issues closed (1)

| # | Title | Path | Key deliverable |
|---|---|---|---|
| 43 | macOS: terminal + text_display + message_list rasterisers | B (PR #186) | Three rasterisers + `mac_text_display_layout` helper. `MacBackend`'s last four `mac_unimpl!` stubs replaced with real impls; the now-unused macro removed. |

### What shipped

- **`macos::terminal`** (5 tests + 1 PPM dump): cell-grid walk mirroring GTK, cell bg/fg with `is_cursor` / `is_find_active` / `is_find_match` / `selected` overlays, glyph skip for `' '` / `'\0'`, right-edge clip via `cell_area_w`. Scrollbar wiring lives in `MacBackend::draw_terminal` — builds `Scrollbar::vertical` from `TerminalScrollbar::effective_scroll_offset` and delegates to `super::scrollbar::draw_scrollbar`, identical shape to `GtkBackend::draw_terminal`.
- **`macos::text_display`** (5 tests + 1 PPM dump): optional title strip, body from `resolved_scroll_offset` (auto-scroll honoured by the primitive's `layout()`), per-line `Decoration` tint (Error / Warning / Muted / Normal), optional timestamp prefix in `theme.muted_fg`, optional scrollbar gutter + thumb. `mac_text_display_layout` mirrors `gtk_text_display_layout` shape (body-local coordinates; 12-pt gutter, 8-pt min thumb constants).
- **`macos::message_list`** (3 tests + 1 PPM dump): walks `rows[scroll_top..]`, paints text at `(x + row.indent, y + i*line_height)` in row's `fg`. Vertically centres glyph within the line-height band. Caller paints panel bg; rasteriser is text-only (matches GTK contract).

### Visual smoke tests added

Following the precedent set by `command_center` / `menu_bar` / `activity_bar` / `headless`, each of the three new rasterisers ships an `#[ignore]`d `dump_smoke_ppm` test that writes `/tmp/quadraui_<name>.ppm` and opens it in Preview. Each test's doc comment lists the visual invariants to eyeball. Run via:

```
cargo test -p quadraui --no-default-features --features macos -- --ignored --nocapture \
  macos::terminal::tests::dump_smoke_ppm \
  macos::text_display::tests::dump_smoke_ppm \
  macos::message_list::tests::dump_smoke_ppm
```

The PPMs capture: terminal cursor inversion + find/selection bands + scrollbar; text_display title strip + decoration palette + timestamp run + scrollbar; message_list `You:`/`AI:` chat with indented body rows.

### `MacBackend` cleanup

- Replaced four `mac_unimpl!` stubs (draw_terminal, draw_text_display, text_display_layout, draw_message_list) with real implementations.
- Removed the now-unused `mac_unimpl!` macro and the `// Drawing stubs` header comment referencing #38–#43.
- Removed `#[allow(dead_code)]` from `current_cg()` — every rasteriser uses it now.

### Cell attribute deferral

`Terminal` cells carry `bold` / `italic` / `underline` flags. The macOS rasteriser **does not honour them** in this PR — Core Text would need per-cell `CTFont` variants (or attributed-string attributes), and the cell grid is hot enough that we prioritised trait-shape parity. Documented in the module header; the same deferred text-attribute pass from the previous session would unlock these along with bold spans in text_display, italic in code editor previews, etc.

### Test count progression

| Checkpoint | macos:: tests | Full lib tests |
|---|---|---|
| Session start (after #41) | 146 (+4 ignored) | 633 |
| After #43 | 159 (+7 ignored) | 646 |

### macOS milestone status at end of session

| # | Ticket | Status |
|---|---|---|
| 37 | headless test surface | ✅ |
| 38 | chrome (status_bar / tab_bar / activity_bar / command_center / menu_bar) | ✅ |
| 39 | content (tree / list / form / editor / data_table / chart) | ✅ |
| 40 | MSV + scrollbar | ✅ |
| 42 | container + indicator (panel / split / toast / progress / spinner) | ✅ |
| 41 | overlay (tooltip / context_menu / dialog / palette / completions / find_replace / rich_text_popup) | ✅ |
| 43 | terminal + text_display + message_list | ✅ |
| 36 | platform services (clipboard / dialogs / notifications / URL open) | open *(independent)* |
| 44 | port all paired examples | open *(now unblocked)* |
| 184 | native menu bar via NSMenu | open *(depends on #36 FFI patterns)* |
| 185 | native right-click context menus | open *(depends on #184)* |

**Every in-window rasteriser on the `Backend` trait now has a Core Graphics implementation.** macOS apps using `quadraui::macos::run` paint every primitive. The remaining macOS milestones (#36, #44, #184, #185) are integration / native-feel work — no further rasterisers needed.

### Process notes

- Pre-existing clippy warnings on `develop` HEAD (`dispatch.rs` `collapsible_match`, `compose/sidebar_system.rs` + `compose/tree_controller.rs` `3_usize.max(1)`) trip `-D warnings` on every feature flag. Confirmed by stashing the #43 diff and running clippy on the bare branch — same errors. Not in scope for #43.
- CLAUDE.md says "the round-trip harness IS the smoke test" for primitive paint/click changes. The PPM dump tests added here go beyond that — they catch glyph baseline drift / antialiasing / overlay-flag visual regressions that the pixel-probe harness can't see by itself, and match the established pattern across the macOS rasterisers.

### Open queue for next session

- **#44** — port all paired examples to macOS. Newly unblocked. Many `macos_*` examples already exist (`macos_app`, `macos_demo`, `macos_data_table`, `macos_chart`, `macos_form_groups`, `macos_multi_tree`, `macos_split`, `macos_panel`, `macos_toast`, `macos_indicators`); audit which are missing (none today for terminal / text_display / message_list shapes) and port the gaps.
- **#36** — macOS platform services. Pure FFI work; natural lead-in to #184 / #185.
- **Text-attribute pass** (not yet filed). Now unblocking only polish work (bold / italic on terminal cells, span attrs across content rasterisers) — no longer gates feature completeness.
- **#184** / **#185** — native menus. Depends on #36 FFI shape.
- **Win-GUI milestone (#19–#31)** — nothing started.
- **#177** / **#180** — small TUI / GTK fixes still open.
- **Future / deferred:** #166, #144, #65, #118, #115.
