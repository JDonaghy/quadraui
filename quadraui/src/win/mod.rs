//! Win-GUI backend: Direct2D + DirectWrite rasterisers.
//!
//! The Win32 window + Direct2D render-target bootstrap
//! (`run.rs`'s message loop, `backend.rs`'s `begin_frame`/`end_frame`)
//! uses real WinAPI calls gated on `cfg(target_os = "windows")` so `cargo
//! check --features win` still type-checks `WinBackend`'s trait
//! completeness on Linux (see `backend.rs`'s module docs and `Cargo.toml`'s
//! `win` feature comment). Chrome-strip rasterisers include
//! `status_bar`/`tab_bar`/`activity_bar`/`menu_bar`; content-area
//! rasterisers include `tree`/`list`/`form`/`data_table`/`editor`/`chart`;
//! overlay/popup rasterisers include `tooltip`/`context_menu`/`dialog`/
//! `palette`/`completions`/`find_replace`/`rich_text_popup`; container/
//! indicator rasterisers include `panel`/`split`/`toast`/`progress`/
//! `spinner`; text-heavy rasterisers include `terminal`/`text_display`/
//! `message_list` — every other `draw_*`/`*_layout` rasteriser is still a
//! `todo!()` stub; implement each one against Direct2D / DirectWrite and
//! the compiler will tell you when you're done.
//!
//! See `quadraui/docs/NATIVE_GUI_LESSONS.md` for pitfalls discovered
//! during Win-GUI work. See the GTK backend (`quadraui/src/gtk/`)
//! as the reference implementation for a pixel-based backend.

