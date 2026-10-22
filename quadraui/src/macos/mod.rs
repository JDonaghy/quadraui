//! Public macOS (AppKit + Core Graphics + Core Text) rasterisers for
//! `quadraui` primitives.
//!
//! Enabled via the `macos` Cargo feature on a macOS host. Apps depend
//! on `quadraui` with `features = ["macos"]` and call into this module
//! to open an AppKit window and paint primitives onto a `CGContextRef`
//! that the runner sets up inside the view's `drawRect:` override.
//!
//! Mirrors the layout of [`crate::gtk`] and [`crate::tui`]: a `run`
//! entry point owns window + run-loop bootstrap, and per-primitive
//! rasterisers live as sibling modules.

pub mod activity_bar;
pub mod backend;
pub mod board;
pub mod button;
pub(crate) mod caret_blink;
// Shared CoreGraphics FFI bindings + fill/stroke/rect helpers.
mod cg;
pub mod chart;
pub mod command_center;
pub mod command_line;
pub mod completions;
pub mod context_menu;
pub mod data_table;
pub mod date_picker;
pub mod dialog;
pub mod diff_view;
pub mod drop_overlay;
pub mod editor;
pub mod events;
pub mod form;
pub mod headless;
pub mod image;
pub mod list;
pub mod menu_bar;
pub(crate) mod menu_bar_install;
pub mod message_list;
pub mod minimap;
pub mod multi_section_view;
pub mod palette;
pub mod panel;
pub mod pipeline_view;
pub mod progress;
pub mod radio_group;
pub mod rich_text_popup;
mod run;
pub mod scrollbar;
pub mod services;
pub mod shell_runner;
pub mod sidebar_panel;
pub mod spinner;
pub mod split;
pub mod split_tree;
pub mod status_bar;
pub mod switch;
// Shared `PaintSurface` adapter over a bare `CGContextRef`.
mod surface;
pub mod tab_bar;
pub mod terminal;
pub mod testing;
pub mod text;
pub mod text_display;
pub mod text_input;
// Text-selection highlight painting — the CoreGraphics twin of
// `gtk::backend`'s Cairo `fill()` / `win::backend`'s Direct2D
// `FillRectangle` call. `pub(crate)`, not `pub`: only `MacBackend::
// apply_selection_highlight` calls into it, mirroring `menu_bar_install`'s
// visibility above.
pub(crate) mod text_selection;
pub mod toast;
pub mod toolbar;
pub mod tooltip;
pub mod tree;
// Tray / status-bar icon control — `NSStatusBar`/`NSStatusItem`
// glue for `Backend::tray`. `pub(crate)`, not `pub`: only
// `impl TrayService for MacBackend` (`backend.rs`) calls into it,
// mirroring `menu_bar_install`'s visibility above.
pub(crate) mod tray;

pub use activity_bar::{draw_activity_bar, draw_activity_bar_with_style, mac_activity_bar_layout};
pub use backend::MacBackend;
pub use board::mac_board_layout;
pub use button::mac_button_layout;
pub use chart::mac_chart_layout;
pub use command_center::{draw_command_center, mac_command_center_layout};
pub use command_line::draw_command_line;
pub use completions::draw_completions;
pub use context_menu::draw_context_menu;
pub use data_table::{draw_data_table, mac_data_table_layout};
pub use dialog::draw_dialog;
pub use editor::draw_editor;
pub use form::{draw_settings_chrome, mac_form_layout};
pub use list::{draw_list, mac_list_layout};
pub use menu_bar::{draw_menu_bar, mac_menu_bar_layout};
pub use message_list::draw_message_list;
pub use minimap::mac_minimap_layout;
pub use multi_section_view::{draw_multi_section_view, mac_msv_layout, mac_msv_metrics};
pub use palette::{draw_palette, mac_palette_layout};
pub use panel::mac_panel_layout;
pub use pipeline_view::mac_pipeline_view_layout;
pub use progress::mac_progress_layout;
pub use radio_group::mac_radio_group_layout;
pub use rich_text_popup::draw_rich_text_popup;
pub use run::{run, run_with, MacRunner, RunConfig, StepOutcome};
pub use services::MacPlatformServices;
pub use sidebar_panel::mac_sidebar_panel_layout;
pub use spinner::{draw_spinner, mac_spinner_layout};
pub use split::mac_split_layout;
pub use split_tree::mac_split_tree_layout;
pub use status_bar::mac_status_bar_layout;
pub use switch::mac_switch_layout;
pub use tab_bar::{draw_tab_bar, mac_tab_bar_layout};
pub use text_display::mac_text_display_layout;
pub use toast::mac_toast_stack_layout;
pub use toolbar::{draw_toolbar, draw_toolbar_with_options, mac_toolbar_layout};
pub use tooltip::{draw_tooltip, draw_tooltip_with_chrome};
pub use tree::{draw_tree, mac_tree_layout};

/// Serialises every `#[test]` fn in this module tree that drives the
/// **one real systemwide `NSPasteboard`**.
///
/// `cargo test` runs `#[test]` fns on a pool of spawned threads, so any
/// two tests that write to the OS clipboard race each other.
///
/// The invariant: **anything that reads or writes the real pasteboard
/// takes this lock first**, for its whole read/write sequence, and holds
/// it until the assertions are done. Poisoning is recovered from rather
/// than propagated — one already-failing clipboard test should report its
/// own failure, not convert every other clipboard test into a second red
/// line.
#[cfg(test)]
static REAL_PASTEBOARD_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Acquire [`REAL_PASTEBOARD_LOCK`] — see that static's doc for the
/// invariant this enforces. Bind the guard to a named local (`let
/// _pasteboard = …`), never to `_`, or it is dropped immediately and
/// the lock buys nothing.
#[cfg(test)]
pub(crate) fn lock_real_pasteboard() -> std::sync::MutexGuard<'static, ()> {
    REAL_PASTEBOARD_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
