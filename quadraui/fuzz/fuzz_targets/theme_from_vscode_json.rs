//! Fuzz [`quadraui::Theme::from_vscode_json`] (quadraui#1130).
//!
//! Untrusted input: a VS Code theme file downloaded from a marketplace,
//! parsed with no prior validation. `Theme::from_vscode_json_str` (the
//! private, string-taking half of this that does the actual parsing) has
//! its own in-tree `proptest` coverage in `src/theme.rs::proptests`; this
//! target drives the exact same code path through the real *public* API
//! — round-tripping through a temp file, since `from_vscode_json` only
//! takes a `Path` — so a fuzz corpus entry can be replayed by any
//! consumer hitting the same bug through the API they'd actually call.
//!
//! One real file per input rather than reusing a single path: libFuzzer
//! runs many inputs in randomized order within one process in some modes,
//! and a shared path would let two inputs race on the same file.
#![no_main]

use libfuzzer_sys::fuzz_target;
use std::io::Write;

fuzz_target!(|data: &[u8]| {
    let mut f = tempfile::NamedTempFile::new().expect("create temp file");
    f.write_all(data).expect("write fuzz input to temp file");
    let _ = quadraui::Theme::from_vscode_json(f.path());
});
