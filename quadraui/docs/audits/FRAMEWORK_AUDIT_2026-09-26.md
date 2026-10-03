# quadraui — independent framework audit

> Landed verbatim from issue #1127 (body + three comments, which carried the report in four parts). Findings were filed as epics #1095 (widget model) and #1096 (language bindings), plus additions to #783, #784, #785 and #788. The prior audit it cites, `ELECTRON_PARITY_AUDIT.md` (2026-09-13), was never committed.

**Audited:** `JDonaghy/quadraui` `develop` @ `ed402b4ae0d9b753279bebe1bba4284dbe515d8a` (2026-09-26).
**Method:** read-only. Source, docs, git history, `cargo check`/`cargo test`/release builds in a scratch target dir. Nothing under `~/src/quadraui` or `~/src/vimcode` was modified. Every number below was measured on this commit; anything I could not confirm is marked **unverified**.
**Prior audit:** `ELECTRON_PARITY_AUDIT.md` (2026-09-13 @ `68f0ef9`) — §4 below records what it got right, what has since been fixed, and what it got wrong.

---

## 0. Verdict

**Somewhere in between — and closer to "serious prototype with a real idea" than to either pole.** It is not AI-slop garbage: the architecture is coherent, deliberately chosen, and defended by an unusually large test suite, and the core invariant (paint and hit-test consume one layout) is a genuinely good engineering idea that most GUI toolkits do not have. It is also not a contender an outside developer should pick today. Not because of bugs — because of what it *is*: a **terminal-shaped widget catalogue with native rasterisers**, unreleased, single-consumer, single-maintainer, with the three things serious desktop apps are legally or commercially required to have (accessibility, IME, bidi) absent by design and honestly documented as such.

The calibrated statement is: **quadraui is a credible framework for one narrow class of app — keyboard-driven, monospace-grid, developer/ops tools that want to run identically over SSH and as a native window — and for that class it occupies a niche nobody else does.** Ratatui/Textual have no native backends; egui/iced/Slint/GPUI have no terminal backend; Electron has neither the binary size (a quadraui macOS app is **0.7–2.0 MB**, measured) nor the terminal story. That niche is real. It is also small, and the framework's design choices — no custom drawing surface, no layout engine, `Rect` in f32 "units" that mean cells on TUI and pixels elsewhere, a rebuild-every-frame model without a rows provider — are exactly the choices that make TUI parity cheap and that cap the ceiling well below "Electron minus the web stack". You cannot build Slack, Figma, Spotify, Notion, or even VS Code's webview-backed panels on it, and the roadmap would have to break the TUI-first grid model to get there. **TUI parity is both the differentiator and the ceiling; the project has not yet chosen which one it wants to be.**

Where it wins: one `AppLogic` body genuinely runs on four backends (68 shared example bodies, vimcode's per-backend code is 1.8% native-touching by its own audit); tiny binaries; event-driven runtime (idle app renders nothing); capability honesty machinery (`BackendCaps` + source-parsed conformance tests) that is better than most crates; declarative, `Serialize` primitives that make a future bindings/web story plausible; ~4,000 tests including headless paint→click round-trips on all four backends. Where it loses badly: no assistive-technology support at all (rules it out for government, enterprise, education, and any WCAG-obligated buyer); no IME (rules out CJK users); no RTL; API churn (131 `#[deprecated]` items at version `0.0.1`, 50 pin bumps in vimcode's history, zero tags, not on crates.io); a sealed 132-method `Backend` god-trait; Windows backend never run by a production consumer; docs that contradict the code in several load-bearing places; and a bus factor of one human plus an LLM (910 of 1,085 commits are `Co-Authored-By: Claude`).

**Who should pick it today:** a Rust developer building a terminal-first dev/ops/data tool who *also* wants a native window from the same code, is comfortable pinning a git rev and reading source, and does not need a11y/IME. **Who should not:** anyone shipping to end users at scale, anyone with accessibility obligations, anyone whose UI needs proportional layout, images, canvases, animation, or more than one window, and anyone outside Rust (no bindings exist).

### Top 10 findings (skimmable)

| # | Finding | Sev |
|---|---|---|
| 1 | **No accessibility of any kind.** Zero AccessKit/AT-SPI/UIA/NSAccessibility code; `A11yInfo` (`src/a11y.rs`) is defined and used by **0** other files. | Critical |
| 2 | **Closed widget catalogue, no custom drawing.** `Backend` exposes no `fill_rect`/`draw_text`/`draw_line`; the pixel surface (`NativeSurface`, `src/native_surface.rs:113`) is `pub(crate)` and `mod native_surface` is private (`lib.rs:240`). An app can only compose the 39 shipped primitives. | Critical (for the stated Electron goal) |
| 3 | **Not released, not published, API still moving.** `version = "0.0.1"`, 0 git tags, not on crates.io; 131 `#[deprecated]` items (50 with `since = "0.0.1"`), 213 `#[allow(deprecated)]` internal call sites; vimcode has bumped its pin 50 times. | Critical (for adoption) |
| 4 | **TUI grid model leaks into the GUI API.** `Rect{f32,...}` (`event.rs:182`) means cells on TUI and pixels on GTK/macOS/Win with no type distinction; `docs/LESSONS.md` has three separate entries on unit bugs; apps do all layout as manual `Rect` arithmetic — there is no box/flex/constraint layout. | High |
| 5 | **`Backend` is a sealed 132-method god-trait** (`backend.rs:836`; 52 `draw_*`, 37 `*_layout`, plus window/tray/focus/modal/services/fonts). Sealed means no third-party backends; the size means no binding can wrap it. | High |
| 6 | **Windows backend: 214 `unsafe {}` blocks, 20 `SAFETY:` comments** (9%). macOS is 145/117 (81%). `win/run.rs:903,941,968,1079,1098,1107,1136,1177` are typical undocumented FFI calls. | High |
| 7 | **God functions.** `gtk/run.rs::activate` ≈ 944 lines (L463–1407), `tui/form.rs::draw_form` ≈ 596 (L108–704), `gtk/editor.rs::draw_editor_with_options` ≈ 567 (L147–714), `compose/menu_system.rs::handle` ≈ 366 (L116–482); 164 functions > 100 lines; 125 `#[allow(clippy::too_many_arguments)]`. | High |
| 8 | **Docs contradict code in load-bearing places.** README says Windows `draw_*` are "largely" `todo!()` (there are **0** non-test `todo!()` in `win/backend.rs`); README `## Features` lists only `tui`/`gtk` (there are 6); README and quadraui's `CLAUDE.md` say vimcode pins by relative path + vendored vt100 (it pins a git rev); `dispatch.rs:14` still says "pilot: `dispatch_mouse_down` only" while exporting 6 dispatchers; `docs/ARCHITECTURE.md` (64 lines) describes a two-backend crate. | High |
| 9 | **No IME, no RTL/bidi, East-Asian width only.** Confirmed: 0 hits for `IMContext`/`NSTextInputClient`/`WM_IME_`; honest in README, but it rules out a large share of the world. | High |
| 10 | **Performance model is unproven.** Every redraw is full-window (`queue_draw()` without region `gtk/run.rs:1017`, `setNeedsDisplay(true)` `macos/run.rs:1205`, `InvalidateRect(…, None)` `win/run.rs:1354`); collection primitives own rows by value and are rebuilt per frame (`ROWS_PROVIDER_PROPOSAL.md` — deferred); **zero benchmarks** in the repo. | Medium-High |

---

## 1. Against the realistic alternatives

