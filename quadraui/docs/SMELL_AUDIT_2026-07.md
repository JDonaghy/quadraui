# quadraui — Code Smell + Cross-Platform Portability Audit

**Date:** 2026-07-25 · **Branch:** `develop` @ `73172ff` · **Scope:** `/home/john/src/quadraui` (read-only), reference: `~/src/vimcode`
**Method:** four parallel evidence sweeps (backend duplication/leaks, panic paths/dead API, test-suite quality, primitive coverage matrix) + first-hand verification of every headline claim + full quality-gate run. The graphify graph mandated by CLAUDE.md could not be used — `graphify-out/` contains only a `.gitignore` (see DOC-04).

Scale: `quadraui/src` = 112,372 LoC / 199 files (`tui/` 22.6K · `gtk/` 17.5K · `macos/` 17.0K · `win/` 546 · `primitives/` 17.8K · `compose/` 22.3K · shared root 14.2K). 1,532 unit tests + 84 TuiDriver + 4 GtkDriver + 1 parity test, all green.

---

## 1. Executive summary + portability readiness verdict

**Verdict: AMBER-RED. The portable *shape* is real; the portable *guarantee* is not.**

The architecture quadraui promises — cfg-free primitives, one trait method per primitive, backend-neutral compose helpers, a unified `UiEvent` boundary — genuinely exists and is unusually well-executed at the module-hygiene level: **zero** backend types or `#[cfg(feature)]` gates leak into `primitives/`, `dispatch.rs`, `frame.rs`, or (production) `compose/` code. The quality gate is green (build, 1,621 tests, clippy `-D warnings` on both features, fmt — note: **#403's clippy failure no longer reproduces on develop**).

But measured against the stated non-negotiable — *"a future agent should be able to write the entire Windows or macOS backend with almost no input, just by implementing the `Backend` trait"* — the repo fails today, on five compounding grounds:

1. **Both non-CI'd backends are compile-broken right now.** `MacBackend` is missing 7 required trait methods (the `macos` feature cannot build on an actual Mac); `WinBackend` is missing 7 methods plus a signature drift, so `cargo check --features win` fails — directly contradicting the Cargo.toml comments and README's "macOS backend is feature-complete". CI builds neither, and CI triggers only on `main`, not `develop`. The exact mechanism that was supposed to keep backends honest ("the compiler tells you when you're done") is disconnected.
2. **~65–75% of every rasteriser is re-implemented shared logic.** 1,671 non-trivial lines in `macos/*.rs` are byte-identical to `gtk/*.rs`; `mac_tree_layout` *is* `gtk_tree_layout` modulo one comment. The copies have already drifted (macOS measures MessageList sections at height 0.0; the CJK wide-glyph fix exists only on GTK). A Windows agent would copy ~10–12K lines of layout math a fourth time and introduce a fourth drift surface.
3. **The runtime layer around the trait is not shared.** Event-loop pre-processing (Ctrl-C copy, select-all, selection clearing, accelerator matching) is forked three ways and absent on macOS; resize debounce is written twice and missing on macOS; `shell_runner` exists only for TUI+GTK; the CSD titlebar drag/resize state machine, modal re-entrancy guard, and smoke harness are trapped in `gtk/`. macOS never emits `DoubleClick`, never matches registered accelerators, never fires resize events.
4. **A backend can silently be wrong.** 13 trait methods have no-op/false defaults, so a new backend compiles while discarding the theme, never rendering selections, and resolving editor clicks with a plausibly-wrong default (`editor_col_at_x`). There is **no error type anywhere in the public API** — the only expressible failure modes are silent no-op or process abort (`win/services.rs` aborts on Ctrl-V today).
5. **There is no backend-agnostic conformance suite.** Cross-backend behavioural checking is one parity test over one scenario; GtkDriver's `find()` is fed by only 2 of 37 rasterisers, structurally capping the GTK suite; macOS has 193 good tests that never run anywhere and no driver at all.

**What is genuinely strong** (and worth saying so an implementing agent trusts the base): the primitive layer's descriptor/layout/hit_test discipline, the paint↔click round-trip harness culture (TUI 20 modules, macOS 21), the `TuiDriver`/`GtkDriver` design, `dispatch.rs` as genuinely shared input routing, and the near-total absence of cfg sprawl. The fix is not a rewrite; it is (a) reconnecting the compile/CI truth loop, (b) lifting the duplicated 65% into shared core, (c) making conformance executable.

Estimated cost for a Windows backend **today**: ~4–6 weeks of agent effort with heavy human course-correction (mostly re-deriving unshared logic and discovering silent gaps). After the four epics proposed in §7: plausibly ~1–2 weeks, mostly native-API work, with a burn-down checklist and green-signal-from-day-one.

---

## 2. Ground truth — quality gate (run 2026-07-25 on `develop`)

| Gate step | Result |
|---|---|
| `cargo build --features tui --features gtk` | ✅ 10.1s |
| `cargo test --features tui` | ✅ 1,532 unit + 84 TuiDriver + 4 GtkDriver + 1 parity + 23 doc (18 ignored) |
| `cargo test --features gtk` | ✅ (workspace feature-unification makes both runs near-identical) |
| `cargo clippy --features tui -- -D warnings` | ✅ **passes** (contra open issue #403) |
| `cargo clippy --features gtk -- -D warnings` | ✅ passes |
| `cargo fmt --check` | ✅ clean |
| `cargo check -p quadraui --features win` | ❌ **E0046 + E0050** (see PORT-02) |
| `--features macos` on macOS host | ❌ would fail — 7 trait methods missing (PORT-01); on Linux the module is cfg-skipped, not checked |

kubeui / kubeui-core / kubeui-gtk: **0 tests** in all three demo crates.

---

## 3. Findings

Severity: **C**ritical / **H**igh / **M**edium / **L**ow. Size: S (<1d) / M (1–3d) / L (>3d).

### 3.1 Portability (PORT-xx) — the priority half

---

**PORT-01 · C · macOS backend cannot compile — 7 required trait methods missing** (Size M)
`quadraui/src/macos/backend.rs:245` (`impl Backend for MacBackend`) lacks `status_bar_layout`, `tab_bar_layout`, `activity_bar_layout` (added by #210), `draw_command_line` (#201), `draw_split_tree` + `split_tree_layout`, `draw_board` (#362) — vs trait decls at `quadraui/src/backend.rs:507,515,518,534,731,736,866`. Masked because `lib.rs:106` gates the module `#[cfg(all(feature = "macos", target_os = "macos"))]` and CI is ubuntu-only. macOS also has **no rasteriser files** for board, command_line, diff_view (impl returns a fake `DiffViewLayout{visible_rows: 0}` at `macos/backend.rs:1418`), drop_overlay (empty body + TODO at `macos/backend.rs:959`), split_tree, text_input.
*Why it matters for the ports:* the macOS backend is the template a Windows agent will copy; today the template is broken and the README claims otherwise. Also proves trait changes do not propagate to non-CI'd backends — the exact failure mode the Windows port will hit continuously.
*Fix:* implement the 7 methods + missing rasterisers (M); pair with PORT-03's CI gate so it can't recur.

**PORT-02 · C · Win stub backend fails `cargo check --features win`** (Size S)
Missing `status_bar_layout`, `tab_bar_layout`, `activity_bar_layout`, `draw_command_line`, `draw_chart`, `chart_layout`, `draw_board` (E0046) + `draw_data_table` has 3 params vs the trait's 4 (E0050) — `win/backend.rs:100-449` (59 `todo!()` across 67 fns). `Cargo.toml`'s `win` feature comment ("the Backend impl is exercised by `cargo check --features win`") is false in both directions: it doesn't compile and nothing exercises it. Additionally `win/services.rs:11,14,46,50,54,58` are `todo!()` where the return types already offer graceful `None`/`()` — Ctrl-V would abort the process.
*Fix:* restore trait conformance with honest stubs; convert services `todo!()` → `None`/no-op; CI-gate (PORT-03). S.

**PORT-03 · C · CI cannot see three of five declared backends, and doesn't run on `develop`** (Size S)
`.github/workflows/ci.yml:4-7` triggers on `main` only; day-to-day work on `develop` gets zero CI. Jobs cover `--features tui` and `--features gtk,tui` only: no `--features win` check, no macOS job/runner (193 macOS tests never execute anywhere), and the `terminal` feature is never enabled standalone — `tui_terminal`/`gtk_terminal` (`required-features`) are silently skipped by every build step despite being the subject of recent fixes (#397 #437 #439 #452).
*Fix:* add `cargo check -p quadraui --features win` + `--features terminal` steps; add a macOS runner job (or at minimum a documented cfg-strategy making macOS code type-checkable off-Mac); trigger on `develop`. S.

**PORT-04 · H · UiEvent emission parity is unenforced and already broken** (Size M)
Verified per-variant emitter scan (`quadraui/src/{tui,gtk,macos,win}`):
| Variant | tui | gtk | macos | note |
|---|---|---|---|---|
| `WindowClose` | — | — | — | never emitted by anyone |
| `MouseEntered` / `MouseLeft` | — | — | — | never emitted |
| `FilesDropped` | — | — | — | never emitted |
| `DpiChanged` | — | — | — | never emitted (see PORT-12) |
| `DoubleClick` | ✅ (synth, `tui/events.rs:140-183`) | ✅ (GDK n_press) | ❌ ignores `clickCount` | tab double-click, titlebar maximize dead on macOS |
| `ClipboardPaste` | ✅ | ✅ | ❌ | paste dead on macOS |
| `CharTyped` | ✅ | ❌ | ✅ | GTK text input arrives only as `KeyPressed{Char}` |
| `WindowResized` | ✅ | ✅ | ❌ (`ns_resize_to_uievent` defined at `macos/events.rs:251`, **never called**) | |
| `Accelerator` | ✅ (`tui/backend.rs:604`) | ✅ (`gtk/run.rs:1044`) | ❌ registered but never matched (`macos/backend.rs:280-292`) | |
LESSONS.md:91-100 states this exact rule ("all runners must fire all UiEvent variants") and it is violated nine ways. A Windows author has no machine-readable list of which variants are mandatory.
*Fix:* publish a required/optional emission matrix per backend; conformance-test it (Epic 2, Tier C2); decide emit-or-remove for the four never-emitted variants. M.

**PORT-05 · H · IME / composition input does not exist anywhere** (Size L)
Zero hits for `IMContext|preedit|marked_text|insertText` across `src/`. GTK wires no `gtk::IMContext` (`gtk/run.rs` key controller only); macOS reads `NSEvent.characters()` in `keyDown:` (`macos/run.rs:309-314`) with no `NSTextInputClient`, so dead keys, Japanese/Chinese/Korean input, and emoji pickers are broken on both GUI backends. `UiEvent::CharTyped`'s doc ("IME-composed, ready for insertion", `event.rs:241-243`) describes a pipeline that doesn't exist, and `UiEvent` has no preedit/composition variants to express one. #415 covers only GTK-terminal paste/IME routing.
*Why it matters:* Windows (TSF) and macOS (NSTextInputClient) both demand a real composition model; retrofitting it later changes the `UiEvent` contract every app depends on. Design it once now. L.

**PORT-06 · H · 13 default no-op trait methods = a backend can compile while silently wrong** (Size M)
`backend.rs`: `set_theme:111`, `set_nerd_fonts:123`, `set_editor_font:143`, `register_text_region:164`, `cancel_text_selection_drag:195`, `install_menu_bar:228`, `show_context_menu:248`, `begin_window_drag:285`, `toggle_window_maximize:296`, `begin_window_resize:322`, `set_cursor:334`, `scales_text_rows:415`, `editor_col_at_x:651`. Consequences per method are catastrophic-but-silent: theme discarded, text selection nonexistent, editor clicks resolved with plausibly-wrong uniform division (#420 re-opens itself per new backend). Live instances: **Win takes the `set_theme` default today**; **GTK takes `cancel_text_selection_drag`'s default** (the spurious-`TextSelectionChanged` bug the method exists to prevent is live on GTK). The `bool` returns conflate "not applicable (TUI)" with "forgot to implement" by documented design (`backend.rs:281`).
*Fix:* don't remove the defaults (they're legitimate for TUI) — make the gaps *visible*: conformance Tier C0/C1 checks per capability + a `fn capabilities(&self) -> BackendCaps` style self-declaration so N/A vs unimplemented is distinguishable. M (design), pairs with Epic 2.

**PORT-07 · H · No error channel in the entire public API** (Size M design, L rollout)
No `Result`, no error type, no `thiserror`/`anyhow` anywhere in `quadraui/src` public API (grep-verified). Fallible operations use `Option` (cancelled vs unsupported vs failed are indistinguishable), `bool` (N/A vs broken indistinguishable), `()` + silent no-op, or abort (`todo!()`/`expect`). Cairo paint errors are swallowed with `.ok()` (`gtk/command_line.rs:52`, `gtk/tree.rs:304,325,343`, and peers). A Direct2D backend *must* report device-lost/swapchain-recreate; today it structurally cannot.
*Fix:* minimal `BackendError`/`ServiceError` for the runtime seams (`begin_frame`, `wait_events`, services) — not a crate-wide Result-ification. M/L.

**PORT-08 · H · 65–75% of every rasteriser is re-implemented shared logic — a 4th copy awaits Windows** (Size L)
Measured: native-drawing lines are only 9–39% of each rasteriser file (tree: tui 14% / gtk 30% / mac 19%; form: 10/39/16; terminal: 9/21/21…). 1,671 non-trivial lines byte-identical between `gtk/*.rs` and `macos/*.rs` (`macos/multi_section_view.rs` 51%, `find_replace.rs` 54%, `sidebar_panel.rs` 61%). Exhibits:
- `gtk/tree.rs:30-58` ≡ `macos/tree.rs:37-64` (`mac_tree_layout` = `gtk_tree_layout`; magic constants 1.2/1.4/0.9/0.65/2.0/4.0 duplicated verbatim).
- MSV `body_measure`: `gtk/multi_section_view.rs:68-111` vs `macos/multi_section_view.rs:72-105` — **already drifted**: GTK computes real MessageList height (`:97-106`), macOS returns `0.0` (`:101`) — a latent macOS layout bug born purely of copy-paste.
- Terminal overlay ladder + magic colours `(255,165,0)`/`(100,80,20)` hardcoded in all three (`tui/terminal.rs:94-100`, `gtk/terminal.rs:100-130`, `macos/terminal.rs:67-77`); the #439 wide-glyph fix exists **only** in `gtk/terminal.rs:78-90` (macOS mis-paints CJK — #440 filed).
- Form metrics: `6.0 + label_w + 12.0` and `row_h = (lh*1.4).round()` duplicated `macos/form.rs:146-150,51-52` vs inlined `gtk/backend.rs:1901-1912`.
*Fix:* a shared **pixel-metrics + paint-plan layer**: pure functions (no native handles) that turn `(primitive, rect, TextMeasure)` into layout + a draw-op list; each backend keeps only the native draw-op executor. Start with tree/MSV/form/terminal where identity is proven. L (incremental per primitive).

**PORT-09 · H · Runner/event-loop layer is copy-paste, forked, and partially absent** (Size L)
- `EventOutcome` declared twice verbatim (`tui/run.rs:275-282`, `gtk/run.rs:979-986` — the latter's comment says "Mirrors crate::tui::run::EventOutcome").
- `dispatch_event` pre-processing (Ctrl-C copy → `TextCopied`, Ctrl-A select-all, MouseDown clears selection, `TextSelectionChanged` bookkeeping) implemented twice and forked (GTK adds ActivityBar intercept + accelerators + Ctrl-V; TUI does accelerators in `wait_events` instead); **macOS has none of it** (`macos/run.rs:490-502` is a bare `app.handle`).
- 120ms resize-settle debounce implemented twice with two mechanisms (`tui/run.rs:52-70,165-207`, `gtk/run.rs:680-754`), absent on macOS.
- `apply_reaction` ×3; render_frame skeleton ×3.
- `shell_runner` (ShellApp → AppLogic adapter construction): TUI factored it as `build_shell_adapter` (`tui/shell_runner.rs:22-67`) explicitly so runner and test driver can't drift; GTK re-inlines the same 45 lines (`gtk/shell_runner.rs:16-59`, same comments, same magic numbers); macOS/Win have none — `ShellApp` apps simply cannot run there (#465).
- Per-backend event constructors are the same 5-line bodies (`gtk/events.rs:61-107` vs `macos/events.rs:83-120`); only keysym tables and button-number mapping are genuinely native. TUI's `DoubleClickDetector` (`tui/events.rs:140-183`) is backend-agnostic and belongs in shared dispatch.
*Fix:* extract a `runtime` core: event-constructor helpers, double-click synthesis, accelerator matcher, unified pre-processing pipeline, debounce, shared shell-adapter builder. Each backend's `run` becomes: pump native events → translate → `runtime::process(...)`. L.

**PORT-10 · M · Generic desktop-windowing logic is trapped in `gtk/`** (Size M)
CSD titlebar drag arm/threshold/commit state machine (`gtk/backend.rs:206-224,345-404` + `gtk/run.rs:560-583`), nested-modal `pump_depth` re-entrancy guard checked in 9 closures (`gtk/run.rs:254-265,325…778`), env-driven headless smoke harness (`gtk/run.rs:128-171,821-878` — the predicates and constants are pure), `PointerShape`→cursor mapping (`gtk/backend.rs:1027-1039`). macOS re-implemented none of these and never overrides `begin_window_drag`/`toggle_window_maximize`/`begin_window_resize`/`set_cursor` — CSD apps are broken on macOS; Windows would need all of it (Win32 file dialogs have the same modal re-entrancy hazard). Good precedent already exists: `ShellContext::window_edge` (`shell.rs:330-376`) is correctly shared.
*Fix:* `desktop/` shared module for the interaction state machines + smoke predicates; backends supply only the native calls. M.

**PORT-11 · M · Unit and numeric-type leaks in the portable API** (Size M)
- `EditorPaintResult.cursor_position: Option<(u16,u16)>` (`backend.rs:937`) — terminal cell coords smuggled through the trait; pixel backends must return `None` or lie.
- `TabBarHits` (`primitives/tab_bar.rs:369-389`): pixels-as-`f64` pairs + `available_cols: usize` in **character columns**, consumed against `f32` `Point`s; kept alive by the "legacy" converter `tab_bar_layout_to_hits` (`backend.rs:872-896`).
- `ActivityBarRowHit{y_start,y_end: f64}` beside all-f32 `ActivityBarLayout::hit_test` (`primitives/activity_bar.rs:138-158`).
- `dispatch.rs:455-486 text_selection_line_range → Vec<(u16,u16,u16)>` — "columns" that are truncated pixels on GTK/macOS; `TextRegion.lines` doc'd as "for pixel backends; TUI ignores this" (`dispatch.rs:428-435`) — a cfg-by-convention field.
- 57 `u16` cell-unit occurrences across 12 primitive files: `FR_PANEL_WIDTH: u16 = 50` (`primitives/find_replace.rs:68`), `StatusBar` cols (`status_bar.rs:92-148`), MSV `Fixed(u16)`/min/max (`multi_section_view.rs:86,225-229`), `SplitTree::cell_position` (`split_tree.rs:449-451` — file :44 documents a shipped rounding bug from exactly this).
- Coordinate-frame contract contradicts itself inside the trait: `draw_status_bar` returns **bar-local** (`backend.rs:473-474`) while `tab_bar_layout` returns **absolute** (`backend.rs:511-513`); LESSONS.md:159-181 records a shipped macOS bug from this ambiguity.
*Fix:* one story per cluster — f32 normalization + retire `TabBarHits` (feeds #456's convergence), cursor_position → `Point`, a documented single coordinate-frame convention with non-zero-origin tests (see TEST-05). M.

**PORT-12 · M · DPI / scaling / colour model unimplemented at runtime** (Size M)
`Viewport.scale` exists but: `DpiChanged` is emitted by no backend; GTK reads `scale_factor()` exactly once for smoke-mode (`gtk/run.rs:743`); no per-monitor DPI story (Windows per-monitor-v2 will change scale at runtime, macOS Retina backing scale differs per screen); no colour-space notion (`Color` is untagged 8-bit sRGB-by-convention, `types.rs:16-22`); no high-contrast/accessibility hook in `Theme` (a flat per-primitive colour struct, `theme.rs:34+`). Fractional scaling on Wayland/Windows will break any layout that rounds in logical units without a quantum — only TUI has `cell_quantum`.
*Fix:* define the DPI contract (who emits `DpiChanged`, what units `Point`/`Rect` are in when scale ≠ 1.0, rounding policy per backend) before the Windows port starts; document colour space as sRGB explicitly. M.

**PORT-13 · M · Scroll semantics under-specified** (Size S, mostly covered by #418)
`ScrollDelta` (`event.rs:139-149`) has no unit discriminator (lines vs pixels vs precise-touchpad ticks) — "backends normalise to their native unit" pushes per-backend interpretation onto every consumer; natural-scroll inversion is unmodelled (#418 already filed for the GTK piece). Windows adds WM_MOUSEWHEEL line-delta vs WM_POINTER precise deltas. Fold the unit/precision design into #418's resolution rather than a new issue.

**PORT-14 · M · Naming/structure drift between backends raises the copy cost** (Size M)
`tui_*`/`gtk_*`/`mac_*` helper prefixes (`mac_` mismatches both module `macos` and the feature name); three names for MSV metrics (anonymous inline in TUI, `metrics_for` in GTK re-exported as `multi_section_view_metrics` at `gtk/mod.rs:93`, `mac_msv_metrics` on macOS); public TUI layout helpers take `ratatui::layout::Rect` (`tui/tree.rs:11,251`, `tui/form.rs:20`, `tui/multi_section_view.rs:118`) while GTK/macOS take `quadraui::Rect`; GTK re-exports `pango::Layout` and a `&gtk4::DrawingArea` API from a public module (`gtk/mod.rs:147,85`); `tui/mod.rs` keeps rasterisers private-with-reexports vs `macos/mod.rs` all-`pub mod`; terminal entry points named `draw_terminal` vs `draw_terminal_cells` ×2; `mac_list_layout` carries two unused params for GTK shape-parity (`macos/list.rs:34-41`). Each inconsistency is a decision a Windows author must re-litigate.
*Fix:* a naming/signature normalization pass with a written convention in PRIMITIVE_RULES.md. M.

**PORT-15 · M · Example pairing is three-way ragged; per-example macOS/Win rewrites loom** (Size report-only)
43 `tui_*` / 40 `gtk_*` / 16 `macos_*` / 0 `win_*`; no macOS example for board, diff_view, pipeline_view, split_tree, terminal, text_input, toolbar, tab_bar, sidebar_panel; no example anywhere for command_line, command_center, completions, tooltip, drop_zone, modal. Because runners are per-backend 10-liners the marginal cost is low **iff** the runner exists (macOS lacks `run_with_shell`, blocking all shell-based examples — #465). Track as acceptance criteria inside Epics 1–2 rather than as separate issues.

**PORT-16 · M · PlatformServices stubs are silent and doc-contradicting** (Size S)
TUI `show_file_open_dialog`/`show_file_save_dialog` return bare `None` (`tui/services.rs:393-399`) while the trait doc promises "writes a hint to stderr" (`backend.rs:983-985`); `GtkServices::send_notification` is `{}` (`gtk/services.rs:120`); Win services abort (PORT-02). A capability-report mechanism (PORT-06 fix) plus doc truth pass (DOC-05) covers this.

### 3.2 Code smells (SM-xx)

---

**SM-01 · H · UTF-8 byte-slicing panic cluster in GUI rasterisers; the fix exists privately ×7** (Size M)
Reachable-from-user-text panics — any multibyte char left of a caret aborts the paint pass:
`gtk/palette.rs:164` + `macos/palette.rs:146` (`&palette.query[..query_cursor]` — byte offset owned by host, doc'd at `primitives/palette.rs:69-72`); `gtk/tree.rs:311-312,337` (inline rename); `gtk/form.rs:167-168,217,476`; `macos/form.rs:290`; `gtk/find_replace.rs:145,150,166` + `macos/find_replace.rs:130`; `gtk/command_line.rs:46` (`:éditer` panics GTK); `macos/editor.rs:130-131,155`; `gtk/editor.rs:659` (unguarded error-path clamp).
Meanwhile the crate contains **seven private copies** of the boundary-snapping fix and zero public ones: `tui/editor.rs:519-525`, byte-identical triples in `compose/chat_controller.rs:1295-1323` and `compose/tree_controller.rs:874-902`. Related: `Color::from_hex` (`types.rs:36-49`) slices bytes after a `len()` check and **panics on `"#€abc"`** contra its own "returns None on malformed input" doc — theme/config parsing on every backend.
*Fix:* one public text-util module (snap/prev/next boundary + safe slice), migrate the 12+ panic sites and 7 private copies, fix `from_hex`. Complements (does not duplicate) #472's consumer-facing safe-truncate API. M.

**SM-02 · H · Display-width measurement is wrong in three different ways** (mostly covered: #471, #472; residual Size S)
`StyledText::visible_width` counts chars (`types.rs:137-139`) — 17 layout call sites consume it, including three in `compose/sidebar_system.rs:1441-1487` that multiply by `char_w` into **pixel** rects (#471 filed). 71 raw `.chars().count()`-as-width sites across 37 files, densest in shared `primitives/status_bar.rs` (7 sites — every backend inherits the error) and `tui/form.rs` (13). The only correct UAX#11 measurer is private `tui/mod.rs:159 cell_width` (#472 filed); `tui/menu_bar.rs:101-103` re-rolls a third, wrong variant. Structural cause worth adding to #471/#472's context: **`unicode-width` is an optional dep of the `tui`/`gtk` features only** (`Cargo.toml:18,23`), so `primitives/`/`types.rs` *cannot* use it today — the fix requires promoting it to a non-optional dep (it's tiny). Report-only here; work belongs on #471/#472.

**SM-03 · M · God files with mechanical splits available** (Size M)
- `lib.rs` 4,395 lines = ~320-line facade + **4,075-line test dumping ground** (157 tests for primitives that each have their own file's test module; banner-sectioned at `lib.rs:830-3409`). Move tests to their primitive files; consider a `prelude`.
- `gtk/backend.rs` 3,608: window handle + font cache + selection engine + accelerator table + ~90 shims + ~1,200 lines of inlined layout (`form_layout` at `:1900-1960`) that macOS keeps in module files — extract to `gtk/*.rs` free fns for symmetry (`mac_form_layout` pattern).
- `compose/app_shell.rs:589-922` — a single 333-line `handle()`; extract per-zone handlers. Siblings `sidebar_system.rs` (3,275) and `tab_group.rs` (3,130) share the shape.
- `dispatch.rs` (2,206) is cohesive but bundles drag/mouse/selection/scroll — mechanical split into `dispatch/{drag,mouse,selection,scroll}.rs`; `terminal_engine.rs` (3,016) should shed pure `xterm_256_color`/`encode_mouse_sgr` into submodules.

**SM-04 · M · Dead / zero-in-repo-consumer public API** (Size M — **prune only after checking out-of-repo consumers** vimcode + coord-tui; in-repo grep cannot see them)
- `primitives/modal.rs` (129 lines): `Modal`/`ModalEvent`/`ModalHit`/`ModalLayout` referenced only by `lib.rs` re-export + its own tests; no backend `draw_modal`; no example. Distinct from (used) `ModalStack`.
- 13 of 27 `pub enum *Event` types referenced only in their own file (`CompletionsEvent`, `ContextMenuEvent`, `DialogEvent`, `DiffViewEvent`, `ModalEvent`, `PanelEvent`, `ProgressBarEvent`, `RichTextPopupEvent`, `SidebarPanelEvent`, `SpinnerEvent`, `SplitEvent`, `ToastEvent`, `ToolbarEvent`, `TooltipEvent`); 9 more ride `UiEvent` but no workspace consumer matches on them.
- `primitives/drop_zone.rs`: everything except `DropOverlay` has one internal consumer (`compose/tab_group.rs`), zero external.
- `compose/focus_group.rs` vs `compose/focus_ring.rs`: two wrap-around Tab-cycling implementations; `FocusGroup`'s own module doc admits the overlap; zero external consumers for `FocusGroup`.
- `primitives/board.rs`: confirmed #476 (already filed — `TestVerdict`, `CardLayout`, `ColumnLayout`, `CardId` at 0 consumers; `board_layout` is a free fn contra every sibling).
- Overall: ~215 of 485 `lib.rs` re-exports have zero consumers in-repo. Theme is fine (76 fields, only 1 under-consumed).

**SM-05 · M · Guard-by-distance unwraps in input-path state machines** (Size S)
`compose/tree_controller.rs` — 13× `self.editing.take()/as_mut().unwrap()` in the inline-edit key dispatcher (`:469-636`); `compose/menu_system.rs:189,209` unwraps `open_item` in an arm that mutates the invariant on the next line; `tui/backend.rs:574` unwrap inside `wait_events` hot path (currently guarded, one refactor from live); `tui/mod.rs:119-120,136-137` `set_cell`/`set_cell_wide` check the right edge but **not `x >= area.x`** — ratatui indexing panics for any sub-rect painter that under-runs left, and every TUI primitive funnels through these; `macos/chart.rs:209-210` unwraps first/last on possibly-empty series. All fixable with `let-else`/saturating guards. S.

**SM-06 · L · Scroll-offset clamp math re-derived in 14 primitive files** (Size S)
`(body_h / step).floor() as usize` + `.min(len)` pairs in board/text_display/tab_bar/terminal/list/data_table/text_input/tree/diff_view/palette/msv/completions/form/rich_text_popup; `fit_thumb` exists for thumbs but nothing for offsets/windows. One `visible_window(offset, content, viewport)` helper ends the drift class. S.

**SM-07 · L · Palette ships no filtering/scoring; the only fuzzy scorer is private** (covered by #474)
`compose/folder_picker.rs:672 dir_fuzzy_score` is private; `Palette` (the command palette!) has no matcher, so every consumer re-rolls one. #474 already filed — noted here only because promoting `dir_fuzzy_score` is the concrete starting point.

### 3.3 Test-suite findings (TEST-xx)

---

**TEST-01 · H · GtkDriver's observable surface is fed by 2 of 37 rasterisers — the structural blocker on all GTK driver tests** (Size M)
`GtkDriver::find`/`screen_contains` read `GtkBackend::painted_text` (`gtk/backend.rs:170`), populated only by `draw_status_bar` (`:1481`) and `draw_pipeline_view` (`:2350-2352`); acknowledged as incremental at `:167-169`. Until `record_painted_text` is adopted across rasterisers, the GTK example-driver suite cannot grow past PipelineApp regardless of test-writing effort — this, not authoring, blocks the #305–#309 GTK twins. Feeds directly into the conformance suite's text-inventory contract (§6).

**TEST-02 · H · The stated acceptance bar is 53% unmet, and nothing machine-checks it** (backfill covered by #305–#309; gate by #311)
23 of 43 `tui_*` examples have no driver test (CLAUDE.md: "Every TUI example also ships an automated black-box test"). #305–#309 are 9/25 done (#308: 0/5). 39/40 `gtk_*`, 16/16 `macos_*`, 2/2 `msv_*` uncovered. Enforcement is "the adversarial reviewer" — #311 (CI gate) is the real fix. Report-only; do not re-file.

**TEST-03 · H · macOS: best-in-repo Tier-2 harness (21 hit round-trips, 29 headless modules), zero execution anywhere, no driver** (Size M/L)
193 tests in `src/macos` never compile in CI (PORT-03); no `MacDriver` exists (`macos/headless.rs` is a surface, not a driver — no AppLogic, no dispatch, no find()). The strongest evidence that the *culture* is right and the *loop* is disconnected.

**TEST-04 · M · Hardcoded coordinates in driver tests, contra two docs** (Size S)
`tests/tui_example_driver.rs:1514 click(99.0,15.0)`, `:1533 click(99.0,29.0)`, `:1583/:1601 Point::new(99.0/50.0,15.0)`, plus the `:1690` 20-driver y-sweep. GTK/parity suites are clean. Fix by deriving from `find()`/viewport, or add the missing semantic locators. S.

**TEST-05 · M · LESSONS.md's non-zero-origin rule is honoured in ~5 of ~56 layout-test modules** (Size M)
TUI: 27/29 modules use origin-(0,0) rects exclusively (form 37/0, list 37/0, MSV 37/0…). macOS: 22/27 origin-zero-only — including `mac_list_layout`, `mac_palette_layout`, `mac_panel_layout`… the exact bug class LESSONS.md:159-181 documents as shipped-before. A conformance-suite structural fix (scenarios run under a translated root) beats hand-adding 50 tests; do both cheaply: parametrize existing harness helpers over an origin.

**TEST-06 · M · Per-primitive round-trip asymmetry: TUI 20 / macOS 21 / GTK 2** (Size L, prioritize)
Only `gtk/multi_section_view.rs` and `gtk/tree.rs` have genuine hit round-trips; **29 of 37 GTK rasterisers have zero tests**. Six primitives' hit-testing is verified only by never-running macOS tests (`activity_bar`, `completions`, `context_menu`, `list`, `palette`, `tab_bar`); four primitives have hit_test but no round-trip on any backend (`editor`, `terminal`, `tooltip`, `rich_text_popup`). GTK is the only CI-tested GUI backend and has the thinnest harness.

**TEST-07 · M · Cross-backend parity = 1 scenario; `ExampleDriver` trait lacks `drag_text`; driver APIs diverge** (Size M)
`tests/cross_backend_parity.rs`: one test, 5-method local trait; TESTING.md's own sketch includes `drag_text` — dropped in realization, so no drag scenario can be shared. `TuiDriver::new(u16,u16)` vs `GtkDriver::new(i32,i32)`; `screen()` `String` vs `Vec<u8>`; GtkDriver lacks `app_mut()`. All resolved by the ConformanceDriver promotion (§6).

**TEST-08 · L · "Verify by mutation" is policy without tooling; demo crates have zero tests** (report-only)
TESTING.md:45-48 mandates mutation-verification; nothing enforces or records it. kubeui/kubeui-core/kubeui-gtk: 0 tests.

### 3.4 Documentation drift (DOC-xx) — all S, batch into one truth-pass story

- **DOC-01** README:21 "The macOS backend is feature-complete" — false (PORT-01).
- **DOC-02** Cargo.toml `win`/`macos` feature comments promise compile-checks that fail/never run (PORT-02/03).
- **DOC-03** `docs/ARCHITECTURE.md:39-42` still describes the Backend trait as "for cross-cutting state" with draw_* as free functions — two generations stale.
- **DOC-04** `graphify-out/` contains only `.gitignore`; CLAUDE.md instructs agents to "query the graph first" against `graph.json`/`GRAPH_REPORT.md` that don't exist (hooks not regenerating).
- **DOC-05** `backend.rs:983-985` claims TUI file dialogs "write a hint to stderr" — they return bare `None` (`tui/services.rs:393-399`).
- **DOC-06** #403 (clippy fails on develop): not reproducible — full gate green on 73172ff. Recommend human verifies and closes.
- **DOC-07** `event.rs:241-243` `CharTyped` "IME-composed" describes a nonexistent pipeline (PORT-05).

---

## 4. Primitive coverage matrix

Trait totals: **37 `draw_*`**, **21 `*_layout`**, plus `msv_metrics`, `list_h/vscrollbar`, `editor_col_at_x` (defaulted). Three layout conventions coexist: *paired draw+layout*, *draw-returns-layout*, *draw-takes-precomputed-layout* — a fourth backend must learn all three.

| Primitive | descriptor (struct/layout/hit_test) | trait draw | trait layout | TUI | GTK | macOS | win | notes |
|---|---|---|---|---|---|---|---|---|
| activity_bar | ✅/✅/✅ | ✅ | ✅ | ✅ | ✅ | draw only — **`activity_bar_layout` missing** | draw only | |
| board | ✅/free-fn/✅ | ✅ | ❌ no `board_layout` | ✅ | ✅ | ❌ **no file, no impl** | ❌ | layout is a free fn contra siblings |
| chart | ✅/✅/✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ **both missing** | |
| command_center | ✅/✅/✅ | ✅ | ✅ | ✅ | ✅ | ✅ | stub | no example anywhere |
| command_line | struct only | ✅ | ❌ | ✅ | ✅ | ❌ **no file, no impl** | ❌ missing | |
| completions | ✅/✅/✅ | ✅ (takes layout) | — | ✅ | ✅ | ✅ | stub | no example |
| context_menu | ✅/✅/✅ | ✅ (takes layout) | — | ✅ (+`_with_submenus` **off-trait, TUI-only**) | ✅ | ✅ | stub | |
| data_table | ✅/✅/✅ | ✅ | ✅ | ✅ | ✅ | ✅ | stub (E0050 sig drift) | |
| dialog | ✅/✅/✅ | ✅ (takes layout) | — | ✅ (+`tui_dialog_layout` helper others lack) | ✅ | ✅ | stub | |
| diff_view | ✅/❌/❌ | ✅ | ❌ | ✅ | ✅ | **fake** (`visible_rows:0`, paints nothing) | stub | |
| drop_zone/overlay | ✅/free-fns/❌ | ✅ | ❌ | ✅ | ✅ | **empty TODO body** | stub | |
| editor | ✅/✅/✅ | ✅ | ❌ (`editor_col_at_x` default) | ✅ | ✅ (+override) | ✅ | stub | no `editor_layout` on trait |
| find_replace | ✅/❌/free-fn | ✅ | ❌ | ✅ | ✅ | ✅ | stub | hit regions = free fn |
| form | ✅/✅/✅ | ✅ | ✅ | ✅ (+`draw_settings_chrome` **off-trait**) | ✅ (+same) | ✅ | stub | |
| list | ✅/✅/✅ | ✅ | ❌ no `list_layout` (only h/v scrollbar) | ✅ | ✅ | ✅ `mac_list_layout` **off-trait** | stub | |
| menu_bar | ✅/✅/✅ | ✅ | ✅ | ✅ | ✅ (+`MenuOverlay` GTK-only) | ✅ (+native install) | stub | |
| message_list | struct only | ✅ | ❌ | ✅ | ✅ | ✅ | stub | no layout/hit_test at all |
| modal | ✅/✅/✅ | ❌ **none** | ❌ | ❌ | ❌ | ❌ | ❌ | fully dead (SM-04) |
| multi_section_view | ✅/✅/✅ | ✅ | ✅ +metrics | ✅ | ✅ | ✅ (body_measure drifted) | stub | |
| palette | ✅/✅/✅ | ✅ | ❌ no `palette_layout` | ✅ | ✅ | ✅ `mac_palette_layout` **off-trait** | stub | |
| panel | ✅/✅/✅ | ✅ | ✅ | ✅ | ✅ | ✅ | stub | |
| pipeline_view | ✅/✅/✅ | ✅ | ✅ | ✅ | ✅ | ✅ | stub | |
| progress / spinner | ✅/✅/✅ | ✅ | ✅ | ✅ | ✅ | ✅ | stub | |
| rich_text_popup | ✅/✅/✅ | ✅ (takes layout) | — | ✅ | ✅ | ✅ | stub | |
| scrollbar | struct only (free `fit_thumb`) | ✅ | ❌ | ✅ | ✅ | ✅ | stub | |
| sidebar_panel | ✅/✅/✅ | ✅ | ✅ | ✅ | ✅ | ✅ | stub | |
| split | ✅/✅/✅ | ✅ | ✅ | ✅ | ✅ | ✅ | stub | |
| split_tree | ✅/✅/✅ | ✅ | ✅ | ✅ | ✅ | ❌ **no file, no impl** | stub | |
| status_bar | ✅/✅/✅ | ✅ | ✅ | ✅ | ✅ | draw only — **layout missing** | draw only | |
| tab_bar | ✅/✅/✅ | ✅ | ✅ | ✅ | ✅ | draw only — **layout missing** | draw only | + legacy `TabBarHits` dual API |
| terminal | ✅/✅/✅×2 | ✅ | ❌ | ✅ (+`draw_terminal_divider` **off-trait ×3**) | ✅ | ✅ (no wide-char fix) | stub | |
| text_display | ✅/✅/✅ | ✅ | ✅ | ✅ | ✅ | ✅ | stub | |
| text_input | ✅/✅/hit-regions field | ✅ | ✅ | ✅ | ✅ | ✅ | stub | |
| toast | ✅/✅/✅ | ✅ | ✅ | ✅ | ✅ | ✅ | stub | |
| toolbar | ✅/✅/✅ | ✅ | ✅ | ✅ | ✅ | ✅ | stub | |
| tooltip | ✅/✅/✅ | ✅ (takes layout) | — | ✅ | ✅ | ✅ | stub | no example anywhere |
| tree | ✅/✅/✅ | ✅ | ✅ | ✅ | ✅ | ✅ | stub | |

Driver-test column (Tier-4): see TEST-02 — 20/47 common shapes covered on TUI, 1 on GTK, 0 on macOS.

**CLAUDE.md-rule violations ("rasterisers but no trait method"):** `draw_terminal_divider` (×3 backends), `draw_settings_chrome` (×2), `draw_context_menu_with_submenus` (TUI only), `mac_palette_layout`, `mac_list_layout`, `tui_dialog_layout`, `tui_board_layout`/`gtk_board_layout` free helpers, `gtk::MenuOverlay`.

**`frame.rs` drift (quantifies #456):** the `Surface`/`FrameZone` second painting API covers 25 primitives — missing Board, DiffView, PipelineView, Toolbar, SidebarPanel, TextInput, Spinner, Progress, CommandCenter, DropOverlay, MessageList — and every new primitive needs 4-place extension (`Surface`, `FrameZone`, `zone_for`, `draw` match) that nothing forces in the same PR as the trait method.

---

## 5. The minimal-native seam — what a new backend must supply vs inherit

**Must supply (irreducibly native, ~15 concerns):**
1. Window/surface bootstrap + frame scope (begin/end draw, present)
2. Native event pump + keysym/keycode → `Key` table + button-number map (the only genuinely native ~40 lines of `events.rs`)
3. Text measurement (`line_height`, `char_width`, run-width, x→byte-index for attributed text)
4. Draw ops: fill/stroke rect, line, glyph-run, clip, (image later)
5. Platform services: clipboard, file dialogs, notification, open-url
6. DPI/backing-scale query + change notification
7. Native menu / native context-menu installers (optional capability)
8. Headless render target for tests (`TestBackend` / `ImageSurface` / `BitmapSurface` / `ID2D1Bitmap`)

**Should inherit free (today duplicated or absent — the gap):** `UiEvent` constructors; double-click synthesis; accelerator matching; the dispatch_event pre-processing pipeline; resize debounce; shell-adapter/`run_with_shell` construction; CSD drag/resize/maximize state machine; modal re-entrancy guard; smoke-mode predicates; **all rasteriser layout math and paint plans** (via the pixel-metrics layer, PORT-08); scenario-driven conformance tests + driver skeleton (§6); text-selection engine (currently ~270 lines in `gtk/backend.rs:613-885`, absent on macOS).

Long-term shape: `Backend` splits conceptually into a small `NativeSurface` trait (the 15 concerns) + a shared raster core implementing the 60+ `draw_*`/`*_layout` methods generically. Near-term: extract the shared modules without changing the trait, so TUI/GTK/macOS converge file-by-file and the Windows port consumes the shared 65% from day one.

---

## 6. Section C — Backend-agnostic conformance suite (design)

### 6.1 What exists and is kept
`TuiDriver` + `GtkDriver` + the `ExampleDriver` trait already realized in `tests/cross_backend_parity.rs` are the right primitives — this design **extends** them; nothing is replaced. `examples/common/*.rs` (47 backend-neutral `AppLogic` shapes) are the fixture library. #322's web id→rect map is the same observable-surface concept; the contract below is written so `WebBackend` can implement it unchanged.

### 6.2 The observable surface (portable equivalent of `find()`/`screen_contains()`)
Every backend exposes, per rendered frame, a **semantic paint inventory** — recorded at draw time by the same code that paints (never re-derived):

```rust
/// quadraui::testing (feature-independent core)
pub struct TextRun  { pub text: String, pub bounds: Rect }      // what text was painted, where
pub struct ZoneRec  { pub id: WidgetId,  pub bounds: Rect }     // which widget zones were registered
pub trait FrameInventory {
    fn text_runs(&self) -> &[TextRun];
    fn zones(&self) -> &[ZoneRec];       // from FrameHitMap / layout returns
}
```
- TUI synthesizes `TextRun`s from the cell grid (existing `screen()` scan, wide-char aware).
- GTK: `record_painted_text` generalized to **all** rasterisers (TEST-01 is the prerequisite).
- macOS: record at each Core Text line draw (`macos/text.rs::draw_text` is the single choke point).
- Web (#322): the id→rect map *is* `zones()`; DOM text *is* `text_runs()`.
- Windows: record at the DirectWrite draw call.

Assertions are **relational only** — no numeric coordinates can appear in a scenario because the schema has no coordinate field:
`screen_has(t)`, `absent(t)`, `click(target)`, `count(t) == n`, and geometry predicates computed on returned rects: `left_of(a,b)`, `above(a,b)`, `inside(a, zone_id)`, `same_row(a,b)`. Hardcoded coordinates become *structurally impossible*, not discouraged.

### 6.3 Driver trait (promotion of `ExampleDriver`)
```rust
/// quadraui::testing::ConformanceDriver — each backend implements behind its feature flag
pub trait ConformanceDriver {
    fn new_fixture(fixture: FixtureId, viewport: LogicalViewport) -> Self;
    fn press_named(&mut self, key: NamedKey);
    fn type_char(&mut self, c: char);
    fn type_text(&mut self, s: &str);
    fn click_text(&mut self, needle: &str);          // find → click centre, native coords
    fn click_text_at(&mut self, needle: &str, at: Anchor); // Center|LeftEdge|RightEdge
    fn drag_text(&mut self, from: &str, to: &str);   // restores TESTING.md's dropped method
    fn scroll_at(&mut self, needle: &str, lines: i32); // delta in line_height multiples
    fn inventory(&self) -> &dyn FrameInventory;
    fn screen_has(&self, needle: &str) -> bool;
    fn exited(&self) -> bool;
}
```
`LogicalViewport { cols: u32, rows: u32 }` — interpreted as cells on TUI and `cols × char_width` / `rows × line_height` device units on pixel backends. All sizes in scenarios are expressed in `lh`/`ch` multiples; the LESSONS.md unit rules become the schema's only vocabulary. Aligning `TuiDriver`/`GtkDriver` signatures (u16 vs i32, `app_mut`, `screen()` types — TEST-07) is part of the promotion.

### 6.4 Scenario files
Location: `quadraui/tests/conformance/scenarios/<area>/<name>.scn.json` (JSON: serde is already a core dep; no new deps). Runner: `quadraui/tests/conformance.rs` iterates scenarios × registered drivers; each backend registers in one line behind its feature gate. Fixtures resolve by name to `examples/common` constructors via a small registry (`quadraui/tests/conformance/fixtures.rs`).

Schema v0 (every step is one key; unknown keys = error; **no numeric coordinate fields exist**):
```json
{
  "id": "string, stable, dotted",
  "fixture": "name from registry",
  "tier": 1,
  "viewport": { "cols": 100, "rows": 30 },
  "requires": ["text_selection"],          // optional capability gates (PORT-06)
  "steps": [ /* ordered step objects, see worked examples */ ]
}
```

### 6.5 Worked example scenarios

**(1) Click routing round-trip** — `scenarios/interaction/pipeline.click_advances_stage.scn.json` (Tier 1; ports the existing parity test):
```json
{
  "id": "pipeline.click_advances_stage",
  "fixture": "pipeline_app",
  "tier": 1,
  "viewport": { "cols": 100, "rows": 30 },
  "steps": [
    { "assert_screen_has": "stage 1" },
    { "press": "Right" },
    { "click_text": "Go" },
    { "assert_screen_has": "stage 3" },
    { "type_char": "q" },
    { "assert_exited": true }
  ]
}
```

**(2) Modal occlusion (ModalStack conformance — also the #455 bug class)** — `scenarios/modal/dialog.blocks_click_through.scn.json` (Tier 1):
```json
{
  "id": "dialog.blocks_click_through",
  "fixture": "dialog_table_demo",
  "tier": 1,
  "viewport": { "cols": 100, "rows": 30 },
  "steps": [
    { "click_text": "Open dialog" },
    { "assert_screen_has": "Confirm" },
    { "note": "Row 'zeta-pod' is visible behind the dialog; clicking its text must NOT select it while the dialog is up" },
    { "click_text": "zeta-pod" },
    { "assert_absent": "selected: zeta-pod" },
    { "click_text": "Cancel" },
    { "click_text": "zeta-pod" },
    { "assert_screen_has": "selected: zeta-pod" }
  ]
}
```

**(3) Text selection drag → copy (exercises `register_text_region`, the drag pipeline, and `TextCopied`)** — `scenarios/selection/panel.drag_select_copy.scn.json` (Tier 1, `requires: ["text_selection"]`):
```json
{
  "id": "panel.drag_select_copy",
  "fixture": "panel_app",
  "tier": 1,
  "viewport": { "cols": 100, "rows": 30 },
  "requires": ["text_selection"],
  "steps": [
    { "assert_screen_has": "alpha beta gamma" },
    { "drag_text": { "from": "alpha", "to": "gamma" } },
    { "ctrl_char": "c" },
    { "assert_screen_has": "Copied" }
  ]
}
```

**(4) Relational geometry (unit-free layout assertion)** — `scenarios/layout/shell.sidebar_left_of_main.scn.json` (Tier 1):
```json
{
  "id": "shell.sidebar_left_of_main",
  "fixture": "shell_app",
  "tier": 1,
  "viewport": { "cols": 120, "rows": 40 },
  "steps": [
    { "assert_left_of": { "a": "EXPLORER", "b": "main content" } },
    { "assert_above":   { "a": "EXPLORER", "b": "status: ready" } },
    { "press": "F6" },
    { "assert_absent": "EXPLORER" }
  ]
}
```

### 6.6 Conformance tiers (the burn-down checklist for a new backend)
- **C0 — Boot (day one):** construct backend headless; `begin_frame`/`end_frame`; viewport sane; draw **every** primitive once with a canned descriptor — no panic AND non-empty `text_runs()` for text-bearing primitives. *This converts the 13 silent no-op defaults (PORT-06) into visible red/green on day one.* Auto-generated per primitive from the trait surface.
- **C1 — Interaction core (mandatory for "complete"):** the ~15 Tier-1 scenarios: click routing, keyboard focus/Tab, modal occlusion, scroll-under-cursor, split drag, text-selection drag+copy, tab close, menu open/navigate/activate, palette open/type/pick, toast dismiss, editor click-to-caret (via `editor_col_at_x`).
- **C2 — Event parity:** for each required `UiEvent` variant, a per-backend native-injection recipe proves it is emitted (fixes PORT-04's matrix and keeps it fixed). Required set: Key/Char/MouseDown/Up/Moved/Scroll/DoubleClick/WindowResized/Accelerator/ClipboardPaste/TextCopied. Optional set (declare, don't fake): FilesDropped, MouseEntered/Left, DpiChanged, native menu events.
- **C3 — Platform services:** clipboard round-trip (headless where possible), capability-honest dialogs/notifications.
- **C4 — Native residue (never shared):** exact colours, font rendering, wide-glyph pixels, live-window smoke (GD-5 stays as-is).

CI emits a scenario × backend matrix artifact (pass/fail/skip-with-reason); "skip" requires a declared missing capability — silence is impossible. This artifact **is** the Windows/macOS implementation checklist.

### 6.7 Relationship to existing tiers
TESTING.md Tiers 1–3 unchanged (round-trip harnesses stay the Tier-2 gate; pty smoke #302 unchanged). The conformance suite is Tier-4 generalized: `tests/tui_example_driver.rs` bodies migrate into scenarios opportunistically (where a body is pure locate/act/assert); complex imperative bodies remain as shared generic fns per the existing `cross_backend_parity.rs` pattern. `GtkDriver`/GD-5 division of labor is unchanged.

---

## 7. Proposed epic breakdown

*(Labels drawn only from the existing set. Ordering within epics is dependency order.)*

### EPIC A — Compile truth: every declared backend builds, and CI proves it forever
Labels: `epic`, `bug` · Goal: `cargo check` green for tui/gtk/win/macos(+terminal) on every push to develop; docs stop overstating.
1. **A1** CI feature-matrix + develop trigger (win check, terminal, macOS strategy) — `bug`,`harness` (unblocks everything; do first)
2. **A2** MacBackend: implement the 7 missing trait methods + missing rasterisers (board, command_line, split_tree) + fix degenerate diff_view/drop_overlay — `bug`,`backend:macos`
3. **A3** WinBackend: restore trait conformance; services `todo!()` → graceful stubs — `bug`,`backend:win-gui`
4. **A4** macOS runtime event parity: DoubleClick, WindowResized wiring, accelerator matching, ClipboardPaste — `bug`,`backend:macos`
5. **A5** Docs truth pass (DOC-01..07) — `documentation`
Deps: A2–A4 before A1 can turn the new jobs red→green (or land A1 with allow-fail first). Acceptance: the §2 table all-green including win/macos rows.

### EPIC B — Conformance suite: scenario-driven black-box tests any backend can run
Labels: `epic`, `enhancement`, `harness` · Goal: §6 shipped; a new backend gets a burn-down matrix on day one.
1. **B1** Promote `ConformanceDriver` + align TuiDriver/GtkDriver APIs (+`drag_text`, `app_mut`) — `harness`
2. **B2** GtkBackend painted-text instrumentation across all rasterisers — `harness`,`backend:gtk` (unblocks GTK halves of #305–#309)
3. **B3** `FrameInventory` (text_runs + zones) as the cross-backend observable contract, aligned with #322 — `harness`,`enhancement`
4. **B4** Scenario schema + runner + first 10 Tier-1 scenarios — `harness`
5. **B5** Tier C0 auto-generated per-primitive paint smoke (kills silent no-op defaults) — `harness`
6. **B6** MacDriver over `BitmapSurface` + registration — `harness`,`backend:macos`
7. **B7** Test-debt cleanup: 5 hardcoded-coordinate sites; origin-parametrized harness helpers (LESSONS rule) — `harness`
Deps: B1→B4; B2→B4(gtk); B3 before B5; B6 after A2.

### EPIC C — Shared runtime core: one implementation of the duplicated 65%
Labels: `epic`, `enhancement` · Goal: a 5th backend writes only the §5 "must supply" list.
1. **C1** Shared event-constructor helpers + `DoubleClickDetector` into dispatch core; 3 backends adopt — `enhancement`
2. **C2** Unified runner pipeline (EventOutcome, pre-processing, apply_reaction, resize debounce); macOS adopts — `enhancement`
3. **C3** Backend-agnostic shell-runner core (generalize `build_shell_adapter`; GTK adopts; unblocks #465) — `enhancement`
4. **C4** `desktop/` module: CSD drag machine, pump-depth guard, smoke predicates, PointerShape mapping — `enhancement`,`backend:gtk`,`backend:macos`
5. **C5** Shared pixel-metrics layer (tree/MSV/form); fix macOS MessageList measure drift — `enhancement`,`primitive`
6. **C6** Terminal shared overlay ladder + wide-char width via one helper (executes #440/#441/#442's shared half) — `enhancement`,`primitive`
7. **C7** UiEvent emission matrix: required/optional sets; emit-or-remove dead variants — `enhancement`
8. **C8** IME/composition design: UiEvent preedit contract + GTK IMContext + macOS NSTextInputClient (supersedes the input half of #415's scope) — `enhancement`,`backend:gtk`,`backend:macos`
Deps: C1→C2→C3; C5 before any Windows rasteriser work; C7 feeds B4 Tier C2.

### EPIC D — API integrity: units, symmetry, safety, and the pruning pass
Labels: `epic`, `enhancement`, `primitive` · Goal: the trait a Windows author reads is consistent, panic-free, and honest.
1. **D1** Public char-boundary text utils; fix the 12+ byte-slice panic sites + `Color::from_hex`; unify the 7 private copies (complements #471/#472) — `bug`,`primitive`
2. **D2** Unit hygiene: `EditorPaintResult` → `Point`, retire `TabBarHits` f64 legacy + converter, `ActivityBarRowHit` f32, `text_selection_line_range` units — `enhancement`
3. **D3** Coordinate-frame convention: document one rule; audit all `*_layout`; non-zero-origin regression coverage (with B7) — `enhancement`,`primitive`
4. **D4** Trait symmetry: add missing `*_layout` methods (board/list/palette/diff_view/editor/terminal); lift off-trait rasteriser fns onto the trait; converge the 3 layout-passing conventions (companion to #456) — `enhancement`,`primitive`
5. **D5** Minimal error channel for backend authors (frame/event/services seams; Unsupported ≠ failure ≠ cancelled) — `enhancement`
6. **D6** Panic-path hardening: tree_controller/menu_system unwraps, `set_cell` left-edge guard, chart empty-series; shared scroll-clamp helper — `bug`
7. **D7** Dead-API disposition pass (modal.rs, 13 dead *Event enums, FocusGroup/FocusRing, drop_zone) — **gated on checking vimcode/coord-tui consumers first** — `enhancement`
8. **D8** God-file mechanical splits: lib.rs test relocation, gtk/backend.rs layout extraction, AppShell::handle, dispatch submodules — `enhancement`
Deps: D1 independent (do early — real crashes); D4 after/with #456's direction; D2/D3 before Windows trait-freeze.

**Cross-epic ordering:** A1 first; then B (the suite makes A2–A4 and all of C verifiable); C and D interleave; Windows port (#19–#31) starts after A+B and C5.

---

## 8. Already covered by existing issues — found independently, NOT re-filed

| Issue | This audit's confirmation / addition |
|---|---|
| #456 two-APIs drift | Quantified: `frame.rs` Surface misses 11 primitives; 4-place extension; 3 layout conventions (§4). D4 is the trait-side companion. |
| #455 registered-but-invisible modals | Conformance scenario (2) in §6.5 is the executable check. |
| #465 macOS ShellApp/run_with_shell | Root cause is the unshared shell_runner (PORT-09); C3 unblocks it. |
| #471 visible_width chars | Confirmed; 17 sites incl. 3 pixel-rect sites in sidebar_system; add: `unicode-width` must become non-optional for the fix to be possible in core. |
| #472 private cell_width / safe truncate | Confirmed (+ third wrong variant in `tui/menu_bar.rs:101-103`); D1 covers the in-crate panic half only. |
| #474 fuzzy scorer | Narrower than stated: exactly one scorer exists (`dir_fuzzy_score`, private); promote it. |
| #476 board.rs half-dead | Quantified (§ SM-04). |
| #403 clippy fails on develop | **Not reproducible** — gate green at 73172ff; recommend verify+close (human action). |
| #305–#309 driver backfill | 9/25 done; GTK halves blocked on TEST-01 (B2), not authoring. #308: 0/5. |
| #310/#311 coverage report / CI gate | B4's matrix artifact generalizes #310 across backends; #311 unchanged. |
| #300/#301/#302/#304 | Extended, not replaced, by Epic B. |
| #314–#324 quadraweb | `FrameInventory` (B3) deliberately matches #322's id→rect map. |
| #19–#31 Win-GUI port | Unchanged scope; sequence after A+B+C5 per §7. |
| #418 scroll direction / natural scroll | PORT-13 folds the unit-semantics question into its resolution. |
| #415 GTK clipboard/IME→PTY | C8 is the general IME design; #415 remains the terminal-routing consumer. |
| #440/#441/#442/#337 wide-char | C6 is the "share one implementation" half those issues imply. |
| #184 native NSMenu · #235/#234 gtk::run · #382 minimap · #365/#342/#343/#338/#339 terminal | Noted; no overlap with new items. |

*Not filed (deliberately):* kubeui zero-tests (demo crates, low value); Theme accessibility/high-contrast (premature before a consumer asks); multi-window (out of scope per BACKEND_TRAIT_PROPOSAL §6.3); PORT-15 example pairing (tracked as acceptance criteria inside A2/C3 instead of standalone issues).

---

## 9. Filed board items (created 2026-07-25 via coord)

| Epic | # | Children |
|---|---|---|
| A — Compile truth: every declared backend builds, CI proves it | **#479** | #483 CI matrix+develop · #484 MacBackend 7 methods · #485 WinBackend conformance · #486 macOS event parity · #487 docs truth pass |
| B — Backend conformance suite | **#480** | #488 ConformanceDriver · #489 GTK painted-text · #490 FrameInventory · #491 scenario schema+runner · #492 Tier C0+BackendCaps · #493 MacDriver · #494 test-debt (coords/origins) |
| C — Shared runtime core | **#481** | #495 event constructors · #496 runner pipeline · #497 shell-runner core · #498 desktop/ module · #499 pixel-metrics layer · #500 terminal sharing · #501 UiEvent matrix · #502 IME design |
| D — Backend API integrity | **#482** | #503 char-boundary utils/panics · #504 unit hygiene · #505 coordinate-frame convention · #506 trait symmetry · #507 error channel · #508 panic hardening+scroll-clamp · #509 dead-API disposition · #510 god-file splits |

`status:refining` applied to: #490, #492, #498, #501, #502, #504, #506, #507, #509. No pre-existing issue was closed, edited, or relabeled.
