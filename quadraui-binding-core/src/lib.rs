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
pub mod services;
pub mod text_input;
pub mod tree_edit;
pub mod widget_state;
pub mod wire;
pub mod wire_schema;

/// A wire-decode failure from any decoder in [`wire`] or
/// [`extra_paint::decode_extra_primitive`] — the core counterpart of a
/// binding's own descriptor-decode exception taxonomy (`quadraui-py`'s
/// `InvalidDescriptorError` and its per-variant subclasses). `#[non_exhaustive]`
/// so a binding match always carries a wildcard arm, and so this crate can add
/// a finer-grained variant later without that being a breaking change.
///
/// Every variant carries the dotted path into the layout tree at which the
/// problem was found — [`Self::path`] returns it verbatim (e.g.
/// `"render › split#body › panel#chrome"`), and [`Display`](std::fmt::Display)
/// renders the full `"<root>: at <rest>: <problem>"` message a binding
/// surfaces unchanged (just mapped to its own exception type) — so an author
/// reading a nested-tree error sees which widget it happened on instead of
/// grepping the whole descriptor for a bare field name. A binding picks
/// its own exception subclass either by `match`ing this enum directly
/// (`quadraui-py`) or, where no such match is available, by reading
/// [`Self::code`]'s short, stable, machine-matchable string tag instead
/// (`quadraui-node`'s napi-rs boundary — see that method's own doc).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    /// A dict at `path` had no `field` key, and this decoder has no
    /// default for it.
    MissingField { path: String, field: String },
    /// `field`'s value at `path` was a recognized shape (a string, most
    /// often) but not one of the specific values this decoder accepts.
    UnknownVariant {
        path: String,
        field: String,
        got: String,
        expected: Vec<String>,
    },
    /// `field`'s value at `path` was not the JSON shape this decoder
    /// expects (e.g. a `"ratio"` that isn't a number).
    TypeMismatch {
        path: String,
        field: String,
        expected: String,
    },
    /// A dict at `path` carried `key`, which isn't one of the keys this
    /// decoder recognizes there.
    UnknownKey {
        path: String,
        field: String,
        key: String,
        expected: Vec<String>,
    },
    /// A `"wire_version"` at `path` is newer than `max`, the highest this
    /// host understands.
    VersionMismatch { path: String, got: u64, max: u64 },
    /// A failure this crate didn't itself diagnose as one of the variants
    /// above — a lower-level `serde`/`serde_json` deserialize error handed
    /// back verbatim, or a bespoke cross-field validation this decoder
    /// performs itself (a duplicate leaf id, an out-of-range `"ratio"`).
    /// The catch-all every other variant exists to be a more specific
    /// alternative to.
    Borrow { path: String, message: String },
}

impl DecodeError {
    /// Builds a [`DecodeError::MissingField`]: `field` is required at
    /// `path` and this decoder has no default for it.
    pub fn missing_field(path: impl Into<String>, field: impl Into<String>) -> Self {
        DecodeError::MissingField {
            path: path.into(),
            field: field.into(),
        }
    }

    /// Builds a [`DecodeError::UnknownVariant`]: `field`'s value at `path`
    /// was recognized shape-wise but `got` isn't one of `expected`.
    pub fn unknown_variant(
        path: impl Into<String>,
        field: impl Into<String>,
        got: impl Into<String>,
        expected: &[&str],
    ) -> Self {
        DecodeError::UnknownVariant {
            path: path.into(),
            field: field.into(),
            got: got.into(),
            expected: expected.iter().map(|s| s.to_string()).collect(),
        }
    }

    /// Builds a [`DecodeError::TypeMismatch`]: `field`'s value at `path`
    /// was not the JSON shape `expected` describes.
    pub fn type_mismatch(
        path: impl Into<String>,
        field: impl Into<String>,
        expected: impl Into<String>,
    ) -> Self {
        DecodeError::TypeMismatch {
            path: path.into(),
            field: field.into(),
            expected: expected.into(),
        }
    }

    /// Builds a [`DecodeError::UnknownKey`]: the dict `field` names at
    /// `path` carried `key`, which isn't one of `expected`.
    pub fn unknown_key(
        path: impl Into<String>,
        field: impl Into<String>,
        key: impl Into<String>,
        expected: &[&str],
    ) -> Self {
        DecodeError::UnknownKey {
            path: path.into(),
            field: field.into(),
            key: key.into(),
            expected: expected.iter().map(|s| s.to_string()).collect(),
        }
    }

    /// Builds a [`DecodeError::VersionMismatch`]: the `"wire_version"` at
    /// `path` was `got`, newer than `max`, the highest this host
    /// understands.
    pub fn version_mismatch(path: impl Into<String>, got: u64, max: u64) -> Self {
        DecodeError::VersionMismatch {
            path: path.into(),
            got,
            max,
        }
    }

