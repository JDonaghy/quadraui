//! Fuzz [`quadraui::text_util::strip_json_comments`] (quadraui#1130).
//!
//! Untrusted input: a VS Code theme file is exactly the kind of thing a
//! user downloads from a marketplace and hands to `Theme::from_vscode_json`
//! (which calls this first) without vetting it. `cargo test`'s `proptest`
//! coverage in `src/text_util.rs::proptests` already exercises the same
//! contract with hundreds of cases per run; this target is the same
//! property run continuously/scheduled (see `.github/workflows/fuzz.yml`)
//! with a much larger, coverage-guided input space and no per-run bound.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // Lossy, not `str::from_utf8`-gated: a real file on disk can contain
    // invalid UTF-8, and `std::fs::read_to_string` (what
    // `Theme::from_vscode_json` actually calls) would simply fail to read
    // it rather than handing `strip_json_comments` anything — so drive the
    // *parsing* logic itself with the widest input libFuzzer can generate,
    // matching this crate's own `proptests::strip_json_comments_never_panics_on_lossy_bytes`.
    let s = String::from_utf8_lossy(data);
    let _ = quadraui::text_util::strip_json_comments(&s);
});
