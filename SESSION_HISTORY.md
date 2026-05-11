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

- #120 — sidebar_search cursor movement + clipboard
- #65 — SplitDragController compose helper (deferred)
- #115 — quadraui-lua bridge crate (future)
- #118 — quadraui-ipc JSON bridge (future)
- Windows milestone (#19–#31)
- macOS milestone (#32–#44)
