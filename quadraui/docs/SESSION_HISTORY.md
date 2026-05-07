# Session History

Archived session summaries. Newest at top.

---

## 2026-05-07d — TreeController inline text editing (#83)

**Agent:** Claude Opus 4.6 (1M context)

**Issue closed:** #83 (PR #84)

Added inline text editing support to `TreeController` and both
rasterisers. When a `TreeRow` has `edit: Some(TreeRowEditState)`,
backends render a text input (cursor, selection, char-by-char painting)
in place of the row's label and badge — no consumer-side rendering.

**Primitive layer:** `TreeRowEditState` struct with `text`, `cursor`,
`selection_anchor`. Added as `#[serde(default)] edit: Option<...>` on
`TreeRow`.

**Compose layer:** `TreeController` gains `EditingState` (private),
`start_editing()` / `cancel_editing()` / `is_editing()` public API,
modal key dispatch (editing mode swallows all keys, routes to text
manipulation helpers), and three new `TreeControllerEvent` variants:
`EditConfirmed`, `EditCancelled`, `EditChanged`.

**SidebarSystem propagation:** `build_view()` stamps editing state
onto the matching row, `CharTyped`/`ClipboardPaste`/`KeyPressed`
forwarded to the editing section's TreeController, matching
`SidebarEvent` variants added.

**TUI rasteriser:** Block cursor via cell inversion, selection via
`selected_bg`, following `InlineInput` pattern.

**GTK rasteriser:** Thin vertical caret bar (1.5px) via Pango prefix
measurement, selection highlight rectangle, following Form `TextInput`
pattern.

**Bug found during smoke test:** `SidebarSystem::build_view()` built
`TreeView` from raw `tc.rows()`, never stamping editing state. Both
rasterisers saw `edit: None` so editing was invisible. Fixed by
applying the same stamping in `build_view()`.

**Files touched:** 11 (primitive, compose, both rasterisers, lib.rs
re-export, 2 MSV test files, 3 example files).

**Tests added:** 14 (10 TreeController unit, 2 TUI paint, 2 GTK paint).
Test count: 518 (up from 504).

---

## 2026-05-07c — TreeController public API + scroll convention fix

**Agent:** Claude Opus 4.6 (1M context)

**Issues closed:** #77 (PR #81), #78 (PR #81), #79 (PR #82)

- **#77** Made `TreeController::scroll_by()` public with
  `(delta, viewport_rows)` signature — no Backend reference needed,
  matching `move_selection_by`, `jump_to_edge`, `page_scroll`.
- **#78** Double-click emits `RowActivated` — added `UiEvent::DoubleClick`
  handling in `TreeController::handle()`, hit-tests the tree and fires
  `TreeControllerEvent::RowActivated` for the clicked row.
- **#79** Fixed TUI scroll delta sign convention — positive `delta.y`
  now means "scroll content up" (toward top of document) in both TUI
  and GTK backends. TUI was inverted. Also added doc comments to
  `dispatch_scroll` and `SidebarSystem` documenting the convention.

**Files touched:** `compose/tree_controller.rs`, `tui/events.rs`,
`dispatch.rs`, `compose/sidebar_system.rs`.

**Tests:** 3 new `scroll_by` tests. TUI event round-trip tests updated
for flipped signs. Test count: 504 (up from 501).

---

## 2026-05-07b — StatusBar hover/pressed visual feedback

**Agent:** Claude Opus 4.6 (1M context)

**Issue closed:** #45 (PR #80)

Added `hovered_id: Option<&WidgetId>` and `pressed_id: Option<&WidgetId>`
parameters to `Backend::draw_status_bar` and both TUI + GTK rasterisers.
Follows the established pattern (ActivityBar `hovered_idx`, TabBar
`hovered_close_tab`) of passing per-frame interaction state alongside
the primitive rather than inside the struct.

- Hovered clickable segment: `bg.lighten(0.05)`
- Pressed clickable segment: `bg.darken(0.05)` (pressed takes precedence)
- Non-clickable segments: unaffected

Also added `Color::darken(amount)` as a symmetric counterpart to
`Color::lighten`.

**Files touched:** 19 (Backend trait, TUI + GTK + Win rasterisers/backends,
10 example AppLogic files, 2 kubeui consumers).

**Tests added:** 4 TUI tests (hover paint, pressed paint,
pressed-over-hover precedence, non-clickable ignores hover).
Test count: 501 (up from 497).

