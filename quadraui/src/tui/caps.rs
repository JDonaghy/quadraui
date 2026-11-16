//! Terminal capability detection: colour depth, kitty keyboard protocol,
//! SGR-Pixels mouse mode, underline-colour support, and the Kitty graphics
//! protocol.
//!
//! This module reads environment signals (`COLORTERM`, `TERM`, `KITTY_WINDOW_ID`,
//! `TERM_PROGRAM`, `WT_SESSION`) and performs live terminal queries to detect
//! what a terminal can actually display and accept.
//!
//! ## Detection rules
//!
//! ### Colour depth
//!
//! 1. `COLORTERM` containing `truecolor` or `24bit` (case-insensitive) ⇒
//!    [`ColorDepth::TrueColor`]. This is the explicit, positive signal a
//!    terminal emulator sets when it supports 24-bit colour — xterm,
//!    Alacritty, Windows Terminal, iTerm2, and (when configured with
//!    `set -ga terminal-overrides ",*256col*:Tc"`) tmux with passthrough
//!    all set this.
//! 2. Otherwise, `TERM` containing `256color` ⇒ [`ColorDepth::Indexed256`].
//!    Covers `xterm-256color`, `screen-256color`, `tmux-256color` — the
//!    common tmux-without-passthrough case: tmux sets `TERM` to one of these
//!    but does not forward `COLORTERM` by default, so this rule alone
//!    correctly lands on 256-colour rather than assuming truecolor.
//! 3. Otherwise ⇒ [`ColorDepth::Ansi16`], the conservative fallback for
//!    every other `TERM` value (`xterm`, `vt100`, `linux`, `screen`, an
//!    unset/empty `TERM`, …) — a wrong `Ansi16` degrades a terminal that
//!    actually supports more into slightly duller colours; a wrong
//!    `TrueColor` renders as garbage escape-sequence artifacts or
//!    mis-picked colours on a terminal that supports less. The cheaper
//!    failure mode is the default.
//!
//! Detection is a pure function of an environment-lookup closure
//! ([`detect_color_depth_from`]) so it can be exercised with a fixed
//! table of `(COLORTERM, TERM)` pairs rather than mutating the real
//! process environment. [`detect_color_depth`] is the thin `std::env::var`-backed
//! wrapper real callers use.
//!
//! ### Kitty keyboard protocol
//!
//! `super::run::run` pushes [kitty's progressive keyboard enhancement
//! flags](https://sw.kovidgoyal.net/kitty/keyboard-protocol/) on a best-effort
//! basis. Two layers:
//!
//! 1. [`detect_kitty_keyboard`] — a pure, instant, environment-only
//!    heuristic (`TERM`, `KITTY_WINDOW_ID`, `TERM_PROGRAM`) for terminals
//!    that support the protocol: kitty itself, foot, and WezTerm.
//!    Never claims a false positive it can't back with a specific, checked
//!    signal — an unrecognised terminal answers `false`, the cheaper failure
//!    mode.
//! 2. [`probe_kitty_keyboard`] — the live, authoritative signal: the real
//!    query/response round trip
//!    ([`ratatui::crossterm::terminal::supports_keyboard_enhancement`],
//!    which writes `ESC[?u ESC[c` and waits up to 2s for a reply) *when
//!    the terminal actually answers it*, falling back to
//!    [`detect_kitty_keyboard`]'s heuristic when it does not. That fallback
//!    is essential: a terminal that genuinely supports the protocol but sits
//!    behind a multiplexer or relay that doesn't forward the query/response
//!    (tmux without `terminal-features`, mosh, some serial links) makes the
//!    live probe time out with no answer — the environment heuristic catches
//!    what the probe cannot. [`super::run::run`] calls [`probe_kitty_keyboard`]
//!    once at startup and stores the result on [`crate::tui::backend::TuiBackend`]
//!    via [`crate::tui::backend::TuiBackend::set_kitty_keyboard`], where
//!    [`crate::backend::BackendCaps::kitty_keyboard`] exposes it to the app
//!    before it relies on any gesture that needs it.
//!
//! ### SGR-Pixels mouse mode
//!
//! Ordinary SGR mouse mode (`?1006h`, always enabled by [`super::run::run`])
//! only reports motion when the pointer crosses a *cell* boundary — every
//! coordinate quadraui hands an app is a whole terminal cell. Mode 1016
//! ("SGR-Pixels") reports the identical `CSI < Cb ; Cx ; Cy M/m` wire
//! format with `Cx`/`Cy` in *pixels* instead of cells, so crossterm parses
//! it unmodified — dividing by the real cell size recovers a fractional
//! cell coordinate.
//!
//! Same two-layer split as colour depth and kitty keyboard, but with one
//! deliberate asymmetry: **a swallowed or negative DECRQM round trip here
//! must resolve to `false`, not fall back to the environment heuristic.**
//! Misdetecting pixel mode is far worse than not having it — a pixel
//! coordinate read as a cell index puts every click in the wrong place.
//!
//! 1. [`detect_sgr_pixel_mouse`] — pure, instant, environment-only
//!    heuristic, positive only for terminals this crate can identify with
//!    high confidence: `TERM=foot*`/`contour*`, `KITTY_WINDOW_ID`,
//!    `TERM_PROGRAM=WezTerm`. `TERM=screen*`/`tmux*` or `$TMUX` set is a
//!    hard `false` regardless of any other signal — a multiplexer that
//!    does not forward the mode must never be inferred from the outer
//!    terminal's identity.
//! 2. [`probe_sgr_pixel_mouse`] — the live, authoritative answer: a DECRQM
//!    query (`CSI ? 1016 $ p`), whose reply (`CSI ? 1016 ; Ps $ y`) reports
//!    `Ps` ∈ {0 not recognised, 1 set, 2 reset, 3 permanently set, 4
//!    permanently reset}. Only `Ps` ∈ {1, 3} answers `true`; a timeout, a
//!    truncated/unparseable reply, or `Ps` ∈ {0, 2, 4} all answer `false`.
//!    Unlike [`probe_kitty_keyboard`], crossterm has no concept of an
//!    arbitrary DECRQM response — reading it that way would leak the raw
//!    `CSI ... $ y` bytes into the app as stray `KeyPressed` events. So this
//!    probe reads its reply directly off the file descriptor (`#[cfg(unix)]`,
//!    via a real `poll(2)` with a hard deadline so a terminal that never
//!    answers can be abandoned cleanly). No non-unix implementation exists
//!    yet; non-unix hosts get the honest `false` a timeout would produce.
//!    [`super::run::run`] calls this once at startup and stores the result
//!    — together with the resolved [`crate::TerminalCellSize`] — on
//!    [`crate::tui::backend::TuiBackend`] via
//!    [`crate::tui::backend::TuiBackend::set_sgr_pixel_mouse`] /
//!    [`crate::tui::backend::TuiBackend::set_cell_pixel_size`], surfaced to
//!    an app via [`crate::backend::BackendCaps::sgr_pixel_mouse`].
//!
//! ### Underline-colour (SGR 58) support
//!
//! `ratatui-crossterm`'s `SetUnderlineColor` only ever emits SGR 58 in the
//! semicolon form (`ESC[58;5;Nm` / `ESC[58;2;R;G;Bm`). ConPTY — the pty
//! layer every native Windows terminal (including Windows Terminal) sits
//! behind — only recognises the *colon* form (`ESC[58:5:Nm`); fed the
//! semicolon form, it drops the leading `58` and reinterprets the
//! remaining numbers as unrelated SGR parameters, so any cell this crate
//! paints with `underline_color` set blinks or fades instead of showing
//! an underline in that colour. Every Windows build sits behind ConPTY, so
//! [`detect_underline_color_support`] answers `false` there unconditionally.
//! A *non-Windows* build can sit behind ConPTY too: a Linux binary in WSL
//! whose output is drawn by Windows Terminal hits the same misparse. Windows
//! Terminal exports `WT_SESSION`, and WSL forwards it into the Linux
//! environment, so [`detect_underline_color_support_from`] also answers
//! `false` whenever that variable is set. macOS Terminal.app
//! (`TERM_PROGRAM=Apple_Terminal`) misparses the semicolon form the same way.
//! [`crate::tui::backend::TuiBackend::new`] seeds
//! [`crate::tui::backend::TuiBackend::underline_color_supported`] with this;
//! [`crate::tui::backend::TuiBackend::set_underline_color_supported`]
//! overrides it for tests. [`crate::tui::backend::TuiBackend::strip_unsupported_underline_colors`]
//! is where the flag is actually acted on — see that method's doc for why
//! a single per-frame sweep, not a parameter threaded through every
//! rasteriser, is the chosen choke point.
//!
//! ### Kitty graphics protocol (with Unicode placeholders)
//!
//! [`crate::tui::backend::TuiBackend::kitty_graphics`] reports whether this
//! session's terminal can be sent Kitty-protocol graphics
//! (<https://sw.kovidgoyal.net/kitty/graphics-protocol/>) placed with
//! Unicode placeholders, plus the pixel size of one cell — both needed by
//! any later rasteriser that wants to emit a real image instead of the
//! text-only fallback every terminal already gets. Same env/live-probe
//! split as kitty keyboard and SGR-Pixels mouse mode, with one addition —
//! an explicit override — and one asymmetry borrowed from SGR-Pixels
//! mouse mode: **an inconclusive probe must resolve to unsupported, not
//! fall back to a heuristic guess.**
//!
//! 1. [`detect_kitty_graphics`] — a pure, instant, environment-only
//!    heuristic (`TERM`, `KITTY_WINDOW_ID`, `TERM_PROGRAM`,
//!    `GHOSTTY_RESOURCES_DIR`) for the two terminal families this crate
//!    can name with high confidence: kitty itself and Ghostty. Unlike
//!    [`detect_kitty_keyboard`] (a fallback for an *inconclusive* probe),
//!    a positive answer here *skips* [`probe_kitty_graphics`]'s live round
//!    trip entirely — these terminals are already known-good, so paying
//!    the up-to-2s wait on top of the kitty-keyboard and SGR-Pixels
//!    probes' own buys nothing.
//! 2. [`kitty_graphics_override_from`] — the `QUADRAUI_TUI_GRAPHICS`
//!    environment override (`off` / `kitty`), checked by
//!    `super::run::setup_terminal` ahead of both the heuristic and the
//!    probe and forcing the answer in either direction without touching
//!    the terminal at all. Exists for a terminal this crate misdetects
//!    either way, and for CI/test environments that want a deterministic
//!    answer.
//! 3. [`probe_kitty_graphics`] — the live, authoritative answer for every
//!    terminal the heuristic doesn't already recognise: writes a Kitty
//!    graphics query action (`a=q`, which the protocol defines to never
//!    display or allocate anything) immediately followed by a DA1
//!    sentinel (`CSI c`, which essentially every VT100-compatible
//!    terminal answers), and reads whatever comes back directly off
//!    stdin via the same raw `poll(2)` route [`query_sgr_pixel_decrqm`]
//!    uses — bypassing crossterm's event reader so neither reply leaks
//!    into the app as a stray `KeyPressed` event. A complete `OK` graphics
//!    reply (`ESC _ G … ; OK ESC \`) appearing before the DA1 reply's own
//!    terminator means supported; the DA1 reply alone (an error response,
//!    a timeout, or any other inconclusive result) means not — see
//!    [`parse_kitty_graphics_probe_reply`].
//! 4. Multiplexers get the same treatment SGR-Pixels mouse mode already
//!    has: `super::run::setup_terminal` skips the live probe entirely
//!    under a multiplexer [`sgr_pixel_mouse_blocked_by_multiplexer`]
//!    already identifies as not forwarding escape sequences it doesn't
//!    understand, unless the `QUADRAUI_TUI_GRAPHICS=kitty` override forces
//!    the answer regardless (tmux passthrough, when configured, is
//!    handled by the transport child, not detected here).
//!
//! Even a confirmed-supported terminal is useless without knowing how big
//! a cell is in pixels — `super::run::setup_terminal` resolves that
//! from the same `TIOCGWINSZ`-backed `window_size()` query SGR-Pixels
//! mouse mode already uses, falling back to a live `CSI 16 t` query
//! ([`query_cell_pixel_size_csi16t`]) when that fails or reports a zero
//! dimension. No resolved pixel size collapses the capability to
//! unsupported, regardless of what the heuristic/override/probe said —
//! see [`crate::tui::backend::TuiBackend::kitty_graphics`]'s doc.

