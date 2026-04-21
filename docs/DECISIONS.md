# quadraui primitive-distinctness decisions

A running log of "do we introduce a new primitive, or reduce it to an
existing one with parameters?" decisions for the quadraui crate. Each
entry records the question, the call, and the reasoning so we don't
re-litigate and so the design stays coherent.

---

## D-001 — `ListView` is a distinct primitive (not `TreeView` with depth 0)

**Status:** Decided. Already shipped in Phase A.5 (commit `63d1b29`).
This memo records the retroactive rationale so D-002 and future
decisions have a precedent to cite.

**Date:** 2026-04-19.

### Question

Quickfix, symbol lists, references lists, diagnostics panes — all flat
scrollable row lists — could in principle be rendered by passing a
`TreeView` with every `TreeRow.indent = 0` and `is_expanded = None`.
Do we expose a separate `ListView` primitive, or reuse `TreeView`?

### Decision

**Separate primitive.** `quadraui::primitives::list::{ListView, ListItem,
ListViewEvent}` is distinct from `TreeView`.

### Why

1. **Discoverability matches the mental model.** Developers searching
   the crate for "list" should find a `ListView`. Every mainstream UI
   toolkit (GTK, Qt, SwiftUI, WPF, React Native, Flutter) exposes list
   and tree as separate types. Forcing a user to learn that "a list is
   a tree with no hierarchy" is an API-discoverability tax with no
   payoff.

2. **Event surface is narrower and more honest.** `TreeEvent` has
   `RowToggleExpand` and path-based selection (`TreePath`). A list
   cannot meaningfully toggle expansion and has no `TreePath` — only
   an index. `ListViewEvent` uses `idx: usize` and drops
   `RowToggleExpand` entirely. The type system now prevents impossible
   events; apps don't have to handle `RowToggleExpand` on a list and
   ignore it as dead code.

3. **Data shape is simpler.** `ListItem` has no `path`, no `indent`,
   no `is_expanded`, no `badge` (it has `detail` instead — different
   semantics). A plugin declaring a `ListView` in Lua via serde writes
   half as many fields as it would for a depth-0 `TreeView`.

4. **Rendering is simpler per backend.** `draw_list` doesn't compute
   chevron columns, indent math, or tree-path hit tests. GTK and
   Direct2D implementations are materially shorter, which matters
   because every primitive ships three backends.

5. **Styling decisions don't cross-contaminate.** Zebra striping,
   right-aligned detail columns, and compact row heights are list
   idioms; expand/collapse animations and guide lines are tree idioms.
   Keeping them on separate types lets each evolve without the other
   having to ignore fields.

6. **Plugin-invariant hygiene (design §10).** Smaller, purpose-built
   structs serialise to smaller Lua tables and reduce "which fields
   are ignored for this use case?" foot-guns.

### What we explicitly give up

- **One less primitive to maintain.** We now port ListView to every
  new backend (GTK ✅ via A.5b, Win-GUI via future A.5c, macOS via
  Phase C). This is the real cost of the decision — paid per backend,
  per primitive.
- **Improvements to the tree renderer don't flow to lists automatically.**
  If `TreeView` gains virtualisation, we add it to `ListView` separately.

### What this does NOT mean

- We won't try to derive one from the other later. If an internal
  shared helper emerges (e.g. a row-layout routine both `draw_tree`
  and `draw_list` call), fine — but the public API stays two types.
- We won't add a `flat: bool` or `hierarchy: Option<...>` knob to
  `TreeView` to cover list cases. That path leads to a god-object
  primitive.

---

## D-002 — `DataTable` vs. `TreeTable` (issue #140)

**Status:** Open. Recommended call below; defer final until the
TreeTable primitive (#139) starts.

### Question

Issue #140: should `DataTable` (flat multi-column) be a separate
primitive, or realised as `TreeTable` with all rows at depth 0?

### Recommended decision

**Separate primitive.** Apply D-001's rationale one level up:
list:tree :: DataTable:TreeTable.

### Why this is not just "apply D-001"

There is one real tension here that didn't exist for list/tree:
**column-sizing logic is a lot of code** (measure, resize, min/max
widths, flex distribution, header drag). Duplicating that across
`DataTable` and `TreeTable` is expensive in a way that duplicating
row-rendering was not.

**Resolution:** put column-sizing in a shared internal helper
(`quadraui::internal::columns` or similar, not public) that both
primitives call. Public API stays two types; implementation shares
the hard part.

### Implications for #140

- Build `TreeTable` first (#139, k8s app needs it). Extract column
  helpers as internal module while building it.
- Build `DataTable` second on top of those helpers. Public shape:
  `DataTable { id, columns, rows: Vec<DataRow>, selected_idx,
  scroll_offset }`, no tree-path, no expand/collapse.
- `DataTableEvent` mirrors `ListViewEvent` shape: idx-based, no
  `RowToggleExpand`.

---

## Principle (established by D-001, applied to D-002)

**One primitive per UX concept, not per algebraic reduction.**

If concept A is a strict subset of concept B's visual rendering but
carries different semantics (what events make sense, what fields are
meaningful, how users think about it), ship them as separate
primitives. Share implementation via internal helpers, not public
type-level parameters. This costs more per-backend porting work; the
payoff is discoverability, honest APIs, and a plugin-friendly serde
surface that holds up as the crate grows.

Future primitive decisions should cite this file when they hit the
same fork.
