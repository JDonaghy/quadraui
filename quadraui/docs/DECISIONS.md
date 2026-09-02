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

## D-004 — `SplitTree` is a distinct primitive from `Split` (issue #435)

**Status:** Decided and shipped.

**Date:** 2026-07-16.

### Question

`Split` (`quadraui/src/primitives/split.rs`) is a two-pane container
with a draggable divider. vimcode's `GroupLayout`/`WindowLayout`
(`vimcode/src/core/window.rs`) need arbitrary N-way *recursive*
nesting of the same kind of divider — editor-group splits and in-group
vim window splits can nest to any depth. Do we generalise `Split`
itself (e.g. a `children: Vec<Split>` or a recursion flag), or ship a
new primitive?

### Decision

**Separate primitive.** `quadraui::primitives::split_tree::{SplitTree,
SplitTreeDivider, SplitTreeLayout, SplitTreeMeasure}`. `Split` is
unchanged.

### Why

Applying D-001's principle: a recursive N-way split tree is a
different UX/data concept from a fixed two-pane divider, not an
algebraic generalisation that should live behind a flag on `Split`.

1. **Different addressing model.** `Split` has exactly one divider and
   one `ratio` field on the struct itself. `SplitTree` has zero-to-many
   dividers, addressed by a stable pre-order `split_index` — the whole
   point of `set_ratio_at_index` / `adjust_ratio_at_index` /
   `parent_split_of` has no analogue in `Split`'s shape.
2. **Different leaf identity.** `Split`'s two panes are `first`/`second`
   by convention; a consumer paints into `first_bounds`/`second_bounds`
   directly. `SplitTree` leaves carry a `WidgetId` each, resolved via
   `SplitTreeLayout::leaves: Vec<(WidgetId, Rect)>` — an N-way host
   needs to know *which* leaf a rect belongs to, a two-pane host never
   does.
3. **A `children: Vec<Split>` shape would still need a second type.**
   Divider geometry for a tree (pre-order index, parent/child bounds
   propagation) is structurally different from a single divider's
   layout math — reusing `Split`'s `SplitLayout` (exactly one
   `divider_bounds` field) for an N-way tree doesn't type-check without
   changing its shape, which would break every existing `Split`
   consumer for a feature most of them don't need.
4. **Existing two-pane consumers stay untouched.** `Split` remains the
   simple, zero-recursion-overhead primitive for sidebar/editor,
   diff-view, and other genuinely-two-pane layouts — the common case
   isn't forced to pay for tree traversal it doesn't use.

### What's shared

`SplitTree` reuses `Split`'s existing `SplitDirection` type (not a new
enum) — same crate, same mental model, same TUI divider glyphs (`│`/
`─`) and GTK divider chrome. The two primitives' rasterisers
(`tui::draw_split` / `tui::draw_split_tree`,
`gtk::draw_split` / `gtk::draw_split_tree`) are siblings, not one
generalised over the other — see D-001's "share implementation via
internal helpers, not public type-level parameters" if a genuinely
shared internal helper emerges later (there wasn't enough duplication
yet to justify one at ship time).

### Cross-cutting infra landed alongside

`DragTarget::SplitDivider` (`quadraui/src/dispatch.rs`) and
`UiEvent::SplitDividerDragged` (`quadraui/src/event.rs`) — a divider
drag now goes through the same `dispatch_mouse_drag` path scrollbars
already use, instead of a backend hand-rolling `Option<usize>` drag
state (vimcode's `dragging_group_divider` / `dragging_window_divider`
pattern, duplicated per backend per tree today).

### What this does NOT mean

- We won't retrofit `Split` to be `SplitTree` with one `Split` node —
  the single-divider case is `Split`, not a degenerate tree, matching
  D-003's equivalent call for `MultiSectionView` vs. a single section.
- `SplitTree`'s ratio clamp (`MIN_RATIO`/`MAX_RATIO` = 0.1/0.9) is a
  simple ratio-based floor, not `Split`'s pixel-based
  `first_min`/`second_min`. If a consumer needs pixel-precise per-node
  minimums, that's an additive follow-up, not a reason to have blocked
  this primitive on designing it up front.

### Known gap (tracked, not blocking)

The macOS backend (`quadraui/src/macos/`) does not yet implement
`Backend::draw_split_tree`/`split_tree_layout` — that module only
compiles under `cfg(target_os = "macos")`, so it can't be authored or
verified from a Linux dev box. `win/backend.rs` has the standard
`todo!()` stub, matching every other not-yet-implemented Win-GUI
primitive.

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
similar tension between "share the algorithm" and "keep the concept
distinct."

---