| Alternative | Where quadraui wins | Where quadraui loses | Net |
|---|---|---|---|
| **Electron** | Binary 0.7–2 MB vs ~100 MB; RSS; startup; no JS; terminal backend; Rust type safety | Everything web gives for free: layout engine, rich text, images/video, a11y (Chromium's), IME, i18n, DevTools, ecosystem, millions of devs, multi-window, printing | Electron wins for any product UI; quadraui wins only for terminal-parity tools |
| **Tauri** | No webview dependency (Tauri inherits WebKitGTK/WebView2 inconsistencies); TUI | Same web-stack advantages as Electron at a small binary; huge community; mobile | Tauri wins broadly |
| **Qt (C++/PySide)** | License simplicity (MIT/Apache); TUI; no moc/qmake | Qt has everything: a11y, IME, RTL, layouts, styles, QML, designer, 25 years of edge cases, bindings to 6+ languages | Qt wins on every axis except licence and TUI |
| **Slint** | TUI backend (Slint has none); Rust-native app code without a DSL | Slint has a real layout engine, declarative `.slint` DSL, a11y (AccessKit), C++/JS/Python bindings, commercial support, embedded/MCU story, animations | Slint is what quadraui would need to become on the GUI side |
| **egui** | Native text rendering (Pango/CoreText/DirectWrite) vs egui's own rasteriser; TUI; event-driven (egui repaints more) | egui has custom painting, proportional layout, immediate-mode ergonomics, web (wasm) target, enormous adoption, a11y via AccessKit | egui wins for general apps; quadraui's text quality is better |
| **iced** | TUI; simpler runtime | iced has Elm architecture, layout engine, custom widgets/canvas, wgpu, wasm, a11y work in progress | iced wins |
| **Dioxus** | TUI (Dioxus dropped its TUI renderer); no VDOM overhead | Dioxus: React model, web + desktop (webview) + mobile, hot reload, big community | different model; Dioxus wins on breadth |
| **GPUI (Zed)** | TUI; smaller | GPUI: GPU-accelerated, production-proven at Zed scale, real layout (Taffy), rich text | GPUI wins for editor-class apps — the exact class vimcode is |
| **Flutter desktop** | Binary size; TUI; Rust | Flutter: everything, plus mobile and web | Flutter wins |
| **Avalonia / .NET MAUI** | TUI; no CLR | Avalonia: XAML, a11y, IME, styling, designer, WinUI-quality; MAUI: mobile | Avalonia wins for .NET shops |
| **ratatui** (TUI only) | quadraui *uses* ratatui; adds a native window for free; adds a widget catalogue, hit-testing, focus, modal stack, conformance tests ratatui lacks | ratatui: stable, published, huge ecosystem, no opinion imposed | For a TUI-only app, use ratatui; quadraui is the only option if you want the GUI twin |
| **Textual** (Python TUI) | Native backends (Textual has `textual-web`, a browser bridge, no native window); Rust performance | Textual: CSS-like layout, published, docs, Python audience, dev tooling (`textual console`) | Textual is a far better *product*; quadraui is the only one with native rasterisers |

The one cell where quadraui is alone: **"same app in a terminal and as a native window, from one code path, with native text rendering."** No other framework in the table does that. Whether that cell is a market is the owner's bet.

---

## 2. Is TUI parity a differentiator or a ceiling?

**Both, and the evidence says the ceiling is currently binding.**

What TUI parity buys (verified):
- The `examples/common/` directory holds **68 shared app bodies**; each `tui_*`/`gtk_*`/`macos_*`/`win_*` file is a 3-line shim (`examples/tui_app.rs`, `gtk_app.rs`, `macos_app.rs` differ only in the runner call). That claim is true.
- vimcode's own `docs/IRREDUCIBLE_SURFACE.md` measures its remaining per-backend production code at **11,489 lines of which 207 (1.8%) touch native APIs**; the rest is duplication being migrated. The "zero per-backend code" promise is approximately delivered for the consumer that drove it.
- Idle apps render nothing (event-driven; `RedrawAfter` for timers) — a property the TUI forced and that benefits GUIs.

What TUI parity costs (verified):
- **The metrics vocabulary is monospace.** `Backend::measure()` returns `line_height`/`char_width` and nothing else; `hello.rs:46` sizes a status bar as `line_height * 1.4`. Proportional text exists at paint time (Pango/CoreText/DirectWrite do shape it) but *layout* is done in character-cell arithmetic. Every primitive's `*_layout` takes a `line_height`/`char_width` pair.
- **No layout engine.** `AppLogic::render` hands you a `Viewport` and you compute every `Rect` yourself (`examples/common/appshell_demo.rs:143–184` is representative). `Split`/`SplitTree`/`Panel`/`AppShell` are the only containers; there is no stack/flex/grid/constraint solver. Terminals make this tolerable; a desktop app with resizable proportional content does not.
- **No custom drawing.** Because every primitive must have a TUI rasteriser, there is no escape hatch: `Backend` has no primitive-free drawing calls (verified by grepping the trait for `draw_rect|fill_rect|draw_text|draw_line|draw_path`: only `draw_image`, `draw_text_display`, `draw_text_input` match, all of which are primitives). `NativeSurface` has exactly the right 15 methods (`surface_fill_rect`, `surface_draw_text_run`, `surface_push_clip`, `surface_draw_image` …) but is crate-private. A `Canvas` primitive whose TUI story is "blank / braille approximation" would cost little and lift the ceiling a lot.
- **Images are second-class.** `Image` decodes via GdkPixbuf/NSImage/WIC per backend and paints a placeholder on TUI (`tui/backend.rs:2997`). Fine for icons; not a media surface.
- **Units are untyped.** `Rect` is four `f32`s (`event.rs:182`). On TUI those are cells, elsewhere pixels; `Viewport.scale` exists but `Rect` does not carry it. `docs/LESSONS.md` has three entries ("Shared AppLogic code must not hardcode backend-native units", "Dropdown item sizing must use backend-native units", "Backend draw_* and *_layout must agree on which dimensions they use") — each is a bug that a `Px`/`Cell` newtype or a single logical-unit contract would have prevented at compile time.
- **Coordinate-frame drift between primitives.** `PRIMITIVE_RULES.md` "Coordinate frames for `*_layout`… converging on ABSOLUTE-only (#816)" documents that some layouts return widget-local and some absolute rects; `macos/list.rs:57` keeps dead `_x, _y` parameters "for symmetry with `draw_list`". Same function, four argument orders across backends: `tui_list_layout(area, list)` (`tui/list.rs:30`), `gtk_list_layout(w, h, list, lh, cw)` (`gtk/list.rs:43`), `mac_list_layout(list, x, y, w, h, lh, cw)` (`macos/list.rs:57`), `win_list_layout(list, rect, lh)` (`win/list.rs:51`).

**Conclusion.** TUI parity does not *force* a lowest-common-denominator model — Textual proves a TUI can have a CSS-grade layout engine, and a `Canvas` with a degraded TUI rendering is a routine pattern — but quadraui has *chosen* the LCD at every fork so far, because vimcode never needed otherwise. If the goal is really "Electron's capability breadth", the next architectural decisions (layout engine, custom drawing, typed logical units, proportional metrics) must be made *against* the TUI's convenience, with the TUI degrading honestly. If the goal is "the best terminal+native framework for dev tools", the current model is right and the Electron framing in `GOAL.md` is a distraction that is pulling effort into tray icons and secret stores before the widget model is finished.

---

## 3. Numbers (measured at `ed402b4`)

| Metric | Value |
|---|---|
| Lines in `quadraui/src` | **217,915** |
| … of which comment lines | 56,211 (26%) |
| … of which inside `#[cfg(test)]` | ≈ 105k (test share per module: primitives 44%, compose 45%, tui 62%, gtk 55%, macos 45%, win 42%) |
| Non-test, non-comment code per module | primitives 11.5k · compose 9.0k · macos 10.8k · win 9.5k · tui 8.8k · gtk 8.4k |
| `#[test]` functions | 3,987 in crate (+85 in root `tests/`) |
| Tests with no assertion/panic path (heuristic) | 159 (4%) — mostly `*_does_not_panic`, `dump_smoke_ppm`, `paint_click_round_trip` bodies that assert via helper |
| Public API | 1,383 `pub fn`, 331 `pub struct`, 152 `pub enum`, 15 `pub trait`; **507 crate-root re-exports** (`lib.rs`) |
| `Backend` trait | **132 methods** (52 `draw_*`, 37 `*_layout`); 7 traits in `backend.rs`; `backend.rs` is 4,206 non-test lines of which **2,988 are doc comments (71%)** |
| `#[deprecated]` items / `#[allow(deprecated)]` | 131 / 213 |
| `#[allow(clippy::too_many_arguments)]` / `dead_code` / `print_stderr` | 125 / 67 / 14 |
| `unsafe` blocks vs `SAFETY:` comments | macOS 145 / 117 · **Win 214 / 20** · GTK 3 · TUI 5 |
| `catch_unwind` at FFI boundaries | macOS 14, Win 8 (good) |
| Non-test `unwrap`/`expect` | macOS **48** (≈16 are `expect("… requires set_current_font")` in `macos/backend.rs:1567–2398`), compose 12, gtk 8, tui 7, primitives 6, win 4 |
| Non-test `todo!()` | win/backend.rs **0** (13 hits are in tests/comments); `win/run.rs` 6 and others are `cfg(not(windows))` stand-ins — **unverified** individually |
| Functions > 100 lines | 164 |
| Comment lines citing an issue number (`#NNN`) | **4,095** |
| Comment lines narrating history ("used to", "previously", "before #", "no longer") | 365 |
| `Cargo.toml` | 1,308 lines (374 comment lines, **157 `[[example]]` entries**) |
| Examples | 159 files: 65 tui, 56 gtk, 18 macos, 16 win |
| Example-driver tests | tui 222, gtk 21, macos 17, **win 5** |
| Conformance scenarios | 19 JSON files; `win` is a non-gating "burn-down" column |
| Git | 1,085 commits since 2026-04-18 (≈6.7/day); **910 (84%) `Co-Authored-By: Claude`**; 2 human author spellings; 0 tags |
| Lines added in Sept 2026 alone | +125,786 / −36,545 |
| Release binary size (`--release`, arm64) | `hello` (TUI) **1.6 MB**, `macos_app` **0.7 MB**, `macos_appshell_demo` **2.0 MB** |
| `cargo check --features tui,macos --examples --tests` | 41.8 s, clean (1 deprecation warning in acceptance tests) |
| `cargo test --features tui,macos,gtk` | see §3.1 |

### 3.1 Test run

`cargo test -p quadraui --features tui,macos,gtk` on this macOS host (arm64, GTK 4.22 via Homebrew, no `DISPLAY`), scratch target dir, 4 min 51 s wall including compile:

- **lib target: 3,210 tests, 3,203 passed, 1 failed, 6 ignored, 88.6 s.**
- The failure is `gtk::services::tests::build_file_dialog_behaviors`, panicking inside gtk4-rs `rt.rs:107` — `"Attempted to initialize GTK on OSX from non-main thread"`. The test's own guard `require_gtk()` (`gtk/services.rs:1134`) calls `gtk4::init().is_ok()` expecting a graceful `Err` when there is no display; on macOS gtk4-rs *asserts* main-thread affinity before it gets that far, and Rust's harness runs tests on worker threads. So **the `gtk` feature's test suite cannot pass on macOS at all**; CI never runs that combination (GTK is Linux-only in `ci.yml`, `macos.yml` runs `--features macos`). Not a library bug; a portability hole in the test guard, and a data point that "four backends" is really "each backend on its own CI OS".
- Because cargo stops after a failing target, the integration tests (`tests/*.rs`, 85 `#[test]`s + the conformance/acceptance suites) did not run in that invocation; see the addendum at the end of this section.

**Addendum — integration targets** (`cargo test -p quadraui --features tui,macos,gtk --no-fail-fast --test '*'`): 20 test binaries, **474 passed, 0 failed, 1 ignored**, including the conformance matrix, the acceptance suite, the four example-driver suites (tui 224), the pty smoke tier, and the doc/manifest meta-tests. Nothing flaked on a second run of the lib target either (**unverified** beyond the two runs above).

Net: **3,677 of 3,684 executed tests pass on macOS; the single failure is a Linux-only test guard, not framework behaviour.** The suite is real, fast for its size (≈90 s lib), and deterministic in my runs.

---

## 4. The prior audit (2026-09-13): fixed, open, wrong

The velocity since the prior audit is startling: **eleven of its sixteen gap blocks landed in 13 days.**

| Prior finding | Status now | Evidence |
|---|---|---|
| G1 no window control surface | **Fixed** | `pub trait WindowControl` `backend.rs:191`; `Backend::window()` `:1446`; `BackendCaps::window_control`; `UiEvent::WindowStateChanged` |
| G3 macOS never emits `WindowClose` | **Fixed** | `windowShouldClose:` `macos/run.rs:923`; `applicationShouldTerminate:` `:1311` |
| `open_url` silent no-op on TUI | **Fixed** | `open_url_result` (`backend.rs:3433`), TUI tries platform opener then OSC 8 (`tui/services.rs:537`), recent commits harden against shell injection |
| GTK `send_notification` `{}` | **Fixed** | `gio::Notification` `gtk/services.rs:375`; `UiEvent::NotificationActivated` |
| G4 native theme | **Fixed** | `system_theme()` `backend.rs:3546`, `UiEvent::SystemThemeChanged` |
| G5 tray | **Fixed** (macOS/Win; GTK **unverified**) | `pub trait TrayService` `:329`, `Backend::tray()` `:1478`, `macos/tray.rs`, `win/tray.rs`, no `gtk/tray.rs` |
| G6 clipboard image | **Fixed** | `Clipboard::read_image` `:3944` |
| G8 shell utils | **Fixed** | `reveal_in_file_manager` `:3450`, `move_to_trash` `:3488`, `beep` `:3501` (`trash` crate dep) |
| G9 deep links / single instance | **Fixed** | `UiEvent::OpenRequested`; GTK `HANDLES_OPEN` + `connect_open` (`gtk/run.rs:368`); Win `CreateMutexW`/`FindWindowW` (`win/run.rs:941–968`) |
| G10 screen API | **Fixed** | `displays()` `:3623`, `UiEvent::DisplaysChanged` |
| G16 safeStorage | **Fixed** | `pub trait SecretStore` `:3677` (`keyring` dep) |
| §3.4 Rule 9 PlatformServices honesty | **Fixed** | `PRIMITIVE_RULES.md` "Rule 9 — `PlatformServices` honesty (issue #949)" |
| G2 frameless / CSD | **Partially** | `ShellConfig::with_client_side_titlebar` `shell.rs:204` exists; per-backend undecorated-window behaviour **unverified** |
| Tagged release / crates.io (#797) | **Open** | 0 tags, `0.0.1`, unpublished |
| Owned `Frame`/`Surface` for bindings | **Open** | `Surface<'a>`/`ScreenLayout<'a>` still borrow (`frame.rs:144,326`) |
| 11 primitives lack a `Surface` variant | **Open and worse: now 14 of 39** | `Surface` has 25 variants; missing: Board, CommandCenter, DiffView, DropZone, Image, MessageList, Minimap, PipelineView, Progress, SidebarPanel, Spinner, SplitTree, TextInput, Toolbar |
| Multi-window | **Open** | no `BackendManager`; runners create exactly one window |
| G11–G15 (badge, global shortcuts, power, drag-out, print) | **Open** (**unverified** — not grepped exhaustively) | — |
| a11y (README) | **Open** | `A11yInfo` defined, 0 uses |

Where the prior audit was wrong or overstated:
- *"The Python blocker is API stability, not immediate mode."* Half right. API stability is the *adoption* blocker; the *structural* blocker is the push-style `render(&self, &mut dyn Backend)` over a 132-method sealed trait. No binding can wrap that; the inverted owned-`Frame` API it recommended is still not built, and the `Surface` gap grew. See §8.
- *"Node is the worst fit."* It is the hardest, not the worst; with a `pump()`-style step API it is tractable (§8.7). Calling it the worst fit while it is the core Electron audience was giving up too early.
- It treated "TUI-for-free is in good shape" as a strength without weighing that the TUI grid is the ceiling on the GUI side (§2).
- It did not look at code quality at all (unsafe hygiene, god functions, docs drift). Those are the findings in §5.

---

## 5. Code smells and design problems

Ordered roughly by how much they would cost an outside adopter. Every item has a citation.

### 5.1 The `Backend` god-trait (High)
`pub trait Backend: sealed::Sealed` (`backend.rs:836`) has 132 methods mixing five concerns: per-primitive paint (`draw_*` ×52), per-primitive layout (`*_layout` ×37), frame lifecycle (`enter_frame_scope` …), platform services (`services()`, `window()`, `tray()`, `waker()`, `register_accelerator`), and host integration (`set_theme`, `set_editor_font`, `register_font_from_memory`, `install_menu_bar`, `begin_window_drag`, `set_cursor`). Consequences:
- **Sealed** (`sealed::Sealed`) means no third-party backend can exist. That is defensible pre-1.0, but it also means the "four-backend portability" story can never be a community story.
- Every new primitive is a **breaking change to every backend** ("rule 7 no-defaults"). The crate's own conformance machinery (`tests/conformance/caps.rs`) exists largely to police the consequences of this shape.
- It is exactly the shape a language binding cannot wrap (§8).
- 71% of `backend.rs`'s non-test lines are doc comments — the trait is documented like a specification because it *is* the whole architecture.

The 14 primitives that route paint through the shared `native_surface_paint` modules (`primitives/{status_bar,panel,toast,...}.rs`) show the better shape already exists: a 15-method `NativeSurface` (`native_surface.rs:113`) plus shared paint code. **Finishing that migration and making `NativeSurface` the public backend seam** would shrink the per-backend surface from 132 methods to ~15 + platform services, and would make custom drawing, third-party backends, and bindings all fall out of one change. `tui` is the exception (0 `native_surface_paint` callers) because cells are not pixels — which is the one place a second seam is justified.

### 5.2 Backends are not thin; they are four rasteriser suites (High)
The README's premise is that primitives are shared and backends "rasterise them in their native idiom". Measured: non-test, non-comment code is **tui 8.8k, gtk 8.4k, macos 10.8k, win 9.5k = 37.5k lines of backend code vs 11.5k of shared primitive code**. Layout *is* shared (verified: `gtk/list.rs:51`, `macos/list.rs:66`, `win/list.rs:57`, `tui/list.rs:69` all call the primitive's `layout`), and 14 primitives share paint via `NativeSurface`. The other 25 primitives have four hand-written painters each (`src/{tui,gtk,macos,win}/{tree,editor,palette,data_table,form,tab_bar,…}.rs`). `docs/SMELL_AUDIT_2026-07.md` §7 "EPIC C — one implementation of the duplicated 65%" acknowledges this; it is about a third done.

### 5.3 God functions (High)
Verified by locating the next top-level `fn`:
- `gtk/run.rs:463` `activate` → next fn at L1407: **~944 lines** of nested closures wiring every GTK signal.
- `tui/form.rs:108` `draw_form` → L704: **~596 lines**.
- `gtk/editor.rs:147` `draw_editor_with_options` → L714: **~567 lines**.
- `compose/menu_system.rs:116` `handle` → L482: **~366 lines**.
- 164 functions exceed 100 lines; 125 sites carry `#[allow(clippy::too_many_arguments)]` (e.g. `gtk/editor.rs:147` takes 8 positional args including two `f64`s that are easy to swap — the unit bugs in `LESSONS.md` are this shape).

### 5.4 Untyped units and coordinate frames (High)
`Rect { x, y, width, height: f32 }` (`event.rs:182`) carries no unit. On TUI it is cells; elsewhere device-independent pixels; `Viewport.scale` (`event.rs:212`) is separate. There is no `Px`/`Cell`/`Lh` newtype anywhere (grep). `PRIMITIVE_RULES.md` documents an in-progress migration from mixed local/absolute frames to absolute-only (#816). `macos/list.rs:57` retains dead `_x, _y` params. This is the single largest source of the "paint/click drift" bug class the crate says it exists to eliminate, and it is a type-system fix.

### 5.5 Stringly-typed identity (Medium)
`pub struct WidgetId(pub String)` (`types.rs:248`); 1,277 `WidgetId::new("…")` literals in `src` alone, 26 built by `format!`. The consumer dispatches on prefixes: vimcode `app.rs:3971` `id if id.starts_with("ext:")`, `:1764` `starts_with("bottom:")`. Every `UiEvent` primitive arm is `(WidgetId, XEvent)`. It works and it serialises, which the bindings story needs, but there is no namespacing, no typo protection, and no way to attach typed payloads. A `WidgetId` that is `Copy` + interned, with a separate string label for debugging/serde, would be the usual fix.

### 5.6 `unsafe` hygiene on Windows (High)
`src/win`: 214 `unsafe {` blocks, 20 `SAFETY:` comments. Samples with no justification within four lines: `win/run.rs:903` (`GetKeyState`), `:941–942` (`CreateMutexW` + `GetLastError` — note the `Ok(_handle)` is dropped immediately, so the mutex handle leaks by design or by accident; **unverified** which), `:968` (`FindWindowW`), `:1079`, `:1098`, `:1107`, `:1136`, `:1177` (`&*state_ptr` deref of the `GWLP_USERDATA` box — the one place a `SAFETY:` is essential). macOS is much better (117/145) and has a source-parsed test enforcing `# Safety` docs on exported `unsafe fn` (`tests/macos_safety_docs.rs`); Windows has no such gate. `catch_unwind` at every AppKit/wndproc callback boundary (macOS 14, Win 8) is correctly done. GTK's `unsafe impl Send` hits are doc comments explaining why one was *not* written (`gtk/backend.rs:131`) — good.

### 5.7 Panics in library paths (Medium)
`macos/backend.rs:1567–2398`: ≈16 `expect("MacBackend::draw_X requires set_current_font")`. A `Backend` that panics if the host forgot a setup call is a footgun; return a `PaintResult::Err` or lazily default the font. `primitives/` and `tui` are clean (6 and 7 non-test unwraps). `tui/backend.rs:3007` `expect("… called outside enter_frame_scope")` is the same pattern.

### 5.8 API churn frozen in place (High)
131 `#[deprecated]` items, 50 of them `since = "0.0.1"` — the *current* version — so nothing has ever been removed; 213 internal `#[allow(deprecated)]` keep the old shapes alive (`lib.rs` re-exports `SyntaxSpan` under `#[allow(deprecated)]`; `ShellApp::on_shell_event` is deprecated in favour of `on_shell_event_ctx` inside the same trait, `shell.rs`). `tests/downstream_struct_literals.rs` exists to prove that *exhaustive struct literals in consumers still compile*, i.e. the crate has committed to never adding a field to `TextInput`/`Toolbar`/`Editor` — which is why `A11yInfo` had to be a sidecar type (`a11y.rs` module doc). That is `#[non_exhaustive]`'s job; using it on the primitives would have been one breaking change instead of a permanent constraint.

### 5.9 Docs-to-code drift (High — because the README is the adoption surface)
- `README.md:29` "most `Backend::draw_*`/`*_layout` methods on `WinBackend` are `todo!()` stubs" — **0** non-test `todo!()` in `win/backend.rs` (4,810 non-test lines).
- `README.md` `## Features` lists `tui`, `gtk`; `Cargo.toml` has `terminal`, `tui`, `gtk`, `win`, `gtk-example`, `macos`.
- `README.md` "vimcode's approach: `path = "../quadraui/quadraui"`" and quadraui `CLAUDE.md:63` (path pin + `vendor/vt100-0.16.2-patched` + `quadraui-pin.txt`) — vimcode pins `rev = "<sha>"` and the vendored vt100 is gone (vimcode `CLAUDE.md` #691, #795).
- `dispatch.rs:14` "What's here in the pilot: `dispatch_mouse_down` only" — the file exports `dispatch_mouse_down/drag/up/scroll/click` + `text_selection_line_range`.
- `docs/ARCHITECTURE.md` (64 lines) describes "TUI + GTK rasterisers" and says nothing about macOS/Win, `compose/`, `runtime.rs`, `dispatch.rs`, `shell.rs`, or `native_surface.rs`.
- `docs/APP_ARCHITECTURE.md` opens with a paragraph declaring itself historical.
- `README.md` "40 primitives" / `CONTRIBUTING.md` "40 in total" — 39 primitive modules.
- Meanwhile `tests/readme_truth.rs` (13 tests) exists specifically to keep README truthful and did not catch any of the above — it checks the primitive list and feature names it was written for, which is the failure mode of testing docs by string match.

### 5.10 Test-suite quality (Medium, mixed)
Genuinely strong: paint→hit-test round-trips on all four backends against headless surfaces (Cairo `ImageSurface`, macOS `BitmapSurface`, D2D `ID2D1DCRenderTarget`, ratatui `TestBackend`); `TuiDriver`/`GtkDriver`/macOS/Win example drivers running shipped apps end-to-end; the conformance matrix with source-parsed cap honesty; a real-pty smoke tier (`tests/tui_pty_smoke.rs`). This is far above the median Rust GUI crate.

Weak spots:
- **Process tests inside the unit suite.** `tests/quality_gate_docs.rs` asserts things about `CLAUDE.md` and `ci.yml`; `tests/downstream_gate_docs.rs` asserts the CI `downstream` job names the same consumers as `CLAUDE.md`; `tests/githooks_worktree.rs` (7 tests) runs `git worktree add` to check graphify hooks; `tests/example_manifest.rs` parses `Cargo.toml`. These are the maintainer's agent-workflow guardrails shipped in the crate's `cargo test`; an outside contributor's PR fails on a `CLAUDE.md` wording check. Move them to a `tools/` lint.
- **Host coupling.** macOS tests `expect("Menlo installed on every macOS host")` (`macos/status_bar.rs` and ≈ a dozen more) — true on stock macOS, false in a minimal CI image or with a font policy; better to embed a test font (`register_font_from_memory` exists).
- **Example parity is lopsided.** 222 TUI driver tests vs **5** Windows; 65 TUI examples vs 16 Win. The "four backends" claim is tested at roughly 100/50/8/2 %.
- 159 tests have no assertion by my heuristic; most are intentional (`*_does_not_panic`, `dump_smoke_ppm`) but `gtk/services.rs:1415 send_notification_without_a_window_is_a_no_op` is representative of tests that can only fail by panicking.
- No property tests, no fuzzing of the vt100/markdown/theme-JSON parsers, no benchmarks.

### 5.11 AI-generation tells (Medium — cost is reader time, not correctness)
- **4,095 comment lines cite an issue number**; 365 narrate history ("used to", "before #", "no longer"). `tui/services.rs:113–125` is a module doc that explains what `open_url` *used to be*. `Cargo.toml` has 374 comment lines, including a 20-line essay on docs.rs target selection. `a11y.rs`'s module doc is 45 lines explaining why a 20-line struct is a sidecar. `backend.rs` is 71% doc comments. Comments should explain the code as it is; the history belongs in the issue tracker and `CHANGELOG.md`. This is the most visible "written by an agent under a rule that says cite the issue" artefact, and it roughly doubles the reading cost of every file.
- 20 comments refer to "this PR"/"the reviewer"/"adversarial review" inside library source (`grep`).
- Over-specified rule docs: `PRIMITIVE_RULES.md` has numbered rules with "the measurement that produced this rule" subsections; `CHANGELOG.md` opens with 40 lines about its own governance before any entry.
- Meta-tests that test process (§5.10).
- 157 `[[example]]` stanzas each with `required-features` — examples as a test corpus rather than as documentation; a reader cannot find "the" example.
- Consistency is actually *good* where it matters (naming `tui_x_layout`/`gtk_x_layout`/`mac_x_layout`/`win_x_layout`, `Raw*Surface` adapters, `*Hit`/`*Layout`/`*Measure` triples) — the tell is verbosity, not chaos.

### 5.12 Miscellany (Low)
- `thread_local!` for `NERD_FONT_FALLBACK` (`gtk/mod.rs:288`) and `WAKE_CALLBACKS` (`gtk/backend.rs:119`) — process-global mutable config for what should be per-backend state; blocks two GTK backends in one process (moot today, single window).
- Feature `gtk-example = ["gtk"]` is an alias with no additional deps — sprawl.
- `WidgetId`, `Rect`, `Theme` (77 fields, `theme.rs:65`) are the graph's god nodes (1,203 / 422 edges); `Theme` as a flat 77-field struct with a VS Code JSON loader is fine for an editor and awkward for anything else.
- `docs/` is 10,340 lines across 22 files, several of which are proposals (`WEB_BACKEND_PROPOSAL`, `IME_INPUT_PROPOSAL`, `ROWS_PROVIDER_PROPOSAL`) that read as roadmap; the README now correctly flags them as unbuilt.

### 5.13 What is genuinely good (say so plainly)
- One-layout paint/click invariant, enforced by tests on every backend. Real, rare, valuable.
- Primitives are plain owned `Serialize + Deserialize` data with no closures. This is the property that makes a web backend, bindings, record/replay, and snapshot testing all *possible*. Most Rust GUI crates do not have it.
- Event-driven runtime; `RedrawAfter(Duration)`; `waker()` for background threads; modal stack with structural occlusion (`dispatch.rs`).
- Capability honesty: `BackendCaps` + `ServiceResult<T>`/`BackendError::Unsupported` + source-parsed conformance tests. The prior audit's guardrail recommendations were adopted in two weeks.
- Native text rendering on every GUI backend (Pango, Core Text, DirectWrite) rather than a home-grown rasteriser — better text than egui out of the box.
- Panic containment at every FFI callback boundary.
- 0.7–2 MB binaries and ~40 s type-check of the whole crate with examples and tests.
- The docs, for all their verbosity, are honest about what is missing (README "What is not supported" is exemplary).

---

## 6. Developer experience

### 6.1 The smallest app
`examples/hello.rs` (≈60 lines): implement `AppLogic` with `type AreaId = ()`, a `render(&self, &mut dyn Backend, ())` that builds a `StatusBar` struct literal and calls `backend.draw_status_bar_interactive(rect, &bar, &InteractionState::new())`, a `handle(&mut self, UiEvent, &mut dyn Backend) -> Reaction`, and `quadraui::tui::run(app)`. Switching backend is one line (`gtk::run`, `macos::run`, `win::run`). That is a good first-five-minutes. The mental model is **immediate-mode description, retained app state**: rebuild descriptors every frame, mutate your own state in `handle`, return `Redraw`. It is simple and it is the right model for the TUI.

What you write next is where it gets heavy:
- **All layout by hand.** `render` receives a `Viewport` and you compute every `Rect`. There is no container that lays out children. `ShellApp` + `AppShell` gives you VS Code chrome (activity bar, sidebar, bottom panel, status bar) and hands you `AppShellLayout` rects to fill; that is the only "layout for free".
- **Struct literals with 5–15 fields per primitive**, exhaustive (no `..Default::default()` in the examples; `tests/downstream_struct_literals.rs` enshrines this). `TreeView` needs pre-flattened `rows: Vec<TreeRow>` with paths, indents, expansion flags — you write the flattening.
- **Event routing is yours.** `handle` receives raw `MouseDown{position}` unless you use the compose controllers (`TreeController`, `TabGroupController`, `MenuSystem`, `SidebarSystem`, `FormController`, `ChatController`, `WorkspaceController`); those own real state machines and emit semantic events. They are the best part of the DX and the least documented (`README` "Compose Helpers" lists three of the fourteen).
- **Text input is app-owned.** `TextInput::apply(EditOp)` (`primitives/text_input.rs:583`) — you translate keys to edit ops (or use `FormController`). No IME means dead keys and CJK do not work at all.
- **Focus** is a `FocusManager` you drive via `tab_stops()`; fine.
- **Units**: you must remember `line_height` is 1.0 on TUI and ~17 on GTK, and write `backend.measure().line_height * n` everywhere (§2).

### 6.2 Onboarding, docs, errors
- `docs/GUIDE.md` is a real tutorial and correctly steers `AppLogic` vs `ShellApp`. `CONSUMER_PATTERNS.md`, `TUI_CONSUMER_TOUR.md`, `EVENT_ROUTING_TOUR.md` are useful. The *README* misleads (§5.9).
- The prelude is 54 lines; the crate root re-exports 507 items — discoverability is by rustdoc search. docs.rs configuration exists (`Cargo.toml` `[package.metadata.docs.rs]`) but the crate is unpublished, so there is no hosted rustdoc. **unverified**: whether `cargo doc` renders cleanly with `-D warnings` for `macos` (CI only checks Linux-buildable features).
- Error messages: `BackendError { Unsupported, PlatformFailure { context } }` — adequate. Panics on missing setup (`set_current_font`) are the bad ones.

### 6.3 Stability, versioning, publishing
Not on crates.io; no tags; `0.0.1`; consumers pin git SHAs and bump often (vimcode: 50 bumps). `CHANGELOG.md` was started recently and explicitly does not backfill. The rule-8 deprecation protocol is good discipline in principle; in practice the `Removed` half has never run. Until a `v0.1.0` exists on crates.io, no outside developer can reasonably depend on it.

### 6.4 Build and packaging per platform
- **TUI**: pure Rust, builds anywhere (`arboard`, `trash`, `keyring` are pulled in by the `tui` feature — `keyring` drags D-Bus/Secret-Service linkage on Linux; **unverified** whether that breaks a fully static musl build).
- **GTK**: needs GTK 4.10+ dev libs via pkg-config; on macOS/Windows that is Homebrew/MSYS2 — realistic only for Linux distribution. Flatpak story exists in vimcode, not in quadraui.
- **macOS**: `objc2` + `core-graphics`, no system deps beyond Xcode CLT. No `.app` bundle, `Info.plist`, code-signing, or notarisation guidance anywhere (grep of `docs/`, README, CONTRIBUTING: 0 hits). Notifications via `osascript` unless bundled (prior audit).
- **Windows**: `windows` 0.62, D2D/DWrite; SxS manifest test exists (`tests/win_sxs_manifest.rs`). No MSI/MSIX guidance.
- There is **no packaging story** at the framework level. Electron's `electron-builder`/`forge` is a large part of why teams pick Electron; Tauri ships bundling. This is a gap the GOAL.md parity list does not mention.

### 6.5 Accessibility, i18n, IME, HiDPI, theming, text, custom drawing, performance
- **Accessibility: none.** This is disqualifying for a "serious app developer" in most enterprise/public-sector contexts and increasingly for consumer apps (EU EAA in force since June 2025). Because primitives are data, AccessKit integration is architecturally *easy* relative to most immediate-mode toolkits: the frame description already is a tree with roles; the missing piece is emitting an AccessKit `TreeUpdate` per frame from `ScreenLayout` plus focus/selection plumbing. Weeks of work per backend, not months, once the owned `Frame` exists.
- **IME: none; RTL/bidi: none.** Honest in README. `Key::Char(char)` + `CharTyped` on one backend only.
- **HiDPI**: `Viewport.scale`, `DpiChanged`, `backingScaleFactor` (`macos/run.rs:501,740`), GTK `scale_factor`, Win `GetDpiForWindow` — present. Rendering at fractional scales **unverified visually**.
- **Theming**: `Theme` 77 colours + `from_vscode_json`; four built-in palettes; `system_theme()` for dark/light. Adequate for editor-class apps; no styling system (padding, radii, fonts per widget) — each rasteriser hard-codes its look.
- **Text quality**: platform shaping engines; monospace-metric layout. Proportional UI fonts exist (`set_ui_font`) but layout still assumes `char_width`. Wide chars via `unicode-width`; Nerd Font glyphs are a soft dependency for icons (`set_nerd_fonts`, `NERD_FONT_FALLBACK`).
- **Custom drawing**: none (§2). No canvas, no paths, no gradients, no shaders, no video.
- **Performance for large data**: rows are owned by value and rebuilt per frame; `TreeView::layout` iterates from `scroll_offset` and breaks at the viewport (`tree.rs:242–248`), so layout is O(visible) but *construction* is O(total) in the app every redraw. `ROWS_PROVIDER_PROPOSAL.md` defers the fix. Full-window invalidation on every redraw. No benchmarks exist, so every performance claim in the docs ("vimcode already does this") is anecdote. Likely fine for ≤10k rows and editor-sized windows; **unverified** beyond that.
- **Multi-window**: no.
- **Animation**: `RedrawAfter` + `Spinner`/`ProgressBar`; no tweening.

---

## 7. Severity-ranked findings and roadmap

### 7.1 Findings

| Sev | Finding | Where | Fix shape | Effort |
|---|---|---|---|---|
| **Critical** | No assistive-technology support | whole crate; `a11y.rs` unused | AccessKit adapter fed from an owned `Frame` tree; per-backend adapters (accesskit_macos/windows/unix) | XL |
| **Critical** | Closed catalogue, no custom drawing | `native_surface.rs` `pub(crate)`; `Backend` has no raw draw | Public `Canvas` primitive over `NativeSurface` ops with a TUI degrade; or expose `NativeSurface` behind a feature | M |
| **Critical** | Unreleased, churning API | `0.0.1`, 0 tags, 131 deprecations | Cut `v0.1.0` on crates.io; run the rule-8 `Removed` half; adopt `#[non_exhaustive]` + builders on primitives | M |
| **High** | Untyped units / mixed coordinate frames | `event.rs:182`; `LESSONS.md`; `PRIMITIVE_RULES.md` #816 | One logical-unit contract (`Rect` in logical px; TUI maps cells↔px via `char_width`/`line_height` internally) or `Px`/`Cell` newtypes; finish #816 | L |
| **High** | 132-method sealed `Backend` | `backend.rs:836` | Split: `PaintSurface` (≈15 ops) + `Services` + `WindowHost`; finish `native_surface_paint` migration for the remaining 25 primitives; unseal `PaintSurface` | XL |
| **High** | No layout engine | everywhere apps compute `Rect`s | Adopt Taffy (flex/grid, used by GPUI/Dioxus/Bevy) as an optional `layout` module producing `Rect`s; TUI works unchanged because Taffy is unit-agnostic | L |
| **High** | Win `unsafe` undocumented (20/214) | `src/win/*` | Extend `tests/macos_safety_docs.rs` scanner to `win`, then write the comments; audit `CreateMutexW` handle lifetime | M |
| **High** | God functions | `gtk/run.rs:463`, `tui/form.rs:108`, `gtk/editor.rs:147`, `compose/menu_system.rs:116` | Extract per-signal setup fns; split `draw_form` by `FieldKind` | M |
| **High** | Docs drift in README/CLAUDE/ARCHITECTURE/dispatch header | §5.9 | Truth pass; make `readme_truth.rs` check the claims that are actually wrong or delete it | S |
| **High** | No IME | all backends | Build `IME_INPUT_PROPOSAL.md`: GTK `IMContext`, macOS `NSTextInputClient`, Win TSF/`WM_IME_*`; TUI gets it free from the terminal | L |
| **High** | Windows unproven | 5 driver tests; burn-down; no consumer | Promote `win` to gating in conformance; ship one real app on it (vimcode's Windows build is the candidate) | M |
| **Medium** | Per-frame row rebuild; full-window redraw; no benchmarks | `ROWS_PROVIDER_PROPOSAL.md`; `gtk/run.rs:1017` | `Rows<T>` provider or `Arc<[Row]>` sharing; damage rects from the frame diff; criterion benches for 100k-row table & 4k-line editor | L |
| **Medium** | Panics on missing setup | `macos/backend.rs:1567–2398`, `tui/backend.rs:3007` | Default font lazily; `PaintResult::Err` | S |
| **Medium** | Stringly `WidgetId` | `types.rs:248` | Interned id + label; keep serde string form | M |
| **Medium** | Process meta-tests in `cargo test`; host font coupling | `tests/quality_gate_docs.rs`, `githooks_worktree.rs`; `expect("Menlo…")` | Move to `tools/lint`; embed a test font | S |
| **Medium** | Comment noise (4,095 issue refs; history narration) | everywhere | Editorial rule: comments describe current behaviour; history → CHANGELOG/issues. A one-time pass with `cargo clippy`-style lint on `#\d+` in `//` comments | M (mechanical) |
| **Medium** | Single window | runners | `WindowHost` with N windows sharing one `AppLogic`; `AreaId` seam already exists | L |
| **Medium** | No packaging story | — | Templates/docs for `.app`+notarise, MSIX, Flatpak/AppImage; or a `cargo-quadraui bundle` | M |
| **Low** | `thread_local!` config, `gtk-example` feature, 157 example stanzas, 507 root re-exports | misc | Per-backend config; drop alias feature; curate ~10 canonical examples, move the rest to `tests/` fixtures; tiered prelude | S–M |

### 7.2 Roadmap (dependency-ordered)

**Phase 0 — truth and a release (weeks, S–M).** Truth pass on README/CLAUDE/ARCHITECTURE/dispatch (S). Delete or complete the 131 deprecations (M). `#[non_exhaustive]` + `Default` on every primitive so fields can be added (M, one breaking change, do it *once*). Tag `v0.1.0`, publish to crates.io with `tui` default-off, host rustdoc (S). Gate macOS CI on every PR, not path-filtered (S). Win `SAFETY:` audit (M). Move process tests out of `cargo test` (S). *Exit criterion: an outside dev can `cargo add quadraui` and read hosted docs that match the code.*

**Phase 1 — the widget model (1–3 months, L–XL).** This is the fork in §2 and must be decided explicitly. (a) One logical-unit contract for `Rect` (L). (b) Finish `native_surface_paint` for the remaining 25 primitives; make `NativeSurface` the public backend seam; split `Backend` (XL, but each primitive is independent — parallelisable). (c) `Canvas` primitive (M). (d) Optional Taffy-based layout module (L). (e) Owned `Frame`/`Surface` and close the 14-primitive `Surface` gap (L) — this is the prerequisite for a11y, web, and bindings simultaneously. (f) `pump()`/`step()` non-blocking runner API (M) — prerequisite for Node and for embedding. *Exit criterion: an app can draw something the catalogue doesn't have, on a GUI backend, in logical pixels, laid out by flex, and the frame can be obtained as an owned value.*

**Phase 2 — the table stakes (3–6 months).** AccessKit from the owned `Frame` (XL across three backends; TUI N/A). IME (L). Multi-window (L). Rows provider + damage rects + benchmarks (L). Packaging templates (M). Second and third real consumers (the `kubeui` demo is 1.6k lines and last touched 2026-09-07 — it is not a consumer).

**Phase 3 — bindings and 1.0 (6–12 months).** §8. Then a stability pledge.

### 7.3 The 1.0 bar (what must be true before recommending it to an outside developer)
1. Published on crates.io with hosted docs; at least one minor version with zero breaking changes in the primitive descriptors.
2. AccessKit integration on GTK, macOS, Windows with a screen-reader smoke test in CI.
3. IME composition on all three GUI backends.
4. A public drawing surface (`Canvas` or `NativeSurface`) and a layout module.
5. `Rect` in one documented unit on every backend.
6. Windows gating in conformance and one shipped app on it.
7. At least two production consumers not owned by the maintainer, or one with real end users.
8. Benchmarks in CI for the 100k-row and 4k-line cases.
9. README, ARCHITECTURE and rustdoc verified against code in the same PR as changes (the current `readme_truth.rs` mechanism, aimed at the right claims).
10. A packaging guide per platform.

RTL/bidi can stay a documented non-goal past 1.0; a11y and IME cannot.

---

## 8. Language bindings

### 8.1 The design that unlocks all of them
Nothing bindable exists today: `render(&self, &mut dyn Backend)` is a push API over a sealed 132-method trait, `Surface<'a>` borrows, `UserPayload` is `Arc<dyn Any>`, and 14 primitives cannot be expressed as a `Surface` at all. **One change fixes this for every language:** a *retained descriptor store* behind an owned-frame API.

- Rust owns a `DescriptorStore`: `handle = store.insert(TreeView{…})`; `store.update(handle, |t| t.rows[3].text = …)`; `store.remove(handle)`. Primitives are already `Serialize + Deserialize` (BACKEND.md Contract A), so the wire form is JSON (or MessagePack/FlatBuffers later) with no new types.
- The foreign `render` returns a `Frame = Vec<(Handle, Rect)>` (plus z-order/modal flags) — a few dozen bytes, at event rate, not 60 Hz. Rust does `ScreenLayout::draw` and `hit_map` from the store.
- The foreign `handle(event)` receives `UiEvent` as JSON (already serde; `User` payloads become an opaque integer token the foreign side maps back).
- Threading: `run()` blocks on the calling thread and requires it to be the process main thread on macOS (already `MainThreadMarker::new().expect`), any thread on Win, the GTK-init thread on GTK. `waker()` is `Arc<dyn Fn + Send + Sync>` — a C function pointer + `void*` wraps it trivially.
- Re-entrancy: native modal loops (GTK dialog pump, `NSAlert runModal`) re-enter `handle`/`render`. The binding must not hold an exclusive borrow of the foreign app object across a `services()` call — a documented rule, or make dialogs async (`UiEvent::DialogClosed`) which is the better shape anyway.
- Per-frame cost: because descriptors live in Rust and the foreign side mutates them in place, a 100k-row table costs the foreign side nothing per redraw. This is the prior audit's "retained-data shim" — it is a retained *data* layer, not a retained *widget* layer, and it stays consistent with `UI_CRATE_DESIGN.md` §3.1 "no diffing". It is also exactly what `ROWS_PROVIDER_PROPOSAL.md` wants and what the web backend proposal needs. **Build it once, in Rust, for Rust consumers first.**

Preconditions (in order): tagged release; owned `Frame` + close the `Surface` gap; `pump()` step API; descriptor store. Then bindings are mostly mechanical.

### 8.2 C ABI — the foundation
- **Tech**: `cbindgen` over an `extern "C"` façade crate (`quadraui-ffi`). Opaque handles (`QuiApp*`, `QuiStore*`), UTF-8 JSON in/out for descriptors and events (`qui_store_set(store, handle, json, len)`), callbacks `struct QuiAppVTable { render(void*, QuiFrameBuilder*), handle(void*, const char* event_json, size_t), tick(...) }`, `qui_run(app, backend, config)`, `qui_step(app, timeout_ms)`, `qui_wake(app, token)`. Errors as `int` + `qui_last_error()`. Strings: callee-allocated, `qui_free`.
- **Threading**: `qui_run` blocks on the calling thread (must be main on macOS). `qui_wake` is the only thread-safe entry.
- **Ownership**: descriptors owned by Rust, referenced by integer handles; frames are built through a builder so nothing is allocated across the boundary; events are borrowed for the callback's duration.
- **Immediate vs retained**: fully absorbed by the store; the per-redraw FFI cost is one `render` callback that emits N `(handle, rect)` pairs.
- **Packaging**: static + shared lib per triple via CI; a header.
- **Verdict: worth it — but only after §8.1.** Effort L. It is the base for C#, Go, C++, Zig, Java (Panama) and a fallback for everything else. Alternative: **Diplomat** (unicode-org) generates C/C++/JS/Dart from Rust and handles opaque types and callbacks well; worth a spike. **UniFFI** (Mozilla) would give Python/Kotlin/Swift/Ruby from one proc-macro description and supports callback interfaces and blocking calls; it is a strong second option if the goal is "four languages cheaply" rather than "best DX per language".

### 8.3 C++
- **Tech**: header-only RAII wrapper over the C ABI (or `cxx` for a richer bridge). Callbacks via `std::function` stored in the `void*`.
- **Threading/reentrancy/ownership**: identical to C.
- **Packaging**: CMake/vcpkg/Conan port carrying the prebuilt static lib.
- **Verdict: maybe later, cheap once C exists (S–M).** C++ desktop devs have Qt; the pull is weak unless they specifically want the terminal twin.

### 8.4 C# / .NET
- **Tech**: `csbindgen` (generates P/Invoke from the Rust FFI source) or hand-written `[LibraryImport]` with source generators; `UnmanagedCallersOnly` delegates for the vtable; `GCHandle` for the app object. Consider NativeAOT-compat from day one (no reflection).
- **Threading**: a console-app `Main` thread is the process main thread — satisfies AppKit and Win32. `[STAThread]` on Windows for dialogs/clipboard OLE. Async: .NET's `SynchronizationContext` can be bridged over `qui_step` (a `QuadraSynchronizationContext` that posts to `qui_wake`), giving `await` on the UI thread — a big DX win.
- **Ownership**: handles as `SafeHandle`; JSON via `System.Text.Json` source-gen (AOT-safe). Immediate/retained: absorbed by the store.
- **Packaging**: single NuGet with `runtimes/{win-x64,win-arm64,osx-arm64,osx-x64,linux-x64}/native/` — well-trodden (SkiaSharp, Avalonia native libs do exactly this).
- **Verdict: worth it, second binding after Python.** The Win backend + WinForms/WPF refugees + LINQPad/PowerShell-tool authors are the strongest *commercial* fit, and .NET's native-interop tooling is the best of any managed runtime. Effort M once C ABI exists.

### 8.5 Java / JVM (and Kotlin)
- **Tech**: Panama FFM (JDK 22+ final; `jextract` from the cbindgen header) — no JNI glue. Upcalls via `Linker.upcallStub`. Kotlin/JVM reuses it; Kotlin/Native could use cinterop directly on the C header.
- **Threading — the real problem**: on macOS the JVM's `main` does not run on the process main thread unless launched with `-XstartOnFirstThread` (SWT, LWJGL, and JavaFX-without-Glass all hit this). `qui_run` would have to `expect` that flag, or the binding must ship a launcher/`Info.plist`. On Windows/GTK, any dedicated thread works. Interaction with Swing/JavaFX EDT is out of scope (you would not mix them).
- **Ownership**: `Arena` scoping for JSON buffers; handles as `long`. GC pauses do not affect Rust-owned descriptors — another argument for the store.
- **Packaging**: JAR with `natives/{os}-{arch}/` + a loader (LWJGL pattern), or per-platform classifier JARs; jlink/jpackage for apps.
- **Verdict: maybe later.** JVM desktop is small and served by JavaFX/Compose Multiplatform; the terminal twin is the only pull (Java ops tools exist). Kotlin via UniFFI is cheaper if UniFFI is chosen for Swift/Python anyway. Effort M.

### 8.6 Python
- **Tech**: PyO3 + maturin directly on the Rust crate (not via the C ABI) — best ergonomics, `#[pyclass]` wrappers around store handles with property setters that mutate in place (`tree.rows[3].text = "…"`), `pythonize` for events, abi3 wheels.
- **Threading**: a script's main thread is the process main thread; `quadraui.run(app)` from top level satisfies AppKit/GTK. Release the GIL inside `run` and re-acquire per callback (`Python::with_gil` in the vtable shims); ~1–3 µs per callback, at event rate — fine. Python threads doing work use `app.wake(token)`; asyncio integration via `qui_step` in a custom event-loop policy (like `qasync`) is the deluxe version.
- **Reentrancy**: nested native modal loops re-enter Python; safe as long as the binding never holds a `PyRefMut` across a services call (document, or make dialogs async).
- **Ownership**: descriptors Rust-owned; Python holds handles; `__del__` → `store.remove`. Per-frame cost: the `render` callback returns a list of `(handle, rect)` tuples — tens of µs.
- **Packaging**: `maturin` wheels for macOS arm64/x64, Windows x64, manylinux x64/arm64; the `gtk` feature cannot ship in a manylinux wheel (system GTK 4.10) — Linux wheels ship TUI-only or require a system package. That is a real DX wart; Textual has no such problem.
- **Prior blockers revisited**: API stability — still unsolved, still the adoption blocker; immediate-mode rebuild cost — solved by the store; retained-data shim — *is* the store; 11-primitive `Surface` gap — now 14, must close; `render` push API — solved by owned `Frame`. All addressable; none addressed yet.
- **Verdict: worth it, first high-level binding.** Largest adoption pool, best tooling, and the terminal-twin story competes head-on with Textual, which is the one incumbent quadraui can plausibly beat on native rendering. Effort: 2-week spike after §8.1, then months of surface maintenance. Success test unchanged from the prior audit: `appshell_demo` in ~80 lines of Python scrolling 10k rows without jank.

### 8.7 TypeScript — Node, Deno, Bun (the Electron audience; evaluated seriously)
- **Why it matters**: the entire `GOAL.md` premise is "teams reach for Electron for the capability box". Those teams write TypeScript. A Rust-only quadraui does not compete for them at all; a TS binding is the only way the Electron comparison becomes more than a benchmark.
- **Tech**: `napi-rs` (Node/Bun via Node-API compat; Bun also has `bun:ffi`), Deno via `Deno.dlopen` + `UnsafeCallback` on the C ABI. napi-rs is the mature path with prebuilt-per-platform npm packages (`@quadraui/core-darwin-arm64` etc. — the `@swc/core`/`@napi-rs/canvas` pattern).
- **Threading — the hard part, and it is solvable**: Node owns the main thread with libuv's loop; a blocking `qui_run` on that thread freezes JS, and moving quadraui to a worker thread is illegal on macOS (AppKit main thread) and awkward on GTK. Electron itself solved exactly this by integrating libuv into the native message pump (`MessagePumpMac`/`NodeBindings`). The quadraui equivalent: **`qui_step()`** — run one iteration of the native loop (`nextEventMatchingMask` with a short timeout / `g_main_context_iteration` / `PeekMessage`) from a libuv `uv_idle`/`uv_timer`/`uv_prepare` hook, and wire `waker` to `uv_async_send`. Latency is bounded by the poll interval (Electron uses the same trick with a blocking `kqueue` on the uv fd — the polished version feeds the uv fd into the native loop instead). This is Phase 1(f) and is the single most valuable API addition for embedding generally.
- **Callbacks/reentrancy**: `render`/`handle` are synchronous JS calls on the main thread via `napi_call_function`; `ThreadsafeFunction` only for `waker`. Nested native modal loops would re-enter JS from inside a JS call — legal in N-API but must not run while a `HandleScope` is held wrongly; napi-rs handles this. Async dialogs (`DialogClosed` event) again preferable.
- **Ownership**: JS holds handles (`FinalizationRegistry` → `store.remove`), descriptors in Rust; `render` returns an array of `[handle, x, y, w, h]` — trivially cheap. Immediate-mode with JS object churn would have been the deal-breaker; the store removes it.
- **Packaging**: napi-rs prebuilds; `gtk` feature same Linux wheel problem as Python (system GTK); TUI-only Linux by default is acceptable for the "CLI tool with a GUI twin" audience.
- **What TS devs will miss**: JSX/React-style composition (you would build a thin declarative wrapper — the store plus JSON descriptors map naturally onto a `createElement`-like builder, and a React reconciler for it is a weekend hack because there is no diffing needed on the Rust side), CSS, hot reload, DevTools. The widget catalogue gap (§2) bites hardest here: an Electron dev's first question is "how do I draw my thing", and the answer today is "you cannot".
- **Verdict: maybe later — strategically the most important, technically gated on `qui_step()` and on the widget-model work.** Do not attempt with the blocking `run()`. Effort L after §8.1 + `step()`. If the owner's Electron framing is sincere, this binding, not tray icons, is the proof.

### 8.8 Go
- **Tech**: cgo over the C ABI; `//export` callbacks; `runtime.LockOSThread()` in `init()` so the main goroutine stays on the main OS thread (documented Go requirement for Cocoa; the same pattern as `go-gl/glfw`, Fyne, Wails). Wails/Fyne/gioui already solve exactly this loop-ownership problem in Go; copy their shape.
- **Ownership**: handles as `uintptr`; `cgo.Handle` for the app object; JSON via `encoding/json` — fine at event rate. cgo call overhead ~100 ns–1 µs; irrelevant.
- **Packaging**: cgo means a C toolchain on every build machine and cross-compilation pain (`zig cc` helps); alternatively `purego` (no cgo) can call the C ABI dynamically but callbacks are harder. Prebuilt static libs per platform vendored in the module.
- **Verdict: maybe.** Go devs write CLIs and ops tools — the TUI twin is a real pull (Bubble Tea is the incumbent). Effort M. Lower priority than Python/C#/TS.

### 8.9 Swift
- **Tech**: Swift imports the cbindgen header via a module map with no glue at all; `swift-bridge` for a richer typed API. Callbacks as `@convention(c)` closures with `Unmanaged` context.
- **Threading**: natural — Swift apps' `main` is the main thread; `@MainActor` maps onto it.
- **Packaging**: SwiftPM binary target (`.xcframework`) — clean on Apple platforms; Linux/Windows Swift is niche.
- **Verdict: no / opportunistic.** A Swift developer on macOS has SwiftUI/AppKit and gets nothing from quadraui except a terminal twin and Linux/Windows ports through a weak Swift toolchain there. If UniFFI is chosen for Python/Kotlin it comes nearly free; otherwise skip.

### 8.10 Others worth naming
- **Lua (`mlua`)** — *unusually good*: the primitives were designed with "Lua plugins" in mind (prior audit), vimcode is an editor, and an embedded scripting layer that builds descriptors is the Neovim-plugin story. Small, in-process, no packaging problem. Worth doing early for vimcode's own benefit.
- **Ruby (`magnus`)** — cheap after PyO3; tiny audience.
- **Zig** — consumes the C header directly with zero work; small but enthusiastic TUI/native audience.
- **Dart/Flutter** — no (Flutter is the competitor, not a host).
- **Elixir/Erlang** — bad: a blocking `run` cannot live in a NIF; would need a port/OS process with a wire protocol — which the web backend proposal's WebSocket-of-HTML approach is closer to.
- **WASM/browser** — not a "binding"; it is the `quadraweb` proposal (server-rendered HTML over WebSocket). A TS *renderer* in the browser was considered and rejected there; that decision stands.

### 8.11 Recommended strategy and order
1. **Phase 1 in Rust first** (owned `Frame`, `Surface` gap, descriptor store, `step()`). Zero bindings before this; every binding written against the current `Backend` shape will be thrown away.
2. **Python via PyO3** — adoption, tooling, Textual as the beatable incumbent.
3. **C ABI via cbindgen** (spike Diplomat first) — the durable foundation.
4. **C# via csbindgen + NuGet** — commercial fit, Windows backend proof.
5. **TypeScript via napi-rs** once `step()` is real — the Electron audience; the strategic one.
6. **Go via cgo**, **Java via Panama**, **C++** wrappers as demand appears — each S–M over the C ABI.
7. **Lua** any time, for vimcode.
Swift, Ruby, Zig: opportunistic.

Honest caveat: bindings multiply the cost of every breaking change by the number of languages. Do not start #2 until the primitive descriptors have survived one minor release unchanged.

---

## 9. What I could not verify
- Test-suite pass/fail and duration on this machine beyond §3.1; Windows tests not run (no Windows host).
- GTK tray (`gtk/tray.rs` absent — presumably `Unsupported`; not confirmed by reading).
- Frameless-window behaviour per backend behind `with_client_side_titlebar`.
- Whether the 6 non-test `todo!()` in `win/run.rs` are all `cfg(not(windows))` stand-ins (the surrounding comments say so; I did not read each).
- `CreateMutexW` handle lifetime (`win/run.rs:941`): leak by design (process-lifetime single-instance mutex is conventional) or oversight.
- Visual quality at fractional HiDPI scales; text rendering quality generally (no screenshots taken).
- Performance at 100k rows / 4k-line editor (no benchmarks exist; I did not write one).
- `keyring`/`trash` linkage implications for static musl TUI builds.
- `cargo doc -D warnings` cleanliness for the `macos` feature (CI does not check it).
- G11–G15 of the prior audit (badge, global shortcuts, power, drag-out, print) — not re-grepped.
- GitHub issue text — `gh` was not used per the read-only brief; issue numbers are quoted from code comments only.