---

## 2026-05-06 (continued) — MenuOverlay refinements + CLAUDE.md split

**Continued from the MenuSystem triage session.**

**Additional fixes:**

6. **FontMetrics for overlay line_height** (`1df80ee`) — MenuOverlay
   measured lh with `pixel_size("Xy")` which undercounts for deep
   descenders. Switched to `FontMetrics` (ascent + descent), matching
   the runner pattern.

7. **Context menu border inset** (`1df80ee`) — 1px border stroke
   overlapped item content by 0.5px. Inset by 0.5px so border stays
   inside bounds.

8. **Removed Operator::Clear** (`a837640`) — the explicit surface
   clear caused Cairo sub-pixel text rendering artifacts (text thinner
   on first frame, bolder on composited repaint). GTK4's scene graph
   clears DrawingArea surfaces each frame, making the explicit clear
   redundant and harmful.

**CLAUDE.md split** (`4d312e5`) — 673 lines down to 98. Reference
sections extracted to `quadraui/docs/`:
- `ARCHITECTURE.md` — workspace layout, compose helpers, backend trait
- `PRIMITIVE_RULES.md` — 7 rules for adding/changing primitives
- `CONSUMER_PATTERNS.md` — MSV debug-sidebar and SC panel recipes
- `LESSONS.md` — durable rules from real failures + "What NOT to do"
- `TESTING.md` — coverage taxonomy, testability, quality gate

**Open item for vimcode:** two-phase dropdown rendering (first paint
clipped, second correct after ~250ms) is likely caused by duplicate
`menu_system.render()` calls — one in the main DA's draw function and
one in MenuOverlay. Vimcode needs to remove the main DA call. Not a
quadraui issue.

---

## 2026-05-06c — SidebarSystem selection nav + GTK menu fixes

**Issues closed:** #68, #69, #70.

- **#68** `NavigationMode::Selection` on `SidebarSystem` — j/k/Up/Down
  move `selected_path` with scroll-to-follow; Home/End/PageUp/PageDown
  jump; Enter → `RowActivated`. 17 tests. Re-exported `NavigationMode`
  at crate root.
- **#69** MenuOverlay viewport 0x0 fix — click/motion handlers call
  `begin_frame` with DA allocation before `menu_system.handle()`.
- **#70** Context menu descender clipping — split `draw_context_menu`
  into backgrounds pass then text pass.

---

## 2026-05-06 — GTK MenuSystem bug triage + MenuOverlay helper

**Context:** Vimcode's agent was failing to integrate quadraui's
MenuSystem on GTK. User brought the dialog for diagnosis.

**Issues diagnosed and fixed:**

1. **bar_rect coordinate mismatch** (vimcode bug) — the dropdown
   overlay DA's click/motion handlers forgot the negative-y offset
   that the draw function applied. Dropdown rendered correctly but
   was unclickable. Prompt given to vimcode agent to fix.

2. **draw_menu_bar vs menu_bar_layout height inconsistency**
   (quadraui bug, `62c65fd` then `03d2324`) — `draw_menu_bar` used
   `self.current_line_height`, `menu_bar_layout` used `rect.height`.
   When consumers passed a bar_rect taller than line_height (titlebar
   with command centre), the dropdown gap appeared. Fixed by making
   both use `rect.height` and vertically centering text in the
   rasteriser.

3. **MenuOverlay helper** (`d1708ef`, PR #67, closes #66) — new
   `quadraui::gtk::MenuOverlay` encapsulates ~120 lines of overlay
   DA boilerplate: DA property setup, coordinate transform, draw/
   click/motion wiring. Eliminates the coordinate mismatch and
   `set_focusable(false)` bug classes by construction.

4. **Dropdown item padding** (`65d85f0`, `9718f6f`, `08a9c99`) —
   items were exactly `line_height` tall with no padding (cramped on
   GTK). Added `lh * 1.4` multiplier with `.round()` to avoid
   fractional TUI cell positions. Separator height similarly rounded.

5. **Overlay surface clearing** (`e3cfa61`) — attempted fix for
   rendering artifacts. Later reverted (`a837640`) as the clear itself
   caused worse artifacts (see continued session above).

**Lessons captured in LESSONS.md:**
- Backend `draw_*` and `*_layout` must agree on dimensions
- Dropdown item sizing must use backend-native units (no absolute px constants)

**Issue #66** filed and closed (MenuOverlay helper).
