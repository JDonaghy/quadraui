//! Guards against `include/quadraui_ffi.h` drifting from what cbindgen
//! actually generates for this crate's `extern "C"` surface today.
//!
//! `build.rs` deliberately never writes into the source tree (it only
//! produces a header under `OUT_DIR`, to build the smoke harness and to
//! satisfy cargo's build-script contract), so the checked-in header is a
//! separate, hand-regenerated artifact. This test is what keeps the two in
//! sync: run plain, it fails loudly with instructions if they differ; run
//! with `QUADRAUI_FFI_UPDATE_HEADER=1` (see `./generate-header.sh`), it
//! overwrites the checked-in header instead of asserting.

use std::env;
use std::fs;
use std::path::PathBuf;

#[test]
fn header_up_to_date() {
    let crate_dir = env!("CARGO_MANIFEST_DIR");
    let header_path = PathBuf::from(crate_dir)
        .join("include")
        .join("quadraui_ffi.h");

    // Keep this `Config` in sync with `build.rs`'s copy: a build script
    // isn't a library, so the two can't share one function.
    let config = cbindgen::Config {
        language: cbindgen::Language::C,
        pragma_once: true,
        ..cbindgen::Config::default()
    };

    let bindings = cbindgen::Builder::new()
        .with_crate(crate_dir)
        .with_config(config)
        .generate()
        .expect("cbindgen failed to generate the C header");
    let mut buffer = Vec::new();
    bindings.write(&mut buffer);
    let generated = String::from_utf8(buffer).expect("generated header is not valid UTF-8");

    if env::var_os("QUADRAUI_FFI_UPDATE_HEADER").is_some() {
        fs::write(&header_path, &generated).expect("failed to write include/quadraui_ffi.h");
        return;
    }

    let committed =
        fs::read_to_string(&header_path).expect("failed to read include/quadraui_ffi.h");
    assert_eq!(
        committed, generated,
        "include/quadraui_ffi.h is stale — run `./generate-header.sh` and commit the result"
    );
}
