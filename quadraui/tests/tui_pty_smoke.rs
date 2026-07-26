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
}

/// How long to wait for the example to render / react before giving up.
/// Generous because the first run compiles `cargo run --example` from
/// scratch if the target dir is cold.
const WAIT: Duration = Duration::from_secs(60);

impl PtyExample {
    /// Spawns `cargo run --quiet --example <name> --features tui` inside a
    /// freshly opened PTY sized `cols`x`rows`.
    fn spawn(name: &str, cols: u16, rows: u16) -> Self {
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
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let chunk = &buf[..n];
                        parser_bg.lock().unwrap().process(chunk);
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