use crate::backend::{ColorDepth, SystemTheme};

/// Detect this process's terminal colour depth from `COLORTERM`/`TERM`.
/// See the module doc for the exact precedence. [`crate::tui::backend::TuiBackend::new`]
/// calls this to seed [`crate::tui::backend::TuiBackend::color_depth`];
/// override it after construction with
/// [`crate::tui::backend::TuiBackend::set_color_depth`] if a host already
/// knows better (or a test wants a specific SGR shape without touching
/// real env vars).
pub fn detect_color_depth() -> ColorDepth {
    detect_color_depth_from(|key| std::env::var(key).ok())
}

/// The pure decision behind [`detect_color_depth`], parameterised over an
/// environment lookup so tests can supply a fixed table instead of
/// mutating the real process environment.
pub(crate) fn detect_color_depth_from(getenv: impl Fn(&str) -> Option<String>) -> ColorDepth {
    if let Some(colorterm) = getenv("COLORTERM") {
        let colorterm = colorterm.to_ascii_lowercase();
        if colorterm.contains("truecolor") || colorterm.contains("24bit") {
            return ColorDepth::TrueColor;
        }
    }
    if let Some(term) = getenv("TERM") {
        if term.contains("256color") {
            return ColorDepth::Indexed256;
        }
    }
    ColorDepth::Ansi16
}

/// Environment-only kitty-keyboard-protocol heuristic — see the module
/// doc's "Kitty keyboard protocol" section for why this exists alongside
/// [`probe_kitty_keyboard`] rather than instead of it.
///
/// [`crate::tui::backend::TuiBackend::new`] seeds
/// [`crate::tui::backend::TuiBackend::kitty_keyboard`] with this (cheap,
/// no terminal I/O, safe to call from a test that never touches a real
/// tty) — [`super::run::run`] overwrites it with [`probe_kitty_keyboard`]'s
/// answer once it has one.
pub fn detect_kitty_keyboard() -> bool {
    detect_kitty_keyboard_from(|key| std::env::var(key).ok())
}

/// The pure decision behind [`detect_kitty_keyboard`], parameterised over
/// an environment lookup for the same reason as
/// [`detect_color_depth_from`].
///
/// Every positive here is a terminal
/// [`ratatui::crossterm::event::PushKeyboardEnhancementFlags`]'s own doc
/// names as supporting the protocol, matched on a signal that terminal
/// sets unconditionally (not merely a `TERM` value another terminal could
/// plausibly reuse):
///
/// 1. `TERM` containing `kitty` (the kitty terminal's own terminfo name,
///    `xterm-kitty`), or exactly `foot`/`foot-extra` (foot's terminfo
///    names) ⇒ `true`.
/// 2. `KITTY_WINDOW_ID` set ⇒ `true` — kitty sets this unconditionally,
///    even in a session (e.g. tmux) that has rewritten `TERM` to something
///    else.
/// 3. `TERM_PROGRAM` exactly `WezTerm` ⇒ `true` — WezTerm sets this
///    unconditionally; unlike kitty/foot it does not change `TERM` for
///    the terminfo name.
/// 4. Otherwise ⇒ `false`, the conservative fallback. Unlike colour depth
///    (where the cheap failure is a duller-than-necessary palette), the
///    cheap failure mode here is the *other* direction: a wrongly-`true`
///    answer tells an app a gesture works when the keys silently never
///    arrive, while a wrongly-`false` answer only leaves a working
///    enhancement unadvertised. So — unlike [`detect_color_depth_from`]'s
///    "assume the safe middle" — this never guesses `true` for a terminal
///    it cannot name a specific signal for (Alacritty and iTerm2 support
///    recent-enough versions of the protocol per
///    `docs/KITTY_KEYBOARD_PROTOCOL.md`'s degrade table, but neither sets
///    an unambiguous, version-independent environment signal, so both stay
///    on this conservative `false` path here and rely on
///    [`probe_kitty_keyboard`]'s live round trip instead).
pub(crate) fn detect_kitty_keyboard_from(getenv: impl Fn(&str) -> Option<String>) -> bool {
    if let Some(term) = getenv("TERM") {
        let term = term.to_ascii_lowercase();
        if term.contains("kitty") || term == "foot" || term == "foot-extra" {
            return true;
        }
    }
    if getenv("KITTY_WINDOW_ID").is_some() {
        return true;
    }
    if getenv("TERM_PROGRAM").as_deref() == Some("WezTerm") {
        return true;
    }
    false
}

