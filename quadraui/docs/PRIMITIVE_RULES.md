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
   pick a backend explicitly.
8. **Public API changes are versioned by deprecation, not by
   announcement.** See rule 8 in full below — it is the one rule whose
   failure mode lands in *other repos*.

## Rule 8 — public-API lifecycle

`quadraui` is `publish = false` at `version = "0.0.1"`. Nothing pins a
published version. `coord-tui` (`claude-coordinator/tui`) and `vimcode`
both path-dep a sibling checkout, and both CI jobs clone `develop` — so
a breaking change is live in both repos the moment it merges, turning
every open PR there red with no version bump to blame. quadraui's own CI
has no downstream build, so it stays green. See the *Downstream
consumers* section of `CLAUDE.md` for the declarations and the #476
post-mortem.

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
grep -rn '<symbol>' ~/src/claude-coordinator/tui/src ~/src/vimcode/src
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
