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
//! checking for `POLLHUP`/`POLLERR`/`POLLNVAL`) reports the fd as hung
//! up with nothing left to read — see that function's doc in
//! `src/tui/backend.rs` for the full rationale, including why this is a
//! guard (the vulnerable crossterm call is never reached) rather than a
//! mitigation of its behaviour once entered, and the real, narrower
//! window that remains when a hangup lands *while* a call is already in
//! flight (`STDIN_HANGUP_POLL_SLICE`'s doc). The first two tests below
//! exercise `TuiBackend` directly; the third exercises
//! [`quadraui::tui::TuiRunner`] — the layer above it that turns
//! `input_gone()` into a clean [`quadraui::tui::StepOutcome::Exited`].
//!
//! ## Why these tests mutate real process state, and why they're `#[ignore]`d
//!
//! Unlike the rest of this crate's test suite, these tests must dup a
//! real pty slave onto file descriptor 0 (and, for the `TuiRunner` test,
//! also file descriptor 1) of **the test binary's own process** —
//! `TuiBackend::wait_events`/`poll_events` read real stdin, there is no
//! injection seam for a fake fd (and adding one here would test the
//! seam, not the real crossterm/kernel interaction this issue is
//! about). Those fds are process-global state, shared by every test in
//! this binary if run with the default parallel test harness — so each
//! test here saves and restores its original fds around its own body,
//! but still should not be trusted to run concurrently with any other
//! test that touches stdin/stdout. Run explicitly and alone:
//!
//! ```sh
//! cargo test --features tui --test crossterm_dead_pty_busy_loop -- --ignored --test-threads=1
//! ```
#![cfg(all(feature = "tui", unix))]

use std::ffi::CStr;
use std::os::fd::RawFd;
use std::time::{Duration, Instant};

use quadraui::tui::{StepOutcome, TuiBackend, TuiRunner};
use quadraui::{AppLogic, Backend, Reaction, UiEvent};

/// Opens a fresh pty pair via the raw POSIX `posix_openpt`/`grantpt`/
/// `unlockpt`/`ptsname` dance (mirroring the issue's own minimal C
/// probe). Both ends are left open and owned by the caller — most
/// callers in this file want the master closed immediately (see
/// [`open_pty_with_master_already_closed`]), but
/// [`tui_runner_exits_once_its_pty_master_closes_under_it`] needs the
/// master kept alive long enough for `TuiRunner::new`'s real terminal
/// negotiation to succeed first.
fn open_pty_pair() -> (RawFd, RawFd) {
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

        (master_fd, slave_fd)
    }
}

