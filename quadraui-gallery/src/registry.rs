//! The demo registry — the one list port issues append a line to.
//!
//! Each entry is a constructor for one boxed [`crate::Demo`]. The gallery
//! shell (`crate::app::GalleryApp`) only ever iterates this list; it has
//! no per-demo knowledge beyond what `Demo` exposes, so adding a demo is
//! exactly one line here plus the demo's own module — no shell code
//! changes.

use crate::demos::activity_bar::ActivityBarDemo;
use crate::demos::bottom_panel::BottomPanelDemo;
use crate::demos::float::FloatDemo;
use crate::demos::menu_bar::MenuBarDemo;
use crate::demos::panel::PanelDemo;
use crate::demos::sidebar::SidebarDemo;
use crate::demos::split::SplitDemo;
use crate::demos::status_bar::StatusBarDemo;
use crate::demos::tab_bar::TabBarDemo;
use crate::demos::toast::ToastDemo;
use crate::demos::toolbar::ToolbarDemo;
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
        // Layout & chrome (issue #1343).
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
    ]
}
