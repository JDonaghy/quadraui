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

- #16 — Rasterisers for 4 remaining descriptor-only primitives: panel (next), toast, progress, spinner
- #7 — SearchPanel primitive spike (exploratory)
- GTK list rasteriser has same sub-pixel text issue (same `move_to` pattern without `.round()`) — follow-up
