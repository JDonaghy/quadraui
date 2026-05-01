# CLAUDE.md — quadraui

This is the agent-facing guide for working in the **quadraui** repo. It
covers what the library is, where things live, and the rules for changes
that touch primitives, rasterisers, or backend infrastructure.

This repo is **self-contained**: no consumer depends on quadraui from
inside the repo at compile time except the demo apps (`kubeui*`) which
are themselves part of the workspace. Vimcode and any other downstream
consumer pin a published version externally and trust the harness +
tests + examples here. Don't introduce assumptions about specific
downstream consumers.

## Session Start Protocol

1. Read `README.md` for the high-level shape (workspace, primitives, status).
2. Read `quadraui/docs/DECISIONS.md` for primitive-distinctness principles.
3. Read `quadraui/docs/BACKEND_TRAIT_PROPOSAL.md` §9 for the resolved
   decisions log.
4. Run `gh issue list --state open` to see active work.

## Development Workflow

All non-trivial work should be tracked via GitHub Issues. Issues are the source of truth for what needs doing, why, and what the design is.

**Documentation-only changes** (pure `.md` edits, or comment-only edits in source files) may be committed directly to `develop` and pushed. No branch, no smoke test, no path decision. This includes `README.md`, `CLAUDE.md`, `quadraui/docs/*.md`, and any other workspace-root `.md`. If any code changes accompany the doc edit — even a one-line code change — use the full branch workflow below.

**For all other changes (issue work, primitive changes, rasteriser changes, mini-app updates, release prep):**

1. **Always work on a local branch off `develop`.** Never commit code directly to `develop`. Branch naming:
   - Issue work: `issue-{number}-{short-description}` (e.g. `issue-6-menu-bar-rasterisers`)
   - Other work: `{kind}-{short-description}` (e.g. `feat-tree-headers`, `fix-msv-scrollbar`, `test-tabbar-harness`, `refactor-extract-tui-tab-bar-layout`)
2. Do the work on that branch, committing as you go. **Run the full quality gate before each commit** (`cargo build` / `cargo test --features tui` / `cargo test --features gtk` / `cargo clippy` / `cargo fmt --check` per the *Testing* section).
3. **Do NOT push the branch yet.** Keep it local until one of the following applies:
   - **(a)** The user has run smoke tests (e.g. `cargo run --example tui_app`, `cd kubeui && cargo run`, etc.) and confirmed the changes work, OR
   - **(b)** The user has explicitly agreed that smoke testing is not needed (e.g. test-only changes where the harness is the verification, doc-only changes, or refactors fully covered by existing tests).
   For primitive paint/click changes specifically, the **paint↔click round-trip harness IS the smoke test** — if the harness passes (and was empirically verified to catch the bug class — see *Lessons captured*), explicit (b) is appropriate.
   Offer smoke tests explicitly and wait for approval.
4. **Once approved, ask the user which landing path they want:**
   - **Path A — merge locally + push.** For small / trivial changes (test fixes, typo fixes, doc updates, single-line refactors): fast-forward-merge the branch into `develop` locally with `git merge --ff-only <branch>`, push `develop`, delete the branch. No PR, no separate review.
   - **Path B — push branch + open PR.** For normal feature, primitive, or rasteriser work, and anything closing an issue: push the branch, open a PR to `develop` with `gh pr create --base develop`. If the work closes an issue, reference it with "Closes #{number}" in the PR body. User reviews and merges.
5. **When the user confirms a merge that closes an issue**, immediately close the issue with `gh issue close <number> -c "Implemented in PR #N"` — do not rely on GitHub auto-close.

**Why both paths exist:** Path A is lower ceremony for changes so small that a PR review adds no information (a 2-line test fix where the fix *is* the verification). Path B is the default for work that warrants a review artifact, closes an issue, or is large enough that someone might want to see the diff separately from the merge commit. **When in doubt, default to Path B.** Primitive shape changes, new rasterisers, harness additions, and any change to the public API all warrant Path B even when small.

**Creating issues:**
- At session end, create issues for any planned but unstarted work discussed during the session.
- Include full design context in the issue body — file paths, primitive shape, expected behavior, harness requirements, where to look in existing rasterisers as a template.
- Use labels for categorization (`enhancement`, `bug`, `documentation`, etc.).
- Issues should be self-contained — a new session should be able to pick one up and implement it from the issue body alone.

**Bug fixes found during other work:**
- If a bug is found while working on something else, create a separate issue for it.
- Fix it on the current branch if it's small and directly related, or leave it for a separate branch if it's independent.

