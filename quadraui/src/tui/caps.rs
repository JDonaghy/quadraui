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

use crate::backend::ColorDepth;

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
}
