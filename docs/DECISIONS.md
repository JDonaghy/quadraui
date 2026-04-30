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

## D-003 — `MultiSectionView` primitive (issue #293)

**Status:** Decided. Design pass complete; implementation in progress
on branch `issue-293-multi-section-view`.

**Date:** 2026-04-30.

### Question

Multi-section sidebars (vimcode's Extensions panel, Debug sidebar,
Source Control panel; future kubeui resource browser; future
Postman-clone collections list; VSCode-style Explorer with Open
Editors / Folder / Outline / Timeline) all share a shape: a
vertically-stacked stack of N sections, each with a title row and a
scrollable body. Today each backend hand-rolls the section walk,
scrollbar overlay, and click hit-test. Bugs from the divergence keep
landing — the #281 smoke wave alone surfaced four classes of
paint/click drift, every one a per-backend fix.

Do we add a `MultiSectionView` primitive to quadraui that owns the
whole layout (chrome + bodies + scrollbars + drag), or stay with the
current per-backend approach + better discipline?

### Decision

**New primitive.** `quadraui::primitives::multi_section_view::{
MultiSectionView, Section, SectionBody, SectionHeader, SectionAux,
SectionSize, ScrollMode, Axis, MultiSectionViewLayout, ... }`.

Not a vimcode-specific helper. Designed to serve any consumer of
quadraui — vimcode's three current panels are the validation set, but
the API targets the broader "vertical N-section sidebar" pattern that
shows up in every IDE, k8s client, API client, chat app, and
admin dashboard.

### Why

1. **The bug class is structural, not local.** The #281 smoke wave's
   four divergences (1.4× row drift, section_heights vs paint heights,
   HiDPI line_height mismatch, cached vs draw-closure line_height)
   were all "paint and click reading from different sources of truth
   for the same layout." A primitive that owns the layout removes the
   second source of truth. No discipline on per-backend code can make
   the same class impossible by construction; a primitive can.

2. **Three-plus consumers in vimcode alone today.** Extensions, Debug,
   Source Control. Plus future panels (per-window symbol outline?
   problems pane?) that would inherit the same shape. Plus a Win-GUI
   rebuild (B.6) and a possible macOS backend, both of which would
   re-clone the bug class without a shared primitive.

