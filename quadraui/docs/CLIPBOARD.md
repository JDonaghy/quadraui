# Clipboard: how copy reaches the system clipboard (and when it doesn't)

Scope: the **TUI backend**. GTK, macOS and Windows talk to a real
native clipboard API, so none of the caveats below apply there — a
`Ctrl-C` copy in those backends either works or errors, with nothing in
between. The TUI backend has no native clipboard API to call; it has to
push bytes at whatever is on the other end of the terminal, and *that*
is where the failure modes live.

Read this when:

- a user reports "Ctrl-A then Ctrl-C highlighted the text and showed
  `Copied:` in the status bar, but pasting elsewhere gives me the *old*
  clipboard contents" — the classic symptom (quadraui#331);
- you are adding a clipboard path to a new backend and want to know
  which of these legs is load-bearing;
- you are writing a terminal-side smoke test and need to know what a
  green run actually proves.

## The copy path, end to end

1. `Ctrl-C` with an active text selection is intercepted in
   `runtime.rs::preprocess_event` (step 4). It never reaches
   `AppLogic::handle` as a key press.
2. The runtime calls `backend.services().clipboard().write_text(&text)`.
3. It then delivers `UiEvent::TextCopied(text)` so the app can show a
   copy confirmation.

**Step 3 is not evidence that step 2 worked.** `TextCopied` is emitted
unconditionally, immediately after `write_text` returns — and
`write_text` returns `()`. Every leg below is best-effort and silent on
failure, so a `Copied:` banner in the status bar means "we tried", not
"the system clipboard now holds this text". That is exactly why
quadraui#331 looked like a UI bug and was actually a terminal-config
one.

