# Gate A Contract — ms-11: Backend conformance suite

_Hand-authored by the operator (coordinator session) 2026-08-12, for milestone #11 (epic **#480**)._
_Not agent-authored: no `mock-author` session was dispatched and no `type="work"` assignment touched this path. Recorded explicitly because ms-38's provenance was ambiguous after the fact and cost real time to reconstruct (claude-coordinator#1314)._
_Road built by **#556** (PR #557): `quadraui/tests/acceptance.rs` is the sealed entrypoint; `acceptance.drivers.quadraui` landed coordinator-side as `coord-settings@7675089`._

**Issues this contract binds:** **#554** (wide-glyph tab labels) · **#542** (structural parity tier) · **#492** (Tier C0 + BackendCaps).
**Issues declared exempt:** #490, #491 (harness construction) · #493 (no macOS runner) · #555 (out of `TestBackend` reach). Reasons in §7; the machine-readable form is `manifest.yml`.

---

## 1. Scope — why most of this milestone is exempt

Epic #480 is a *test-harness* milestone: most of its children build the conformance suite rather than consume it. The oracle loop exists to hold **behaviour** to a contract, so an issue whose entire deliverable *is* test infrastructure has nothing for a sealed slice to assert that would not merely restate the implementation.

Epic #480 already anticipated this — "Groups A–C of that milestone are harness-construction work and are expected to be manifest-`exempt` … The oracle bites from Group D onward, and on #554." This contract adopts that split, with two corrections established while authoring it (§7.2, §7.3): **#493 and #555 are also exempt**, for runner-availability and observability reasons respectively, not because they lack user-visible behaviour.

| Issue | Group | Oracle | Why |
|---|---|---|---|
| **#554** | A | **binds** | A real user-visible defect with a clean black-box surface. The flagship case — see §2, §3. |
| #490 | B | exempt | Builds `FrameInventory`; it *is* the observation surface every other clause asserts through. |
| #491 | C | exempt | Builds the scenario schema + runner. Harness construction. |
| **#492** | D | **binds** | `BackendCaps` is a declared, queryable surface — assertable without restating the generator (§5). |
| #493 | D | exempt | **No macOS machine in the fleet.** §7.2. |
| **#542** | D | **binds** | "A backend that drops chrome fails" is a behavioural claim with an observable verdict (§4). |
| #555 | D | exempt | A pty/vt100 observer is by construction outside what `TestBackend` can see. §7.3. |

---

## 2. ⚠ Assertion polarity — read before authoring any slice

**`screen_contains` is the wrong tool for this milestone, and for #554 it is actively inverted.** A slice written the obvious way is *green on the broken tree and red on the fixed one*. This section is normative.

`TuiDriver` exposes two readbacks that disagree on purpose:

| API | How it reads the grid | Models |
|---|---|---|
| `screen()` / `screen_contains(needle)` | concatenates **every** cell's `symbol()`, one cell at a time | the ratatui `Buffer` as stored |
| `find_bounds(needle)` / `find(needle)` | walks `row_cells`, advancing `x` by **`char_cell_width(ch)`** | **what the terminal actually displays** |

