# Consumer Patterns

Recipes for shaping common consumer integrations onto quadraui
primitives. Each pattern lives next to a runnable example AND a
consumer-state round-trip harness; new patterns are added here once
both gates pass.

## MSV with N stacked TreeView sections (Debug-sidebar shape)

The shape vimcode's Debug sidebar (Variables / Watch / Call Stack /
Breakpoints) and any "N collapsible tree panes" host wants.

- **Per-section state lives on the host.** Each section has its own
  `scroll_offset: usize` and `selected_path: Option<TreePath>` owned
  by the host's `AppState`/struct, NOT smuggled back into the
  primitive via `Cell<T>` engine fields. Primitives are declarative;
  the host rebuilds a fresh `MultiSectionView` from its state every
  frame.
- **Section sizing.** All sections `EqualShare`, `ScrollMode::PerSection`,
  `Axis::Vertical`. Headers without chevrons (`show_chevron: false`)
  match VSCode's Debug-sidebar styling.
- **Click routing.** Use the layout that paint produced — either
  via `backend.msv_layout(&view, area)` (if inputs are guaranteed
  identical to paint) or by caching the layout paint returned (if
  paint and click run in different contexts — see LESSONS.md "What
  NOT to do" for the two safe patterns). On `Body { section }`, fetch
  `layout.sections[section].body_bounds`, call
  `backend.tree_layout(&tree, body_area)` (or the cached tree
  layout), hit_test at `(x - body_b.x, y - body_b.y)`. Header
  click -> activate without selecting; body row -> activate AND
  select; empty body -> activate only.
- **Scrollbar routing.** The layout splits each gutter into three
  hit regions based on inner body scroll state:
  - `Scrollbar { kind: Thumb }` — capture `(section, origin_y,
    origin_offset, viewport_rows)` on press; on each `MouseMoved`
    write `new = origin_offset + (y - origin_y)` clamped to
    `[0, rows.len() - viewport_rows]` to
    `state.sections[section].scroll_offset` ONLY. Other sections
    must remain untouched.
  - `Scrollbar { kind: TrackBefore }` — page up by `body_bounds.height`
    rows.
  - `Scrollbar { kind: TrackAfter }` — page down by `body_bounds.height`
    rows.

  **Why `rows.len() - viewport_rows`, not `rows.len() - 1`?** The
  thumb saturates at the natural max (`fit_thumb` clamps `scroll/range`
  to `[0, 1]`), so dragging past it leaves the thumb idle while the
  inner `TreeView::scroll_offset` keeps advancing — yielding states
  where only the trailing row is visible. The natural-max clamp
  makes that mode unreachable. Tested in
  `tui::multi_section_view::tests::consumer_drag_past_natural_max_clamps_to_keep_viewport_full`.

Runnable: `quadraui/examples/msv_multi_tree.rs` (TUI) +
`quadraui/examples/gtk_multi_tree.rs` (GTK). Both are ~25-line
runner shells; the canonical `AppLogic` impl + state shape live in
`quadraui/examples/common/multi_tree.rs`. Harness:
`quadraui/src/tui/multi_section_view.rs::tests` ("Consumer-state
round-trip harness" block). All three must be updated together when
changing the pattern.

## MSV with aux=Input + N collapsible TreeView sections (SC panel shape)

The shape vimcode's Source Control sidebar (commit input + Changes /
Staged / Worktrees) and any "editor + grouped tree sections" host
wants. Different from the Debug-sidebar shape: aux row carries an
input editor, sections are collapsible, and section count is dynamic.

- **Section sizing.** Section 0 is `SectionSize::Fixed(N)` with
  `aux: Some(SectionAux::Input(...))` and `body: SectionBody::Tree(...)`
  (the commit input + first tree). Sections 1..N are `EqualShare` with
  `body: SectionBody::Tree(...)`. All sections have `show_chevron: true`
  so chevron clicks toggle collapse. `MultiSectionView::allow_collapse`
  is `true`.
- **Host state.** Adds `commit_message: String`, `commit_caret: usize`,
  `commit_input_active: bool`, plus a `collapsed: bool` per section
  (in addition to the per-section `scroll_offset` / `selected_path`
  the Debug-sidebar shape already needs).
- **Click routing.** On top of the Debug-sidebar routes:
  - `Aux { kind: Input }` -> set `commit_input_active = true`,
    `active_section = None`.
  - `Header { kind: Chevron }` -> toggle `state.sections[section].collapsed`,
    activate that section.
  - `Header { kind: TitleArea }` -> activate without selecting (same
    as Debug-sidebar shape).
  - Any other hit (Body, Scrollbar, Inert, Outside) -> blur the input
    (`commit_input_active = false`).
- **Keystroke routing.** When `commit_input_active`:
  - printable char -> insert at caret, advance caret.
  - Backspace -> delete char before caret, retreat caret.
  - Left / Right -> move caret.
  - Esc -> blur (`commit_input_active = false`).
  When `!commit_input_active`, keys behave as in the Debug-sidebar
  shape (Tab cycles section, up/down scrolls active section, etc.).
- **Collapsed semantics.** A collapsed section's `body_bounds.height`
  is `0`; its hit_regions never include a `Body` variant. The y-range
  that *would* be its body if expanded is occupied by the next
  section's chrome — there is no "would-be body click" to handle.
  Multiple gates in `MultiSectionView::layout` enforce this
  (size-resolution, body_main, hit-region emission); the
  consumer-state harness asserts the contract from outside.
- **Dynamic section count.** Sections can be added or removed at
  runtime; `cell_quantum` snapping holds for any section count
  (tested 1..=8). Build a fresh `MultiSectionView` with the current
  sections every frame — primitives are declarative.

Runnable: `quadraui/examples/msv_sc_panel.rs`. Harness:
`quadraui/src/tui/multi_section_view.rs::tests` ("SC panel" sub-block
within "Consumer-state round-trip harness").
