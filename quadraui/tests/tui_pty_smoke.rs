//! Tier-3 pty black-box smoke tests for TUI examples (#302).
//!
//! [`quadraui::tui::testing::TuiDriver`] (`tests/tui_example_driver.rs`,
//! #300) renders into ratatui's in-memory `TestBackend` — it never touches a
//! real TTY, so terminal-protocol bugs are invisible to it: raw-mode / alt-
//! screen setup, real ANSI escape-sequence emission and parsing, SGR mouse
//! decoding (e.g. #293's class — mouse motion leaking an SGR sequence into a
//! focused input). This file closes that gap by spawning the *actual*
//! example binary in a real pseudo-terminal (`portable-pty`, the same crate
//! `terminal_engine.rs` uses for the embedded-terminal primitive) and
//! parsing its emitted byte stream with `vt100` into a screen model.
//!
//! Deliberately thin — 2 representative examples, not broad coverage. The
//! deterministic in-process `TuiDriver` remains the primary tool; see the
//! "Tier-3 pty smoke" section of `quadraui/docs/TESTING.md`.
#![cfg(all(feature = "tui", feature = "terminal"))]

use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};

/// A running `cargo run --example <name> --features tui` process wired to a
/// real PTY, with its output continuously parsed into a `vt100::Screen`.
///
/// The PTY *master* side plays the role a real terminal emulator plays for
/// a normal interactive session — including answering the escape-sequence
/// queries a real terminal answers. `ratatui`'s crossterm backend queries
/// the cursor position (`ESC [ 6 n`, expects `ESC [ row ; col R` back) once
/// during `Terminal::new()`, and treats a missing reply as fatal. A dumb
/// byte-in/byte-out pty with nothing on the master side to answer that
/// query would make every example fail before it ever renders — so the
/// background reader thread below acts as that minimal terminal-emulator
/// stand-in.
struct PtyExample {
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    // Keeps the PTY master's file descriptors alive for the life of the
    // session — dropping it early can tear down the slave side under the
    // child. Never read directly; `writer`/the reader thread hold the
    // handles actually used.
    _master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    parser: Arc<Mutex<vt100::Parser>>,
    // Every byte the child has written so far, verbatim — `vt100::Parser`
    // is a *screen model*, and deliberately abstracts away exactly the
    // thing quadraui#826's SGR-depth fixtures need to observe: the raw
    // escape-sequence bytes (`38;2;…` truecolor vs `38;5;…` indexed) a
    // real terminal receives. `screen_text()`/`wait_for` stay the
    // content-only assertions every other test here already used; the raw
    // log is additive, read via `raw_contains`/`wait_for_raw`.
    raw: Arc<Mutex<Vec<u8>>>,
}

/// How long to wait for the example to render / react before giving up.
/// Generous because the first run compiles `cargo run --example` from
/// scratch if the target dir is cold.
const WAIT: Duration = Duration::from_secs(60);

/// How long to wait for a keystroke's *effect to disappear* from the screen
/// (an input cleared by Escape). Much shorter than [`WAIT`] because nothing
/// has to compile or start by this point — the example is already running
/// and has already echoed typed text — but far longer than the round trip
/// actually takes, so a loaded runner can't turn a pass into a failure.
/// Only the failure path pays it.
#[cfg_attr(not(unix), allow(dead_code))]
const ESC_SETTLE: Duration = Duration::from_secs(10);

impl PtyExample {
    /// Spawns `cargo run --quiet --example <name> --features tui` inside a
    /// freshly opened PTY sized `cols`x`rows`, with `TERM=xterm-256color`
    /// and no `COLORTERM` — every pre-#826 caller's environment, and
    /// (quadraui#826) the exact environment the issue's acceptance
    /// criteria name for the indexed-SGR fixture.
    fn spawn(name: &str, cols: u16, rows: u16) -> Self {
        Self::spawn_with_env(name, cols, rows, &[("TERM", "xterm-256color")])
    }

