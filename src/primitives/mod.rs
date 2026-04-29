//! Widget primitives.
//!
//! Each primitive module exports a declarative data struct describing the
//! widget, a companion event enum, and any supporting types. Backends
//! implement rendering and input handling against these types.

pub mod activity_bar;
pub mod completions;
pub mod context_menu;
pub mod dialog;
pub mod find_replace;
pub mod form;
pub mod list;
pub mod menu_bar;
pub mod message_list;
pub mod modal;
pub mod palette;
pub mod panel;
pub mod progress;
pub mod rich_text_popup;
pub mod scrollbar;
pub mod spinner;
pub mod split;
pub mod status_bar;
pub mod tab_bar;
pub mod terminal;
pub mod text_display;
pub mod toast;
pub mod tooltip;
pub mod tree;