(This is deliberate, not an oversight: there is no synchronous way to
learn whether an OSC 52 sequence was honoured. The terminal never
replies. Reporting a failure we cannot detect would mean reporting a
failure that usually didn't happen.)

## The three legs of `TuiClipboard::write_text`

`quadraui/src/tui/services.rs`. All three run, independently, on every
copy. No leg's failure stops another from being attempted; none of them
report success.

| # | Leg | Covers | Fails silently when |
|---|---|---|---|
| 1 | **arboard** | Local desktop session (X11/Wayland/macOS/Windows) | No display — e.g. over SSH, or a bare TTY |
| 2 | **OSC 52** escape sequence, written to *both* stdout and `/dev/tty` | SSH, tmux, and anything where the terminal emulator owns the clipboard | The terminal doesn't support OSC 52, or a multiplexer swallows it (see below) |
| 3 | **Native tool** — `wl-copy` / `xclip` / `xsel` (Unix only) | Local X11/Wayland session running *inside* an outer tmux, where leg 1 can fail to own the `CLIPBOARD` selection and leg 2 is dropped (quadraui#398) | The tool isn't installed, or `$DISPLAY`/`$WAYLAND_DISPLAY` is unset/stale |

Leg 2 is written to `/dev/tty` as well as stdout (quadraui#790) so it
still reaches the terminal when the app's stdout has been redirected —
launched from a wrapper script that pipes output, for instance.

## tmux

This is the case that actually bites, and the reason this document
exists.

**tmux gates OSC 52 in two independent ways, and quadraui emits a form
for each.** On every copy, when `$TMUX` is set, leg 2 writes the *raw*
OSC 52 sequence **and then** a second copy wrapped in tmux's DCS
passthrough (`ESC P tmux ; <sequence with every ESC doubled> ESC \`).
Emitting both is harmless — each tmux configuration consumes the form
it understands and ignores the other — but **at least one of the two
knobs below must be set, or neither form gets through**:

```tmux
# ~/.tmux.conf — either of these makes clipboard copy work.
# Prefer the first.

set -g set-clipboard on      # tmux accepts the app's OSC 52 and
                             # forwards it to the outer terminal.
                             # This is what the *raw* form needs.

set -g allow-passthrough on  # tmux forwards a DCS-wrapped sequence
                             # verbatim without interpreting it.
                             # This is what the *wrapped* form needs.
```

Then `tmux kill-server` (or `tmux source-file ~/.tmux.conf` and restart
the affected clients) — tmux does not re-read the config for existing
sessions on its own.

Note that `set-clipboard` has three values, and the default is not the
one you want: `off` drops the app's sequence entirely, and `external`
tells tmux to *send* to the outer terminal for its own copies but **not
to accept** sequences from applications — so an app copy is still
swallowed. Only `on` accepts and forwards. Check what you actually have
with:

```sh
tmux show -gv set-clipboard      # want: on
tmux show -gv allow-passthrough  # want: on   (tmux 3.3+)
```

The outer terminal has to support OSC 52 too. kitty, WezTerm, iTerm2,
alacritty and xterm all do (xterm needs
`XTerm*disallowedWindowOps: 20,21,SetXprop` relaxed in some distro
defaults). GNOME Terminal / VTE historically did not.

### Mouse selection inside tmux

A separate tmux annoyance that looks like the same bug. When the app
grabs the mouse, tmux hands mouse events to the app, so dragging
selects *inside quadraui* rather than making a terminal-native
selection. To get your terminal emulator's own selection (the one
middle-click pastes), **hold Shift while dragging**. That is a terminal
convention, not something quadraui can change — an app that has
requested mouse reporting is supposed to receive the events.

If what you want is quadraui's own selection, drag without Shift, then
`Ctrl-C` — which puts you back on the path documented above.

### GNU screen

Not covered. `screen` does not implement OSC 52 passthrough the way
tmux does, and quadraui emits no screen-specific form. Copy falls back
to legs 1 and 3.

## SSH

Over SSH, leg 1 (arboard) has no display and leg 3's tools are usually
absent or pointed at the wrong display, so **leg 2 is the only one that
can work**. It generally does: the sequence travels up the SSH channel
as ordinary terminal output and the *local* terminal emulator honours
it. If there is a tmux on the remote host, the tmux section above
applies on that host's `~/.tmux.conf`, not yours.

## Payload size

Many terminals cap the OSC 52 base64 payload at roughly 74–100 KB
encoded (≈ 55–75 KB of raw text) and **silently drop or truncate**
anything larger. tmux applies its own limit. A `Ctrl-A` select-all over
a large buffer can exceed this. There is no feedback when it happens —
the copy simply doesn't arrive, or arrives cut short.

## Troubleshooting a "copy didn't work" report

Work down the list; each step distinguishes a different leg.

1. **Is it tmux?** `echo $TMUX` inside the app's shell. Non-empty ⇒ go
   to step 2. Empty ⇒ skip to step 4.
2. `tmux show -gv set-clipboard`. Anything but `on` is very likely the
   whole bug. Set it, `tmux kill-server`, retry.
3. Confirm the outer terminal honours OSC 52 at all, with tmux out of
   the picture — run the app *outside* tmux and copy. Works outside but
   not inside ⇒ tmux config. Fails both ⇒ terminal doesn't support
   OSC 52; step 5 is your fallback.
4. **Is it a size limit?** Retry with a one-line selection instead of
   select-all. Small copy works, large one doesn't ⇒ payload cap.
5. **Is leg 3 available?** On a local Unix desktop, check `which xclip
   wl-copy xsel` and that `$DISPLAY` / `$WAYLAND_DISPLAY` is set.
   Installing one of these tools is the most reliable fix for a local
   session inside tmux, because it bypasses the terminal entirely.

## What the automated tests do and don't prove

`quadraui/src/tui/services.rs::tests` pins the *bytes*: the base64
encoding, the raw OSC 52 framing, that `in_tmux` produces raw-then-DCS
with every inner `ESC` doubled, and the native-tool candidate ordering
and arguments. `quadraui/tests/tui_example_driver.rs` pins the
*dispatch*: that `Ctrl-A` then `Ctrl-C` reaches `write_text` and emits
`TextCopied`.

Neither can prove the bytes arrived. There is no terminal in a
`TestBackend` run — no tmux, no emulator, no system clipboard. **The
last hop is operator-verified only**, via `cargo run --example
tui_clipboard --features tui` inside a real tmux session; see that
example's module doc for the procedure.

## History

- quadraui#269 — TUI mouse text selection + OSC 52 clipboard; the tmux
  DCS-passthrough form landed on this branch.
- quadraui#329 — `Ctrl-A` select-all for `TextRegion`.
- quadraui#331 — reported as "#329's copy doesn't reach the clipboard
  inside tmux". The passthrough emission and `$TMUX` detection were
  already in place; the actual remaining gap was that the required tmux
  configuration was documented nowhere a user would look. This file is
  that gap.
- quadraui#398 — native-tool leg added for local X11-inside-tmux.
- quadraui#790 — OSC 52 also written to `/dev/tty`, not just stdout.
