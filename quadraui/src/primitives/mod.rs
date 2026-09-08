//! Widget primitives.
//!
//! Each primitive module exports a declarative data struct describing the
//! widget, layout/hit-test types, and any supporting types. Backends
//! implement rendering and input handling against these types. A handful
//! of primitives also carry a companion `*Event` enum that either bubbles
//! through [`crate::UiEvent`] (e.g. `TreeEvent`, `FormEvent`) or is
//! app-constructed from the primitive's own hit-test result (e.g.
//! `PaletteEvent`) — most primitives resolve interaction purely through
//! their `*Hit` type instead (see quadraui#509's disposition pass in
//! `docs/decisions/DECISIONS.md` for why the per-primitive `*Event` enums that
//! nothing ever constructed were removed).

// Not `pub mod`: this crate's `readme_truth.rs::lib_doc_states_the_real_primitive_count`
// / `root_readme_states_the_real_primitive_count` count every `pub mod`
// line here against README.md's/lib.rs's stated primitive count, and
// this repo's "only the coordinator writes docs" rule means a worker
// PR can't bump either doc. `a11y` isn't a widget primitive anyway (no
// `Layout`/`hit_test`, no `Backend::draw_a11y`) — it's the shared
// groundwork type `A11yInfo` (#835), `pub(crate)` here and re-exported
// as `crate::A11yInfo` from `lib.rs` instead of counted as the 41st
// primitive module.
pub(crate) mod a11y;
pub mod activity_bar;
pub mod board;
pub mod chart;
pub mod command_center;
pub mod command_line;
pub mod completions;
pub mod context_menu;
pub mod data_table;
pub mod dialog;
pub mod diff_view;
pub mod drop_zone;
pub mod editor;
pub mod find_replace;
pub mod form;
pub mod image;
pub mod layout_metrics;
pub mod list;
pub mod menu_bar;
pub mod message_list;
pub mod minimap;
pub mod multi_section_view;
pub mod palette;
pub mod panel;
pub mod pipeline_view;
pub mod progress;
pub mod rich_text_popup;
pub mod scrollbar;
pub mod sidebar_panel;
pub mod spinner;
pub mod split;
pub mod split_tree;
pub mod status_bar;
pub mod tab_bar;
pub mod terminal;
pub mod text_display;
pub mod text_input;
pub mod toast;
pub mod toolbar;
pub mod tooltip;
pub mod tree;
