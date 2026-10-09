//! The demo registry — the one list port issues append a line to.
//!
//! Each entry is a constructor for one boxed [`crate::Demo`]. The gallery
//! shell (`crate::app::GalleryApp`) only ever iterates this list; it has
//! no per-demo knowledge beyond what `Demo` exposes, so adding a demo is
//! exactly one line here plus the demo's own module — no shell code
//! changes.

use crate::demos::activity_bar::ActivityBarDemo;
use crate::demos::board::BoardDemo;
use crate::demos::bottom_panel::BottomPanelDemo;
use crate::demos::canvas::CanvasDemo;
use crate::demos::caret_shape::CaretShapeDemo;
use crate::demos::chart::ChartDemo;
use crate::demos::command_line::CommandLineDemo;
use crate::demos::data_table::DataTableDemo;
use crate::demos::diff_view::DiffViewDemo;
use crate::demos::file_picker::FilePickerDemo;
use crate::demos::find_replace::FindReplaceDemo;
use crate::demos::float::FloatDemo;
use crate::demos::focus::FocusDemo;
use crate::demos::form::FormDemo;
use crate::demos::image::ImageDemo;
use crate::demos::indicators::IndicatorsDemo;
use crate::demos::menu_bar::MenuBarDemo;
use crate::demos::message_list::MessageListDemo;
use crate::demos::minimap::MinimapDemo;
use crate::demos::palette::PaletteDemo;
use crate::demos::panel::PanelDemo;
use crate::demos::pipeline::PipelineDemo;
use crate::demos::search_panel::SearchPanelDemo;
use crate::demos::sidebar::SidebarDemo;
use crate::demos::split::SplitDemo;
use crate::demos::status_bar::StatusBarDemo;
use crate::demos::tab_bar::TabBarDemo;
use crate::demos::text_input::TextInputDemo;
use crate::demos::text_selection::TextSelectionDemo;
use crate::demos::toast::ToastDemo;
use crate::demos::toolbar::ToolbarDemo;
use crate::demos::tree::TreeDemo;
use crate::demos::window_control::WindowControlDemo;
use crate::demos::workspace::WorkspaceDemo;
use crate::Demo;

/// Construct every registered demo, in registration order.
///
/// Port issues append one `Box::new(...)` line to this `vec!` — nothing
/// else in the gallery needs to change. The registry-driven test in
/// `tests/gallery_driver.rs` exercises every entry this returns, so a
/// newly-appended demo gets smoke coverage for free.
pub fn registry() -> Vec<Box<dyn Demo>> {
    vec![
        Box::new(ToastDemo::new()),
        // Layout & chrome.
        Box::new(ActivityBarDemo::new()),
        Box::new(MenuBarDemo::new()),
        Box::new(ToolbarDemo::new()),
        Box::new(StatusBarDemo::new()),
        Box::new(TabBarDemo::new()),
        Box::new(BottomPanelDemo::new()),
        Box::new(PanelDemo::new()),
        Box::new(SidebarDemo::new()),
        Box::new(SplitDemo::new()),
        Box::new(FloatDemo::new()),
        Box::new(WindowControlDemo::new()),
        Box::new(WorkspaceDemo::new()),
        // Input & forms.
        Box::new(TextInputDemo::new()),
        Box::new(FormDemo::new()),
        Box::new(CaretShapeDemo::new()),
        Box::new(CommandLineDemo::new()),
        Box::new(TextSelectionDemo::new()),
        Box::new(FindReplaceDemo::new()),
        Box::new(FocusDemo::new()),
        Box::new(PaletteDemo::new()),
        Box::new(FilePickerDemo::new()),
        // Data views.
        Box::new(DataTableDemo::new()),
        Box::new(TreeDemo::new()),
        Box::new(ChartDemo::new()),
        Box::new(BoardDemo::new()),
        Box::new(PipelineDemo::new()),
        Box::new(DiffViewDemo::new()),
        Box::new(MinimapDemo::new()),
        Box::new(IndicatorsDemo::new()),
        Box::new(CanvasDemo::new()),
        Box::new(ImageDemo::new()),
        Box::new(SearchPanelDemo::new()),
        Box::new(MessageListDemo::new()),
    ]
}
