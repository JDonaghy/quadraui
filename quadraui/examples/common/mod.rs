//! Backend-agnostic app code shared by every example in this crate.
//!
//! After the runner-crate vision shipped (#269 / #270 stage B), every
//! example is just a thin `main` that constructs an `AppLogic` impl
//! and calls `quadraui::{tui,gtk}::run(app)`. The `AppLogic` bodies
//! themselves live in this module, identical across both backends —
//! that's the payoff of the runner abstraction.
//!
//! Cargo doesn't auto-treat `examples/common/mod.rs` as an example
//! binary (it would if it were named `common.rs` at the examples/
//! root). Each example references it via `#[path = "common/mod.rs"]
//! mod common;`. This is the canonical pattern for shared example
//! helpers in Rust crates.
//!
//! - [`MiniApp`] (in [`mini_app`]) — minimal one-StatusBar app, used
//!   by `tui_app` / `gtk_app`.
//! - [`AppState`] (in [`demo`]) — richer demo state (tabs + status
//!   focus + last message), used by `tui_demo` / `gtk_demo`.
//! - [`DebugSidebar`] (in [`multi_tree`]) — `MultiSectionView` with
//!   N collapsible-tree sections, used by `tui_multi_tree` /
//!   `gtk_multi_tree`. Demonstrates the consumer pattern for
//!   per-section scroll/selection state.

// Each example uses a subset of the shared items, so dead-code +
// unused-import warnings are expected and not actionable here.
#![allow(dead_code, unused_imports)]

pub mod chart_app;
pub mod data_table_app;
pub mod demo;
pub mod form_groups;
pub mod form_scroll;
pub mod hscroll_editor;
pub mod indicators_app;
pub mod menu_bar_app;
pub mod mini_app;
pub mod multi_tree;
pub mod panel_app;
pub mod search_panel;
pub mod shell_app;
pub mod sidebar_search;
pub mod split_app;
pub mod toast_app;

pub use chart_app::ChartApp;
pub use data_table_app::DataTableApp;
pub use demo::AppState;
pub use form_groups::FormGroupsApp;
pub use form_scroll::FormScrollApp;
pub use hscroll_editor::HScrollEditor;
pub use indicators_app::IndicatorsApp;
pub use menu_bar_app::MenuBarApp;
pub use mini_app::MiniApp;
pub use multi_tree::DebugSidebar;
pub use panel_app::PanelApp;
pub use search_panel::SearchPanelApp;
pub use shell_app::ShellApp;
pub use sidebar_search::SidebarSearchApp;
pub use split_app::SplitApp;
pub use toast_app::ToastApp;
