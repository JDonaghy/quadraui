//! The cell-grid toolbar helpers the TUI activity bar paints its rail
//! through — [`crate::primitives::toolbar::cell_layout`]'s rail layout
//! and [`crate::primitives::toolbar::cell_paint`]'s rail painter.

pub(crate) use crate::primitives::toolbar::cell_layout::{
    rail_layout_options, tui_toolbar_layout_with_options,
};
pub(crate) use crate::primitives::toolbar::cell_paint::draw_toolbar_with_rail_style;
