//! TUI entry points for [`crate::primitives::toolbar::Toolbar`] — thin
//! re-exports of the widget's own cell-grid layout and rasteriser,
//! [`crate::primitives::toolbar::cell_layout`] and
//! [`crate::primitives::toolbar::cell_paint`].
//!
//! Those two modules hold the whole TUI story (the `[ icon label (key) ]`
//! bracket cells, the `Group` degrade tiers, the per-state colouring
//! table, the `Wrap`/`Menu` overflow rows and the vertical icon rail),
//! next to the descriptor they paint and to
//! [`crate::primitive::Primitive::paint_cells`], which reaches the same
//! painter — so painting a `Toolbar` through the widget contract and
//! through [`crate::Backend::draw_toolbar_interactive`] is identical by
//! construction rather than by separately maintained parity.
//!
//! The names stay spelled exactly as they were here, so every
//! `crate::tui::draw_toolbar*`/`crate::tui::tui_toolbar_layout*` caller
//! (and `crate::tui::mod`'s own re-exports) keeps compiling untouched.

pub(crate) use crate::primitives::toolbar::cell_layout::{rail_layout_options, tui_item_width};
pub use crate::primitives::toolbar::cell_layout::{
    tui_toolbar_layout, tui_toolbar_layout_with_options, tui_toolbar_layout_with_overflow,
};
pub(crate) use crate::primitives::toolbar::cell_paint::draw_toolbar_with_rail_style;
pub use crate::primitives::toolbar::cell_paint::{
    draw_toolbar, draw_toolbar_oriented, draw_toolbar_with_options, draw_toolbar_with_overflow,
};
