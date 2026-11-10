//! The canonical wire-type schema for every layout-tree node, leaf
//! descriptor, and `UiEvent`/activation shape a binding's wire format
//! accepts or emits — the single Rust-side source of truth
//! [`scripts/gen_wire_types.py`] reads (via [`crate::bin::export_wire_schema`]'s
//! JSON dump) to produce `quadraui-py/python/quadraui/wire.pyi` and
//! `quadraui-node/js/wire.d.ts`.
//!
//! [`schema()`] builds the whole schema as a tree of [`TyJson`]/[`FieldJson`]/
//! [`DefJson`] values (deliberately an abstract type-expression, not a
//! pre-rendered string — the parenthesization/narrowing rules for each
//! target language stay in `gen_wire_types.py`'s renderer, which this
//! module has no business duplicating). The functions below mirror that
//! script's own `REF`/`NULLABLE`/`LIST`/`flat`/`union_of`/`define`
//! vocabulary one-for-one, so a schema entry here and the matching Python
//! renderer logic stay easy to cross-reference.
//!
//! # Why this is the source of truth, not a transcription of it
//!
//! Every struct/enum below that corresponds to a real `quadraui`/
//! `quadraui-binding-core` Rust type (not an invented wire-only dict shape
//! like a layout-tree node) is paired with an `assert_fields!`/
//! `assert_variants!` call in [`coverage`] — an exhaustive destructure or
//! match, with no `..`/wildcard arm, over that real type. Adding a field to
//! one of those structs, or a variant to one of those enums, without also
//! updating the matching entry here makes the destructure/match
//! non-exhaustive and fails `cargo build`/`cargo check` outright — the
//! schema can't silently drift out of sync with the Rust types it
//! describes the way a hand-copied Python transcription could.
//!
//! The layout-tree node shapes (`"row"`/`"column"`/`"split"`/...) have no
//! single Rust type to destructure this way — they're dynamically-keyed
//! dicts `crate::wire` decodes field-by-field, not a `#[derive(Deserialize)]`
//! struct (see that module's own doc). Their dispatch keys are still pulled
//! straight from [`crate::layout::LEAF_KIND_KEYS`] and
//! [`crate::extra_paint::EXTRA_PRIMITIVE_KINDS`] below rather than
//! re-typed, so a new leaf kind or additive primitive always gets a schema
//! entry (even if, for the 28 additive kinds beyond `status_bar`, only an
//! untyped placeholder one — narrowing those field-by-field is tracked
//! follow-on work, not done here).

use serde::Serialize;

use crate::extra_paint::EXTRA_PRIMITIVE_KINDS;
use crate::layout::LEAF_KIND_KEYS;

// ─── A tiny two-target type expression, mirroring `gen_wire_types.py`'s `Ty` ──

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TyJson {
    /// One of `gen_wire_types.py`'s `STR`/`INT`/`FLOAT`/`BOOL`/`ANY` primitives,
    /// named by that constant's own identifier.
    Prim {
        name: String,
    },
    /// A forward/backward reference to another top-level [`DefJson`] by name.
    Ref {
        name: String,
    },
    /// `Optional[T]` / `T | null` — the key is still required; see
    /// `gen_wire_types.py`'s `NULLABLE` doc for why this differs from
    /// [`FieldJson::optional`].
    Nullable {
        inner: Box<TyJson>,
    },
    List {
        inner: Box<TyJson>,
    },
    Dict {
        key: Box<TyJson>,
        value: Box<TyJson>,
    },
    Tuple {
        items: Vec<TyJson>,
    },
    /// A set of string literals (`Literal[...]` / a `|`-joined set of
    /// string literal types).
    Lit {
        values: Vec<String>,
    },
    Union {
        items: Vec<TyJson>,
    },
}

pub fn str_() -> TyJson {
    TyJson::Prim { name: "STR".into() }
}
pub fn int_() -> TyJson {
    TyJson::Prim { name: "INT".into() }
}
pub fn float_() -> TyJson {
    TyJson::Prim {
        name: "FLOAT".into(),
    }
}
pub fn bool_() -> TyJson {
    TyJson::Prim {
        name: "BOOL".into(),
    }
}
pub fn any_() -> TyJson {
    TyJson::Prim { name: "ANY".into() }
}
pub fn re(name: &str) -> TyJson {
    TyJson::Ref { name: name.into() }
}
pub fn nullable(inner: TyJson) -> TyJson {
    TyJson::Nullable {
        inner: Box::new(inner),
    }
}
pub fn list(inner: TyJson) -> TyJson {
    TyJson::List {
        inner: Box::new(inner),
    }
}
pub fn dictt(key: TyJson, value: TyJson) -> TyJson {
    TyJson::Dict {
        key: Box::new(key),
        value: Box::new(value),
    }
}
pub fn tup(items: Vec<TyJson>) -> TyJson {
    TyJson::Tuple { items }
}
pub fn lit(values: &[&str]) -> TyJson {
    TyJson::Lit {
        values: values.iter().map(|s| s.to_string()).collect(),
    }
}
pub fn uniont(items: Vec<TyJson>) -> TyJson {
    TyJson::Union { items }
}

// ─── Fields + definitions ───────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct FieldJson {
    pub name: String,
    pub ty: TyJson,
    /// Key may be omitted (`#[serde(default...)]` on the real Rust field).
    #[serde(default)]
    pub optional: bool,
    #[serde(default)]
    pub doc: String,
}

/// A required field with no doc comment -- the common case.
pub fn field(name: &str, ty: TyJson) -> FieldJson {
    FieldJson {
        name: name.into(),
        ty,
        optional: false,
        doc: String::new(),
    }
}

/// [`field`], but the key may be omitted on the wire.
pub fn field_opt(name: &str, ty: TyJson) -> FieldJson {
    FieldJson {
        name: name.into(),
        ty,
        optional: true,
        doc: String::new(),
    }
}