3. **Outside-vimcode consumers are real.** k8s client sidebar with
   Workloads / Networking / Storage / Config sections (#145).
   Postman clone with Collections / Environments / History / Mock
   sections (#147, #169). kubeui has already filed friction issues
   (#224) and is actively consuming quadraui primitives. This isn't a
   speculative cross-app payoff — apps the project already plans to
   build need this shape.

4. **Composes existing primitives, doesn't replace them.**
   `MultiSectionView` doesn't reimplement tree painting; it uses
   `TreeView` for tree-bodied sections, `ListView` for list-bodied,
   `Form` for settings-bodied, etc. The new code is the *orchestration*
   of N sections — sizing strategies, headers with action buttons,
   collapse/expand, divider drag, per-section vs whole-panel scroll.
   Each body type is unchanged.

5. **Cites D-001's principle.** A multi-section sidebar is a distinct
   UX concept from any of its constituent bodies. You can't get
   collapse + per-section sizing + divider drag + per-section scroll +
   header action buttons from a tree or list parameterisation; the
   semantics are different. One primitive per UX concept.

### Locked design choices (the seven decisions of the design pass)

#### 1. Body composition

`SectionBody` is an **enum of supported quadraui primitives** plus a
`Custom(WidgetId)` escape hatch:

```rust
pub enum SectionBody {
    Tree(TreeView),
    List(ListView),
    Form(Form),
    Terminal(Terminal),
    MessageList(MessageList),
    Text(StyledLines),
    Empty(EmptyBody),
    Custom(WidgetId),       // host paints in returned bounds
}
```

Built-in variants give the rasteriser everything it needs to paint
without host involvement; `Custom` lets apps drop in primitives we
haven't enumerated (or their own widgets) and paint them in the
bounds the layout returns. This matches the pattern in
`SectionBody::Tree(TreeView)` directly carrying the body data — no
indirection through trait objects.

#### 2. Scroll model

```rust
pub enum ScrollMode {
    PerSection,             // each body owns its scrollbar
    WholePanel,             // single scrollbar; sections size to content
}
```

No hybrid mode. `WholePanel` forces all sections to content-sized
semantics (any other `SectionSize` would be meaningless when the
container itself scrolls). `PerSection` is what every vimcode panel
uses today.

#### 3. Resize redistribute policy

**Fixed-on-drag.** When the user drags a divider between sections A
and B, both adjacent sections become `SectionSize::Fixed(measured)`.
Other sections are untouched and continue to honour their original
strategy. Container resize after a drag works because non-adjacent
flex sections still soak up the remainder.

We considered a "preserve strategy when both sides match" variant.
Rejected as YAGNI — promoting to it later is non-breaking if users
actually complain.

#### 4. Axis

```rust
pub enum Axis { Vertical, Horizontal }
```

Wired through the API and layout algorithm from day one (main-axis /
cross-axis terminology internally). **Vertical-only rasterisers in
v1.** Horizontal rasterisers tracked in #294. The cost of plumbing the
field is near-zero; the cost of breaking the API later to add it is
higher.

#### 5. Min/max enforcement

**Strict clamp during drag.** The divider stops at the threshold even
if the cursor moves past it. Mouse-up commits a position that always
matches a legal layout state. No "snap back on release" surprise.

#### 6. Header hit-test

```rust
pub enum HeaderHit {
    Chevron,                // explicit collapse/expand
    TitleArea,              // icon, title, badge — host decides intent
    Action(ActionId),       // right-aligned action button
}
```

Right-aligned action buttons are hit-tested first (so they "punch
through" the title area). Disabled actions are inert and fall through
to `TitleArea`. Splitting `Chevron` from `TitleArea` lets richer apps
wire them to different intents (chevron = toggle, title = focus +
toggle); simple apps wire both to toggle.

#### 7. Empty-state body

Rich struct from day one:

```rust
pub struct EmptyBody {
    pub icon: Option<Icon>,
    pub text: StyledText,
    pub hint: Option<StyledText>,
    pub action: Option<HeaderAction>,
}
```

Covers everything from a plain "No data" up to a VSCode-style welcome
view ("Open Folder" / "Clone Repository" buttons in an empty Source
Control panel) without a future API break. Centered + muted styling
applied by the rasteriser, not the host.

### Layout algorithm (three-pass)

1. **Fixed pass.** Sum `SectionSize::Fixed(n)`,
   `ContentClamped { min, max }` (clamped to content), and
   collapsed-section header heights. Subtract from container.
2. **Percent pass.** Allocate `SectionSize::Percent(p)` against the
   *original* container size (not the post-fixed remainder). On
   overflow, scale all percent allocations down proportionally.
3. **Flex pass.** Distribute remaining space across `Weight(w)`,
   `EqualShare`, and `Content`. `Content` gets its content size first;
   `Weight`/`EqualShare` share what's left by weight.

`min_size` floors and `max_size` ceilings are honoured per section.
On collision (fixed > container), a deterministic later-sections-lose
rule applies.

### What we explicitly give up

- **One more primitive to maintain.** Three backends × one new
  primitive. The vertical-only v1 holds at two backends shipping (TUI,
  GTK) until #294 lands horizontal.
- **No "free" extensions across primitives.** Improvements to
  `MultiSectionView` don't automatically improve `TreeView`. Same
  trade as D-001 / D-002.

### What this does NOT mean

- We won't try to express `TreeView` itself as a `MultiSectionView`
  with one section. The single-section degenerate case is a tree, not
  a multi-section view, and the API surfaces (events, hit-test,
  sizing) are different.
- We won't add a per-section `Sub: MultiSectionView` recursion. Nested
  multi-section panels are not a real-world pattern (file an issue if
  one shows up).
- We won't ship `Custom(WidgetId)` rasteriser dispatch — `Custom`
  means "host paints, we return bounds." If a custom body type is
  common enough to want shared painting, promote it to a first-class
  enum variant.

### Migration plan

Three vimcode panels migrate sequentially as the validation set:

1. **Extensions** — smallest, validates the primitive shape
   end-to-end with the simplest body composition (2× TreeView,
   `EqualShare`, no aux).
2. **Debug sidebar** — 4× TreeView, `EqualShare`, no aux. Verifies the
   #281 bug classes are gone by construction.
3. **Source Control** — 1× `Fixed(3)` aux=Input + N× TreeView,
   `EqualShare`. Stresses the `SectionAux::Input` pathway. Verifies
   Session 197's async-diff-open path still works.

---

## Principle (established by D-001, applied to D-002 and D-003)

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
