# Kitty keyboard protocol: detection and degrade table

quadraui#827 ("Terminal-honest #3"). Read this when:

- a user reports a gesture that needs unambiguous modifier keys (e.g.
  Ctrl+Enter distinct from plain Enter — see `src/compose/chat_controller.rs`)
  silently doesn't fire on their terminal;
- you're deciding whether an app can rely on the protocol for a gesture,
  and want to know what `BackendCaps::kitty_keyboard` actually promises;
- you're adding a new terminal/environment to the table below and need to
  know the bar for a new row (a pty fixture, or an honest "untested").

## The bug this closes

[kitty's progressive keyboard enhancement protocol](https://sw.kovidgoyal.net/kitty/keyboard-protocol/)
lets a terminal report modifier keys and key-release events unambiguously
— the mechanism behind gestures like Ctrl+Enter being distinct from plain
Enter. `crate::tui::run::run` has always pushed it on a best-effort basis.
Before this issue, the *result* of that attempt was consulted only to
decide whether to pop the flags on exit — an app had no way to read it at
all. A gesture built on the assumption the push worked simply never fired
on a terminal where it didn't, with nothing anywhere to tell the app (or
the user) why. That is the "silently degrades" failure quadraui#787's
Terminal-honest epic exists to close for this repo's terminal-facing
surfaces, one protocol at a time.

## How detection works

Two layers, in `quadraui/src/tui/caps.rs`:

1. **`detect_kitty_keyboard()`** (and its pure, testable core
   `detect_kitty_keyboard_from`) — an instant, environment-only heuristic.
   Checked in order:
   1. `TERM` contains `kitty`, or is exactly `foot`/`foot-extra`.
   2. `KITTY_WINDOW_ID` is set (kitty sets this unconditionally, even
      inside a session that has rewritten `TERM`).
   3. `TERM_PROGRAM` is exactly `WezTerm`.
   4. Otherwise `false` — deliberately never a hopeful guess. A terminal
      this heuristic can't name a specific, unconditional signal for
      (Alacritty, iTerm2 — both support recent-enough protocol versions,
      per the table below, but neither sets a version-independent
      environment marker) stays on this conservative path.

   `TuiBackend::new()` seeds `TuiBackend::kitty_keyboard` with this —
   cheap, no terminal I/O, safe to call from a test that never touches a
   real tty.

