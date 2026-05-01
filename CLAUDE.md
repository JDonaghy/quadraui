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

`main` is the only long-lived branch. Feature work happens on
`<kind>-<short-description>` branches; merge via PR after CI is green.
Versions live in each crate's Cargo.toml.

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
