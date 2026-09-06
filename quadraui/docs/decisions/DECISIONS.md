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
| **ABSOLUTE** | `text_input_layout`, `msv_layout`, `menu_bar_layout`, `split_layout`, `split_tree_layout`, `panel_layout`, `toast_stack_layout`, `pipeline_view_layout`, `progress_layout`, `spinner_layout`, `command_center_layout`, `toolbar_layout`, `sidebar_panel_layout`, `chart_layout`, `minimap_layout`, `command_line_layout` (added by #705, after this audit; `CommandLineLayout::text_origin_x` carries `rect.x`/`bounds.x`, same convention as `text_input_layout`) |

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

## D-007 — `Backend` trait symmetry: missing `*_layout` methods, off-trait rasteriser fns (issue #506)

### Question

#456/D-006 found that `Surface` and `Backend::draw_*` were two paint
paths with nothing steering a consumer toward one. #506 is the
trait-internal half of the same audit: does every primitive with a
real `layout()` also expose a `Backend::<name>_layout` method, the way
`data_table_layout` / `tree_layout` / `chart_layout` etc. already do?
And are there rasteriser entry points that exist only as backend-crate
free functions (`tui_board_layout`, `mac_list_layout`, ...) with no
trait-level home at all — the "off-trait rasteriser fns" CLAUDE.md's
portability commitment §1 calls a bug, since a Windows/macOS author
can't discover them by reading the trait?

### Audit

Six primitives had a `draw_<name>` but no `<name>_layout` twin, despite
a real `layout()` existing somewhere in the stack:

| Primitive | What existed before #506 |
|---|---|
| `BoardModel` | `board_layout` free fn (`primitives/board.rs`); every backend already wrapped it in its own off-trait helper (`tui_board_layout` / `gtk_board_layout` / `mac_board_layout`) — nothing put it on the trait |
| `ListView` | `ListView::layout()` (`primitives/list.rs:176`); TUI/GTK computed it *inline* inside `draw_list` (no reusable fn at all); Win/macOS had off-trait `win_list_layout` / `mac_list_layout` |
| `Editor` | `Editor::layout()` (`primitives/editor.rs:524`); every backend's `draw_editor` already calls it with `(self.char_width(), self.line_height())` |
| `Terminal` | `Terminal::layout()` (`primitives/terminal.rs:251`, plus `TerminalLayout::hit_test` / `cell_bounds`); **no backend called it at all** — `draw_terminal` iterates cells directly and callers had to re-derive `rect.width / char_width` by hand to hit-test a click |
| `DiffView` | No standalone `layout()`; `DiffViewLayout { visible_rows, total_rows }` was computed by hand, identically, inside every backend's `draw_diff_view` (header-row reservation in side-by-side mode, `+1` per hunk in unified mode) |
| `Palette` | `Palette::layout()` (`primitives/palette.rs:273`); Win/macOS had off-trait `win_palette_layout` / `mac_palette_layout`. **See "Palette: deferred, not missed" below — this one did not get a trait method.** |

Off-trait rasteriser entry points named in the issue, re-audited against
the *current* tree (some had already been resolved by earlier work on
this branch before #506 started):

| Entry point | Resolution |
|---|---|
| `draw_terminal_divider` | Already a `Backend` method on all four backends (no default — `PRIMITIVE_RULES.md` rule 7). The issue text predates this; no action needed. |
| `draw_settings_chrome` | Same — already a required `Backend` method (TUI + GTK implement it; no consumer needs it on Win/macOS yet). No action needed. |
| `tui_dialog_layout` | **Exempt, by design.** `Dialog` is an *overlay-with-caller-anchor* primitive (see the convention table below) — same class as `Tooltip`/`ContextMenu`/`Completions`. The host builds its own `DialogMeasure` from portable `Backend::line_height()` and calls `dialog.layout(...)` directly (see `examples/common/modal_occlusion_demo.rs::dialog_layout`); no `Backend::dialog_layout` exists for any of those primitives, and Dialog shouldn't be the odd one out. `tui_dialog_layout` is TUI's *own* internal default measurer for dialogs TUI paints without a caller-supplied `DialogMeasure` — a backend-private convenience, not a missing trait method. |
| `draw_context_menu_with_submenus` (TUI only) | **Exempt, written down, not promoted.** GTK has no cascading-submenu rasteriser yet (#371). A trait method here would need a default for GTK/Win/macOS, and CLAUDE.md's portability commitment explicitly rejects a no-op default that silently hides a real capability gap (`draw_terminal_divider`'s doc, `docs/SMELL_AUDIT_2026-07.md` PORT-01) — unlike `TabChrome`/`ActivityBarStyle`, ignoring `submenu_path` isn't a legitimate "no chrome vocabulary" fallback, it's "multi-level menus silently don't cascade." Also has zero call sites today (no compose helper or example wires it up) — it's a tested capability with no production consumer. Revisit once #371 gives GTK a real submenu rasteriser to pair it with; promoting it before that would ship a trait method that is honest for one backend out of four. |
| `mac_palette_layout` | **Deferred alongside `palette_layout` — see below.** |
| `mac_list_layout` | **Resolved.** Now the real backing implementation of `Backend::list_layout` for macOS (this issue). Kept as a `pub fn` — same "public free fn is the primitive-side thin wrapper" shape `data_table_layout`'s implementers already use; not deprecated, since nothing about its own contract changed. |
| `gtk::MenuOverlay` | **Exempt, decided.** This is a GTK-hosting/compositing helper (coordinate transform for GTK's native overlay-widget positioning), not a primitive rasteriser — it implements no primitive's `layout()`/paint contract, so there is no `draw_<name>` for it to pair with. TUI/Win/macOS have no equivalent concept because they don't have GTK's overlay-widget model to bridge into. Same category as `AppShell`'s GTK widget-tree bootstrap: legitimately backend-specific, and rule 7 ("every primitive gets a trait method") doesn't apply to it because it isn't a primitive. |

### Palette: deferred, not missed

`palette_layout` looks like the same fix as `list_layout` — add the
missing trait method, wire each backend's existing helper into it. The
audit found a reason not to do that yet: **GTK's `draw_palette` paints
item rows at `rows_y + i*line_height`, computed independently of the
`PaletteLayout` struct's own `visible_items[i].bounds.y`.** When
`show_query` is true, GTK reserves an extra 1px for the query/list
separator stroke *outside* the `Palette::layout()` call (baked into
`rows_y`, not into the `query_height` argument), so the struct's own
`bounds.y` under-reports the real paint position by that 1px. This is
harmless today only because `draw_palette` returns `()` — nothing
outside the function ever reads the mismatched struct.

Shipping `Backend::palette_layout` now would make that latent 1px drift
externally visible the moment a host trusted it for hit-testing —
exactly the "paint and no-paint silently disagree" bug class rule 5
("one source of truth for layout") and the `mac_tree_layout` postmortem
in `LESSONS.md` both exist to prevent. TUI's `draw_palette` doesn't
call `Palette::layout()` at all (its border/query/separator rows
predate the shared D6 layout refactor other primitives went through),
which is a second, larger version of the same problem: a `tui_palette_layout`
built from scratch today would be a parallel reimplementation, not an
extraction, with no guarantee it matches what `draw_palette` actually
paints.

**Decision: do not add `Backend::palette_layout` in this PR.** Fixing
`gtk::draw_palette` to consume `PaletteLayout.visible_items[i].bounds`
directly (eliminating the independent `rows_y` recomputation) and
refactoring `tui::draw_palette` to route through `Palette::layout()`
are both rendering-behavior changes, not trait-symmetry ones — real
work, tracked as a follow-up, that must land *before* a `palette_layout`
trait method can honestly claim to match its `draw_palette` twin. Rule
5 exists precisely so a new `*_layout` method is never the first thing
to notice a paint function's internal layout was already lying.

### Decision: the convention table

Three conventions coexist on the trait, and #506 asked which primitive
class picks which. Audited against every `draw_<name>` / `<name>_layout`
pair on the trait:

| Primitive class | Convention | Examples |
|---|---|---|
| **Content-in-rect** (backend paints inline, inside a caller-supplied `rect`, and can compute the same layout without a live paint context) | **Paired**: `draw_<name>` + `<name>_layout`, both taking `rect`. | `data_table_layout`, `tree_layout`, `form_layout`, `list_layout`, `board_layout` (new), `terminal_layout` (new), `editor_layout` (new) |
| **Overlay-with-caller-anchor** (host computes anchor/viewport/measure itself and calls the primitive's own `.layout(...)` directly; the backend only paints at the resolved bounds) | **Draw-takes-layout**: `draw_<name>(&self, thing, layout)`, no `Backend::<name>_layout` at all — there's nothing backend-specific to compute. | `Tooltip`, `ContextMenu`, `Completions`, `RichTextPopup`, `Dialog` (see `tui_dialog_layout`'s exemption above) |
| **Interactive chrome** (freestanding widget painted at its own screen rect; used to have `draw_<name>` return its own layout with no separate no-paint accessor) | **Draw-returns-layout is redundant with paired — collapsed.** Every primitive that used to be draw-returns-layout-only now also has the paired `<name>_layout` twin, so a host can ask for hit-test geometry without a frame in progress. `BoardModel`/`DiffView` were the two hold-outs (`draw_board` / `draw_diff_view` already returned their layout struct, but had no no-paint twin) — `board_layout` closes this for `BoardModel`; `diff_view_layout` closes it for `DiffView`. | `chart_layout`, `toolbar_layout`, `sidebar_panel_layout`, `board_layout` (new), `diff_view_layout` (new) |

A fourth shape showed up during the audit that the original three-way
split didn't anticipate: **counts, not coordinates.** `diff_view_layout`
returns `{ visible_rows, total_rows }` — there is no LOCAL/ABSOLUTE
frame to pick because nothing it returns is a position. Its doc
comment says so explicitly rather than silently defaulting to one.

**Defaults, where the formula is provably uniform across backends.**
`terminal_layout`, `editor_layout`, and `diff_view_layout` all turned
out to be pure functions of `Backend::char_width()` / `line_height()`
(plus, for diff views, `DiffView::mode`) — exactly the values every
backend's own `draw_terminal` / `draw_editor` / `draw_diff_view`
already resolves them to. Rather than hand-copy the same three-line
body into `TuiBackend`, `GtkBackend`, `MacBackend`, and `WinBackend`
(and every test `MockBackend`), these three got a **default trait
body**, each backed by a real paint-vs-layout parity test in
`tui/backend.rs` / `gtk/backend.rs` — see the doc comments on
`Backend::terminal_layout` / `editor_layout` / `diff_view_layout` for
the per-method contract, and
`{tui,gtk}::backend::tests::{terminal_layout_reserves_scrollbar_gutter_matching_draw_terminal,
editor_layout_matches_draw_editor_*, diff_view_layout_matches_draw_diff_view_*}`
for the tests themselves (issue #506 review fix — a first pass at this
paragraph asserted these were already "verified byte-for-byte" and
covered by "parity tests," which was aspirational, not true: no such
tests existed at the time, and `terminal_layout`'s default body turned
out to have exactly the drift the missing test would have caught —
next paragraph). This is new territory for a `*_layout` method — every
prior one required an explicit per-backend override — so
`tests/conformance/caps.rs`'s `ACCEPTED_DEFAULTS` list (quadraui#492's
"no silent no-op defaults" honesty check) carries all twelve `(backend,
method)` pairs with the reason, so a future backend that overrides one
of these three without a good reason still shows up as a
source-vs-declaration mismatch, not a silently-accepted default. The
review fix below added a thirteenth method, `terminal_scrollbar_default_width`
— a helper `terminal_layout` calls internally, not a `*_layout` method
itself — with three more `ACCEPTED_DEFAULTS` entries (GTK/macOS/Win; TUI
overrides it, so needs none), for fifteen total.

**`terminal_layout`'s scrollbar gap (found at review, fixed before
merge).** Unlike `Editor::layout` — which already takes the vertical
scrollbar's presence into account internally — `Terminal::layout` is a
bare `viewport_width / cell_width` division with no scrollbar concept
at all; the scrollbar-gutter reservation happens entirely in each
backend's `draw_terminal`, *before* it calls the cell-iteration logic
(`cell_area_w = area.width.saturating_sub(sb_cols)` on TUI,
`cell_area_w = (rect.width - sb_width).max(0.0)` on GTK/macOS/Win). The
first version of `terminal_layout`'s default body called
`term.layout(rect.width, …)` directly — the *unreduced* width — so
`TerminalLayout::grid_cols` came out too wide by the scrollbar's column
count whenever `term.scrollbar` was `Some`, meaning `hit_test` would
report a click on the scrollbar gutter as a valid cell. Fixed by adding
`Backend::terminal_scrollbar_default_width` (default `8.0`, matching
GTK/macOS/Win's `unwrap_or(8.0)`; TUI overrides it to `1.0`, matching
its own `unwrap_or(1)`) and having `terminal_layout` reduce the
viewport width by the scrollbar's reserved width — `sb.width` when the
caller set it, else that default — before calling `Terminal::layout`,
mirroring every backend's real `draw_terminal` order of operations. The
parity tests named above pin a `Terminal` with `scrollbar: Some(..)`
against this exact formula so the drift can't reopen silently.

`board_layout` and `list_layout` do **not** get a default: `BoardMeasure`'s
column/card sizing and `ListView`'s scrollbar-reservation logic are
backend-native constants (GTK's card height is 64px, not a multiple of
`line_height()`), the same reason `TreeStyle::row_height`'s backend
derivation isn't a single formula either. Every backend implements
these two explicitly, mirroring `draw_board`'s existing "no default —
a backend that forgets this silently reports an empty board" rule
(quadraui#600, `PRIMITIVE_RULES.md` rule 7).

### Surface-enum gap list

D-006 already tracked and quantified this (`SMELL_AUDIT_2026-07.md` §4:
`Board`, `DiffView`, `PipelineView`, `Toolbar`, `SidebarPanel`,
`TextInput`, `Spinner`, `Progress`, `CommandCenter`, `DropOverlay`,
`MessageList` have `Backend::draw_*` but no `Surface` variant), scoped
to #456 as "Epic D's `D4`, trait symmetry, separately." #506 is that
D4 follow-up for the *trait* half of the gap; the `Surface`/`FrameZone`
half stays #456's to close — this decision doesn't duplicate or
re-quantify it, only cross-references it so a reader doesn't go
looking for a second gap list here.

### What this does NOT mean

- It does not mean every off-trait free fn in a backend crate is a bug.
  `tui_board_layout` / `gtk_board_layout` / `mac_board_layout` /
  `win_list_layout` / `mac_list_layout` all stay exactly as public as
  they were — they're each backend's own thin wrapper that the new
  trait method calls into (`data_table_layout`'s implementers already
  established this shape). The bug #506 fixes is a rasteriser
  capability with **no trait-level path at all**, not "a free function
  exists alongside a trait method."
- It does not mean `palette_layout` is cancelled — see "Palette:
  deferred, not missed" above. It's blocked on a rendering fix, not a
  design question, and should land as its own follow-up once GTK's
  `draw_palette` and TUI's `draw_palette` both consume `Palette::layout()`'s
  own returned bounds instead of recomputing paint positions
  independently.
- It does not promote `draw_context_menu_with_submenus` preemptively.
  That waits on #371 (GTK cascading submenus) so the eventual trait
  method is honest on more than one backend out of four.

## D-008 — Dead-API disposition pass: `modal.rs`, per-primitive `*Event` enums, `FocusGroup`/`FocusRing`, `drop_zone` (issue #509)

### Question

`SMELL_AUDIT_2026-07.md`'s in-repo scan flagged `primitives/modal.rs`,
13(ish) of 27 `primitives::*Event` enums, `drop_zone.rs`'s non-`DropOverlay`
half, `FocusGroup` vs. `FocusRing`, and the full terminal split-layout /
`dispatch_scroll` family as zero-consumer. But that scan only grepped
this repo — quadraui is a library with two out-of-repo consumers
(`vimcode`, path-dep on `develop`'s tip; `coord-tui`, git-rev-pinned),
and "zero in-repo consumers" is not "dead": several of these turned out
to be exactly the load-bearing API a consumer depends on, which an
in-repo-only grep can't see. #509 is the gate that actually checked
`~/src/vimcode/src` and `~/src/coord-tui/src` before deleting anything,
per this repo's downstream-consumer policy (`CLAUDE.md` → *Downstream
consumers*).

### Method

For every candidate symbol: `grep -rlw '<symbol>' src examples tests`
in quadraui (word-boundary — the original audit's plain `grep -l` had
false positives, e.g. `PanelEvent` substring-matching
`SidebarPanelEvent`), then the same word-boundary grep against
`~/src/vimcode/src` and `~/src/coord-tui/src`, then manual inspection
of each hit — several names (`Modal`, `DropZone`) collided with an
unrelated local identifier in vimcode's own `core::window` module and
had to be confirmed as *not* `quadraui::` usages by reading the
surrounding code, not just counting grep hits.

### Disposition table

**DELETE — zero consumers anywhere, no partial wiring found.** All 14
removed in this PR (`git log` / diff is the source of truth for exact
edits): `CompletionsEvent`, `ContextMenuEvent`, `DialogEvent`,
`DiffViewEvent`, `ModalEvent` (via the `modal.rs` deletion below),
`PanelEvent`, `ProgressBarEvent`, `RichTextPopupEvent`,
`SidebarPanelEvent`, `SpinnerEvent`, `SplitEvent`, `ToastEvent`,
`ToolbarEvent`, `TooltipEvent`, and `MenuBarEvent`. That's 14 non-Modal
enums plus `ModalEvent`, not the audit's "13" — the audit's own count
was off by one; the true in-repo-only-referenced set includes
`MenuBarEvent`, which the audit missed because `event.rs` mentions it
in a doc comment (`grep -l` counted that as a "file reference" even
though no code ever constructs `MenuBarEvent::` anything). Each of
these enums duplicated information a real, actively-used `*Hit` +
`*Layout` pair on the same primitive already exposes (`PanelHit`,
`DialogHit`, `ToolbarHit`, `MenuBarHit`, …) — the pattern this codebase
actually shipped is "app calls `<Primitive>Layout::hit_test`, matches
on `<Primitive>Hit`, decides what to do," not "backend constructs a
`<Primitive>Event` and hands it back." None of the 14 had a single
non-definition, non-`lib.rs`-re-export reference anywhere in quadraui,
and zero hits in either `vimcode/src` or `coord-tui/src`. Deleted
alongside each: the enum's own serde-roundtrip test where one existed
(`DialogEvent`, `SidebarPanelEvent`, `ToolbarEvent`), and doc-comment
prose that named the deleted variant — rewritten to point at the real
`*Hit` mechanism instead of just deleting the sentence, so each
primitive's "Backend contract" doc section stays accurate.

**KEEP, documented as unshipped future wiring — do not delete.** Eight
enums *are* wired into the canonical [`crate::UiEvent`] contract
(`event.rs` has a real `UiEvent::Tree(WidgetId, TreeEvent)`-shaped
variant, not just a doc mention) but no backend anywhere in this repo
ever constructs that variant: `TreeEvent`, `ListViewEvent`,
`TabBarEvent`, `StatusBarEvent`, `TerminalEvent`, `TextDisplayEvent`,
`ChartEvent`, `DataTableEvent`. This is the #473-style "unshipped
design" case the issue asked to be recorded rather than deleted:
removing the variant would shrink `UiEvent`'s surface (a real breaking
change for any exhaustive-ish match, `#[non_exhaustive]`
notwithstanding) for a primitive whose event-bubbling path a future
backend author is expected to fill in, not one that was never
designed. Compare the four enums with genuine, exercised consumers —
`ActivityBarEvent`, `FormEvent`, `PaletteEvent` (constructed in
`dispatch.rs`), `PipelineEvent` (constructed in
`examples/common/pipeline_app.rs`) — proving the `UiEvent::Primitive(id,
PrimitiveEvent)` bubbling shape does work end-to-end once a backend
actually wires it; these 8 are the same shape with the wiring half
done. Follow-up: file an issue when a backend needs one of these eight
primitives to participate in the `UiEvent` bus (tree/list/tab-bar
click-to-select routed through `AppLogic::handle` instead of the app
polling `*Layout::hit_test` itself), rather than leaving this paragraph
as the only record of intent.

**`primitives/modal.rs` — DELETE (whole file).** `Modal` / `ModalHit` /
`ModalLayout` / `ModalEvent`: zero consumers outside `lib.rs`'s
re-export and the module's own tests, zero hits in vimcode or
coord-tui, and — found independently while fixing the resulting doc
breakage — `compose/help_layer.rs` had *already* documented working
around `Modal`'s deadness (`# Why not the old Modal primitive?`: "no
`Backend::draw_modal` method, and no backend ever painted one," built
its cheatsheet overlay on `Panel` instead). That in-repo doc predates
this issue and independently reached the same conclusion `modal.rs`
was never wired past its own struct + tests, which is stronger
evidence than the grep alone. Not "given rasterisers + trait method"
(the PRIMITIVE_RULES.md rule 7 alternative the issue offered) because
nothing in either consumer or this repo has ever wanted a
backdrop+centered-box overlay through this specific shape — every real
modal-ish overlay in this repo (`Dialog`, `Palette`, `ContextMenu`,
`RichTextPopup`) is its own primitive with its own working rasteriser,
arbitrated for hit-precedence by [`crate::ModalStack`] (which is a
*different*, actively-used module — see next paragraph). Deleted the
`pub mod modal;` declaration, the `lib.rs` re-export, and the
`lib.rs`-resident `modal_layout_*` / `modal_hit_test_*` tests.

**`modal_stack.rs` (`ModalStack`, `ModalEntry`) — KEEP, not part of the
above.** The issue's problem statement flagged `ModalEntry` itself as
zero-external-refs, distinct from the (used) `ModalStack`. Confirmed:
`ModalEntry` never appears by name in either consumer, and — checked
directly, not assumed — neither does `pop_top()`. vimcode's
`ModalStack` call sites (`~/src/vimcode/src/tui_main/mouse.rs`,
`~/src/vimcode/src/tui_main/render_impl.rs`,
`~/src/vimcode/src/gtk/mod.rs`) all construct/consult `ModalStack`
directly, but only via `.push(id, bounds)`, `.pop(id)` (the
bool-returning removal-by-id method, *not* `pop_top()`), and
`.hit_test(point)`; `grep -rn 'pop_top\|ModalEntry'` against both
`~/src/vimcode/src` and `~/src/coord-tui/src` returns zero hits in
either. So no consumer touches `ModalEntry` directly *or* indirectly
today. Kept anyway because it's `ModalStack::pop_top()`'s return
type — a real, if currently uncalled, method on a heavily-used type —
and could be needed the moment a consumer starts calling it; deleting
it would just mean re-adding it under time pressure later for no
present benefit.

**`primitives/terminal.rs` (`TerminalHit`, `TerminalLayout`,
`TerminalCellSize`) — KEEP; audit was stale.** These three were flagged
zero-consumer, but by the time #509 ran, #506 (the immediately prior
issue on this branch) had already given `Backend::terminal_layout` a
default trait body that calls `TerminalLayout`/`TerminalHit` for real,
with both TUI and GTK backends now exercising it
(`{tui,gtk}::backend.rs`, see D-007's terminal-layout paragraphs above)
— that's genuine, current in-repo usage, not a doc mention. Zero
external hits in either consumer, but `TerminalCellSize` is also the
field type of `TerminalLayout::cell_size`, so it's structurally
required regardless.

**`TerminalSplitHit` / `TerminalSplitLayout` — KEEP; audit was simply
wrong for these.** Heavily used in vimcode: `quadraui::TerminalSplitHit`
appears in `~/src/vimcode/src/render.rs` and
`~/src/vimcode/src/tui_main/mouse.rs` (bottom-panel split-pane hit
routing), `quadraui::TerminalSplitLayout` is a field type on vimcode's
own engine state (`core/engine/mod.rs`) and gets constructed in
`render.rs`. This is the clearest example in this pass of why the
external-consumer gate exists: an in-repo-only audit would have deleted
a type a consumer's core rendering path depends on.

**`dispatch_scroll` / `ScrollSurface` / `SurfaceScrollbar` — KEEP;
audit was wrong for these too.** All three are constructed extensively
in vimcode (`tui_main/mouse.rs`, `tui_main/panels.rs`,
`tui_main/render_impl.rs`, `tui_main/shell_app.rs`, `gtk/mod.rs`,
`core/engine/mod.rs` all reference `quadraui::{dispatch_scroll,
ScrollSurface, SurfaceScrollbar}`) as vimcode's primary scroll-dispatch
mechanism. Zero hits in coord-tui (which doesn't do scroll-surface
registration at all yet), but one real consumer with heavy production
use is enough.

**`primitives/drop_zone.rs` — KEEP everything, not just `DropOverlay`.**
The issue flagged `compute_drop_zone` / `drop_zone_overlay` /
`DropZone` / `DropZoneKind` / `DropEdge` / `DropGroupRect` as having
"exactly one internal consumer" (`compose/tab_group.rs`) and zero
external. That claim does not hold for four of those six symbols:
`compute_drop_zone`, `DropZoneKind`, `DropEdge`, and `DropGroupRect`
each have a real, direct external consumer.
`~/src/vimcode/src/render.rs` calls `quadraui::compute_drop_zone(cursor_x,
cursor_y, &rects, tab_bar_height)` inside its own
`compute_tab_drop_zone` helper, matches the result against
`quadraui::DropZoneKind::{Center, Split, TabReorder}` and
`quadraui::DropEdge::{Left, Right, Top, Bottom}`, and both constructs
and stores `quadraui::DropGroupRect` values (as a struct field type and
via direct construction) — all confirmed by reading the call sites,
not just counting grep hits. Plain `DropZone` is the one symbol among
the six where the issue's "one internal consumer" framing is
accurate as far as *that name* goes: vimcode does have its own
`core::window::DropZone`, but it's a same-named, unrelated local type,
not a use of `quadraui::DropZone` — confirmed by reading the call
sites. So the real picture is: `DropZone` and `drop_zone_overlay` have
exactly the one internal consumer the issue described; `compute_drop_zone`,
`DropZoneKind`, `DropEdge`, and `DropGroupRect` additionally have a
production external consumer and must be treated as breaking-change
surface, not in-repo-only helpers, if their shape ever changes.
Regardless of that split, the disposition here is unchanged: keep
everything in this file. That one internal consumer is
`TabGroupController`, a real, fully-tested, actively-maintained compose
helper whose own public methods (`handle_tab_drag_move`,
`handle_tab_drag_drop`, `drop_zone_at`, `drop_group_rects`) return or
consume these types directly. `DropOverlay` additionally has a genuine
external consumer (`quadraui::DropOverlay { .. }` constructed directly
in `~/src/vimcode/src/tui_main/render_impl.rs` and
`~/src/vimcode/src/gtk/mod.rs`) but *not* via `drop_zone_overlay()` —
vimcode builds `DropOverlay` values itself rather than calling the
helper. None of this is "dead": it's a working internal composition
layer with two adopted pieces (`DropOverlay`, and now the four symbols
identified above) and the rest not yet adopted externally, which is a
different disposition than "nothing anywhere ever calls this." "Zero
*external* consumers" is not the bar this repo's own downstream policy
sets for in-repo library code with a real in-repo caller — see
`CLAUDE.md`'s distinction between "no in-tree use" (free to remove) and
everything else (breaking-change rules apply) — and for
`compute_drop_zone`/`DropZoneKind`/`DropEdge`/`DropGroupRect` it isn't
even the right bar to apply, since they clear the *external*-consumer
bar directly. `TabGroupController` itself isn't referenced by name in
either consumer yet (both have their own hand-rolled tab-drag/drop
logic — vimcode's is `core::window::DropZone`, a same-named but
*unrelated* local type, confirmed by reading the call sites, not just
counting grep hits), so *that* composition is prototyped-but-not-adopted
rather than proven-in-production; nothing here changes as a result of
this pass.

**`compose::focus_group::FocusGroup` vs. `compose::focus_ring::FocusRing`
— reduced to one cycling implementation, both public types kept.** The
issue's acceptance bar is "reduced to one implementation." Both types
have real, distinct in-repo call sites with real, distinct
requirements: `FocusGroup` backs `sidebar_system.rs` and `tab_group.rs`,
both of which need index-based cycling over a **dynamically-resized**
run of anonymous regions (`FocusGroup::set_count` gets called at
runtime as tab-group panes are added/removed —
`compose/tab_group.rs`'s `self.focus.set_count(new_n)` call sites are
not incidental, they're load-bearing) with no natural `WidgetId` to key
by. `FocusRing` backs `examples/common/form_groups.rs`, which needs
exactly the opposite: a fixed list of named fields, looked up by the
`WidgetId` a form hit-test already returns
(`self.focus.set(id)`/`self.focus.current().cloned()` used directly
against `Form::focused_field`). Deleting either would force a real
consumer either to fake `WidgetId`s for anonymous indexed regions, or
to fake indices for named fields it doesn't have — not a cleanup, a
regression. Neither type has an external consumer (`FocusGroup`: 0 in
both; `FocusRing`: 0 in both — `macos/events.rs`'s "SidebarSystem /
FocusRing" mention is a comment, not a call site), so "merge or delete
one" from the issue is satisfied by collapsing the *duplicated cycling
arithmetic* — the actual thing the two module docs admitted was
duplicated — into one implementation: `FocusRing` is now a thin
`WidgetId`-keyed wrapper over a `FocusGroup`
(`compose/focus_ring.rs`), so `FocusGroup::cycle`'s modulo/wrap-around
logic is the *only* cycling implementation in the crate. `FocusRing`
preserves its exact prior behaviour — including the one real semantic
difference from raw `FocusGroup::cycle` (`advance`/`retreat` on a
*cleared* ring must stay a no-op, whereas `FocusGroup::cycle` on an
*unfocused* group jumps to the first/last item — `FocusRing`'s wrapper
guards on `group.active().is_some()` before delegating to preserve
this) — verified by every pre-existing `focus_ring::tests` case passing
unchanged against the new implementation.

### What this does NOT mean

- It does not mean "zero in-repo consumers" is a safe signal to delete
  on by itself, ever again, for this crate. Four separate items in this
  pass (`TerminalHit`/`TerminalLayout`/`TerminalCellSize`,
  `TerminalSplitHit`/`TerminalSplitLayout`, `dispatch_scroll` /
  `ScrollSurface` / `SurfaceScrollbar`, `DropOverlay`) would have been
  wrongly deleted by the original in-repo-only audit. The gate this
  issue enforced — grep both `~/src/vimcode/src` and
  `~/src/coord-tui/src`, then read the surrounding code to rule out
  same-named-but-unrelated local identifiers — is the actual
  bar, not a formality.
- It does not mean every remaining `pub` item with zero external
  refcount is safe to delete in a future pass without repeating this
  same two-consumer grep. `~215` other `lib.rs` re-exports the original
  audit flagged as zero-in-repo-consumer are untouched by #509 — this
  issue's scope was `modal.rs`, the per-primitive `*Event` enums,
  `FocusGroup`/`FocusRing`, and `drop_zone.rs` specifically, not the
  full re-export list.
- It does not mean the 8 UiEvent-embedded-but-unconstructed enums
  (`TreeEvent` et al.) are cancelled or that a future cleanup pass
  should sweep them next. They're recorded here as intentionally kept;
  removing them later needs its own issue and its own justification,
  not a re-read of this one.

## D-009 — Minimal error channel for backend authors: `Unsupported` vs `PlatformFailure` vs `SurfaceLost` at frame/event/services seams (issue #507)

### Question

The crate has no error type anywhere in its public API — fallible
operations use `Option` (`show_file_open_dialog`, `Clipboard::read_text`,
`Color::from_hex`), `bool` (the four CSD window-chrome methods, "false
means no-op, not an error" by documented design), or `()` with a silent
no-op (every `draw_*`; `win/backend.rs`'s still-`todo!()` rasterisers
abort where a real backend would need to report a device-lost error). A
Direct2D backend must be able to surface device-lost / swapchain-recreate
somewhere; today the trait has structurally nowhere for it to go. Does
quadraui need an error type, and if so, at which trait seams, and what
does it cost existing call sites?

### Decision

**Yes, a minimal three-variant `BackendError`, added at exactly two
seams — a polled frame/event error and `PlatformServices`' three
dialog-ish methods — both additive, neither breaking.** Draw methods
(`draw_*`) and the four CSD window-chrome `bool` methods stay exactly as
they are; #492's `BackendCaps` already resolved the "is this a real gap
or a genuine no-op" question those two areas raise, and re-litigating
them into `Result` would spend a breaking change on a distinction the
crate already has a non-breaking answer for. See "Why not
Result-ify draw_* / the CSD bools" below.

```rust
/// A backend-reported failure at a frame, event-loop, or
/// PlatformServices seam. Not used by draw_* (see D-009) or by the
/// four CSD bool methods (BackendCaps already disambiguates their
/// N/A case). `Clone`, not `std::error::Error` — `context` is a
/// short, ungrepped human string for logs/error UI, not a machine-
/// matched code; backend authors compose it from whatever the native
/// API gave them (`HRESULT`, `GError`, `errno`) with `format!`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackendError {
    /// This backend has no implementation of the surface being asked
    /// for. Prefer a `BackendCaps` field where the gap is a whole
    /// method; reach for this only where the gap is data-dependent
    /// within a method BackendCaps already declares supported (e.g. a
    /// `MessageDialogOptions` shape this backend's native dialog API
    /// can't represent, even though `native_dialogs` is `true`).
    Unsupported,
    /// A native call failed for a reason this backend cannot recover
    /// from inside the current call. `context` names the failing
    /// native call/API, e.g. `"IDXGISwapChain::Present"`,
    /// `"GtkFileChooserNative"`, `"CreateNotifyIcon"`.
    PlatformFailure { context: String },
    /// The render surface/device was lost mid-frame (D3D
    /// `DXGI_ERROR_DEVICE_REMOVED`, a destroyed GTK `GdkSurface`, …)
    /// and must be recreated before the next `begin_frame`. Distinct
    /// from `PlatformFailure` because callers handle it differently —
    /// recreate-and-retry, not log-and-continue.
    SurfaceLost,
}
```

**Seam 1 — frame + event loop: a polled `last_error()`, not a
`Result`-returning `begin_frame`/`end_frame`/`wait_events`.** The
candidate signatures named in #507 were rejected for cost:
`begin_frame(&mut self, viewport: Viewport) -> Result<(), BackendError>`
and `end_frame(&mut self) -> Result<(), BackendError>` would force every
caller of both (the runtime loop in every one of the four backends, plus
`vimcode`'s own GTK/TUI draw-cycle call sites) to add `?`/`match`
handling immediately, for a class of failure
(`SurfaceLost`) that only Win-GUI can produce today and that the other
three backends would have to `Ok(())` around forever. `wait_events` is
worse: it's documented to "never block" beyond `timeout` and return an
empty `Vec` on timeout — folding "no events" and "the event source
itself failed" into one `Result<Vec<UiEvent>, BackendError>` return
means every call site's existing `for event in backend.wait_events(t)`
loop has to become a `match` before it compiles, for a failure mode
(TUI: crossterm's terminal FD closing; GTK: none, callback-driven) that
is vanishingly rare. Instead:

```rust
/// The most recent BackendError this backend recorded, if any, since
/// the last time this method was called. Default: always None — a
/// backend that never sets an internal error field (TUI, GTK today)
/// answers this exactly like it doesn't exist, at zero cost to
/// existing callers. A backend that can hit SurfaceLost/PlatformFailure
/// (Win-GUI, once #21's device-lost recovery lands) sets an internal
/// field from begin_frame/end_frame/poll_events/wait_events and clears
/// it here — clear-on-read, matching every other "drain and reset"
/// method already on this trait (poll_events itself).
fn last_error(&mut self) -> Option<BackendError> {
    None
}
```

One method, not "a frame one and a separate events one" — `begin_frame`,
`end_frame`, `poll_events`, and `wait_events` all funnel into the same
field, because a caller that wants to notice backend trouble at all
polls once per loop iteration (after `end_frame`, conventionally) and
doesn't need to know which of the four calls produced it; the `context`
string inside `PlatformFailure` carries that detail when it matters. A
default-provided method with a trivial always-`None` body costs existing
implementors nothing (rule 7's "no default" stance is for primitives
with real per-backend behavior — this is closer to `read_primary_selection`'s
already-precedented "most backends have nothing to say here" default).
Win-GUI is the only backend with a concrete near-term producer
(`end_frame`'s already-planned device-lost recovery, `backend.rs:179`'s
"can still be re-created" comment) and overrides it when #507's
follow-up issue lands that recovery for real.

**Seam 2 — `PlatformServices`: additive `_result` twins, not
signature changes.** `show_file_open_dialog`, `show_file_save_dialog`,
and `show_message_dialog` keep their existing `Option<...>` signatures
(cancel and "no native facility" stay merged there, as documented, and
`BackendCaps::file_dialogs`/`native_dialogs` remain the way a caller
tells them apart *before* calling). Each gets a new sibling method with
a default body that just wraps the old one, so no implementor (in-tree:
`Tui`/`Gtk`/`Win`/`MacPlatformServices`; out-of-tree: none —
`PlatformServices` is implemented only by this crate's own backends, per
`PRIMITIVE_RULES.md` rule 8's consumer table) has to change to keep
compiling:

```rust
type ServiceResult<T> = Result<T, BackendError>;

fn show_file_open_dialog_result(&self, opts: FileDialogOptions) -> ServiceResult<Option<PathBuf>> {
    Ok(self.show_file_open_dialog(opts))
}
fn show_file_save_dialog_result(&self, opts: FileDialogOptions) -> ServiceResult<Option<PathBuf>> {
    Ok(self.show_file_save_dialog(opts))
}
fn show_message_dialog_result(&self, opts: MessageDialogOptions) -> ServiceResult<Option<MessageDialogChoice>> {
    Ok(self.show_message_dialog(opts))
}
```

`Ok(None)` is "the user cancelled" (unchanged meaning); `Err(..)` is new
information no caller could get before. A backend that can tell
cancel apart from a native API failure (Win-GUI's `IFileOpenDialog::Show`
returning an `HRESULT` that isn't the user-cancelled code; GTK's
`GtkFileChooserNative` response codes) overrides the `_result` method
directly instead of the old one, and now has somewhere to put that
information. This is rule 2's "new function alongside the old one
instead of a rename" — the cheapest non-breaking shape on
`PRIMITIVE_RULES.md`'s list — chosen over rule 3's deprecate-then-remove
two-PR protocol because there is no removal end-state in view: the
`Option`-returning methods stay useful for every caller (both
consumers, today) that only cares about cancel-vs-picked and has no use
for a failure reason. `Clipboard::read_text` is explicitly **not**
given this treatment: its `None` already means one thing only ("empty
or non-text clipboard"; "platform error" was never a real, distinguished
case any backend actually reports today), so adding a fallible twin
here would create a channel with nothing to say. Revisit only if a
backend author hits a concrete clipboard failure that needs surfacing.

### Capability vs. error — the split, made explicit

#492's `BackendCaps` and this decision answer two different questions
and must not collapse into one:

| Question | Answered by |
|---|---|
| "Can this backend do X *at all*, structurally?" (known ahead of the call, doesn't change frame to frame) | `BackendCaps` — `window_chrome`, `pointer_cursor`, `file_dialogs`, `native_dialogs`, `native_menu`, `ime`, `notifications`, `text_selection` |
| "Did *this specific call* fail, and can I tell why?" (only knowable at call time) | `BackendError` via `last_error()` / the `*_result` `PlatformServices` twins |

`BackendError::Unsupported` exists for the narrow gap between these two
— a method `BackendCaps` says is supported in general but that can't
service one particular request (a dialog shape the native API has no
representation for) — not as a second way to spell "this backend never
implements X," which is what a `BackendCaps` field of `false` already
says, mechanically checked by `tests/conformance/caps.rs`. A design
that reached for `Unsupported` as the general answer would have built a
second, unchecked capability vocabulary next to the checked one #492
just finished consolidating from two into one (`BackendCaps`'s own doc
comment, "This is the *only* capability vocabulary") — the same mistake
in a new location.

### Why not Result-ify `draw_*` / the CSD `bool` methods

**`draw_*` stays infallible, by the issue's own scoping** ("Draw methods
stay infallible (record-and-report via `end_frame`/log hook instead)")
— `last_error()` *is* that log hook: a `draw_*` implementation that hits
a native failure mid-paint (a Cairo `cr.fill()` error, a Direct2D
`todo!()`'s eventual real body hitting a lost device) records into the
same internal field `begin_frame`/`end_frame` use and returns normally,
so the frame finishes painting whatever it can rather than aborting
partway through — `last_error()` after `end_frame` tells the caller
something went wrong without deciding mid-frame which primitive gets to
abort the other nineteen. Wiring every individual `cr.fill().ok()` /
`cr.stroke().ok()` call site (`gtk/activity_bar.rs`, `chart.rs`,
`command_center.rs`, `panel.rs`, `status_bar.rs` — ~25 sites) through
`last_error()` is real per-call-site work, tracked as its own follow-up
issue (below), not part of this design pass.

**The four CSD `bool` methods (`begin_window_drag`,
`toggle_window_maximize`, `begin_window_resize`, `set_cursor`) keep
their `bool` return, unmigrated.** #507 asked for a migration plan
("tri-state or caps + bool"); the answer is **caps + bool, already
shipped, no further migration** — not a new decision, a recognition
that #492 already closed this. `BackendCaps::window_chrome` /
`::pointer_cursor` answer "does this backend have window chrome /
cursor hinting at all" ahead of the call; the `bool` return answers "did
*this specific* drag-start / resize-start / cursor-set happen" — and for
these four methods that's a legitimate, common `false`, not a failure:
`begin_window_resize` returns `false` whenever the click wasn't within
the resize-edge margin, `set_cursor` per its own doc "callers should
treat false as a no-op, not an error." There is no third state a real
backend needs here — a `begin_window_drag` that structurally cannot
start a drag (no window, no OS drag API) is `window_chrome: false`
territory, already covered; one where the OS drag call itself errors out
degrades identically to "the drag didn't start" from the caller's
perspective, and `vimcode/src/gtk/mod.rs:6834-6849` — the only consumer
that calls any of these four — already discards every one of these
`bool` returns outright (it decides whether to *call* `begin_window_drag`
from its own title-bar hit-test, not from the method's answer). A
tri-state would add a discriminant no caller reads. If a future backend
author hits a concrete case where "declined" and "the OS call failed"
must be told apart, that's grounds to reopen this specific sub-decision
— it is not blocked by anything else here.

### Follow-up implementation issues (design does not implement)

This is a design decision, not an implementation PR — `BackendError`,
`Backend::last_error`, and the three `PlatformServices` `_result`
methods described above do not exist in the tree yet. Filed as
follow-ups, one seam each, so no single PR mixes "add the type" with
"wire N call sites":

1. **Add `BackendError` + `Backend::last_error` (default `None`) to
   `backend.rs`.** No behavior change yet — every existing backend
   compiles unchanged.
2. **Win-GUI: wire `end_frame`'s device-lost recovery (`backend.rs:179`,
   `:189`) through `last_error()`.** The one backend with a concrete
   near-term producer; also the natural place to replace the
   `begin_frame`/`end_frame` `todo!()`s' eventual real bodies' failure
   path.
3. **`PlatformServices`: add the three `_result` twins + `ServiceResult`
   alias.** Default bodies wrap the existing methods (zero behavior
   change); Win-GUI's `services.rs` overrides them with real
   `HRESULT`-derived `Err` values as its dialogs are implemented.
4. **GTK: wire the ~25 swallowed `cr.fill().ok()`/`cr.stroke().ok()`
   call sites (`gtk/activity_bar.rs`, `chart.rs`, `command_center.rs`,
   `panel.rs`, `status_bar.rs`) through `last_error()`** instead of
   discarding the `cairo::Error`.

### Sign-off: vimcode's usage survives this shape

Checked against `~/src/vimcode/src` (`coord-tui` has none of these call
sites — grepped separately, zero hits on every symbol below):

- `show_file_open_dialog` / `show_file_save_dialog` / `show_message_dialog`
  (`gtk/mod.rs:1327,1340,1372`) — all three keep their current
  `Option`-returning signatures unchanged; vimcode's `run_pending_file_dialog`
  / `run_pending_native_dialog` need zero edits. Migrating to the
  `_result` twins to gain failure detail is optional and later.
- `begin_window_drag` / `toggle_window_maximize` / `begin_window_resize`
  / `set_cursor` (`gtk/mod.rs:6774,6834,6839,6849`) — signatures
  unchanged (this decision, above); vimcode already discards every
  return value, so there is nothing to migrate, now or later.
- `Clipboard::read_text` (`tui_main/mod.rs:484`) — untouched; explicitly
  out of scope (above).
- `Color::from_hex` — the ~680 `Color::from_hex(...)` hits in
  `render.rs` (e.g. line 9177) are vimcode's **own** local `Color` type
  (`render.rs:33`), not `quadraui::Color`; `grep -rn
  'quadraui::Color::from_hex'` against both consumers is zero hits.
  Nothing to check here even in principle, and it's out of #507's scope
  either way — it's a pure value parser, not a frame/event/services
  seam (`Option` there is a separate, already-tracked panic-audit
  concern, not this decision's).
- `wait_events`/`poll_events`/`begin_frame`/`end_frame` — vimcode's GTK
  runtime doesn't call these directly (Relm4 owns its own event loop
  per `CLAUDE.md`'s GTK-migration note); the one hit (`gtk/mod.rs`, a
  doc comment) is prose, not a call site. Nothing to break.

No consumer hit requires any call-site change. `last_error()` and the
`_result` twins are additive and opt-in; a consumer that never calls
them observes no difference at all.

## D-010 — `UiEvent` emission conformance matrix; disposition of the five never-emitted variants; `CharTyped` vs. `KeyPressed{Char}` (issue #501)

### Question

`SMELL_AUDIT_2026-07.md`'s PORT-04 verified-emitter scan found five
`UiEvent` variants no backend constructed anywhere in-repo —
`WindowClose`, `MouseEntered`, `MouseLeft`, `FilesDropped`,
`DpiChanged` — and flagged a sixth problem: GTK's text input arrives
only as `KeyPressed{Key::Char}`, never `CharTyped`, while TUI/macOS were
believed to emit both. Issue #501 asked, per variant: wire it, mark it
an optional capability, or remove it (pre-1.0 allows breaking); and for
`CharTyped`/`KeyPressed{Char}`, pick one canonical text-input event and
make backends conform.

Two things had changed by the time this issue ran, both discovered by
re-verifying rather than trusting the July scan:

1. **`WindowClose` and `DpiChanged` are no longer "never emitted."**
   `win/run.rs`'s `WM_CLOSE`/`WM_DPICHANGED` handlers construct both
   (`win/run.rs:466,611`) — real code, not a stub, predating this issue.
   Only GTK and macOS still had the gap for `WindowClose`; only GTK and
   macOS still had it for `DpiChanged`. TUI has no OS window at all, so
   it is correctly exempt from both, not "missing" them.
2. **The audit's `CharTyped` row (`tui ✅ / gtk ❌ / macos ✅`) doesn't
   hold today.** Re-checked directly: `tui::events::crossterm_to_uievents`
   translates every printable keystroke to `KeyPressed{Key::Char}` and
   has no `CharTyped` arm at all; `macos::events::ns_key_to_uievent`
   does the same (`Key::Char(base)`/`Key::Char(first)`,
   `macos/events.rs:162,170`). **No backend constructs `UiEvent::CharTyped`
   from native input anywhere in this repo.** The only places that
   construct it are `compose/{sidebar_system,tree_controller,chat_controller}.rs`'s
   own `handle` methods (as something they'd react to *if* fed one —
   directly, in tests, or by a future backend) and `folder_picker.rs`'s
   test suite (which constructs one specifically to prove its handler
   *ignores* it — see that file's `# Why not CharTyped` doc, added
   before this issue and already reaching the conclusion this decision
   formalises).

### Method

Per variant: `grep -rn 'UiEvent::<Variant>' quadraui/src` (excluding
the definition site) to find every real construction, distinguishing
translation-layer construction (a native event → this variant, what
"emits" means) from unrelated matches (`match` arms in app-level
compose helpers, doc-comment mentions, `#[cfg(test)]` fixtures that
build one directly to test a *consumer*). Then, per D-008's established
gate, `grep -rn '<Variant>' ~/src/vimcode/src ~/src/coord-tui/src` —
this repo's grep alone answers "does any backend emit it," not "does
any consumer need it," and the July audit's methodology mistake on
five other symbols (§D-008) was exactly trusting the narrower grep.

### Downstream impact — grep evidence

```
$ grep -rn '\bWindowClose\b'   ~/src/vimcode/src ~/src/coord-tui/src
vimcode/src/gtk/mod.rs:7144:            UiEvent::WindowClose => {
$ grep -rn '\bMouseEntered\b'  ~/src/vimcode/src ~/src/coord-tui/src
(no hits)
$ grep -rn '\bMouseLeft\b'     ~/src/vimcode/src ~/src/coord-tui/src
(no hits)
$ grep -rn '\bFilesDropped\b'  ~/src/vimcode/src ~/src/coord-tui/src
(no hits)
$ grep -rn '\bDpiChanged\b'    ~/src/vimcode/src ~/src/coord-tui/src
(no hits)
$ grep -rn '\bCharTyped\b'     ~/src/vimcode/src ~/src/coord-tui/src
vimcode/src/gtk/mod.rs:6959:            UiEvent::CharTyped(c) => {
vimcode/src/gtk/mod.rs:6960:                // Ctrl-modified characters arrive via KeyPressed; CharTyped is
```

Two of the six have a real external consumer, and both are exactly the
"this repo's own grep would have said DELETE and been wrong" shape
D-008 warned about:

- **`WindowClose`**: `~/src/vimcode/src/gtk/mod.rs:7144`'s
  `AppLogic::handle` arm calls `self.show_quit_confirm()` — an
  unsaved-changes veto prompt. Since GTK never emitted this event
  before this issue, clicking vimcode's GTK window's "×" went straight
  to GTK's own default close handling with **no** `UiEvent` in between:
  `show_quit_confirm` was unreachable dead code on a real click, and a
  user with unsaved changes lost them silently. `win/run.rs:611-620`'s
  `WM_CLOSE` handler comment ("matching the GTK runner's
  `Reaction::Exit => window.close()`") already assumed GTK did this —
  it didn't.
- **`CharTyped`**: same file, `:6959`, inside the identical `handle`
  arm — reserved by vimcode's own comment for "IME-composed printable
  characters only," explicitly *not* the general typing path (which the
  adjacent `KeyPressed` arm already covers, `:6910-6949`). Confirms
  §"CharTyped vs. KeyPressed{Char}" below independently: vimcode's own
  reference implementation already treats `KeyPressed{Char}` as the
  live text-input path and reserves `CharTyped` for IME.

`MouseEntered`/`MouseLeft`/`FilesDropped`/`DpiChanged`: zero hits in
either consumer, and (per the re-verification above) zero real
constructors in this repo either — genuinely unshipped, not merely
in-repo-quiet the way D-008's eight kept `*Event` enums were.

### Disposition

**`WindowClose` — WIRE (GTK; issue #501's own scope). Required
capability on every windowed backend; N/A on TUI.** Fixed in this PR:
`gtk::run::activate` now calls `window.connect_close_request`, funnels
the OS close through the same `dispatch_event` every other GTK signal
uses, and maps the app's outcome the way `win/run.rs` already documented
wanting — `Reaction::Exit` → `glib::Propagation::Proceed` (let the OS
proceed), anything else → `glib::Propagation::Stop` (veto, keep the
window open). `win/run.rs`'s `WM_CLOSE` arm already did this; GTK was
the one gap. **macOS is not wired by this PR** — `macos::run` has no
`windowShouldClose:`/`applicationShouldTerminate:` delegate method
today, and wiring it belongs with #486's window-lifecycle scope, not
this issue's event-taxonomy scope. Filed as a follow-up (coordinator:
please open) rather than silently left undocumented — see *Follow-ups*
below. TUI is correctly exempt: a terminal has no OS window distinct
from the process, so there is no "close" to observe independent of the
process exiting (Ctrl-C, `q`, or whatever accelerator the app itself
defines already covers that via `Reaction::Exit`).

**`MouseEntered` / `MouseLeft` — KEEP, OPTIONAL capability, not wired
by this PR.** Zero consumers anywhere (in-repo or downstream) and no
backend emits either today. Not the D-008 DELETE shape: those 14 dead
`*Event` enums each duplicated a `*Hit`/`*Layout` pair that already
shipped the same information through a different, working path: no such
substitute exists for hover enter/leave. Hover state is real,
plausible near-future work (tooltip auto-show-on-hover, hover
highlighting on activity-bar/tab items) that both GTK
(`EventControllerMotion`'s `enter`/`leave` signals) and a future macOS
backend (`NSTrackingArea`) can wire cheaply once a primitive actually
needs it — deleting the variant now would just mean re-adding it later
under time pressure for the same "no present benefit" reason D-008 kept
`ModalEntry`. Declared, not wired: `BackendCaps` has no field for this
yet (see *Follow-ups*).

**`FilesDropped` — KEEP, OPTIONAL capability, not wired by this PR.**
Same reasoning as `MouseEntered`/`MouseLeft`: zero consumers, no
emitter, no substitute mechanism, real plausible future need
(drag-a-file-onto-the-explorer-sidebar import). GTK's `Gtk::DropTarget`
and a future Win `WM_DROPFILES`/`IDropTarget` are both real, just
unbuilt. Declared, not wired.

**`DpiChanged` — OPTIONAL capability; already wired on Win, not
extended to GTK by this PR.** Win emits it from `WM_DPICHANGED`
(`win/run.rs:466`) — real, working code, not a stub. GTK reads
`scale_factor()` exactly once, at smoke-check time
(`gtk/run.rs::schedule_smoke_check`), but never on a live runtime DPI
change (monitor move, external monitor plug/unplug while the window is
open) — that gap is PORT-12's, a distinct, larger DPI/scaling design
story this issue doesn't reopen. TUI is always `scale == 1.0` and
correctly never emits it. Kept required-on-Win/optional-elsewhere
rather than "required everywhere" because GTK's runtime case is
unbuilt and TUI's is inapplicable by construction, not by oversight.

**`CharTyped` vs. `KeyPressed{Key::Char}` — not a duality to collapse
into one; they were never duplicates, the doc comment just described
them as if they were.** `event.rs`'s prior doc on `CharTyped` ("a
character was typed, ready for insertion … this is a direct
keystroke-to-char translation, not the result of IME composition")
directly contradicted vimcode's own `AppLogic::handle` arm for it
("IME-composed printable characters only") — and contradicted this
crate's *own* compose helpers: `folder_picker.rs` deliberately listens
to `KeyPressed{Char}` **only**, with a doc comment already explaining
why (avoiding a double-insert "if a future backend emits `CharTyped`
too"), while `sidebar_system.rs`/`tree_controller.rs` match `CharTyped`
for their edit-typing path *in addition to* `Key::Char` inside
`KeyPressed` (`tree_controller.rs:272,506`) — both unconditionally
calling `edit_insert_char`. That's the exact double-insert hazard
`folder_picker.rs` was written to dodge, currently latent only because
no backend emits `CharTyped` yet.

Resolution: **`KeyPressed{Key::Char}` is the canonical, always-on
text-input event — required on every backend, already emitted by all
of them.** `CharTyped` is re-scoped, in its doc comment (this PR), to
mean exclusively "an IME committed composed text for one character" —
a distinct signal with no `KeyPressed` equivalent, not a second way to
report an ordinary keystroke. Consequence for a future IME
implementation (#502): while a composition is in progress, the raw
keydowns must **not** also reach the app as `KeyPressed` — that's how
every native IME already behaves (the IME consumes them), so the two
events are naturally mutually exclusive per keystroke once IME lands
correctly, and `sidebar_system`/`tree_controller`'s existing
"listen to both" handlers are correct *as long as that invariant
holds*. Recorded here explicitly so #502's implementation is measured
against it rather than rediscovering the hazard.

### What this does NOT mean

- It does not mean `MouseEntered`/`MouseLeft`/`FilesDropped`/GTK's
  runtime `DpiChanged` are cancelled. They're recorded as intentionally
  unwired-but-kept, same status as D-008's eight kept `*Event` enums —
  a future issue that wires one needs its own justification (a
  primitive that needs hover, a sidebar that needs drop-import), not a
  re-read of this entry.
- It does not mean macOS's `WindowClose` gap is resolved. It is
  explicitly **not** wired by this PR; #486 (or a new follow-up, if
  #486's scope doesn't cover it) owns that.
- It does not mean `BackendCaps` gained new fields for the optional
  variants above. `docs/BACKEND.md`'s emission matrix records the
  required/optional status in prose; wiring it into `BackendCaps` (so
  `tests/conformance/caps.rs`'s honesty check and scenario `requires:`
  can reference it mechanically) is real follow-up work, not done here.
- It does not mean `CharTyped` is now "IME-only" as an implementation
  detail nobody else should touch. It's a public, documented contract
  change for backend authors: **do not emit `CharTyped` for a plain,
  non-IME keystroke**, full stop, because two real in-tree consumers
  will double-insert if you do.

### Follow-ups (issues to file — not filed by this PR; workers don't
have GitHub write access, coordinator: please open against #481)

1. Wire `WindowClose` for macOS (`macos::run`'s window-close delegate
   methods) — likely folds into #486, confirm scope first.
2. Wire `DpiChanged` for GTK's live runtime case (`notify::scale-factor`
   on the surface, debounced like resize) — PORT-12's scope.
3. Add `BackendCaps` fields for the four optional-capability variants
   (`MouseEntered`/`MouseLeft` as one flag — they're always emitted or
   not as a pair — plus `FilesDropped`, plus a GTK-runtime `dpi_live`
   distinct from Win's already-working case) once a real consumer wants
   one, so `tests/conformance/caps.rs`'s honesty check and scenario
   `requires:` can reference them mechanically instead of only in prose.
4. Build the Tier C2 "native-injection recipe" harness beyond the
   mouse/key/scroll/resize slice this issue adds
   (`quadraui/tests/conformance/c2.rs`) — `DoubleClick`, `Accelerator`,
   `ClipboardPaste`, `TextCopied` still need their own per-backend
   proof-of-emission rows.
5. Give every `gtk_*` example an explicit `WindowClose` opinion.
   Review caught a real interaction this PR's own tests didn't: the
   veto default means an app with *no* `WindowClose` arm — the
   unhandled catch-all, true of every `examples/common/*.rs` file
   except `terminal_app.rs` (fixed in the same PR that added this
   follow-up) — silently ignores the native "×" button, since its
   catch-all's `Reaction::Continue` is exactly what the veto logic
   treats as "don't close." Two internal call sites that used to hit
   the same trap (`schedule_smoke_check`'s forced timeout-close and
   `GtkSink::request_exit`, the target of every *other*
   `Reaction::Exit`) are already fixed, by using `window.destroy()`
   instead of `window.close()` so neither re-enters the
   `close-request` veto handler — so `gtk_smoke.sh` no longer hangs
   and a keyboard-driven quit (e.g. "press q") still actually quits.
   What's still open is the remaining ~49 examples' native "×" button:
   until each gets its own `WindowClose => Reaction::Exit` arm (or the
   veto default is reconsidered — rejected here to keep GTK consistent
   with `win/run.rs`'s pre-existing, already-shipped `WM_CLOSE`
   contract), clicking "×" on any of them silently does nothing. Until
   this follow-up lands, `docs/BACKEND.md`'s emission-matrix "required:
   GTK ✅" cell for `WindowClose` is proven only at the
   `dispatch_event`-funnel level (`gtk::run::window_close_tests`) and
   for the one updated example — not end-to-end for the example set as
   a whole.

## D-011 — Clipboard paste/copy keypress contract: shift tolerance, letter case, forced redraw, native modifier (issue #728)

### Question

`runtime.rs`'s module doc (quadraui#496) audited `tui::run::dispatch_event`
against `gtk::run::dispatch_event` line-by-line and found three real,
load-bearing divergences in their Ctrl-C-copy handling, then deliberately
left all three unresolved rather than silently picking one side:

1. **Shift tolerance.** TUI's Ctrl-C guard accepts Shift as a stray
   modifier (`modifiers.ctrl && !modifiers.alt && !modifiers.cmd`, no
   `shift` check). GTK's requires `Modifiers { ctrl: true, shift: false,
   alt: false, cmd: false }` exactly.
2. **Letter case.** TUI matches both `Key::Char('c')` and `Key::Char('C')`
   (tolerating CapsLock). GTK matches only lowercase `'c'`.
3. **Forced redraw.** TUI's handler forces `EventOutcome::Redraw` after
   `app.handle(TextCopied, …)` regardless of the app's own `Reaction`
   (`Reaction::Exit => Exit, _ => Redraw`). GTK folds the app's `Reaction`
   through unchanged (`Continue => Continue, Redraw => Redraw, Exit =>
   Exit`).

Issue #728 (Win-GUI clipboard paste) needed its own answer to a sibling
question — GTK's private `is_paste_keypress` (Ctrl-V, quadraui#415) and
macOS's inline Cmd-V match guard already disagreed on shift tolerance the
same way — before it could lift a single shared predicate into
`desktop.rs` for all three backends to call. Resolving *that* smaller
fork is what forced this decision to finally happen, rather than leaving
it as an open question a fourth backend could hit again.

### Decision

**Shift-tolerant, case-insensitive, and reaction-transparent — TUI's
Ctrl-C behavior and GTK/macOS's reaction-folding both win, on different
axes.** For every backend, present and future:

1. **Shift is tolerated, not required-absent**, on both the copy chord
   (Ctrl-C / Cmd-C) and the paste chord (Ctrl-V / Cmd-V). This was
   already true for paste on GTK (`Ctrl-Shift-V` — quadraui#415: some
   terminal emulators reserve plain Ctrl-V for a literal control byte and
   use Shift as the paste-disambiguator instead) and already true for
   copy on TUI. It now applies uniformly: `Ctrl-Shift-C`/`Cmd-Shift-C`
   copy and `Ctrl-Shift-V`/`Cmd-Shift-V` paste all fire the same as their
   plain forms everywhere. Rationale: a terminal grid (TUI's own
   rationale for tolerating Shift on copy) and a bespoke `DrawingArea`/
   `NSView` (GTK/macOS's rationale for tolerating it on paste) have the
   same "no formatting-aware clipboard op" property either way — there is
   no principled reason the *letter* (`C` vs `V`) should get a different
   answer.
2. **Both letter cases match** (`'c'`/`'C'`, `'v'`/`'V'`), i.e. TUI's
   existing CapsLock-tolerant behavior wins over GTK's lowercase-only
   check. A user with CapsLock on should not silently lose Ctrl-C/Ctrl-V
   — that was already effectively true for the letter `v` (both GTK's
   `is_paste_keypress` and macOS's Cmd-V guard already matched
   `Key::Char('v') | Key::Char('V')`; only the *copy* side's `'c'`-only
   check was the outlier), so this is a small, one-sided fix, not a new
   axis of divergence.
3. **`TextCopied`'s outcome folds the app's `Reaction` through unchanged**
   — GTK's existing behavior wins over TUI's forced redraw. On inspection
   TUI's forced redraw looks less like a deliberate feature and more like
   an unexamined side effect: `backend.clear_text_selection()` runs
   unconditionally before `app.handle(TextCopied, …)`, and if the
   selection highlight is only erased on the *next* redraw, an app that
   legitimately returns `Reaction::Continue` (nothing else changed) would
   leave a stale highlight on screen over text that's already been
   copied-and-deselected internally — TUI's forced redraw was papering
   over that, not implementing a feature independent of it. The correct
   fix for *that* problem is for the redraw need to be an explicit,
   visible fact at the call site (`force_redraw = true` alongside the
   `clear_text_selection()` call, the same pattern `dispatch_event`
   already uses for `TextSelectionChanged`), not a blanket override of
   whatever the app decided. A `Reaction::Continue` app's intent should
   survive the round trip.

4. **Exactly one platform modifier is native per backend, and the shared
   predicate enforces *that specific one* — never "whichever of
   `ctrl`/`cmd` happens to be held."** [`crate::Modifiers::cmd`] is the
   crate-wide abstraction for "this platform's OS-level modifier key":
   GTK maps `SUPER_MASK`/`META_MASK` → `cmd` (`gtk::events`), Win32 maps
   the Windows key → `cmd` (`win::events::win_modifiers`), and macOS maps
   ⌘ → `cmd`. A first mechanical lift of GTK's `is_paste_keypress` used
   `(modifiers.ctrl ^ modifiers.cmd) && !modifiers.alt` — accept
   whichever single one of `ctrl`/`cmd` is held, reject both/neither —
   reasoning that XOR alone was enough because "exactly one platform
   modifier" sounded backend-neutral. It wasn't: `cmd` means *Super* on
   GTK and *the Windows key* on `win`, not "the OS-native modifier
   restricted to the modifier that's actually native to the running
   platform." That XOR silently made plain **Super+V trigger paste on
   GTK** (GTK's own pre-lift predicate required `ctrl` unconditionally
   and explicitly rejected `cmd` being held at all: `ctrl && !alt &&
   !cmd`) and made plain **Ctrl+V trigger paste on macOS** (whose pre-lift
   inline guard required `ctrl: false` exactly) — a real behavior change
   on both existing backends introduced by the "generalisation" itself,
   caught at review before merge rather than recorded here up front as
   this section's own rule requires.

   The fix: [`crate::desktop::is_paste_keypress`] takes a third
   parameter, `PasteModifier` (`Ctrl` or `Cmd`), that each backend's
   `dispatch_event` passes explicitly — GTK and `win` both pass
   `PasteModifier::Ctrl`, macOS passes `PasteModifier::Cmd`. The
   predicate then requires that specific modifier to be held **and** the
   *other* one to be explicitly absent (not merely unchecked), restoring
   both backends' original, narrower pre-lift behavior exactly while
   still sharing one function body. This is the "parameter" option, not
   the `target_os`/feature-branch option — a parameter keeps the
   predicate itself platform-agnostic (still zero toolkit dependency,
   still unit-testable for both arms on any host) and pushes the
   platform fact to the one place that already knows it: the call site.

This is a decision about the *contract*, not a code change to
`tui::run`/`gtk::run` in this PR: #728's own scope is landing the paste
half for `win` (see its issue body — "the `TextCopied` half needs a text
selection to copy" and depends on `win`/`macos` gaining
`BackendCaps::text_selection` first, tracked by this epic's
text-selection issue). Applying points 1–3 above to `tui::run`'s and
`gtk::run`'s existing Ctrl-C code — and then lifting the now-identical
bodies into a shared, capability-gated helper the way `runtime.rs`'s doc
already sketches (a `SelectionBackend` trait implemented only where
`BackendCaps::text_selection` is `true`) — is that follow-up issue's job,
not this one's. What #728 *does* land now, consistent with point 1 above:
[`crate::desktop::is_paste_keypress`], the shared Ctrl-V/Cmd-V predicate
every backend's `dispatch_event` calls, is shift-tolerant on every
adopter (including macOS's Cmd-V, which used to require `shift: false`
before adopting the shared predicate in this PR) — and, per point 4,
requires each backend to name its own native modifier rather than
accepting either.

### What this does NOT mean

- It does not mean TUI's and GTK's Ctrl-C code already agree after this
  PR — they don't yet; `runtime.rs`'s module doc still accurately
  describes the current divergence until the follow-up issue applies
  this decision to both bodies.
- It does not mean every future clipboard shortcut inherits Shift
  tolerance automatically. This decision is scoped to the copy/paste
  chords specifically (`Ctrl`/`Cmd` + `C`/`V`) — a different shortcut
  (e.g. Ctrl-A select-all, already Shift-*exclusive* on both TUI and GTK
  today: `!modifiers.shift`) keeps its own, separately-reasoned contract.
- It does not retroactively bless "any modifier goes" — Alt and holding
  both `ctrl`+`cmd` at once are still rejected everywhere; only `shift`
  moved from "must be absent" to "don't care."
- It does not mean `ctrl` and `cmd` are interchangeable paste triggers on
  any given backend, even though both exist as fields on every
  `Modifiers` value regardless of platform. Per point 4, exactly one is
  native to each backend and the *other* must be absent, not merely
  unchecked — plain Super+V on GTK and plain Ctrl+V on macOS are both
  still rejected, exactly as they were before `is_paste_keypress` was
  shared.

Future backends (or a fourth desktop toolkit) implementing copy/paste
should read this decision rather than pattern-matching whichever
existing backend they skim first.

### §5 — a `win` paste test may not assert on the *host's* clipboard

Landed as a CI fix on this same issue. The first cut of
`win::run::paste_dispatch_tests` asserted that dispatching Ctrl-V left
the app having received **nothing at all** — "empty clipboard ⇒ nothing
to paste ⇒ the chord is simply swallowed." That is a compile-time truth
off Windows, where `WinClipboard::read_text` is an unconditional `None`
(`win::services`'s `Clipboard` impl), so it passed both locally and on
the `ubuntu-latest` `--features win` leg. On the `windows-latest` leg the
very same line is a real `CF_UNICODETEXT` read of the machine's live,
systemwide clipboard — state no test in this repo owns or seeds — so the
app legitimately received a `ClipboardPaste` and the assertion failed.
`tui + win (build, test, clippy) — windows-latest` went red on a genuine
defect in the test, not in `dispatch_event`.

The rule this settles, for every `win` test and every future backend
whose `PlatformServices` reach real OS resources: **assert on state the
test owns.** Concretely, the three shapes now in that module:

1. The invariant that holds on *every* host with *any* clipboard
   contents gets an ungated test — here, "once `is_paste_keypress`
   recognises the chord the raw `KeyPressed` is consumed; anything the
   app sees is a `ClipboardPaste`, never the key event." That is what
   #728 actually promises, and it is checkable without knowing what is
   on the clipboard.
2. A claim that is only true because of a `cfg`'d stub gets that same
   `cfg`. "Nothing to paste" is `#[cfg(not(target_os = "windows"))]`.
3. The real-OS branch gets a Windows-gated test that **seeds the
   clipboard first** and asserts on its own payload — turning the
   Windows leg from a hazard into the only coverage the
   "read real text → deliver `ClipboardPaste`" path can ever get. It
   self-guards on the write→read round-trip: a host whose clipboard the
   test process cannot open (a non-interactive window station, say) is a
   property of the environment, not of `dispatch_event`, so it reports
   and returns rather than failing.

The generalisation: a `--features win` test passing on `ubuntu-latest`
proves only that the `todo!()`/`None` fallback bodies behave. Before
adding one, ask which arm of every `cfg(target_os = "windows")` it
actually exercises on each leg — see `CLAUDE.md`'s "Win-GUI: building and
testing for real".

## D-012 — `FrameRung`/`FramePresence`/`check_frame_order`: presence gating + paint/hit-test order invariant (issue #774)

### Question

`frame.rs`'s `Surface`/`ScreenLayout` (D-006) answer "what does a frame
look like" for the primitives quadraui already ships. They don't answer
the question a non-trivial *app* also has: "which of my own rungs (an
editor pane, a bottom panel, an overlay stack) exist this frame, and in
what back-to-front order must they be visited — identically — whether
you're painting them or resolving a click?"

A vimcode platform-neutrality audit (`develop @ ee26268`, filed as
quadraui#774) found that question answered twice, independently, in
vimcode: `FRAME_Z_ORDER` (paint, `src/render.rs`) and
`MOUSE_ARBITRATION_ORDER` (hit-test) — ~1,000 lines plus 978 lines of
per-backend walkers. The two lists drifted exactly once
(vimcode#587/#592: `ScreenLayout.picker` painted on no backend for
months while still occupying a hit-test rung) and the fix at the time
was a `debug_assert!` comparing the two lists after the fact — a
detector, not a preventer.

### Decision

Add three small, backend-independent pieces to `frame.rs`, orthogonal to
`Surface`/`ScreenLayout` (they order an app's *own* rung enum, not
quadraui's primitive set, so they compose with either the `Surface` path
or a bespoke per-backend paint routine):

1. **`FrameRung`** — a trait an app implements once per rung enum,
   supplying `z_order() -> &'static [Self]`: the single declared
   back-to-front order. `rank()` (a provided method) turns a rung into
   its position in that order, panicking if a variant was left out of
   `z_order()` — a programmer error, not a runtime condition.
2. **`FramePresence<R>`** — evaluates a presence predicate against
   every rung in `R::z_order()` exactly once per frame
   (`FramePresence::from_fn`), so "is the palette even open" is
   computed a single time and consulted by both painting and
   hit-testing rather than re-derived (and potentially re-diverging) at
   each call site.
3. **`compose_frame`** + **`check_frame_order`** — `compose_frame` walks
   present rungs in `z_order()` with one non-branching callback (no
   `match` over paint-operation variants: the order comes from
   `z_order()`, not from the shape of the walk), so anything driven
   through it cannot be out of order by construction.
   `check_frame_order` is the guard for code that *isn't* driven through
   `compose_frame` yet (a hit-test walk, or a legacy per-backend paint
   routine mid-migration): it asserts an independently-produced sequence
   of visited rungs is consistent with `z_order()`, turning a
   `FRAME_Z_ORDER`-vs-`MOUSE_ARBITRATION_ORDER`-style drift into a
   failing test instead of a months-later bug report.

This lands only the pure, self-contained mechanism in quadraui — no
backend code, no new `Surface` variants, no vimcode call-site changes.
vimcode's own adoption (deleting `FrameOp`/`FramePresence`/
`compose_frame`/`FRAME_Z_ORDER`/`check_frame_order` from `render.rs` and
the 978 lines of per-backend walkers, in favor of these) is tracked
against vimcode milestone #7 "Platform-Neutral", left open behind this
issue and out of scope here.

### What this does NOT mean

- It does not mean every quadraui primitive now has a `FrameRung`
  variant, or that `Surface`/`ScreenLayout` gained rung-awareness —
  `FrameRung` is an app-defined enum an app implements for its *own*
  rungs; quadraui ships the trait and the three functions that operate
  on it, nothing more.
- It does not migrate vimcode. `FrameOp` (29 variants), `FramePresence`
  (the vimcode-local, pre-existing struct of the same name), `EditorOp`/
  `BottomOp`, and the two per-backend walkers all still exist in
  vimcode's `render.rs`/`app.rs`/`tui_main/shell_app.rs` until that
  cross-referenced follow-up lands.
- It does not resolve the `ScreenLayout` name collision the issue notes
  (vimcode declares its own `pub struct ScreenLayout` at
  `src/render.rs:10098`) — that's a vimcode-side rename, not something
  quadraui's API can fix from this side.
