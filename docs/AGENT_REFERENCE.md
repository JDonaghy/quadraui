# Agent reference — quadraui

Reference material moved out of the repo-root `CLAUDE.md` so it is not
re-read on every agent turn. `CLAUDE.md` keeps the rules; this file keeps the
rationale, the war stories and the long-form playbooks behind them. **Read a
section only when your task touches it.**

## Background reading (on demand only)

None of these is required at session start — each is large (DECISIONS.md alone
is ~150 KB). Open one when the task needs it:

- `README.md` — workspace shape, primitives, status.
- `quadraui/docs/decisions/DECISIONS.md` (primitive-distinctness principles) and
  `decisions/BACKEND_TRAIT_PROPOSAL.md` §4 / §9 (Backend trait shape, resolved decisions).
- `quadraui/docs/ARCHITECTURE.md` — workspace layout, two-layer split, compose
  helpers, GTK hosting helpers, backend trait.
- `quadraui/docs/PRIMITIVE_RULES.md` — the rules for adding/changing primitives
  + maturity levels. **Rule 8 (public-API lifecycle) before removing or renaming
  anything `pub`.**
- `quadraui/docs/CONSUMER_PATTERNS.md` — MSV debug-sidebar and SC panel recipes.
- `quadraui/docs/TESTING.md` — coverage taxonomy, backend testability
  requirement, the Win-GUI playbook.
- `quadraui/docs/LESSONS.md` — durable rules from real failures + "What NOT to do."

## TUI is not a second-class backend — the full rationale

**An app written on quadraui must actually be usable on TUI.** Menus, file and
folder pickers, message dialogs, buttons, text boxes, lists — all of it has to
function. It does not have to look like the GUI. It has to *work*. vimcode's TUI
build is the existence proof: it drives the same `AppShell`, activity bar and
omnibar as the GUI backends.

So `Unsupported` / `None` is **not** an acceptable answer for a capability the
user is meant to interact with. Reserve it for things that are *physically*
absent on a terminal — a tray icon, a dock badge, an OS global shortcut. There
is nothing to degrade to for those. There is always something to degrade to for
a dialog.

**The crate owns the degrade, not the app.** A `PlatformServices` method that
returns `None` on TUI and tells the app to "provide an in-canvas picker instead"
pushes the work onto every consumer — the duplication this crate exists to
prevent. The degrade belongs in `compose/`, behind the same call, so one
app-side call site gets a native dialog on GUI and an in-canvas one on TUI.

`compose::FolderPickerController` is the pattern done right: it began as
vimcode's TUI-local `FolderPickerState` and was lifted here verbatim, so today
vimcode drives one backend-neutral picker on every backend and branches nowhere.
Copy that shape. What is still missing is a **file** open/save picker (only the
folder one exists) and a **message-dialog** controller — `primitives/dialog.rs`
paints, but nothing drives the show-block-return-a-choice contract
`show_message_dialog` has. Until those land, `show_file_open_dialog`,
`show_file_save_dialog` and `show_message_dialog` are GUI-only.

A synchronous in-canvas degrade is not a novelty: GTK's own
`show_message_dialog` already pumps its loop (`pump_until_ready`) to make a
native dialog block. A TUI nested draw-and-read loop is the same shape.

When adding any capability, state its TUI story explicitly and ship a `tui_*`
test for it. "Terminal, so no" is a conclusion that has to be *earned* per
capability, not assumed.

## Downstream consumers — history and CI-job details

### Why the delivery rule exists (#476)

#476 ("de-coord board.rs") replaced `Stage` with `CardBadge`, renamed `BadgeStatus::RequestChanges` → `Warning`, deleted two `BoardCard` fields and `BoardAction`'s domain verbs — all correct as *design*. Both consumers broke on 2026-08-05 and coord-tui needed a migration PR (`claude-coordinator#1864`). The change shipped believing it was safe partly because CLAUDE.md then claimed consumers "pin a published version externally." At the time they did not (both built `develop`'s tip); today vimcode takes crates.io `0.1.x` and coord-tui a git rev.