## D-005 — `*_layout` coordinate-frame convention (issue #505)

### Question

`LESSONS.md` records a rule from a shipped bug (`mac_tree_layout` /
`mac_form_layout` silently returning absolute coords while their
TUI/GTK twins returned local, drifting clicks by `area.y` on macOS):
*"layout helpers return local-frame coords."* Issue #505 asked whether
that rule is actually followed trait-wide, or whether — as
`tab_bar_layout`'s already-documented absolute-coordinate exception
(#552) suggested — the real picture is more mixed and the doc comments
just don't say so.

### Audit

Every `Backend::<name>_layout` trait method in `quadraui/src/backend.rs`
was read against its TUI, GTK, and (where implemented) macOS
implementation, tracing whether the returned `hit_regions` / `bounds`
are shifted by `rect.x`/`rect.y` (**ABSOLUTE**) or left relative to
`rect`'s origin (**LOCAL**). Result, 20 methods audited (excludes
`tab_bar_layout*` and `activity_bar_layout`, already resolved by #552):

| Frame | Methods |
|---|---|
| **LOCAL** | `data_table_layout`, `status_bar_layout`, `text_display_layout`, `tree_layout`, `form_layout` |
| **ABSOLUTE** | `text_input_layout`, `msv_layout`, `menu_bar_layout`, `split_layout`, `split_tree_layout`, `panel_layout`, `toast_stack_layout`, `pipeline_view_layout`, `progress_layout`, `spinner_layout`, `command_center_layout`, `toolbar_layout`, `sidebar_panel_layout`, `chart_layout`, `minimap_layout` |

**No live cross-backend mismatch was found.** For every method, every
backend that implements it agrees with its siblings on which frame it
uses — unlike the historical `mac_tree_layout` bug, nothing here
silently drifts between backends today. What *was* missing: 19 of the
20 methods' trait doc comments said nothing at all about coordinate
frame (the sole exception, `status_bar_layout`, already had its
bar-local frame stated from #552), so the agreement was accidental
(preserved by copy-paste and `#499`'s later unification into shared
`layout_metrics` helpers) rather than contracted.

### Decision

**Keep both frames — do not collapse everything to LOCAL.** Converting
the 15 ABSOLUTE methods to LOCAL would be a breaking change to every
caller of every one of them (both in-tree composers and, per
`CLAUDE.md`'s downstream-consumers policy, `coord-tui` and `vimcode` if
either calls them directly) for a purely cosmetic uniformity gain — the
audit found no bug the conversion would fix, only a documentation gap.
Instead:

1. Each method's doc comment now states its frame explicitly, in the
   words **LOCAL** or **ABSOLUTE** (grep-able), matching the audited
   reality above. Landed in the same PR as this decision — see
   `quadraui/src/backend.rs`'s module doc "Coordinate frames for
   `*_layout` methods" and each method's doc comment.
2. `PRIMITIVE_RULES.md` gained a "Coordinate frames for `*_layout`
   methods" section stating the convention going forward: a new
   primitive's layout method picks LOCAL if a parent composer paints it
   inline and already tracks the origin, ABSOLUTE if it's painted as a
   freestanding widget at its own screen rect. `LESSONS.md`'s original
   rule is marked superseded and points here instead of re-asserting
   "always local."
3. `tab_bar_layout`'s ABSOLUTE convention (audited and fixed under
   #552, opposite of `activity_bar_layout`'s LOCAL one) is ratified by
   this decision, not treated as an outstanding exception to resolve —
   it's simply one more ABSOLUTE-frame method in the table above.

### What this does NOT mean

- It does not mean "frame doesn't matter." The rule that *does* still
  bind unconditionally: whichever frame a method picks, every backend
  implementing it must agree with its siblings, that agreement is
  stated on the doc comment, and it is regression-tested at a non-zero
  `rect.x`/`rect.y` (the case that hides a LOCAL/ABSOLUTE mixup, per
  the `mac_tree_layout` postmortem). A future primitive that lets its
  backends silently disagree on frame is still a bug of exactly this
  kind.
- It does not block deprecating an individual method's frame later —
  `PRIMITIVE_RULES.md` rule 8's two-PR deprecation protocol applies if
  a specific method's frame ever needs to change.

Future primitive decisions should cite this file when they hit the
same fork.

## D-006 — `Surface`/`ScreenLayout` is canonical; `Backend::draw_*` is the low-level entry point (issue #456)

### Question

`quadraui` exposes two independent ways to paint the same primitive:
`backend.draw_<name>(rect, &data)` directly, or
`layout.push(Surface::<Name> { rect, .. })` followed by `layout.draw(backend)`.
Nothing steers a consumer toward one, and nothing keeps a call site that
picks one in sync with a sibling call site (same app, other backend)
that picks the other. #456 found exactly that in vimcode: the TUI
palette painted via `b.draw_palette(...)`, the GTK palette via
`frame.push(Surface::Palette { .. })` — five of six surrounding lines
identical, only the paint call differing for no reason but that both
APIs exist. That drift contributed to vimcode#587 (a GTK paint function
with zero live callers going unnoticed).

### Audit

`SMELL_AUDIT_2026-07.md` §4 quantified the gap: `frame.rs`'s `Surface`
enum covers 25 primitives. 11 primitives that already have a
`Backend::draw_<name>` trait method have **no** `Surface` variant:
Board, DiffView, PipelineView, Toolbar, SidebarPanel, TextInput,
Spinner, Progress, CommandCenter, DropOverlay, MessageList. `Surface`
also only ever wraps primitive paints, not the helper draws that exist
alongside a primitive's main one (`draw_terminal_divider`,
`draw_settings_chrome`, `draw_tab_bar_with_chrome`,
`draw_activity_bar_with_style`, `draw_tooltip_with_chrome`), which have
no z-order slot of their own by design — they're always painted
adjacent to a primitive that does.

