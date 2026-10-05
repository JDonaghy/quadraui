//! Regression test: `TuiBackend::wait_events` must still surface an event
//! crossterm already parsed and buffered internally during an unrelated
//! *filtered* read (e.g. the cursor-position query this crate's own
//! capability negotiation performs), even though the fd itself has
//! nothing left on it by the time `wait_events` runs.
//!
//! crossterm's internal event reader buffers any event it parses but that
//! doesn't match the filter a caller gave it (`skipped_events`), and its own
//! `poll`/`read` consult that buffer *before* ever touching the fd again. A
//! plain keypress byte sitting in the same chunk as a cursor-position reply
//! is parsed, doesn't match `CursorPositionFilter`, and is buffered rather
//! than discarded — so by the time `wait_events` is asked for the next
//! event, the fd itself reports nothing new at all, but crossterm is still
//! holding a real event. This test writes both into the master in a single
//! call, performs the filtered read directly (`crossterm::cursor::position`,
//! the exact call this crate's own `TuiRunner::new` negotiation makes), and
//! then asserts `TuiBackend::wait_events` returns the buffered keypress
//! promptly — with no further bytes ever written to the master.
//!
//! ## Why this is its own test binary
//!
//! crossterm's internal event reader is a single process-wide instance,
//! bound (via mio) to whatever file was installed at file descriptor 0 the
//! first time crossterm's blocking event machinery actually engaged it. A
//! later `dup2` onto that same fd number (as every test in this file's
//! siblings performs, to redirect stdin to its own pty) does not rebind
//! that registration — so a test that needs crossterm to perform a real
//! blocking read, like this one, cannot share a process with any other
//! test that has already done the same against a *different* fd. See
//! `tests/pty_support/mod.rs`'s module doc.
//!
//! ## Why this test mutates real process state, and why it's `#[ignore]`d
//!
//! Same caveat as `tests/crossterm_dead_pty_busy_loop.rs`: dups a real pty
//! slave onto this test binary's own `STDIN_FILENO`/`STDOUT_FILENO` —
//! process-global state. Run explicitly:
//!
//! ```sh
//! cargo test --features tui --test wait_events_recovers_crossterm_buffered_event -- --ignored
//! ```
#![cfg(all(feature = "tui", unix))]

#[path = "pty_support/mod.rs"]
mod pty_support;

use std::time::{Duration, Instant};

use quadraui::tui::TuiBackend;
use quadraui::{Backend, Key, UiEvent};

use pty_support::{open_pty_pair, StdinOverride, StdoutOverride};

#[test]
#[ignore = "mutates this process's fd 0 and fd 1 — see module doc"]
fn wait_events_recovers_an_event_crossterm_buffered_during_a_filtered_read() {
    let (master_fd, slave_fd) = open_pty_pair();
    let _stdin_override = StdinOverride::install(slave_fd);
    let _stdout_override = StdoutOverride::install(slave_fd);

    // A fresh pty defaults to canonical mode, which buffers input per line
    // until a terminator arrives — the reply below has none. Put it in raw
    // mode first so the bytes land in the read queue immediately rather
    // than waiting on a newline that never comes.
    {
        let mut term: libc::termios = unsafe { std::mem::zeroed() };
        assert_eq!(
            unsafe { libc::tcgetattr(slave_fd, &mut term) },
            0,
            "tcgetattr failed: {:?}",
            std::io::Error::last_os_error()
        );
        unsafe { libc::cfmakeraw(&mut term) };
        assert_eq!(
            unsafe { libc::tcsetattr(slave_fd, libc::TCSANOW, &term) },
            0,
            "tcsetattr failed: {:?}",
            std::io::Error::last_os_error()
        );
    }

    // A canned cursor-position reply (`ESC[row;colR`) immediately followed,
    // in the exact same `write(2)` call, by one ordinary printable
    // character. Writing both before `crossterm::cursor::position()` is
    // ever called means they are already sitting in the slave's read
    // buffer together, so crossterm's first (and only) `read()` picks up
    // both in one chunk — no timing race to land.
    let reply_and_stray_key = b"\x1b[5;5Rq";
    // SAFETY: `reply_and_stray_key` is a valid slice for its own length;
    // `write(2)` only reads from it, for at most that many bytes.
    let written = unsafe {
        libc::write(
            master_fd,
            reply_and_stray_key.as_ptr().cast(),
            reply_and_stray_key.len(),
        )
    };
    assert_eq!(
        written,
        reply_and_stray_key.len() as isize,
        "write(2) of the canned reply + stray key failed: {:?}",
        std::io::Error::last_os_error()
    );

    // The exact filtered read this crate's own startup negotiation
    // performs (`TuiRunner::new`) — blocks on crossterm's internal reader
    // until a `CursorPosition` event is parsed, buffering anything else it
    // parses along the way rather than discarding it.
    crossterm::cursor::position().expect(
        "crossterm::cursor::position() against the canned reply written above should succeed",
    );

    let mut backend = TuiBackend::new();

    // No further bytes are ever written to the master — if this returns
    // the stray key at all, it can only be the one crossterm already had
    // buffered internally from the read above.
    let start = Instant::now();
    let events = backend.wait_events(Duration::from_secs(1));
    let elapsed = start.elapsed();

    assert!(
        elapsed < Duration::from_millis(500),
        "wait_events took {elapsed:?} to return a crossterm-buffered event that required no \
         further fd activity at all — it should have been immediate"
    );
    assert!(
        events.iter().any(|ev| matches!(
            ev,
            UiEvent::KeyPressed {
                key: Key::Char('q'),
                ..
            }
        )),
        "wait_events did not surface the stray 'q' keypress crossterm buffered internally \
         during the preceding filtered cursor-position read; got {events:?}"
    );

    // SAFETY: both fds are plain, this-process-owned fds opened above.
    unsafe {
        libc::close(master_fd);
        libc::close(slave_fd);
    }
}
