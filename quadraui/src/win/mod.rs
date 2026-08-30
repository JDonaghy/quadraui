//! Win-GUI backend: Direct2D + DirectWrite rasterisers.
//!
//! #19 landed the Win32 window + Direct2D render-target bootstrap
//! (`run.rs`'s message loop, `backend.rs`'s `begin_frame`/`end_frame`)
//! — real WinAPI calls gated on `cfg(target_os = "windows")` so `cargo
//! check --features win` still type-checks `WinBackend`'s trait
//! completeness on Linux (see `backend.rs`'s module docs and `Cargo.toml`'s
//! `win` feature comment). Every `draw_*`/`*_layout` rasteriser is still
//! a `todo!()` stub — implement each one against Direct2D / DirectWrite
//! and the compiler will tell you when you're done.
//!
//! See `quadraui/docs/NATIVE_GUI_LESSONS.md` for pitfalls discovered
//! during earlier Win-GUI work. See the GTK backend (`quadraui/src/gtk/`)
//! as the reference implementation for a pixel-based backend.

pub mod backend;
/// Win32 message → `quadraui::UiEvent` translation (mouse, keyboard,
/// focus). Pure free functions, host-independent and unit-tested off
/// Windows — mirrors `crate::gtk::events` / `crate::macos::events`. See
/// its module docs. `WM_SIZE`/`WM_DPICHANGED`/`WM_CLOSE` translation
/// landed in #19 via `msg` + `run` instead — see this module's docs.
pub mod events;
/// Win32 message-payload decoding (`WPARAM`/`LPARAM` word unpacking, DPI
/// ratio). Crate-private and host-independent — it is the one part of this
/// backend that is pure arithmetic, so it is the one part that can be
/// unit-tested off Windows. See its module docs.
pub(crate) mod msg;
pub mod run;
pub mod services;

pub use backend::WinBackend;
pub use run::run;
pub use services::WinPlatformServices;
