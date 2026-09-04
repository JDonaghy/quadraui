//! Win-GUI backend: Direct2D + DirectWrite rasterisers.
//!
//! #19 landed the Win32 window + Direct2D render-target bootstrap
//! (`run.rs`'s message loop, `backend.rs`'s `begin_frame`/`end_frame`)
//! — real WinAPI calls gated on `cfg(target_os = "windows")` so `cargo
//! check --features win` still type-checks `WinBackend`'s trait
//! completeness on Linux (see `backend.rs`'s module docs and `Cargo.toml`'s
//! `win` feature comment). #25 landed the four chrome-strip rasterisers
//! (`status_bar`/`tab_bar`/`activity_bar`/`menu_bar`), #26 the six
//! content-area rasterisers (`tree`/`list`/`form`/`data_table`/`editor`/
//! `chart`), #27 the multi-section view + standalone scrollbar, and #28
//! the seven overlay/popup rasterisers (`tooltip`/`context_menu`/
//! `dialog`/`palette`/`completions`/`find_replace`/`rich_text_popup`),
//! #29 the five container/indicator rasterisers (`panel`/`split`/
//! `toast`/`progress`/`spinner`), and #30 the three text-heavy
//! rasterisers (`terminal`/`text_display`/`message_list`) — every other
//! `draw_*`/`*_layout` rasteriser is still a `todo!()` stub; implement
//! each one against Direct2D / DirectWrite and the compiler will tell
//! you when you're done.
//!
//! See `quadraui/docs/NATIVE_GUI_LESSONS.md` for pitfalls discovered
//! during earlier Win-GUI work. See the GTK backend (`quadraui/src/gtk/`)
//! as the reference implementation for a pixel-based backend.