**Cross-repo prereq tracking:**
- If quadraui work is blocked on a downstream consumer change (rare, but possible), or if a vimcode issue is blocked on quadraui work (common during the harness-first phase), label the issue `blocked` and reference the prereq in the body as `<owner>/<repo>#<N>`.
- The `/plan-next` skill resolves these cross-repo links and reports which `blocked` issues are now ready to unblock (all prereqs CLOSED).

## Architecture

**Workspace members:**

| Crate | Purpose |
|---|---|
| `quadraui/` | Core library: primitives, types, theme, backend traits, TUI + GTK rasterisers. |
| `kubeui-core/` | Domain logic for a Kubernetes dashboard demo. No rendering deps — testable in isolation. |
| `kubeui/` | TUI Kubernetes dashboard. Real consumer of every TUI rasteriser. |
| `kubeui-gtk/` | GTK Kubernetes dashboard. Same domain logic as `kubeui`. |

**Two-layer split** inside `quadraui/`:

- `quadraui/src/primitives/` — backend-agnostic widget descriptions +
  layout functions + hit_test. **Must NOT depend on `ratatui`, `gtk4`,
  `cairo`, or any backend crate.** Tests live as inline `#[cfg(test)]
  mod tests` blocks at the bottom of each primitive file.
- `quadraui/src/tui/` and `quadraui/src/gtk/` — backend rasterisers.
  Each consumes the primitive's `layout()` and paints verbatim. **Must
  NOT contain layout decisions** that the primitive could express.

**Backend trait** in `quadraui/src/backend.rs` plumbs frame state and
the `set_current_theme` / `set_nerd_fonts` setters that hosts call once
per frame. Per-primitive `draw_*` functions are free functions in the
backend modules; the trait is for cross-cutting state.

**`set_cell` / `cairo_rgb` / `ratatui_color` / etc.** are private
backend helpers — never call them from primitives.

## Primitive Authoring Rules

When adding or changing a primitive:

1. **Declarative description first.** Add the struct + `Layout` +
   `hit_test` to `quadraui/src/primitives/<name>.rs`. Tests for layout
   correctness go at the bottom of that file as inline tests; they must
   not pull in any backend feature.
