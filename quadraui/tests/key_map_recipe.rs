//! Keeps `examples/common/key_map.rs`'s own `#[cfg(test)]` suite live under
//! `cargo test --workspace` (#825 fix-round-1).
//!
//! `KeyMap`/`KeyContext` were demoted from `quadraui::compose` to
//! `examples/common/key_map.rs` as an example-only copy-paste recipe (zero
//! adopters — see that file's module doc). Cargo's `[[example]]` targets
//! default to `test = false`, so a `#[cfg(test)] mod tests` block that is
//! only ever reached via an example's `mod common;` include is compiled and
//! linked but **never executed** by `cargo test --workspace` — the 12 tests
//! that validate `KeyMap::resolve`'s scope logic would otherwise be dead
//! code from CI's perspective.
//!
//! The fix used everywhere else in this crate for exactly this situation
//! (see `tests/tui_example_driver.rs`'s module doc) is a `#[path]` include
//! from a file under `tests/`, since `[[test]]` targets default to
//! `test = true`. This file exists solely to be that include — it has no
//! tests of its own; `examples/common/key_map.rs::tests` runs as a nested
//! module of this binary.
#[path = "../examples/common/key_map.rs"]
mod key_map;
