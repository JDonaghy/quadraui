//! Backend-agnostic app code shared by every example in this crate.
//!
//! After the runner-crate vision shipped (#269 / #270 stage B), every
//! example is just a thin `main` that constructs an `AppLogic` impl
//! and calls `quadraui::{tui,gtk}::run(app)`. The `AppLogic` bodies
//! themselves live in this module, identical across both backends —
//! that's the payoff of the runner abstraction.
//!
//! Cargo doesn't auto-treat `examples/common/mod.rs` as an example
//! binary (it would if it were named `common.rs` at the examples/
//! root). Each example references it via `#[path = "common/mod.rs"]
//! mod common;`. This is the canonical pattern for shared example
//! helpers in Rust crates.
//!
//! - [`MiniApp`] (in [`mini_app`]) — minimal one-StatusBar app, used
//!   by `tui_app` / `gtk_app`.
//! - [`AppState`] (in [`demo`]) — richer demo state (tabs + status
//!   focus + last message), used by `tui_demo` / `gtk_demo`.

// Each example uses a subset of the shared items, so dead-code +
// unused-import warnings are expected and not actionable here.
#![allow(dead_code, unused_imports)]

pub mod demo;
pub mod mini_app;

pub use demo::AppState;
pub use mini_app::MiniApp;
