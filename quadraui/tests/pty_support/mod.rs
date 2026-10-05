//! Shared helpers for this crate's real-pty integration tests
//! (`crossterm_dead_pty_busy_loop.rs`, `pty_master_closed_during_wait.rs`,
//! `wait_events_recovers_crossterm_buffered_event.rs`). Not a test binary
//! of its own — it lives under a subdirectory of `tests/` specifically so
//! Cargo's integration-test discovery (every `*.rs` file *directly* under
//! `tests/`) skips it, and each file above pulls it in via
//! `#[path = "pty_support/mod.rs"] mod pty_support;`.
//!
//! Every test that uses this module mutates file descriptor 0 (and, for
//! anything that drives [`TuiRunner::new`], file descriptor 1 too) of its
//! own process — real, process-global state. Each test here saves and
//! restores its original fds via [`StdinOverride`]/[`StdoutOverride`], but
//! that only protects against leaking the override past one test's own
//! body; it says nothing about a *second* test in the same process reusing
//! crossterm's own internal reader state, which is bound to whatever file
//! was installed at fd 0 the first time crossterm's blocking event machinery
//! engaged it, and **not** automatically rebound just because a later
//! `dup2` swaps in a different file at that same fd number. Any test that
//! needs crossterm to actually perform a real blocking read (as opposed to
//! this crate's own `poll(2)`-based hangup guard, which never touches
//! crossterm at all) must therefore run in its own test binary — i.e. its
//! own top-level `tests/*.rs` file — rather than sharing a process with any
//! other such test. Every test in every file that uses this module must
//! still be run alone:
//!
//! ```sh
//! cargo test --features tui --test <file> -- --ignored --test-threads=1
//! ```
#![cfg(all(feature = "tui", unix))]
#![allow(dead_code)] // not every test file that includes this module uses every item in it.

use std::ffi::CStr;
use std::os::fd::RawFd;

use quadraui::{AppLogic, Backend, Reaction, UiEvent};

/// Opens a fresh pty pair via the raw POSIX `posix_openpt`/`grantpt`/
/// `unlockpt`/`ptsname` dance, `O_NOCTTY` throughout — never the calling
/// process's controlling terminal, so there is no implicit `SIGHUP`
/// backstop on hangup (the shape a headless session driven over a
/// `std::process::Command`-wired pty produces). Both ends are left open
/// and owned by the caller.
pub fn open_pty_pair() -> (RawFd, RawFd) {
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
/// fd returned is the only thing left alive, a permanent hangup with
/// nothing ever queued on it.
pub fn open_pty_with_master_already_closed() -> RawFd {
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
/// restores the test process's original stdin on drop.
pub struct StdinOverride {
    saved: RawFd,
}

impl StdinOverride {
    pub fn install(fd: RawFd) -> Self {
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

/// Same as [`StdinOverride`], for `STDOUT_FILENO` — needed by anything
/// that drives `TuiRunner::new`, since it writes real terminal-setup
/// escape sequences (alternate screen, raw mode, kitty-keyboard push,
/// cursor-position query) to stdout, not just stdin.
pub struct StdoutOverride {
    saved: RawFd,
}

impl StdoutOverride {
    pub fn install(fd: RawFd) -> Self {
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

/// The smallest possible [`AppLogic`] — every test that needs
/// `TuiRunner::new` to call `setup`/`render` on something, with no
/// app-specific behaviour to exercise, uses this rather than reaching for
/// one of `examples/common`'s real demo apps.
pub struct MinimalApp;

impl AppLogic for MinimalApp {
    type AreaId = ();

    fn render(&self, _backend: &mut dyn Backend, _area: ()) {}

    fn handle(&mut self, _event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        Reaction::Continue
    }
}
