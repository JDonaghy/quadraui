//! Deterministic regression test for quadraui#1295 — in the shape of
//! vimcode's own `tests/crossterm_dead_pty_busy_loop.rs` (written for
//! vimcode#1735), reproduced here against `quadraui::tui::TuiBackend`
//! directly rather than a full `vcd` session.
//!
//! ## The bug
//!
//! crossterm 0.29's unix event source
//! (`UnixInternalEventSource::try_read`, `src/event/source/unix/mio.rs`)
//! has a TTY-readiness read loop whose only break arm is
//! `ErrorKind::WouldBlock` — a bare `Ok(0)` (EOF) read falls through with
//! no break and no event, so the loop immediately calls `read()` again.
//! Once a pty's master side is fully closed, every `read()` on the slave
//! returns `Ok(0)` forever (Linux does **not** turn this into an `EIO`),
//! and mio's own `poll()` keeps reporting the fd "ready" regardless of
//! requested timeout — a hangup condition is always "ready" to
//! `poll(2)`/`epoll`. The result, confirmed by a standalone `mio` +
//! `libc` reproduction in the issue's own investigation, is a genuine,
//! unbounded, un-rate-limited spin inside `crossterm::event::poll`/
//! `read` with no way for the caller to interrupt it once that call is
//! made — control never returns.
//!
//! This specifically matters for a pty that was never the process's
//! *controlling* terminal (no implicit `SIGHUP` backstop on hangup) —
//! exactly the shape a headless `vcd`-style session driven over a
//! `std::process::Command`-wired pty produces, and exactly the shape
//! this test constructs: a real pty slave dup'd onto this test
//! process's own `STDIN_FILENO`, with the master closed and zero bytes
//! ever queued in either direction, so there is no confound from a
//! trailing byte sequence that might (by luck) parse into a complete
//! event before the trap is reached.
//!
//! ## The fix under test
//!
//! `quadraui::tui::backend::TuiBackend::{poll_events,wait_events}` never
//! calls into `ratatui::crossterm::event::poll`/`read` at all once its
//! own `stdin_hung_up` guard (a non-blocking `poll(2)` on the fd,
//! checking for `POLLHUP`/`POLLERR`/`POLLNVAL` with no `POLLIN`) reports
//! the fd as hung up with nothing left to read — see that function's doc
//! in `src/tui/backend.rs` for the full rationale, including why this is
//! a guard (the vulnerable crossterm call is never reached) rather than
//! a mitigation of its behaviour once entered.
//!
//! ## Why this test mutates real process state, and why it's `#[ignore]`d
//!
//! Unlike the rest of this crate's test suite, this test must dup a real
//! pty slave onto file descriptor 0 of **the test binary's own
//! process** — `TuiBackend::wait_events`/`poll_events` read real stdin,
//! there is no injection seam for a fake fd (and adding one here would
//! test the seam, not the real crossterm/kernel interaction this issue
//! is about). File descriptor 0 is process-global state, shared by every
//! test in this binary if run with the default parallel test harness —
//! so this test saves and restores the original fd 0 around its own
//! body, but still should not be trusted to run concurrently with any
//! other test that touches stdin. Run explicitly and alone:
//!
//! ```sh
//! cargo test --features tui --test crossterm_dead_pty_busy_loop -- --ignored --test-threads=1
//! ```
#![cfg(all(feature = "tui", unix))]

use std::ffi::CStr;
use std::os::fd::RawFd;
use std::time::{Duration, Instant};

use quadraui::tui::TuiBackend;
use quadraui::Backend;

/// Opens a fresh pty pair via the raw POSIX `posix_openpt`/`grantpt`/
/// `unlockpt`/`ptsname` dance (mirroring the issue's own minimal C
/// probe), then **closes the master immediately** — the slave fd
/// returned is the only thing left alive, exactly the "pty master
/// closed" condition quadraui#1295 is about. Zero bytes are ever
/// written to either side, so there is nothing queued for the slave to
/// drain before it reports pure EOF.
fn open_pty_with_master_already_closed() -> RawFd {
    // SAFETY: standard POSIX pty-allocation sequence. `posix_openpt`
    // returns an owned fd on success (checked below); `grantpt`/
    // `unlockpt` only operate on that fd; `ptsname` returns a pointer
    // into thread-local/static storage that's immediately copied into
    // an owned `CStr`-backed `Vec<u8>` before any further libc call can
    // invalidate it.
    unsafe {
        let master_fd = libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY);
        assert!(
            master_fd >= 0,
            "posix_openpt failed: {:?}",
            std::io::Error::last_os_error()
        );
        assert_eq!(libc::grantpt(master_fd), 0, "grantpt failed");
        assert_eq!(libc::unlockpt(master_fd), 0, "unlockpt failed");

        let name_ptr = libc::ptsname(master_fd);
        assert!(!name_ptr.is_null(), "ptsname returned null");
        let slave_path = CStr::from_ptr(name_ptr).to_owned();

        let slave_fd = libc::open(slave_path.as_ptr(), libc::O_RDWR | libc::O_NOCTTY);
        assert!(
            slave_fd >= 0,
            "open(slave) failed: {:?}",
            std::io::Error::last_os_error()
        );

        // The master side is now fully closed — no other fd in this
        // process (or any other) points at it, so the slave's read end
        // sees a permanent, unambiguous hangup with nothing buffered.
        assert_eq!(libc::close(master_fd), 0, "close(master) failed");

        slave_fd
    }
}

