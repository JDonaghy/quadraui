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
//! rasterisers live as sibling modules. This pre-foundation milestone
//! (#32) only ships the bootstrap — events (#33), Core Text (#34), the
//! `MacBackend` trait impl (#35), and the per-primitive rasterisers
//! (#38–#43) land in follow-up issues.
//!
//! Per the [milestone description][milestone]: "Every existing
//! `AppLogic`-driven example runs on macOS unchanged once this
//! milestone ships." The trait integration that delivers that promise
//! lands in #35; #32 proves the AppKit + CG plumbing in isolation.
//!
//! [milestone]: https://github.com/JDonaghy/quadraui/milestone/4

pub mod activity_bar;
pub mod backend;
pub mod chart;
pub mod command_center;
pub mod completions;
pub mod context_menu;
pub mod data_table;
pub mod dialog;
pub mod editor;
pub mod events;
pub mod find_replace;
pub mod form;
#[cfg(test)]
pub mod headless;
pub mod list;
pub mod menu_bar;
pub(crate) mod menu_bar_install;
pub mod message_list;
pub mod multi_section_view;
pub mod palette;
pub mod panel;
pub mod progress;
pub mod rich_text_popup;
mod run;
pub mod scrollbar;
pub mod services;
pub mod spinner;
pub mod split;
pub mod status_bar;
pub mod tab_bar;
pub mod terminal;
pub mod text;
pub mod text_display;
pub mod toast;
pub mod tooltip;
pub mod tree;

pub use activity_bar::draw_activity_bar;
pub use backend::MacBackend;
pub use chart::{draw_chart, mac_chart_layout};
pub use command_center::{draw_command_center, mac_command_center_layout};
pub use completions::draw_completions;
pub use context_menu::draw_context_menu;
pub use data_table::{draw_data_table, mac_data_table_layout};
pub use dialog::draw_dialog;
pub use editor::draw_editor;
pub use find_replace::draw_find_replace;
pub use form::{draw_form, mac_form_layout};
pub use list::{draw_list, mac_list_layout};
pub use menu_bar::{draw_menu_bar, mac_menu_bar_layout};
pub use message_list::draw_message_list;
pub use multi_section_view::{draw_multi_section_view, mac_msv_layout, mac_msv_metrics};
pub use palette::{draw_palette, mac_palette_layout};
pub use panel::{draw_panel, mac_panel_layout};
pub use progress::{draw_progress, mac_progress_layout};
pub use rich_text_popup::draw_rich_text_popup;
pub use run::run;
pub use scrollbar::draw_scrollbar;
pub use services::MacPlatformServices;
pub use spinner::{draw_spinner, mac_spinner_layout};
pub use split::{draw_split, mac_split_layout};
pub use status_bar::draw_status_bar;
pub use tab_bar::draw_tab_bar;
pub use terminal::{draw_terminal_cells, draw_terminal_divider};
pub use text_display::{draw_text_display, mac_text_display_layout};
pub use toast::{draw_toast_stack, mac_toast_stack_layout};
pub use tooltip::draw_tooltip;
pub use tree::{draw_tree, mac_tree_layout};
