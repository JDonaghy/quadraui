# Primitive Authoring Rules

Read this when adding or changing a primitive.

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
4. **Paint/click round-trip harness.** Every primitive that has clicks
   must have a paint/click round-trip test in its TUI rasteriser
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
   The paint/click harness covers coordinate-system drift; this rule
   covers paint-formula bugs (e.g. "thumb hardcoded at top of gutter
   instead of computed from scroll_offset"). Verify empirically by
   mutating the formula and observing the test fail. See *Coverage
   taxonomy* under *Testing* for the full bug-class breakdown.
7. **Add the primitive to the `Backend` trait.** Every primitive MUST
   have a `Backend::draw_<name>` (and where applicable, `Backend::<name>_layout`)
   method. Per `BACKEND_TRAIT_PROPOSAL.md` section 4, adding a primitive is
   an intentional breaking change to the trait — every backend
   implementer sees the new method as a compile error and fills in
   their rasteriser. **No primitive ships with TUI/GTK free-function
   rasterisers but no trait coverage.** That's how the
   *Cross-backend portability commitment* stays load-bearing —
   if a primitive isn't on the trait, downstream consumer code has to
   pick a backend explicitly. **If the new primitive is composed into
   top-level screens (most are), add its `Surface`/`FrameZone` variant
   in the same PR** — see "One primitive, one canonical paint path"
   below (issue #456).

   **Which `<name>_layout` shape to pick (issue #506):**

   | Primitive class | Convention | Examples |
   |---|---|---|
   | **Content-in-rect** — backend paints inline inside a caller-supplied `rect` and can compute the same geometry with no live paint context | **Paired**: `draw_<name>(rect, ..)` + `<name>_layout(&self, rect, ..)`, same signature shape, same resolver underneath. | `data_table_layout`, `tree_layout`, `form_layout`, `list_layout`, `board_layout`, `terminal_layout`, `editor_layout` |
   | **Overlay-with-caller-anchor** — host computes anchor/viewport/measure itself and calls the *primitive's own* `.layout(...)` directly; the backend only paints at the resolved bounds | **Draw-takes-layout**: `draw_<name>(&self, thing, layout)`. No `Backend::<name>_layout` — there's nothing backend-specific left to compute. | `Tooltip`, `ContextMenu`, `Completions`, `RichTextPopup`, `Dialog` |
   | **Interactive chrome** — freestanding widget at its own screen rect | **Paired**, same as content-in-rect. (Draw-returns-layout-only, with no no-paint twin, is the same convention minus the accessor a host needs to hit-test without a frame in progress — add the twin rather than leaving a primitive as the odd one out.) | `chart_layout`, `toolbar_layout`, `sidebar_panel_layout`, `board_layout`, `diff_view_layout` |

   A `<name>_layout` gets a **default trait body** only when the
   geometry is a *provably uniform* pure function of
   `Backend::char_width()` / `Backend::line_height()` (and primitive
   state) — i.e. every backend's own `draw_<name>` already resolves the
   same two values into the same formula (`terminal_layout`,
   `editor_layout`, `diff_view_layout`). "The same two values" can
   include a small backend-shaped accessor of the same shape as
   `char_width()`/`line_height()` when the *formula* is still uniform
   but a scalar it depends on genuinely isn't — `terminal_layout` also
   calls `Backend::terminal_scrollbar_default_width()` to reserve the
   scrollbar gutter's default width before dividing by `char_width()`,
   because every backend's real `draw_terminal` reserves that gutter
   with a backend-specific fallback (1 cell on TUI, 8px on GTK/macOS/Win)
   before iterating cells — skip that reservation and the default body's
   `grid_cols` silently over-reports by the gutter's width whenever a
   caller omits an explicit scrollbar width (issue #506 review fix; see
   `docs/DECISIONS.md` D-007's "`terminal_layout`'s scrollbar gap" note). If a backend supplies its own
   sizing constants that aren't derivable from those two accessors
   (`BoardMeasure`'s per-backend column/card pixel sizes, `ListView`'s
   scrollbar reservation), there is no default — every backend
   implements it explicitly, same as `draw_<name>` itself. A default
   body still needs a from-scratch, per-backend-verified justification
   in `tests/conformance/caps.rs`'s `ACCEPTED_DEFAULTS` (quadraui#492) —
   it is exempt from that check's "silently defaulted" failure only
   because the reason is written down, not because a default exists.
   See `quadraui/docs/DECISIONS.md` D-007 for the full audit, the
   off-trait-fn resolutions, and why `Palette` did **not** get a
   `palette_layout` method despite fitting the content-in-rect row
   above (a latent paint/layout drift, not a design gap — D-007's
   "Palette: deferred, not missed").
8. **Public API changes are versioned by deprecation, not by
   announcement.** See rule 8 in full below — it is the one rule whose
   failure mode lands in *other repos*.

## Rule 8 — public-API lifecycle

`quadraui` is `publish = false` at `version = "0.0.1"`. Nothing pins a
published version. `vimcode` path-deps a sibling checkout and its CI
clones `develop`, so a breaking change is live there the moment it
merges, turning every open PR red with no version bump to blame.
`coord-tui` — `JDonaghy/coord-tui`, a standalone repo since
`claude-coordinator#2899` (2026-08-29) — pins a git rev instead, which
delays the same break to whenever someone bumps that pin. quadraui's CI
does carry a `downstream` compile-truth job (#528), but its coord-tui
leg is currently skipped for lack of a read token on that repo, so
coord-tui breakage will *not* show up here. See the *Downstream
consumers* section of `CLAUDE.md` for the declarations, the token, and
the #476 post-mortem.

**What is breaking here vs. what isn't.** Consumers implement `ShellApp`
and `AppLogic` — never `Backend`. So:

| Change | Breaks consumers? |
|---|---|
| New `Backend` method, no default (rule 7's intentional compile error) | **No** — in-tree backends only. Keep doing this. |
| New required method on `ShellApp` / `AppLogic` | **Yes** — give it a default impl. |
| New field on a public struct a consumer constructs | **Yes** unless the struct is `#[non_exhaustive]` + built via `Default`/builder. |
| New variant on a public enum a consumer matches | **Yes** unless the enum is `#[non_exhaustive]`. |
| Rename / removal of any `pub` item with a consumer hit | **Yes.** |

**Measure before you cut.** Both consumers sit beside this checkout:

```bash
grep -rn '<symbol>' ~/src/coord-tui/src ~/src/vimcode/src
```

Zero hits in both plus no in-tree use ⇒ remove it outright; that is what
a dead-API pass is for. **Paste the grep output into the PR body** — the
claim is cheap to assert and cheap to verify, so verify it. Note that
`vimcode` is easy to forget: #476's commit message named coord-tui as
"the only known" consumer and broke vimcode too.

**Deprecation shims, in ascending cost.** All of these keep a consumer
compiling, with a warning that names the fix:

```rust
// Rename a type — the whole cost of a non-breaking rename.
pub struct CardBadge { /* … */ }
#[deprecated(since = "0.0.2", note = "renamed to `CardBadge`")]
pub type Stage = CardBadge;

// Rename an enum variant — keep the old spelling as an associated const.
// (`allow` is required: the quality gate runs clippy with `-D warnings`,
// and a PascalCase const trips `non_upper_case_globals`.)
impl BadgeStatus {
    #[allow(non_upper_case_globals)]
    #[deprecated(since = "0.0.2", note = "renamed to `BadgeStatus::Warning`")]
    pub const RequestChanges: BadgeStatus = BadgeStatus::Warning;
}

// Change a shape — forward from the old one.
#[deprecated(since = "0.0.2", note = "use `CardBadge { label, status }`")]
impl From<(Stage, BadgeStatus)> for CardBadge { /* … */ }

// Drop a field consumers still set — accept and ignore, don't delete.
#[deprecated(since = "0.0.2", note = "no backend ever painted this; drop the call")]
pub fn with_machine(self, _machine: impl Into<String>) -> Self { self }
```

**The two-PR protocol.** A breaking change is never one PR:

- **PR 1** — add the new shape, keep the old one compiling behind
  `#[deprecated]`, update in-tree examples and tests to the new shape.
  Open the migration issue in each affected consumer repo and link it
  here. Consumers are now on a warning, not an error, and can migrate on
  their own schedule.
- **PR 2** — delete the shims, once those migrations have merged.
  Reference them by number.

**One breaking change per PR.** #476 removed a type, renamed a variant,
deleted two struct fields and gutted a keymap in a single commit, so the
consumer's migration was all-or-nothing with no partially-compiling
state to bisect from. Split them.

**Every PR touching a `pub` item carries a `## Downstream impact`
section** naming each consumer file that must move, or stating "no
consumer hits" with the grep. A public-API PR without one should be sent
back at review.

## Coordinate frames for `*_layout` methods (issue #505)

Every `Backend::<name>_layout` method — and its `draw_<name>` twin, where
one returns hit-region data — returns its `hit_regions` / `bounds` in
one of exactly two frames, and its doc comment states which one:

- **LOCAL** — relative to the `rect` passed in; `(0, 0)` is `rect`'s
  top-left corner. The caller subtracts `rect.x` / `rect.y` from an
  absolute click coordinate before calling `hit_test`.
- **ABSOLUTE** — shifted by `rect.x` / `rect.y` (target-surface
  coordinates). The caller compares raw click coordinates against the
  returned bounds with no further adjustment.

**Which one a given primitive uses is a design choice made once, not a
per-backend one.** In practice: primitives a parent composer paints
*inline* and already tracks the origin for (`tree_layout`,
`form_layout`, `data_table_layout`, `text_display_layout`,
`status_bar_layout`, `activity_bar_layout`) are LOCAL. Primitives
painted as a freestanding widget at their own screen rect, where the
caller has no other reason to track that rect, are ABSOLUTE
(`tab_bar_layout`, `menu_bar_layout`, `split_layout`,
`split_tree_layout`, `panel_layout`, `toast_stack_layout`,
`pipeline_view_layout`, `progress_layout`, `spinner_layout`,
`command_center_layout`, `toolbar_layout`, `sidebar_panel_layout`,
`chart_layout`, `minimap_layout`, `msv_layout`, `text_input_layout`).
See `quadraui/docs/DECISIONS.md` D-005 for the full audit and why the
split isn't collapsed to one frame everywhere.

**The rule this enforces is not "always LOCAL" — `docs/LESSONS.md`'s
"Layout helpers must return coords in the same frame across backends"
predates this audit and reads that way; treat this section as its
successor.** The rule is: (1) the method's doc comment states its frame
explicitly — **LOCAL** or **ABSOLUTE**, in those words, so grep finds
every one — and (2) every backend implementation actually agrees with
that statement and with its TUI/GTK/macOS siblings. A `*_layout` doc
comment with neither word is an #505 regression.

**Every `*_layout` method needs a non-zero-origin regression test** on
every backend it ships on (TUI + GTK always; macOS per #483
availability) — `rect.x != 0` or `rect.y != 0`. `area = (0, 0)` is
exactly the case where a LOCAL/ABSOLUTE mixup is invisible (adding or
skipping `rect.x` is a no-op when `rect.x == 0`), which is why the
historical `mac_tree_layout` bug (`docs/LESSONS.md`) shipped past
tests that all used the origin.

## One primitive, one canonical paint path (issue #456)

`quadraui` has two ways to paint a primitive: `backend.draw_<name>(rect,
&data)` directly, or `layout.push(Surface::<Name> { rect, .. })` +
`layout.draw(backend)` via `quadraui::frame::ScreenLayout`. Nothing
stops two backends of the same consumer app from picking different
ones for the identical call site — and when they do, the two paint
paths silently drift, because the compiler can't see across the split.
That's exactly what happened in vimcode: the TUI palette painted via
`b.draw_palette(...)`, the GTK palette via `Surface::Palette`, with
every other line at both call sites identical (#456; contributed to
vimcode#587).

**`ScreenLayout` + `Surface` is the canonical path for a consumer
assembling a top-level screen from multiple primitives** — it forces
both backends through the same call site and its `zone_for` helper
keeps the hit-map in lock-step with what was painted by construction
(see `quadraui/src/frame.rs`'s module doc). `Backend::draw_<name>`
stays public, non-deprecated, low-level API — `ScreenLayout::draw`
calls it internally, and it's the *only* path for any primitive that
has no `Surface` variant yet. See `DECISIONS.md` D-006 for the full
audit and why `Backend::draw_*` isn't hidden or deprecated over that
gap.

**Going forward:** when rule 7 above has you adding a `Backend::draw_<name>`
method for a primitive that's composed into top-level screens (i.e. a
consumer will paint it as one layer of a multi-primitive frame, not
just as the sole content of its own pane), add its `Surface::<Name>` /
`FrameZone::<Name>` variant and `ScreenLayout::zone_for` arm in the
**same PR**. Skipping this is how the trait and `Surface` started drifting apart in
the first place (11 primitives have a trait
method but no `Surface` variant as of the #456 audit — Board,
DiffView, PipelineView, Toolbar, SidebarPanel, TextInput, Spinner,
Progress, CommandCenter, DropOverlay, MessageList). Backfilling that
existing gap is tracked separately (`SMELL_AUDIT_2026-07.md` §7 Epic D,
`D4`) — this rule stops it from growing, it doesn't retroactively close
it.

**When `Backend::draw_*` direct calls are still the right choice**, even
for a primitive that does have a `Surface` variant: a rasteriser's own
paint/click round-trip test (rule 4), a compose helper painting a
primitive it fully owns, or any call site that has no need for the
frame-level hit-map `ScreenLayout` produces. The rule is "assembling a
multi-primitive screen goes through `Surface`," not "never call
`Backend::draw_*` directly."

## Shared pixel-layout math (#499)

`gtk::tree::gtk_tree_layout` and `macos::tree::mac_tree_layout` used to
be byte-identical apart from the function name and one comment — same
magic constants (`line_height * 1.2` header pitch, `* 1.4` item pitch,
`* 0.9` indent, `* 0.65` chevron glyph estimate), same body. The
`MultiSectionView` body-measure match arms were the same story, and had
already **drifted**: GTK measured real `MessageList` content height,
macOS's copy returned `0.0` — a live macOS layout bug born of
copy-paste, not a deliberate backend difference.

**Rule: pixel-backend layout math that doesn't touch a native drawing
handle belongs in `quadraui/src/primitives/layout_metrics.rs`, not in
`gtk::<name>` / `macos::<name>` / `win::<name>`.** A function belongs
there if it can be written as `(primitive, rect, line_height, ..) ->
*Layout` / `*Measure` with no `cairo::Context`, `CGContextRef`,
`pango::Layout`, `CTFont`, or `ID2D1RenderTarget` in its signature.
Each backend's `<name>.rs` then keeps only:

- a thin wrapper (`gtk_tree_layout`, `mac_tree_layout`, …) that calls
  the shared fn — kept per-backend only so each rasteriser's public
  layout-helper name matches rule 5 above, not because the body
  differs;
- the native paint code, which consumes the same `*Layout` /
  `*Measure` the shared fn produced.

**When a shared fn needs a real glyph width** (Form's per-item hit
regions can't use a fixed-ratio estimate the way tree/MSV do), it takes
`&dyn primitives::layout_metrics::TextMeasure` instead of a native font
handle. Each backend supplies a private one-method adapter over its own
live font/context — see `macos::form::CtFontMeasure` wrapping `CTFont` +
`measure_text`. The shared fn never learns a native type exists.

**Migrating a primitive's layout math is not the same PR as fixing a
drift it exposes**, unless the drift *is* the primitive's whole reason
for migrating (as MSV's `MessageList` fix was for #499) — see rule 8's
"one breaking change per PR" discipline; the same "don't batch" logic
applies to behavior changes uncovered mid-refactor. If sharing a
function would change one backend's *current* output (see
`layout_metrics::form_field_measure`'s doc on `FieldKind::TextArea` for
a case where sharing was deliberately **not** done for this reason),
leave that backend's existing behavior in place and note the gap in the
shared fn's doc comment rather than silently changing it.

**A file-overlap conflict on a fenced backend file (e.g. concurrent
work on `gtk/backend.rs`) blocks that backend's migration, not the
others.** Migrate every backend whose files are free; leave the
blocked one's duplication in place with a comment pointing at the
tracking issue, and land what's clean rather than waiting on the whole
set.

## Native-vs-painted menu convention

`ContextMenu` has two display paths and apps pick by trigger, not by
backend. The split is deliberate — it's what makes "feels native on
Mac while staying portable" the default outcome.

| Trigger | Path | Why |
|---|---|---|
| `MouseDown { button: Right }` on a canvas / row / cell | `backend.show_context_menu(&menu, position)` | User-triggered system menu — gets the platform's fonts, accent colour, Dark Mode, VoiceOver, ⌘-shortcuts. macOS uses `NSMenu.popUpMenuPositioningItem` natively; TUI/GTK fall through to the painted overlay today (stash-and-paint default lands when a consumer asks). Activation arrives as `UiEvent::ContextMenuItemActivated(WidgetId)`; dismissal as `UiEvent::ContextMenuDismissed`. |
| Left-click on a UI affordance that opens a menu-style dropdown (palette sub-menu, in-window action list) | `Backend::draw_context_menu` in the next render pass | App-controlled rendering, pixel-consistent across backends, positioned relative to a widget rather than the cursor. Use the `MenuSystem` compose helper. |

## Primitive maturity levels

A primitive file in `primitives/` is not "shipped" until it has all
three legs: (1) descriptor + layout + hit_test + layout tests, (2) TUI
and GTK rasterisers with paint/click harnesses, (3) `Backend` trait
methods. Primitives with only leg (1) are **descriptors** — their shape
is real and tested, but consumers cannot adopt them yet because there's
nothing to paint. As of 2026-05-03, all previously descriptor-only
primitives have shipped with full rasteriser + Backend trait coverage.
Don't delete descriptors — the layout + hit_test work is real and
reusable. Do prioritise adding rasterisers for any descriptor that
blocks a consumer's bespoke-paint elimination.