2. **One source of truth for layout.** Both rasterisers AND tests must
   call the primitive's `layout(...)` — never re-derive bounds inline.
   Where a backend needs additional metrics (like TUI's `cell_quantum`),
   they go on `LayoutMetrics` so the primitive can apply them.
3. **Both backends, same shape.** Add the rasteriser to
   `quadraui/src/tui/<name>.rs` AND `quadraui/src/gtk/<name>.rs`. Even
   if the GTK consumer hasn't been written yet, add a stub rasteriser
   so the primitive's contract is honoured on every backend it claims
   to support.
4. **Paint↔click round-trip harness.** Every primitive that has clicks
   must have a paint↔click round-trip test in its TUI rasteriser
   (`quadraui/src/tui/<name>.rs::tests`). The pattern: paint into a
   `ratatui::Buffer`, find painted glyphs, hit_test those exact
   coordinates, assert paint and click identify the same region.
   Examples to copy: `tui::multi_section_view::tests`,
   `tui::tree::tests`. The harness must catch the bug class it's
   designed for — verify by temporarily mutating the rasteriser to
   break the contract; the harness should fail. Restore + commit only
   when both sides round-trip cleanly.
5. **Public layout helper.** Each backend exposes a public layout
   helper (e.g. `tui_msv_layout`, `tui_tree_layout`) so consumers can
   drive hit-testing without re-deriving metrics. The rasteriser uses
   this same helper internally — paint and consumer-driven hit_test
   consume one source of truth.
6. **Test state-derived paint geometry.** Whenever a painter computes
   a position from primitive state — scrollbar thumb position from
   `(scroll_offset, content_rows, viewport_rows)`, focus ring from
   `has_focus`, badge offset from text length, animation phase from
   a tick counter — write a test that paints at a known state and
   asserts the painted indicator lands where the formula predicts.
   The paint↔click harness covers coordinate-system drift; this rule
   covers paint-formula bugs (e.g. "thumb hardcoded at top of gutter
   instead of computed from scroll_offset"). Verify empirically by
   mutating the formula and observing the test fail. See *Coverage
   taxonomy* under *Testing* for the full bug-class breakdown.

## Consumer patterns

Recipes for shaping common consumer integrations onto quadraui
primitives. Each pattern lives next to a runnable example AND a
consumer-state round-trip harness; new patterns are added here once
both gates pass.

### MSV with N stacked TreeView sections (Debug-sidebar shape)

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
- **Click routing.** Call `tui_msv_layout(&view, area)` once per
  click. On `Body { section }`, fetch `layout.sections[section].body_bounds`,
  call `tui_tree_layout(&tree, body_area)`, hit_test at
  `(x - body_b.x, y - body_b.y)`. Header click → activate without
  selecting; body row → activate AND select; empty body → activate
  only.
- **Drag routing.** On `Scrollbar { section }`, capture
  `(section, origin_y, origin_offset)`. On each `MouseMoved`,
  recompute `new = origin_offset + (y - origin_y)` clamped to
  `[0, rows.len() - 1]` and write to `state.sections[section].scroll_offset`
  ONLY. Other sections must remain untouched — that's the test
  the consumer-state harness verifies.

Runnable: `quadraui/examples/msv_multi_tree.rs`. Harness:
`quadraui/src/tui/multi_section_view.rs::tests` ("Consumer-state
round-trip harness" block). Both must be updated together when
changing the pattern.

## Testing

**Lib tests:** `cargo test --features tui` (or `--features gtk`).
Primitive layout tests run with no features.

**Required for any change touching a primitive or rasteriser:**

```bash
cargo build --features tui --features gtk
cargo test --features tui
cargo test --features gtk    # If GTK runtime is available
cargo clippy --features tui -- -D warnings
cargo clippy --features gtk -- -D warnings
cargo fmt --check
```

If GTK runtime libraries aren't available locally, the GTK feature won't
build — that's a CI concern. Run TUI checks at minimum.

### Coverage taxonomy

Three bug classes, three test shapes. An agent picking up an issue
should map the work to the relevant rows and add tests accordingly —
no per-issue restatement needed.

| Bug class | Test shape | Lives in |
|---|---|---|
| **Coordinate drift** between paint and click (paint computes one set of bounds, hit_test computes another) | Paint↔click round-trip — paint into the backend's headless surface, find a painted glyph, hit_test that exact coordinate, assert the hit identifies the painted element. | `tui/<name>.rs::tests` and `gtk/<name>.rs::tests`. Templates: `tui::multi_section_view::tests`, `tui::tree::tests`. |
| **Consumer-side click-routing drift** (the host translates a primitive hit into state mutation; that translation can drift from paint independently of the primitive's own correctness) | Consumer-state round-trip — paint, simulate the consumer's click handler, assert the host's state mutation matches the painted UI. | Adjacent to the consumer pattern. Template: `tui::multi_section_view::tests` "Consumer-state round-trip harness" block. |
| **State-derived paint geometry** (the painter computes a position from primitive state — thumb from scroll_offset, focus ring from has_focus, badge offset from text length) | Painted-indicator test — set state to a known value, paint, find the indicator in the buffer/surface, assert it lands at the position the formula predicts. | Same module as the rasteriser. |

Every primitive needs (1). Primitives with consumer-pattern recipes
need (2) when those patterns land. Primitives with state-derived
indicators need (3) per *Primitive Authoring Rule #6*.

**Each test must be empirically verified by mutation.** Break the
contract (zero out the offset, swap a +/-, paint at the wrong y),
observe at least one test fail, restore. A green test that doesn't
catch its bug class is theatre — see *Lessons captured* /
"Theory-only iteration doesn't converge".

### Backend testability requirement

Every backend MUST support headless paint-to-memory so tests don't
need a real display, terminal, window manager, or font server. A
backend that only paints to a live window is not shippable.

- TUI: `ratatui::Buffer` (in-memory char + style cells).
- GTK: `cairo::ImageSurface::create(Format::ARGB32, w, h)` + Pango
  layout queries.
- Windows (when implemented): `ID2D1Bitmap` as offscreen render
  target.
- macOS (when implemented): `CGBitmapContext`.

This is a design requirement on each backend, not a discovered
property. New backends ship with their harness on day one.

### What unit tests don't cover

Animation cadence, font-rendering quirks across host platforms,
terminal-specific edge cases (kitty vs xterm vs urxvt), exact color
choices, accessibility heuristics, "does this feel right". These
remain manual smoke / human review. Goal: every story ratchets
harness coverage forward so the manual-residue surface shrinks
toward "things that genuinely need eyes". Manual smoke is the
fallback, not the gate.

**Mini-app validation:** if the change affects a primitive consumed by
the kubeui demos, verify they still build:

```bash
cd kubeui && cargo build
cd kubeui-gtk && cargo build  # if GTK is available
```

## Code Style

- `rustfmt` defaults (4-space indent).
- `PascalCase` types, `snake_case` functions/vars.
- Tests in `#[cfg(test)] mod tests` at file bottom.
- Doc comments on public types/functions; `//!` module headers describe
  intent + invariants.

## Commit conventions

`<type>(<scope>): <imperative summary>`. Examples:

- `feat(quadraui): add TreeView column headers`
- `fix(quadraui): MSV scrollbar bounds clip body width correctly`
- `test(quadraui): TUI tree paint↔click round-trip harness`
- `refactor(quadraui): extract tui_tree_layout helper`
- `docs(quadraui): update DECISIONS.md with D-004`

Scope is `quadraui` for library changes, `kubeui` / `kubeui-gtk` /
`kubeui-core` for demo changes. Cross-cutting (e.g. workspace Cargo.toml,
CI, license) can omit the scope or use `(workspace)`.

## Branching + releases

Two long-lived branches:

- `main` — released/stable. Only updated by release merges from `develop`.
- `develop` — integration branch. All feature work merges here first (per *Development Workflow* above).

## Lessons captured

  Durable rules that came out of real failures. Each one is load-bearing —
  read at session start; apply as you work. New lessons get appended
  (date + one-line incident summary). Lessons don't get removed unless
  they turn out wrong.

  ### Paint↔click drift is the structural bug class quadraui exists to eliminate

  When a primitive's paint code computes one set of coordinates and its
  hit_test code computes another, every consumer eventually sees "I
  clicked X but Y was selected" — and each consumer works around it in a
  different ad-hoc way. The library's job is to make that impossible by
  construction:

  - **One layout, two consumers.** Paint and hit_test must consume the
    *same* `Layout` instance. Never let either side re-derive bounds.
  - **Coordinate-system agreement is structural.** When a backend rounds
    to a discrete unit (TUI cells, integer pixels), the layout itself
    must snap to that unit *before* emitting bounds — not at paint time.
    See `LayoutMetrics::cell_quantum` for the TUI case.
  - **Round-trip coverage proves agreement.** A test that paints, finds
    the painted region, and hit_tests at that exact position is the only
    test that catches drift across paint/hit_test formulae. Unit tests of
    either side alone will miss it.

  ### The band-aid trap

  When a consumer hits a paint/click drift bug mid-migration, the
  tempting "fix" is to cache layout inputs on the consumer's state
  (`Cell<T>` or similar) so click reads what paint wrote. **This
  perpetuates the bug class.** Two code paths still derive the same
  answer from the same inputs; if they ever diverge in inputs *or* in
  derivation, the bug returns in a new shape.

  The structural fix is always one derivation — inside the primitive's
  `layout()` (preferred) or on a `Backend`-owned cache shared by paint
  and hit_test. Per-consumer caches ship the problem; they don't solve it.

  ### Theory-only iteration doesn't converge

  When a migration breaks and the agent can't run the consumer (common
  with TUI apps needing a real terminal), each "plausibly correct from
  code reading" fix ships a new bug. The only escape is a harness that
  exercises paint→hit_test agreement automatically, in unit-test time,
  without a human in the loop. **The harness is the gate.** Don't ship
  a primitive change or a consumer migration without one.

  ### Migration discipline (corollary)

  Migrating a consumer onto a primitive is a contract change, not a
  refactor. The consumer commits to the primitive's `layout()` as the
  source of truth for widget bounds. Every migration MUST add a
  *consumer-state* round-trip test before merging:

  1. Paint the consumer's MSV / TreeView / etc. into a buffer.
  2. Find painted regions in that buffer.
  3. Simulate the consumer's click handler at those coordinates.
  4. Assert consumer-state mutation matches the painted UI — right
     section activated, right item selected, right scroll offset moved.

  A green primitive test does not prove the consumer integration is
  correct.


## What NOT to do

- **Don't add per-consumer Cell-on-state fields for paint→click
  bridging.** That perpetuates the two-source-of-truth bug class
  primitives exist to eliminate. If paint and click need to agree on
  state, it lives on the primitive's `Layout` struct, not on the
  consumer's app state.
- **Don't import vimcode-specific patterns.** The library is pre-1.0
  but we still want it API-stable enough that downstream consumers
  besides vimcode can adopt it. If a primitive feels too "vimcode-
  shaped" (e.g. references `vimcode::core::*` patterns), simplify.
- **Don't migrate a consumer onto a primitive without writing the
  paint↔click harness first.** The harness is the gate. This is the
  rule that came out of the Sessions 343–346 #296 smoke wave in vimcode
  — see [vimcode's PLAN.md](https://github.com/JDonaghy/vimcode/blob/develop/PLAN.md)
  "🧭 Course correction" if you need the historical context.
