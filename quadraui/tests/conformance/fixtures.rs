//! Conformance **fixture registry** (quadraui#491, audit §6.4).
//!
//! Maps a scenario's `"fixture"` string to an `examples/common/*.rs`
//! constructor. The registry is written once and is backend-agnostic:
//! [`build`] is generic over the [`DriverFactory`] a backend supplies, so
//! adding a backend costs one factory impl plus one registration line in
//! `tests/conformance.rs` — **not** one line per fixture.
//!
//! Adding a fixture is one match arm plus one [`FIXTURES`] entry. Every
//! fixture here is an existing paired example app (`tui_<name>` /
//! `gtk_<name>`), so a scenario written against it is by construction
//! runnable on every backend that has the example.

use quadraui::testing::LogicalViewport;

use super::common;
use super::runner::{DriverFactory, DynDriver};

/// Every fixture name [`build`] accepts, in registry order.
///
/// Kept beside the match arms so `fixtures_list_matches_build` can prove
/// the two never drift — a name listed but unbuildable (or vice versa)
/// would otherwise only surface as a mysterious "unknown fixture" row in
/// the matrix.
pub const FIXTURES: &[&str] = &[
    "data_table_app",
    "dialog_table_demo",
    "file_dialog_demo",
    "folder_picker_app",
    "form_scroll",
    "menu_bar_app",
    "mini_app",
    "modal_occlusion_demo",
    "palette_dual_mode_app",
    "panel_app",
    "pipeline_app",
    "shell_app",
    "split_app",
    "tab_group_demo",
    "toast_app",
];

/// Construct the driver for `fixture` using backend factory `F`.
///
/// `None` means "no such fixture" — the runner turns that into a `FAIL`
/// row naming this file, rather than a silent pass.
pub fn build<F: DriverFactory>(
    fixture: &str,
    viewport: LogicalViewport,
) -> Option<Box<dyn DynDriver>> {
    Some(match fixture {
        "data_table_app" => F::make(common::data_table_app::DataTableApp::new(), viewport),
        "dialog_table_demo" => F::make(common::dialog_table_demo::DialogTableDemo::new(), viewport),
        "file_dialog_demo" => F::make(common::file_dialog_demo::FileDialogDemo::new(), viewport),
        "folder_picker_app" => F::make(common::folder_picker_app::FolderPickerApp::new(), viewport),
        "form_scroll" => F::make(common::form_scroll::FormScrollApp::new(), viewport),
        "menu_bar_app" => F::make(common::menu_bar_app::MenuBarApp::new(), viewport),
        "mini_app" => F::make(common::mini_app::MiniApp::new(), viewport),
        "modal_occlusion_demo" => F::make(
            common::modal_occlusion_demo::ModalOcclusionDemo::new(),
            viewport,
        ),
        "palette_dual_mode_app" => F::make(
            common::palette_dual_mode_app::PaletteDualModeApp::new(),
            viewport,
        ),
        "panel_app" => F::make(common::panel_app::PanelApp::new(), viewport),
        "pipeline_app" => F::make(common::pipeline_app::PipelineApp::new(), viewport),
        "shell_app" => F::make(common::shell_app::ShellApp::new(), viewport),
        "split_app" => F::make(common::split_app::SplitApp::new(), viewport),
        "tab_group_demo" => F::make(common::tab_group_demo::TabGroupDemo::new(), viewport),
        "toast_app" => F::make(common::toast_app::ToastApp::new(), viewport),
        _ => return None,
    })
}
