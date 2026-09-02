//! Win-GUI backend: Direct2D + DirectWrite rasterisers.
//!
//! #19 landed the Win32 window + Direct2D render-target bootstrap
//! (`run.rs`'s message loop, `backend.rs`'s `begin_frame`/`end_frame`)
//! — real WinAPI calls gated on `cfg(target_os = "windows")` so `cargo
//! check --features win` still type-checks `WinBackend`'s trait
//! completeness on Linux (see `backend.rs`'s module docs and `Cargo.toml`'s
//! `win` feature comment). #25 landed the four chrome-strip rasterisers
//! (`status_bar`/`tab_bar`/`activity_bar`/`menu_bar`) and #26 the six
//! content-area rasterisers (`tree`/`list`/`form`/`data_table`/`editor`/
//! `chart`) — every other `draw_*`/`*_layout` rasteriser is still a
//! `todo!()` stub; implement each one against Direct2D / DirectWrite
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
/// Direct2D / DirectWrite rasteriser for [`crate::primitives::chart::Chart`]
/// (#26). Windows-only in full — see its module docs.
#[cfg(target_os = "windows")]
mod chart;
/// Direct2D / DirectWrite rasteriser for [`crate::DataTable`] (#26).
/// Windows-only in full — see its module docs.
#[cfg(target_os = "windows")]
mod data_table;
/// Direct2D / DirectWrite rasteriser for [`crate::primitives::editor::Editor`]
/// (#26). Windows-only in full — see its module docs.
#[cfg(target_os = "windows")]
mod editor;
/// Win32 message → `quadraui::UiEvent` translation (mouse, keyboard,
/// focus). Pure free functions, host-independent and unit-tested off
/// Windows — mirrors `crate::gtk::events` / `crate::macos::events`. See
/// its module docs. `WM_SIZE`/`WM_DPICHANGED`/`WM_CLOSE` translation
/// landed in #19 via `msg` + `run` instead — see this module's docs.
pub mod events;
/// Direct2D / DirectWrite rasteriser for [`crate::Form`] (#26).
/// Windows-only in full — see its module docs.
#[cfg(target_os = "windows")]
mod form;
/// Direct2D / DirectWrite rasteriser for [`crate::ListView`] (#26).
/// Windows-only in full — see its module docs.
#[cfg(target_os = "windows")]
mod list;
/// Direct2D / DirectWrite rasteriser for [`crate::MenuBar`] (#25).
/// Windows-only in full — see its module docs.
#[cfg(target_os = "windows")]
mod menu_bar;
/// Win32 message-payload decoding (`WPARAM`/`LPARAM` word unpacking, DPI
/// ratio). Crate-private and host-independent — it is the one part of this
/// backend that is pure arithmetic, so it is the one part that can be
/// unit-tested off Windows. See its module docs.
pub(crate) mod msg;
/// Direct2D / DirectWrite rasteriser for [`crate::MultiSectionView`] (#27).
/// Windows-only in full — see its module docs.
#[cfg(target_os = "windows")]
mod multi_section_view;
pub mod run;
/// Direct2D rasteriser for [`crate::Scrollbar`] (#27). Windows-only in
/// full — see its module docs.
#[cfg(target_os = "windows")]
mod scrollbar;
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
///
/// `pub` rather than `pub(crate)` (#25): the chrome rasterisers re-exported
/// below (`draw_status_bar`, `draw_tab_bar`, …) take a `&DWrite` measurer in
/// their signatures, exactly as [`crate::macos`]'s take a `&CTFont` from its
/// own `pub mod text`. A `pub` function whose parameter type is `pub(crate)`
/// is a `private_interfaces` warning — and this repo's CI runs with
/// `RUSTFLAGS: "-D warnings"`, so it is a *build failure* on the
/// windows-latest leg (nothing in this module compiles on Linux, so the
/// ubuntu `cargo check --features win` leg cannot see it). Keep this module
/// and `DWrite`'s public surface in step with the rasterisers that take it.
#[cfg(target_os = "windows")]
pub mod text;
/// Direct2D / DirectWrite rasteriser for [`crate::TreeView`] (#26).
/// Windows-only in full — see its module docs.
#[cfg(target_os = "windows")]
mod tree;

#[cfg(target_os = "windows")]
pub use activity_bar::{draw_activity_bar, win_activity_bar_layout, ACTIVITY_ROW_DIP};
pub use backend::WinBackend;
#[cfg(target_os = "windows")]
pub use chart::{draw_chart, win_chart_layout};
#[cfg(target_os = "windows")]
pub use data_table::{draw_data_table, win_data_table_layout};
#[cfg(target_os = "windows")]
pub use editor::draw_editor;
#[cfg(target_os = "windows")]
pub use form::{draw_form, win_form_layout};
#[cfg(target_os = "windows")]
pub use list::{draw_list, win_list_layout};
#[cfg(target_os = "windows")]
pub use menu_bar::{draw_menu_bar, win_menu_bar_layout};
#[cfg(target_os = "windows")]
pub use multi_section_view::{draw_multi_section_view, win_msv_layout, win_msv_metrics};
pub use run::run;
#[cfg(target_os = "windows")]
pub use scrollbar::draw_scrollbar;
pub use services::WinPlatformServices;
#[cfg(target_os = "windows")]
pub use status_bar::{draw_status_bar, win_status_bar_layout, MIN_GAP_DIP};
#[cfg(target_os = "windows")]
pub use tab_bar::{draw_tab_bar, draw_tab_bar_icons, win_tab_bar_layout, win_tab_bar_layout_icons};
#[cfg(target_os = "windows")]
pub use tree::{draw_tree, win_tree_layout};
