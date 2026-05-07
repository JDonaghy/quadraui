# Session History

Archived session summaries. Newest at top.

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