/// The live, authoritative kitty-keyboard-protocol answer — see the module
/// doc's "Kitty keyboard protocol" section. Real terminal I/O (writes the
/// query, blocks up to 2s for a reply): call this once at startup, not
/// from a hot path or a test that doesn't want that latency.
///
/// Checks [`super::backend::stdin_hung_up`] first, on unix, and skips the
/// live query entirely if it's already true: `supports_keyboard_enhancement`
/// reaches crossterm's own `poll_internal`/`read_internal` internally
/// (`terminal::sys::unix::query_keyboard_enhancement_flags_raw`) — the
/// identical vulnerable `UnixInternalEventSource::try_read` loop
/// [`super::backend::TuiBackend::wait_events`]'s own guard exists for, just
/// reached through a different crossterm entry point that this backend
/// never otherwise touches. A pty that's already hung up before this
/// crate's startup capability probes even run (plausible for a headless
/// session wired up over an already-dead pty) would otherwise hang here
/// before a single frame ever renders, with no `TuiBackend` guard able to
/// see it. Falling back to [`detect_kitty_keyboard`] here is exactly what
/// an inconclusive live probe already does on any other failure.
pub(crate) fn probe_kitty_keyboard() -> bool {
    if super::backend::stdin_hung_up() {
        return detect_kitty_keyboard();
    }
    ratatui::crossterm::terminal::supports_keyboard_enhancement()
        .unwrap_or_else(|_| detect_kitty_keyboard())
}

/// Environment-only SGR-Pixels-mouse-mode heuristic — see the module doc's
/// "SGR-Pixels mouse mode" section for why this exists alongside
/// [`probe_sgr_pixel_mouse`] rather than instead of it.
pub fn detect_sgr_pixel_mouse() -> bool {
    detect_sgr_pixel_mouse_from(|key| std::env::var(key).ok())
}

/// The pure decision behind [`detect_sgr_pixel_mouse`], parameterised over
/// an environment lookup for the same reason as [`detect_color_depth_from`].
///
/// The multiplexer check ([`sgr_pixel_mouse_blocked_by_multiplexer_from`])
/// runs *first* and short-circuits to `false` regardless of any other
/// signal — including a positive one, so `TERM_PROGRAM=WezTerm` behind
/// `$TMUX` still answers `false`. Without that ordering, a terminal identity
/// signal the outer terminal sets unconditionally (the same reasoning
/// [`detect_kitty_keyboard_from`]'s doc gives for `KITTY_WINDOW_ID`
/// surviving a rewritten `TERM`) would be wrongly inferred *through* a
/// multiplexer that does not forward the mode at all.
pub(crate) fn detect_sgr_pixel_mouse_from(getenv: impl Fn(&str) -> Option<String>) -> bool {
    if sgr_pixel_mouse_blocked_by_multiplexer_from(&getenv) {
        return false;
    }
    let term = getenv("TERM").map(|t| t.to_ascii_lowercase());
    if let Some(term) = &term {
        if term.starts_with("foot") || term.starts_with("contour") {
            return true;
        }
    }
    if getenv("KITTY_WINDOW_ID").is_some() {
        return true;
    }
    if getenv("TERM_PROGRAM").as_deref() == Some("WezTerm") {
        return true;
    }
    false
}

/// Whether the environment identifies a multiplexer session known not to
/// forward escape-sequence-based terminal features at all: `$TMUX` set, or
/// `TERM` starting with `screen`/`tmux`. Extracted out of
/// [`detect_sgr_pixel_mouse_from`] so [`super::run::run_with`] can skip
/// [`probe_sgr_pixel_mouse`]'s live DECRQM round trip entirely in this case
/// — a multiplexer that doesn't forward the mode will never answer the
/// query usefully, so the up-to-2s wait for a reply that never comes only
/// stacks startup latency on top of the kitty-keyboard probe's own up-to-2s
/// wait, and (per the module doc's `query_sgr_pixel_decrqm` note) leaves
/// that same window open for a real keystroke to be read and discarded as
/// a candidate probe reply. This check alone is *not* a positive detector
/// — see [`detect_sgr_pixel_mouse_from`] for the terminals it still runs
/// the live probe against.
///
/// Reused (not duplicated) by [`detect_kitty_graphics_from`] for the
/// identical reason: tmux does not forward Kitty graphics escapes either,
/// so the same session-identity signals that would wrongly answer `true`
/// for SGR-Pixels mouse mode behind tmux would wrongly answer `true` for
/// Kitty graphics behind tmux too.
pub(crate) fn sgr_pixel_mouse_blocked_by_multiplexer_from(
    getenv: impl Fn(&str) -> Option<String>,
) -> bool {
    if getenv("TMUX").is_some() {
        return true;
    }
    if let Some(term) = getenv("TERM") {
        let term = term.to_ascii_lowercase();
        if term.starts_with("screen") || term.starts_with("tmux") {
            return true;
        }
    }
    false
}

/// The real-environment-backed wrapper around
/// [`sgr_pixel_mouse_blocked_by_multiplexer_from`] — see that function's doc.
pub(crate) fn sgr_pixel_mouse_blocked_by_multiplexer() -> bool {
    sgr_pixel_mouse_blocked_by_multiplexer_from(|key| std::env::var(key).ok())
}

/// Parse a DECRQM report reply (`CSI ? Pd ; Ps $ y`) for mode 1016, in
/// response to the `CSI ? 1016 $ p` query [`probe_sgr_pixel_mouse`] sends —
/// see <https://vt100.net/docs/vt510-rm/DECRPM.html>. Returns the `Ps`
/// digit (0–4) on a well-formed reply for *any* mode number (the caller
/// already knows which mode it queried), or `None` for anything else: a
/// truncated reply, extra `;`-delimited fields, a non-numeric `Ps`, or
/// bytes that aren't valid UTF-8 at all.
///
/// Pure and allocation-light on purpose — this is the piece
/// `probe_sgr_pixel_mouse`'s unit tests actually exercise; the real
/// file-descriptor read around it has no meaningful way to fake a terminal
/// reply in a `cargo test` process.
///
/// The `allow(dead_code)` is **not** a "compiles on my machine" escape
/// hatch: the only production caller is the `#[cfg(unix)]`
/// [`query_sgr_pixel_decrqm`], and the `#[cfg(not(unix))]` arm answers
/// `None` without ever reading a reply — so on Windows (a *blocking*
/// `tui + win` CI leg, and `RUSTFLAGS: -D warnings` workflow-wide) this
/// function has no non-test caller and `-D dead-code` rejects the build.
/// Deliberately `allow`-ed rather than `#[cfg(any(unix, test))]`-ed out:
/// the parser is pure byte-shuffling with nothing platform-specific in it,
/// and keeping it compiled *and unit-tested* on every target is what
/// proves that — cfg-ing it away would silently drop its whole test block
/// from the Windows leg, which is the leg most likely to regress it if a
/// non-unix DECRQM read is ever added (see [`query_sgr_pixel_decrqm`]'s
/// non-unix doc for what that would take).
#[cfg_attr(not(unix), allow(dead_code))]
pub(crate) fn parse_decrqm_reply(buf: &[u8]) -> Option<u8> {
    let text = std::str::from_utf8(buf).ok()?;
    let body = text.strip_prefix("\x1b[?")?.strip_suffix("$y")?;
    let mut parts = body.split(';');
    parts.next()?; // the mode number itself (1016) — unchecked, see doc.
    let ps = parts.next()?;
    if parts.next().is_some() {
        return None; // more fields than DECRPM's `Pd ; Ps` shape has.
    }
    ps.trim().parse::<u8>().ok()
}

/// Whether a DECRQM `Ps` value (see [`parse_decrqm_reply`]) means the mode
/// is actually active: `1` (set) or `3` (permanently set). `0`
/// (not recognised), `2` (reset), `4` (permanently reset), and `None` (no
/// reply parsed at all — a timeout, or a truncated/malformed one) all mean
/// `false` — see the module doc's "asymmetry from the kitty-keyboard probe"
/// paragraph for why this never falls back to a heuristic guess.
pub(crate) fn decrqm_reply_supports_mode(ps: Option<u8>) -> bool {
    matches!(ps, Some(1) | Some(3))
}

/// The live, authoritative SGR-Pixels-mouse-mode answer — see the module
/// doc's "SGR-Pixels mouse mode" section. Real terminal I/O (writes the
/// DECRQM query, blocks up to 2s for a reply): call this once at startup,
/// before the event loop starts, not from a hot path or a test that
/// doesn't want that latency.
pub(crate) fn probe_sgr_pixel_mouse() -> bool {
    decrqm_reply_supports_mode(query_sgr_pixel_decrqm())
}

