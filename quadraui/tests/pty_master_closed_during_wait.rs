//! Deterministic regression test for quadraui#1301 — the in-flight race
//! `tests/crossterm_dead_pty_busy_loop.rs` (quadraui#1295) deliberately
//! does **not** exercise: a pty master closing *while* a
//! `TuiBackend::wait_events`/`TuiRunner::pump` call is already blocked
//! waiting for input, rather than strictly before that call begins.
//!
//! ## Why #1295 alone doesn't close this
//!
//! Before #1301, `TuiBackend::wait_events` delegated its idle wait to
//! `ratatui::crossterm::event::poll` in fixed-size (20ms) chunks,
//! re-checking its own `stdin_hung_up` guard *between* chunks.
//! crossterm 0.29's unix event source
//! (`UnixInternalEventSource::try_read`) has a TTY-readiness read loop
//! with no break arm for `Ok(0)` (EOF): once mio reports a hung-up fd
//! "ready" (which it always does — a hangup condition is always ready
//! to `poll(2)`/`epoll`), that loop calls `read()` again immediately,
//! forever, with no way for the caller to interrupt it. So a hangup
//! landing while one of those 20ms delegated calls is already blocked
//! inside crossterm is invisible to the guard — it only gets to look
//! again *after* the call returns, which, if the hangup is what woke
//! it, it structurally cannot. This test closes the master from a
//! background thread partway through a multi-second `wait_events`/
//! `pump` call specifically to land inside that window: with a call
//! blocked for seconds in ~20ms slices, an asynchronous close almost
//! certainly lands while one of those slices is in flight, not in the
//! brief gap between them.
//!
//! ## The fix under test
//!
//! `TuiBackend::wait_events`'s `wait_for_stdin_ready` (quadraui#1301)
//! never delegates a blocking call to crossterm at all. It blocks on
//! this crate's own `poll(2)` syscall against the exact same fd, for
//! the caller's real timeout, and only ever delegates to crossterm with
//! `Duration::ZERO` (guaranteed non-blocking) once that call has
//! already confirmed readiness with no hangup bit set. A hangup that
//! lands while that `poll(2)` call is parked in the kernel is exactly
//! the event that wakes it — `poll(2)` reports `POLLHUP` on a hung-up
//! fd the instant it happens, not on some later sampling interval — so
//! there is no slice-sized window left for this test to land in.
//!
//! Before this fix, both tests below hang forever (RED-verified against
//! quadraui `8425673`, the tip #1295 landed at) rather than failing an
//! assertion — see `tests/crossterm_dead_pty_busy_loop.rs`'s module doc
//! for why that's expected and why CI's own job timeout is this test
//! suite's real backstop for that failure mode. After this fix, both
//! return within [`CALL_BUDGET`] regardless of when the background
//! thread happens to close the master.
//!
//! ## Why these tests mutate real process state, and why they're `#[ignore]`d
//!
//! Same caveat as `tests/crossterm_dead_pty_busy_loop.rs`: these tests
//! dup a real pty slave onto this test binary's own `STDIN_FILENO` (and,
//! for the `TuiRunner` test, `STDOUT_FILENO` too) — process-global state
//! shared by every test in this binary. Run explicitly and alone:
//!
//! ```sh
//! cargo test --features tui --test pty_master_closed_during_wait -- --ignored --test-threads=1
//! ```
#![cfg(all(feature = "tui", unix))]

#[path = "pty_support/mod.rs"]
mod pty_support;

use std::time::{Duration, Instant};

use quadraui::tui::{StepOutcome, TuiBackend, TuiRunner};
use quadraui::Backend;

use pty_support::{open_pty_pair, MinimalApp, StdinOverride, StdoutOverride};

/// How long the blocked call under test is allowed to take once its
/// background thread closes the master mid-wait. Generous relative to
/// the few hundred milliseconds [`CLOSE_AFTER`] + a `poll(2)` wakeup
/// actually costs, but deliberately kept *below* [`WAIT_TIMEOUT`]: a
/// call that merely waited out its own full timeout (rather than being
/// woken by the hangup) would still return within a budget larger than
/// `WAIT_TIMEOUT`, silently passing for the wrong reason. Below
/// `WAIT_TIMEOUT`, the elapsed-time assertion is itself load-bearing —
/// as in `tests/crossterm_dead_pty_busy_loop.rs`, nowhere close to long
/// enough to hide an infinite loop: before the fix, these calls never
/// return at all once the race lands, so any finite bound here
/// eventually catches it (CI's own job timeout is the real backstop for
/// that case).
const CALL_BUDGET: Duration = Duration::from_millis(1500);

/// How long the background thread waits before closing the master — well
/// inside the much longer `wait_events`/`pump` timeout each test below
/// requests, so the close is guaranteed to land while that call is
/// already blocked rather than before it starts or after it would have
/// timed out on its own.
const CLOSE_AFTER: Duration = Duration::from_millis(300);

/// How long `wait_events`/`pump` is asked to block for, per call —
/// comfortably longer than [`CALL_BUDGET`] so an un-fixed regression
/// (which wouldn't return until this full timeout, if ever) can't be
/// mistaken for the in-flight wakeup this test is asserting.
const WAIT_TIMEOUT: Duration = Duration::from_secs(3);

