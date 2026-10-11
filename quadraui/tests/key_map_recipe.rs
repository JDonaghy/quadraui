//! Includes `examples/common/key_map.rs`'s `#[cfg(test)]` suite.
//!
//! `KeyMap`/`KeyContext` are example-only code in `examples/common/key_map.rs`.
//! Cargo's `[[example]]` targets default to `test = false`, so tests in that
//! file are compiled but never executed by `cargo test --workspace`. This
//! binary uses `#[path]` to include the example file under `tests/` (where
//! `[[test]]` targets default to `test = true`), making the 12 tests that
//! validate `KeyMap::resolve`'s scope logic run during `cargo test --workspace`.
#[path = "../examples/common/key_map.rs"]
mod key_map;
