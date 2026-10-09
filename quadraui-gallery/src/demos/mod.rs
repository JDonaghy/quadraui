//! One module per ported demo. Each module is registered in
//! [`crate::registry::registry`] — adding a demo module here does
//! nothing by itself until it also gets a line there.

pub mod activity_bar;
pub mod bottom_panel;
pub mod float;
pub mod menu_bar;
pub mod panel;
pub mod sidebar;
pub mod split;
pub mod status_bar;
pub mod tab_bar;
pub mod toast;
pub mod toolbar;
pub mod window_control;
pub mod workspace;