    /// Same as [`Self::spawn`], but with `extra_env` applied *after* the
    /// `TERM=xterm-256color` default — so a caller can override `TERM`
    /// and/or add `COLORTERM` to exercise a different
    /// [`quadraui::ColorDepth`] detection outcome (quadraui#826).
    fn spawn_with_env(name: &str, cols: u16, rows: u16, extra_env: &[(&str, &str)]) -> Self {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("openpty");

        let mut cmd = CommandBuilder::new("cargo");
        cmd.args(["run", "--quiet", "--example", name, "--features", "tui"]);
        cmd.cwd(env!("CARGO_MANIFEST_DIR"));
        cmd.env("TERM", "xterm-256color");
        // `CommandBuilder::new()` seeds its env map from the *entire*
        // parent process environment (portable-pty-0.9.0's
        // `get_base_env`), so a `COLORTERM` ambiently set in the shell
        // `cargo test` runs under (VS Code's integrated terminal, tmux
        // with passthrough, some terminal emulators) would otherwise leak
        // into the child and make `detect_color_depth()` resolve to
        // `TrueColor` regardless of the `TERM`/`extra_env` this test
        // intends to exercise. Strip it here so only an explicit entry in
        // `extra_env` below can reintroduce it — the indexed/ansi16
        // fixtures below depend on this being absent unless they add it
        // back themselves.
        cmd.env_remove("COLORTERM");
        for (key, value) in extra_env {
            cmd.env(key, value);
        }

        let child = pair
            .slave
            .spawn_command(cmd)
            .unwrap_or_else(|e| panic!("failed to spawn example {name}: {e}"));
        // The slave fd is owned by the child now; drop our copy so EOF on
        // the master side is detected once the child exits.
        drop(pair.slave);

        let writer: Arc<Mutex<Box<dyn Write + Send>>> = Arc::new(Mutex::new(
            pair.master.take_writer().expect("take pty writer"),
        ));
        let mut reader = pair.master.try_clone_reader().expect("clone pty reader");
        let master = pair.master;

        let parser = Arc::new(Mutex::new(vt100::Parser::new(rows, cols, 0)));
        let parser_bg = Arc::clone(&parser);
        let writer_bg = Arc::clone(&writer);
        let raw = Arc::new(Mutex::new(Vec::new()));
        let raw_bg = Arc::clone(&raw);
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let chunk = &buf[..n];
                        parser_bg.lock().unwrap().process(chunk);
                        raw_bg.lock().unwrap().extend_from_slice(chunk);
                        // Answer `ESC [ 6 n` (cursor position report) the way
                        // a real terminal would — see the struct doc. Without
                        // this, `Terminal::new()` fails immediately on a
                        // real pty and no example ever renders.
                        if contains_subslice(chunk, b"\x1b[6n") {
                            let (row, col) = parser_bg.lock().unwrap().screen().cursor_position();
                            let reply = format!("\x1b[{};{}R", row + 1, col + 1);
                            let mut w = writer_bg.lock().unwrap();
                            let _ = w.write_all(reply.as_bytes());
                            let _ = w.flush();
                        }
                    }
                }
            }
        });

        Self {
            writer,
            _master: master,
            child,
            raw,
            parser,
        }
    }

    fn send(&mut self, bytes: &[u8]) {
        let mut w = self.writer.lock().unwrap();
        w.write_all(bytes).expect("write to pty stdin");
        w.flush().expect("flush pty stdin");
    }

    fn send_str(&mut self, s: &str) {
        self.send(s.as_bytes());
    }

    fn screen_text(&self) -> String {
        self.parser.lock().unwrap().screen().contents()
    }

    /// Polls the emulated screen until it contains `needle`, or times out.
    fn wait_for(&self, needle: &str, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            if self.screen_text().contains(needle) {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    /// Polls the emulated screen until it *stops* containing `needle`, or
    /// times out.
    ///
    /// The mirror of [`Self::wait_for`], for assertions about something
    /// being removed from the screen (an input cleared, a toast dismissed).
    /// A fixed `sleep` before sampling would work only as long as the whole
    /// write → pty → event loop → repaint → `vt100` round trip fits inside
    /// it, which is not a property this harness can guarantee on a loaded
    /// CI runner — this returns the moment the change lands and only pays
    /// the full `timeout` when it genuinely never does.
    ///
    /// Unix-only in practice: its sole caller is `#[cfg(unix)]` (see
    /// [`tui_chat_escape_glued_to_sgr_motion_in_one_write_does_not_leak`]),
    /// so the `allow(dead_code)` keeps the `-D warnings` clippy leg green
    /// on Windows — same pattern as [`Self::raw_contains`] above.
    #[cfg_attr(not(unix), allow(dead_code))]
    fn wait_for_absence(&self, needle: &str, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            if !self.screen_text().contains(needle) {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    /// Whether the raw byte stream observed so far contains `needle`
    /// verbatim — for asserting on exact SGR escape sequences (quadraui#826),
    /// which `screen_text()` can't see (it's `vt100`'s parsed *content*,
    /// not the bytes that produced it).
    ///
    /// Only meaningful on Unix — see the `sgr_color_depth` module below
    /// for why, and why its callers are `#[cfg(unix)]`. The
    /// `allow(dead_code)` keeps the `-D warnings` clippy leg green on
    /// Windows, where nothing calls this (and, by seeding this method as
    /// a dead-code root, keeps the `raw` field it reads live too).
    #[cfg_attr(not(unix), allow(dead_code))]
    fn raw_contains(&self, needle: &[u8]) -> bool {
        contains_subslice(&self.raw.lock().unwrap(), needle)
    }

    /// Polls the raw byte stream until it contains `needle`, or times out.
    /// Unix-only for the same reason as [`Self::raw_contains`].
    #[cfg_attr(not(unix), allow(dead_code))]
    fn wait_for_raw(&self, needle: &[u8], timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            if self.raw_contains(needle) {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    /// Polls until the child process has exited, or times out.
    fn wait_exit(&mut self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }
}

/// Naive substring search over raw bytes — `[u8]` has no `contains(&[u8])`.
fn contains_subslice(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

impl Drop for PtyExample {
    fn drop(&mut self) {
        // Best-effort cleanup: if a test fails/panics mid-way, don't leak a
        // live `cargo run` process holding a raw-mode PTY.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

// ─── tui_pipeline: real raw-mode render + real arrow-key decoding ─────────

/// Drives the actual `tui_pipeline` binary over a real pty: confirms the
/// alt-screen + raw-mode setup in `tui::run` produces the expected initial
/// render, that real ANSI arrow-key escape sequences decode to focus moves,
/// that Enter fires the focused stage's action, and that `q` cleanly exits
/// the process (raw mode is torn down, not left hanging).
#[test]
fn tui_pipeline_keyboard_and_quit_roundtrip() {
    let mut ex = PtyExample::spawn("tui_pipeline", 100, 30);

    // Real render over the pty: alt-screen entered, raw mode active, the
    // ratatui frame painted — none of which TuiDriver's TestBackend touches.
    assert!(
        ex.wait_for("Deploy", WAIT),
        "example did not render expected pipeline stages over the pty; screen:\n{}",
        ex.screen_text()
    );

    // Right, Right: real SGR/ANSI cursor-key escapes (ESC [ C), not an
    // injected UiEvent — moves focus Build -> Test -> Deploy.
    ex.send(b"\x1b[C\x1b[C");
    // Enter: fires the focused stage's action ("Go" on Deploy).
    ex.send(b"\r");

    assert!(
        ex.wait_for("Go on 'Deploy'", WAIT),
        "arrow-key + Enter roundtrip did not fire the Deploy action over a real pty; screen:\n{}",
        ex.screen_text()
    );

    ex.send(b"q");
    assert!(
        ex.wait_exit(WAIT),
        "example did not exit after 'q' — raw-mode teardown or event loop may be hanging"
    );
}

// ─── tui_chat: SGR mouse-motion round-trip (#293 class) ───────────────────

/// Sends a raw SGR mouse-motion report (`ESC [ < 35 ; x ; y M` — motion, no
/// button, per xterm's SGR encoding) directly to the pty's stdin, mid-way
/// through composing an input. This is exactly the byte-for-byte shape #293
/// reported leaking into a focused `TextInput` as literal text. Confirms the
/// escape sequence decodes to a mouse event (or is otherwise consumed) and
/// never spills its bytes into the transcript, and that typing still works
/// normally afterwards.
#[test]
fn tui_chat_sgr_mouse_motion_does_not_leak_into_input() {
    let mut ex = PtyExample::spawn("tui_chat", 100, 30);

    assert!(
        ex.wait_for("Ctrl+Enter or Alt+Enter to send", WAIT),
        "chat example did not render its status strip over the pty; screen:\n{}",
        ex.screen_text()
    );

    // A pure-motion SGR mouse report: Cb=35 (32 motion + 3 no-button),
    // column 10, row 5. No real mouse is involved — this is what the
    // terminal emits while the cursor merely moves over a mouse-tracking
    // pane, the exact shape #293 was filed against.
    ex.send(b"\x1b[<35;10;5M");
    // Give the parser a moment to process before sampling the screen.
    std::thread::sleep(Duration::from_millis(200));

    let after_motion = ex.screen_text();
    assert!(
        !after_motion.contains("35;10;5"),
        "raw SGR mouse-motion bytes leaked into the rendered screen (#293 class):\n{after_motion}"
    );

    // The input must still work normally after the mouse event.
    ex.send_str("hello");
    assert!(
        ex.wait_for("hello", WAIT),
        "input stopped accepting text after an SGR mouse-motion report; screen:\n{}",
        ex.screen_text()
    );

    ex.send(b"\x03"); // Ctrl+C — quit immediately.
    assert!(
        ex.wait_exit(WAIT),
        "chat example did not exit after Ctrl+C — raw-mode teardown or event loop may be hanging"
    );
}

/// A real Escape keypress landing in the *same* pty write() as an
/// adjacent SGR mouse-motion report — the exact race #293's root cause
/// documents (see `recover_leaked_sgr_mouse_fragments`'s doc in
/// `src/tui/backend.rs`): crossterm's reader can split `ESC [ < …
/// (M|m)` right after the leading `ESC`, decode that lone byte as a
/// standalone Escape, and then leak the report's tail as literal
/// characters. Three byte orderings a real terminal's write scheduling
/// could plausibly produce, all coalesced into one `write()` so the
/// race actually has a chance to fire: motion-then-Escape,
/// Escape-then-motion, and Escape sandwiched between two motions (a
/// continuous any-motion stream with a keystroke landing mid-burst).
/// Escape must still clear the input in every case, and no fragment of
/// the SGR report may leak into the transcript.
///
/// **`#[cfg(unix)]` — deliberately, for the same transport reason as the
/// `sgr_color_depth` module below, and this gate must not be removed.**
/// This fixture works by controlling the exact *byte* boundaries the
/// terminal-input reader sees ("all coalesced into one `write()` so the
/// race actually has a chance to fire"), and that lever only exists where
/// crossterm parses a byte stream — i.e. its Unix event source, whose
/// `parse_event(buffer, input_available)` is the very function that
/// resolves a lone `ESC` to a standalone Escape when no further bytes are
/// already buffered. That is #293's whole mechanism.
///
/// On Windows crossterm never parses bytes at all: `event::source::windows`
/// reads `INPUT_RECORD`s through the console API (`read_single_input_event`
/// → `handle_key_event`/`handle_mouse_event`), so there is no buffer, no
/// `input_available`, and structurally no split-ESC race to reproduce.
/// Worse, the bytes this test writes never reach the child as bytes:
/// `portable-pty`'s Windows backend is ConPTY, so conhost's own VT *input*
/// state machine parses `\x1b`/`\x1b[<…M` on the master side and hands the
/// child whatever records *it* decides they mean — its escape
/// disambiguation, its mouse translation, its flush-at-end-of-string
/// timing. A failure here on windows-latest therefore reports on conhost's
/// input parser, not on quadraui's recovery path, and no change to
/// `recover_leaked_sgr_mouse_fragments` could move it either way. (Which is
/// exactly what happened: this fixture failed on windows-latest while both
/// content-level pty tests above passed on the same runner.)
///
/// What Windows keeps: the recovery logic itself, unit-tested end-to-end
/// over real `UiEvent` batches in `src/tui/backend.rs`'s
/// `recover_leaked_sgr_mouse_fragments` tests (pure-motion, click down/up,
/// drag, non-matching runs, multiple leaks per batch) — platform-independent,
/// and run by the same windows-latest job — plus both pty tests above.
#[cfg(unix)]
#[test]
fn tui_chat_escape_glued_to_sgr_motion_in_one_write_does_not_leak() {
    let cases: [(&str, Vec<u8>); 3] = [
        ("motion-then-escape", {
            let mut b = b"\x1b[<35;10;5M".to_vec();
            b.push(0x1b);
            b
        }),
        ("escape-then-motion", {
            let mut b = vec![0x1bu8];
            b.extend_from_slice(b"\x1b[<35;10;5M");
            b
        }),
        ("escape-sandwiched-between-motions", {
            let mut b = b"\x1b[<35;10;5M".to_vec();
            b.push(0x1b);
            b.extend_from_slice(b"\x1b[<35;11;5M");
            b
        }),
    ];

    for (label, combined) in cases {
        let mut ex = PtyExample::spawn("tui_chat", 100, 30);
        assert!(
            ex.wait_for("Ctrl+Enter or Alt+Enter to send", WAIT),
            "[{label}] chat example did not render its status strip over the pty"
        );

        ex.send_str("hello");
        assert!(
            ex.wait_for("hello", WAIT),
            "[{label}] typed text should appear before the race is triggered"
        );

        ex.send(&combined);
        // Wait for the Escape to take effect rather than sampling after a
        // fixed sleep — the round trip (write → pty → 16 ms event-loop poll
        // → redraw → `vt100`) is fast, but not bounded on a loaded CI
        // runner. Returns as soon as the input clears.
        let cleared = ex.wait_for_absence("hello", ESC_SETTLE);
        // A leaked fragment lands in the input in the same batch as the
        // (mis-)decoded Escape, but let one more redraw settle before
        // sampling so a fragment arriving just after the clear is still seen.
        std::thread::sleep(Duration::from_millis(150));

        let screen = ex.screen_text();
        assert!(
            !screen.contains("35;1"),
            "[{label}] raw SGR mouse bytes leaked into the input after an interleaved Escape \
             (#293 class):\n{screen}"
        );
        assert!(
            cleared && !screen.contains("hello"),
            "[{label}] Escape did not clear the input within {ESC_SETTLE:?} — either it was \
             swallowed by the adjacent SGR report or the input got polluted before Escape \
             ran:\n{screen}"
        );

        ex.send(b"\x03"); // Ctrl+C — quit immediately.
        assert!(
            ex.wait_exit(WAIT),
            "[{label}] chat example did not exit after Ctrl+C"
        );
    }
}

// ─── tui_pipeline: colour-depth SGR quantisation (quadraui#826) ───────────

/// Byte-exact SGR-family fixtures for #826's colour-depth quantisation.
///
/// **`#[cfg(unix)]` — deliberately, and this gate must not be removed.**
/// Every assertion in here reads the *raw escape-sequence bytes* that
/// arrive on the pty master, and that observation channel is only
/// faithful on Unix. There, `openpty(3)` is a kernel byte pipe: whatever
/// crossterm writes to the slave fd arrives at the master verbatim, so
/// the master genuinely sees "the bytes a real terminal would receive
/// from our app".
///
/// On Windows there is no such device. `portable-pty`'s Windows backend
/// is ConPTY (`CreatePseudoConsole` — see `portable-pty`'s
/// `src/win/psuedocon.rs`), which puts a *whole conhost terminal
/// emulator* between the child and the master handle: the child's bytes
/// are parsed into conhost's own text buffer, and what the master then
/// reads is conhost's re-serialised repaint of that buffer, not the
/// child's output. Its VT renderer re-encodes attributes on its own
/// terms — separate `38;…m` / `48;…m` sequences rather than crossterm's
/// combined `SetColors` form, its own palette normalisation, its own
/// redraw batching — so a byte-exact match on any of the three constants
/// below can never hold there.
///
/// That this is a property of the *transport*, not of #826's colour
/// logic, is provable from the CI failure that added this gate: the
/// `..._is_byte_identical_to_before` fixture, whose expected bytes are
/// the ones quadraui emitted *before* #826 existed, failed on
/// windows-latest too. There was never a Windows run in which those
/// bytes were observable to regress from.
///
/// What Windows keeps: `detect_color_depth`'s full decision table
/// (`src/tui/caps.rs`), `DepthLimitedBackend`'s per-cell quantisation
/// including the pipeline theme pair end-to-end through a real
/// `RtBackend::draw` (`src/tui/color.rs`), and the two content-level pty
/// fixtures above — all platform-independent, all run by the same
/// windows-latest job. Only the SGR *byte encoding* — crossterm's job,
/// not quadraui's — goes unobserved there.
///
/// Within this module, all three fixtures assert on the exact combined
/// `SetColors` sequence `draw_pipeline_view` paints stage names with:
/// `Theme::default()`'s `(foreground, surface_bg)` pair, `(220,220,220)`
/// / `(28,32,44)`. They drive the real `tui::run` event loop (not a
/// unit-tested `quantize()` call) — `screen_text()`/`vt100::Color` can't
/// tell a `38;2;…` byte sequence from a `38;5;…` one, since `vt100`
/// resolves both down to a colour, not an SGR family. The four quantised
/// numbers (`253`/`234` for `Indexed256`, `7`/`0` for `Ansi16`) are
/// pinned independently in `src/tui/color.rs`'s
/// `theme_default_pipeline_colors_quantise_to_the_values_the_pty_fixture_expects`
/// unit test — if quadraui's own palette math ever drifts, that much
/// faster in-process test catches it before this pty test would.
#[cfg(unix)]
mod sgr_color_depth {
    use super::*;

    /// The exact combined-`SetColors` SGR sequence `draw_pipeline_view` emits
    /// for `(fg, bg)` at 24-bit truecolor: `Theme::default().foreground` =
    /// `Color::rgb(220, 220, 220)`, `.surface_bg` = `Color::rgb(28, 32, 44)`.
    const TRUECOLOR_PIPELINE_SGR: &[u8] = b"\x1b[38;2;220;220;220;48;2;28;32;44m";

    /// The nearest indexed-256 quantisation of the same pair —
    /// `rgb_to_indexed256(220,220,220) == 253` (grayscale ramp),
    /// `rgb_to_indexed256(28,32,44) == 234` (grayscale ramp).
    const INDEXED256_PIPELINE_SGR: &[u8] = b"\x1b[38;5;253;48;5;234m";

    /// The nearest 16-colour (`Ansi16`) quantisation of the same pair —
    /// `rgb_to_ansi16(220,220,220) == Gray` (SGR index 7),
    /// `rgb_to_ansi16(28,32,44) == Black` (SGR index 0).
    const ANSI16_PIPELINE_SGR: &[u8] = b"\x1b[38;5;7;48;5;0m";

    /// Acceptance item 2: `TERM=xterm-256color` with no `COLORTERM` set — the
    /// common case (plain SSH, tmux without passthrough) #826 was filed
    /// against — must emit indexed SGR, never truecolor. This is exactly
    /// `PtyExample::spawn`'s default environment (unchanged by #826, since it
    /// was already the right fixture environment), so this test is the RED
    /// case from the issue turned GREEN: before #826's `DepthLimitedBackend`
    /// wrapper existed, this assertion would have failed against the
    /// unconditional-truecolor `38;2;…` output.
    #[test]
    fn tui_pipeline_under_256color_term_emits_indexed_sgr_not_truecolor() {
        let mut ex = PtyExample::spawn("tui_pipeline", 100, 30);

        assert!(
            ex.wait_for("Deploy", WAIT),
            "example did not render expected pipeline stages over the pty; screen:\n{}",
            ex.screen_text()
        );

        assert!(
            ex.wait_for_raw(INDEXED256_PIPELINE_SGR, WAIT),
            "TERM=xterm-256color with no COLORTERM did not produce the expected indexed SGR \
             sequence {:?} — output stayed (or never became) indexed",
            String::from_utf8_lossy(INDEXED256_PIPELINE_SGR)
        );
        assert!(
            !ex.raw_contains(b"38;2;"),
            "TERM=xterm-256color with no COLORTERM still emitted truecolor SGR (`38;2;…`) — \
             this is the exact bug #826 reports: truecolor unconditionally, even on a terminal \
             that only declared 256-colour support"
        );

        ex.send(b"q");
        assert!(
            ex.wait_exit(WAIT),
            "example did not exit after 'q' — raw-mode teardown or event loop may be hanging"
        );
    }

    /// Acceptance item 4: a truecolor terminal (`COLORTERM=truecolor`) must
    /// stay byte-identical to pre-#826 output — the `DepthLimitedBackend`
    /// wrapper's `ColorDepth::TrueColor` arm is a pure pass-through, so this
    /// asserts the exact 24-bit SGR sequence still appears, unchanged.
    #[test]
    fn tui_pipeline_with_colorterm_truecolor_is_byte_identical_to_before() {
        let mut ex = PtyExample::spawn_with_env(
            "tui_pipeline",
            100,
            30,
            &[("TERM", "xterm-256color"), ("COLORTERM", "truecolor")],
        );

        assert!(
            ex.wait_for("Deploy", WAIT),
            "example did not render expected pipeline stages over the pty; screen:\n{}",
            ex.screen_text()
        );

        assert!(
            ex.wait_for_raw(TRUECOLOR_PIPELINE_SGR, WAIT),
            "COLORTERM=truecolor did not produce the exact pre-#826 24-bit SGR sequence {:?}",
            String::from_utf8_lossy(TRUECOLOR_PIPELINE_SGR)
        );

        ex.send(b"q");
        assert!(
            ex.wait_exit(WAIT),
            "example did not exit after 'q' — raw-mode teardown or event loop may be hanging"
        );
    }

    /// Acceptance item 3: the `Ansi16` path exists and is reachable, not just
    /// unit-tested. `TERM=xterm` (no `256color`, no `COLORTERM`) is the
    /// conservative-fallback case — a terminal this backend cannot positively
    /// identify as supporting more than 16 colours.
    #[test]
    fn tui_pipeline_under_plain_xterm_emits_ansi16_sgr() {
        let mut ex = PtyExample::spawn_with_env("tui_pipeline", 100, 30, &[("TERM", "xterm")]);

        assert!(
            ex.wait_for("Deploy", WAIT),
            "example did not render expected pipeline stages over the pty; screen:\n{}",
            ex.screen_text()
        );

        assert!(
            ex.wait_for_raw(ANSI16_PIPELINE_SGR, WAIT),
            "plain TERM=xterm did not produce the expected 16-colour SGR sequence {:?}",
            String::from_utf8_lossy(ANSI16_PIPELINE_SGR)
        );
        assert!(
            !ex.raw_contains(b"38;2;"),
            "plain TERM=xterm emitted truecolor SGR (`38;2;…`) — should have quantised to Ansi16"
        );
        assert!(
            !ex.raw_contains(INDEXED256_PIPELINE_SGR),
            "plain TERM=xterm reused the 256-colour quantisation ({:?}) instead of actually \
             taking the Ansi16 path",
            String::from_utf8_lossy(INDEXED256_PIPELINE_SGR)
        );

        ex.send(b"q");
        assert!(
            ex.wait_exit(WAIT),
            "example did not exit after 'q' — raw-mode teardown or event loop may be hanging"
        );
    }
}

// ─── tui_pipeline: kitty keyboard protocol detection (quadraui#827) ───────

/// Byte-level fixtures for #827's `BackendCaps::kitty_keyboard` detection.
///
/// **`#[cfg(unix)]` — deliberately, for the same transport reason as
/// `sgr_color_depth` above, and this gate must not be removed.** Both
/// fixtures below assert on the exact raw bytes `PushKeyboardEnhancementFlags`
/// writes (`ESC[>11u` — `DISAMBIGUATE_ESCAPE_CODES | REPORT_EVENT_TYPES |
/// REPORT_ALL_KEYS_AS_ESCAPE_CODES` = `1 | 2 | 8` = `11`), and ConPTY's
/// re-serialising terminal emulator sits between the child and the master
/// handle on Windows exactly as `sgr_color_depth`'s doc describes — a
/// byte-exact match there proves nothing about quadraui's own logic.
/// `ratatui::crossterm`'s Windows `supports_keyboard_enhancement()` is a
/// separate, unconditional `Ok(false)` in any case — see
/// `docs/KITTY_KEYBOARD_PROTOCOL.md`'s degrade table for that row.
///
/// **What these fixtures actually exercise.** This harness's simulated
/// terminal (the background reader thread in [`PtyExample::spawn`]) only
/// ever answers the cursor-position query (`ESC[6n`); it never answers the
/// kitty-protocol query (`ESC[?u ESC[c`) crossterm's
/// `supports_keyboard_enhancement()` sends. That means the *live* probe
/// always times out here (`Err`, after ~2s) — these fixtures observe
/// `crate::tui::caps::probe_kitty_keyboard`'s **fallback to the
/// environment heuristic**, not a real terminal answering the live query.
/// That fallback path is exactly what a tmux session without passthrough,
/// a mosh link, or a serial console hits too — see
/// `docs/KITTY_KEYBOARD_PROTOCOL.md` for the full table and which rows
/// remain genuinely untested (no real kitty/WezTerm/foot binary, no tmux,
/// no mosh, no macOS/Windows host, is available to this fixture).
///
/// **Observed RED before the fix.** Before #827, `push_keyboard_enhancement`
/// resolved an unanswered live query straight to `false`
/// (`supports_keyboard_enhancement().unwrap_or(false)`, no environment
/// fallback at all) — so `tui_pipeline_under_kitty_term_pushes_enhancement_flags`
/// below would have failed even under `TERM=xterm-kitty`, the exact
/// environment its own name promises the protocol is pushed for.
#[cfg(unix)]
mod kitty_keyboard_protocol {
    use super::*;

    /// The exact bytes `PushKeyboardEnhancementFlags` writes for the flag
    /// set `crate::tui::run::push_keyboard_enhancement` requests —
    /// `DISAMBIGUATE_ESCAPE_CODES (1) | REPORT_EVENT_TYPES (2) |
    /// REPORT_ALL_KEYS_AS_ESCAPE_CODES (8) = 11`.
    const PUSH_KITTY_FLAGS: &[u8] = b"\x1b[>11u";

    /// `TERM=xterm-kitty` — kitty's own terminfo name, and the first signal
    /// `detect_kitty_keyboard_from` checks. With the live query unanswered
    /// (see the module doc), detection falls through to this heuristic,
    /// which must land on `true` — so the example pushes the enhancement
    /// flags, observable as the exact `ESC[>11u` sequence on the wire.
    #[test]
    fn tui_pipeline_under_kitty_term_pushes_enhancement_flags() {
        let mut ex =
            PtyExample::spawn_with_env("tui_pipeline", 100, 30, &[("TERM", "xterm-kitty")]);

        assert!(
            ex.wait_for("Deploy", WAIT),
            "example did not render expected pipeline stages over the pty; screen:\n{}",
            ex.screen_text()
        );
        assert!(
            ex.wait_for_raw(PUSH_KITTY_FLAGS, WAIT),
            "TERM=xterm-kitty did not push the kitty keyboard enhancement flags ({:?}) — the \
             environment-heuristic fallback in `detect_kitty_keyboard_from` did not fire the way \
             quadraui#827 requires",
            String::from_utf8_lossy(PUSH_KITTY_FLAGS)
        );

        ex.send(b"q");
        assert!(
            ex.wait_exit(WAIT),
            "example did not exit after 'q' — raw-mode teardown or event loop may be hanging"
        );
    }

    /// A plain `TERM=xterm-256color` session (`PtyExample::spawn`'s
    /// default — no `COLORTERM`, no kitty/WezTerm/foot signal) — the
    /// common case named in `docs/KITTY_KEYBOARD_PROTOCOL.md`'s degrade
    /// table (a stock SSH session). Detection must land on `false`, so the
    /// enhancement flags are never pushed at all.
    #[test]
    fn tui_pipeline_under_plain_term_does_not_push_enhancement_flags() {
        let mut ex = PtyExample::spawn("tui_pipeline", 100, 30);

        assert!(
            ex.wait_for("Deploy", WAIT),
            "example did not render expected pipeline stages over the pty; screen:\n{}",
            ex.screen_text()
        );
        // Give the (unanswered, ~2s) live query time to time out and the
        // fallback decision to resolve before asserting absence — a
        // negative assertion sampled too early would pass for the wrong
        // reason.
        std::thread::sleep(Duration::from_secs(3));
        assert!(
            !ex.raw_contains(PUSH_KITTY_FLAGS),
            "plain TERM=xterm-256color pushed the kitty keyboard enhancement flags ({:?}) — \
             the environment heuristic should have stayed false with no kitty/WezTerm/foot \
             signal present",
            String::from_utf8_lossy(PUSH_KITTY_FLAGS)
        );

        ex.send(b"q");
        assert!(
            ex.wait_exit(WAIT),
            "example did not exit after 'q' — raw-mode teardown or event loop may be hanging"
        );
    }
}

// ─── tui_no_mouse: `RunConfig { mouse: false, .. }` withholds capture (#828) ──

/// Byte-level proof that **`no-mouse` mode** (`RunConfig::no_mouse()`,
/// `src/tui/run.rs`) actually withholds the mouse-capture escape sequences
/// instead of merely discarding whatever the terminal sends back.
///
/// Before this module, `run_with`/`RunConfig` — the only public entry point
/// for the feature — had zero call sites anywhere in `tests/` or
/// `examples/`: the `if config.mouse { EnableMouseCapture… } else { … }`
/// branch in `run_with` was reachable only in theory. `examples/tui_no_mouse.rs`
/// is the sole caller of `run_with` with a non-default `RunConfig` in the
/// tree; this module drives that binary over a real pty and inspects the
/// raw byte stream, the same technique `sgr_color_depth` and
/// `kitty_keyboard_protocol` above use for other escape-sequence claims.
///
/// **`#[cfg(unix)]` — deliberately, for the same transport reason as
/// `sgr_color_depth`/`kitty_keyboard_protocol` above.** These fixtures
/// assert on the exact bytes crossterm's `EnableMouseCapture`/
/// `DisableMouseCapture` commands write (`ESC[?1000h` et al.); on Windows,
/// ConPTY sits between the child and the master handle and re-serialises
/// its own view of the session rather than passing the child's bytes
/// through verbatim — a byte-exact presence *or* absence claim there would
/// prove something about conhost, not about `quadraui::tui::run_with`. What
/// Windows keeps: `TuiBackend::set_mouse_enabled`/`mouse_enabled`'s
/// session-level-toggle unit tests in `src/tui/backend.rs`, which are
/// platform-independent, plus this module's content-level assertion that
/// the app stays fully operable by keyboard alone — that one doesn't
/// depend on the pty being a faithful byte pipe.
#[cfg(unix)]
mod no_mouse {
    use super::*;

    /// Any one of `EnableMouseCapture`'s five constituent SGR sequences
    /// (crossterm's `event::EnableMouseCapture::write_ansi`) is sufficient
    /// evidence the terminal was asked to negotiate mouse capture at all.
    /// `?1000h` (X10/normal tracking) is the first and least ambiguous of
    /// the five — no other quadraui escape sequence starts with it.
    const MOUSE_CAPTURE_ENABLE: &[u8] = b"\x1b[?1000h";

    /// `RunConfig::no_mouse()` must never emit the mouse-capture escape
    /// sequence at all, and the app must stay fully operable by keyboard —
    /// the `[`/`]` keyboard-resize path `SplitApp` documents as the Tier-1
    /// equivalent of dragging the divider (quadraui#828).
    #[test]
    fn tui_no_mouse_never_negotiates_mouse_capture_and_stays_keyboard_operable() {
        let mut ex = PtyExample::spawn("tui_no_mouse", 100, 30);

        assert!(
            ex.wait_for("ratio: 50%", WAIT),
            "tui_no_mouse did not render SplitApp's initial status bar over the pty; screen:\n{}",
            ex.screen_text()
        );
        // Give the alt-screen/raw-mode setup a moment to fully settle
        // before sampling the raw stream — the render above only proves
        // *a* frame painted, not that setup has finished writing every
        // escape sequence it's going to write.
        std::thread::sleep(Duration::from_millis(200));

        assert!(
            !ex.raw_contains(MOUSE_CAPTURE_ENABLE),
            "RunConfig::no_mouse() still emitted the mouse-capture escape sequence {:?} — \
             the `if config.mouse {{ .. }} else {{ .. }}` branch in `tui::run_with` did not \
             withhold it",
            String::from_utf8_lossy(MOUSE_CAPTURE_ENABLE)
        );

        // Keyboard still works with no mouse ever negotiated: `]` nudges
        // the ratio up by 5% (see `KEYBOARD_RESIZE_STEP` in
        // `examples/common/split_app.rs`).
        ex.send(b"]");
        assert!(
            ex.wait_for("ratio: 55%", WAIT),
            "the keyboard-only resize path ([`/`]) did not fire in no-mouse mode; screen:\n{}",
            ex.screen_text()
        );

        ex.send(b"q");
        assert!(
            ex.wait_exit(WAIT),
            "tui_no_mouse did not exit after 'q' — raw-mode teardown or event loop may be hanging"
        );

        // The escape sequence must still be absent from everything emitted
        // across the whole session, not just the initial frame.
        assert!(
            !ex.raw_contains(MOUSE_CAPTURE_ENABLE),
            "the mouse-capture escape sequence {:?} appeared at some point during the session \
             (initial render was clean) — a later frame or the teardown path must have emitted \
             it",
            String::from_utf8_lossy(MOUSE_CAPTURE_ENABLE)
        );
    }

    /// Control case: `tui_split` — the same `SplitApp`, but run through
    /// plain `quadraui::tui::run` (`RunConfig::default()`, mouse enabled) —
    /// *does* emit the mouse-capture escape sequence. Without this, a bug
    /// that made `raw_contains` always return `false` (a typo'd needle, a
    /// reader-thread race that drops bytes before they're recorded) would
    /// make the test above pass for the wrong reason indefinitely.
    #[test]
    fn tui_split_negotiates_mouse_capture_by_default() {
        let mut ex = PtyExample::spawn("tui_split", 100, 30);

        assert!(
            ex.wait_for("ratio: 50%", WAIT),
            "tui_split did not render SplitApp's initial status bar over the pty; screen:\n{}",
            ex.screen_text()
        );

        assert!(
            ex.wait_for_raw(MOUSE_CAPTURE_ENABLE, WAIT),
            "tui_split (default RunConfig, mouse enabled) never emitted the mouse-capture \
             escape sequence {:?} — either crossterm's setup changed shape, or the \
             `raw_contains`/`wait_for_raw` observation channel this module relies on is broken",
            String::from_utf8_lossy(MOUSE_CAPTURE_ENABLE)
        );

        ex.send(b"q");
        assert!(
            ex.wait_exit(WAIT),
            "tui_split did not exit after 'q' — raw-mode teardown or event loop may be hanging"
        );
    }
}