/// Write the `CSI ? 1016 $ p` DECRQM query and read its reply directly off
/// stdin, bypassing crossterm's event reader entirely (see the module doc
/// for why routing this through crossterm would leak the reply as stray
/// `KeyPressed` events). Returns the parsed `Ps` digit, or `None` on any
/// failure to write, poll, read, or parse — including a timeout.
#[cfg(unix)]
fn query_sgr_pixel_decrqm() -> Option<u8> {
    use std::io::{Read, Write};
    use std::os::unix::io::AsRawFd;
    use std::time::{Duration, Instant};

    let mut stdout = std::io::stdout();
    stdout.write_all(b"\x1b[?1016$p").ok()?;
    stdout.flush().ok()?;

    let stdin = std::io::stdin();
    let fd = stdin.as_raw_fd();
    let mut handle = stdin.lock();
    let deadline = Instant::now() + Duration::from_millis(2000);
    let mut collected = Vec::with_capacity(16);

    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            // Give up: no leftover reader is left running (this function
            // returns, full stop), so a byte that arrives after this point
            // — the user's first real keystroke on a terminal that never
            // answers, e.g. tmux — reaches the real event loop untouched.
            return None;
        }
        // 0 = timed out; negative = an error (e.g. `EINTR`). Neither is
        // worth a retry loop for a one-shot startup probe on stdin.
        let timeout_ms = i32::try_from(remaining.as_millis()).unwrap_or(i32::MAX);
        super::backend::poll_fd_revents(fd, timeout_ms).revents()?;
        let mut byte = [0u8; 1];
        match handle.read(&mut byte) {
            Ok(0) | Err(_) => return None,
            Ok(_) => {
                collected.push(byte[0]);
                if byte[0] == b'y' || collected.len() >= 32 {
                    break;
                }
            }
        }
    }
    parse_decrqm_reply(&collected)
}

/// Non-unix hosts have no safe, cancellable way (through `std` alone) to
/// abandon a raw stdin read after a timeout without leaving a reader that
/// could later steal a real keystroke — see the unix arm's doc for exactly
/// what that failure mode looks like. Rather than risk it, this probe
/// simply never answers `true` off this platform; `window_size()` already
/// returns `Unsupported` on Windows (crossterm's own doc), which
/// independently keeps SGR-Pixels mode off there regardless — see
/// [`super::run::run_with`].
#[cfg(not(unix))]
fn query_sgr_pixel_decrqm() -> Option<u8> {
    None
}

/// Whether this terminal correctly renders SGR 58 (`underline_color`) in
/// the semicolon form crossterm emits — see the module doc's
/// "Underline-colour (SGR 58) support" section for the full ConPTY
/// misparse this guards against.
///
/// `false` on every Windows build, and on any other build whose output is
/// drawn by Windows Terminal (`WT_SESSION` set, e.g. a Linux binary in
/// WSL), and in macOS Terminal.app (`TERM_PROGRAM=Apple_Terminal`). The cheaper failure mode mirrors [`detect_kitty_keyboard_from`]'s
/// reasoning: a wrongly-`true` answer reproduces the blink/fade bug this
/// function exists to prevent, while a wrongly-`false` answer only costs a
/// plain (uncoloured) underline on a terminal that could have shown an
/// accent colour.
pub fn detect_underline_color_support() -> bool {
    detect_underline_color_support_from(cfg!(windows), |key| std::env::var(key).ok())
}

/// Testable core of [`detect_underline_color_support`]: `is_windows` is
/// the build target, `getenv` the environment lookup.
pub(crate) fn detect_underline_color_support_from(
    is_windows: bool,
    getenv: impl Fn(&str) -> Option<String>,
) -> bool {
    if is_windows {
        return false;
    }
    // Windows Terminal (via WSL) sits behind ConPTY even for a Linux
    // binary. An empty value is treated as unset.
    if getenv("WT_SESSION").is_some_and(|v| !v.is_empty()) {
        return false;
    }
    // macOS Terminal.app has no underline-colour support and applies the
    // trailing parameters of the semicolon form as plain SGR codes
    // (`2` -> faint), the same leak ConPTY produces.
    getenv("TERM_PROGRAM").as_deref() != Some("Apple_Terminal")
}

/// Environment-only Kitty-graphics-protocol heuristic — see the module
/// doc's "Kitty graphics protocol" section for why a positive answer here
/// skips [`probe_kitty_graphics`]'s live round trip entirely, rather than
/// merely backstopping it the way [`detect_kitty_keyboard`] backstops
/// [`probe_kitty_keyboard`].
pub fn detect_kitty_graphics() -> bool {
    detect_kitty_graphics_from(|key| std::env::var(key).ok())
}

/// The pure decision behind [`detect_kitty_graphics`], parameterised over
/// an environment lookup for the same reason as [`detect_color_depth_from`].
///
/// The multiplexer check ([`sgr_pixel_mouse_blocked_by_multiplexer_from`],
/// reused rather than duplicated — see that function's doc) runs *first*
/// and short-circuits to `false` regardless of any other signal below,
/// mirroring [`detect_sgr_pixel_mouse_from`]'s identical ordering and for
/// the identical reason: tmux inherits its server environment from the
/// client that started it, so a tmux session launched from kitty or
/// Ghostty still has `KITTY_WINDOW_ID`/`GHOSTTY_RESOURCES_DIR` set in every
/// pane even though `TERM` has been rewritten to `screen*`/`tmux*` and tmux
/// does not forward Kitty graphics escapes at all. Without this ordering,
/// signal 2 or 3 below would be wrongly inferred *through* a multiplexer
/// that cannot act on it.
///
/// Otherwise, only the two terminal families this crate can name with high
/// confidence:
///
/// 1. `TERM` containing `kitty` (kitty's own terminfo name, `xterm-kitty`)
///    ⇒ `true`.
/// 2. `KITTY_WINDOW_ID` set ⇒ `true` — kitty sets this unconditionally,
///    even inside a session that has rewritten `TERM` to something else,
///    mirroring [`detect_kitty_keyboard_from`]'s identical reasoning for
///    the same variable.
/// 3. `TERM_PROGRAM` exactly `ghostty`, or `GHOSTTY_RESOURCES_DIR` set ⇒
///    `true` — Ghostty sets both unconditionally.
/// 4. Otherwise ⇒ `false`, the conservative fallback
///    [`probe_kitty_graphics`]'s live round trip exists to answer for
///    instead.
pub(crate) fn detect_kitty_graphics_from(getenv: impl Fn(&str) -> Option<String>) -> bool {
    if sgr_pixel_mouse_blocked_by_multiplexer_from(&getenv) {
        return false;
    }
    if let Some(term) = getenv("TERM") {
        if term.to_ascii_lowercase().contains("kitty") {
            return true;
        }
    }
    if getenv("KITTY_WINDOW_ID").is_some() {
        return true;
    }
    if getenv("TERM_PROGRAM").as_deref() == Some("ghostty") {
        return true;
    }
    if getenv("GHOSTTY_RESOURCES_DIR").is_some() {
        return true;
    }
    false
}

/// The `QUADRAUI_TUI_GRAPHICS` override real callers consult — see
/// [`kitty_graphics_override_from`].
pub(crate) fn kitty_graphics_override() -> Option<bool> {
    kitty_graphics_override_from(|key| std::env::var(key).ok())
}

/// Parse the `QUADRAUI_TUI_GRAPHICS` override: `off` forces
/// [`crate::tui::backend::TuiBackend::kitty_graphics`] to `None`, `kitty`
/// forces it supported (still subject to a usable cell pixel size — see
/// the module doc). Any other value, or the variable being unset, is
/// `None` here — "no opinion", not "off" — so
/// `super::run::setup_terminal` falls through to the heuristic and live
/// probe.
pub(crate) fn kitty_graphics_override_from(
    getenv: impl Fn(&str) -> Option<String>,
) -> Option<bool> {
    match getenv("QUADRAUI_TUI_GRAPHICS").as_deref() {
        Some("off") => Some(false),
        Some("kitty") => Some(true),
        _ => None,
    }
}

/// The live, authoritative Kitty-graphics-protocol answer — see the
/// module doc's "Kitty graphics protocol" section. Real terminal I/O
/// (writes the graphics query + DA1 sentinel, blocks up to 2s for a
/// reply): call this once at startup, not from a hot path or a test that
/// doesn't want that latency.
///
/// Checks [`super::backend::stdin_hung_up`] first, on unix, for the
/// identical reason [`probe_kitty_keyboard`]'s doc gives — except this
/// probe resolves straight to `false` rather than falling back to
/// [`detect_kitty_graphics`], matching [`probe_sgr_pixel_mouse`]'s
/// asymmetry, not [`probe_kitty_keyboard`]'s.
pub(crate) fn probe_kitty_graphics() -> bool {
    if super::backend::stdin_hung_up() {
        return false;
    }
    query_kitty_graphics_probe()
        .map(|buf| parse_kitty_graphics_probe_reply(&buf))
        .unwrap_or(false)
}