`row_cells` strides by the character's own display width rather than by the following cell's contents, because ratatui blanks the continuation cell after a double-width glyph and that blank is indistinguishable from a real space (this is quadraui#488's fix; its regression guard is `TuiDriver::find_bounds_is_wide_char_aware_for_adjacent_cjk_glyphs` in `src/tui/testing.rs`).

Now apply both to #554's defect, where the painter writes each char into **one** cell (`set_cell_styled`, `x += 1`) regardless of width:

| | Cells written | `screen_contains("日本語.rs")` | `find_bounds("日本語.rs")` |
|---|---|---|---|
| **Today (broken)** | `日` `本` `語` `.` `r` `s` — 6 cells | **`true`** ✗ | **`None`** ✓ |
| **After the fix** | `日` ␣ `本` ␣ `語` ␣ `.` `r` `s` — 9 cells | **`false`** ✗ | **`Some(width 9)`** ✓ |

Read the middle column twice. `screen_contains` **passes while the bug is present and fails once it is fixed** — because `screen()` sees the six glyphs the painter stored, while the terminal (and `row_cells`) strides two columns past `日` and never reads the cell holding `本`. Those skipped cells are exactly the glyphs the issue reports as silently dropped.

**Therefore, normative for every clause in §3–§5:**

- Assert through **`find_bounds` / `find` / `click_text`**, never `screen_contains`, for anything whose correctness depends on column geometry.
- `screen_contains` remains fine for pure presence checks of **ASCII** text (the seam tests in `quadraui/tests/acceptance.rs` use it correctly for exactly that).
- A slice that would pass against the current `develop` tree is a **defective slice**, not a passing implementation. Gate C should re-check this.

> This is the ms-38 §5c lesson one turn of the screw harder. There, eight required strings were all satisfiable by the status bar alone, so an implementation that never rendered the feature passed. Here the naive assertion does not merely *fail to detect* the bug — it **inverts**, rewarding the defect.

---

## 3. #554 — tab labels measured and painted in columns

### 3a. The defect, restated as a black-box surface

A `TabItem` whose `label` contains double-width characters must paint **every** glyph, and the tab must occupy the number of columns it actually renders. Today `src/tui/backend.rs:1028`/`:1094` measure with `.chars().count()` and `src/tui/tab_bar.rs:167` paints with a flat `x += 1` stride, so a CJK label loses glyphs and the tab is measured narrower than it draws.

**Both sides must move together.** Measure-only widens the tab without fixing the dropped glyph; paint-only overruns the measured budget. The issue is explicit that a partial fix is *worse than none* because it moves consumers' hit boxes off the paint. §3c and §3e are chosen so that **each partial fix fails at least one clause** — see §3f.

### 3b. Fixture

Labels under test (exact strings, including the surrounding spaces that are part of `TabItem::label`):

| Tab | `label` | chars | display columns |
|---|---|---|---|
| 0 (wide, active) | `" 1: 日本語.rs "` | 11 | **14** |
| 1 (ASCII control) | `" 2: main.rs "` | 12 | **12** |

Sub-spans the clauses assert on:

| Needle | display columns | composition |
|---|---|---|
| `"日本語.rs"` | **9** | 3 wide (6) + 3 narrow (3) |
| `"main.rs"` | **7** | 7 narrow |

Viewport: **80 × 6**, `TuiDriver::new(app, 80, 6)`.

### 3c. Wide label paints every glyph — **the load-bearing clause**

**Required:** `driver.find_bounds("日本語.rs")` is `Some(_)`.
**Required:** that bounds' `width == 9.0`.

Red today: `row_cells` reconstructs the row as `日 語 r s` — `本` and `.` are stepped over — so the six-char needle matches no window and `find_bounds` returns `None`.

### 3d. ASCII labels are unchanged

**Required:** `driver.find_bounds("main.rs")` is `Some(_)` with `width == 7.0`.

The issue requires ASCII output be byte-for-byte identical. This clause is the regression half; it is green today and must stay green.

### 3e. The measured budget matches the painted width

**Required:** `find_bounds("main.rs").x  >  find_bounds("日本語.rs").x + 9.0`.

Relational, not a hardcoded column (per epic #480 pillar 3 and quadraui's own "locate targets with `find`, never hardcode coordinates" rule). The ASCII tab sits to the right of the wide tab, so once tab 0 is measured at its true 14 columns, tab 1's label must begin beyond the end of the wide sub-span.

### 3f. Why these three clauses reject partial fixes

| Tree state | §3c (glyphs paint) | §3e (budget agrees) |
|---|---|---|
| Today — neither side fixed | **fail** | fail (unreachable — §3c fails first) |
| Paint fixed, measure not | fail — the label overruns `label_end` and is clipped mid-span | — |
| Measure fixed, paint not | fail — striding still drops `本`/`.` | — |
| Both fixed | pass | pass |

### 3g. Hit regions agree with what was painted

This is the whole downstream point: vimcode's `tab_hit_width` is pinned on the current behaviour and its doc comment names this fix as the unblocker.

**Required:** `driver.click_text("日本語.rs")` activates tab 0 — observable as the app's recorded active-tab id becoming tab 0's after the click.

`click_text` locates via `find_bounds` and clicks the span's centre, so it exercises paint→hit agreement rather than restating either side. Seed the fixture with tab **1** active so the click is a real state change and not a no-op.

### 3h. Emoji — secondary, not load-bearing

An emoji label (e.g. `" 3: 🚀ship.rs "`) *should* behave identically, but emoji width depends on the exact `char_cell_width` table, which #545 has already had to correct once for PUA ranges. Assert emoji **only** as `find_bounds(...).is_some()`, never with a pinned width. If it disagrees, that is a #545-family issue and not a #554 regression — file it separately rather than widening this slice.

---

## 4. #542 — structural parity tier

### 4a. The claim

`tests/cross_backend_parity.rs` observes logical state only (`screen_has -> bool`, `exited -> bool`), so a backend that silently drops chrome — a border, a title, a scrollbar — passes. #541 is the proof: GTK's `draw_tooltip` strokes a full box, TUI paints side-bars only, and `screen_has("Keybindings")` is `true` on both.

### 4b. Required observable

Both drivers already implement `ConformanceDriver::inventory() -> FrameInventory` (`text_runs`, `zones`). The structural tier asserts on the **inventory**, not on text presence.

**Required:** for a fixture drawing a `Tooltip`, both backends report a tooltip zone in `inventory().zones()`, and a backend that omits the border fails a structural assertion **while still passing the behavioural one** — the two tiers must fail separately and be distinguishable from the test output alone (#542 acceptance bullet 3).

**Required:** the suite reproduces the pre-#541 tooltip divergence as a **failure** when run against that tree (#542 acceptance bullet 2). This is the milestone's own mutation check — a structural tier that cannot fail on the case that motivated it is not a tier.

### 4c. Dependency — this clause cannot go green before #490

`FrameInventory.zones` is **declared but always empty today**; its doc comment says so ("reserved for the widget-zone contract … empty until that recording lands"). That recording is **#490**, which is group B. So:

- A slice authored now asserts `zones()` and is red for a reason that is *not* #542's implementation.
- The test-author must therefore either author #542's slice **after #490 merges**, or gate the zone assertions behind an explicit "pending #490" precondition that prints the diagnosis rather than failing on a §4b assertion.

Prefer the former. This is exactly the ms-38 CC-2 failure mode — five tests stranded on a harness gap, failing at a shared precondition instead of on their real clause — and the DAG already orders #490 before #542, so there is no reason to author early.

---

## 5. #492 — Tier C0 + `BackendCaps`

### 5a. Capability declaration

Each backend declares a `BackendCaps` describing what it can do, so a scenario that a backend legitimately cannot run is **skipped with a declared reason** rather than silently passing or spuriously failing (epic #480 acceptance: "skips require a declared capability reason").

**Required:** for every registered backend, its declared `BackendCaps` is queryable from the conformance harness.
**Required:** a scenario skipped for capability reasons reports the **reason string**, and a skip is distinguishable from a pass in the runner's output.

The second clause is the load-bearing one. A capability system whose skip is indistinguishable from a pass converts every unimplemented backend into a green matrix — the precise failure `BackendCaps` exists to prevent.

### 5b. C0 paint smoke

**Required:** every primitive the backend declares support for produces a **non-empty** `inventory().text_runs()` **or** a non-empty `zones()` when drawn once into a fixture.

Deliberately weak — C0 is a boot tier ("this primitive draws *something*"), not a rendering assertion. Strengthening it belongs to C1/#491, not here.

### 5c. Not asserted

The generator itself. #492 auto-generates the per-primitive smoke; a slice that asserted the generated test *list* would restate the implementation and break on every new primitive. Assert the **properties above**, which hold whatever the generator emits.

---

## 6. Test-support seam — how a slice builds its fixture

**No fixture change and no `test-support` feature are required.** #556 established (and its seam tests prove) that this external integration-test crate reaches quadraui's public API and the whole `examples/common` tree via the `#[path]` include already in `quadraui/tests/acceptance.rs`.

For §3, do **not** try to drive `common::tab_group_demo::TabGroupDemo` — its `group` field is private and its only constructor hardcodes ASCII labels, so tab labels cannot be set through it. **Define the fixture inline in the slice instead.** `AppLogic` requires only two methods (`setup` and `tick` have default impls):

```rust
// in the slice file — public quadraui API only
use quadraui::primitives::tab_bar::{TabBar, TabItem};
use quadraui::{AppLogic, Backend, Reaction, Rect, UiEvent, WidgetId};

struct WideTabFixture { /* TabBar + recorded active-tab id */ }

impl AppLogic for WideTabFixture {
    type AreaId = ();
    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        backend.draw_tab_bar(Rect::new(0.0, 0.0, 80.0, 1.0), &self.bar, None);
    }
    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction { /* … */ }
}
```

`draw_tab_bar` returns `TabBarHits`; record them in `handle` so §3g can observe which tab a click activated. `TabBar`/`TabItem` are public, `#[serde(default)]`-heavy structs, so only `id`, `tabs`, and each item's `label`/`is_active` need setting.

**Why inline rather than extending the demo fixture:** an added constructor on `TabGroupDemo` would have to land *before* the sealed slice compiles, and a slice that fails to compile breaks the whole `--test acceptance` target — including other milestones' slices — rather than failing red on its own clause. Inline keeps the slice self-contained and its redness attributable.

Slices are `include!`d below the `SEALED` marker in `quadraui/tests/acceptance.rs`:

```rust
include!("../../tests/acceptance/ms-11/<name>.rs");
```

That entrypoint is declared as this driver's `entrypoint:` in the fleet config, so a `test-author` registering a slice there is **expected**, while a `type="work"` worker touching it trips oracle tamper.

**Feature gates:** per item, never on the file. A TUI-only slice carries `#[cfg(feature = "tui")]` so it never forces a GTK build; §4's cross-backend clauses carry both.

---

## 7. Exemptions — declared reasons

Mirrored machine-readably in `manifest.yml`. Per ORACLE_LOOP.md the issue-level `exempt:` list says "this *issue* doesn't consume the sealed suite" — it does **not** bypass Gate A itself, which is why this contract exists and needs your sign-off even though most of the milestone is exempt.

### 7.1 #490, #491 — harness construction
They build the observation surface and the scenario runner that §4 and §5 assert *through*. Per epic #480's own work order.

### 7.2 #493 — MacDriver: no runner exists in this fleet
`MacDriver` targets the macOS backend. The fleet is precision / elitebook / dellserver, all Linux; `acceptance.drivers.quadraui` declares `capability: gtk` and routes accordingly. A macOS box is milestone #39 / `docs/MAC_MINI.md` and **is not built**. A sealed slice for #493 could be authored but never executed — and per `reference_sealed_acceptance_suite_runs_in_no_gate` an unrunnable suite is worse than none, because it reads as coverage. Revisit when a macOS runner joins the fleet.

### 7.3 #555 — pty/vt100 observer is outside `TestBackend`
#555's whole point is that the matrix observes *draw-time intent* and needs an observer that sees what the terminal actually shows. `TuiDriver` renders to ratatui's `TestBackend`, so terminal-protocol behaviour is out of reach by construction — ORACLE_LOOP.md names this as the known limit, and it is the same boundary quadraui#302's unbuilt pty+vt100 tier exists to cross.

Note this is a limit on the *observer*, not on the repo: quadraui already vendors a patched vt100 (`vendor/vt100-0.16.2-patched`), so once #302's tier lands, #555 becomes contract-bindable and this exemption should be revisited. Note also §2's asymmetry: `row_cells` already models terminal column semantics faithfully, which is why **#554 is testable today without a pty** — the two issues are not blocked on the same thing, despite both being about "what the terminal really shows."

---

## 8. Mocks

| File | Scenario | Issue |
|---|---|---|
| `mocks/tabbar-wide-labels.screen` | Tab bar with a CJK tab and an ASCII control, painted correctly | #554 |

**The mock is illustrative here, not the assertion fixture — this is a deliberate departure from the TUI default.** ORACLE_LOOP.md's "mock == assertion" property holds when a `.screen` grid distinguishes right from wrong. For #554 it *cannot*: §2 shows the broken and fixed renders stringify identically through `screen()`, differing only in cell striding. A golden-grid diff would be blind to the exact defect this milestone's flagship issue is about.

So the mock exists for **your** Gate-A review — to show the intended render — and every load-bearing assertion in §3 goes through `find_bounds`. Do not author a slice that diffs it as a golden grid.

No mock is provided for §4/§5: their observable is a `FrameInventory` and a capability/skip report, neither of which is a screen grid. Their contract text is the reviewable artifact.

---

## 9. Notes / open questions

1. **ORACLE_LOOP.md excludes "quadraui the library."** Its Dogfooding section says the oracle excludes quadraui because it is the still-evolving framework. That was written 2026-07-04 and has since been overtaken: #556 deliberately put quadraui on the loop, and the standing operator policy is that bug fixes are not dispatched without an oracle. The exclusion should be amended in that doc; flagged here rather than silently contradicted. **This is not a blocker for sign-off** — it is a docs-consistency follow-up.

2. **#554 needs a downstream-impact section in its PR.** Epic #480 carries the downstream contract: vimcode's `tab_hit_width` is deliberately pinned on the *current* behaviour and is the single vimcode-side edit once this lands; coord-tui shares the same tab bar and should be checked for the equivalent assumption. The fix must state this. Not an acceptance clause (it is a PR-review requirement), recorded so the reviewer looks for it.

3. **#554 is group A but its siblings are done.** #488, #489, #494 and #556 have all merged, so #554 is the only remaining group-A item and is dispatchable the moment this contract is approved and its slice lands. It does not wait on #490/#491.

4. **§4's mutation check needs a pre-#541 tree.** #542 acceptance bullet 2 requires demonstrating the tier catches the tooltip divergence. Beware the trap recorded in `reference_retro_oracle_mutation_check_and_its_incrate_trap`: reverting a whole file to prove a guard fires can produce a false "unguarded" result. Revert only `draw_tooltip`'s border stroke, not the file.

5. **Amendment invalidates approval.** Per #2063 the Gate-A verdict is keyed to this file's content hash; `coord acceptance mock --amend` re-opens it. Whitespace-only changes do not invalidate. If §4c's dependency ordering or §7's exemptions change, that is an amendment and needs a fresh `coord gate-a --approved`.
