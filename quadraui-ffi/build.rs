//! Generates `include/quadraui_ffi.h` from this crate's `extern "C"`
//! surface — the PoC evidence for how much (or how little) cbindgen needs
//! hand-holding on quadraui's actual exported shapes (opaque structs,
//! `#[repr(C)]` vtables with function-pointer fields, `Option<fn(...)>`).

use std::env;
use std::path::PathBuf;

fn main() {
    let crate_dir = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set by cargo");
    let out_path = PathBuf::from(&crate_dir)
        .join("include")
        .join("quadraui_ffi.h");

    let config = cbindgen::Config {
        language: cbindgen::Language::C,
        ..cbindgen::Config::default()
    };

    match cbindgen::Builder::new()
        .with_crate(&crate_dir)
        .with_config(config)
        .generate()
    {
        Ok(bindings) => {
            bindings.write_to_file(&out_path);
        }
        Err(err) => {
            // A spike build.rs failing loudly (rather than silently skipping
            // header generation) is the point: it is itself evidence for the
            // write-up of how brittle/noisy cbindgen's failure mode is on
            // this crate's surface.
            panic!("cbindgen failed to generate {}: {err}", out_path.display());
        }
    }

    println!("cargo:rerun-if-changed=src/lib.rs");
}