/// Core regression test: [`TuiBackend::wait_events`] must return
/// promptly even when the pty master closes *while* this exact call is
/// already blocked inside it, not just when it closes beforehand (that
/// easier case is `tests/crossterm_dead_pty_busy_loop.rs`'s
/// `wait_events_returns_once_stdin_hangs_up_on_a_dead_pty`).
#[test]
#[ignore = "mutates this process's fd 0 — see module doc"]
fn wait_events_returns_once_master_closes_mid_wait() {
    let (master_fd, slave_fd) = open_pty_pair();
    let _stdin_override = StdinOverride::install(slave_fd);

    // Constructed before the closer thread is spawned below, so the
    // close can only ever land during the `wait_events` call itself,
    // never during construction.
    let mut backend = TuiBackend::new();

    let closer = std::thread::spawn(move || {
        std::thread::sleep(CLOSE_AFTER);
        // SAFETY: `master_fd` is a plain, this-process-owned fd from
        // `open_pty_pair` above, never dup'd — closing it here is
        // exactly the "master side is now fully closed" condition this
        // test is about. No other fd in this process (or any other)
        // points at it.
        unsafe {
            libc::close(master_fd);
        }
    });

    let start = Instant::now();
    let events = backend.wait_events(WAIT_TIMEOUT);
    let elapsed = start.elapsed();

    closer.join().expect("closer thread panicked");

    assert!(
        elapsed < CALL_BUDGET,
        "wait_events took {elapsed:?} to notice its pty master closed mid-wait — \
         the quadraui#1301 in-flight-hangup guard appears to have regressed"
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

/// Same race, one layer up: [`TuiRunner::pump`] must turn a mid-wait
/// hangup into a clean [`StepOutcome::Exited`] within bounded time, the
/// same acceptance bar quadraui#1301 states for "the runner" (vimcode's
/// `vcd` drives this exact layer, not `TuiBackend` directly).
#[test]
#[ignore = "mutates this process's fd 0 and fd 1 — see module doc"]
fn tui_runner_exits_once_its_pty_master_closes_mid_wait() {
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
    // (never a `dup`). Stopped before `master_fd` is closed below.
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
    // it only borrows that fd's raw number rather than owning a `dup`.
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    responder.join().expect("responder thread panicked");

    let mut runner = new_result.expect("TuiRunner::new against a live pty");

    // One `pump` call against the still-live pty, purely to consume the
    // pending first-frame render `TuiRunner::new` leaves armed. Without
    // this, the *next* `pump` call (the one this test cares about) would
    // hit that pending render first and fail with its own `EIO` from
    // writing to a now-dead pty — a real, but different and uninteresting
    // write-side failure this guard was never meant to cover.
    assert_eq!(
        runner
            .pump(Duration::from_millis(1))
            .expect("priming pump call against a live pty"),
        StepOutcome::Continue,
        "priming pump call unexpectedly reported Exited before the master ever closed"
    );

    // Close the master from a background thread partway through the
    // *next* `pump` call below, landing the hangup while that call is
    // already blocked inside it. See this file's module doc.
    let closer = std::thread::spawn(move || {
        std::thread::sleep(CLOSE_AFTER);
        // SAFETY: `master_fd` is a plain, this-process-owned fd from
        // `open_pty_pair` above, never dup'd — the only open reference
        // to the master side anywhere in this process.
        unsafe {
            libc::close(master_fd);
        }
    });

    // A single `pump` call: `run_one` makes exactly one `wait_events`
    // call per `pump`, so one call is both necessary and sufficient to
    // exercise the in-flight race — tolerating a second call here would
    // weaken that claim by letting an un-fixed first call's eventual
    // `TimedOut` return (rather than the in-flight hangup) go unnoticed.
    let start = Instant::now();
    let outcome = runner
        .pump(WAIT_TIMEOUT)
        .expect("pump should not itself error on a hung-up pty");
    let elapsed = start.elapsed();

    closer.join().expect("closer thread panicked");

    assert_eq!(
        outcome,
        StepOutcome::Exited,
        "pump did not report StepOutcome::Exited within {elapsed:?} of its pty master \
         closing mid-wait — the quadraui#1301 in-flight-hangup guard appears to have \
         regressed at the TuiRunner layer"
    );
    assert!(
        elapsed < CALL_BUDGET,
        "pump took {elapsed:?} to notice its pty master had closed mid-wait"
    );
    assert!(
        runner.backend().input_gone(),
        "pump reported Exited but TuiBackend::input_gone() is false — \
         StepOutcome::Exited should only come from this guard here, nothing else \
         asked MinimalApp to quit"
    );

    // SAFETY: `slave_fd` is a plain fd this test opened above. `runner`
    // (and its `Drop` teardown) holds no fd of its own — it only ever
    // reached the redirected `STDIN_FILENO`/`STDOUT_FILENO` through the
    // two `Override` guards, which restore the test process's real fds
    // on drop, after this.
    unsafe {
        libc::close(slave_fd);
    }
}
