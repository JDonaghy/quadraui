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
pub mod board;
pub(crate) mod caret_blink;
pub mod chart;
pub mod command_center;
pub mod command_line;
pub mod completions;
pub mod context_menu;
pub mod data_table;
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
pub mod tab_bar;
pub mod terminal;
pub mod testing;
pub mod text;
pub mod text_display;
// Text-selection highlight painting (#803) — the CoreGraphics twin of
// `gtk::backend`'s Cairo `fill()` / `win::backend`'s Direct2D
// `FillRectangle` call. `pub(crate)`, not `pub`: only `MacBackend::
// apply_selection_highlight` calls into it, mirroring `menu_bar_install`'s
// visibility above.
pub(crate) mod text_selection;
pub mod toast;
pub mod toolbar;
pub mod tooltip;
pub mod tree;

pub use activity_bar::{draw_activity_bar, draw_activity_bar_with_style, mac_activity_bar_layout};
pub use backend::MacBackend;
pub use board::{draw_board, mac_board_layout};
pub use chart::mac_chart_layout;
pub use command_center::{draw_command_center, mac_command_center_layout};
pub use command_line::draw_command_line;
pub use completions::draw_completions;
pub use context_menu::draw_context_menu;
pub use data_table::{draw_data_table, mac_data_table_layout};
pub use dialog::draw_dialog;
// #866: `draw_diff_view` is `#[deprecated]` — see
// `diff_view::draw_diff_view`'s doc for why the shim exists and why
// re-exporting it here (rather than dropping the re-export) is the
// point. `#[allow(deprecated)]` for the same reason as `form::draw_form`'s
// re-export above.
#[allow(deprecated)]
pub use diff_view::draw_diff_view;
// #865: `draw_drop_overlay` is `#[deprecated]` — see
// `drop_overlay::draw_drop_overlay`'s doc for why the shim exists and
// why re-exporting it here (rather than dropping the re-export) is the
// point. `#[allow(deprecated)]` for the same reason as `form::draw_form`'s
// re-export above.
#[allow(deprecated)]
pub use drop_overlay::draw_drop_overlay;
pub use editor::draw_editor;
pub use form::{draw_settings_chrome, mac_form_layout};
// #808: `draw_form` is `#[deprecated]` — see `form::draw_form`'s doc for
// why the shim exists and why re-exporting it here (rather than dropping
// the re-export) is the point. `#[allow(deprecated)]` because a `pub
// use` of a deprecated item is itself a `deprecated`-lint use site, and
// this crate denies that lint in-repo (`RUSTFLAGS: -D warnings`) — see
// CLAUDE.md's "the `deprecated` lint is denied in-repo and allowed
// downstream" section for why that split is deliberate.
#[allow(deprecated)]
pub use form::draw_form;
pub use image::mac_draw_image;
pub use list::{draw_list, mac_list_layout};
pub use menu_bar::{draw_menu_bar, mac_menu_bar_layout};
pub use message_list::draw_message_list;
pub use minimap::mac_minimap_layout;
pub use multi_section_view::{draw_multi_section_view, mac_msv_layout, mac_msv_metrics};
pub use palette::{draw_palette, mac_palette_layout};
pub use panel::mac_panel_layout;
// #859: `draw_panel` is `#[deprecated]` — see `panel::draw_panel`'s doc
// for why the shim exists and why re-exporting it here (rather than
// dropping the re-export) is the point. `#[allow(deprecated)]` for the
// same reason as `form::draw_form`'s re-export above.
#[allow(deprecated)]
pub use panel::draw_panel;
pub use pipeline_view::{draw_pipeline_view, mac_pipeline_view_layout};
pub use progress::{draw_progress, mac_progress_layout};
pub use rich_text_popup::draw_rich_text_popup;
pub use run::run;
// #811: `draw_scrollbar` is `#[deprecated]` — see `scrollbar::draw_scrollbar`'s
// doc for why the shim exists and why re-exporting it here (rather than
// dropping the re-export) is the point. `#[allow(deprecated)]` for the
// same reason as `form::draw_form`'s re-export above.
#[allow(deprecated)]
pub use scrollbar::draw_scrollbar;
pub use services::MacPlatformServices;
pub use sidebar_panel::mac_sidebar_panel_layout;
// #862: `draw_sidebar_panel` is `#[deprecated]` — see
// `sidebar_panel::draw_sidebar_panel`'s doc for why the shim exists and
// why re-exporting it here (rather than dropping the re-export) is the
// point. `#[allow(deprecated)]` for the same reason as `form::draw_form`'s
// re-export above.
#[allow(deprecated)]
pub use sidebar_panel::draw_sidebar_panel;
pub use spinner::{draw_spinner, mac_spinner_layout};
pub use split::mac_split_layout;
// #864: `draw_split` is `#[deprecated]` — see `split::draw_split`'s doc
// for why the shim exists and why re-exporting it here (rather than
// dropping the re-export) is the point. `#[allow(deprecated)]` for the
// same reason as `form::draw_form`'s re-export above.
#[allow(deprecated)]
pub use split::draw_split;
// #863: `draw_split_tree` is `#[deprecated]` — see
// `split_tree::draw_split_tree`'s doc for why the shim exists and why
// re-exporting it here (rather than dropping the re-export) is the
// point. `#[allow(deprecated)]` for the same reason as `form::draw_form`'s
// re-export above.
#[allow(deprecated)]
pub use split_tree::{draw_split_tree, mac_split_tree_layout};
// #860: `draw_status_bar` is `#[deprecated]` — see
// `status_bar::draw_status_bar`'s doc for why the shim exists and why
// re-exporting it here (rather than dropping the re-export) is the
// point. `#[allow(deprecated)]` for the same reason as `form::draw_form`'s
// re-export above.
#[allow(deprecated)]
pub use status_bar::{draw_status_bar, mac_status_bar_layout};
pub use tab_bar::{draw_tab_bar, mac_tab_bar_layout};
pub use text_display::mac_text_display_layout;
// #861: `draw_toast_stack` is `#[deprecated]` — see
// `toast::draw_toast_stack`'s doc for why the shim exists and why
// re-exporting it here (rather than dropping the re-export) is the
// point. `#[allow(deprecated)]` for the same reason as `form::draw_form`'s
// re-export above.
#[allow(deprecated)]
pub use toast::{draw_toast_stack, mac_toast_stack_layout};
pub use toolbar::{draw_toolbar, mac_toolbar_layout};
pub use tooltip::{draw_tooltip, draw_tooltip_with_chrome};
pub use tree::{draw_tree, mac_tree_layout};
