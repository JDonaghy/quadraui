//! One module per ported demo. Each module is registered in
//! [`crate::registry::registry`] — adding a demo module here does
//! nothing by itself until it also gets a line there.

pub mod activity_bar;
pub mod bottom_panel;
pub mod caret_shape;
pub mod command_line;
pub mod file_picker;
pub mod find_replace;
pub mod float;
pub mod focus;
pub mod form;
pub mod menu_bar;
pub mod palette;
pub mod panel;
pub mod sidebar;
pub mod split;
pub mod status_bar;
pub mod tab_bar;
pub mod text_input;
pub mod text_selection;
pub mod toast;
pub mod toolbar;
pub mod window_control;
pub mod workspace;