    /// Builds a [`DecodeError::Borrow`]: the catch-all for a decode
    /// failure no more specific constructor above fits -- a lower-level
    /// `serde`/`serde_json` error handed back verbatim, or a bespoke
    /// cross-field validation this crate performs itself, at `path`,
    /// described by `message`.
    pub fn borrow(path: impl Into<String>, message: impl Into<String>) -> Self {
        DecodeError::Borrow {
            path: path.into(),
            message: message.into(),
        }
    }

    /// The ` › `-joined path into the layout tree this error occurred at,
    /// e.g. `"render › split#body › panel#chrome"` — the whole string,
    /// including the leading root label. [`Display`](std::fmt::Display)
    /// splits this on its first ` › ` to render `"<root>: at <rest>:
    /// <problem>"`, with `<rest>` being everything after that split, not
    /// this method's own return value.
    pub fn path(&self) -> &str {
        match self {
            DecodeError::MissingField { path, .. }
            | DecodeError::UnknownVariant { path, .. }
            | DecodeError::TypeMismatch { path, .. }
            | DecodeError::UnknownKey { path, .. }
            | DecodeError::VersionMismatch { path, .. }
            | DecodeError::Borrow { path, .. } => path,
        }
    }

    /// A short, stable, snake_case tag naming which variant this is —
    /// e.g. `"missing_field"` — for a binding that wants to pick its own
    /// exception subclass by string rather than matching this enum
    /// directly (`quadraui-node`'s napi-rs boundary has no custom
    /// exception-class mechanism, so it tags a thrown `Error`'s message
    /// with this and reconstructs the typed subclass on the JS side).
    pub fn code(&self) -> &'static str {
        match self {
            DecodeError::MissingField { .. } => "missing_field",
            DecodeError::UnknownVariant { .. } => "unknown_variant",
            DecodeError::TypeMismatch { .. } => "type_mismatch",
            DecodeError::UnknownKey { .. } => "unknown_key",
            DecodeError::VersionMismatch { .. } => "version_mismatch",
            DecodeError::Borrow { .. } => "borrow",
        }
    }

    /// The problem description [`Display`](std::fmt::Display) renders
    /// after `"at <path>: "` — everything this variant carries besides
    /// `path` itself, folded into one human-readable clause.
    fn problem(&self) -> String {
        match self {
            DecodeError::MissingField { field, .. } => format!("missing field {field:?}"),
            DecodeError::UnknownVariant {
                field,
                got,
                expected,
                ..
            } => {
                format!("{field:?} must be one of {expected:?}, got {got:?}")
            }
            DecodeError::TypeMismatch {
                field, expected, ..
            } => {
                format!("{field:?} must be {expected}")
            }
            DecodeError::UnknownKey {
                field,
                key,
                expected,
                ..
            } => {
                format!("{field:?} has unknown key {key:?} -- expected one of {expected:?}")
            }
            DecodeError::VersionMismatch { got, max, .. } => {
                format!(
                    "\"wire_version\" {got} is newer than this host understands \
                     (max {max}) — upgrade the host, or downgrade the binding"
                )
            }
            DecodeError::Borrow { message, .. } => message.clone(),
        }
    }
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let problem = self.problem();
        match self.path().split_once(" › ") {
            Some((root, rest)) => write!(f, "{root}: at {rest}: {problem}"),
            None => write!(f, "{}: {problem}", self.path()),
        }
    }
}

impl std::error::Error for DecodeError {}

/// Deserializes `deserializer` into `T`, wrapping any failure as a
/// [`DecodeError::Borrow`] whose path is `what` — the shared tail every
/// per-kind decoder in this crate (`wire::decode_layout_node`,
/// `extra_paint::decode_extra_primitive`, ...) reduces to once it has a
/// concrete type and a `serde::Deserializer` in hand. Public so a binding
/// crate with its own fixed-slot descriptor types (`quadraui-ffi`'s
/// `qui_<kind>_insert` family) can decode straight into them with the same
/// error shape every decoder in this module already uses, instead of
/// calling `serde_json`/`serde` directly and inventing its own.
pub fn decode_value<'de, T, D>(what: &str, deserializer: D) -> Result<T, DecodeError>
where
    T: serde::Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    T::deserialize(deserializer)
        .map_err(|err| DecodeError::borrow(what, format!("invalid descriptor: {err}")))
}