/// [`open_pty_pair`], then **closes the master immediately** — the slave
/// fd returned is the only thing left alive, exactly the "pty master
/// closed" condition quadraui#1295 is about. Zero bytes are ever
/// written to either side, so there is nothing queued for the slave to
/// drain before it reports pure EOF.
fn open_pty_with_master_already_closed() -> RawFd {
    let (master_fd, slave_fd) = open_pty_pair();
    // SAFETY: `master_fd` is a plain, this-process-owned fd from
    // `open_pty_pair` above, never dup'd — closing it here is exactly
    // the "master side is now fully closed" condition this function
    // promises its caller. No other fd in this process (or any other)
    // points at it, so the slave's read end sees a permanent,
    // unambiguous hangup with nothing buffered.
    assert_eq!(unsafe { libc::close(master_fd) }, 0, "close(master) failed");
    slave_fd
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

/// Same as [`StdinOverride`], for `STDOUT_FILENO` — needed by
/// [`tui_runner_exits_once_its_pty_master_closes_under_it`] because
/// `TuiRunner::new` writes real terminal-setup escape sequences
/// (alternate screen, raw mode, kitty-keyboard push) to stdout, not just
/// stdin; without redirecting it too, that setup would land on this
/// test binary's *real* stdout instead of the pty this test constructs.
struct StdoutOverride {
    saved: RawFd,
}

impl StdoutOverride {
    fn install(fd: RawFd) -> Self {
        // SAFETY: `dup`/`dup2` on process-owned fds; `STDOUT_FILENO` (1)
        // is always a valid fd number to target.
        let saved = unsafe { libc::dup(libc::STDOUT_FILENO) };
        assert!(
            saved >= 0,
            "dup(stdout) failed: {:?}",
            std::io::Error::last_os_error()
        );
        let result = unsafe { libc::dup2(fd, libc::STDOUT_FILENO) };
        assert!(
            result >= 0,
            "dup2(fd, stdout) failed: {:?}",
            std::io::Error::last_os_error()
        );
        Self { saved }
    }
}

impl Drop for StdoutOverride {
    fn drop(&mut self) {
        // SAFETY: restoring the exact fd this process started with.
        unsafe {
            libc::dup2(self.saved, libc::STDOUT_FILENO);
            libc::close(self.saved);
        }
    }
}

/// The smallest possible [`AppLogic`] — this test only needs *something*
/// for `TuiRunner::new` to call `setup`/`render` on; it never exercises
/// any app-specific behaviour, so there's nothing to gain from reusing
/// one of `examples/common`'s real demo apps here.
struct MinimalApp;

impl AppLogic for MinimalApp {
    type AreaId = ();

    fn render(&self, _backend: &mut dyn Backend, _area: ()) {}

    fn handle(&mut self, _event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        Reaction::Continue
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
#[ignore = "mutates this process's fd 0 — see module doc"]
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
#[ignore = "mutates this process's fd 0 — see module doc"]
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
#[ignore = "mutates this process's fd 0 — see module doc"]
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

/// Black-box proof that [`TuiRunner`] itself — not just [`TuiBackend`] in
/// isolation, see the two tests above for that half — turns a hung-up
/// input fd into a clean [`StepOutcome::Exited`] rather than hanging
/// `pump` forever.
///
/// Deliberately keeps the master alive through [`TuiRunner::new`]'s real
/// terminal negotiation (raw mode, alternate screen, kitty-keyboard
/// push — all real escape-sequence round trips against the pty this test
/// constructs, both fds of it redirected into this very process) and
/// only closes it *after* that call returns successfully, with nothing
/// else running concurrently in this single-threaded test body to race
/// it. That ordering is what makes this deterministic rather than a
/// coin flip: `STDIN_HANGUP_POLL_SLICE`'s doc (`src/tui/backend.rs`)
/// explains why a hangup landing *while* a `wait_events`/`poll_events`
/// call is already in flight can still race this guard — closing the
/// master only after `new` has fully returned, before this function's
/// own first `pump` call even begins, puts the hangup unambiguously
/// *before* that first call rather than during one.
#[test]
#[ignore = "mutates this process's fd 0 and fd 1 — see module doc"]
fn tui_runner_exits_once_its_pty_master_closes_under_it() {
    let (master_fd, slave_fd) = open_pty_pair();
    let _stdin_override = StdinOverride::install(slave_fd);
    let _stdout_override = StdoutOverride::install(slave_fd);

    // A sane size so `TuiRunner::new`'s terminal-size query doesn't seed
    // a degenerate `0x0` viewport.
    let ws = libc::winsize {
        ws_row: 24,
        ws_col: 80,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // SAFETY: `ws` is a valid, fully-owned local `libc::winsize`;
    // `ioctl(TIOCSWINSZ)` only reads through the pointer it's given.
    assert_eq!(
        unsafe { libc::ioctl(slave_fd, libc::TIOCSWINSZ, &ws) },
        0,
        "TIOCSWINSZ failed: {:?}",
        std::io::Error::last_os_error()
    );

    // `TuiRunner::new` queries the cursor position (`ESC[6n`) as part of
    // its real terminal negotiation and blocks for a reply — this
    // background thread plays the minimal terminal-emulator role of
    // answering it, reading/writing `master_fd`'s raw number directly
    // (never a `dup`, so the "only open reference" invariant below still
    // holds once it's stopped and joined). Stopped as soon as `new`
    // returns, strictly before `master_fd` is closed.
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let responder = {
        let stop = std::sync::Arc::clone(&stop);
        std::thread::spawn(move || {
            let mut received = Vec::new();
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                let mut pollfd = libc::pollfd {
                    fd: master_fd,
                    events: libc::POLLIN,
                    revents: 0,
                };
                // SAFETY: `pollfd` is a single valid `libc::pollfd` on
                // the stack; `poll(2)` only reads/writes through the
                // pointer+length (1) it's given, for this call's
                // duration. A short timeout so `stop` is re-checked
                // promptly rather than blocking past it.
                let ready = unsafe { libc::poll(&mut pollfd, 1, 20) };
                if ready <= 0 {
                    continue;
                }
                let mut chunk = [0u8; 256];
                // SAFETY: `chunk` is a valid, fully-owned local buffer;
                // `read(2)` writes at most `chunk.len()` bytes into it.
                let n = unsafe { libc::read(master_fd, chunk.as_mut_ptr().cast(), chunk.len()) };
                if n > 0 {
                    received.extend_from_slice(&chunk[..n as usize]);
                    if received.windows(4).any(|w| w == b"\x1b[6n") {
                        let reply = b"\x1b[1;1R";
                        // SAFETY: `reply` is a valid slice for its own
                        // length; `write(2)` only reads from it, for at
                        // most that many bytes.
                        unsafe {
                            libc::write(master_fd, reply.as_ptr().cast(), reply.len());
                        }
                        received.clear();
                    }
                }
            }
        })
    };

    // Real terminal negotiation against a live pty — must succeed before
    // this test ever touches the master.
    let new_result = TuiRunner::new(MinimalApp);

    // Stop and join the responder *before* closing `master_fd` below —
    // it only borrows that fd's raw number rather than owning a `dup`,
    // so it must not still be polling or reading it once this test
    // closes it out from under it.
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    responder.join().expect("responder thread panicked");

    let mut runner = new_result.expect("TuiRunner::new against a live pty");

    // One `pump` call against the still-live pty, purely to consume the
    // pending first-frame render `TuiRunner::new` leaves armed
    // (`needs_redraw`, seeded `true`). Without this, the *next* `pump`
    // call (the one this test cares about, after the master closes)
    // would hit that pending render first and fail with its own `EIO`
    // from writing to a now-dead pty — a real, but entirely different
    // and uninteresting write-side failure this guard was never meant to
    // cover. This call's own event wait still completes against a live
    // master, so it can't itself race anything.
    assert_eq!(
        runner
            .pump(Duration::from_millis(1))
            .expect("priming pump call against a live pty"),
        StepOutcome::Continue,
        "priming pump call unexpectedly reported Exited before the master ever closed"
    );

    // The only open reference to the master side anywhere in this
    // process — see this test's doc for why closing it *here*, after
    // `new` has already returned and the priming `pump` call above has
    // already consumed the pending first-frame render, and before any
    // further `pump` call, is exactly the point.
    // SAFETY: `master_fd` is a plain, this-process-owned fd from
    // `open_pty_pair` above, never dup'd.
    unsafe {
        libc::close(master_fd);
    }

    let start = Instant::now();
    let mut outcome = StepOutcome::Continue;
    while start.elapsed() < CALL_BUDGET && outcome != StepOutcome::Exited {
        outcome = runner
            .pump(Duration::from_millis(50))
            .expect("pump should not itself error on a hung-up pty");
    }
    let elapsed = start.elapsed();

    assert_eq!(
        outcome,
        StepOutcome::Exited,
        "pump did not report StepOutcome::Exited within {elapsed:?} of its pty master \
         closing — the crossterm dead-pty busy-loop guard (quadraui#1295) appears to have \
         regressed at the TuiRunner layer"
    );
    assert!(
        elapsed < CALL_BUDGET,
        "pump took {elapsed:?} to notice its pty master had closed"
    );
    assert!(
        runner.backend().input_gone(),
        "pump reported Exited but TuiBackend::input_gone() is false — \
         StepOutcome::Exited should only come from this guard here, nothing else \
         asked MinimalApp to quit"
    );

    // SAFETY: `slave_fd` is a plain fd this test opened above. `runner`
    // (and its `Drop` teardown) is still alive at this point and holds
    // no fd of its own — it only ever reached the redirected
    // `STDIN_FILENO`/`STDOUT_FILENO` through the two `Override` guards,
    // which restore the test process's real fds on drop, after this.
    unsafe {
        libc::close(slave_fd);
    }
}