/// Direct2D / DirectWrite rasteriser for [`crate::ActivityBar`].
/// Windows-only in full — see its module docs.
#[cfg(target_os = "windows")]
mod activity_bar;
pub mod backend;
/// Direct2D / DirectWrite rasteriser for
/// [`crate::primitives::board::BoardModel`]. Windows-only in full —
/// see its module docs.
#[cfg(target_os = "windows")]
mod board;
/// Direct2D / DirectWrite rasteriser for [`crate::primitives::chart::Chart`].
/// Windows-only in full — see its module docs.
#[cfg(target_os = "windows")]
mod chart;
/// Direct2D / DirectWrite rasteriser for [`crate::CommandCenter`].
/// Windows-only in full — see its module docs.
#[cfg(target_os = "windows")]
mod command_center;
/// Direct2D / DirectWrite rasteriser for
/// [`crate::primitives::command_line::CommandLine`]. Windows-only
/// in full — see its module docs.
#[cfg(target_os = "windows")]
mod command_line;
/// Direct2D / DirectWrite rasteriser for [`crate::Completions`].
/// Windows-only in full — see its module docs.
#[cfg(target_os = "windows")]
mod completions;
/// Direct2D / DirectWrite rasteriser for [`crate::ContextMenu`].
/// Windows-only in full — see its module docs.
#[cfg(target_os = "windows")]
mod context_menu;
/// Direct2D / DirectWrite rasteriser for [`crate::DataTable`]. Issue
/// Only its paint fn is Windows-only — see its module docs.
mod data_table;
/// Direct2D / DirectWrite rasteriser for [`crate::Dialog`].
/// Windows-only in full — see its module docs.
#[cfg(target_os = "windows")]
mod dialog;
/// Direct2D / DirectWrite rasteriser for
/// [`crate::primitives::diff_view::DiffView`]. Windows-only in
/// full — see its module docs.
#[cfg(target_os = "windows")]
mod diff_view;
/// Direct2D rasteriser for
/// [`crate::primitives::drop_zone::DropOverlay`]. Windows-only
/// in full — see its module docs.
#[cfg(target_os = "windows")]
mod drop_overlay;
/// Direct2D / DirectWrite rasteriser for [`crate::primitives::editor::Editor`].
/// Windows-only in full — see its module docs.
#[cfg(target_os = "windows")]
mod editor;
/// Win32 message → `quadraui::UiEvent` translation (mouse, keyboard,
/// focus). Pure free functions, host-independent and unit-tested off
/// Windows — mirrors `crate::gtk::events` / `crate::macos::events`. See
/// its module docs. `WM_SIZE`/`WM_DPICHANGED`/`WM_CLOSE` translation
/// is handled in `msg` + `run` — see this module's docs.
pub mod events;
/// Direct2D / DirectWrite rasteriser for [`crate::Form`].
/// Windows-only in full — see its module docs.
#[cfg(target_os = "windows")]
mod form;
/// Direct2D + WIC rasteriser for [`crate::primitives::image::Image`].
/// Windows-only in full — see its module docs.
#[cfg(target_os = "windows")]
mod image;
/// Direct2D / DirectWrite rasteriser for [`crate::ListView`].
/// [`list::win_list_layout`] is pure geometry and compiles/runs
/// everywhere; only [`list::draw_list`] is Windows-only — see that
/// module's docs.
mod list;
/// Direct2D / DirectWrite rasteriser for [`crate::MenuBar`]. Issue
/// Only its paint fn is Windows-only — see its module docs.
mod menu_bar;
/// Direct2D / DirectWrite rasteriser for [`crate::MessageList`].
/// Windows-only in full — see its module docs.
#[cfg(target_os = "windows")]
mod message_list;
/// Direct2D / DirectWrite rasteriser for
/// [`crate::primitives::minimap::Minimap`]. Windows-only in full —
/// see its module docs.
#[cfg(target_os = "windows")]
mod minimap;
/// Win32 message-payload decoding (`WPARAM`/`LPARAM` word unpacking, DPI
/// ratio). Crate-private and host-independent — it is the one part of this
/// backend that is pure arithmetic, so it is the one part that can be
/// unit-tested off Windows. See its module docs.
pub(crate) mod msg;
/// Direct2D / DirectWrite rasteriser for [`crate::MultiSectionView`].
/// Windows-only in full — see its module docs.
#[cfg(target_os = "windows")]
mod multi_section_view;
/// Direct2D / DirectWrite rasteriser for [`crate::Palette`].
/// Windows-only in full — see its module docs.
#[cfg(target_os = "windows")]
mod palette;
/// Direct2D / DirectWrite rasteriser for [`crate::Panel`].
/// Windows-only in full — see its module docs.
#[cfg(target_os = "windows")]
mod panel;
/// Direct2D / DirectWrite rasteriser for
/// [`crate::primitives::pipeline_view::PipelineView`]. Windows-only
/// in full — see its module docs.
#[cfg(target_os = "windows")]
mod pipeline_view;
/// Direct2D / DirectWrite rasteriser for [`crate::ProgressBar`].
/// Windows-only in full — see its module docs.
#[cfg(target_os = "windows")]
mod progress;
/// Direct2D / DirectWrite rasteriser for [`crate::RichTextPopup`].
/// Windows-only in full — see its module docs.
#[cfg(target_os = "windows")]
mod rich_text_popup;
pub mod run;
/// Win painter for [`crate::ScrollView`]: `paint_clipped` +
/// `draw_scrollbar_for`, generic over [`crate::Backend`] like its GTK
/// counterpart — see its module docs.
pub mod scroll_view;
/// Direct2D rasteriser for [`crate::Scrollbar`]. Windows-only in
/// full — see its module docs.
#[cfg(target_os = "windows")]
mod scrollbar;
pub mod services;
/// `run_with_shell()`: composes a [`crate::ShellApp`] into an `AppShell`
/// and drives it through the Win32 message loop — the Windows
/// analogue of `gtk::shell_runner` / `tui::shell_runner` /
/// `macos::shell_runner`. `pub` (not target-gated) for the same
/// "compiles everywhere, only *works* on Windows" reason as `run`/`mod
/// run` above — see this module's own doc.
pub mod shell_runner;
/// Direct2D / DirectWrite rasteriser for
/// [`crate::primitives::sidebar_panel::SidebarPanel`]. Windows-only
/// in full — see its module docs.
#[cfg(target_os = "windows")]
mod sidebar_panel;
/// Direct2D / DirectWrite rasteriser for [`crate::Spinner`]. Issue
/// Only its paint fn is Windows-only — see its module docs.
mod spinner;
/// Direct2D rasteriser for [`crate::Split`]. Windows-only in
/// full — see its module docs.
#[cfg(target_os = "windows")]
mod split;
/// Direct2D rasteriser for [`crate::primitives::split_tree::SplitTree`].
/// Windows-only in full — see its module docs.
#[cfg(target_os = "windows")]
mod split_tree;
/// Direct2D / DirectWrite rasteriser for [`crate::StatusBar`]. Issue
/// Only its deprecated paint shim is Windows-only — see its
/// module docs.
mod status_bar;
/// Shared `PaintSurface` adapter over a bare `&ID2D1RenderTarget`.
/// Windows-only — same gating as the per-primitive
/// rasteriser modules above that consume it.
#[cfg(target_os = "windows")]
mod surface;
/// Direct2D / DirectWrite rasteriser for [`crate::TabBar`]. Issue
/// Only its paint fns are Windows-only — see its module docs.
mod tab_bar;
/// Direct2D / DirectWrite rasteriser for [`crate::Terminal`] cell grids.
/// Windows-only in full — see its module docs.
#[cfg(target_os = "windows")]
mod terminal;
/// Headless Direct2D test surface: an offscreen `ID2D1DCRenderTarget`
/// backed by an in-memory DIB section, so `#[cfg(test)]` blocks can paint
/// a primitive and read pixels back with no `HWND`, display, or GPU.
/// `pub` (like [`crate::tui::testing`] / [`crate::gtk::testing`]) rather
/// than `pub(crate)` — see its module docs for why.
///
/// `feature = "win"` alone — **not** `target_os = "windows"`
/// — mirrors `backend`/`run`/`shell_runner` above: every real Direct2D/GDI
/// call inside is individually `cfg(target_os = "windows")`-gated with a
/// non-functional fallback everywhere else, so `WinDriver`/
/// `driver_with_shell`/`ConformanceDriver for WinDriver` type-check on an
/// ordinary Linux host — see this module's own doc for why that matters
/// to downstream adoption of `quadraui::testing::ConformanceDriver`.
pub mod testing;
/// DirectWrite text infrastructure — factory + text-format creation,
/// font-metrics measurement, and Direct2D text painting. Windows-only
/// in full: nothing in here has a meaningful non-Windows fallback (unlike
/// `msg`, this isn't pure arithmetic), so the module itself only exists on
/// `target_os = "windows"` rather than being internally `cfg`-gated
/// line-by-line like `backend.rs`.
///
/// `pub` rather than `pub(crate)`: the chrome rasterisers re-exported
/// below (`draw_tab_bar`, …) take a `&DWrite` measurer in
/// their signatures, exactly as [`crate::macos`]'s take a `&CTFont` from its
/// own `pub mod text`. A `pub` function whose parameter type is `pub(crate)`
/// is a `private_interfaces` warning — and this repo's CI runs with
/// `RUSTFLAGS: "-D warnings"`, so it is a *build failure* on the
/// windows-latest leg (nothing in this module compiles on Linux, so the
/// ubuntu `cargo check --features win` leg cannot see it). Keep this module
/// and `DWrite`'s public surface in step with the rasterisers that take it.
#[cfg(target_os = "windows")]
pub mod text;
/// Direct2D / DirectWrite rasteriser for [`crate::TextDisplay`].
/// Windows-only in full — see its module docs.
#[cfg(target_os = "windows")]
mod text_display;
/// Direct2D / DirectWrite rasteriser for
/// [`crate::primitives::text_input::TextInput`]. Windows-only in
/// full — see its module docs.
#[cfg(target_os = "windows")]
mod text_input;
/// Direct2D / DirectWrite rasteriser for [`crate::ToastOverlay`]. Issue
/// Only its deprecated paint shim is Windows-only — see its
/// module docs.
mod toast;
/// Direct2D / DirectWrite rasteriser for
/// [`crate::primitives::toolbar::Toolbar`]. Windows-only in full
/// — see its module docs.
#[cfg(target_os = "windows")]
mod toolbar;
/// Direct2D / DirectWrite rasteriser for [`crate::Tooltip`].
/// Windows-only in full — see its module docs.
#[cfg(target_os = "windows")]
mod tooltip;
/// Tray/status-bar icon control (`Shell_NotifyIconW`) for `Backend::tray`.
/// `pub(crate)` (not target-gated) for the same "compiles
/// everywhere via internal `cfg(target_os = "windows")`, only *works* on
/// Windows" reason as `backend`/`services`/`run` above — see this
/// module's own doc and `tray`'s module doc.
pub(crate) mod tray;
/// Direct2D / DirectWrite rasteriser for [`crate::TreeView`].
/// Windows-only in full — see its module docs.
#[cfg(target_os = "windows")]
mod tree;