/// Write the Kitty graphics query (`a=q`, an action the protocol defines
/// to never display or allocate anything — safe to send to a terminal
/// that may not understand it at all) immediately followed by a DA1
/// sentinel (`CSI c`, which essentially every VT100-compatible terminal
/// answers), and read whatever comes back directly off stdin — see
/// [`query_sgr_pixel_decrqm`]'s doc for why this bypasses crossterm's
/// event reader entirely.
///
/// Collects bytes until a complete DA1 reply (any CSI sequence whose
/// final byte is `c`) has been read, the 2-second deadline passes, or an
/// overall size guard is hit, then returns whatever was collected —
/// including a reply with no DA1 terminator at all on a timeout — for
/// [`parse_kitty_graphics_probe_reply`] to classify. Never panics on a
/// malformed/partial reply; classification, not this collector, is where
/// that's proven (see this module's `proptests`).
#[cfg(unix)]
fn query_kitty_graphics_probe() -> Option<Vec<u8>> {
    use std::io::{Read, Write};
    use std::os::unix::io::AsRawFd;
    use std::time::{Duration, Instant};

    let mut stdout = std::io::stdout();
    // `i=1` is an arbitrary image id; `a=q` is the query action; `s=1,v=1`
    // a throwaway 1x1 size; `f=24` 24-bit RGB with no alpha; `AAAA` a
    // minimal base64 payload a query action never actually reads.
    stdout
        .write_all(b"\x1b_Gi=1,a=q,s=1,v=1,t=d,f=24;AAAA\x1b\\")
        .ok()?;
    stdout.write_all(b"\x1b[c").ok()?;
    stdout.flush().ok()?;

    let stdin = std::io::stdin();
    let fd = stdin.as_raw_fd();
    let mut handle = stdin.lock();
    let deadline = Instant::now() + Duration::from_millis(2000);
    let mut collected = Vec::with_capacity(64);
    let mut esc_seen = false;
    let mut csi_open = false;

    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break; // Timeout: whatever was collected is still handed off.
        }
        let timeout_ms = i32::try_from(remaining.as_millis()).unwrap_or(i32::MAX);
        if super::backend::poll_fd_revents(fd, timeout_ms)
            .revents()
            .is_none()
        {
            break;
        }
        let mut byte = [0u8; 1];
        match handle.read(&mut byte) {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                let b = byte[0];
                collected.push(b);
                if csi_open {
                    if (0x40..=0x7e).contains(&b) {
                        csi_open = false;
                        if b == b'c' {
                            break; // DA1 reply complete.
                        }
                    }
                } else if esc_seen && b == b'[' {
                    csi_open = true;
                }
                esc_seen = b == 0x1b;
                if collected.len() >= 512 {
                    break; // Guard against an unbounded/garbage reply.
                }
            }
        }
    }
    Some(collected)
}

/// See [`query_sgr_pixel_decrqm`]'s non-unix arm's doc — identical
/// rationale: no safe, cancellable raw-stdin read exists off unix, so this
/// probe simply never answers `true` there.
#[cfg(not(unix))]
fn query_kitty_graphics_probe() -> Option<Vec<u8>> {
    None
}

/// Classify a Kitty-graphics probe reply collected by
/// [`query_kitty_graphics_probe`] (or a hand-built fixture in tests):
/// `true` only when a complete `OK` graphics response (`ESC _ G … ; OK
/// ESC \`) appears in the buffer — which, by construction of the read
/// loop that stops at the DA1 terminator, can only be present if it
/// arrived *before* that terminator. Anything else — an error response
/// (`ESC _ G … ; <error> ESC \`), the DA1 sentinel alone with no graphics
/// reply at all, a truncated/non-UTF-8 fragment, or no bytes collected —
/// resolves to `false`: the same "never guess true on an inconclusive
/// answer" posture the module doc gives for [`parse_decrqm_reply`]/
/// [`decrqm_reply_supports_mode`], since a wrong `true` here would have a
/// later rasteriser emit real Kitty graphics escape sequences at a
/// terminal that doesn't understand them.
pub(crate) fn parse_kitty_graphics_probe_reply(buf: &[u8]) -> bool {
    let text = String::from_utf8_lossy(buf);
    let Some(start) = text.find("\x1b_G") else {
        return false;
    };
    let after = &text[start + "\x1b_G".len()..];
    let Some(end) = after.find("\x1b\\") else {
        return false;
    };
    after[..end].rsplit(';').next() == Some("OK")
}

/// Resolve this terminal's per-cell pixel size via `CSI 16 t` ("report
/// character cell size in pixels"), whose reply has the shape `CSI 6 ;
/// height ; width t` — the fallback `super::run::query_cell_pixel_size`'s
/// `TIOCGWINSZ`-backed `window_size()` reaches for when that call fails or
/// reports a zero dimension (the same "unused `ws_xpixel`/`ws_ypixel`"
/// gap that function's own doc names). Reads the reply directly off
/// stdin, bypassing crossterm's event reader, for the same reason
/// [`query_sgr_pixel_decrqm`] does.
#[cfg(unix)]
pub(crate) fn query_cell_pixel_size_csi16t() -> Option<(u16, u16)> {
    use std::io::{Read, Write};
    use std::os::unix::io::AsRawFd;
    use std::time::{Duration, Instant};

    let mut stdout = std::io::stdout();
    stdout.write_all(b"\x1b[16t").ok()?;
    stdout.flush().ok()?;

    let stdin = std::io::stdin();
    let fd = stdin.as_raw_fd();
    let mut handle = stdin.lock();
    let deadline = Instant::now() + Duration::from_millis(2000);
    let mut collected = Vec::with_capacity(32);

    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return None;
        }
        let timeout_ms = i32::try_from(remaining.as_millis()).unwrap_or(i32::MAX);
        super::backend::poll_fd_revents(fd, timeout_ms).revents()?;
        let mut byte = [0u8; 1];
        match handle.read(&mut byte) {
            Ok(0) | Err(_) => return None,
            Ok(_) => {
                collected.push(byte[0]);
                if byte[0] == b't' || collected.len() >= 32 {
                    break;
                }
            }
        }
    }
    parse_csi16t_reply(&collected)
}

/// See [`query_sgr_pixel_decrqm`]'s non-unix arm's doc — identical
/// rationale.
#[cfg(not(unix))]
pub(crate) fn query_cell_pixel_size_csi16t() -> Option<(u16, u16)> {
    None
}

/// Parse a `CSI 16 t` reply (`ESC [ 6 ; height ; width t`) — see
/// [`query_cell_pixel_size_csi16t`]. Returns `(width, height)` in pixels,
/// or `None` for anything else: a truncated reply, the wrong leading
/// field (`6` is this report's own fixed tag — any other value is a
/// different `t`-terminated report this crate didn't ask for), a
/// non-numeric or zero field, an extra field, or non-UTF-8 bytes.
///
/// The `allow(dead_code)` mirrors [`parse_decrqm_reply`]'s: the only
/// production caller is the `#[cfg(unix)]` [`query_cell_pixel_size_csi16t`],
/// so a non-unix `-D warnings` build (Windows) would otherwise reject this
/// pure parser as dead code despite it having nothing platform-specific
/// in it — and its own unit tests (kept compiled on every target) are
/// what proves that.
#[cfg_attr(not(unix), allow(dead_code))]
pub(crate) fn parse_csi16t_reply(buf: &[u8]) -> Option<(u16, u16)> {
    let text = std::str::from_utf8(buf).ok()?;
    let body = text.strip_prefix("\x1b[")?.strip_suffix('t')?;
    let mut parts = body.split(';');
    if parts.next()? != "6" {
        return None;
    }
    let height: u16 = parts.next()?.parse().ok()?;
    let width: u16 = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    if height == 0 || width == 0 {
        return None;
    }
    Some((width, height))
}

/// System dark/light detection — the TUI half of
/// `PlatformServices::system_theme`'s "honest degrade" story. Unlike
/// [`detect_color_depth`]/[`detect_kitty_keyboard`], there is no `probe_*`
/// live-query twin here yet: a real answer would need an OSC 11
/// background-colour query/response round trip (xterm, kitty, WezTerm,
/// iTerm2, foot, and tmux-with-passthrough all answer it), which needs raw
/// terminal I/O timed against the same 2s-ish window
/// [`probe_kitty_keyboard`] uses — deferred rather than guessed at here,
/// so this reads only the one static environment signal terminals already
/// set for exactly this purpose.
///
/// `COLORFGBG` is set by rxvt, urxvt, and several other terminals (and
/// forwarded by tmux) as `"<fg-index>;<bg-index>"` (occasionally a third
/// `;default` suffix, ignored) — ANSI colour indices, not RGB. Terminals
/// that don't set it at all (a large majority — xterm, GNOME Terminal,
/// Alacritty, Windows Terminal, iTerm2, kitty, WezTerm all leave it unset)
/// give this function nothing to work with, so it returns `None` rather
/// than guessing: `PlatformServices::system_theme` turns that into
/// `Err(BackendError::Unsupported)`, the same "no signal, don't fake one"
/// posture [`detect_kitty_keyboard_from`]'s doc argues for.
pub fn detect_system_theme() -> Option<SystemTheme> {
    detect_system_theme_from(|key| std::env::var(key).ok())
}