2. **`probe_kitty_keyboard()`** — the live, authoritative answer:
   [`ratatui::crossterm::terminal::supports_keyboard_enhancement()`],
   which performs the protocol's own [documented detection
   handshake](https://sw.kovidgoyal.net/kitty/keyboard-protocol/#detection-of-support-for-this-protocol) —
   write `ESC[?u ESC[c`, wait up to 2s for a reply — **falling back to
   `detect_kitty_keyboard()`'s heuristic only when the query gets no
   answer at all** (an `Err`, not an `Ok(false)`). `crate::tui::run::run`
   calls this once at startup and stores the result via
   `TuiBackend::set_kitty_keyboard`, overwriting the constructor's
   environment-only guess — this is what `BackendCaps::kitty_keyboard`
   ultimately reports, and what an app should check before relying on a
   gesture that needs the protocol.

   The fallback is the actual fix, not a decoration. A terminal that
   genuinely supports the protocol but sits behind something that
   swallows the query/response round trip (a multiplexer without
   passthrough configured, a lossy relay) makes the live probe time out
   with *no* answer — collapsing "no answer" to "unsupported" (this
   crate's behaviour before #827, via `.unwrap_or(false)`) is
   indistinguishable from a terminal that was asked and genuinely said no.
   The environment heuristic is what recovers the cases it can.

   **One case the fallback does *not* recover, verified from source, not
   guessed:** on Windows, `ratatui::crossterm`'s Windows implementation of
   `supports_keyboard_enhancement()` is
   ```rust
   // crossterm-0.29.0/src/terminal/sys/windows.rs
   pub fn supports_keyboard_enhancement() -> std::io::Result<bool> {
       Ok(false)
   }
   ```
   — an unconditional `Ok(false)`, not an `Err`. Because it's a definite
   `Ok`, quadraui's fallback never triggers on Windows, regardless of
   `TERM`/`TERM_PROGRAM` or which terminal is actually hosting the
   session. See the Windows Terminal row below.

## Degrade table

Every row states what `BackendCaps::kitty_keyboard` resolves to and how
that's known — a pty fixture where one exists, an explicit "untested"
where it doesn't. No row asserts a behavior nobody checked.

| Environment | What decides it | `kitty_keyboard` | Verified by |
|---|---|---|---|
| Raw terminal, real kitty (interactive session, query actually answered) | Live probe answers `Ok(true)` directly — the heuristic is never consulted | `true` | **Untested.** No real `kitty` binary is available to this repo's CI to drive an interactive session against. The *heuristic's* positive signal for kitty (`TERM=xterm-kitty` / `KITTY_WINDOW_ID`) is unit-tested (`caps::tests::xterm_kitty_term_is_detected`, `kitty_window_id_is_detected_even_under_a_rewritten_term`) and pty-fixture-tested for the *fallback* path — see the next row. |
| Raw terminal, `TERM=xterm-kitty` but the query goes unanswered (a scripted/harness pty, or a real kitty session behind something that drops the reply) | Live probe times out (`Err`) → falls back to the heuristic, which says `true` | `true` | **pty fixture:** `tests/tui_pty_smoke.rs`, `kitty_keyboard_protocol::tui_pipeline_under_kitty_term_pushes_enhancement_flags`. This is the exact fixture shape a synthetic pty can drive: our harness's simulated terminal never answers the live query (it only answers the unrelated cursor-position query), so this row is what's actually observable from a controlled test — see that test's doc for why it exercises the *fallback*, not the *live-answered*, path. |
| Raw terminal, WezTerm | Heuristic: `TERM_PROGRAM=WezTerm` | `true` | **Unit-tested only** (`caps::tests::wezterm_term_program_is_detected`) — not pty-fixture-verified; no WezTerm binary in this repo's CI. |
| Raw terminal, foot | Heuristic: `TERM=foot`/`foot-extra` | `true` | **Unit-tested only** (`caps::tests::foot_term_is_detected`) — same reason as WezTerm. |
| Raw terminal, Alacritty ≥0.12 (recent enough to support the protocol per crossterm's own `PushKeyboardEnhancementFlags` doc) | No heuristic signal (`TERM=alacritty` alone is not treated as positive — see `detect_kitty_keyboard_from`'s doc for why); relies entirely on the live probe | `true` if the live probe is actually answered, `false` otherwise | **Untested.** No Alacritty binary in this repo's CI. |
| Raw terminal, plain `xterm`/`xterm-256color` with no kitty/WezTerm/foot signal (the common case — a stock SSH session to an unconfigured box) | Live probe unanswered → heuristic has no positive signal | `false` | **pty fixture:** `tests/tui_pty_smoke.rs`, `kitty_keyboard_protocol::tui_pipeline_under_plain_term_does_not_push_enhancement_flags`. |
| tmux, no passthrough configured | tmux does not forward the protocol's query/response by default (the same class of gate `docs/CLIPBOARD.md`'s tmux section documents for OSC 52 — `allow-passthrough`), so the live probe gets no answer through tmux; tmux typically rewrites the inner `TERM` to `screen-256color`/`tmux-256color`, defeating the `TERM`-based heuristic signals even when the outer terminal is kitty/WezTerm/foot. `KITTY_WINDOW_ID` may or may not survive into the tmux session depending on tmux's `update-environment` config and how the session was created. | `false` in the common (unconfigured) case; `true` only if `KITTY_WINDOW_ID` happens to survive | **Untested.** No tmux available to this repo's CI to drive an automated fixture; documented from tmux's own passthrough mechanism, not independently confirmed here. |
| mosh | mosh's own protocol does not reliably forward arbitrary escape-sequence round trips; mosh sessions commonly present `TERM=xterm-256color` with no kitty-specific signal | `false` in the common case | **Untested.** No mosh available to this repo's CI. |
| Windows Terminal (or any Windows console host) | `ratatui::crossterm`'s Windows `supports_keyboard_enhancement()` is an unconditional `Ok(false)` (see the source excerpt above) — a definite answer, so quadraui's environment-heuristic fallback never runs | `false`, always, regardless of `TERM`/`TERM_PROGRAM` or whether the hosting terminal has any real support | **Verified from the vendored dependency's source** (`crossterm-0.29.0/src/terminal/sys/windows.rs`), not a pty fixture — this repo's Linux CI cannot spawn a Windows console to exercise it end to end, but the fact that the fallback can never trigger there is a property of code this repo depends on, not a guess. |
| Terminal.app (macOS's built-in terminal) | Uses the same unix live-query + heuristic path as any other unix terminal. Terminal.app does not implement the kitty protocol (a widely documented terminal-capability fact — e.g. the protocol's own ["which terminals support this"](https://sw.kovidgoyal.net/kitty/keyboard-protocol/) list and kitty/neovim's compatibility notes do not include it), so the live query gets no reply; `TERM_PROGRAM=Apple_Terminal` is not one of the heuristic's three positive signals either | `false` | **Untested by this repo's CI** (no macOS runner exercises this path here — `macos.yml` covers `src/macos/`, not the TUI backend running under Terminal.app). The "Terminal.app doesn't support this" premise itself is not independently re-verified in this repo; treat it as a documented-but-unconfirmed-here claim, not a pty-backed one. |
| Serial console | No special-cased detection — same live-probe + heuristic path as any terminal. A serial link's latency and duplex characteristics make a reliable query/response round trip within the 2s timeout terminal-and-link dependent | `false` in the conservative/common case (no heuristic signal, live probe likely unanswered or answered by a terminal with no kitty support) | **Untested.** No serial hardware or emulation available to this repo's CI. |

## What to do with a `false`

`BackendCaps::kitty_keyboard == false` does not mean "broken" — most rows
above are `false` by design or by a real environment limitation quadraui
cannot change. It means: don't build a gesture whose *only* binding needs
the protocol. `src/compose/chat_controller.rs`'s send-message binding is
the existing pattern to follow — Ctrl+Enter *and* Alt+Enter both send,
because `Ctrl+Enter` needs the protocol and `Alt+Enter` doesn't. Check
`backend.backend_caps().kitty_keyboard` (or the concrete
`TuiBackend::kitty_keyboard()` getter) from `AppLogic::setup` if you want
to adjust a hint/help string rather than just keeping a redundant binding.