pub use drag::{find_split_layout, tab_bar_for_right_segment, tree_row_path, tree_scroll_state};
pub use drag::{ActiveDrag, ScrollbarDrag};
pub use engine::{
    message_dialog_options_from_buttons, DialogContentSize, EventSink, InteractionEngine,
};
pub use extra_paint::{decode_extra_primitive, extra_as, ExtraPaint, EXTRA_PRIMITIVE_KINDS};
pub use hit::{data_table_rows_and_viewport, list_rows_and_viewport, tree_rows_and_viewport};
pub use hit::{event_position, hit_test_painted, set_event_widget};
pub use hit::{HitDetail, PaintedHit, ScrollChildHit};
#[cfg(any(feature = "gtk", feature = "macos", feature = "win"))]
pub use host::NativeExitPolicy;
pub use host::{
    app_path_kind, apply_data_table_nav_key, apply_list_nav_key, apply_split_divider_drag,
    apply_tab_activated, apply_tab_closed, apply_tree_nav_key, apply_tree_toggle_expand,
    callback_reaction, callback_reaction_for_render, caret_shape, click_down_up_events,
    color_depth_name, dialog_severity, font_role, guarded, key_pressed_event, menu_for_surface,
    merge_overrides, panic_message, pointer_shape, power_save_kind, record_pending_error,
    reentrant_call_message, render_overlays, resize_edge, resolve_data_table_cursor,
    resolve_layout_node, resolve_layout_tabs_active, resolve_list_cursor, resolve_render_refs,
    resolve_scroll_offset, resolve_split_ratio, resolve_tab_bar_active, resolve_tree_cursor,
    resolve_tree_expansion, step_driver, take_pending_error, text_display_from_lines,
    theme_color_value, toast_corner, widget_state_snapshot, window_close_reaction, with_tray,
    with_window, BindingHost, ErasedShellDriver, GuardedFailure, OverrideError,
    WidgetStateSnapshot, WindowCloseHandling, SLOT_BUTTON, SLOT_DATA_TABLE, SLOT_LIST,
    SLOT_TAB_BAR, SLOT_TEXT_DISPLAY, SLOT_TREE,
};
pub use layout::{validate_unique_leaf_ids, EdgeInsets, LayoutContainer, LayoutLeaf, LayoutNode};
pub use layout::{LayoutPanel, LayoutScroll, LayoutSidebarPanel, LayoutSplit, LayoutSplitTree};
pub use layout::{LayoutTabEntry, LayoutTabs, LeafDescriptor, Spacing, LEAF_KIND_KEYS};
pub use layout::{StyleOverride, StyleSets, BUILTIN_STYLE_SET_NAMES};
pub use render::{
    collect_leaf_tooltips, draw_bands, draw_tree_with_scrollbar, paint_layout, paint_leaf,
    split_rects,
};
pub use render::{
    data_table_viewport_rows, list_viewport_rows, split_rects_with_gap, tree_viewport_rows,
    RenderFrame, RenderRefs,
};
pub use semantic::{focus_scoped_key_event, key_to_focus_string, semantic_event_for_hit};
pub use services::{
    add_recent_document, app_identifier, begin_drag_out, begin_power_save_block,
    clear_recent_documents, end_power_save_block, install_crash_handler,
    is_default_protocol_client, login_item_enabled, remove_as_default_protocol_client,
    resolve_app_path, set_app_identifier, set_as_default_protocol_client, set_dock_menu,
    set_jump_list, set_login_item, update_apply_on_quit, update_check_for_update,
    update_download_and_stage, ServiceError, UpdateInfoWire,
};
pub use text_input::{apply_text_input_edit, clamp_cursor, text_input_click_event};
pub use text_input::{text_input_insert_event, text_input_key_event, TextEditorCache};
pub use tree_edit::tree_row_rename_key_event;
pub use widget_state::WidgetState;
pub use wire::{
    decode_layout_dialog, decode_layout_node, decode_layout_node_with_styles, decode_popover,
    decode_render_output, decode_render_output_with_styles, decode_shell_config,
    decode_style_overrides, validate_style_set_name, OverlayContent, RenderOutput,
    StaticTabContent, StyleContext, LAYOUT_NODE_KEYS,
};

/// Lift an optional `"scroll_px"` number out of a `list`/`tree`/
/// `data_table` leaf's own descriptor dict (the nested value under its
/// `LEAF_KIND_KEYS` key — e.g. the `{...}` in `{"id": ..., "list":
/// {...}}`) into the paint-time `scroll_px` a binding passes to a
/// `ListPaintState`/`TreePaintState`/`DataTablePaintState` builder.
///
/// `scroll_px` is **not** a field [`wire::decode_layout_node`] (or any
/// `ListView`/`TreeView`/`DataTable` decoder) reads — those types have
/// no such field, by design (see `quadraui`'s `CHANGELOG.md`
/// `[Unreleased]` → `Added` entry for `ListPaintState`/`TreePaintState`/
/// `DataTablePaintState`). This function instead reads the key directly
/// out of the raw `serde_json::Value` a binding already holds
/// before/alongside decoding, so a descriptor dict that sets
/// `"scroll_px"` doesn't need a Rust struct field to carry it, and a
/// dict that never sets it is completely unaffected (`None`, same as
/// if the key didn't exist).
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
