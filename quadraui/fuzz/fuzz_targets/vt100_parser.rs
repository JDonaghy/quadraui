//! Fuzz the `vt100` parser `quadraui::terminal_engine::TerminalSession`
//! wraps (quadraui#1130).
//!
//! Untrusted input: everything `vt100::Parser::process` sees is whatever
//! bytes the child shell/program running inside a PTY chooses to write —
//! a hostile or simply buggy program emitting malformed escape sequences
//! included. `src/terminal_engine.rs::proptests` covers the same "never
//! panics, across arbitrary sizes and a mid-stream resize" contract with
//! `proptest`'s bounded per-run case count; this target runs it under
//! continuous, coverage-guided fuzzing instead (see
//! `.github/workflows/fuzz.yml`).
//!
//! Drives the `vt100` crate directly (the same dependency
//! `terminal_engine.rs` uses, added explicitly below rather than
//! re-exported — `quadraui` itself exposes no `pub` "feed raw bytes to
//! the live parser" entry point off a real `TerminalSession`, which needs
//! a spawned child process per case and would be far too slow for
//! libFuzzer's per-input-process-reuse model). This is exactly how
//! `terminal_engine.rs`'s own `vt100_parser_never_panics*` property tests
//! exercise it too — see that module's doc for why bypassing the PTY
//! plumbing loses nothing: the parser itself, not the PTY around it, is
//! what actually walks the untrusted byte stream.
//!
//! Grid dimensions are floored to 2 (both axes) rather than fed straight
//! from fuzz input: vt100 0.16.2 has two known upstream panics below
//! that — see `quadraui::terminal_engine`'s `MIN_VT100_ROWS`/
//! `MIN_VT100_COLS` doc comments (private consts, so duplicated here as
//! a comment rather than an import) — already filed as the reason this
//! crate never hands vt100 anything smaller. Flooring here keeps the
//! fuzzer's corpus exploring escape-sequence handling instead of
//! rediscovering the same two already-known crashes on every run.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if data.len() < 2 {
        return;
    }
    let rows = 2 + (data[0] as u16 % 58); // 2..=59
    let cols = 2 + (data[1] as u16 % 198); // 2..=199
    let mut parser = vt100::Parser::new(rows, cols, 0);
    parser.process(&data[2..]);
});