/// RAII guard that dup's `fd` onto `STDIN_FILENO` for its lifetime, and
/// restores the test process's original stdin on drop — the save/
/// restore half of the process-global-state caveat in this file's
/// module doc.
struct StdinOverride {
    saved: RawFd,
}

impl StdinOverride {
    fn install(fd: RawFd) -> Self {
        // SAFETY: `dup`/`dup2` on process-owned fds; `STDIN_FILENO` (0)
        // is always a valid fd number to target.
        let saved = unsafe { libc::dup(libc::STDIN_FILENO) };
        assert!(
            saved >= 0,
            "dup(stdin) failed: {:?}",
            std::io::Error::last_os_error()
        );
        let result = unsafe { libc::dup2(fd, libc::STDIN_FILENO) };
        assert!(
            result >= 0,
            "dup2(fd, stdin) failed: {:?}",
            std::io::Error::last_os_error()
        );
        Self { saved }
    }
}

impl Drop for StdinOverride {
    fn drop(&mut self) {
        // SAFETY: restoring the exact fd this process started with.
        unsafe {
            libc::dup2(self.saved, libc::STDIN_FILENO);
            libc::close(self.saved);
        }
    }
}

/// How long [`TuiBackend::wait_events`]/[`TuiBackend::poll_events`] are
/// allowed to take once stdin has hung up. Generous relative to the
/// microseconds a `poll(2)` syscall actually costs — this bounds "did
/// the guard work at all," not steady-state latency — but nowhere close
/// to "long enough to hide an infinite loop": before the fix, this call
/// never returns at all, so any finite bound here would eventually catch
/// it (CI's own job timeout is the real backstop for that case).
const CALL_BUDGET: Duration = Duration::from_secs(5);

#[test]
#[ignore] // see module doc: mutates this process's fd 0.
fn wait_events_returns_once_stdin_hangs_up_on_a_dead_pty() {
    let slave_fd = open_pty_with_master_already_closed();
    let _stdin_override = StdinOverride::install(slave_fd);

    let mut backend = TuiBackend::new();

    let start = Instant::now();
    let events = backend.wait_events(Duration::from_millis(50));
    let elapsed = start.elapsed();

    assert!(
        elapsed < CALL_BUDGET,
        "wait_events took {elapsed:?} — the crossterm busy-loop guard \
         (quadraui#1295) appears to have regressed"
    );
    assert!(
        events.is_empty(),
        "a dead, never-written-to pty has no real event to report"
    );
    assert!(
        backend.input_gone(),
        "wait_events observed a hung-up stdin but didn't set input_gone()"
    );

    // SAFETY: `slave_fd` is a plain fd this test opened above.
    unsafe {
        libc::close(slave_fd);
    }
}

/// Same scenario as above, but through [`TuiBackend::poll_events`] (the
/// never-blocks entry point `TuiRunner::step` uses) — see that method's
/// own guard, which mirrors `wait_events`'s exactly.
#[test]
#[ignore] // see module doc: mutates this process's fd 0.
fn poll_events_returns_once_stdin_hangs_up_on_a_dead_pty() {
    let slave_fd = open_pty_with_master_already_closed();
    let _stdin_override = StdinOverride::install(slave_fd);

    let mut backend = TuiBackend::new();

    let start = Instant::now();
    let events = backend.poll_events();
    let elapsed = start.elapsed();

    assert!(
        elapsed < CALL_BUDGET,
        "poll_events took {elapsed:?} — the crossterm busy-loop guard \
         (quadraui#1295) appears to have regressed"
    );
    assert!(events.is_empty());
    assert!(
        backend.input_gone(),
        "poll_events observed a hung-up stdin but didn't set input_gone()"
    );

    // SAFETY: `slave_fd` is a plain fd this test opened above.
    unsafe {
        libc::close(slave_fd);
    }
}

/// Repeated calls (the shape the live `TuiRunner`/`run_with` loop
/// actually takes — one `wait_events`/`poll_events` call per iteration,
/// forever) must stay bounded too: this is the "bounded CPU" half of the
/// issue's acceptance criteria, exercised as "N bounded-time calls in a
/// row" rather than an actual CPU-percentage sample, since a unit test
/// has no reliable cross-platform way to sample its own process's CPU
/// usage over a short window. Before the fix this test would never even
/// reach its first assertion (the very first `wait_events` call hangs
/// forever); after it, 200 iterations complete in well under a second.
#[test]
#[ignore] // see module doc: mutates this process's fd 0.
fn repeated_wait_events_calls_stay_bounded_after_stdin_hangs_up() {
    let slave_fd = open_pty_with_master_already_closed();
    let _stdin_override = StdinOverride::install(slave_fd);

    let mut backend = TuiBackend::new();

    let start = Instant::now();
    for _ in 0..200 {
        let events = backend.wait_events(Duration::from_millis(1));
        assert!(events.is_empty());
    }
    let elapsed = start.elapsed();

    assert!(
        elapsed < CALL_BUDGET,
        "200 wait_events calls took {elapsed:?} — expected each to \
         return near-instantly once input_gone() is latched"
    );
    assert!(backend.input_gone());

    // SAFETY: `slave_fd` is a plain fd this test opened above.
    unsafe {
        libc::close(slave_fd);
    }
}
