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

/// Direct2D / DirectWrite rasteriser for [`crate::ActivityBar`] (#25).
/// Windows-only in full — see its module docs.
#[cfg(target_os = "windows")]
mod activity_bar;
pub mod backend;
/// Win32 message → `quadraui::UiEvent` translation (mouse, keyboard,
/// focus). Pure free functions, host-independent and unit-tested off
/// Windows — mirrors `crate::gtk::events` / `crate::macos::events`. See
/// its module docs. `WM_SIZE`/`WM_DPICHANGED`/`WM_CLOSE` translation
/// landed in #19 via `msg` + `run` instead — see this module's docs.
pub mod events;
/// Direct2D / DirectWrite rasteriser for [`crate::MenuBar`] (#25).
/// Windows-only in full — see its module docs.
#[cfg(target_os = "windows")]
mod menu_bar;
/// Win32 message-payload decoding (`WPARAM`/`LPARAM` word unpacking, DPI
/// ratio). Crate-private and host-independent — it is the one part of this
/// backend that is pure arithmetic, so it is the one part that can be
/// unit-tested off Windows. See its module docs.
pub(crate) mod msg;
pub mod run;
pub mod services;
/// Direct2D / DirectWrite rasteriser for [`crate::StatusBar`] (#25).
/// Windows-only in full — see its module docs.
#[cfg(target_os = "windows")]
mod status_bar;
/// Direct2D / DirectWrite rasteriser for [`crate::TabBar`] (#25).
/// Windows-only in full — see its module docs.
#[cfg(target_os = "windows")]
mod tab_bar;
/// Headless Direct2D test surface (#24): an offscreen `ID2D1DCRenderTarget`
/// backed by an in-memory DIB section, so `#[cfg(test)]` blocks can paint
/// a primitive and read pixels back with no `HWND`, display, or GPU.
/// `pub` (like [`crate::tui::testing`] / [`crate::gtk::testing`]) rather
/// than `pub(crate)` — see its module docs for why. Windows-only in full,
/// same reasoning as `text` below.
#[cfg(target_os = "windows")]
pub mod testing;
/// DirectWrite text infrastructure — factory + text-format creation,
/// font-metrics measurement, and Direct2D text painting (#21). Windows-only
/// in full: nothing in here has a meaningful non-Windows fallback (unlike
/// `msg`, this isn't pure arithmetic), so the module itself only exists on
/// `target_os = "windows"` rather than being internally `cfg`-gated
/// line-by-line like `backend.rs`.
#[cfg(target_os = "windows")]
pub(crate) mod text;

#[cfg(target_os = "windows")]
pub use activity_bar::{draw_activity_bar, win_activity_bar_layout, ACTIVITY_ROW_DIP};
pub use backend::WinBackend;
#[cfg(target_os = "windows")]
pub use menu_bar::{draw_menu_bar, win_menu_bar_layout};
pub use run::run;
pub use services::WinPlatformServices;
#[cfg(target_os = "windows")]
pub use status_bar::{draw_status_bar, win_status_bar_layout, MIN_GAP_DIP};
#[cfg(target_os = "windows")]
pub use tab_bar::{draw_tab_bar, draw_tab_bar_icons, win_tab_bar_layout, win_tab_bar_layout_icons};
