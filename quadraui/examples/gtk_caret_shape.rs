//! GTK runner for the caret-shape demo (issue #1015).
//!
//! Same `AppLogic` as `tui_caret_shape`, proving the
//! `Backend::set_caret_shape` dispatch is backend-generic even though
//! GTK's implementation stays at the trait's no-op default: GTK4 paints
//! its own caret in `draw_editor` and has no separate hardware cursor to
//! steer, so there is nothing for this call to do here — see
//! `Backend::set_caret_shape`'s doc for why that default is the honest
//! answer rather than a gap.

#[path = "common/mod.rs"]
mod common;

fn main() {
    quadraui::gtk::run(common::caret_shape_demo::CaretShapeDemo::new());
}
