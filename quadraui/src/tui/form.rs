//! TUI entry points for [`crate::Form`] — thin re-exports of the
//! widget's own cell-grid layout and rasteriser,
//! [`crate::primitives::form::cell_layout`] and
//! [`crate::primitives::form::cell_paint`].
//!
//! Those two modules hold the whole TUI story (the uniform 1-cell row
//! measure, the shared label column, the per-`FieldKind` control glyphs
//! and the settings-panel chrome strip), next to the descriptor they
//! paint and to [`crate::primitive::Primitive::paint_cells`], which
//! reaches the same painter — so painting a `Form` through the widget
//! contract and through [`crate::Backend::draw_form`] is identical by
//! construction rather than by separately maintained parity.
//!
//! The names stay spelled exactly as they were here, so every
//! `crate::tui::draw_form` / `crate::tui::tui_form_layout` /
//! `crate::tui::draw_settings_chrome` caller (and `crate::tui::mod`'s
//! own re-exports) keeps compiling untouched.

pub use crate::primitives::form::cell_layout::tui_form_layout;
pub use crate::primitives::form::cell_paint::{draw_form, draw_settings_chrome};