/// The pure decision behind [`detect_system_theme`], parameterised over an
/// environment lookup for the same reason as [`detect_color_depth_from`].
///
/// Background indices `7` (white) and `15` (bright white) are the only two
/// conventional "light background" values a terminal actually sets in
/// `COLORFGBG` — every other index (`0`/`8` black, and every other hue) is
/// treated as dark. That is the cheaper failure mode, mirroring
/// [`detect_color_depth_from`]'s reasoning rather than
/// [`detect_kitty_keyboard_from`]'s: a dark background is both the more
/// common terminal default and the safer wrong guess (an app that themes
/// itself for dark-on-light when the terminal is actually light-on-dark
/// merely looks duller, not unreadable, the same asymmetry that section's
/// doc names for colour depth).
pub(crate) fn detect_system_theme_from(
    getenv: impl Fn(&str) -> Option<String>,
) -> Option<SystemTheme> {
    let colorfgbg = getenv("COLORFGBG")?;
    let bg_index: u8 = colorfgbg.rsplit(';').next()?.trim().parse().ok()?;
    Some(SystemTheme {
        dark: !matches!(bg_index, 7 | 15),
        // No accent-colour, high-contrast, or reduce-motion signal exists
        // in a terminal environment — see `SystemTheme`'s field docs.
        accent: None,
        high_contrast: false,
        reduce_motion: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// Builds the `getenv` closure `detect_color_depth_from` expects from
    /// a fixed `(key, value)` table — the fixture shape every case below
    /// shares.
    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |key| map.get(key).cloned()
    }

    #[test]
    fn colorterm_truecolor_wins_regardless_of_term() {
        assert_eq!(
            detect_color_depth_from(env(&[("COLORTERM", "truecolor"), ("TERM", "xterm")])),
            ColorDepth::TrueColor
        );
    }

    #[test]
    fn colorterm_24bit_also_counts() {
        assert_eq!(
            detect_color_depth_from(env(&[("COLORTERM", "24bit")])),
            ColorDepth::TrueColor
        );
    }

    /// This test case covers the acceptance criterion: `TERM=xterm-256color`
    /// with no `COLORTERM` set must land on indexed, not truecolor.
    #[test]
    fn xterm_256color_with_no_colorterm_is_indexed() {
        assert_eq!(
            detect_color_depth_from(env(&[("TERM", "xterm-256color")])),
            ColorDepth::Indexed256
        );
    }

    /// tmux without passthrough: `TERM` becomes `screen-256color` or
    /// `tmux-256color` and `COLORTERM` is typically not forwarded.
    #[test]
    fn tmux_without_passthrough_is_indexed() {
        assert_eq!(
            detect_color_depth_from(env(&[("TERM", "screen-256color")])),
            ColorDepth::Indexed256
        );
        assert_eq!(
            detect_color_depth_from(env(&[("TERM", "tmux-256color")])),
            ColorDepth::Indexed256
        );
    }

    #[test]
    fn plain_xterm_falls_back_to_ansi16() {
        assert_eq!(
            detect_color_depth_from(env(&[("TERM", "xterm")])),
            ColorDepth::Ansi16
        );
    }

    #[test]
    fn nothing_set_falls_back_to_ansi16() {
        assert_eq!(detect_color_depth_from(env(&[])), ColorDepth::Ansi16);
    }

    #[test]
    fn empty_colorterm_does_not_falsely_claim_truecolor() {
        assert_eq!(
            detect_color_depth_from(env(&[("COLORTERM", ""), ("TERM", "xterm-256color")])),
            ColorDepth::Indexed256
        );
    }

    // ── Kitty keyboard protocol ─────────────────────────────────────────

    #[test]
    fn xterm_kitty_term_is_detected() {
        assert!(detect_kitty_keyboard_from(env(&[("TERM", "xterm-kitty")])));
    }

    #[test]
    fn foot_term_is_detected() {
        assert!(detect_kitty_keyboard_from(env(&[("TERM", "foot")])));
        assert!(detect_kitty_keyboard_from(env(&[("TERM", "foot-extra")])));
    }

    /// kitty sets `KITTY_WINDOW_ID` unconditionally, even inside a tmux
    /// session that has rewritten `TERM` to `screen-256color` — the
    /// detector must not require `TERM` to say `kitty` too.
    #[test]
    fn kitty_window_id_is_detected_even_under_a_rewritten_term() {
        assert!(detect_kitty_keyboard_from(env(&[
            ("TERM", "screen-256color"),
            ("KITTY_WINDOW_ID", "1"),
        ])));
    }

    #[test]
    fn wezterm_term_program_is_detected() {
        assert!(detect_kitty_keyboard_from(env(&[(
            "TERM_PROGRAM",
            "WezTerm"
        )])));
    }

    /// This test case covers the acceptance criterion: a plain
    /// `TERM=xterm-256color` session (raw SSH, tmux without `terminal-features`)
    /// has none of the three positive signals and must land on `false`.
    #[test]
    fn plain_256color_term_is_not_detected() {
        assert!(!detect_kitty_keyboard_from(env(&[(
            "TERM",
            "xterm-256color"
        )])));
    }

    #[test]
    fn nothing_set_is_not_detected() {
        assert!(!detect_kitty_keyboard_from(env(&[])));
    }

    /// A terminal whose `TERM_PROGRAM` merely *contains* `WezTerm` (or
    /// differs in case) must not match — the signal is WezTerm's own
    /// unconditional exact value, not a substring guess that some other
    /// terminal's `TERM_PROGRAM` could accidentally satisfy.
    #[test]
    fn term_program_match_is_exact_not_a_substring() {
        assert!(!detect_kitty_keyboard_from(env(&[(
            "TERM_PROGRAM",
            "wezterm"
        )])));
        assert!(!detect_kitty_keyboard_from(env(&[(
            "TERM_PROGRAM",
            "NotWezTermAtAll"
        )])));
    }

    /// Alacritty and iTerm2 both support the protocol in recent versions
    /// (`docs/KITTY_KEYBOARD_PROTOCOL.md`'s degrade table) but neither has
    /// an unambiguous, version-independent environment signal, so the
    /// heuristic deliberately does not claim them — `probe_kitty_keyboard`'s
    /// live round trip is what actually detects them.
    #[test]
    fn alacritty_term_is_not_guessed() {
        assert!(!detect_kitty_keyboard_from(env(&[("TERM", "alacritty")])));
    }

    // ── SGR-Pixels mouse mode ───────────────────────────────────────────

    #[test]
    fn kitty_window_id_is_detected_for_sgr_pixel_mouse() {
        assert!(detect_sgr_pixel_mouse_from(env(&[(
            "KITTY_WINDOW_ID",
            "1"
        )])));
    }

    #[test]
    fn foot_term_is_detected_for_sgr_pixel_mouse() {
        assert!(detect_sgr_pixel_mouse_from(env(&[("TERM", "foot")])));
        assert!(detect_sgr_pixel_mouse_from(env(&[("TERM", "foot-extra")])));
    }

    #[test]
    fn wezterm_term_program_is_detected_for_sgr_pixel_mouse() {
        assert!(detect_sgr_pixel_mouse_from(env(&[(
            "TERM_PROGRAM",
            "WezTerm"
        )])));
    }

    #[test]
    fn contour_term_is_detected_for_sgr_pixel_mouse() {
        assert!(detect_sgr_pixel_mouse_from(env(&[("TERM", "contour")])));
    }

    #[test]
    fn tmux_env_var_hard_disables_sgr_pixel_mouse() {
        assert!(!detect_sgr_pixel_mouse_from(env(&[(
            "TMUX",
            "/tmp/tmux-1000/default,1234,0"
        )])));
    }

    #[test]
    fn screen_term_is_not_detected() {
        assert!(!detect_sgr_pixel_mouse_from(env(&[(
            "TERM",
            "screen-256color"
        )])));
    }

    #[test]
    fn tmux_term_is_not_detected() {
        assert!(!detect_sgr_pixel_mouse_from(env(&[(
            "TERM",
            "tmux-256color"
        )])));
    }

    #[test]
    fn unset_term_is_not_detected_for_sgr_pixel_mouse() {
        assert!(!detect_sgr_pixel_mouse_from(env(&[])));
    }

    /// The acceptance-criteria case: a real WezTerm session running inside
    /// tmux must not be inferred through the multiplexer — `$TMUX` wins
    /// even though `TERM_PROGRAM` alone would otherwise say `true`.
    #[test]
    fn wezterm_term_program_under_tmux_is_not_detected() {
        assert!(!detect_sgr_pixel_mouse_from(env(&[
            ("TERM_PROGRAM", "WezTerm"),
            ("TMUX", "/tmp/tmux-1000/default,1234,0"),
        ])));
    }

    // ── SGR-Pixels probe gating ─────────────────────────────────────────

    #[test]
    fn tmux_env_var_blocks_the_live_probe() {
        assert!(sgr_pixel_mouse_blocked_by_multiplexer_from(env(&[(
            "TMUX",
            "/tmp/tmux-1000/default,1234,0"
        )])));
    }

    #[test]
    fn screen_term_blocks_the_live_probe() {
        assert!(sgr_pixel_mouse_blocked_by_multiplexer_from(env(&[(
            "TERM",
            "screen-256color"
        )])));
    }

    #[test]
    fn tmux_term_blocks_the_live_probe() {
        assert!(sgr_pixel_mouse_blocked_by_multiplexer_from(env(&[(
            "TERM",
            "tmux-256color"
        )])));
    }

    /// A terminal with none of the multiplexer signals must not have the
    /// live probe blocked, even one this crate's heuristic doesn't
    /// otherwise recognise — the probe, not the heuristic, is what answers
    /// for unrecognised-but-genuinely-supporting terminals.
    #[test]
    fn unrecognised_terminal_does_not_block_the_live_probe() {
        assert!(!sgr_pixel_mouse_blocked_by_multiplexer_from(env(&[(
            "TERM",
            "xterm-256color"
        )])));
        assert!(!sgr_pixel_mouse_blocked_by_multiplexer_from(env(&[])));
    }

    // ── DECRQM reply parsing ────────────────────────────────────────────

    #[test]
    fn decrqm_reply_parses_each_ps_value() {
        for ps in 0u8..=4 {
            let reply = format!("\x1b[?1016;{ps}$y");
            assert_eq!(
                parse_decrqm_reply(reply.as_bytes()),
                Some(ps),
                "failed to parse Ps={ps}"
            );
        }
    }

    #[test]
    fn decrqm_reply_truncated_before_terminator_fails_to_parse() {
        assert_eq!(parse_decrqm_reply(b"\x1b[?1016;1"), None);
        assert_eq!(parse_decrqm_reply(b""), None);
        assert_eq!(parse_decrqm_reply(b"\x1b[?1016$y"), None); // no Ps field at all
        assert_eq!(parse_decrqm_reply(b"\x1b[?1016;abc$y"), None); // non-numeric Ps
    }

    #[test]
    fn decrqm_ps_1_and_3_report_the_mode_supported() {
        assert!(decrqm_reply_supports_mode(Some(1)));
        assert!(decrqm_reply_supports_mode(Some(3)));
    }

    #[test]
    fn decrqm_ps_0_2_4_report_the_mode_unsupported() {
        for ps in [0u8, 2, 4] {
            assert!(
                !decrqm_reply_supports_mode(Some(ps)),
                "Ps={ps} must resolve to unsupported"
            );
        }
    }

    // ── Underline-colour (SGR 58) support ───────────────

    /// Windows builds always strip: no environment can turn it back on.
    #[test]
    fn underline_color_unsupported_on_windows_regardless_of_env() {
        assert!(!detect_underline_color_support_from(true, env(&[])));
        assert!(!detect_underline_color_support_from(
            true,
            env(&[("TERM", "xterm-kitty")])
        ));
    }

    /// A plain non-Windows terminal keeps the coloured underline.
    #[test]
    fn underline_color_supported_off_windows_without_wt_session() {
        assert!(detect_underline_color_support_from(false, env(&[])));
        assert!(detect_underline_color_support_from(
            false,
            env(&[("WT_SESSION", "")])
        ));
    }

    /// A Linux binary in WSL under Windows Terminal (`WT_SESSION` set)
    /// sits behind ConPTY and must strip, or every underline-coloured
    /// cell leaks SGR 2 (faint) into the text that follows it.
    #[test]
    fn underline_color_unsupported_under_windows_terminal_wsl() {
        assert!(!detect_underline_color_support_from(
            false,
            env(&[("WT_SESSION", "4c6d2a0e-1b2f-4a7e-9d3c-5f8e7a6b1c20")])
        ));
    }

    /// macOS Terminal.app renders the semicolon form's trailing `2` as
    /// faint, so it must strip; iTerm2 and other terminals keep the colour.
    #[test]
    fn underline_color_unsupported_in_apple_terminal() {
        assert!(!detect_underline_color_support_from(
            false,
            env(&[("TERM_PROGRAM", "Apple_Terminal")])
        ));
        assert!(detect_underline_color_support_from(
            false,
            env(&[("TERM_PROGRAM", "iTerm.app")])
        ));
    }

    /// A timeout (no reply parsed at all — represented as `None`, the same
    /// value a truncated/unparseable reply produces) must resolve to
    /// `false`, never fall back to a heuristic guess.
    #[test]
    fn decrqm_timeout_reports_the_mode_unsupported() {
        assert!(!decrqm_reply_supports_mode(None));
    }

    // ── Kitty graphics protocol ──────────────────────────────────────────

    #[test]
    fn xterm_kitty_term_is_detected_for_graphics() {
        assert!(detect_kitty_graphics_from(env(&[("TERM", "xterm-kitty")])));
    }

    #[test]
    fn kitty_window_id_is_detected_for_graphics() {
        assert!(detect_kitty_graphics_from(env(&[("KITTY_WINDOW_ID", "1")])));
    }

    #[test]
    fn ghostty_term_program_is_detected_for_graphics() {
        assert!(detect_kitty_graphics_from(env(&[(
            "TERM_PROGRAM",
            "ghostty"
        )])));
    }

    #[test]
    fn ghostty_resources_dir_is_detected_for_graphics() {
        assert!(detect_kitty_graphics_from(env(&[(
            "GHOSTTY_RESOURCES_DIR",
            "/usr/share/ghostty"
        )])));
    }

    /// An unrecognised terminal is never guessed `true` — the live probe,
    /// not the heuristic, is what answers for it.
    #[test]
    fn plain_256color_term_is_not_detected_for_graphics() {
        assert!(!detect_kitty_graphics_from(env(&[(
            "TERM",
            "xterm-256color"
        )])));
    }

    #[test]
    fn nothing_set_is_not_detected_for_graphics() {
        assert!(!detect_kitty_graphics_from(env(&[])));
    }

    /// A tmux session started from a kitty/Ghostty client still has
    /// `KITTY_WINDOW_ID` set in every pane (tmux inherits its server
    /// environment from the client that launched it) even though `TERM`
    /// has been rewritten to `screen*`/`tmux*` — but tmux does not forward
    /// Kitty graphics escapes, so this must answer `false` regardless of
    /// the inherited identity signal. This is the exact regression the
    /// multiplexer gate guards against.
    #[test]
    fn kitty_window_id_inside_tmux_is_not_detected_for_graphics() {
        assert!(!detect_kitty_graphics_from(env(&[
            ("TMUX", "/tmp/tmux-1000/default,1234,0"),
            ("TERM", "screen-256color"),
            ("KITTY_WINDOW_ID", "1"),
        ])));
    }

    /// Same scenario via Ghostty's identity signal instead of kitty's.
    #[test]
    fn ghostty_resources_dir_inside_tmux_is_not_detected_for_graphics() {
        assert!(!detect_kitty_graphics_from(env(&[
            ("TMUX", "/tmp/tmux-1000/default,1234,0"),
            ("TERM", "tmux-256color"),
            ("GHOSTTY_RESOURCES_DIR", "/usr/share/ghostty"),
        ])));
    }

    // ── Kitty graphics override ──────────────────────────────────────────

    #[test]
    fn graphics_override_off_forces_false() {
        assert_eq!(
            kitty_graphics_override_from(env(&[("QUADRAUI_TUI_GRAPHICS", "off")])),
            Some(false)
        );
    }

    #[test]
    fn graphics_override_kitty_forces_true() {
        assert_eq!(
            kitty_graphics_override_from(env(&[("QUADRAUI_TUI_GRAPHICS", "kitty")])),
            Some(true)
        );
    }

    #[test]
    fn graphics_override_unset_or_unrecognised_is_no_opinion() {
        assert_eq!(kitty_graphics_override_from(env(&[])), None);
        assert_eq!(
            kitty_graphics_override_from(env(&[("QUADRAUI_TUI_GRAPHICS", "nonsense")])),
            None
        );
    }

    // ── Kitty graphics probe-reply parsing ─────────────────────────────

    /// An `OK` graphics response arriving before the DA1 reply means
    /// supported.
    #[test]
    fn kitty_graphics_ok_reply_before_da1_is_supported() {
        let mut buf = b"\x1b_Gi=1;OK\x1b\\".to_vec();
        buf.extend_from_slice(b"\x1b[?62;c");
        assert!(parse_kitty_graphics_probe_reply(&buf));
    }

    /// An error response (anything other than the literal `OK` message)
    /// means unsupported.
    #[test]
    fn kitty_graphics_error_reply_is_unsupported() {
        let mut buf = b"\x1b_Gi=1;EINVAL:bad request\x1b\\".to_vec();
        buf.extend_from_slice(b"\x1b[?62;c");
        assert!(!parse_kitty_graphics_probe_reply(&buf));
    }

    /// The DA1 sentinel alone, with no graphics reply at all, means the
    /// terminal never answered the graphics query — unsupported.
    #[test]
    fn kitty_graphics_da1_only_is_unsupported() {
        assert!(!parse_kitty_graphics_probe_reply(b"\x1b[?62;c"));
    }

    /// Garbage, truncated, or empty bytes never panic and never resolve
    /// to supported.
    #[test]
    fn kitty_graphics_garbage_reply_is_unsupported() {
        assert!(!parse_kitty_graphics_probe_reply(b""));
        assert!(!parse_kitty_graphics_probe_reply(
            b"not an escape sequence at all"
        ));
        assert!(!parse_kitty_graphics_probe_reply(b"\x1b_Gi=1;OK")); // no ST terminator
        assert!(!parse_kitty_graphics_probe_reply(&[0xff, 0xfe, 0x00, 0x1b]));
    }

    // ── `CSI 16 t` cell-pixel-size reply parsing ───────────────────────

    #[test]
    fn csi16t_reply_parses_width_and_height() {
        assert_eq!(parse_csi16t_reply(b"\x1b[6;16;8t"), Some((8, 16)));
    }

    #[test]
    fn csi16t_reply_rejects_wrong_leading_field() {
        // `14t`'s own leading field is `4`, not this report's `6` — must
        // not be misread as a cell-size reply.
        assert_eq!(parse_csi16t_reply(b"\x1b[4;600;800t"), None);
    }

    #[test]
    fn csi16t_reply_rejects_zero_dimensions() {
        assert_eq!(parse_csi16t_reply(b"\x1b[6;0;8t"), None);
        assert_eq!(parse_csi16t_reply(b"\x1b[6;16;0t"), None);
    }

    #[test]
    fn csi16t_reply_rejects_truncated_or_malformed() {
        assert_eq!(parse_csi16t_reply(b""), None);
        assert_eq!(parse_csi16t_reply(b"\x1b[6;16"), None); // no trailing `t`
        assert_eq!(parse_csi16t_reply(b"\x1b[6;16;8;99t"), None); // extra field
        assert_eq!(parse_csi16t_reply(b"\x1b[6;abc;8t"), None); // non-numeric
    }

    // ── System theme detection ──────────────────────────────────────────

    #[test]
    fn no_colorfgbg_returns_none() {
        assert_eq!(detect_system_theme_from(env(&[])), None);
    }

    #[test]
    fn dark_background_index_zero_is_detected() {
        let theme = detect_system_theme_from(env(&[("COLORFGBG", "15;0")]))
            .expect("COLORFGBG should be parsed");
        assert!(theme.dark);
        assert_eq!(theme.accent, None);
        assert!(!theme.high_contrast);
        assert!(!theme.reduce_motion);
    }

    #[test]
    fn light_background_index_seven_is_detected() {
        let theme = detect_system_theme_from(env(&[("COLORFGBG", "0;7")]))
            .expect("COLORFGBG should be parsed");
        assert!(!theme.dark);
    }

    #[test]
    fn light_background_index_fifteen_is_detected() {
        let theme = detect_system_theme_from(env(&[("COLORFGBG", "0;15")]))
            .expect("COLORFGBG should be parsed");
        assert!(!theme.dark);
    }

    /// Every background index other than the two conventional "light"
    /// values (7, 15) reads as dark — the cheaper failure mode this
    /// function's doc argues for.
    #[test]
    fn other_background_indices_read_as_dark() {
        for bg in [0, 1, 4, 8, 9, 14] {
            let theme = detect_system_theme_from(env(&[("COLORFGBG", &format!("15;{bg}"))]))
                .unwrap_or_else(|| panic!("COLORFGBG with bg={bg} should be parsed"));
            assert!(theme.dark, "bg index {bg} should read as dark");
        }
    }

    #[test]
    fn malformed_colorfgbg_returns_none() {
        assert_eq!(
            detect_system_theme_from(env(&[("COLORFGBG", "not-a-number")])),
            None
        );
        assert_eq!(detect_system_theme_from(env(&[("COLORFGBG", "")])), None);
    }
}

/// Property tests for [`parse_decrqm_reply`]: the bytes it
/// parses come straight off a real terminal's reply to a DECRQM query — a
/// foreign, unvalidated byte stream this crate does not control — so
/// "never panics, and only ever returns a `Ps` digit that was actually in
/// the reply" needs to hold for more than the hand-picked fixtures in
/// `mod tests` above.
#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        /// No byte sequence, however malformed or non-UTF-8, may panic —
        /// this is a real terminal's reply, and a well-formed *positive*
        /// reply must resolve to `false`-by-default (module doc), so a
        /// crash here would be strictly worse than the "unsupported"
        /// fallback it's meant to prevent.
        #[test]
        fn parse_decrqm_reply_never_panics(bytes in prop::collection::vec(any::<u8>(), 0..200)) {
            let _ = parse_decrqm_reply(&bytes);
        }

        /// Whenever a `Ps` digit *is* parsed, it is a genuine 0-9 ASCII
        /// digit or run of digits that fits `u8` — never a value invented
        /// from unrelated bytes elsewhere in the reply.
        #[test]
        fn parse_decrqm_reply_only_returns_a_digit_actually_present(
            mode in 0u32..9999,
            ps in 0u16..999, // deliberately allow out-of-u8-range Ps too
        ) {
            let reply = format!("\x1b[?{mode};{ps}$y");
            let parsed = parse_decrqm_reply(reply.as_bytes());
            match u8::try_from(ps) {
                Ok(ps_u8) => prop_assert_eq!(parsed, Some(ps_u8)),
                Err(_) => prop_assert_eq!(parsed, None), // overflowed u8::parse
            }
        }

        /// [`decrqm_reply_supports_mode`] is a pure classifier over
        /// `Option<u8>` — must never panic on any value, and its "mode
        /// active" answer is exactly the two documented cases (1, 3).
        #[test]
        fn decrqm_reply_supports_mode_matches_its_documented_cases(ps in any::<Option<u8>>()) {
            prop_assert_eq!(
                decrqm_reply_supports_mode(ps),
                matches!(ps, Some(1) | Some(3))
            );
        }

        /// [`parse_kitty_graphics_probe_reply`] reads a real terminal's
        /// reply — no byte sequence, however malformed or non-UTF-8, may
        /// panic.
        #[test]
        fn parse_kitty_graphics_probe_reply_never_panics(
            bytes in prop::collection::vec(any::<u8>(), 0..200)
        ) {
            let _ = parse_kitty_graphics_probe_reply(&bytes);
        }

        /// A well-formed graphics response whose message is exactly `OK`
        /// is always read as supported, regardless of what the image id
        /// field holds.
        #[test]
        fn parse_kitty_graphics_probe_reply_recognises_any_ok_response(
            image_id in 0u32..999999,
        ) {
            let reply = format!("\x1b_Gi={image_id};OK\x1b\\\x1b[?62;c");
            prop_assert!(parse_kitty_graphics_probe_reply(reply.as_bytes()));
        }

        /// A well-formed graphics response whose message is anything
        /// other than exactly `OK` is always read as unsupported.
        #[test]
        fn parse_kitty_graphics_probe_reply_rejects_non_ok_messages(
            message in "[A-Za-z0-9:]{1,20}",
        ) {
            prop_assume!(message != "OK");
            let reply = format!("\x1b_Gi=1;{message}\x1b\\\x1b[?62;c");
            prop_assert!(!parse_kitty_graphics_probe_reply(reply.as_bytes()));
        }

        /// [`parse_csi16t_reply`] reads a real terminal's reply — no byte
        /// sequence may panic, and whenever it does parse, the two fields
        /// it returns are exactly the ones present in a well-formed
        /// `CSI 6 ; height ; width t` reply.
        #[test]
        fn parse_csi16t_reply_never_panics(bytes in prop::collection::vec(any::<u8>(), 0..200)) {
            let _ = parse_csi16t_reply(&bytes);
        }

        #[test]
        fn parse_csi16t_reply_round_trips_well_formed_replies(
            height in 1u16..=u16::MAX,
            width in 1u16..=u16::MAX,
        ) {
            let reply = format!("\x1b[6;{height};{width}t");
            prop_assert_eq!(parse_csi16t_reply(reply.as_bytes()), Some((width, height)));
        }
    }
}