/// Direct2D / DirectWrite rasteriser for [`crate::ActivityBar`] (#25).
/// Windows-only in full — see its module docs.
#[cfg(target_os = "windows")]
mod activity_bar;
pub mod backend;
/// Direct2D / DirectWrite rasteriser for
/// [`crate::primitives::board::BoardModel`] (#736). Windows-only in full —
/// see its module docs.
#[cfg(target_os = "windows")]
mod board;
/// Direct2D / DirectWrite rasteriser for [`crate::primitives::chart::Chart`]
/// (#26). Windows-only in full — see its module docs.
#[cfg(target_os = "windows")]
mod chart;
/// Direct2D / DirectWrite rasteriser for [`crate::CommandCenter`] (#732).
/// Windows-only in full — see its module docs.
#[cfg(target_os = "windows")]
mod command_center;
/// Direct2D / DirectWrite rasteriser for
/// [`crate::primitives::command_line::CommandLine`] (#725). Windows-only
/// in full — see its module docs.
#[cfg(target_os = "windows")]
mod command_line;
/// Direct2D / DirectWrite rasteriser for [`crate::Completions`] (#28).
/// Windows-only in full — see its module docs.
#[cfg(target_os = "windows")]
mod completions;
/// Direct2D / DirectWrite rasteriser for [`crate::ContextMenu`] (#28).
/// Windows-only in full — see its module docs.
#[cfg(target_os = "windows")]
mod context_menu;
/// Direct2D / DirectWrite rasteriser for [`crate::DataTable`] (#26).
/// Windows-only in full — see its module docs.
#[cfg(target_os = "windows")]
mod data_table;
/// Direct2D / DirectWrite rasteriser for [`crate::Dialog`] (#28).
/// Windows-only in full — see its module docs.
#[cfg(target_os = "windows")]
mod dialog;
/// Direct2D / DirectWrite rasteriser for
/// [`crate::primitives::diff_view::DiffView`] (#737). Windows-only in
/// full — see its module docs.
#[cfg(target_os = "windows")]
mod diff_view;
/// Direct2D rasteriser for
/// [`crate::primitives::drop_zone::DropOverlay`] (#726). Windows-only
/// in full — see its module docs.
#[cfg(target_os = "windows")]
mod drop_overlay;
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
/// Direct2D / DirectWrite rasteriser for [`crate::FindReplacePanel`]
/// (#28). Windows-only in full — see its module docs.
#[cfg(target_os = "windows")]
mod find_replace;
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
/// Direct2D / DirectWrite rasteriser for [`crate::MessageList`] (#30).
/// Windows-only in full — see its module docs.
#[cfg(target_os = "windows")]
mod message_list;
/// Direct2D / DirectWrite rasteriser for
/// [`crate::primitives::minimap::Minimap`] (#738). Windows-only in full —
/// see its module docs.
#[cfg(target_os = "windows")]
mod minimap;
/// Win32 message-payload decoding (`WPARAM`/`LPARAM` word unpacking, DPI
/// ratio). Crate-private and host-independent — it is the one part of this
/// backend that is pure arithmetic, so it is the one part that can be
/// unit-tested off Windows. See its module docs.
pub(crate) mod msg;
/// Direct2D / DirectWrite rasteriser for [`crate::MultiSectionView`] (#27).
/// Windows-only in full — see its module docs.
#[cfg(target_os = "windows")]
mod multi_section_view;
/// Direct2D / DirectWrite rasteriser for [`crate::Palette`] (#28).
/// Windows-only in full — see its module docs.
#[cfg(target_os = "windows")]
mod palette;
/// Direct2D / DirectWrite rasteriser for [`crate::Panel`] (#29).
/// Windows-only in full — see its module docs.
#[cfg(target_os = "windows")]
mod panel;
/// Direct2D / DirectWrite rasteriser for
/// [`crate::primitives::pipeline_view::PipelineView`] (#735). Windows-only
/// in full — see its module docs.
#[cfg(target_os = "windows")]
mod pipeline_view;
/// Direct2D / DirectWrite rasteriser for [`crate::ProgressBar`] (#29).
/// Windows-only in full — see its module docs.
#[cfg(target_os = "windows")]
mod progress;
/// Direct2D / DirectWrite rasteriser for [`crate::RichTextPopup`] (#28).
/// Windows-only in full — see its module docs.
#[cfg(target_os = "windows")]
mod rich_text_popup;
pub mod run;
/// Direct2D rasteriser for [`crate::Scrollbar`] (#27). Windows-only in
/// full — see its module docs.
#[cfg(target_os = "windows")]
mod scrollbar;
pub mod services;
/// `run_with_shell()`: composes a [`crate::ShellApp`] into an `AppShell`
/// and drives it through the Win32 message loop (#707) — the Windows
/// analogue of `gtk::shell_runner` / `tui::shell_runner` /
/// `macos::shell_runner`. `pub` (not target-gated) for the same
/// "compiles everywhere, only *works* on Windows" reason as `run`/`mod
/// run` above — see this module's own doc.
pub mod shell_runner;
/// Direct2D / DirectWrite rasteriser for
/// [`crate::primitives::sidebar_panel::SidebarPanel`] (#731). Windows-only
/// in full — see its module docs.
#[cfg(target_os = "windows")]
mod sidebar_panel;
/// Direct2D / DirectWrite rasteriser for [`crate::Spinner`] (#29).
/// Windows-only in full — see its module docs.
#[cfg(target_os = "windows")]
mod spinner;
/// Direct2D rasteriser for [`crate::Split`] (#29). Windows-only in
/// full — see its module docs.
#[cfg(target_os = "windows")]
mod split;
/// Direct2D / DirectWrite rasteriser for [`crate::StatusBar`] (#25).
/// Windows-only in full — see its module docs.
#[cfg(target_os = "windows")]
mod status_bar;
/// Direct2D / DirectWrite rasteriser for [`crate::TabBar`] (#25).
/// Windows-only in full — see its module docs.
#[cfg(target_os = "windows")]
mod tab_bar;
/// Direct2D / DirectWrite rasteriser for [`crate::Terminal`] cell grids
/// (#30). Windows-only in full — see its module docs.
#[cfg(target_os = "windows")]
mod terminal;
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
/// Direct2D / DirectWrite rasteriser for [`crate::TextDisplay`] (#30).
/// Windows-only in full — see its module docs.
#[cfg(target_os = "windows")]
mod text_display;
/// Direct2D / DirectWrite rasteriser for
/// [`crate::primitives::text_input::TextInput`] (#733). Windows-only in
/// full — see its module docs.
#[cfg(target_os = "windows")]
mod text_input;
/// Direct2D / DirectWrite rasteriser for [`crate::ToastStack`] (#29).
/// Windows-only in full — see its module docs.
#[cfg(target_os = "windows")]
mod toast;
/// Direct2D / DirectWrite rasteriser for
/// [`crate::primitives::toolbar::Toolbar`] (#730). Windows-only in full
/// — see its module docs.
#[cfg(target_os = "windows")]
mod toolbar;
/// Direct2D / DirectWrite rasteriser for [`crate::Tooltip`] (#28).
/// Windows-only in full — see its module docs.
#[cfg(target_os = "windows")]
mod tooltip;
/// Direct2D / DirectWrite rasteriser for [`crate::TreeView`] (#26).
/// Windows-only in full — see its module docs.
#[cfg(target_os = "windows")]
mod tree;

