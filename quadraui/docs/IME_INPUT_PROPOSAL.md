# IME / composition input model — proposal

**Status:** Draft, design phase (issue #502). Not yet implemented on any
backend — this document is the thing #502's acceptance criteria asks
for ("written proposal in docs/ … reviewed against vimcode's needs").
Parent epic: **#481**. Related: **#415** ("route clipboard paste +
IME/dead-key composition into the focused terminal PTY") is the
Terminal-specific consumer of the general contract this document
defines; §7 below is written to satisfy it directly.

**Date:** 2026-09-02.

---

## TL;DR

- IME/composition input does not exist anywhere in quadraui today.
  GTK's key controller only ever sees already-resolved keysyms
  (`gtk::run` module doc, quadraui#415's note); macOS's `keyDown:`
  reads `NSEvent.characters()` directly, bypassing `NSTextInputClient`
  entirely (`macos/run.rs`'s `objc_key_down`, ~line 522). `UiEvent`
  has no preedit/composition variants to build a pipeline from —
  `CharTyped`'s doc comment describes the *output* of IME composition
  but nothing produces the composition itself (D-010).
- **Proposal: four new `UiEvent` variants** —
  `CompositionStarted`, `CompositionUpdated { preedit, cursor }`,
  `CompositionCommitted(String)`, `CompositionCancelled` — routed by
  focus exactly like `CharTyped` is documented to be today (no
  `widget` field; the app's own focus tracking decides the target).
- **`CompositionCommitted` supersedes `CharTyped` for IME output.**
  `CharTyped(char)` cannot represent a multi-character CJK commit (a
  whole word committed at once); `CompositionCommitted(String)` can.
  Recommended disposition: deprecate the `CharTyped` variant in place
  (nothing constructs it today — D-010's audit found zero real
  emitters — so this is a documentation-cost-only deprecation, not a
  behavioral one). See §5.
- **New required-with-default `Backend` method**:
  `fn set_ime_cursor_area(&mut self, area: Rect) -> bool`, matching
  the existing `set_cursor`/`begin_window_resize` shape (default
  no-op returning `false`; TUI stays `false` forever — no candidate
  window concept in a terminal). This is exactly the gap
  `BackendCaps::ime`'s doc comment already names ("quadraui has no
  backend-level IME method yet") — the field has existed, permanently
  `false`, since #501/D-010. Adding a `Backend` method with a default
  body is **not a breaking change** per `PRIMITIVE_RULES.md` rule 8's
  table (consumers implement `ShellApp`/`AppLogic`, never `Backend`).
- **GTK and macOS have a push/pull asymmetry that the trait method
  must absorb, not expose.** GTK's `IMContext::set_cursor_location` is
  push (we call it, GTK's popup positions itself). macOS's
  `NSTextInputClient::firstRectForCharacterRange:actualRange:` is pull
  (AppKit calls *us*, synchronously, whenever it wants to know). Both
  backends implement `set_ime_cursor_area` by caching the last rect;
  GTK's override additionally forwards it to `IMContext` immediately,
  macOS's override only updates the cache that the pull callback reads
  later. Same public API, different internal wiring per backend — the
  portability commitment's whole point.
- **Terminal (#415) never forwards preedit bytes to the child PTY.**
  Only `CompositionCommitted`'s final string is written, the same way
  `ClipboardPaste` already routes through `TerminalSession::paste`
  (`terminal_engine.rs:743`). Every native terminal emulator (xterm,
  alacritty, kitty) renders preedit as a local overlay near the cursor
  and never leaks transient composition bytes into the child's input
  stream; quadraui's Terminal primitive must match that behavior
  exactly, or a composing CJK user would see garbage bytes echoed by
  their shell on every keystroke of the composition.
- Downstream impact measured (§8): **zero consumer hits on any symbol
  this proposal adds** (nothing to add yet — it doesn't exist), and
  both consumers' `AppLogic::handle` match arms use a wildcard `_ =>`
  catch-all (checked directly against `~/src/coord-tui/src/app/events.rs`
  and `~/src/vimcode/src/gtk/mod.rs`), so four new `UiEvent` variants
  do not break either consumer's build even without `#[non_exhaustive]`
  on the enum. `CharTyped`'s deprecation is the one item with a real,
  if currently-dead, downstream hit — see §8.
- This document does not implement anything. §9 lists the follow-up
  wiring issues to file per backend once this design is approved, per
  #502's acceptance bar.

---

## 1. Problem, with evidence

Grepped fresh on this branch, not assumed from the issue body:

```
$ grep -rn 'IMContext\|preedit\|marked_text\|insertText\|NSTextInputClient' quadraui/src
quadraui/src/gtk/run.rs: (module-doc prose only, "no gtk4::IMMulticontext … not wired up")
quadraui/src/examples/common/terminal_app.rs: (module-doc prose only, same note)
quadraui/src/backend.rs:241-247 (BackendCaps::ime field doc, "no backend-level IME method yet")
```

No `.rs` file constructs, matches, or calls anything IME-shaped outside
those three doc comments — all three already anticipate this design
story and point at it by number (`#481`, `#502`, `#415`).

- **GTK.** `gtk::run`'s module doc (`quadraui/src/gtk/run.rs:87-103`)
  states the gap explicitly: `EventControllerKey` sees only raw,
  already-resolved keysyms via `gdk_key_to_uievent`
  (`quadraui/src/gtk/run.rs:418-452` wires the controller with no
  `gtk4::IMMulticontext` in front of it). Dead-key sequences (`´` + `e`
  → `é`) and CJK input methods (ibus, fcitx) do not work at all — GDK
  delivers the raw physical keysym for the dead key itself (which has
  no printable `Key::Char` translation) and the composed result never
  arrives.
- **macOS.** `QuadraView::objc_key_down`
  (`quadraui/src/macos/run.rs:522-532`) reads `NSEvent.characters()`
  synchronously off the `keyDown:` event and translates it directly via
  `ns_key_to_uievent`. AppKit's actual input-method contract routes
  `keyDown:` through `-[NSResponder interpretKeyEvents:]`, which calls
  back into `NSTextInputClient` methods (`insertText:replacementRange:`,
  `setMarkedText:selectedRange:replacementRange:`, `unmarkText`,
  `hasMarkedText`, `firstRectForCharacterRange:actualRange:`) — none of
  which `QuadraView` implements. Reading `characters()` directly is the
  same class of shortcut GTK's raw-keysym path takes: it works for plain
  ASCII and silently produces wrong or missing output the moment a real
  input method (Kotoeri, Pinyin, or even the Option-key dead-key layer
  on a US keyboard) is active.
- **Windows.** No TSF (`ITfThreadMgr`, `ITfContextView`,
  `ITfCompositionSink`) wiring exists; `src/win/` doesn't reach text
  input at all yet outside `todo!()` stubs. Out of scope for wiring in
  this design (Win-GUI milestone owns its own dispatch timing), but
  §7's mapping table includes it so the contract is validated against
  three real, structurally different platform APIs before anyone
  starts wiring any of them — retrofitting the `UiEvent` shape after a
  TSF implementation exists would be exactly the kind of breaking
  change `CLAUDE.md`'s downstream-consumers section warns is expensive
  once `develop` has shipped it.
- **`UiEvent::CharTyped`'s doc comment describes a pipeline that
  doesn't exist.** D-010 (`DECISIONS.md`) already re-scoped its
  contract to "IME committed composed text for one character" and
  confirmed **no backend emits it** — the doc comment is forward-looking
  design intent, correctly flagged as unimplemented, not a stale
  claim. This proposal is the design D-010 deferred to #502.

---

## 2. Design goals and constraints

1. **Match D-010's mutual-exclusion invariant.** D-010 already commits
   quadraui to: *"while a composition is in progress, the raw keydowns
   must not also reach the app as `KeyPressed`… the two events are
   naturally mutually exclusive per keystroke once IME lands
   correctly."* This document extends that invariant to the whole
   composition lifecycle (§4), not just the final commit.
2. **Routed by focus, not by hit-test — same as `CharTyped` and
   `KeyPressed` today** (`event.rs`'s routing table). No `widget`
   field on any new variant. This keeps the shape consistent with the
   existing keyboard-event family and avoids inventing a second focus
   model.
3. **Backend-generic.** `AppLogic::handle` must not special-case GTK
   vs. macOS vs. Windows composition quirks — that translation is the
   backend's job, exactly like `poll_events` already normalises
   crossterm/GDK/Cocoa/Win32 events into one `UiEvent` enum.
4. **No silent no-op defaults that hide a real capability gap**
   (`CLAUDE.md`'s portability commitment, `draw_terminal_divider`'s
   precedent). `Backend::set_ime_cursor_area` returns `bool` so a
   caller can tell "not supported on this backend" from "supported,
   applied" — same contract as `set_cursor`/`begin_window_resize`.
5. **v1 does not attempt clause/attribute fidelity.** Real IMEs
   (ibus, fcitx, Kotoeri) report per-segment underline/highlight
   attributes inside the preedit string (GTK's `preedit-changed`
   hands back a `PangoAttrList`; Cocoa's `setMarkedText:` takes an
   `NSAttributedString`). Plumbing `Vec<(Range<usize>, Attr)>` through
   `UiEvent` and every text-input rasteriser is real design surface
   this issue doesn't need to resolve to unblock basic dead-key/CJK
   typing. v1 renders the whole `preedit` string as one underlined run
   (§6) — the same simplified rendering most terminal emulators use
   for CJK preedit today. Tracked as a named follow-up (§9), not
   silently dropped.
6. **Terminal PTY correctness is non-negotiable, not a nice-to-have**
   (§7). Leaking preedit bytes into a child process is a functional
   regression a CJK user would hit on literally the first keystroke of
   every composed character.

---

## 3. `UiEvent` additions

```rust
/// A native input method (IME) began composing text. Fired once per
/// composition sequence, before the first `CompositionUpdated`.
///
/// Routes to the focused text-input widget, same as `KeyPressed` and
/// `CharTyped` — no `widget` field; the app's own focus tracking
/// decides the target, matching every other keyboard-class variant.
///
/// Not every composition is visible to the app as a distinct "start" —
/// some IMEs resolve a dead-key sequence with no preedit shown at all
/// (see the per-backend mapping table, §7). A backend that cannot
/// distinguish "instant, invisible composition" from "no composition"
/// may skip `CompositionStarted`/`CompositionUpdated` and go straight
/// to `CompositionCommitted` — this is conformant, not a bug (§8's
/// emission-matrix update reflects this as an explicitly optional
/// sub-sequence, the same "declare the gap, don't fake it" rule
/// `MouseEntered`/`FilesDropped` already use).
CompositionStarted,

/// The in-progress composition's preedit text changed. `preedit` is
/// the current candidate text as the IME wants it displayed (already
/// resolved to a single string — v1 does not carry per-segment
/// underline/highlight attributes, see design goal 5). `cursor` is a
/// **byte offset into `preedit`**, matching
/// `primitives::form::TextInput::cursor`'s existing convention — not
/// a char index, not a display-column index.
///
/// Fired zero or more times between `CompositionStarted` and the
/// terminating `CompositionCommitted`/`CompositionCancelled`. An IME
/// may fire this many times per composition (every keystroke that
/// changes the candidate) or, per `CompositionStarted`'s doc, never at
/// all for compositions the platform resolves invisibly.
CompositionUpdated { preedit: String, cursor: usize },

/// The composition finished and produced text. Routes to the focused
/// text-input widget, same as `CompositionStarted`.
///
/// **Supersedes `UiEvent::CharTyped` as the canonical IME-output
/// event** — see `DECISIONS.md` D-0NN (this document's own decision,
/// filed alongside the wiring issue that first emits this variant).
/// Unlike `CharTyped(char)`, this carries the *whole* committed
/// string: a CJK IME routinely commits a multi-character word in one
/// shot, which `CharTyped`'s single-`char` payload cannot represent
/// at all (the wiring backend would have had to fan it out into N
/// synthetic `CharTyped` events, silently re-inventing the multi-char
/// commit this variant exists to carry honestly).
///
/// A backend must not also emit `KeyPressed`/`Accelerator` for any
/// keystroke consumed while composing (§4) — this is the terminating
/// event for the suppressed window, not an addition on top of it.
CompositionCommitted(String),

/// The composition was aborted with no text produced (Escape while
/// composing, on platforms where that cancels rather than commits
/// literally; focus loss mid-composition). No payload — there is
/// nothing to insert.
///
/// **Best-effort, not a platform guarantee.** Some IMEs never truly
/// cancel: Escape just collapses the candidate list back to the raw
/// keystrokes typed so far, which the platform then reports as an
/// ordinary commit of that literal text, not a cancellation. Backends
/// emit whichever their platform actually does; apps must not assume
/// every composition ends in exactly one of `Committed`/`Cancelled`
/// paired 1:1 with every `Started` — a composition can also simply
/// never produce either if a backend crashes/the app loses focus in a
/// way the platform doesn't report cleanly. Treat both as
/// best-effort cleanup signals, not as an invariant to assert on.
CompositionCancelled,
```

### Routing table update (`event.rs` module doc)

| Class | Routed by |
|---|---|
| Keyboard (`KeyPressed`, `CharTyped`, `Composition*`) | Focus |

No new row — `Composition*` joins the existing "Keyboard … Focus" row,
it doesn't need its own.

### Emission-matrix update (`docs/BACKEND.md`)

New **optional-capability** rows (composition is a real thing GTK,
macOS, and Windows can all do, but TUI structurally cannot — a
terminal has no OS-level input method framework, only ever raw
bytes/escape sequences from the pty, matching `WindowClose`'s "N/A on
TUI" precedent, not "gap on TUI"):

| Variant | TUI | GTK | macOS | Win | Note |
|---|---|---|---|---|---|
| `CompositionStarted` | N/A | ❌ (design only) | ❌ (design only) | ❌ (design only) | Wiring tracked per §9's follow-up issues. |
| `CompositionUpdated` | N/A | ❌ | ❌ | ❌ | Same. |
| `CompositionCommitted` | N/A | ❌ | ❌ | ❌ | Same — supersedes `CharTyped` once wired. |
| `CompositionCancelled` | N/A | ❌ | ❌ | ❌ | Same. |

---

## 4. Suppression: interaction with `KeyPressed` and `Accelerator`

D-010 already commits quadraui to: raw keydowns consumed by an active
composition must not also reach `KeyPressed`. This section states the
full rule the four new variants must satisfy, because it is wider than
just `KeyPressed`:

**While a composition is active (between `CompositionStarted` and its
terminating `CompositionCommitted`/`CompositionCancelled`), a backend
must not emit `KeyPressed`, `CharTyped`, or `Accelerator` for any
keystroke the IME consumes.** This is not something quadraui has to
build — it is what every native input stack already does *below* the
level quadraui backends read from:

- **GTK**: `GtkIMContext` sits in front of the normal
  `key-press-event`/`EventControllerKey` signal. When
  `gtk_im_context_filter_keypress` returns `TRUE` (the IME consumed the
  key), the widget's own key-press handling never runs for that event —
  there is no raw keysym left for `gdk_key_to_uievent` to translate.
  The suppression is structural: `dispatch_event`'s key-press closure
  simply never fires for a consumed key, nothing to filter after the
  fact.
- **macOS**: `interpretKeyEvents:` calls `NSTextInputClient` methods
  *instead of* letting the raw `keyDown:` fall through to
  `ns_key_to_uievent`. If `objc_key_down` is rewritten to call
  `interpretKeyEvents:` first (§7), a consumed key never reaches the
  existing raw-character path at all — same structural guarantee as
  GTK, not a manual "check if composing, skip" branch.
- **Windows (TSF)**: `ITfKeyEventSink::OnTestKeyDown`/`OnKeyDown`
  give the input method first refusal on every keystroke before the
  app's own `WM_CHAR`/`WM_KEYDOWN` handling runs; a consumed key's
  `WM_CHAR` is suppressed by TSF itself.

**Consequence for a wiring implementation**: this is a "call the
platform API in the right order" requirement, not a "track a
`composing: bool` flag and manually swallow events" requirement. A
backend that finds itself writing `if self.composing { return; }`
around its own `KeyPressed` emission has done the wiring wrong — it
means the raw key event reached the backend at all, which shouldn't
happen if the input-method call is correctly placed *before*
translation, exactly the ordering GTK's `IMContext` and Cocoa's
`interpretKeyEvents:` already enforce for free.

**Accelerators are included, not exempted.** If an app binds Escape or
Enter as an accelerator, that binding must not fire while an IME
candidate window has first refusal on those keys — this is universal
native platform behavior (an open candidate list intercepts
Escape/Enter/arrows for candidate navigation, full stop) and follows
automatically from the same "consumed key never reaches translation"
structural guarantee above, since `Accelerator` matching happens
downstream of the same raw key event `KeyPressed` would have used.

---

## 5. `CharTyped` disposition

D-010 already narrowed `CharTyped`'s contract to "IME committed
composed text for one character" and confirmed zero real emitters
exist anywhere in the tree. `CompositionCommitted(String)` is a strict
superset of that contract (a one-`char` string is a valid
`CompositionCommitted` payload) with none of `CharTyped(char)`'s
inability to represent a multi-character commit.

**Recommendation: deprecate the `CharTyped` variant in place**, via
Rust's per-variant `#[deprecated]` attribute (stable, applies directly
to enum variants — no wrapper type or `pub use` alias needed, unlike
the type-rename or const-alias shims `PRIMITIVE_RULES.md` rule 8
documents for shape-preserving renames):

```rust
#[deprecated(
    since = "0.0.2",
    note = "IME-committed text now arrives via `CompositionCommitted`, \
            which can carry a multi-character commit `CharTyped(char)` \
            cannot represent; see docs/IME_INPUT_PROPOSAL.md"
)]
CharTyped(char),
```

This is the rare case where the ordinary two-PR shim (§ rule 8) is
*cheaper* than usual: `CharTyped`'s payload type (`char`) doesn't
shape-match `CompositionCommitted`'s (`String`), so a `From`/const-alias
forwarding shim isn't possible — but since nothing constructs
`CharTyped` today (confirmed by D-010's grep), there is no real call
site to forward *from*. The deprecation attribute alone is the whole
shim; there's no behavior to migrate, only a doc-comment claim to
retract.

**Downstream impact**: `~/src/vimcode/src/gtk/mod.rs:6959` has a live
`UiEvent::CharTyped(c) => { … }` match arm (found by D-010's own grep,
re-confirmed for this document — see §8). Because
`CLAUDE.md`'s downstream contract explicitly excludes the `deprecated`
lint from the consumer-side `-D warnings` gate (*"a `deprecated`
warning must never fail CI in `coord-tui` or `vimcode` on account of a
quadraui shim"*), this deprecation does not turn vimcode's CI red —
it surfaces as a warning naming the fix, exactly the intended shape.

**PR 1** (a #502 follow-up wiring issue, not this design PR): add the
four `Composition*` variants, mark `CharTyped` `#[deprecated]`, migrate
this crate's own two in-tree `CharTyped`-matching call sites
(`compose::sidebar_system`, `compose::tree_controller` — both already
match `Key::Char` from `KeyPressed` too, so dropping the `CharTyped` arm
loses no behavior since no backend ever fed it one) to stop matching
the deprecated variant, so `-D warnings` stays green in this repo per
rule 8's "quadraui migrates its own call sites in the same PR" rule.
Open vimcode's migration issue in the same session (swap its dead
`CharTyped(c)` arm for `CompositionCommitted(s)` — trivial, since the
arm is unreachable today either way).

**PR 2**: delete `CharTyped` once vimcode's migration merges.

---

## 6. Caret-rect feedback channel

```rust
/// Report the screen-space rect of the text-input caret, so the
/// backend can position a native IME candidate window (or, on
/// platforms with a pull-based query instead of a push API, cache the
/// rect to answer that query when the OS asks).
///
/// Call on every `CompositionStarted`/`CompositionUpdated` and on
/// caret movement while a composition may reopen (the OS may query the
/// caret rect lazily — see the per-backend mapping table, §7 — so a
/// stale cached rect is a real, if minor, positioning bug, not merely
/// a missed optimization).
///
/// `area` is in the same absolute, window-native coordinate frame
/// `Backend::text_input_layout` already returns (D-005: ABSOLUTE,
/// screen/view-relative units — TUI cells, GTK/macOS points, Win
/// DIPs). Apps that already resolve a caret rect for their own
/// text-input rendering (every text-input primitive's `layout()`
/// already returns one) pass it through unchanged; no new coordinate
/// math.
///
/// Returns `false` on backends with no native IME candidate-window
/// concept (TUI — a terminal cannot draw an OS-level popup over its
/// own cells) or before a window exists. Callers should treat `false`
/// as a no-op, not an error — same contract as `set_cursor` and
/// `begin_window_resize`.
fn set_ime_cursor_area(&mut self, _area: Rect) -> bool {
    false
}
```

Adding this to `Backend` with a default body is a **non-breaking**
change under `PRIMITIVE_RULES.md` rule 8's table: consumers implement
`ShellApp`/`AppLogic`, never `Backend` — "in-tree backends only, keep
doing this."

**Wires up the capability flag that has been waiting for it since
#501.** `BackendCaps::ime`'s doc comment (`backend.rs:241-247`)
already states: *"quadraui has no backend-level IME method yet …
composed text arrives pre-resolved from the OS either way, so this
tracks positioning the IME candidate window."* That doc comment also
references `crate::event::UiEvent::Text`, a variant that has never
existed in this crate (confirmed: `grep -n 'UiEvent::Text\b'
quadraui/src/event.rs` returns nothing) — a stale reference this
document's wiring PR should fix to point at `CompositionStarted`/
`CompositionUpdated` instead. `BackendCaps::ime` flips to `true` for a
backend the moment it overrides `set_ime_cursor_area` with a real body
(`CAP_CONTRACTS`, `backend.rs:2288`, already wired to check exactly
that).

---

## 7. Per-backend mapping table

| Concern | GTK (`gtk4::IMMulticontext`) | macOS (`NSTextInputClient`) | Windows (TSF) |
|---|---|---|---|
| Attach point | New `IMMulticontext` in front of the existing `EventControllerKey` in `gtk::run`'s widget setup (`gtk/run.rs:418-452`). `gtk_im_context_filter_keypress` gets first refusal on every key event. | `QuadraView` implements `NSTextInputClient`; `objc_key_down` calls `self.interpretKeyEvents(&[event])` instead of reading `characters()` directly (`macos/run.rs:522-532`). | `ITfThreadMgr`/`ITfDocumentMgr` registered against the window's `HWND`; `ITfKeyEventSink` gets first refusal via `OnTestKeyDown`/`OnKeyDown`. |
| Start | `preedit-start` signal | First `setMarkedText:selectedRange:replacementRange:` call (no separate "start" callback — the first call with non-empty marked text *is* the start) | `ITfCompositionSink::OnStartComposition` |
| Update | `preedit-changed` signal; `gtk_im_context_get_preedit_string` returns `(text, PangoAttrList, cursor_pos)` — v1 uses `text`/`cursor_pos` only, drops the attr list (design goal 5) | Repeated `setMarkedText:selectedRange:replacementRange:` calls; `selectedRange` maps to `cursor` | `ITfCompositionSink::OnUpdateComposition` |
| Commit | `commit` signal, carries the final `&str` directly | `insertText:replacementRange:` — carries an `NSString` or `NSAttributedString` (strip attributes for v1, same as Update) | `ITfCompositionSink::OnEndComposition` + the context's final text run |
| Cancel | No universal single signal — inferred from `preedit-changed` firing with empty text followed by no `commit`, or focus-out mid-composition | `unmarkText` called with no immediately-preceding `insertText:` | Composition ends via `OnEndComposition` with an empty/unchanged text run — same "infer from absence" shape as GTK |
| Caret-rect feedback | **Push.** `gtk_im_context_set_cursor_location(rect)` — call every time the app's known caret rect changes; GTK positions its own popup. | **Pull.** `firstRectForCharacterRange:actualRange:` — AppKit calls back into `QuadraView` synchronously whenever it wants the rect. `set_ime_cursor_area` only updates a cached `Cell<Rect>` the callback reads; there is no direct "push" call on this platform. | **Pull**, similar shape to macOS: `ITfContextView::GetTextExt`. Same caching strategy as macOS. |
| Suppression mechanism | Structural — a consumed key never reaches `EventControllerKey`'s own handler (§4) | Structural — a consumed key never reaches `ns_key_to_uievent` because `interpretKeyEvents:` intercepts it (§4) | Structural — `OnTestKeyDown`/`OnKeyDown` run before the app's own `WM_CHAR` (§4) |

The push/pull split (GTK push vs. macOS/Windows pull) is the one place
the three platforms genuinely disagree on control flow, not just
naming. `Backend::set_ime_cursor_area`'s signature absorbs this
difference entirely inside each backend's implementation — GTK's
override both caches the rect *and* forwards it immediately; macOS's
and Windows' overrides only cache it, because there is nothing to
"forward" until the OS asks. No caller-visible difference; this is
exactly the kind of asymmetry the portability commitment (`CLAUDE.md`)
says the trait exists to absorb.

---

## 8. Downstream impact

Per `CLAUDE.md`'s downstream-consumers policy and `PRIMITIVE_RULES.md`
rule 8, measured before proposing anything, both checkouts sitting
beside this one:

```
$ grep -rn 'CompositionStarted\|CompositionUpdated\|CompositionCommitted\|CompositionCancelled\|set_ime_cursor_area' \
    ~/src/coord-tui/src ~/src/vimcode/src
(no hits)
```

Nothing to break — these symbols don't exist yet anywhere. The one
real hit from this design is `CharTyped`'s deprecation (§5):

```
$ grep -rn '\bCharTyped\b' ~/src/coord-tui/src ~/src/vimcode/src
vimcode/src/gtk/mod.rs:6959:            UiEvent::CharTyped(c) => {
```

Already covered by §5's plan: `#[deprecated]` on the variant (excluded
from the consumer `-D warnings` gate per `CLAUDE.md`), with vimcode's
migration issue opened in the same session as the PR that adds the
attribute, per rule 8's two-PR protocol.

**New `UiEvent` variants and match exhaustiveness.** `UiEvent` is not
`#[non_exhaustive]` today. Per rule 8's table, a new enum variant
"breaks consumers… unless `#[non_exhaustive]`" — meaning any exhaustive
`match` without a wildcard arm fails to compile the moment these
variants are added. Checked directly rather than assumed:

```
$ grep -n '_ =>' ~/src/coord-tui/src/app/events.rs | wc -l
29
$ grep -n 'UiEvent::CharTyped' ~/src/vimcode/src/gtk/mod.rs
6959:            UiEvent::CharTyped(c) => {
   (same match block ends in a catch-all `_ => {}` arm)
```

Both consumers' `AppLogic::handle`-adjacent matches on `UiEvent` use a
`_ =>` catch-all, not an exhaustive variant list — adding four new
variants does not fail either consumer's build. This is evidence, not
a guarantee for all time: a future match added to either consumer
without a wildcard would still break on the next variant add, which is
exactly what `#[non_exhaustive]` exists to make impossible by
construction. **Recommendation for the wiring PR**: add
`#[non_exhaustive]` to `UiEvent` in the same PR that adds the four
`Composition*` variants. It costs nothing today (both real match sites
already use wildcards) and forecloses the failure mode for good,
matching rule 8's own preference ordering ("prefer a shape that isn't
breaking at all… `#[non_exhaustive]` on public structs and enums").

**`## Downstream impact` section for the eventual wiring PR(s)**: each
should restate this section's grep against `develop`'s tip at PR time
(not copy this document's, which may be stale by then), per rule 8's
"paste the grep output, don't just assert it."

---

## 9. Conformance and testing plan

Per the issue's own acceptance item 5 and `docs/TESTING.md`'s tier
taxonomy:

- **Tier C4 (native residue, never shared)** — the actual
  dead-key/CJK/IME candidate-window behavior is real OS integration
  that cannot be driven headlessly (`TestBackend`/`GtkDriver` have no
  IME to compose with). Per-backend, human-run smoke: type a dead-key
  accent on GTK with a real X11/Wayland IME active, type Japanese via
  Kotoeri on macOS, confirm the candidate window appears near the
  caret and the committed text lands in the focused field. This is the
  same tier `docs/TESTING.md` already reserves for "exact colours, font
  rendering, wide-glyph pixels, live-window smoke" — composition is
  categorically the same kind of thing.
- **Tier C2 (event parity)** — once a backend wires real IME
  translation, `tests/conformance/c2.rs`'s native-injection-recipe
  pattern extends naturally: feed a synthetic `preedit-changed`/
  `setMarkedText:` call into the translation function directly (not
  through a live IME) and assert the resulting `UiEvent` shape. This
  proves the *translation*, not the OS's IME itself — same split C2
  already draws for mouse/key/scroll today.
- **Driver-injectable preedit path, for `TuiDriver`/shared `AppLogic`
  tests.** TUI has no real IME (§3's emission table: N/A on every
  `Composition*` row), but text-input primitives (`TextInput`, `Form`,
  `Editor`) still need their preedit-rendering code path exercised by
  something headless. Proposal: `TuiDriver` gains a
  `compose(preedit: &str, cursor: usize)` scripting method (alongside
  the existing `press`/`click`/`drag`/`ctrl_char`) that synthesizes a
  `CompositionUpdated` event directly — the same "script the event, not
  the native input" pattern `TuiDriver` already uses for every other
  interaction. This lets any `AppLogic` fixture exercise "an IME is
  mid-composition" behavior (preedit rendered inline, underlined,
  `KeyPressed` suppressed for the duration) without a real TUI backend
  ever emitting the event natively — mirrors how `ClipboardPaste` is
  already driver-injectable today without any backend running a real
  OS clipboard.

---

## 10. Text-input primitives: rendering preedit inline

Every text-input consumer (`TextInput`/`PasswordInput` in `Form`,
`Editor`, and — per #415 — `Terminal`) needs a way to show the preedit
string inline at the caret while composing, visually distinct from
already-committed text. Convention, matching what most native text
widgets already do and requiring no new primitive-level fields beyond
what `CompositionUpdated` already carries:

- Preedit renders **inline at the cursor position**, using the
  existing `cursor: Option<usize>` field's position as the insertion
  point — the app splices `preedit` into the displayed string at that
  byte offset for the duration of the composition, the same way it
  already splices in confirmed keystrokes.
- Visual treatment: **underline** the preedit span (GTK's Pango
  underline attribute and macOS's `NSUnderlineStyleAttributeName` are
  literally what `setMarkedText/preedit-changed` want to convey; TUI
  has no native concept but ratatui's `Modifier::UNDERLINED` maps
  directly). No color/background change in v1 — clause-segment
  highlighting is design-goal-5's deferred attribute plumbing.
- `CompositionUpdated.cursor` (byte offset *within `preedit`*) positions
  a secondary, thinner caret inside the underlined span — this is the
  "where would the next keystroke land" indicator IMEs show while
  composing a multi-segment candidate (e.g. converting between kana and
  kanji).
- On `CompositionCommitted`, the app replaces the inline preedit span
  with the committed string at the same offset and advances `cursor`
  past it — structurally identical to how a `KeyPressed{Key::Char}`
  already gets spliced in today, just with a string instead of one
  char.
- On `CompositionCancelled`, the app simply removes the preedit span
  and restores the pre-composition cursor position — no text changes
  ever touch the primitive's committed `value` until a real commit
  happens, so cancellation is a pure no-op on the underlying string.

**Terminal is the one exception to "splice into the primitive's own
text buffer"** — see §11 below; a PTY-backed terminal's "text buffer"
is the child process's own state, which quadraui cannot and must not
speculatively mutate with in-flight preedit.

---

## 11. Terminal / PTY interaction (#415)

`Terminal`'s `draw_terminal` renders cells sourced from the child
process's own screen state (`terminal_engine.rs`), not from
quadraui-owned text like `TextInput`. This makes the correct behavior
different in one crucial way from every other text-input primitive:

**Preedit must never be written to the PTY.** Only
`CompositionCommitted`'s final string is forwarded, through the same
path #415 already established for paste:

```rust
// #415's existing precedent (terminal_engine.rs:743):
session.paste(&pasted_text);

// This proposal's analogous call, once wired:
UiEvent::CompositionCommitted(text) => session.paste(&text),
```

(Whether the wiring issue reuses `paste` verbatim or adds a distinct
`commit_composed_text` method that skips paste's bracketed-paste
wrapping is an implementation detail for that issue — IME-committed
text is typically not bracket-wrapped the way a large multi-line paste
is, since it arrives as ordinary keystroke-equivalent input as far as
the child shell is concerned. Flagged here so the wiring issue doesn't
default to the wrong one without considering it.)

**Preedit renders as a local overlay, not a PTY write.** While
composing, the terminal draws the current `preedit` string as an
underlined overlay at the cursor cell (§10's inline convention,
applied to `Terminal`'s own cell grid instead of a `TextInput`'s
string buffer) — purely a rendering decision inside `draw_terminal`,
touching no PTY state. This matches xterm/alacritty/kitty's own
behavior exactly: they all render preedit locally and only write bytes
to the child on commit. Getting this wrong (writing preedit bytes
speculatively, then "undoing" them on cancel) is not just visually
wrong — many shells and full-screen programs (vim, less) cannot have
speculative bytes retracted once written, so a naive implementation
would corrupt the child's input stream on every composition that gets
revised or cancelled.

---

## 12. Follow-up wiring issues to file

Workers don't have GitHub write access — **coordinator: please open
these against epic #481** once this design is approved, mirroring
D-010's own "Follow-ups" section:

1. **`UiEvent` + `Backend` trait changes** — add the four
   `Composition*` variants, `#[non_exhaustive]` on `UiEvent` (§8),
   `Backend::set_ime_cursor_area` with its default body, deprecate
   `CharTyped` in place (§5), migrate quadraui's own two in-tree
   `CharTyped`-matching call sites. No backend wiring yet — this issue
   is the shape-only PR the three backend issues below build on.
2. **GTK wiring** — `IMMulticontext` in `gtk::run`'s widget setup,
   `preedit-start`/`preedit-changed`/`commit` signal handlers emitting
   the new variants, `set_ime_cursor_area` forwarding to
   `gtk_im_context_set_cursor_location`. Closes the "not wired up" gap
   `gtk/run.rs`'s own module doc already names by number.
3. **macOS wiring** — `NSTextInputClient` on `QuadraView`
   (`insertText:replacementRange:`, `setMarkedText:selectedRange:
   replacementRange:`, `unmarkText`, `hasMarkedText`, `markedRange`,
   `firstRectForCharacterRange:actualRange:`), `objc_key_down` routes
   through `interpretKeyEvents:` first. `set_ime_cursor_area` caches
   the rect for the pull-based callback (§7).
4. **Windows TSF wiring** — scoped separately since `src/win/` is
   still pre-implementation; folds into the Win-GUI milestone's own
   sequencing, not blocking on GTK/macOS landing first.
5. **Text-input primitive rendering** — `TextInput`/`PasswordInput`
   (`Form`), `Editor` inline preedit rendering per §10, on TUI + GTK
   first (macOS/Win once their backends exist for those primitives).
   Ships with a `tui_ime_demo`/`gtk_ime_demo` example pair per
   `CLAUDE.md`'s "demos are mandatory for visual features" rule, and
   the example's `TuiDriver` end-to-end test using the `compose(...)`
   scripting method §9 proposes.
6. **Terminal PTY wiring (#415's remaining half)** — `CompositionCommitted`
   → `TerminalSession::paste`-or-sibling per §11, preedit overlay
   rendering in `draw_terminal`. This is the issue that actually closes
   #415, which currently only has the clipboard-paste half done.
7. **`TuiDriver::compose(...)` scripting method** (§9) — needed by
   issue 5 above's driver test; can land standalone or as part of it.
8. **Clause/attribute-span fidelity** (design goal 5's deferred scope)
   — plumb per-segment underline/highlight through `CompositionUpdated`
   once a real consumer needs it (this document intentionally does not
   block basic composition support on it).

---

## 13. Open questions for review

1. Should `CompositionUpdated.cursor` also carry a *selection* range
   (some IMEs let you use arrow keys to move within an uncommitted
   candidate, distinct from the final insertion point), the way
   `TextInput`'s own `cursor`/`anchor` pair does for confirmed text? v1
   above proposes a single cursor offset, matching what GTK's
   `preedit-changed` and macOS's `setMarkedText:`'s `selectedRange`
   both actually provide out of the box — extending to a full
   anchor/cursor pair is possible later without breaking the existing
   field (additive).
2. Is `#[non_exhaustive]` on `UiEvent` (§8) in scope for *this* design
   story, or should it be its own small PR ahead of the wiring issue
   that adds the four variants? Recommendation above bundles it with
   issue 1 in §12; happy to split if reviewers prefer a smaller diff
   per PR.
3. `CompositionCancelled`'s best-effort framing (§3) means an app
   cannot reliably assert "every `Started` is followed by exactly one
   terminating event." Is that an acceptable contract, or does a
   future backend need a stronger guarantee (e.g. synthesizing a
   `Cancelled` on focus-loss even when the platform doesn't report one
   cleanly)? Leaning toward "best-effort is honest and sufficient" —
   text-input consumers already have to handle focus loss generally
   (abandoning an in-progress edit is not a new failure mode IME adds).
