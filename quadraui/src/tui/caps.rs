//! Terminal capability detection: colour depth (quadraui#826) and kitty
//! keyboard protocol support (quadraui#827).
//!
//! Before this module existed, [`super::ratatui_color`] emitted 24-bit
//! `RatatuiColor::Rgb` unconditionally — correct on a truecolor terminal,
//! silently wrong (not missing, wrong-*looking*) on the 256-colour and
//! 16-colour terminals real users hit constantly: plain SSH to an older
//! box, tmux without `Tc`/`RGB` passthrough, a serial console, older
//! Windows terminals. [`detect_color_depth`] reads the same environment
//! signals every terminal-aware CLI tool reads (`COLORTERM`, `TERM`) to
//! answer "what can this terminal actually show", and
//! [`super::color::DepthLimitedBackend`] is what acts on the answer.
//!
//! ## Detection rules
//!
//! 1. `COLORTERM` containing `truecolor` or `24bit` (case-insensitive) ⇒
//!    [`ColorDepth::TrueColor`]. This is the explicit, positive signal a
//!    terminal emulator sets when it supports 24-bit colour — xterm,
//!    Alacritty, Windows Terminal, iTerm2, and (when configured with
//!    `set -ga terminal-overrides ",*256col*:Tc"`) tmux with passthrough
//!    all set this.
//! 2. Otherwise, `TERM` containing `256color` ⇒ [`ColorDepth::Indexed256`].
//!    Covers `xterm-256color`, `screen-256color`, `tmux-256color` — the
//!    common tmux-without-passthrough case named in quadraui#826: tmux
//!    sets `TERM` to one of these but does not forward `COLORTERM` by
//!    default, so this rule alone (without rule 1 firing) correctly lands
//!    on 256-colour rather than assuming truecolor.
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
//! process environment (which is both `unsafe` in modern Rust and
//! flaky under `cargo test`'s multi-threaded runner, where env vars are
//! genuinely global mutable state shared across every test in the
//! binary). [`detect_color_depth`] is the thin `std::env::var`-backed
//! wrapper real callers use.
//!
//! ## Kitty keyboard protocol (quadraui#827)
//!
//! `super::run::run` pushes [kitty's progressive keyboard enhancement
//! flags](https://sw.kovidgoyal.net/kitty/keyboard-protocol/) (so e.g.
//! Ctrl+Enter is unambiguous from plain Enter) on a best-effort basis —
//! before this issue, the *result* of that attempt was thrown away the
//! instant it was used to decide whether to pop the flags on exit. An app
//! had no way to ask "did that actually work", so it could only build a
//! gesture on the assumption it did and silently never see the events on a
//! terminal where it didn't — the exact "silently degrades" failure this
//! module exists to close (see `docs/KITTY_KEYBOARD_PROTOCOL.md`'s degrade
//! table for which real terminals land on which side).
//!
//! Two layers, same shape as the colour-depth split above:
//!
//! 1. [`detect_kitty_keyboard`] — a pure, instant, environment-only
//!    heuristic (`TERM`, `KITTY_WINDOW_ID`, `TERM_PROGRAM`) for the
//!    terminals crossterm's own [`ratatui::crossterm::event::PushKeyboardEnhancementFlags`]
//!    doc names as supporting the protocol and that this crate can
//!    identify with high confidence from the environment alone: kitty
//!    itself, foot, and WezTerm. Never claims a false positive it can't
//!    back with a specific, checked signal — an unrecognised terminal
//!    answers `false`, the cheaper failure mode (see this function's doc).
//! 2. [`probe_kitty_keyboard`] — the live, authoritative signal: the real
//!    query/response round trip
//!    ([`ratatui::crossterm::terminal::supports_keyboard_enhancement`],
//!    which writes `ESC[?u ESC[c` and waits up to 2s for a reply) *when
//!    the terminal actually answers it*, falling back to
//!    [`detect_kitty_keyboard`]'s heuristic when it does not. That fallback
//!    is the fix, not a decoration: a terminal that genuinely supports the
//!    protocol but sits behind a multiplexer or relay that doesn't forward
//!    the query/response (tmux without `terminal-features`, mosh, some
//!    serial links) makes the live probe time out with no answer at all —
//!    collapsing "no answer" to "unsupported" (this crate's behaviour
//!    before #827) is indistinguishable from a terminal that was actually
//!    asked and said no, and silently drops support the environment
//!    heuristic would have caught. [`super::run::run`] calls
//!    [`probe_kitty_keyboard`] once at startup and stores the result on
//!    [`crate::tui::backend::TuiBackend`] via
//!    [`crate::tui::backend::TuiBackend::set_kitty_keyboard`], where
//!    [`crate::backend::BackendCaps::kitty_keyboard`] exposes it to the
//!    app before it relies on any gesture that needs it.
//!
//! ## SGR-Pixels mouse mode (quadraui#1048)
//!
//! Ordinary SGR mouse mode (`?1006h`, always enabled by [`super::run::run`])
//! only reports motion when the pointer crosses a *cell* boundary — every
//! coordinate quadraui hands an app is a whole terminal cell, which is fine
//! for clicks but too coarse for an absolute drag over a track shorter than
//! its content (a scrollbar or minimap thumb; see vimcode#1271). Mode 1016
//! ("SGR-Pixels") reports the identical `CSI < Cb ; Cx ; Cy M/m` wire
//! format with `Cx`/`Cy` in *pixels* instead of cells, so crossterm parses
//! it unmodified — the finer values just arrive in
//! `MouseEvent.column`/`.row`, and dividing by the real cell size recovers
//! a fractional cell coordinate.
//!
//! Same two-layer split as colour depth and kitty keyboard, but with one
//! deliberate asymmetry from the kitty-keyboard probe: **a swallowed or
//! negative DECRQM round trip here must resolve to `false`, not fall back
//! to the environment heuristic.** Misdetecting pixel mode is far worse
//! than not having it — a pixel coordinate read as a cell index puts every
//! click in the wrong place, whereas a swallowed kitty-keyboard query only
//! costs an ambiguous key.
//!
//! 1. [`detect_sgr_pixel_mouse`] — pure, instant, environment-only
//!    heuristic, positive only for terminals this crate can identify with
//!    high confidence: `TERM=foot*`/`contour*`, `KITTY_WINDOW_ID`,
//!    `TERM_PROGRAM=WezTerm`. `TERM=screen*`/`tmux*` or `$TMUX` set is a
//!    hard `false` regardless of any other signal — a multiplexer that
//!    does not forward the mode must never be inferred from the outer
//!    terminal's identity (tmux 3.7c's own binary contains zero references
//!    to `1016`: it implements neither the mode nor a passthrough for it).
//! 2. [`probe_sgr_pixel_mouse`] — the live, authoritative answer: a DECRQM
//!    query (`CSI ? 1016 $ p`), whose reply (`CSI ? 1016 ; Ps $ y`) reports
//!    `Ps` ∈ {0 not recognised, 1 set, 2 reset, 3 permanently set, 4
//!    permanently reset}. Only `Ps` ∈ {1, 3} answers `true`; a timeout, a
//!    truncated/unparseable reply, or `Ps` ∈ {0, 2, 4} all answer `false`.
//!    Unlike [`probe_kitty_keyboard`] (which routes its query through
//!    crossterm's own internal event reader, whose filters already know how
//!    to recognise and discard that specific reply), crossterm has no
//!    concept of an arbitrary DECRQM response — reading it that way would
//!    itself leak the raw `CSI ... $ y` bytes into the app as stray
//!    `KeyPressed` events. So this probe reads its reply directly off the
//!    file descriptor (`#[cfg(unix)]`, via a real `poll(2)` with a hard
//!    deadline so a terminal that never answers — tmux — can be abandoned
//!    cleanly without leaving a reader that could later steal the user's
//!    first genuine keystroke; see [`super::run::run`]'s call site for why
//!    this must run *before* the event loop starts). No non-unix
//!    implementation exists yet — see this module's `query_sgr_pixel_decrqm`
//!    for why a spawned-thread-based read would be a correctness bug, not
//!    just a missing feature, on a host with no cancellable fd-level poll;
//!    non-unix hosts get the honest `false` a timeout would have produced
//!    anyway. `super::run::run` calls this once at startup and stores the
//!    result — together with the resolved [`crate::TerminalCellSize`] a
//!    live `window_size()` query provides — on
//!    [`crate::tui::backend::TuiBackend`] via
//!    [`crate::tui::backend::TuiBackend::set_sgr_pixel_mouse`] /
//!    [`crate::tui::backend::TuiBackend::set_cell_pixel_size`], surfaced to
//!    an app via [`crate::backend::BackendCaps::sgr_pixel_mouse`].

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
pub(crate) fn probe_kitty_keyboard() -> bool {
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
/// The multiplexer check runs *first* and short-circuits to `false`
/// regardless of any other signal — including a positive one, so
/// `TERM_PROGRAM=WezTerm` behind `$TMUX` still answers `false`. Without that
/// ordering, a terminal identity signal the outer terminal sets
/// unconditionally (the same reasoning [`detect_kitty_keyboard_from`]'s doc
/// gives for `KITTY_WINDOW_ID` surviving a rewritten `TERM`) would be
/// wrongly inferred *through* a multiplexer that does not forward the mode
/// at all.
pub(crate) fn detect_sgr_pixel_mouse_from(getenv: impl Fn(&str) -> Option<String>) -> bool {
    if getenv("TMUX").is_some() {
        return false;
    }
    if let Some(term) = getenv("TERM") {
        let term = term.to_ascii_lowercase();
        if term.starts_with("screen") || term.starts_with("tmux") {
            return false;
        }
    }
    if let Some(term) = getenv("TERM") {
        let term = term.to_ascii_lowercase();
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
        let mut pollfd = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        let timeout_ms = i32::try_from(remaining.as_millis()).unwrap_or(i32::MAX);
        // SAFETY: `pollfd` is a single valid `libc::pollfd` on the stack;
        // `poll(2)` only reads/writes through the pointer+length (1) it's
        // given, for the duration of this call.
        let ready = unsafe { libc::poll(&mut pollfd, 1, timeout_ms) };
        if ready <= 0 {
            // 0 = timed out; negative = an error (e.g. `EINTR`). Neither is
            // worth a retry loop for a one-shot startup probe on stdin.
            return None;
        }
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

/// System dark/light detection (quadraui#952) — the TUI half of
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
        // No accent-colour or high-contrast signal exists in a terminal
        // environment — see `SystemTheme`'s field docs.
        accent: None,
        high_contrast: false,
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

    /// The acceptance-criteria case named directly in quadraui#826:
    /// `TERM=xterm-256color` with no `COLORTERM` set at all must land on
    /// indexed, not truecolor.
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

    // ── Kitty keyboard protocol (quadraui#827) ──────────────────────────

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

    /// The acceptance-criteria case for quadraui#827's degrade table: a
    /// plain `TERM=xterm-256color` session (raw SSH, tmux without
    /// `terminal-features`) has none of the three positive signals and
    /// must land on `false`, not a hopeful guess.
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

    // ── SGR-Pixels mouse mode (quadraui#1048) ───────────────────────────

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

    // ── DECRQM reply parsing (quadraui#1048) ────────────────────────────

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

    /// A timeout (no reply parsed at all — represented as `None`, the same
    /// value a truncated/unparseable reply produces) must resolve to
    /// `false`, never fall back to a heuristic guess.
    #[test]
    fn decrqm_timeout_reports_the_mode_unsupported() {
        assert!(!decrqm_reply_supports_mode(None));
    }

    // ── System theme detection (quadraui#952) ───────────────────────────

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
