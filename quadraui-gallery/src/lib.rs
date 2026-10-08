//! `quadraui-gallery` — an interactive catalogue of quadraui primitives.
//!
//! One [`app::GalleryApp`] (a [`quadraui::ShellApp`]) drives an activity
//! bar of primitive groups, a sidebar tree of the demos in the active
//! group, a main pane with Demo / Code / Data tabs, and a bottom-panel
//! event log. Every demo implements [`Demo`] and is listed once in
//! [`registry::registry`] — the shell has no per-demo knowledge beyond
//! that trait, so porting a demo never touches shell code.
//!
//! No backend-specific code lives in this crate outside `src/main.rs`'s
//! ~5-line-per-backend runner bodies (CLAUDE.md's cross-backend
//! portability commitment) — `GalleryApp` and every `Demo` are plain
//! [`quadraui::ShellApp`] / rendering code that runs unmodified under
//! `tui`, `gtk`, `macos` and `win`.

pub mod app;
pub mod demo;
pub mod demos;
pub mod registry;

pub use app::GalleryApp;
pub use demo::Demo;
