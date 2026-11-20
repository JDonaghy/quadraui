//! GTK runner for `CellGridDemo` — proves the virtual spreadsheet sheet,
//! its formula bar and its sheet tabs are backend-generic: the same
//! `AppLogic` as `tui_cell_grid`, painted through `CellGrid`'s shared
//! pixel-paint path (`native_surface_paint::paint`) instead of the
//! terminal rasteriser.
#[path = "common/mod.rs"]
mod common;

fn main() {
    quadraui::gtk::run(common::CellGridDemo::new());
}