#[cfg(target_os = "windows")]
pub use activity_bar::{draw_activity_bar, win_activity_bar_layout, ACTIVITY_ROW_DIP};
pub use backend::WinBackend;
#[cfg(target_os = "windows")]
pub use board::win_board_layout;
#[cfg(target_os = "windows")]
pub use chart::win_chart_layout;
#[cfg(target_os = "windows")]
pub use command_center::{draw_command_center, win_command_center_layout};
#[cfg(target_os = "windows")]
pub use command_line::{draw_command_line, draw_command_line_selection, win_command_line_layout};
#[cfg(target_os = "windows")]
pub use completions::draw_completions;
#[cfg(target_os = "windows")]
pub use context_menu::draw_context_menu;
#[cfg(target_os = "windows")]
pub use data_table::draw_data_table;
pub use data_table::win_data_table_layout;
#[cfg(target_os = "windows")]
pub use dialog::draw_dialog;
#[cfg(target_os = "windows")]
pub use editor::draw_editor;
#[cfg(target_os = "windows")]
pub use form::{draw_settings_chrome, win_form_layout};
#[cfg(target_os = "windows")]
pub use image::draw_image;
#[cfg(target_os = "windows")]
pub use list::draw_list;
pub use list::win_list_layout;
#[cfg(target_os = "windows")]
pub use menu_bar::draw_menu_bar;
pub use menu_bar::win_menu_bar_layout;
#[cfg(target_os = "windows")]
pub use message_list::draw_message_list;
#[cfg(target_os = "windows")]
pub use minimap::{draw_minimap, win_minimap_layout};
#[cfg(target_os = "windows")]
pub use multi_section_view::{draw_multi_section_view, win_msv_layout, win_msv_metrics};
#[cfg(target_os = "windows")]
pub use palette::{draw_palette, win_palette_layout};
#[cfg(target_os = "windows")]
pub use panel::win_panel_layout;
#[cfg(target_os = "windows")]
pub use pipeline_view::win_pipeline_view_layout;
#[cfg(target_os = "windows")]
pub use progress::win_progress_layout;
#[cfg(target_os = "windows")]
pub use rich_text_popup::draw_rich_text_popup;
pub use run::{run, run_with, RunConfig};
pub use services::WinPlatformServices;
#[cfg(target_os = "windows")]
pub use sidebar_panel::win_sidebar_panel_layout;
#[cfg(target_os = "windows")]
pub use spinner::draw_spinner;
pub use spinner::win_spinner_layout;
#[cfg(target_os = "windows")]
pub use split::win_split_layout;
#[cfg(target_os = "windows")]
pub use split_tree::win_split_tree_layout;
pub use status_bar::{win_status_bar_layout, MIN_GAP_DIP};
#[cfg(target_os = "windows")]
#[allow(deprecated)]
pub use tab_bar::{draw_tab_bar, draw_tab_bar_icons};
#[allow(deprecated)]
pub use tab_bar::{win_tab_bar_layout, win_tab_bar_layout_icons};
// The `TabBarLayout`-returning counterparts to the four above —
// exported for the same reason: a consumer holding only the free
// rasteriser fns (not going through `Backend`) gets the same additive
// pair every other new-method site does. The `win_tab_bar_*` no-paint
// fns are pure geometry (generic over `TextMeasure`) and compile
// everywhere; only the two `draw_tab_bar_*` paint fns are Windows-only.
#[cfg(target_os = "windows")]
pub use tab_bar::{draw_tab_bar_icons_layout, draw_tab_bar_layout};
pub use tab_bar::{win_tab_bar_native_layout, win_tab_bar_native_layout_icons};
#[cfg(target_os = "windows")]
pub use text_display::win_text_display_layout;
#[cfg(target_os = "windows")]
pub use text_input::{draw_text_input, win_text_input_layout};
pub use toast::win_toast_stack_layout;
#[cfg(target_os = "windows")]
pub use toolbar::{draw_toolbar, draw_toolbar_with_options, win_toolbar_layout};
#[cfg(target_os = "windows")]
pub use tooltip::{draw_tooltip, draw_tooltip_with_chrome};
#[cfg(target_os = "windows")]
pub use tree::{draw_tree, win_tree_layout};