**The design rule and the delivery rule point in opposite directions, and both hold.** Keep one consumer's vocabulary *out* of the primitives (that is what #476 was fixing, and it was right). Keep both consumers' *compile status* in mind while landing it.

### The `downstream` CI job (#528)

`ci.yml`'s `downstream` job `cargo check --all-targets`s each consumer against
the PR's quadraui, with a control run against `develop`'s tip so pre-existing
consumer breakage doesn't fail quadraui's own CI. It catches "doesn't compile"
before merge — not "compiles but does the wrong thing."

`JDonaghy/coord-tui` (split out of the coordinator repo by
`claude-coordinator#2899`, 2026-08-29) is **not anonymously readable**, so the
coord-tui leg only runs when a repo secret `COORD_TUI_TOKEN` (read-only,
`contents:read`) is present; without it that leg skips with a loud
`::warning::`. For coord-tui the job overrides its git-rev pin with an absolute
`.cargo/config.toml` `paths` override onto the PR's checkout.

Three details in that job are load-bearing and easy to "tidy" into a
permanently-green no-op (`tools/lint/src/downstream_gate_docs.rs` enforces them):

- **Each cargo step `cd`s into the consumer** (`working-directory:`), never `cargo check --manifest-path …` from the workspace root. Cargo finds `.cargo/config.toml` by walking up from the *process CWD*, not from `--manifest-path`'s directory, so the coord-tui `paths` override is silently discarded by the `--manifest-path` form and the check quietly builds the pinned git rev instead of the PR. A `cargo metadata` assertion step fails the job loudly if that ever regresses.
- **`--all-targets`, not a bare `cargo check`.** quadraui's most-consumed public surface, `tui::testing::{TuiDriver, driver_with_shell}`, is referenced only from coord-tui's *test* targets, which a bare `cargo check` never compiles.
- **`RUSTFLAGS: ""` overrides the workflow-level `-D warnings`**, so a `#[deprecated]` shim (CLAUDE.md's `pub`-change rule 3) stays green downstream (see the *deprecated lint* note below). The features are each consumer's own default (`tui,terminal` for coord-tui via its dep line; `gui` for vimcode, which is why the job apt-installs GTK4 — that also buys consumer compile-truth for quadraui's GTK backend).

### The `deprecated` lint: denied in-repo, allowed downstream

`ci.yml` sets `RUSTFLAGS: "-D warnings"` workflow-wide, so the instant PR 1 lands, `#[deprecated]` turns every remaining in-repo call site into a build failure. That split is intentional, not a bug to "fix" by relaxing this repo's lint:
- **In-repo (this repo's `ci.yml`): `deprecated` stays denied.** quadraui migrates its own call sites (examples, `kubeui*` demo apps, tests) in the *same* PR that adds the `#[deprecated]` attribute — PR 1 doesn't merge with a warning still live in-tree. That's what forces the shim to actually compile clean here rather than just existing on paper.
- **Downstream (the consumer lint gate, #543): `deprecated` is allowed.** A consumer mid-migration is expected to keep calling the old shape for a while after PR 1 merges — that's the whole point of deprecate-then-remove instead of a hard break. If the downstream gate denies `deprecated` too, a rule-3-compliant PR 1 turns both consumers' CI red on merge, which is exactly the failure this rule exists to prevent, and indistinguishable from just breaking the API outright. Worse: the non-compliant path (skip the deprecation shim, break it directly) would stay green under a `-D warnings` downstream gate, since there's no warning to deny — so a strict downstream gate quietly *punishes* following this rule and rewards skipping it.
- Consequence: **a `deprecated` warning must never fail CI in `coord-tui` or `vimcode`** on account of a quadraui shim. If it does, the downstream gate has drifted from this policy — fix the gate (#543), don't stop deprecating.

## Quality gate — the annotated CI matrix

These are the commands `.github/workflows/ci.yml` runs (the repo-lint
`quality_gate_docs` check keeps `CLAUDE.md`'s copy in sync with ci.yml).

```bash
# tui leg — kubeui-gtk is EXCLUDED (it has no `tui` feature, and its
# unconditional gtk4/pangocairo dep needs pkg-config + the GTK4 -dev
# packages). Excluding it is what lets the whole tui gate run on a
# machine with no GTK toolchain installed.
cargo build  --features tui --workspace --exclude kubeui-gtk
cargo test   --features tui --workspace --exclude kubeui-gtk
cargo clippy --features tui --workspace --exclude kubeui-gtk -- -D warnings

# gtk leg — kubeui (the TUI demo binary) is excluded instead. Needs
# pkg-config + libgtk-4-dev; skip this leg locally if you don't have them
# and let CI's gtk job cover it.
cargo build  --features gtk,tui --workspace --exclude kubeui
cargo test   --features gtk,tui --workspace --exclude kubeui --no-fail-fast
cargo clippy --features gtk,tui --workspace --exclude kubeui -- -D warnings

# win leg — no GTK, no Windows host required. `src/win/` is compiled on
# every platform (only its WinAPI calls are `cfg(target_os = "windows")`),
# so both of these run anywhere. They are a TYPE-CHECK of the
# `cfg(target_os = "windows")` arms, not a test of them: on Linux every
# real WinAPI call compiles to its `todo!()` fallback body. If your diff
# touches `src/win/`, these two passing means nothing about whether your
# rasteriser works — see "Win-GUI: building and testing for real" below.
cargo check -p quadraui --features win
cargo test  -p quadraui --features win

cargo fmt --all --check
```

**Do NOT substitute a bare `cargo test --features tui` at the workspace
root.** It selects every member, so it drags in `kubeui-gtk` → `gtk4` →
`glib-sys` → `pkg-config`, and on any machine without those system
packages it fails in a build script before compiling a single line of
quadraui — a failure that says nothing whatsoever about your diff. That
is why CI spells out `--workspace --exclude kubeui-gtk`, and why you
should too.

## Win-GUI: building and testing for real

`dell64` (WSL2 on a Windows 11 host) cross-compiles `x86_64-pc-windows-msvc`
via `cargo-xwin` and **runs the resulting `.exe` on its own Windows host** through
WSL interop — against a live Direct2D stack, not a stub. If your issue is in the
Win-GUI milestone you are dispatched there so you can run your own code; do not
settle for `cargo check`.

```bash
# build
RUSTFLAGS="-C target-feature=+crt-static" \
  cargo xwin build --target x86_64-pc-windows-msvc -p quadraui --features win
```

Test with **`tools/win-test.sh`** (forwards its args, e.g. `--test <name>`), never
by retyping the env-var prefix — it sets `RUSTFLAGS`, `RUSTDOCFLAGS` (the doctest
leg needs it separately) and the runner override. The canonical spelled-out
command lives in `quadraui/docs/TESTING.md`'s Win-GUI playbook. None of these
traps produces an error that points at its own cause:

1. **`-C target-feature=+crt-static`.** The host has no `vcruntime140.dll` (no VC++
   redistributable; installing one needs a UAC click at the console). Without it the
   `.exe` exits **53 with completely empty output** — indistinguishable from a
   program that ran and printed nothing.
2. **`CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUNNER=env`.** `cargo xwin test` silently
   injects a `wine` runner and dies with `could not execute process 'wine ...'`.
   Override it and `binfmt_misc` (registered for the PE `MZ` magic) does the exec
   directly; wine is never needed. stdout and exit codes round-trip intact.
3. **CWD becomes `C:\Windows`.** Launching a PE from a WSL path prints `UNC paths
   are not supported. Defaulting to Windows directory.`, so relative paths in tests
   resolve there. Use `CARGO_MANIFEST_DIR` or absolute paths.
4. **`WNDCLASSW` / `RegisterClassW` need the `Win32_Graphics_Gdi` feature**, not just
   `Win32_UI_WindowsAndMessaging` — the struct carries `HBRUSH`/`HICON`. The error is
   a bare "cannot find struct ... in this scope", which does not name the feature.

**No interactive Windows desktop is required** for rasteriser work. `src/win/testing.rs`
`HeadlessSurface` is `ID2D1DCRenderTarget` + `CreateDIBSection` with
`D2D1_RENDER_TARGET_TYPE_SOFTWARE` (WARP): real Direct2D, no HWND, no GPU, no session.
That is what the milestone's paint↔click round-trip tests run against, and it works
from a headless shell. Only a live-HWND GUI smoke needs a desktop, and that tier is
operator-run.

**Scope warning for every rasteriser issue in this milestone.** They all replace
`todo!()` stubs in the *same* file, `quadraui/src/win/backend.rs`. Touch only the
`draw_*` methods your issue names — a drive-by fix to a neighbouring stub is what
turns a serialized lane into a rebase conflict.

## Reference consumer: vimcode — where each pattern was prototyped

Historical map (line counts and paths date from the extraction era and may have
moved since — verify in `~/src/vimcode` before relying on one).

**vimcode is quadraui's primary consumer and R&D lab.** Every primitive, rasteriser, hit_test pattern, and compose helper in quadraui was first prototyped as per-backend code in vimcode, then extracted. When building new quadraui features — especially the runtime epics (#202 GTK, #203 TUI, #204 macOS) — **read vimcode's existing implementation first:**

| quadraui feature | vimcode reference code |
|-----------------|----------------------|
| `Backend::draw_frame()` (#199) | `src/gtk/draw.rs::draw_editor()` — 3874-line orchestration function that calls each `draw_*` in z-order. This is the spec for what `draw_frame` must do. |
| `FrameHitMap` / unified click dispatch (#197, #198) | `src/gtk/click.rs::pixel_to_click_target()` — zone detection pipeline using `screen_zone_hit_test` + `window_zone_hit_test`. Shows every click zone the hit map must cover. |
| GTK widget tree (#202 Stage 1) | `src/gtk/mod.rs::fn init()` (~2122 lines) — creates every GTK widget, event controller, and draw closure. This is the mechanical boilerplate `AppShell` must generate. |
| Event wiring (#202 Stage 2) | `src/gtk/mod.rs::fn init()` event controller blocks + `enum Msg` (~333 variants) + `fn update()` (~736-line dispatch). Shows every GDK event type that must be translated. |
| TUI event loop (#203) | `src/tui_main/mod.rs` — crossterm poll loop, `handle_mouse()` dispatch, `draw_frame()` calls. Same structure the TUI runtime must own. |
| Cached layout hit-test pattern | `CompletionsLayout::hit_test()`, `ContextMenuLayout::hit_test()`, `BottomPanelGeometry` + `resolve_bottom_panel_zone()` — all proven in vimcode Sessions 379. Cache at paint, hit-test at click. |
| SidebarSystem GTK rasteriser (#200) | `src/gtk/draw.rs::draw_source_control_panel()` (405 lines) — bespoke Cairo rendering that should delegate to quadraui. TUI already delegates via `SidebarSystem`. |
| Per-panel handlers (#202 Stage 5) | `src/gtk/mod.rs::handle_*_msg()` functions (~1500 lines total) — explorer, SC, extensions, debug, settings, terminal, AI, dialog. Shows what engine methods the runtime must call. |

**How to use this:** Before implementing a quadraui runtime feature, `cd ~/src/vimcode` and read the corresponding backend code. The vimcode implementation is the working prototype — extract the pattern, don't reinvent it. The goal is that vimcode's `src/gtk/` shrinks from 16K lines to ~60 lines as each stage lands.
