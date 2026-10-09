//! One module per ported demo. Each module is registered in
//! [`crate::registry::registry`] — adding a demo module here does
//! nothing by itself until it also gets a line there.

pub mod activity_bar;
pub mod board;
pub mod bottom_panel;
pub mod canvas;
pub mod caret_shape;
pub mod chart;
pub mod chat;
pub mod clipboard;
pub mod command_line;
pub mod context_menu;
pub mod data_table;
pub mod dialog;
pub mod diff_view;
pub mod editor;
pub mod file_dialog;
pub mod file_picker;
pub mod find_replace;
pub mod float;
pub mod focus;
pub mod form;
pub mod help_overlay;
pub mod image;
pub mod indicators;
pub mod markdown;
pub mod menu_bar;
pub mod message_dialog;
pub mod message_list;
pub mod minimap;
pub mod palette;
pub mod panel;
pub mod pipeline;
pub mod search_panel;
pub mod sidebar;
pub mod split;
pub mod status_bar;
pub mod tab_bar;
// Spawns a real PTY shell session (quadraui::terminal_engine) — gated on
// the `terminal` feature so a plain `tui`/`gtk` build never pulls in
// portable-pty + vt100 just to compile a demo it has no way to run.
#[cfg(feature = "terminal")]
pub mod terminal;
pub mod text_display;
pub mod text_input;
pub mod text_selection;
pub mod toast;
pub mod toolbar;
pub mod tooltip;
pub mod tree;
pub mod window_control;
pub mod workspace;