### Decision

**`ScreenLayout` + `Surface` is the canonical path for a consumer
assembling a top-level screen out of multiple primitives.** Pushing
`Surface` entries and calling `.draw(backend)` forces both backends'
call sites through the same list, and `zone_for` (shared by `draw` and
`hit_map`, see the doc comment on `ScreenLayout::zone_for`) guarantees
the hit-map matches what was painted — the class of "two backends, two
different paint calls, silently different behavior" bug #456 describes
becomes a compile-time non-option for anything routed through
`Surface`, because there's only one call site to route through.

**`Backend::draw_<name>` is not deprecated, hidden, or downgraded — it
stays public, documented API**, because:

1. `ScreenLayout::draw` calls it internally. `Surface` is sugar over
   `Backend::draw_*`, not a replacement for it; the trait method has to
   stay exactly as public as it is today or the "canonical" path has
   nothing to delegate to.
2. The 11 primitives listed above have no `Surface` variant yet, so
   `Backend::draw_*` is the *only* path available for them. A consumer
   painting `Toolbar` or `Progress` today is not taking a shortcut —
   there is no `Surface::Toolbar` to take instead.
3. Rasteriser tests (`PRIMITIVE_RULES.md` rule 4's paint/click
   round-trip harness) and compose helpers legitimately call
   `Backend::draw_*` directly — they're testing or composing the
   primitive itself, not assembling an app screen, and have no reason
   to go through `ScreenLayout`.

**Going forward: rule 7 in `PRIMITIVE_RULES.md` ("every primitive gets
a `Backend::draw_<name>` trait method") is extended one hop.** A new
primitive that participates in top-level screen composition (i.e. gets
a `draw_<name>` trait method that a consumer calls directly to paint
part of a frame) gets its `Surface::<Name>` / `FrameZone::<Name>` /
`zone_for` arm added in the *same PR* — see the new "One primitive, one
canonical paint path" section in `PRIMITIVE_RULES.md`. This stops new
drift; it does not retroactively backfill the 11-primitive gap above,
which is `SMELL_AUDIT_2026-07.md` §7 Epic D's `D4` ("trait symmetry"),
scoped separately because it touches 11 primitives' worth of
`Surface`/`FrameZone` plumbing rather than a single decision.

### What this does NOT mean

- It does not mean `Backend::draw_*` should get `#[doc(hidden)]` or
  `#[deprecated]`. vimcode's TUI palette call site
  (`b.draw_palette(...)`, the #456 evidence) is exactly the kind of
  call a primitive with no `Surface` variant, a rasteriser test, or a
  compose helper legitimately makes — hiding or deprecating the whole
  trait surface over an 11-primitive coverage gap would force a worse
  workaround than the drift it's meant to fix, and would breach rule
  8's downstream-impact bar for no compensating benefit (both
  consumers call `Backend::draw_*` methods directly today).
- It does not retroactively fix vimcode's existing palette divergence
  — that's vimcode's own migration (tracked as vimcode#587's
  follow-up). This decision fixes which API a *new* call site should
  reach for; it doesn't rewrite call sites in other repos.
- It does not block a primitive that is legitimately never composed
  into a multi-primitive frame (none identified so far) from staying
  `Backend::draw_*`-only indefinitely — the rule is "if it's composed
  into a `ScreenLayout` frame, it needs a `Surface` variant," not
  "every primitive must have one."