#[cfg(target_os = "windows")]
pub use activity_bar::{draw_activity_bar, win_activity_bar_layout, ACTIVITY_ROW_DIP};
pub use backend::WinBackend;
#[cfg(target_os = "windows")]
pub use board::{draw_board, win_board_layout};
#[cfg(target_os = "windows")]
pub use chart::{draw_chart, win_chart_layout};
#[cfg(target_os = "windows")]
pub use command_center::{draw_command_center, win_command_center_layout};
#[cfg(target_os = "windows")]
pub use command_line::{draw_command_line, win_command_line_layout};
#[cfg(target_os = "windows")]
pub use completions::draw_completions;
#[cfg(target_os = "windows")]
pub use context_menu::draw_context_menu;
#[cfg(target_os = "windows")]
pub use data_table::{draw_data_table, win_data_table_layout};
#[cfg(target_os = "windows")]
pub use dialog::draw_dialog;
#[cfg(target_os = "windows")]
pub use diff_view::draw_diff_view;
#[cfg(target_os = "windows")]
pub use drop_overlay::draw_drop_overlay;
#[cfg(target_os = "windows")]
pub use editor::draw_editor;
#[cfg(target_os = "windows")]
pub use find_replace::draw_find_replace;
#[cfg(target_os = "windows")]
pub use form::{draw_form, draw_settings_chrome, win_form_layout};
#[cfg(target_os = "windows")]
pub use list::{draw_list, win_list_layout};
#[cfg(target_os = "windows")]
pub use menu_bar::{draw_menu_bar, win_menu_bar_layout};
#[cfg(target_os = "windows")]
pub use message_list::draw_message_list;
#[cfg(target_os = "windows")]
pub use minimap::{draw_minimap, win_minimap_layout};
#[cfg(target_os = "windows")]
pub use multi_section_view::{draw_multi_section_view, win_msv_layout, win_msv_metrics};
#[cfg(target_os = "windows")]
pub use palette::{draw_palette, win_palette_layout};
#[cfg(target_os = "windows")]
pub use panel::{draw_panel, win_panel_layout, ACTION_BUTTON_DIP};
#[cfg(target_os = "windows")]
pub use pipeline_view::{draw_pipeline_view, win_pipeline_view_layout};
#[cfg(target_os = "windows")]
pub use progress::{draw_progress, win_progress_layout, CANCEL_WIDTH_DIP};
#[cfg(target_os = "windows")]
pub use rich_text_popup::draw_rich_text_popup;
pub use run::{run, run_with, RunConfig};
#[cfg(target_os = "windows")]
pub use scrollbar::draw_scrollbar;
pub use services::WinPlatformServices;
#[cfg(target_os = "windows")]
pub use sidebar_panel::{draw_sidebar_panel, win_sidebar_panel_layout};
#[cfg(target_os = "windows")]
pub use spinner::{draw_spinner, win_spinner_layout};
#[cfg(target_os = "windows")]
pub use split::{draw_split, win_split_layout, DIVIDER_DIP};
#[cfg(target_os = "windows")]
pub use status_bar::{draw_status_bar, win_status_bar_layout, MIN_GAP_DIP};
#[cfg(target_os = "windows")]
pub use tab_bar::{draw_tab_bar, draw_tab_bar_icons, win_tab_bar_layout, win_tab_bar_layout_icons};
#[cfg(target_os = "windows")]
pub use terminal::{draw_terminal_cells, draw_terminal_divider};
#[cfg(target_os = "windows")]
pub use text_display::{draw_text_display, win_text_display_layout};
#[cfg(target_os = "windows")]
pub use text_input::{draw_text_input, win_text_input_layout};
#[cfg(target_os = "windows")]
pub use toast::{draw_toast_stack, win_toast_stack_layout};
#[cfg(target_os = "windows")]
pub use toolbar::{draw_toolbar, win_toolbar_layout};
#[cfg(target_os = "windows")]
pub use tooltip::{draw_tooltip, draw_tooltip_with_chrome};
#[cfg(target_os = "windows")]
pub use tree::{draw_tree, win_tree_layout};