/// Attaches a doc comment to an already-built [`FieldJson`].
pub fn with_doc(mut f: FieldJson, doc: &str) -> FieldJson {
    f.doc = doc.to_string();
    f
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DefJson {
    Obj {
        name: String,
        fields: Vec<FieldJson>,
        #[serde(default)]
        doc: String,
        /// See `gen_wire_types.py`'s `ObjDef.discriminated` doc: a `flat()`-
        /// built discriminated union (every field optional, mutually
        /// exclusive in practice) renders as a `TypedDict` in Python but a
        /// union of single-key object types in TypeScript.
        #[serde(default)]
        discriminated: bool,
        #[serde(default)]
        common: Vec<FieldJson>,
    },
    Alias {
        name: String,
        ty: TyJson,
        #[serde(default)]
        doc: String,
    },
}

impl DefJson {
    fn name(&self) -> &str {
        match self {
            DefJson::Obj { name, .. } => name,
            DefJson::Alias { name, .. } => name,
        }
    }
}

/// An ordinary struct-shaped [`DefJson::Obj`] -- some fields required, some
/// [`field_opt`].
pub fn obj(name: &str, fields: Vec<FieldJson>) -> DefJson {
    DefJson::Obj {
        name: name.into(),
        fields,
        doc: String::new(),
        discriminated: false,
        common: Vec::new(),
    }
}

pub fn obj_doc(name: &str, fields: Vec<FieldJson>, doc: &str) -> DefJson {
    DefJson::Obj {
        name: name.into(),
        fields,
        doc: doc.to_string(),
        discriminated: false,
        common: Vec::new(),
    }
}

/// A [`DefJson::Obj`] with one optional field per `(tag, payload_type)` pair
/// -- see `gen_wire_types.py`'s `flat()` doc for when to reach for this
/// over a per-variant union. `common` is a key that rides alongside
/// whichever tag is set, not a tag of its own.
pub fn flat(name: &str, variants: &[(&str, TyJson)], common: Vec<FieldJson>) -> DefJson {
    DefJson::Obj {
        name: name.into(),
        fields: variants
            .iter()
            .map(|(tag, ty)| field_opt(tag, ty.clone()))
            .collect(),
        doc: String::new(),
        discriminated: true,
        common,
    }
}

pub fn flat_doc(
    name: &str,
    variants: &[(&str, TyJson)],
    common: Vec<FieldJson>,
    doc: &str,
) -> DefJson {
    let mut d = flat(name, variants, common);
    if let DefJson::Obj { doc: d_doc, .. } = &mut d {
        *d_doc = doc.to_string();
    }
    d
}

pub fn alias(name: &str, ty: TyJson) -> DefJson {
    DefJson::Alias {
        name: name.into(),
        ty,
        doc: String::new(),
    }
}

pub fn alias_doc(name: &str, ty: TyJson, doc: &str) -> DefJson {
    DefJson::Alias {
        name: name.into(),
        ty,
        doc: doc.to_string(),
    }
}

pub fn union_of(name: &str, members: Vec<TyJson>) -> DefJson {
    alias(name, uniont(members))
}

pub fn union_of_doc(name: &str, members: Vec<TyJson>, doc: &str) -> DefJson {
    alias_doc(name, uniont(members), doc)
}

/// Accumulates [`DefJson`]s in declaration order -- the order
/// [`schema()`] returns them in, and the order `gen_wire_types.py` renders
/// them in.
#[derive(Default)]
struct Builder {
    defs: Vec<DefJson>,
}

impl Builder {
    /// Registers `def` and returns a [`TyJson::Ref`] naming it -- the same
    /// "define and get back a reference" shape `gen_wire_types.py`'s
    /// `define()` has.
    fn define(&mut self, def: DefJson) -> TyJson {
        let name = def.name().to_string();
        self.defs.push(def);
        re(&name)
    }
}

// ─── The schema itself ──────────────────────────────────────────────────────

/// Builds the full wire-type schema, in render order. See this module's own
/// doc for the source-of-truth contract every entry here that corresponds
/// to a real Rust type carries, enforced by [`coverage`].
pub fn schema() -> Vec<DefJson> {
    let mut b = Builder::default();

    // ── Identifiers + small scalar aliases ─────────────────────────────
    b.define(alias_doc(
        "WidgetId",
        str_(),
        "A widget's own string label.",
    ));
    b.define(alias_doc(
        "TreePath",
        list(int_()),
        "Route from the tree root, as child indices.",
    ));
    b.define(alias("AcceleratorId", str_()));
    b.define(alias("GlobalShortcutId", int_()));

    // ── Shared value types ──────────────────────────────────────────────
    let color = b.define(obj_doc(
        "Color",
        vec![
            field("r", int_()),
            field("g", int_()),
            field("b", int_()),
            field("a", int_()),
        ],
        "8-bit RGBA.",
    ));
    let styled_span = b.define(obj(
        "StyledSpan",
        vec![
            field("text", str_()),
            field("fg", nullable(color.clone())),
            field("bg", nullable(color.clone())),
            field_opt("bold", bool_()),
            field_opt("italic", bool_()),
            field_opt("underline", bool_()),
        ],
    ));
    let styled_text = b.define(obj(
        "StyledText",
        vec![field("spans", list(styled_span.clone()))],
    ));
    let icon_glyph = b.define(obj_doc(
        "IconGlyph",
        vec![
            with_doc(field("glyph", str_()), "Nerd Font glyph, painted verbatim."),
            with_doc(field("fallback", str_()), "ASCII/basic-Unicode fallback."),
            field_opt("color", nullable(color.clone())),
        ],
        "The glyph form of `Icon` -- also the only shape `Icon::serialize` ever emits.",
    ));
    let icon_codicon = b.define(obj_doc(
        "IconCodicon",
        vec![
            with_doc(field("codicon", str_()), "Bundled codicon name."),
            with_doc(field("fallback", str_()), "ASCII/basic-Unicode fallback."),
            field_opt("color", nullable(color.clone())),
        ],
        "The codicon form of `Icon` -- lets an app pass a bundled icon name instead of hand-encoding its glyph codepoint.",
    ));
    let icon = b.define(union_of_doc(
        "Icon",
        vec![icon_glyph, icon_codicon],
        "An icon reference: either `{\"glyph\", \"fallback\", \"color\"?}` or `{\"codicon\", \"fallback\", \"color\"?}` -- see `Icon`'s own doc in `quadraui/src/types.rs`.",
    ));
    let badge = b.define(obj(
        "Badge",
        vec![
            field("text", str_()),
            field("fg", nullable(color.clone())),
            field("bg", nullable(color.clone())),
        ],
    ));
    let decoration = b.define(alias(
        "Decoration",
        lit(&["Normal", "Header", "Muted", "Error", "Warning", "Modified"]),
    ));
    let selection_mode = b.define(alias("SelectionMode", lit(&["None", "Single", "Multi"])));
    let tree_style = b.define(obj(
        "TreeStyle",
        vec![
            field("indent", int_()),
            field("show_chevrons", bool_()),
            field("chevron_expanded", str_()),
            field("chevron_collapsed", str_()),
            field_opt("row_height", nullable(int_())),
            field_opt("bordered", bool_()),
        ],
    ));
    let modifiers = b.define(obj(
        "Modifiers",
        vec![
            field_opt("shift", bool_()),
            field_opt("ctrl", bool_()),
            field_opt("alt", bool_()),
            field_opt("cmd", bool_()),
        ],
    ));
    let point = b.define(obj(
        "Point",
        vec![field("x", float_()), field("y", float_())],
    ));
    let scroll_delta = b.define(obj(
        "ScrollDelta",
        vec![field("x", float_()), field("y", float_())],
    ));
    let button_mask = b.define(obj(
        "ButtonMask",
        vec![
            field_opt("left", bool_()),
            field_opt("right", bool_()),
            field_opt("middle", bool_()),
        ],
    ));
    let viewport = b.define(obj(
        "Viewport",
        vec![
            field("width", float_()),
            field("height", float_()),
            field("scale", float_()),
        ],
    ));
    let system_theme = b.define(obj(
        "SystemTheme",
        vec![
            field("dark", bool_()),
            field("accent", nullable(color.clone())),
            field("high_contrast", bool_()),
            field_opt("reduce_motion", bool_()),
        ],
    ));
    let power_event_kind = b.define(alias(
        "PowerEventKind",
        lit(&[
            "Suspend",
            "Resume",
            "OnBattery",
            "OnAc",
            "LockScreen",
            "UnlockScreen",
        ]),
    ));
    let named_key_f = b.define(obj_doc(
        "_NamedKeyF",
        vec![field("F", int_())],
        "Function key 1-24.",
    ));
    let named_key = b.define(alias(
        "NamedKey",
        uniont(vec![
            lit(&[
                "Escape",
                "Tab",
                "BackTab",
                "Enter",
                "Backspace",
                "Delete",
                "Insert",
                "Home",
                "End",
                "PageUp",
                "PageDown",
                "Up",
                "Down",
                "Left",
                "Right",
                "CapsLock",
                "NumLock",
                "ScrollLock",
                "Menu",
            ]),
            named_key_f,
        ]),
    ));
    let key = b.define(flat_doc(
        "Key",
        &[("Char", str_()), ("Named", named_key)],
        vec![],
        "A raw keyboard key: `{\"Char\": \"a\"}` or `{\"Named\": \"Up\"}`/`{\"Named\": {\"F\": 2}}`.",
    ));
    let mouse_button_other = b.define(obj("_MouseButtonOther", vec![field("Other", int_())]));
    let mouse_button = b.define(alias(
        "MouseButton",
        uniont(vec![
            lit(&["Left", "Right", "Middle", "X1", "X2"]),
            mouse_button_other,
        ]),
    ));
    let backend_native_event = b.define(obj(
        "BackendNativeEvent",
        vec![
            field("backend", str_()),
            field("kind", str_()),
            field("payload", str_()),
        ],
    ));

    // ── Common "extra" primitive already typed: status_bar ─────────────
    let status_bar_segment = b.define(obj(
        "StatusBarSegment",
        vec![
            field("text", str_()),
            field("fg", color.clone()),
            field("bg", color.clone()),
            field_opt("bold", bool_()),
            field_opt("action_id", nullable(re("WidgetId"))),
        ],
    ));
    let status_bar_descriptor = b.define(obj(
        "StatusBarDescriptor",
        vec![
            field("id", re("WidgetId")),
            field("left_segments", list(status_bar_segment.clone())),
            field("right_segments", list(status_bar_segment)),
        ],
    ));

    // ── Core leaf descriptors (the six fixed primitive kinds) ───────────
    let button_variant = b.define(alias(
        "ButtonVariant",
        lit(&["Primary", "Secondary", "Icon", "Danger", "Warning"]),
    ));
    let button_icon_position = b.define(alias("ButtonIconPosition", lit(&["Leading", "Trailing"])));
    let button_descriptor = b.define(obj(
        "ButtonDescriptor",
        vec![
            field("id", re("WidgetId")),
            field("label", str_()),
            field_opt("variant", button_variant.clone()),
            field_opt("disabled", bool_()),
            field_opt("focused", bool_()),
            field_opt("tooltip", nullable(str_())),
            field_opt("icon", nullable(icon.clone())),
            field_opt("icon_position", button_icon_position.clone()),
            field_opt("icon_gap", float_()),
        ],
    ));
    let button_spec_descriptor = b.define(obj(
        "ButtonSpecDescriptor",
        vec![
            field("label", str_()),
            field_opt("variant", button_variant),
            field_opt("icon", nullable(icon.clone())),
            field_opt("icon_position", button_icon_position),
            field_opt("icon_gap", float_()),
            field_opt("tooltip", nullable(str_())),
            field_opt("disabled", bool_()),
        ],
    ));

    let text_display_line_descriptor = b.define(obj(
        "TextDisplayLineDescriptor",
        vec![
            field("spans", list(styled_span.clone())),
            field_opt("decoration", decoration.clone()),
            field_opt("timestamp", nullable(str_())),
        ],
    ));
    let text_display_descriptor = b.define(obj(
        "TextDisplayDescriptor",
        vec![
            field("id", re("WidgetId")),
            field("lines", list(text_display_line_descriptor)),
            field_opt("scroll_offset", int_()),
            field_opt("auto_scroll", bool_()),
            field_opt("max_lines", int_()),
            field_opt("has_focus", bool_()),
            field_opt("title", nullable(styled_text.clone())),
            field_opt("show_scrollbar", bool_()),
        ],
    ));

    let list_item_descriptor = b.define(obj(
        "ListItemDescriptor",
        vec![
            field("text", styled_text.clone()),
            field_opt("icon", nullable(icon.clone())),
            field_opt("detail", nullable(styled_text.clone())),
            field_opt("decoration", decoration.clone()),
        ],
    ));
    let list_view_descriptor = b.define(obj(
        "ListViewDescriptor",
        vec![
            field("id", re("WidgetId")),
            field_opt("title", nullable(styled_text.clone())),
            field("items", list(list_item_descriptor)),
            field("selected_idx", int_()),
            field_opt("scroll_offset", int_()),
            field_opt("has_focus", bool_()),
            field_opt("bordered", bool_()),
            field_opt("h_scroll", int_()),
            field_opt("max_content_width", nullable(int_())),
            field_opt("show_v_scrollbar", bool_()),
            with_doc(
                field_opt("scroll_px", float_()),
                "binding-core extension key, lifted out by `scroll_px_from_value` -- not a `ListView` field.",
            ),
        ],
    ));

    let tree_row_edit_state_descriptor = b.define(obj(
        "TreeRowEditStateDescriptor",
        vec![
            field("text", str_()),
            field("cursor", int_()),
            field("selection_anchor", nullable(int_())),
            field_opt("placeholder", nullable(str_())),
        ],
    ));
    let tree_row_descriptor = b.define(obj(
        "TreeRowDescriptor",
        vec![
            field("path", re("TreePath")),
            field("indent", int_()),
            field("icon", nullable(icon.clone())),
            field("text", styled_text.clone()),
            field("badge", nullable(badge)),
            field("is_expanded", nullable(bool_())),
            field_opt("decoration", decoration.clone()),
            field_opt("edit", nullable(tree_row_edit_state_descriptor)),
        ],
    ));
    let tree_view_descriptor = b.define(obj(
        "TreeViewDescriptor",
        vec![
            field("id", re("WidgetId")),
            field("rows", list(tree_row_descriptor)),
            field("selection_mode", selection_mode),
            field("selected_path", nullable(re("TreePath"))),
            field_opt("scroll_offset", int_()),
            field("style", tree_style),
            field_opt("has_focus", bool_()),
            field_opt("scroll_px", float_()),
        ],
    ));

    let column_align = b.define(alias("ColumnAlign", lit(&["Left", "Center", "Right"])));
    let sort_direction = b.define(alias("SortDirection", lit(&["Ascending", "Descending"])));
    let column_width_content = b.define(obj(
        "_ColumnWidthContent",
        vec![field("min", float_()), field("max", float_())],
    ));
    let column_width = b.define(flat_doc(
        "ColumnWidth",
        &[
            ("FixedCells", float_()),
            ("FixedPx", float_()),
            ("Flex", float_()),
            ("Content", column_width_content),
        ],
        vec![],
        "Column sizing strategy, e.g. `{\"Flex\": 2.0}` -- the deprecated `\"Fixed\"` variant is omitted here; use `FixedCells`/`FixedPx` instead.",
    ));
    let column_descriptor = b.define(obj(
        "ColumnDescriptor",
        vec![
            field("title", str_()),
            field_opt("width", column_width),
            field_opt("align", column_align),
        ],
    ));
    let data_row_descriptor = b.define(obj(
        "DataRowDescriptor",
        vec![
            field("cells", list(styled_text.clone())),
            field_opt("decoration", decoration.clone()),
        ],
    ));
    let data_table_descriptor = b.define(obj(
        "DataTableDescriptor",
        vec![
            field("id", re("WidgetId")),
            field("columns", list(column_descriptor)),
            field("rows", list(data_row_descriptor.clone())),
            field_opt("selected_idx", nullable(int_())),
            field_opt("scroll_offset", int_()),
            field_opt("sort", nullable(tup(vec![int_(), sort_direction]))),
            field_opt("has_focus", bool_()),
            field_opt("show_scrollbar", bool_()),
            field_opt("min_total_width", nullable(float_())),
            field_opt("h_scroll", float_()),
            field_opt("column_overrides", list(nullable(float_()))),
            field_opt("footer", nullable(data_row_descriptor)),
            field_opt("scroll_px", float_()),
        ],
    ));

    let tab_item_descriptor = b.define(obj_doc(
        "TabItemDescriptor",
        vec![
            field("label", str_()),
            field_opt("is_active", bool_()),
            field_opt("is_dirty", bool_()),
            field_opt("is_preview", bool_()),
            field_opt("is_closable", bool_()),
        ],
        "The raw `tab_bar` leaf's own tab shape -- distinct from a `\"tabs\"` layout node's tab entry (`LayoutTabEntryNode`), which uses `\"title\"`/`\"closable\"` instead of `\"label\"`/`\"is_closable\"`.",
    ));
    let tab_bar_segment_descriptor = b.define(obj(
        "TabBarSegmentDescriptor",
        vec![
            field("text", str_()),
            field("width_cells", int_()),
            field_opt("id", nullable(re("WidgetId"))),
            field_opt("is_active", bool_()),
        ],
    ));
    let tab_bar_descriptor = b.define(obj(
        "TabBarDescriptor",
        vec![
            field("id", re("WidgetId")),
            field("tabs", list(tab_item_descriptor)),
            field_opt("scroll_offset", int_()),
            field_opt("right_segments", list(tab_bar_segment_descriptor.clone())),
            field_opt("active_accent", nullable(color.clone())),
            field_opt("show_tab_close", bool_()),
            field_opt("compact", bool_()),
        ],
    ));

    // ── Chrome descriptors (toolbar / panel / sidebar_panel) ────────────
    let toolbar_button_action = b.define(obj(
        "_ToolbarButtonAction",
        vec![
            field("id", re("WidgetId")),
            field("label", str_()),
            field_opt("icon", nullable(str_())),
            field_opt("key_hint", nullable(str_())),
            field_opt("enabled", bool_()),
            field_opt("is_active", bool_()),
            field_opt("tooltip", str_()),
            field_opt("spec", nullable(button_spec_descriptor)),
        ],
    ));
    let toolbar_button_action_wrapper = b.define(obj(
        "_ToolbarButtonActionWrapper",
        vec![field("Action", toolbar_button_action)],
    ));
    let toolbar_button_label = b.define(obj(
        "_ToolbarButtonLabel",
        vec![
            field("text", str_()),
            field_opt("fg", nullable(color.clone())),
        ],
    ));
    let toolbar_button_label_wrapper = b.define(obj(
        "_ToolbarButtonLabelWrapper",
        vec![field("Label", toolbar_button_label)],
    ));
    let toolbar_button_descriptor = b.define(union_of_doc(
        "ToolbarButtonDescriptor",
        vec![
            lit(&["Separator", "Spacer"]),
            toolbar_button_action_wrapper,
            toolbar_button_label_wrapper,
        ],
        "One `Toolbar::buttons` entry: `\"Separator\"`/`\"Spacer\"` (bare strings), `{\"Action\": {...}}`, or `{\"Label\": {...}}`.",
    ));
    let toolbar_descriptor = b.define(obj(
        "ToolbarDescriptor",
        vec![
            field("id", re("WidgetId")),
            field("buttons", list(toolbar_button_descriptor)),
            field_opt("bg", nullable(color.clone())),
            field_opt("focused_index", nullable(int_())),
        ],
    ));
    b.define(obj(
        "PanelActionDescriptor",
        vec![
            field("id", re("WidgetId")),
            field("icon", str_()),
            field_opt("tooltip", str_()),
            field_opt("is_active", bool_()),
        ],
    ));

    // ── Layout tree node wire shapes ─────────────────────────────────────
    let padding_axis = b.define(obj(
        "PaddingAxis",
        vec![field_opt("x", float_()), field_opt("y", float_())],
    ));
    let padding_sides = b.define(obj(
        "PaddingSides",
        vec![
            field_opt("top", float_()),
            field_opt("right", float_()),
            field_opt("bottom", float_()),
            field_opt("left", float_()),
        ],
    ));
    let padding_value = b.define(alias_doc(
        "PaddingValue",
        uniont(vec![float_(), padding_axis, padding_sides]),
        "A `\"padding\"` spacing value: one number (every side), `{\"x\", \"y\"}`, or `{\"top\", \"right\", \"bottom\", \"left\"}` -- never both shapes at once.",
    ));

    let layout_node = re("LayoutNode");

    let layout_row_node = b.define(obj(
        "LayoutRowNode",
        vec![
            field("row", list(layout_node.clone())),
            field_opt("padding", padding_value.clone()),
            field_opt("margin", float_()),
            field_opt("gap", float_()),
        ],
    ));
    let layout_column_node = b.define(obj(
        "LayoutColumnNode",
        vec![
            field("column", list(layout_node.clone())),
            field_opt("padding", padding_value.clone()),
            field_opt("margin", float_()),
            field_opt("gap", float_()),
        ],
    ));
    let layout_split_payload = b.define(obj(
        "LayoutSplitPayload",
        vec![
            field("id", str_()),
            field("orientation", lit(&["horizontal", "vertical"])),
            field("ratio", float_()),
            field_opt("first_min", float_()),
            field_opt("second_min", float_()),
            field("first", layout_node.clone()),
            field("second", layout_node.clone()),
        ],
    ));
    let layout_split_node = b.define(obj(
        "LayoutSplitNode",
        vec![
            field("split", layout_split_payload),
            field_opt("padding", padding_value.clone()),
            field_opt("margin", float_()),
            field_opt("gap", float_()),
        ],
    ));
    let layout_tab_entry_node = b.define(obj(
        "LayoutTabEntryNode",
        vec![
            field("id", str_()),
            field("title", str_()),
            field("content", layout_node.clone()),
            field_opt("closable", bool_()),
            field_opt("is_dirty", bool_()),
            field_opt("is_preview", bool_()),
        ],
    ));
    let layout_tabs_payload = b.define(obj(
        "LayoutTabsPayload",
        vec![
            field("id", str_()),
            field("tabs", list(layout_tab_entry_node)),
            field_opt("selected", int_()),
            field_opt("closable", bool_()),
            field_opt("scroll_offset", int_()),
            field_opt("focused_index", nullable(int_())),
            field_opt("right_segments", list(tab_bar_segment_descriptor)),
            field_opt("active_accent", nullable(color.clone())),
            field_opt("compact", bool_()),
        ],
    ));
    let layout_tabs_node = b.define(obj(
        "LayoutTabsNode",
        vec![
            field("tabs", layout_tabs_payload),
            field_opt("padding", padding_value.clone()),
            field_opt("margin", float_()),
        ],
    ));
    let layout_panel_payload = b.define(obj_doc(
        "LayoutPanelPayload",
        vec![
            field("id", str_()),
            field_opt("title", nullable(styled_text.clone())),
            field_opt("actions", list(re("PanelActionDescriptor"))),
            field_opt("accent", nullable(color.clone())),
            field_opt("collapsed", bool_()),
            field("content", layout_node.clone()),
        ],
        "The deprecated `panel` node's own chrome dict -- prefer `LayoutSidebarPanelNode`.",
    ));
    let layout_panel_node = b.define(obj(
        "LayoutPanelNode",
        vec![
            field("panel", layout_panel_payload),
            field_opt("padding", padding_value.clone()),
            field_opt("margin", float_()),
        ],
    ));
    let layout_sidebar_panel_payload = b.define(obj(
        "LayoutSidebarPanelPayload",
        vec![
            field("id", str_()),
            field_opt("toolbar", nullable(toolbar_descriptor)),
            field_opt("toolbar_height", nullable(float_())),
            field("content", layout_node.clone()),
        ],
    ));
    let layout_sidebar_panel_node = b.define(obj(
        "LayoutSidebarPanelNode",
        vec![field("sidebar_panel", layout_sidebar_panel_payload)],
    ));

    let split_tree_recursive_node = re("SplitTreeRecursiveNode");
    let split_tree_leaf = b.define(obj(
        "_SplitTreeLeaf",
        vec![field("leaf", layout_node.clone())],
    ));
    let split_tree_split_payload = b.define(obj(
        "SplitTreeSplitPayload",
        vec![
            field("orientation", lit(&["horizontal", "vertical"])),
            field("ratio", float_()),
            field("first", split_tree_recursive_node.clone()),
            field("second", split_tree_recursive_node),
        ],
    ));
    let split_tree_split_wrapper = b.define(obj(
        "_SplitTreeSplitWrapper",
        vec![field("split", split_tree_split_payload)],
    ));
    b.define(union_of(
        "SplitTreeRecursiveNode",
        vec![split_tree_leaf, split_tree_split_wrapper],
    ));

    let layout_split_tree_payload = b.define(obj(
        "LayoutSplitTreePayload",
        vec![
            field("id", str_()),
            field("root", re("SplitTreeRecursiveNode")),
        ],
    ));
    let layout_split_tree_node = b.define(obj(
        "LayoutSplitTreeNode",
        vec![field("split_tree", layout_split_tree_payload)],
    ));

    // `LayoutLeafNode`/`RenderFrameDict`'s descriptor-kind keys: the six
    // fixed `LEAF_KIND_KEYS`, typed, plus every `EXTRA_PRIMITIVE_KINDS`
    // additive kind -- `status_bar` typed (the one already-modeled extra
    // kind), every other one an untyped placeholder (see this module's own
    // doc). Reading straight off the real Rust consts means a new fixed or
    // additive kind always gets *a* schema entry with no further edit here.
    debug_assert_eq!(
        LEAF_KIND_KEYS.len(),
        6,
        "update the fixed-kind fields below if this changes"
    );
    let mut leaf_fields = vec![field("id", str_())];
    for key in LEAF_KIND_KEYS {
        let ty = match key {
            "button" => button_descriptor.clone(),
            "text_display" => text_display_descriptor.clone(),
            "list" => list_view_descriptor.clone(),
            "tree" => tree_view_descriptor.clone(),
            "data_table" => data_table_descriptor.clone(),
            "tab_bar" => tab_bar_descriptor.clone(),
            other => unreachable!("LEAF_KIND_KEYS grew an unhandled kind {other:?}"),
        };
        leaf_fields.push(field_opt(key, ty));
    }
    let mut render_frame_fields = leaf_fields[1..].to_vec();
    leaf_fields.push(field_opt("status_bar", status_bar_descriptor.clone()));
    for kind in EXTRA_PRIMITIVE_KINDS {
        if kind == "status_bar" {
            continue;
        }
        leaf_fields.push(field_opt(kind, any_()));
        render_frame_fields.push(field_opt(kind, nullable(any_())));
    }
    let layout_leaf_node = b.define(obj_doc(
        "LayoutLeafNode",
        leaf_fields,
        "One `{\"id\", \"<kind>\": <descriptor>}` leaf -- exactly one kind key is present. `status_bar` and the six core kinds are fully typed; every other additive kind (`progress`, `form`, ...) decodes here too but is typed `Any` pending field-by-field narrowing.",
    ));

    b.define(union_of_doc(
        "LayoutNode",
        vec![
            layout_row_node,
            layout_column_node,
            layout_split_node,
            layout_tabs_node,
            layout_panel_node,
            layout_sidebar_panel_node,
            layout_split_tree_node,
            layout_leaf_node,
        ],
        "One node of a render callback's returned layout tree.",
    ));

    b.define(obj_doc(
        "RenderFrameDict",
        render_frame_fields,
        "The pre-layout-tree flat per-slot shape a render callback may still return instead of a `LayoutNode` tree. The six core kinds and `status_bar` are fully typed; every other additive kind is typed `Any`.",
    ));
    b.define(alias_doc(
        "RenderOutput",
        uniont(vec![re("LayoutNode"), re("RenderFrameDict")]),
        "A render callback's return value: a layout tree or the flat shape.",
    ));

    // ── `run_shell` config ───────────────────────────────────────────────
    b.define(obj(
        "PanelDefinitionDescriptor",
        vec![
            field("id", str_()),
            field_opt("icon", str_()),
            field_opt("tooltip", str_()),
            field_opt("title", str_()),
        ],
    ));
    b.define(obj(
        "BottomPanelTabDescriptor",
        vec![
            field("id", str_()),
            field_opt("label", str_()),
            field_opt("closable", bool_()),
            field_opt("badge", nullable(str_())),
        ],
    ));
    b.define(obj(
        "BottomPanelConfigDescriptor",
        vec![
            field_opt("tabs", list(re("BottomPanelTabDescriptor"))),
            field_opt("active_tab_id", str_()),
            field_opt("maximised", bool_()),
            field_opt("height_fraction", float_()),
        ],
    ));
    b.define(obj_doc(
        "ShellConfigDescriptor",
        vec![
            field("title", str_()),
            field_opt("panels", list(re("PanelDefinitionDescriptor"))),
            field_opt("bottom_items", list(re("PanelDefinitionDescriptor"))),
            field_opt("position", lit(&["left", "right"])),
            field_opt("sidebar_visible", bool_()),
            field_opt("default_sidebar_width", float_()),
            field_opt("min_sidebar_width", float_()),
            field_opt("max_sidebar_width", float_()),
            field_opt("activity_bar_width", float_()),
            field_opt("has_title_bar", bool_()),
            field_opt("title_bar_height_lh", float_()),
            field_opt("has_status_bar", bool_()),
            field_opt("has_command_line", bool_()),
            field_opt("has_bottom_panel", bool_()),
            field_opt("bottom_panel_height_lh", float_()),
            field_opt("bottom_panel", re("BottomPanelConfigDescriptor")),
        ],
        "`run_shell`'s `config` dict.",
    ));

    // ── Per-widget activation payloads ───────────────────────────────────
    let key_pressed_str_payload = b.define(obj(
        "_KeyPressedStrPayload",
        vec![field("key", str_()), field("modifiers", modifiers.clone())],
    ));

    let tree_path_payload = b.define(obj("_TreePathPayload", vec![field("path", re("TreePath"))]));
    let tree_row_clicked_payload = b.define(obj(
        "_TreeRowClickedPayload",
        vec![
            field("path", re("TreePath")),
            field("modifiers", modifiers.clone()),
        ],
    ));
    let tree_selection_set_payload = b.define(obj(
        "_TreeSelectionSetPayload",
        vec![field("paths", list(re("TreePath")))],
    ));
    let tree_row_rename_changed_payload = b.define(obj(
        "_TreeRowRenameChangedPayload",
        vec![
            field("path", re("TreePath")),
            field("text", str_()),
            field("cursor", int_()),
            field("selection_anchor", nullable(int_())),
        ],
    ));
    let tree_row_rename_committed_payload = b.define(obj(
        "_TreeRowRenameCommittedPayload",
        vec![field("path", re("TreePath")), field("text", str_())],
    ));
    b.define(flat(
        "TreeEvent",
        &[
            ("RowClicked", tree_row_clicked_payload),
            ("RowDoubleClicked", tree_path_payload.clone()),
            ("RowToggleExpand", tree_path_payload.clone()),
            ("SelectionChanged", tree_path_payload.clone()),
            ("KeyPressed", key_pressed_str_payload.clone()),
            ("SelectionSet", tree_selection_set_payload),
            ("RowRenameChanged", tree_row_rename_changed_payload),
            ("RowRenameCommitted", tree_row_rename_committed_payload),
            ("RowRenameCancelled", tree_path_payload),
        ],
        vec![],
    ));

    let idx_payload = b.define(obj("_IdxPayload", vec![field("idx", int_())]));
    let indices_payload = b.define(obj("_IndicesPayload", vec![field("indices", list(int_()))]));
    b.define(flat(
        "ListViewEvent",
        &[
            ("SelectionChanged", idx_payload.clone()),
            ("ItemActivated", idx_payload.clone()),
            ("ItemDoubleClicked", idx_payload.clone()),
            ("KeyPressed", key_pressed_str_payload.clone()),
            ("SelectionSet", indices_payload),
        ],
        vec![],
    ));

    let col_payload = b.define(obj("_ColPayload", vec![field("col", int_())]));
    let data_table_scroll_payload = b.define(obj(
        "_DataTableScrollPayload",
        vec![
            field("delta", int_()),
            field("modifiers", modifiers.clone()),
        ],
    ));
    let column_resized_payload = b.define(obj(
        "_ColumnResizedPayload",
        vec![field("col", int_()), field("width", float_())],
    ));
    let data_table_selection_set_payload = b.define(obj(
        "_DataTableSelectionSetPayload",
        vec![field("indices", list(int_()))],
    ));
    b.define(flat(
        "DataTableEvent",
        &[
            ("HeaderClicked", col_payload),
            ("RowActivated", idx_payload.clone()),
            ("RowSelected", idx_payload.clone()),
            ("RowDoubleClicked", idx_payload),
            ("Scroll", data_table_scroll_payload),
            ("ColumnResized", column_resized_payload),
            ("SelectionSet", data_table_selection_set_payload),
        ],
        vec![],
    ));

    let index_payload = b.define(obj("_IndexPayload", vec![field("index", int_())]));
    let id_payload = b.define(obj("_IdPayload", vec![field("id", re("WidgetId"))]));
    b.define(flat(
        "TabBarEvent",
        &[
            ("TabActivated", index_payload.clone()),
            ("TabClosed", index_payload.clone()),
            ("ButtonClicked", id_payload.clone()),
            ("KeyPressed", key_pressed_str_payload.clone()),
            ("FocusMoved", index_payload),
        ],
        vec![],
    ));

    b.define(flat(
        "StatusBarEvent",
        &[
            ("SegmentClicked", id_payload.clone()),
            ("KeyPressed", key_pressed_str_payload.clone()),
        ],
        vec![],
    ));

    let new_offset_payload = b.define(obj("_NewOffsetPayload", vec![field("new_offset", int_())]));
    let enabled_payload = b.define(obj("_EnabledPayload", vec![field("enabled", bool_())]));
    let text_payload = b.define(obj("_TextPayload", vec![field("text", str_())]));
    b.define(flat(
        "TextDisplayEvent",
        &[
            ("Scrolled", new_offset_payload),
            ("AutoScrollToggled", enabled_payload),
            ("Copied", text_payload),
            ("KeyPressed", key_pressed_str_payload.clone()),
        ],
        vec![],
    ));

    b.define(flat(
        "ActivityBarEvent",
        &[
            ("ItemClicked", id_payload.clone()),
            ("KeyPressed", key_pressed_str_payload.clone()),
        ],
        vec![],
    ));

    let row_col_payload = b.define(obj(
        "_RowColPayload",
        vec![field("row", int_()), field("col", int_())],
    ));
    let int_delta_payload = b.define(obj("_IntDeltaPayload", vec![field("delta", int_())]));
    let terminal_event_dict = b.define(flat(
        "_TerminalEventDict",
        &[
            ("KeyPressed", key_pressed_str_payload.clone()),
            ("SelectStart", row_col_payload.clone()),
            ("SelectExtend", row_col_payload),
            ("Scroll", int_delta_payload),
        ],
        vec![],
    ));
    b.define(alias_doc(
        "TerminalEvent",
        uniont(vec![lit(&["SelectEnd"]), terminal_event_dict]),
        "Mouse release has no payload -- bare `\"SelectEnd\"`, unlike every other variant.",
    ));

    let data_point_clicked_payload = b.define(obj(
        "_DataPointClickedPayload",
        vec![
            field("id", re("WidgetId")),
            field("series_idx", int_()),
            field("data_idx", int_()),
        ],
    ));
    let legend_clicked_payload = b.define(obj(
        "_LegendClickedPayload",
        vec![field("id", re("WidgetId")), field("series_idx", int_())],
    ));
    b.define(flat(
        "ChartEvent",
        &[
            ("Clicked", id_payload.clone()),
            ("DataPointClicked", data_point_clicked_payload),
            ("LegendClicked", legend_clicked_payload),
        ],
        vec![],
    ));

    let toolbar_button_clicked_payload = b.define(obj(
        "_ToolbarButtonClickedPayload",
        vec![field("id", re("WidgetId")), field("is_active", bool_())],
    ));
    b.define(flat(
        "ToolbarEvent",
        &[("ButtonClicked", toolbar_button_clicked_payload)],
        vec![],
    ));

    // ── `FormEvent` (quadraui/src/primitives/form.rs) ───────────────────
    let calendar_date = b.define(obj(
        "CalendarDate",
        vec![
            field("year", int_()),
            field("month", int_()),
            field("day", int_()),
        ],
    ));
    let id_value_bool = b.define(obj(
        "_IdValueBoolPayload",
        vec![field("id", re("WidgetId")), field("value", bool_())],
    ));
    let id_value_str = b.define(obj(
        "_IdValueStrPayload",
        vec![field("id", re("WidgetId")), field("value", str_())],
    ));
    let id_value_float = b.define(obj(
        "_IdValueFloatPayload",
        vec![field("id", re("WidgetId")), field("value", float_())],
    ));
    let id_value_color = b.define(obj(
        "_IdValueColorPayload",
        vec![field("id", re("WidgetId")), field("value", color.clone())],
    ));
    let id_selected_idx = b.define(obj(
        "_IdSelectedIdxPayload",
        vec![field("id", re("WidgetId")), field("selected_idx", int_())],
    ));
    let id_value_date = b.define(obj(
        "_IdValueDatePayload",
        vec![field("id", re("WidgetId")), field("value", calendar_date)],
    ));
    let id_only = b.define(obj("_IdOnlyPayload", vec![field("id", re("WidgetId"))]));
    let toolbar_button_clicked_form_payload = b.define(obj(
        "_FormToolbarButtonClickedPayload",
        vec![
            field("field_id", re("WidgetId")),
            field("button_id", re("WidgetId")),
        ],
    ));
    let id_value_f64 = b.define(obj(
        "_IdValueF64Payload",
        vec![field("id", re("WidgetId")), field("value", float_())],
    ));
    let id_draft = b.define(obj(
        "_IdDraftPayload",
        vec![field("id", re("WidgetId")), field("draft", str_())],
    ));
    let id_index = b.define(obj(
        "_IdIndexPayload",
        vec![field("id", re("WidgetId")), field("index", int_())],
    ));
    let id_selected = b.define(obj(
        "_IdSelectedPayload",
        vec![field("id", re("WidgetId")), field("selected", int_())],
    ));
    b.define(flat_doc(
        "FormEvent",
        &[
            ("ToggleChanged", id_value_bool),
            ("TextInputChanged", id_value_str.clone()),
            ("TextInputCommitted", id_value_str.clone()),
            ("SliderChanged", id_value_float),
            ("ColorChanged", id_value_color),
            ("DropdownChanged", id_selected_idx.clone()),
            ("SegmentedControlChanged", id_selected_idx),
            ("DateChanged", id_value_date),
            ("FocusChanged", id_only.clone()),
            ("ButtonClicked", id_only),
            ("ToolbarButtonClicked", toolbar_button_clicked_form_payload),
            ("KeyPressed", key_pressed_str_payload.clone()),
            ("ComboBoxTextChanged", id_value_str.clone()),
            ("ComboBoxHighlightChanged", id_index),
            ("ComboBoxCommitted", id_value_str),
            ("ComboBoxCancelled", id_payload.clone()),
            ("NumberChanged", id_value_f64.clone()),
            ("NumberDraftChanged", id_draft),
            ("NumberCommitted", id_value_f64),
            ("NumberCancelled", id_payload),
            ("RadioChanged", id_selected),
        ],
        vec![],
        "Events a `Form` emits back to the app -- see `quadraui/src/primitives/form.rs`'s `FormEvent`.",
    ));

    // ── `PaletteEvent` (quadraui/src/primitives/palette.rs) ─────────────
    let query_changed_payload = b.define(obj("_QueryChangedPayload", vec![field("value", str_())]));
    let selection_changed_idx_payload = b.define(obj(
        "_SelectionChangedIdxPayload",
        vec![field("idx", int_())],
    ));
    let item_confirmed_payload = b.define(obj("_ItemConfirmedPayload", vec![field("idx", int_())]));
    let expand_toggled_payload = b.define(obj(
        "_ExpandToggledPayload",
        vec![field("idx", int_()), field("expanded", bool_())],
    ));
    b.define(flat_doc(
        "PaletteEvent",
        &[
            ("QueryChanged", query_changed_payload),
            ("SelectionChanged", selection_changed_idx_payload),
            ("ItemConfirmed", item_confirmed_payload),
            ("ExpandToggled", expand_toggled_payload),
            ("KeyPressed", key_pressed_str_payload.clone()),
        ],
        vec![],
        "Events a `Palette` emits back to the app -- see `quadraui/src/primitives/palette.rs`'s `PaletteEvent`. `Closed` has no payload -- bare `\"Closed\"`, like `TerminalEvent`'s `\"SelectEnd\"`.",
    ));

    // ── `HitDetail` (quadraui-binding-core/src/hit.rs) ───────────────────
    type HitDetailExtraField = (&'static str, fn() -> TyJson);
    type HitDetailKind = (&'static str, &'static str, &'static [HitDetailExtraField]);
    const HIT_DETAIL_KINDS: &[HitDetailKind] = &[
        ("_HitDetailButtonBody", "button_body", &[]),
        ("_HitDetailTreeRow", "tree_row", &[("row", int_)]),
        ("_HitDetailTreeChevron", "tree_chevron", &[("row", int_)]),
        ("_HitDetailListTitle", "list_title", &[]),
        ("_HitDetailListItem", "list_item", &[("index", int_)]),
        (
            "_HitDetailDataTableHeader",
            "data_table_header",
            &[("col", int_)],
        ),
        (
            "_HitDetailDataTableHeaderDivider",
            "data_table_header_divider",
            &[("col", int_)],
        ),
        ("_HitDetailDataTableRow", "data_table_row", &[("row", int_)]),
        ("_HitDetailDataTableFooter", "data_table_footer", &[]),
        ("_HitDetailTabBarTab", "tab_bar_tab", &[("tab", int_)]),
        (
            "_HitDetailTabBarTabClose",
            "tab_bar_tab_close",
            &[("tab", int_)],
        ),
        ("_HitDetailTabBarScrollLeft", "tab_bar_scroll_left", &[]),
        ("_HitDetailTabBarScrollRight", "tab_bar_scroll_right", &[]),
        ("_HitDetailTabBarRightSegment", "tab_bar_right_segment", &[]),
        (
            "_HitDetailTextDisplayLine",
            "text_display_line",
            &[("line", int_)],
        ),
        (
            "_HitDetailTextDisplayScrollbarThumb",
            "text_display_scrollbar_thumb",
            &[],
        ),
        (
            "_HitDetailTextDisplayScrollbarTrackBefore",
            "text_display_scrollbar_track_before",
            &[],
        ),
        (
            "_HitDetailTextDisplayScrollbarTrackAfter",
            "text_display_scrollbar_track_after",
            &[],
        ),
        ("_HitDetailSplitFirstPane", "split_first_pane", &[]),
        ("_HitDetailSplitSecondPane", "split_second_pane", &[]),
        ("_HitDetailSplitDivider", "split_divider", &[]),
        (
            "_HitDetailSplitTreeDivider",
            "split_tree_divider",
            &[("split_index", int_)],
        ),
        ("_HitDetailPanelAction", "panel_action", &[]),
        ("_HitDetailPanelTitleBar", "panel_title_bar", &[]),
        (
            "_HitDetailSidebarPanelToolbarButton",
            "sidebar_panel_toolbar_button",
            &[],
        ),
        (
            "_HitDetailTreeScrollbarTrackBefore",
            "tree_scrollbar_track_before",
            &[],
        ),
        ("_HitDetailTreeScrollbarThumb", "tree_scrollbar_thumb", &[]),
        (
            "_HitDetailTreeScrollbarTrackAfter",
            "tree_scrollbar_track_after",
            &[],
        ),
        ("_HitDetailExtraBody", "extra_body", &[]),
        ("_HitDetailExtraChrome", "extra_chrome", &[]),
    ];
    let mut hit_detail_members = Vec::new();
    for (name, kind, extra_fields) in HIT_DETAIL_KINDS {
        let mut fields = vec![field("kind", lit(&[kind]))];
        for (fname, fty) in *extra_fields {
            fields.push(field(fname, fty()));
        }
        if *kind == "sidebar_panel_toolbar_button" || *kind == "extra_chrome" {
            fields.push(field("bar", re("WidgetId")));
        }
        hit_detail_members.push(b.define(obj(name, fields)));
    }
    b.define(union_of_doc(
        "HitDetail",
        hit_detail_members,
        "The sub-region of a primitive a pointer event landed on -- see `HitDetail` in `quadraui-binding-core/src/hit.rs`. Narrow on `.kind`.",
    ));

    // ── `UiEvent` (quadraui/src/event.rs) ────────────────────────────────
    let ui_key_pressed_payload = b.define(obj(
        "_UiKeyPressedPayload",
        vec![
            field("key", key),
            field("modifiers", modifiers.clone()),
            field("repeat", bool_()),
        ],
    ));
    let mouse_down_payload = b.define(obj(
        "_MouseDownPayload",
        vec![
            field("widget", nullable(re("WidgetId"))),
            field("button", mouse_button.clone()),
            field("position", point.clone()),
            field("modifiers", modifiers.clone()),
        ],
    ));
    let mouse_up_payload = b.define(obj(
        "_MouseUpPayload",
        vec![
            field("widget", nullable(re("WidgetId"))),
            field("button", mouse_button),
            field("position", point.clone()),
        ],
    ));
    let mouse_moved_payload = b.define(obj(
        "_MouseMovedPayload",
        vec![
            field("position", point.clone()),
            field("buttons", button_mask),
        ],
    ));
    let widget_payload = b.define(obj("_WidgetPayload", vec![field("widget", re("WidgetId"))]));
    let double_click_payload = b.define(obj(
        "_DoubleClickPayload",
        vec![
            field("widget", nullable(re("WidgetId"))),
            field("position", point.clone()),
        ],
    ));
    let ui_scroll_payload = b.define(obj(
        "_UiScrollPayload",
        vec![
            field("widget", nullable(re("WidgetId"))),
            field("delta", scroll_delta),
            field("position", point.clone()),
        ],
    ));
    let window_resized_payload = b.define(obj(
        "_WindowResizedPayload",
        vec![field("viewport", viewport)],
    ));
    let window_state_changed_payload = b.define(obj(
        "_WindowStateChangedPayload",
        vec![
            field("maximized", bool_()),
            field("fullscreen", bool_()),
            field("minimized", bool_()),
        ],
    ));
    let files_dropped_payload = b.define(obj(
        "_FilesDroppedPayload",
        vec![field("paths", list(str_())), field("position", point)],
    ));
    let open_requested_payload = b.define(obj(
        "_OpenRequestedPayload",
        vec![field("urls", list(str_())), field("files", list(str_()))],
    ));
    let power_state_changed_payload = b.define(obj(
        "_PowerStateChangedPayload",
        vec![field("kind", power_event_kind)],
    ));
    let scroll_offset_changed_payload = b.define(obj(
        "_ScrollOffsetChangedPayload",
        vec![field("widget", re("WidgetId")), field("new_offset", int_())],
    ));
    let text_selection_changed_payload = b.define(obj(
        "_TextSelectionChangedPayload",
        vec![
            field("region", re("WidgetId")),
            field("anchor", re("Point")),
            field("focus", re("Point")),
        ],
    ));
    let split_divider_dragged_payload = b.define(obj(
        "_SplitDividerDraggedPayload",
        vec![
            field("tree", re("WidgetId")),
            field("split_index", int_()),
            field("new_ratio", float_()),
        ],
    ));
    let tray_clicked_payload = b.define(obj(
        "_TrayClickedPayload",
        vec![field("button", re("MouseButton"))],
    ));
    let notification_activated_payload = b.define(obj(
        "_NotificationActivatedPayload",
        vec![
            field("tag", nullable(str_())),
            field("action", nullable(re("WidgetId"))),
        ],
    ));

    b.define(flat_doc(
        "_UiEventDict",
        &[
            ("Accelerator", tup(vec![re("AcceleratorId"), modifiers.clone()])),
            ("GlobalShortcut", re("GlobalShortcutId")),
            ("KeyPressed", ui_key_pressed_payload),
            ("CharTyped", str_()),
            ("MouseDown", mouse_down_payload),
            ("MouseUp", mouse_up_payload),
            ("MouseMoved", mouse_moved_payload),
            ("MouseEntered", widget_payload.clone()),
            ("MouseLeft", widget_payload),
            ("DoubleClick", double_click_payload),
            ("Scroll", ui_scroll_payload),
            ("WindowResized", window_resized_payload),
            ("WindowFocused", bool_()),
            ("DpiChanged", float_()),
            ("WindowStateChanged", window_state_changed_payload),
            ("FilesDropped", files_dropped_payload),
            ("ClipboardPaste", str_()),
            ("OpenRequested", open_requested_payload),
            ("TextCopied", str_()),
            ("FocusChanged", nullable(re("WidgetId"))),
            ("SystemThemeChanged", system_theme),
            ("PowerStateChanged", power_state_changed_payload),
            ("ScrollOffsetChanged", scroll_offset_changed_payload),
            ("TextSelectionChanged", text_selection_changed_payload),
            ("SplitDividerDragged", split_divider_dragged_payload),
            ("MenuActivated", re("WidgetId")),
            ("ContextMenuItemActivated", re("WidgetId")),
            ("MessageDialogResolved", re("WidgetId")),
            ("ToastActionClicked", re("WidgetId")),
            ("CompletionItemActivated", tup(vec![re("WidgetId"), int_()])),
            ("RichTextLinkClicked", tup(vec![re("WidgetId"), str_()])),
            ("TrayClicked", tray_clicked_payload),
            ("NotificationActivated", notification_activated_payload),
            ("Tree", tup(vec![re("WidgetId"), re("TreeEvent")])),
            ("List", tup(vec![re("WidgetId"), re("ListViewEvent")])),
            ("Form", tup(vec![re("WidgetId"), re("FormEvent")])),
            ("Palette", tup(vec![re("WidgetId"), re("PaletteEvent")])),
            ("TabBar", tup(vec![re("WidgetId"), re("TabBarEvent")])),
            ("StatusBar", tup(vec![re("WidgetId"), re("StatusBarEvent")])),
            ("ActivityBar", tup(vec![re("WidgetId"), re("ActivityBarEvent")])),
            ("Terminal", tup(vec![re("WidgetId"), re("TerminalEvent")])),
            ("TextDisplay", tup(vec![re("WidgetId"), re("TextDisplayEvent")])),
            ("Chart", tup(vec![re("WidgetId"), re("ChartEvent")])),
            ("DataTable", tup(vec![re("WidgetId"), re("DataTableEvent")])),
            ("Toolbar", tup(vec![re("WidgetId"), re("ToolbarEvent")])),
            ("BackendNative", backend_native_event),
            ("User", any_()),
        ],
        vec![with_doc(
            field_opt("hit", re("HitDetail")),
            "The hit-tested sub-region a pointer landed on, when the binding attached one (never present on `User`).",
        )],
        "Everything a user (or platform) can do that an app might care about -- see `quadraui/src/event.rs`'s `UiEvent`.",
    ));

    b.define(alias_doc(
        "UiEvent",
        uniont(vec![
            lit(&["WindowClose", "DisplaysChanged", "ContextMenuDismissed"]),
            re("_UiEventDict"),
        ]),
        "`isinstance(event, str)` (Python) / `typeof event === \"string\"` (TS) separates the three no-payload variants from the rest before narrowing on a key.",
    ));

    b.defs
}

#[cfg(test)]
mod coverage {
    //! Compile-time drift guards: one exhaustive `match`/destructure per
    //! real Rust type [`super::schema`] models, with no wildcard arm and
    //! no `..`. Adding a variant or field to the matching Rust type without
    //! updating [`super::schema`] breaks compilation here (`E0004`
    //! non-exhaustive match, or `E0027` pattern missing a field) --
    //! [`super::schema`] can't silently drift out of sync with the real
    //! types it describes.
    //!
    //! These guards never run as tests (there is nothing to execute --
    //! every function body only destructures its argument); `cargo build`/
    //! `cargo check` already type-checks every function in the crate
    //! whether or not it's called, so gating them behind `#[cfg(test)]`
    //! only changes *which* command needs to run to see the failure, not
    //! whether this module compiles. Keeping them in a dedicated test
    //! module (rather than scattered through `schema()` itself) keeps the
    //! schema construction and its coverage guards reviewable side by
    //! side without interleaving unrelated code.

    #![allow(dead_code, unused_variables)]
    // `Panel`/`PanelAction` are `#[deprecated]` (use `SidebarPanel` + a
    // header `Toolbar` instead); this module's own coverage guards for
    // their fields are the deprecated type's own plumbing, same rationale
    // as `crate::wire`'s identical allow.
    #![allow(deprecated)]

    use quadraui::{
        ActivityBarEvent, Badge, Button, ChartEvent, Color, Column, DataRow, DataTable,
        DataTableEvent, FormEvent, ListItem, ListView, ListViewEvent, PaletteEvent, Panel,
        PanelAction, SidebarPanel, StatusBar, StatusBarEvent, StatusBarSegment, StyledSpan,
        StyledText, TabBar, TabBarEvent, TabItem, TerminalEvent, TextDisplay, TextDisplayEvent,
        TextDisplayLine, Toolbar, ToolbarButton, ToolbarEvent, TreeEvent, TreeRow,
        TreeRowEditState, TreeView, UiEvent,
    };

    use crate::hit::HitDetail;
    use crate::layout::{LayoutNode, LeafDescriptor};

    fn assert_ui_event(e: UiEvent) {
        match e {
            UiEvent::Accelerator(_, _) => {}
            UiEvent::GlobalShortcut(_) => {}
            UiEvent::KeyPressed {
                key: _,
                modifiers: _,
                repeat: _,
            } => {}
            UiEvent::CharTyped(_) => {}
            UiEvent::MouseDown {
                widget: _,
                button: _,
                position: _,
                modifiers: _,
            } => {}
            UiEvent::MouseUp {
                widget: _,
                button: _,
                position: _,
            } => {}
            UiEvent::MouseMoved {
                position: _,
                buttons: _,
            } => {}
            UiEvent::MouseEntered { widget: _ } => {}
            UiEvent::MouseLeft { widget: _ } => {}
            UiEvent::DoubleClick {
                widget: _,
                position: _,
            } => {}
            UiEvent::Scroll {
                widget: _,
                delta: _,
                position: _,
            } => {}
            UiEvent::WindowResized { viewport: _ } => {}
            UiEvent::WindowClose => {}
            UiEvent::WindowFocused(_) => {}
            UiEvent::DpiChanged(_) => {}
            UiEvent::WindowStateChanged {
                maximized: _,
                fullscreen: _,
                minimized: _,
            } => {}
            UiEvent::FilesDropped {
                paths: _,
                position: _,
            } => {}
            UiEvent::ClipboardPaste(_) => {}
            UiEvent::OpenRequested { urls: _, files: _ } => {}
            UiEvent::TextCopied(_) => {}
            UiEvent::FocusChanged(_) => {}
            UiEvent::SystemThemeChanged(_) => {}
            UiEvent::DisplaysChanged => {}
            UiEvent::PowerStateChanged { kind: _ } => {}
            UiEvent::ScrollOffsetChanged {
                widget: _,
                new_offset: _,
            } => {}
            UiEvent::TextSelectionChanged {
                region: _,
                anchor: _,
                focus: _,
            } => {}
            UiEvent::SplitDividerDragged {
                tree: _,
                split_index: _,
                new_ratio: _,
            } => {}
            UiEvent::MenuActivated(_) => {}
            UiEvent::ContextMenuItemActivated(_) => {}
            UiEvent::ContextMenuDismissed => {}
            UiEvent::MessageDialogResolved(_) => {}
            UiEvent::ToastActionClicked(_) => {}
            UiEvent::CompletionItemActivated(_, _) => {}
            UiEvent::RichTextLinkClicked(_, _) => {}
            UiEvent::TrayClicked { button: _ } => {}
            UiEvent::NotificationActivated { tag: _, action: _ } => {}
            UiEvent::Tree(_, _) => {}
            UiEvent::List(_, _) => {}
            UiEvent::Form(_, _) => {}
            UiEvent::Palette(_, _) => {}
            UiEvent::TabBar(_, _) => {}
            UiEvent::StatusBar(_, _) => {}
            UiEvent::ActivityBar(_, _) => {}
            UiEvent::Terminal(_, _) => {}
            UiEvent::TextDisplay(_, _) => {}
            UiEvent::Chart(_, _) => {}
            UiEvent::DataTable(_, _) => {}
            UiEvent::Toolbar(_, _) => {}
            UiEvent::BackendNative(_) => {}
            // `User` carries an opaque `Arc<dyn Any>` -- deliberately
            // typed `Any`/`unknown` in the schema (see `UserPayload`'s own
            // doc); still named here so a *new* variant after it still
            // trips this match.
            UiEvent::User(_) => {}
        }
    }

    fn assert_form_event(e: FormEvent) {
        match e {
            FormEvent::ToggleChanged { id: _, value: _ } => {}
            FormEvent::TextInputChanged { id: _, value: _ } => {}
            FormEvent::TextInputCommitted { id: _, value: _ } => {}
            FormEvent::SliderChanged { id: _, value: _ } => {}
            FormEvent::ColorChanged { id: _, value: _ } => {}
            FormEvent::DropdownChanged {
                id: _,
                selected_idx: _,
            } => {}
            FormEvent::SegmentedControlChanged {
                id: _,
                selected_idx: _,
            } => {}
            FormEvent::DateChanged { id: _, value: _ } => {}
            FormEvent::FocusChanged { id: _ } => {}
            FormEvent::ButtonClicked { id: _ } => {}
            FormEvent::ToolbarButtonClicked {
                field_id: _,
                button_id: _,
            } => {}
            FormEvent::KeyPressed {
                key: _,
                modifiers: _,
            } => {}
            FormEvent::ComboBoxTextChanged { id: _, value: _ } => {}
            FormEvent::ComboBoxHighlightChanged { id: _, index: _ } => {}
            FormEvent::ComboBoxCommitted { id: _, value: _ } => {}
            FormEvent::ComboBoxCancelled { id: _ } => {}
            FormEvent::NumberChanged { id: _, value: _ } => {}
            FormEvent::NumberDraftChanged { id: _, draft: _ } => {}
            FormEvent::NumberCommitted { id: _, value: _ } => {}
            FormEvent::NumberCancelled { id: _ } => {}
            FormEvent::RadioChanged { id: _, selected: _ } => {}
        }
    }

    fn assert_palette_event(e: PaletteEvent) {
        match e {
            PaletteEvent::QueryChanged { value: _ } => {}
            PaletteEvent::SelectionChanged { idx: _ } => {}
            PaletteEvent::ItemConfirmed { idx: _ } => {}
            PaletteEvent::ExpandToggled {
                idx: _,
                expanded: _,
            } => {}
            PaletteEvent::Closed => {}
            PaletteEvent::KeyPressed {
                key: _,
                modifiers: _,
            } => {}
        }
    }

    fn assert_tree_event(e: TreeEvent) {
        match e {
            TreeEvent::RowClicked {
                path: _,
                modifiers: _,
            } => {}
            TreeEvent::RowDoubleClicked { path: _ } => {}
            TreeEvent::RowToggleExpand { path: _ } => {}
            TreeEvent::SelectionChanged { path: _ } => {}
            TreeEvent::KeyPressed {
                key: _,
                modifiers: _,
            } => {}
            TreeEvent::SelectionSet { paths: _ } => {}
            TreeEvent::RowRenameChanged {
                path: _,
                text: _,
                cursor: _,
                selection_anchor: _,
            } => {}
            TreeEvent::RowRenameCommitted { path: _, text: _ } => {}
            TreeEvent::RowRenameCancelled { path: _ } => {}
        }
    }

    fn assert_list_view_event(e: ListViewEvent) {
        match e {
            ListViewEvent::SelectionChanged { idx: _ } => {}
            ListViewEvent::ItemActivated { idx: _ } => {}
            ListViewEvent::ItemDoubleClicked { idx: _ } => {}
            ListViewEvent::KeyPressed {
                key: _,
                modifiers: _,
            } => {}
            ListViewEvent::SelectionSet { indices: _ } => {}
        }
    }

    fn assert_data_table_event(e: DataTableEvent) {
        match e {
            DataTableEvent::HeaderClicked { col: _ } => {}
            DataTableEvent::RowActivated { idx: _ } => {}
            DataTableEvent::RowSelected { idx: _ } => {}
            DataTableEvent::RowDoubleClicked { idx: _ } => {}
            DataTableEvent::Scroll {
                delta: _,
                modifiers: _,
            } => {}
            DataTableEvent::ColumnResized { col: _, width: _ } => {}
            DataTableEvent::SelectionSet { indices: _ } => {}
        }
    }

    fn assert_tab_bar_event(e: TabBarEvent) {
        match e {
            TabBarEvent::TabActivated { index: _ } => {}
            TabBarEvent::TabClosed { index: _ } => {}
            TabBarEvent::ButtonClicked { id: _ } => {}
            TabBarEvent::KeyPressed {
                key: _,
                modifiers: _,
            } => {}
            TabBarEvent::FocusMoved { index: _ } => {}
            // `TabBarEvent` is `#[non_exhaustive]` -- a new variant doesn't
            // need a wildcard-arm migration here, so this guard can't catch
            // one landing unnoticed. Still worth naming every variant that
            // exists today explicitly.
            _ => {}
        }
    }

    fn assert_status_bar_event(e: StatusBarEvent) {
        match e {
            StatusBarEvent::SegmentClicked { id: _ } => {}
            StatusBarEvent::KeyPressed {
                key: _,
                modifiers: _,
            } => {}
        }
    }

    fn assert_text_display_event(e: TextDisplayEvent) {
        match e {
            TextDisplayEvent::Scrolled { new_offset: _ } => {}
            TextDisplayEvent::AutoScrollToggled { enabled: _ } => {}
            TextDisplayEvent::Copied { text: _ } => {}
            TextDisplayEvent::KeyPressed {
                key: _,
                modifiers: _,
            } => {}
        }
    }

    fn assert_activity_bar_event(e: ActivityBarEvent) {
        match e {
            ActivityBarEvent::ItemClicked { id: _ } => {}
            ActivityBarEvent::KeyPressed {
                key: _,
                modifiers: _,
            } => {}
        }
    }

    fn assert_terminal_event(e: TerminalEvent) {
        match e {
            TerminalEvent::KeyPressed {
                key: _,
                modifiers: _,
            } => {}
            TerminalEvent::SelectStart { row: _, col: _ } => {}
            TerminalEvent::SelectExtend { row: _, col: _ } => {}
            TerminalEvent::SelectEnd => {}
            TerminalEvent::Scroll { delta: _ } => {}
        }
    }

    fn assert_chart_event(e: ChartEvent) {
        match e {
            ChartEvent::Clicked { id: _ } => {}
            ChartEvent::DataPointClicked {
                id: _,
                series_idx: _,
                data_idx: _,
            } => {}
            ChartEvent::LegendClicked {
                id: _,
                series_idx: _,
            } => {}
        }
    }

    fn assert_toolbar_event(e: ToolbarEvent) {
        match e {
            ToolbarEvent::ButtonClicked {
                id: _,
                is_active: _,
            } => {}
        }
    }

    fn assert_hit_detail(h: HitDetail) {
        match h {
            HitDetail::ButtonBody => {}
            HitDetail::TreeRow { row: _ } => {}
            HitDetail::TreeChevron { row: _ } => {}
            HitDetail::ListTitle => {}
            HitDetail::ListItem { index: _ } => {}
            HitDetail::DataTableHeader { col: _ } => {}
            HitDetail::DataTableHeaderDivider { col: _ } => {}
            HitDetail::DataTableRow { row: _ } => {}
            HitDetail::DataTableFooter => {}
            HitDetail::TabBarTab { tab: _ } => {}
            HitDetail::TabBarTabClose { tab: _ } => {}
            HitDetail::TabBarScrollLeft => {}
            HitDetail::TabBarScrollRight => {}
            HitDetail::TabBarRightSegment => {}
            HitDetail::TextDisplayLine { line: _ } => {}
            HitDetail::TextDisplayScrollbarThumb => {}
            HitDetail::TextDisplayScrollbarTrackBefore => {}
            HitDetail::TextDisplayScrollbarTrackAfter => {}
            HitDetail::SplitFirstPane => {}
            HitDetail::SplitSecondPane => {}
            HitDetail::SplitDivider => {}
            HitDetail::SplitTreeDivider { split_index: _ } => {}
            HitDetail::PanelAction => {}
            HitDetail::PanelTitleBar => {}
            HitDetail::SidebarPanelToolbarButton { bar: _ } => {}
            HitDetail::TreeScrollbarTrackBefore => {}
            HitDetail::TreeScrollbarThumb => {}
            HitDetail::TreeScrollbarTrackAfter => {}
            HitDetail::ExtraBody => {}
            HitDetail::ExtraChrome { bar: _ } => {}
        }
    }

    fn assert_leaf_descriptor(d: LeafDescriptor) {
        match d {
            LeafDescriptor::Button(_) => {}
            LeafDescriptor::TextDisplay(_) => {}
            LeafDescriptor::List(_) => {}
            LeafDescriptor::Tree(_) => {}
            LeafDescriptor::DataTable(_) => {}
            LeafDescriptor::TabBar(_) => {}
            LeafDescriptor::Extra(_) => {}
        }
    }

    fn assert_layout_node(n: LayoutNode) {
        match n {
            LayoutNode::Row(_) => {}
            LayoutNode::Column(_) => {}
            LayoutNode::Leaf(_) => {}
            LayoutNode::Split(_) => {}
            LayoutNode::Tabs(_) => {}
            LayoutNode::Panel(_) => {}
            LayoutNode::SidebarPanel(_) => {}
            LayoutNode::SplitTree(_) => {}
        }
    }

    macro_rules! assert_fields {
        ($fn_name:ident, $ty:path { $($field_name:ident),+ $(,)? }) => {
            #[allow(dead_code)]
            fn $fn_name(v: $ty) {
                let $ty { $($field_name),+ } = v;
                $(let _ = $field_name;)+
            }
        };
    }

    /// Same contract as [`assert_fields`], for a `#[non_exhaustive]` struct
    /// (`Button`, `Toolbar`): the compiler requires a trailing `..` on a
    /// non-exhaustive struct pattern from outside its defining crate, so a
    /// *new* field on one of these two stays invisible to this build,
    /// unlike every other struct below -- the `#[non_exhaustive]` marker is
    /// this project's own deliberate "add a field without breaking
    /// consumers" escape hatch (see `Button`'s own doc), and this guard
    /// can't see through it. Still worth listing explicitly: a *removed*
    /// or *renamed* field still breaks this.
    macro_rules! assert_fields_non_exhaustive {
        ($fn_name:ident, $ty:path { $($field_name:ident),+ $(,)? }) => {
            #[allow(dead_code)]
            fn $fn_name(v: $ty) {
                let $ty { $($field_name),+, .. } = v;
                $(let _ = $field_name;)+
            }
        };
    }

    assert_fields_non_exhaustive!(
        assert_button_fields,
        Button {
            id,
            label,
            variant,
            disabled,
            focused,
            tooltip,
            icon,
            icon_position,
            icon_gap
        }
    );
    assert_fields!(
        assert_text_display_fields,
        TextDisplay {
            id,
            lines,
            scroll_offset,
            auto_scroll,
            max_lines,
            has_focus,
            title,
            show_scrollbar
        }
    );
    assert_fields!(
        assert_text_display_line_fields,
        TextDisplayLine {
            spans,
            decoration,
            timestamp
        }
    );
    assert_fields!(
        assert_list_view_fields,
        ListView {
            id,
            title,
            items,
            selected_idx,
            scroll_offset,
            has_focus,
            bordered,
            h_scroll,
            max_content_width,
            show_v_scrollbar
        }
    );
    assert_fields!(
        assert_list_item_fields,
        ListItem {
            text,
            icon,
            detail,
            decoration
        }
    );
    assert_fields!(
        assert_tree_view_fields,
        TreeView {
            id,
            rows,
            selection_mode,
            selected_path,
            scroll_offset,
            style,
            has_focus
        }
    );
    assert_fields!(
        assert_tree_row_fields,
        TreeRow {
            path,
            indent,
            icon,
            text,
            badge,
            is_expanded,
            decoration,
            edit
        }
    );
    assert_fields!(
        assert_tree_row_edit_state_fields,
        TreeRowEditState {
            text,
            cursor,
            selection_anchor,
            placeholder
        }
    );
    assert_fields!(
        assert_data_table_fields,
        DataTable {
            id,
            columns,
            rows,
            selected_idx,
            scroll_offset,
            sort,
            has_focus,
            show_scrollbar,
            min_total_width,
            h_scroll,
            column_overrides,
            footer
        }
    );
    assert_fields!(
        assert_column_fields,
        Column {
            title,
            width,
            align
        }
    );
    assert_fields!(assert_data_row_fields, DataRow { cells, decoration });
    assert_fields!(
        assert_tab_bar_fields,
        TabBar {
            id,
            tabs,
            scroll_offset,
            right_segments,
            active_accent,
            show_tab_close,
            compact
        }
    );
    assert_fields!(
        assert_tab_item_fields,
        TabItem {
            label,
            is_active,
            is_dirty,
            is_preview,
            is_closable
        }
    );
    assert_fields!(
        assert_panel_fields,
        Panel {
            id,
            title,
            actions,
            accent,
            collapsed
        }
    );
    assert_fields!(
        assert_sidebar_panel_fields,
        SidebarPanel {
            id,
            toolbar,
            toolbar_height
        }
    );
    assert_fields_non_exhaustive!(
        assert_toolbar_fields,
        Toolbar {
            id,
            buttons,
            bg,
            focused_index
        }
    );
    assert_fields!(
        assert_panel_action_fields,
        PanelAction {
            id,
            icon,
            tooltip,
            is_active
        }
    );
    assert_fields!(
        assert_status_bar_fields,
        StatusBar {
            id,
            left_segments,
            right_segments
        }
    );
    assert_fields!(
        assert_status_bar_segment_fields,
        StatusBarSegment {
            text,
            fg,
            bg,
            bold,
            action_id
        }
    );
    assert_fields!(assert_color_fields, Color { r, g, b, a });
    assert_fields!(
        assert_styled_span_fields,
        StyledSpan {
            text,
            fg,
            bg,
            bold,
            italic,
            underline
        }
    );
    assert_fields!(assert_styled_text_fields, StyledText { spans });
    assert_fields!(assert_badge_fields, Badge { text, fg, bg });

    fn assert_toolbar_button(b: ToolbarButton) {
        match b {
            ToolbarButton::Action {
                id: _,
                label: _,
                icon: _,
                key_hint: _,
                enabled: _,
                is_active: _,
                tooltip: _,
                spec: _,
            } => {}
            ToolbarButton::Separator => {}
            ToolbarButton::Label { text: _, fg: _ } => {}
            ToolbarButton::Spacer => {}
            // `ToolbarButton` is `#[non_exhaustive]` -- see
            // `assert_tab_bar_event`'s matching comment.
            _ => {}
        }
    }
}
