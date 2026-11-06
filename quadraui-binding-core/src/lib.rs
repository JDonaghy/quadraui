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
#![allow(rustdoc::private_intra_doc_links)]

pub mod drag;
pub mod engine;
pub mod extra_paint;
pub mod hit;
pub mod host;
pub mod layout;
pub mod render;
pub mod semantic;
pub mod text_input;
pub mod tree_edit;
pub mod wire;

/// A wire-decode failure from any decoder in [`wire`] or
/// [`extra_paint::decode_extra_primitive`] — the core counterpart of a
/// binding's own descriptor-decode exception (`quadraui-py`'s
/// `InvalidDescriptorError`). Carries a human-readable `"{what}: ..."`
/// message in the same shape every decoder in this crate produces, so a
/// binding can surface it unchanged (just mapped to its own exception
/// type) rather than reformatting it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodeError(pub String);

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for DecodeError {}

/// Deserializes `deserializer` into `T`, wrapping any failure as a
/// [`DecodeError`] whose message names `what` — the shared tail every
/// per-kind decoder in this crate (`wire::decode_layout_node`,
/// `extra_paint::decode_extra_primitive`, ...) reduces to once it has a
/// concrete type and a `serde::Deserializer` in hand.
pub(crate) fn decode_value<'de, T, D>(what: &str, deserializer: D) -> Result<T, DecodeError>
where
    T: serde::Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    T::deserialize(deserializer)
        .map_err(|err| DecodeError(format!("{what}: invalid descriptor: {err}")))
}

pub use drag::{find_split_layout, tab_bar_for_right_segment, tree_row_path, tree_scroll_state};
pub use drag::{ActiveDrag, ScrollbarDrag};
pub use engine::{message_dialog_options_from_buttons, EventSink, InteractionEngine};
pub use extra_paint::{decode_extra_primitive, extra_as, ExtraPaint, EXTRA_PRIMITIVE_KINDS};
pub use hit::{event_position, hit_test_painted, set_event_widget};
pub use hit::{HitDetail, PaintedHit};
#[cfg(any(feature = "gtk", feature = "macos", feature = "win"))]
pub use host::NativeExitPolicy;
pub use host::{
    callback_reaction, callback_reaction_for_render, caret_shape, click_down_up_events,
    color_depth_name, dialog_severity, font_role, key_pressed_event, merge_overrides,
    panic_message, pointer_shape, render_overlays, resize_edge, step_driver,
    text_display_from_lines, theme_color_value, toast_corner, window_close_reaction, BindingHost,
    ErasedShellDriver, OverrideError, WindowCloseHandling, SLOT_BUTTON, SLOT_DATA_TABLE, SLOT_LIST,
    SLOT_TAB_BAR, SLOT_TEXT_DISPLAY, SLOT_TREE,
};
pub use layout::{validate_unique_leaf_ids, EdgeInsets, LayoutContainer, LayoutLeaf, LayoutNode};
pub use layout::{LayoutPanel, LayoutSidebarPanel, LayoutSplit, LayoutSplitTree, LayoutTabEntry};
pub use layout::{LayoutTabs, LeafDescriptor, Spacing, LEAF_KIND_KEYS};
pub use render::{draw_bands, draw_tree_with_scrollbar, paint_layout, paint_leaf, split_rects};
pub use render::{split_rects_with_gap, tree_viewport_rows, RenderFrame, RenderRefs};
pub use semantic::{focus_scoped_key_event, key_to_focus_string, semantic_event_for_hit};
pub use text_input::{apply_text_input_edit, clamp_cursor, text_input_click_event};
pub use text_input::{text_input_insert_event, text_input_key_event, TextEditorCache};
pub use tree_edit::tree_row_rename_key_event;
pub use wire::{
    decode_layout_node, decode_render_output, decode_shell_config, RenderOutput, StaticTabContent,
    LAYOUT_NODE_KEYS,
};

/// Lift an optional `"scroll_px"` number out of a `list`/`tree`/
/// `data_table` leaf's own descriptor dict (the nested value under its
/// `LEAF_KIND_KEYS` key — e.g. the `{...}` in `{"id": ..., "list":
/// {...}}`) into the paint-time `scroll_px` a binding passes to a
/// `ListPaintState`/`TreePaintState`/`DataTablePaintState` builder.
///
/// `scroll_px` is **not** a field [`wire::decode_layout_node`] (or any
/// `ListView`/`TreeView`/`DataTable` decoder) reads — those types have
/// no such field, by design (see `quadraui`'s `CHANGELOG.md` "A3c-seam"
/// entry). This function instead reads the key directly out of the raw
/// `serde_json::Value` a binding already holds before/alongside
/// decoding, so a descriptor dict that sets `"scroll_px"` doesn't need
/// a Rust struct field to carry it, and a dict that never sets it is
/// completely unaffected (`None`, same as if the key didn't exist).
///
/// Returns `None` when the key is absent, not a JSON number, or the
/// value isn't a plain object — never an error: a malformed or missing
/// `scroll_px` degrades to the pre-existing whole-row behaviour rather
/// than failing the whole leaf's decode.
pub fn scroll_px_from_value(value: &serde_json::Value) -> Option<f32> {
    value
        .as_object()?
        .get("scroll_px")?
        .as_f64()
        .map(|v| v as f32)
}

#[cfg(test)]
mod scroll_px_tests {
    use super::scroll_px_from_value;
    use serde_json::json;

    #[test]
    fn present_numeric_scroll_px_is_lifted_out() {
        let dict = json!({"scroll_offset": 0, "scroll_px": 12.5});
        assert_eq!(scroll_px_from_value(&dict), Some(12.5));
    }

    #[test]
    fn absent_scroll_px_is_none_and_leaves_the_dict_unaffected() {
        let dict = json!({"scroll_offset": 3});
        assert_eq!(scroll_px_from_value(&dict), None);
    }

    #[test]
    fn non_numeric_scroll_px_is_none_not_an_error() {
        let dict = json!({"scroll_px": "not a number"});
        assert_eq!(scroll_px_from_value(&dict), None);
    }

    #[test]
    fn non_object_value_is_none() {
        assert_eq!(scroll_px_from_value(&json!([1, 2, 3])), None);
    }
}