/// Serialises every `#[test]` fn in this module tree that drives the
/// **one real systemwide Windows clipboard**.
///
/// `cargo test` runs `#[test]` fns on a pool of spawned threads, so any
/// two tests that write to the OS clipboard race each other — and the
/// loser reads back whatever the winner just wrote. On Windows the
/// clipboard is a single global owned by whichever thread most recently
/// called `OpenClipboard`/`SetClipboardData`, so there is no per-test
/// isolation to fall back on.
///
/// Three tests in [`run`] touch it, all only on `target_os = "windows"`
/// (off Windows `WinClipboard` is an unconditional no-op stub, so they
/// can't race):
///
/// - `paste_dispatch_tests::ctrl_v_delivers_real_clipboard_text_as_clipboard_paste`
///   seeds a distinctive payload, reads it back to confirm the host's
///   clipboard is usable at all, then asserts Ctrl-V delivers *that*
///   payload — a three-step sequence that is only sound if nothing else
///   writes in between.
/// - `text_selection_dispatch_tests::drag_select_then_ctrl_c_copies_the_selection`
///   presses Ctrl-C over a selection, which copies `"hello world"` to
///   that same real clipboard.
/// - `paste_dispatch_tests::ctrl_v_is_never_forwarded_to_the_app_as_a_raw_keypress`
///   reads it (its assertion tolerates any contents, but it holds the
///   clipboard open while doing so, which is enough to make a concurrent
///   `OpenClipboard` fail).
///
/// This enforces the invariant that any sequence of clipboard
/// operations must complete atomically without interleaving from other
/// tests.
///
/// **Anything that reads or writes the real clipboard takes this lock
/// first**, for its whole read/write sequence, and holds it until the
/// assertions are done. Poisoning is recovered from rather than
/// propagated — one already-failing clipboard test should report its own
/// failure, not convert every other clipboard test into a second red
/// line.
#[cfg(test)]
static REAL_CLIPBOARD_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Acquire [`REAL_CLIPBOARD_LOCK`] — see that static's doc for the
/// invariant this enforces. Bind the guard to a named local (`let
/// _clipboard = …`), never to `_`, or it is dropped immediately and the
/// lock buys nothing.
#[cfg(test)]
pub(crate) fn lock_real_clipboard() -> std::sync::MutexGuard<'static, ()> {
    REAL_CLIPBOARD_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
