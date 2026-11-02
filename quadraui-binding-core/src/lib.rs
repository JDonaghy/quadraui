//! Language-neutral interaction wiring shared by every native `quadraui`
//! binding: hit routing for the fixed and additive primitives,
//! [`semantic::semantic_event_for_hit`] synthesis, the text-input/focus/
//! drag controllers, the retained [`layout::LayoutNode`] descriptor model,
//! and the render -> frame assembly ([`render::draw_bands`]/
//! [`render::paint_layout`]).
//!
//! This crate carries zero dependency on any host-language FFI layer. A
//! binding crate (`quadraui-py` today, a further native binding next)
//! decodes its own wire format (a Python `dict`, a JS object, ...) into
//! the descriptor types this crate defines
//! ([`layout::LayoutNode`], [`render::RenderFrame`]), drives this crate's
//! `event -> hit -> semantic -> render` engine with real Rust types, and
//! supplies only a callback boundary back into user code — every binding
//! inherits this engine's behavior (including its bug fixes) for free.
#![allow(rustdoc::broken_intra_doc_links, rustdoc::private_intra_doc_links)]

pub mod drag;
pub mod extra_paint;
pub mod hit;
pub mod layout;
pub mod render;
pub mod semantic;
pub mod text_input;

pub use drag::{find_split_layout, tab_bar_for_right_segment, tree_row_path, tree_scroll_state};
pub use drag::{ActiveDrag, ScrollbarDrag};
pub use extra_paint::{extra_as, ExtraPaint, EXTRA_PRIMITIVE_KINDS};
pub use hit::{event_position, hit_test_painted, set_event_widget};
pub use hit::{HitDetail, PaintedHit};
pub use layout::{LayoutLeaf, LayoutNode, LayoutPanel, LayoutSidebarPanel, LayoutSplit};
pub use layout::{LayoutSplitTree, LayoutTabEntry, LayoutTabs, LeafDescriptor, LEAF_KIND_KEYS};
pub use render::{draw_bands, draw_tree_with_scrollbar, paint_layout, paint_leaf, split_rects};
pub use render::{tree_viewport_rows, RenderFrame, RenderRefs};
pub use semantic::{focus_scoped_key_event, key_to_focus_string, semantic_event_for_hit};
pub use text_input::{apply_text_input_edit, clamp_cursor, text_input_insert_event};
pub use text_input::{text_input_key_event, TextCursorState};

#[cfg(test)]
mod tests;
