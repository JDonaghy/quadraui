//! Terminal colour-depth detection (quadraui#826).
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
}
