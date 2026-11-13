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
//! like a layout-tree node) is paired with a guard in [`coverage`] — an
//! `assert_fields!` destructure or a hand-written exhaustive `match`, with
//! no `..`/wildcard arm for a type that isn't itself `#[non_exhaustive]`.
//! Adding a field to one of those structs, or a variant to one of those
//! enums, without also updating the matching entry here makes the
//! destructure/match non-exhaustive and fails to compile — but only under
//! a command that builds test targets (`cargo test`, or `cargo check
//! --tests`), since [`coverage`] is a `#[cfg(test)]` module and a bare
//! `cargo build`/`cargo check` skips it entirely. Two caveats:
//!
//! - The guards match on field/variant **names** only — every binding is
//!   `_`, so changing a field's or variant payload's *type* (e.g.
//!   `FormEvent::SliderChanged { value: f32 }` to `value: String`) compiles
//!   clean while the schema keeps emitting the old type.
//! - A good number of the real types below (`Button`, `Toolbar`,
//!   `TextInput`, `NumberInput`, `ComboBox`, `DatePicker`, `RadioGroup`,
//!   `Switch`, `Scrollbar`, `ScrollView`, `Editor`, ...) are themselves
//!   `#[non_exhaustive]` **structs**. A struct marked `#[non_exhaustive]`
//!   requires a trailing `..` in any pattern built outside its defining
//!   crate, so a *new field* is invisible to [`assert_fields_non_exhaustive!`]
//!   there. Every one of those structs closes that gap with a
//!   `serde_json::to_value` key-set assertion instead (see
//!   [`coverage::value_shape`]).
//! - `ToolbarButton`, `TabBarEvent`, `ChartKind` and `DrawOp` are
//!   `#[non_exhaustive]` **enums**. An enum marked `#[non_exhaustive]` only
//!   requires a wildcard arm for *unlisted* variants; a known variant's own
//!   fields (`ToolbarButton::Action { .. }`, `TabBarEvent::KeyPressed { .. }`)
//!   are still matched exhaustively by name, so a new field on one of
//!   those is caught the same as any other type here — only a brand new
//!   *variant* stays invisible to the match. Every one of those enums
//!   closes that gap too, by deserializing a deliberately unrecognised tag
//!   and parsing serde's own "expected one of ..." error message back into
//!   the real variant list (see [`coverage::variant_shape`]).
//!
//! The layout-tree node shapes (`"row"`/`"column"`/`"split"`/...) have no
//! single Rust type to destructure this way — they're dynamically-keyed
//! dicts `crate::wire` decodes field-by-field, not a `#[derive(Deserialize)]`
//! struct (see that module's own doc). Their dispatch keys are still pulled
//! straight from [`crate::layout::LEAF_KIND_KEYS`] and
//! [`crate::extra_paint::EXTRA_PRIMITIVE_KINDS`] below rather than
//! re-typed, so a new leaf kind or additive primitive always gets a schema
//! entry -- every registered kind, including every `EXTRA_PRIMITIVE_KINDS`
//! additive kind beyond `status_bar`, is narrowed field-by-field, not left
//! as an untyped placeholder.

use serde::Serialize;

use crate::extra_paint::EXTRA_PRIMITIVE_KINDS;
use crate::layout::LEAF_KIND_KEYS;

// ─── Wire format version ───────────────────────────────────────────────────

/// Version of the layout-tree / render-output wire shape a render
/// callback's returned `dict`/object carries in its optional top-level
/// `"wire_version"` key -- see [`crate::wire::check_wire_version`] for the
/// decode-time check against this constant.
///
/// # Mismatch policy
///
/// - **Absent key**: treated as version `1` -- every render callback
///   written before this field existed, in either binding, keeps decoding
///   unchanged.
/// - **`wire_version` <= this constant**: accepted. The wire shape only
///   ever grows additively (new optional keys, never a changed or removed
///   one -- see `quadraui/docs/PRIMITIVE_RULES.md` rule 8), so a tree
///   built against an older version is still fully decodable by a newer
///   host.
/// - **`wire_version` > this constant**: rejected with a `DecodeError`
///   naming both versions. A binding newer than this host may have
///   started emitting a shape this decoder doesn't know about yet;
///   accepting it silently risks silently dropping data the binding meant
///   to send, so this host raises instead of guessing.
///
/// Bump this constant only alongside a real additive change to the wire
/// shape, and only once both `quadraui-py` and `quadraui-node` emit the
/// new value.
pub const WIRE_VERSION: u32 = 1;

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

pub(crate) fn str_() -> TyJson {
    TyJson::Prim { name: "STR".into() }
}
pub(crate) fn int_() -> TyJson {
    TyJson::Prim { name: "INT".into() }
}
pub(crate) fn float_() -> TyJson {
    TyJson::Prim {
        name: "FLOAT".into(),
    }
}
pub(crate) fn bool_() -> TyJson {
    TyJson::Prim {
        name: "BOOL".into(),
    }
}
pub(crate) fn any_() -> TyJson {
    TyJson::Prim { name: "ANY".into() }
}
pub(crate) fn re(name: &str) -> TyJson {
    TyJson::Ref { name: name.into() }
}
pub(crate) fn nullable(inner: TyJson) -> TyJson {
    TyJson::Nullable {
        inner: Box::new(inner),
    }
}
pub(crate) fn list(inner: TyJson) -> TyJson {
    TyJson::List {
        inner: Box::new(inner),
    }
}
/// A `Dict[K, V]`/`Record<K, V>` wire shape -- e.g. `EditorDescriptor`'s
/// `diagnostic_gutter` field, keyed by the JSON-object string form of a
/// Rust `HashMap<usize, _>`'s integer key.
pub(crate) fn dictt(key: TyJson, value: TyJson) -> TyJson {
    TyJson::Dict {
        key: Box::new(key),
        value: Box::new(value),
    }
}
pub(crate) fn tup(items: Vec<TyJson>) -> TyJson {
    TyJson::Tuple { items }
}
pub(crate) fn lit(values: &[&str]) -> TyJson {
    TyJson::Lit {
        values: values.iter().map(|s| s.to_string()).collect(),
    }
}
pub(crate) fn uniont(items: Vec<TyJson>) -> TyJson {
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
pub(crate) fn field(name: &str, ty: TyJson) -> FieldJson {
    FieldJson {
        name: name.into(),
        ty,
        optional: false,
        doc: String::new(),
    }
}

/// [`field`], but the key may be omitted on the wire.
pub(crate) fn field_opt(name: &str, ty: TyJson) -> FieldJson {
    FieldJson {
        name: name.into(),
        ty,
        optional: true,
        doc: String::new(),
    }
}

/// Attaches a doc comment to an already-built [`FieldJson`].
pub(crate) fn with_doc(mut f: FieldJson, doc: &str) -> FieldJson {
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
pub(crate) fn obj(name: &str, fields: Vec<FieldJson>) -> DefJson {
    DefJson::Obj {
        name: name.into(),
        fields,
        doc: String::new(),
        discriminated: false,
        common: Vec::new(),
    }
}

pub(crate) fn obj_doc(name: &str, fields: Vec<FieldJson>, doc: &str) -> DefJson {
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
pub(crate) fn flat(name: &str, variants: &[(&str, TyJson)], common: Vec<FieldJson>) -> DefJson {
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

pub(crate) fn flat_doc(
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

pub(crate) fn alias(name: &str, ty: TyJson) -> DefJson {
    DefJson::Alias {
        name: name.into(),
        ty,
        doc: String::new(),
    }
}

pub(crate) fn alias_doc(name: &str, ty: TyJson, doc: &str) -> DefJson {
    DefJson::Alias {
        name: name.into(),
        ty,
        doc: doc.to_string(),
    }
}

pub(crate) fn union_of(name: &str, members: Vec<TyJson>) -> DefJson {
    alias(name, uniont(members))
}

pub(crate) fn union_of_doc(name: &str, members: Vec<TyJson>, doc: &str) -> DefJson {
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
    let rect = b.define(obj(
        "Rect",
        vec![
            field("x", float_()),
            field("y", float_()),
            field("width", float_()),
            field("height", float_()),
        ],
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
            field_opt("spec", nullable(button_spec_descriptor.clone())),
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

    // An optional `"wire_version"` sibling key, valid on *any* top-level
    // `RenderOutput` shape (every variant below, plus `RenderFrameDict`
    // further down) -- never on a nested child node. See
    // `crate::wire_schema::WIRE_VERSION`'s own doc for the accept/reject
    // policy `crate::wire::check_wire_version` checks it against.
    let wire_version_field = field_opt("wire_version", int_());

    // Every node-kind object below also accepts the three child-sizing
    // keys a `"row"`/`"column"` parent reads off its own children
    // (`crate::wire::layout_children_from_value`) -- `"size"` (a
    // `SectionSize`, defaulting to `EqualShare`), `"min"`/`"max"` (a
    // portable surface-native-pixel clamp, same unit as `padding`/
    // `margin`/`gap`). They're meaningless on a node that isn't itself a
    // row/column child, but harmless there too -- `layout_node_from_value`
    // never reads them except via that one path.
    let child_size_fields = || {
        vec![
            field_opt("size", re("SectionSize")),
            field_opt("min", float_()),
            field_opt("max", float_()),
        ]
    };

    // An optional `"style"` sibling key, valid on every node-kind object
    // below (`crate::wire::style_override_from_value` reads it off the
    // same outer node dict `child_size_fields`/`wire_version_field` read
    // theirs off) -- `crate::layout::StyleOverride`'s own wire shape: a
    // dict of a known subset of `Theme` colour tokens and `Style` metric
    // tokens, every one optional. `style_color_value` mirrors
    // `crate::host::theme_color_value`'s hex-string convenience on top of
    // the plain `Color` object shape.
    let style_color_value = b.define(alias_doc(
        "StyleColorValue",
        uniont(vec![color.clone(), str_()]),
        "A `\"style\"` colour token: a `Color` object, or a \"#rrggbb\"/\"#rrggbbaa\" hex string.",
    ));
    let style_value = b.define(obj_doc(
        "StyleValue",
        vec![
            field_opt("bg", style_color_value.clone()),
            field_opt("fg", style_color_value.clone()),
            field_opt("border", style_color_value.clone()),
            field_opt("accent", style_color_value.clone()),
            field_opt("surface_bg", style_color_value.clone()),
            field_opt("accent_bg", style_color_value.clone()),
            field_opt("padding", float_()),
            field_opt("corner_radius", float_()),
            field_opt("border_width", float_()),
        ],
        "A per-node \"style\" override, scoped to that node's own subtree: \"bg\"/\"fg\"/\"border\"/\"accent\"/\"surface_bg\"/\"accent_bg\" override Theme colour tokens, \"padding\"/\"corner_radius\"/\"border_width\" override Style metric tokens. \"surface_bg\"/\"accent_bg\" are the aliases a Button/List/Dialog/... fill needs -- plain \"bg\" only ever reaches Theme::background. See docs/bindings-wire-format.md for the full per-leaf token mapping.",
    ));

    // An optional `"class"` sibling key, valid everywhere `"style"` is
    // (`crate::wire::style_classes_from_value` reads it off the same outer
    // node dict) -- one or more names of style sets registered through a
    // binding's own `define_style`, merged left to right underneath the
    // node's own inline `"style"`. A bare string is split on whitespace,
    // the CSS `class=` convention this key is the analogue of.
    let class_value = b.define(alias_doc(
        "ClassValue",
        uniont(vec![str_(), list(str_())]),
        "One or more named style sets (see define_style) applied to this node's own subtree: a single name, a whitespace-separated list of names in one string, or an array of names. Merged left to right, then overridden by this node's own \"style\" key. A name no set was ever defined under is ignored.",
    ));
    let layout_row_node = b.define(obj(
        "LayoutRowNode",
        [
            vec![field("row", list(layout_node.clone()))],
            vec![
                field_opt("padding", padding_value.clone()),
                field_opt("margin", float_()),
                field_opt("gap", float_()),
                field_opt("justify", lit(&["start", "center", "end", "space_between"])),
                field_opt("align", lit(&["start", "center", "end", "stretch"])),
                wire_version_field.clone(),
                field_opt("style", style_value.clone()),
                field_opt("class", class_value.clone()),
            ],
            child_size_fields(),
        ]
        .concat(),
    ));
    let layout_column_node = b.define(obj(
        "LayoutColumnNode",
        [
            vec![field("column", list(layout_node.clone()))],
            vec![
                field_opt("padding", padding_value.clone()),
                field_opt("margin", float_()),
                field_opt("gap", float_()),
                field_opt("justify", lit(&["start", "center", "end", "space_between"])),
                field_opt("align", lit(&["start", "center", "end", "stretch"])),
                wire_version_field.clone(),
                field_opt("style", style_value.clone()),
                field_opt("class", class_value.clone()),
            ],
            child_size_fields(),
        ]
        .concat(),
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
        [
            vec![
                field("split", layout_split_payload),
                field_opt("padding", padding_value.clone()),
                field_opt("margin", float_()),
                field_opt("gap", float_()),
                wire_version_field.clone(),
                field_opt("style", style_value.clone()),
                field_opt("class", class_value.clone()),
            ],
            child_size_fields(),
        ]
        .concat(),
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
        [
            vec![
                field("tabs", layout_tabs_payload),
                field_opt("padding", padding_value.clone()),
                field_opt("margin", float_()),
                wire_version_field.clone(),
                field_opt("style", style_value.clone()),
                field_opt("class", class_value.clone()),
            ],
            child_size_fields(),
        ]
        .concat(),
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
        [
            vec![
                field("panel", layout_panel_payload),
                field_opt("padding", padding_value.clone()),
                field_opt("margin", float_()),
                wire_version_field.clone(),
                field_opt("style", style_value.clone()),
                field_opt("class", class_value.clone()),
            ],
            child_size_fields(),
        ]
        .concat(),
    ));
    let layout_sidebar_panel_payload = b.define(obj(
        "LayoutSidebarPanelPayload",
        vec![
            field("id", str_()),
            field_opt("toolbar", nullable(toolbar_descriptor.clone())),
            field_opt("toolbar_height", nullable(float_())),
            field("content", layout_node.clone()),
        ],
    ));
    let layout_sidebar_panel_node = b.define(obj(
        "LayoutSidebarPanelNode",
        [
            vec![
                field("sidebar_panel", layout_sidebar_panel_payload),
                wire_version_field.clone(),
                field_opt("style", style_value.clone()),
                field_opt("class", class_value.clone()),
            ],
            child_size_fields(),
        ]
        .concat(),
    ));

    let layout_scroll_payload = b.define(obj(
        "LayoutScrollPayload",
        vec![
            field("id", str_()),
            field_opt("axis", lit(&["vertical", "horizontal"])),
            field_opt("offset", float_()),
            field("content", layout_node.clone()),
        ],
    ));
    let layout_scroll_node = b.define(obj(
        "LayoutScrollNode",
        [
            vec![
                field("scroll", layout_scroll_payload),
                field_opt("padding", padding_value.clone()),
                field_opt("margin", float_()),
                wire_version_field.clone(),
                field_opt("style", style_value.clone()),
                field_opt("class", class_value.clone()),
            ],
            child_size_fields(),
        ]
        .concat(),
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
        [
            vec![
                field("split_tree", layout_split_tree_payload),
                wire_version_field.clone(),
                field_opt("style", style_value.clone()),
                field_opt("class", class_value.clone()),
            ],
            child_size_fields(),
        ]
        .concat(),
    ));

    // ── Additive primitive descriptors (`EXTRA_PRIMITIVE_KINDS`) ─────────
    // Every kind below `status_bar` is what one `EXTRA_PRIMITIVE_KINDS`
    // entry decodes to, field-by-field -- see this module's own doc for
    // the source-of-truth contract each entry carries (a matching
    // `coverage` guard) and why the layout-tree node shapes above don't
    // get the same treatment.
    let progress_bar_descriptor = b.define(obj(
        "ProgressBarDescriptor",
        vec![
            field("id", re("WidgetId")),
            field_opt("label", str_()),
            field_opt("value", nullable(float_())),
            field_opt("frame_idx", int_()),
            field_opt("cancellable", bool_()),
            field_opt("accent", nullable(color.clone())),
        ],
    ));
    let spinner_descriptor = b.define(obj(
        "SpinnerDescriptor",
        vec![
            field("id", re("WidgetId")),
            field_opt("label", str_()),
            field_opt("frame_idx", int_()),
            field_opt("accent", nullable(color.clone())),
        ],
    ));
    let image_source_bytes = b.define(obj("_ImageSourceBytes", vec![field("Bytes", list(int_()))]));
    let image_source_path = b.define(obj("_ImageSourcePath", vec![field("Path", str_())]));
    let image_source = b.define(union_of_doc(
        "ImageSource",
        vec![image_source_bytes, image_source_path],
        "`{\"Bytes\": [u8, ...]}` or `{\"Path\": \"...\"}` -- a `PathBuf` serializes as a plain string.",
    ));
    let image_fit = b.define(alias(
        "ImageFit",
        lit(&["Contain", "Cover", "Fill", "None"]),
    ));
    let image_descriptor = b.define(obj(
        "ImageDescriptor",
        vec![
            field("id", re("WidgetId")),
            field("source", image_source),
            field("intrinsic_size", nullable(tup(vec![int_(), int_()]))),
            field("fit", image_fit),
            field("fallback_text", str_()),
        ],
    ));
    let text_input_descriptor = b.define(obj(
        "TextInputDescriptor",
        vec![
            field("id", re("WidgetId")),
            field("lines", list(str_())),
            field_opt("cursor_line", int_()),
            field_opt("cursor_col", int_()),
            field_opt("placeholder", nullable(str_())),
            field_opt("scroll_offset", int_()),
            field_opt("scroll_col", int_()),
            field_opt("has_focus", bool_()),
        ],
    ));
    let number_input_descriptor = b.define(obj(
        "NumberInputDescriptor",
        vec![
            field("id", re("WidgetId")),
            field_opt("value", float_()),
            field("min", float_()),
            field("max", float_()),
            field_opt("step", float_()),
            field_opt("precision", int_()),
            field_opt("draft", nullable(str_())),
            field_opt("disabled", bool_()),
            field_opt("focused", bool_()),
        ],
    ));
    let combo_box_descriptor = b.define(obj(
        "ComboBoxDescriptor",
        vec![
            field("id", re("WidgetId")),
            field("options", list(str_())),
            field_opt("text", str_()),
            field_opt("committed", str_()),
            field_opt("open", bool_()),
            field_opt("highlighted", nullable(int_())),
            field_opt("disabled", bool_()),
            field_opt("focused", bool_()),
        ],
    ));
    let switch_descriptor = b.define(obj(
        "SwitchDescriptor",
        vec![
            field("id", re("WidgetId")),
            field_opt("value", bool_()),
            field_opt("disabled", bool_()),
            field_opt("focused", bool_()),
        ],
    ));
    let checkbox_descriptor = b.define(obj(
        "CheckboxDescriptor",
        vec![
            field("id", re("WidgetId")),
            field_opt("value", bool_()),
            field_opt("disabled", bool_()),
            field_opt("focused", bool_()),
        ],
    ));
    let slider_descriptor = b.define(obj(
        "SliderDescriptor",
        vec![
            field("id", re("WidgetId")),
            field("value", float_()),
            field("min", float_()),
            field("max", float_()),
            field_opt("step", float_()),
            field_opt("disabled", bool_()),
            field_opt("focused", bool_()),
        ],
    ));
    let select_descriptor = b.define(obj(
        "SelectDescriptor",
        vec![
            field("id", re("WidgetId")),
            field("options", list(str_())),
            field_opt("selected_idx", int_()),
            field_opt("disabled", bool_()),
            field_opt("focused", bool_()),
        ],
    ));
    let password_descriptor = b.define(obj_doc(
        "PasswordDescriptor",
        vec![
            field("id", re("WidgetId")),
            field_opt("value", str_()),
            field_opt("cursor", nullable(int_())),
            field_opt("has_focus", bool_()),
            field_opt("mask_char", str_()),
        ],
        "`mask_char` must decode to exactly one Unicode scalar value -- the Rust side holds it as a `char`, so a multi-character string here fails to decode.",
    ));
    // Mirrors `TextArea`'s own hand-written `Deserialize` in
    // `extra_paint.rs` (`id`/`value`/`placeholder`/`cursor`/`has_focus`/
    // `visible_rows`), NOT `TextInputDescriptor` above, even though both
    // are standalone-leaf siblings of it -- `TextArea::inner` is a plain
    // single-line-shaped `TextInput` buffer decoded from a flat `value`
    // string and a byte-offset `cursor`, not `TextInputDescriptor`'s own
    // multi-line `lines`/`cursor_line`/`cursor_col` convention.
    let textarea_descriptor = b.define(obj(
        "TextAreaDescriptor",
        vec![
            field("id", re("WidgetId")),
            field_opt("value", str_()),
            field_opt("placeholder", nullable(str_())),
            field_opt("cursor", nullable(int_())),
            field_opt("has_focus", bool_()),
            field_opt("visible_rows", int_()),
        ],
    ));
    // `source` is raw markdown text re-rendered through
    // `quadraui::compose::markdown::render_markdown_to_styled` on every
    // paint (see `extra_paint::Markdown::render`) -- there is no separate
    // `lines`/styling key here the way `TextInputDescriptor` has, since the
    // adapter is the single source of truth for turning `source` into
    // styled rows.
    let markdown_descriptor = b.define(obj(
        "MarkdownDescriptor",
        vec![
            field("id", re("WidgetId")),
            field("source", str_()),
            field_opt("scroll_offset", int_()),
        ],
    ));
    let scroll_axis = b.define(alias("ScrollAxis", lit(&["Vertical", "Horizontal"])));
    let scrollbar_descriptor = b.define(obj(
        "ScrollbarDescriptor",
        vec![
            field("id", re("WidgetId")),
            field("axis", scroll_axis.clone()),
            field("track", rect.clone()),
            field("thumb_start", float_()),
            field("thumb_len", float_()),
            field_opt("hovered", bool_()),
            field_opt("dragging", bool_()),
        ],
    ));
    let scroll_view_descriptor = b.define(obj(
        "ScrollViewDescriptor",
        vec![
            field("id", re("WidgetId")),
            field_opt("axis", scroll_axis),
            field("viewport", rect.clone()),
            field("content_len", float_()),
            field_opt("scroll_offset", float_()),
            field_opt("focused", bool_()),
        ],
    ));
    let command_line_descriptor = b.define(obj(
        "CommandLineDescriptor",
        vec![
            field("id", re("WidgetId")),
            field("text", str_()),
            field_opt("cursor_offset", nullable(int_())),
            field_opt("right_align", bool_()),
        ],
    ));
    let command_center_descriptor = b.define(obj(
        "CommandCenterDescriptor",
        vec![
            field("id", re("WidgetId")),
            field("back_enabled", bool_()),
            field("forward_enabled", bool_()),
            field_opt("search_label", str_()),
        ],
    ));
    let radio_option_descriptor = b.define(obj(
        "RadioOptionDescriptor",
        vec![
            field("id", re("WidgetId")),
            field("label", str_()),
            field_opt("disabled", bool_()),
        ],
    ));
    let radio_group_descriptor = b.define(obj(
        "RadioGroupDescriptor",
        vec![
            field("id", re("WidgetId")),
            field("options", list(radio_option_descriptor)),
            field_opt("selected", nullable(int_())),
            field_opt("focused", bool_()),
        ],
    ));
    let message_row_descriptor = b.define(obj(
        "MessageRowDescriptor",
        vec![
            field("text", str_()),
            field("fg", color.clone()),
            field_opt("indent", float_()),
            field_opt("spans", list(styled_span.clone())),
            field_opt("scale", float_()),
        ],
    ));
    let message_list_descriptor = b.define(obj(
        "MessageListDescriptor",
        vec![
            field("id", re("WidgetId")),
            field("rows", list(message_row_descriptor)),
            field_opt("scroll_top", int_()),
        ],
    ));
    let weekday = b.define(alias(
        "Weekday",
        lit(&[
            "Sunday",
            "Monday",
            "Tuesday",
            "Wednesday",
            "Thursday",
            "Friday",
            "Saturday",
        ]),
    ));
    let date_format = b.define(alias("DateFormat", lit(&["Iso8601", "UsSlash", "EuSlash"])));
    let date_locale = b.define(obj(
        "DateLocale",
        vec![
            field("first_weekday", weekday),
            field("format", date_format),
        ],
    ));
    let time_of_day = b.define(obj(
        "TimeOfDay",
        vec![field("hour", int_()), field("minute", int_())],
    ));
    let date_picker_descriptor = b.define(obj(
        "DatePickerDescriptor",
        vec![
            field("id", re("WidgetId")),
            field("visible_month", re("CalendarDate")),
            field_opt("selected", nullable(re("CalendarDate"))),
            field("focused", re("CalendarDate")),
            field("locale", date_locale),
            field_opt("time", nullable(time_of_day)),
            field_opt("has_focus", bool_()),
        ],
    ));
    let chart_kind = b.define(alias(
        "ChartKind",
        lit(&["Sparkline", "Line", "Bar", "BarGrouped"]),
    ));
    let series_descriptor = b.define(obj(
        "SeriesDescriptor",
        vec![
            field("label", str_()),
            field("data", list(float_())),
            field_opt("color", nullable(color.clone())),
            field_opt("fill", bool_()),
        ],
    ));
    let chart_descriptor = b.define(obj(
        "ChartDescriptor",
        vec![
            field("id", re("WidgetId")),
            field("kind", chart_kind),
            field("series", list(series_descriptor)),
            field_opt("x_label", nullable(str_())),
            field_opt("y_label", nullable(str_())),
            field_opt("y_range", nullable(tup(vec![float_(), float_()]))),
            field_opt("x_range", nullable(tup(vec![float_(), float_()]))),
            field_opt("show_legend", bool_()),
            field_opt("y_ticks", nullable(int_())),
            field_opt("x_ticks", nullable(int_())),
            field_opt("show_grid", bool_()),
        ],
    ));
    let stage_status = b.define(alias(
        "StageStatus",
        lit(&["Pending", "Active", "Done", "Failed", "Skipped", "Stale"]),
    ));
    let pipeline_stage_descriptor = b.define(obj(
        "PipelineStageDescriptor",
        vec![
            field("label", str_()),
            field("status", stage_status),
            field_opt("action", nullable(str_())),
        ],
    ));
    let pipeline_view_descriptor = b.define(obj(
        "PipelineViewDescriptor",
        vec![
            field("id", re("WidgetId")),
            field("stages", list(pipeline_stage_descriptor)),
            field_opt("focused_stage", nullable(int_())),
        ],
    ));
    let badge_status = b.define(alias(
        "BadgeStatus",
        lit(&["Pending", "Running", "Passed", "Warning", "Blocked"]),
    ));
    let card_badge_descriptor = b.define(obj(
        "CardBadgeDescriptor",
        vec![field("label", str_()), field("status", badge_status)],
    ));
    let board_card_descriptor = b.define(obj(
        "BoardCardDescriptor",
        vec![
            field("id", re("WidgetId")),
            field("title", str_()),
            field_opt("labels", list(str_())),
            field_opt("badges", list(card_badge_descriptor)),
            field_opt("hint", nullable(str_())),
        ],
    ));
    let board_column_descriptor = b.define(obj(
        "BoardColumnDescriptor",
        vec![
            field("id", re("WidgetId")),
            field("title", str_()),
            field("cards", list(board_card_descriptor)),
            field_opt("scroll_offset", int_()),
        ],
    ));
    let board_model_descriptor = b.define(obj(
        "BoardModelDescriptor",
        vec![
            field("id", re("WidgetId")),
            field("columns", list(board_column_descriptor)),
            field_opt("selected_card_id", nullable(re("WidgetId"))),
            field_opt("col_scroll_offset", int_()),
        ],
    ));
    let minimap_line_descriptor = b.define(obj(
        "MinimapLineDescriptor",
        vec![field("text", str_()), field("line_idx", int_())],
    ));
    let minimap_span_descriptor = b.define(obj(
        "MinimapSpanDescriptor",
        vec![
            field("line_idx", int_()),
            field("start_col", int_()),
            field("end_col", int_()),
            field("color", color.clone()),
        ],
    ));
    let minimap_descriptor = b.define(obj(
        "MinimapDescriptor",
        vec![
            field("id", re("WidgetId")),
            field("lines", list(minimap_line_descriptor)),
            field("syntax_spans", list(minimap_span_descriptor)),
            field("visible_row_start", int_()),
            field("visible_row_count", int_()),
            field("total_buffer_lines", int_()),
        ],
    ));
    let activity_item_descriptor = b.define(obj(
        "ActivityItemDescriptor",
        vec![
            field("id", re("WidgetId")),
            field("icon", icon.clone()),
            field_opt("tooltip", str_()),
            field_opt("is_active", bool_()),
            field_opt("is_keyboard_selected", bool_()),
        ],
    ));
    let activity_bar_descriptor = b.define(obj(
        "ActivityBarDescriptor",
        vec![
            field("id", re("WidgetId")),
            field("top_items", list(activity_item_descriptor.clone())),
            field_opt("bottom_items", list(activity_item_descriptor)),
            field_opt("active_accent", nullable(color.clone())),
            field_opt("selection_bg", nullable(color.clone())),
            field_opt("is_keyboard_focused", bool_()),
        ],
    ));
    let palette_mode = b.define(alias("PaletteMode", lit(&["List", "Input"])));
    let palette_item_descriptor = b.define(obj(
        "PaletteItemDescriptor",
        vec![
            field("text", styled_text.clone()),
            field_opt("detail", nullable(styled_text.clone())),
            field_opt("icon", nullable(icon.clone())),
            field_opt("match_positions", list(int_())),
            field_opt("depth", int_()),
            field_opt("expandable", bool_()),
            field_opt("expanded", bool_()),
        ],
    ));
    let palette_preview_descriptor = b.define(obj(
        "PalettePreviewDescriptor",
        vec![
            field("lines", list(styled_text.clone())),
            field_opt("title", nullable(str_())),
            field_opt("scroll_offset", int_()),
            field_opt("highlight_line", nullable(int_())),
        ],
    ));
    let palette_descriptor = b.define(obj(
        "PaletteDescriptor",
        vec![
            field("id", re("WidgetId")),
            field("title", str_()),
            field("query", str_()),
            field_opt("query_cursor", int_()),
            field("items", list(palette_item_descriptor)),
            field("selected_idx", int_()),
            field_opt("scroll_offset", int_()),
            field_opt("total_count", int_()),
            field_opt("has_focus", bool_()),
            field_opt("show_query", bool_()),
            field_opt("create_label", nullable(str_())),
            field_opt("preview", nullable(palette_preview_descriptor)),
            field_opt("mode", palette_mode),
        ],
    ));
    let terminal_cursor_shape = b.define(alias(
        "TerminalCursorShape",
        lit(&["Block", "Underline", "Bar"]),
    ));
    let terminal_cell_descriptor = b.define(obj(
        "TerminalCellDescriptor",
        vec![
            field("text", str_()),
            field("fg", color.clone()),
            field("bg", color.clone()),
            field_opt("bold", bool_()),
            field_opt("italic", bool_()),
            field_opt("underline", bool_()),
            field_opt("dim", bool_()),
            field_opt("selected", bool_()),
            field_opt("is_cursor", bool_()),
            field_opt("cursor_shape", terminal_cursor_shape),
            field_opt("cursor_blinking", bool_()),
            field_opt("is_find_match", bool_()),
            field_opt("is_find_active", bool_()),
        ],
    ));
    let terminal_scrollbar_descriptor = b.define(obj(
        "TerminalScrollbarDescriptor",
        vec![
            field("total_lines", int_()),
            field("visible_lines", int_()),
            field("scroll_offset", int_()),
            field_opt("inverted", bool_()),
            field_opt("width", nullable(int_())),
        ],
    ));
    let terminal_descriptor = b.define(obj(
        "TerminalDescriptor",
        vec![
            field("id", re("WidgetId")),
            field("cells", list(list(terminal_cell_descriptor))),
            field_opt("scrollbar", nullable(terminal_scrollbar_descriptor)),
        ],
    ));
    let canvas_font_role = b.define(alias("CanvasFontRole", lit(&["Chrome", "Editor"])));
    let draw_op_rect = b.define(obj(
        "_DrawOpRect",
        vec![field("rect", rect.clone()), field("color", color.clone())],
    ));
    let draw_op_rounded_rect = b.define(obj(
        "_DrawOpRoundedRect",
        vec![
            field("rect", rect.clone()),
            field("radius", float_()),
            field("color", color.clone()),
        ],
    ));
    let draw_op_line = b.define(obj(
        "_DrawOpLine",
        vec![
            field("from", point.clone()),
            field("to", point.clone()),
            field("color", color.clone()),
            field("stroke_width", float_()),
        ],
    ));
    let draw_op_path = b.define(obj(
        "_DrawOpPath",
        vec![
            field("points", list(point.clone())),
            field("color", color.clone()),
            field("stroke_width", float_()),
            field("closed", bool_()),
        ],
    ));
    let draw_op_text_run = b.define(obj(
        "_DrawOpTextRun",
        vec![
            field("rect", rect.clone()),
            field("text", str_()),
            field("color", color.clone()),
        ],
    ));
    let draw_op_image = b.define(obj(
        "_DrawOpImage",
        vec![
            field("rect", rect.clone()),
            field("image", image_descriptor.clone()),
        ],
    ));
    let draw_op_push_clip = b.define(obj("_DrawOpPushClip", vec![field("rect", rect.clone())]));
    let draw_op_polygon = b.define(obj(
        "_DrawOpPolygon",
        vec![
            field("points", list(point.clone())),
            field("color", color.clone()),
        ],
    ));
    let draw_op_ellipse = b.define(obj(
        "_DrawOpEllipse",
        vec![
            field("center", point.clone()),
            field("radius_x", float_()),
            field("radius_y", float_()),
            field("color", color.clone()),
        ],
    ));
    let draw_op_arc = b.define(obj(
        "_DrawOpArc",
        vec![
            field("center", point.clone()),
            field("radius", float_()),
            field("start_angle", float_()),
            field("end_angle", float_()),
            field("color", color.clone()),
            field("stroke_width", float_()),
        ],
    ));
    let draw_op_bezier = b.define(obj(
        "_DrawOpBezier",
        vec![
            field("from", point.clone()),
            field("control1", point.clone()),
            field("control2", point.clone()),
            field("to", point.clone()),
            field("color", color.clone()),
            field("stroke_width", float_()),
        ],
    ));
    let draw_op_dashed_line = b.define(obj(
        "_DrawOpDashedLine",
        vec![
            field("from", point.clone()),
            field("to", point.clone()),
            field("color", color.clone()),
            field("stroke_width", float_()),
            field("dash_len", float_()),
            field("gap_len", float_()),
        ],
    ));
    let draw_op_stroke_rect = b.define(obj(
        "_DrawOpStrokeRect",
        vec![
            field("rect", rect.clone()),
            field("color", color.clone()),
            field("stroke_width", float_()),
        ],
    ));
    let draw_op_styled_text_run = b.define(obj(
        "_DrawOpStyledTextRun",
        vec![
            field("rect", rect.clone()),
            field("text", str_()),
            field("color", color.clone()),
            field("role", canvas_font_role),
            field("italic", bool_()),
            field_opt("size", nullable(float_())),
        ],
    ));
    let draw_op_dict = b.define(flat(
        "_DrawOpDict",
        &[
            ("Rect", draw_op_rect),
            ("RoundedRect", draw_op_rounded_rect),
            ("Line", draw_op_line),
            ("Path", draw_op_path),
            ("TextRun", draw_op_text_run),
            ("Image", draw_op_image),
            ("PushClip", draw_op_push_clip),
            ("Polygon", draw_op_polygon),
            ("Ellipse", draw_op_ellipse),
            ("Arc", draw_op_arc),
            ("Bezier", draw_op_bezier),
            ("DashedLine", draw_op_dashed_line),
            ("StrokeRect", draw_op_stroke_rect),
            ("StyledTextRun", draw_op_styled_text_run),
        ],
        vec![],
    ));
    let draw_op = b.define(alias_doc(
        "DrawOp",
        uniont(vec![lit(&["PopClip"]), draw_op_dict]),
        "A single `Canvas` paint instruction -- `\"PopClip\"` (bare string, no payload) or `{\"<Variant>\": {...}}`.",
    ));
    let canvas_descriptor = b.define(obj(
        "CanvasDescriptor",
        vec![field("id", re("WidgetId")), field("ops", list(draw_op))],
    ));
    let diagnostic_severity = b.define(alias(
        "DiagnosticSeverity",
        lit(&["Error", "Warning", "Information", "Hint"]),
    ));
    let git_line_status = b.define(alias(
        "GitLineStatus",
        lit(&["Added", "Modified", "Deleted"]),
    ));
    let editor_diff_line = b.define(alias(
        "EditorDiffLine",
        lit(&["Same", "Added", "Removed", "Padding"]),
    ));
    let cursor_shape = b.define(alias("CursorShape", lit(&["Block", "Bar", "Underline"])));
    let selection_kind = b.define(alias("SelectionKind", lit(&["Char", "Line", "Block"])));
    let cursor_pos = b.define(obj(
        "CursorPos",
        vec![field("view_line", int_()), field("col", int_())],
    ));
    let editor_cursor = b.define(obj(
        "EditorCursor",
        vec![
            field("pos", cursor_pos.clone()),
            field("shape", cursor_shape),
        ],
    ));
    let editor_selection = b.define(obj(
        "EditorSelection",
        vec![
            field("kind", selection_kind),
            field("start_line", int_()),
            field("start_col", int_()),
            field("end_line", int_()),
            field("end_col", int_()),
        ],
    ));
    let editor_style = b.define(obj(
        "EditorStyle",
        vec![
            field("fg", color.clone()),
            field("bg", nullable(color.clone())),
            field_opt("bold", bool_()),
            field_opt("italic", bool_()),
            field("font_scale", float_()),
        ],
    ));
    let editor_styled_span = b.define(obj(
        "EditorStyledSpan",
        vec![
            field("start_byte", int_()),
            field("end_byte", int_()),
            field("style", editor_style),
        ],
    ));
    let diagnostic_mark = b.define(obj(
        "DiagnosticMark",
        vec![
            field("start_col", int_()),
            field("end_col", int_()),
            field("severity", diagnostic_severity.clone()),
            field("message", str_()),
        ],
    ));
    let spell_mark = b.define(obj(
        "SpellMark",
        vec![field("start_col", int_()), field("end_col", int_())],
    ));
    let editor_line = b.define(obj(
        "EditorLine",
        vec![
            field("raw_text", str_()),
            field("gutter_text", str_()),
            field("spans", list(editor_styled_span)),
            field("line_idx", int_()),
            field_opt("is_current_line", bool_()),
            field_opt("is_fold_header", bool_()),
            field_opt("folded_line_count", int_()),
            field_opt("git_diff", nullable(git_line_status)),
            field_opt("diff_status", nullable(editor_diff_line)),
            field_opt("diagnostics", list(diagnostic_mark)),
            field_opt("spell_errors", list(spell_mark)),
            field_opt("is_breakpoint", bool_()),
            field_opt("is_conditional_bp", bool_()),
            field_opt("is_dap_current", bool_()),
            field_opt("is_wrap_continuation", bool_()),
            field_opt("segment_col_offset", int_()),
            field_opt("annotation", nullable(str_())),
            field_opt("ghost_suffix", nullable(str_())),
            field_opt("is_ghost_continuation", bool_()),
            field_opt("indent_guides", list(int_())),
            field_opt("colorcolumns", list(int_())),
        ],
    ));
    let editor_descriptor = b.define(obj(
        "EditorDescriptor",
        vec![
            field("id", re("WidgetId")),
            field("rect", rect.clone()),
            field("lines", list(editor_line)),
            field_opt("cursor", nullable(editor_cursor)),
            field_opt("extra_cursors", list(cursor_pos)),
            field_opt("selection", nullable(editor_selection.clone())),
            field_opt("extra_selections", list(editor_selection.clone())),
            field_opt("yank_highlight", nullable(editor_selection)),
            field("scroll_top", int_()),
            field_opt("scroll_left", int_()),
            field("total_lines", int_()),
            field_opt("max_col", int_()),
            field_opt("gutter_char_width", int_()),
            field("is_active", bool_()),
            field("show_active_bg", bool_()),
            field("has_git_diff", bool_()),
            field("has_breakpoints", bool_()),
            field_opt("diagnostic_gutter", dictt(str_(), diagnostic_severity)),
            field_opt("code_action_lines", list(int_())),
            field_opt("bracket_match_positions", list(tup(vec![int_(), int_()]))),
            field_opt("active_indent_col", nullable(int_())),
            field("tabstop", int_()),
            field("cursorline", bool_()),
            field_opt("lightbulb_glyph", str_()),
        ],
    ));
    let find_replace_click_target_find_input =
        b.define(obj("_FrClickFindInput", vec![field("FindInput", int_())]));
    let find_replace_click_target_replace_input = b.define(obj(
        "_FrClickReplaceInput",
        vec![field("ReplaceInput", int_())],
    ));
    let find_replace_click_target = b.define(union_of(
        "FindReplaceClickTarget",
        vec![
            lit(&[
                "Chevron",
                "ToggleCase",
                "ToggleWholeWord",
                "ToggleRegex",
                "PrevMatch",
                "NextMatch",
                "ToggleInSelection",
                "Close",
                "TogglePreserveCase",
                "ReplaceCurrent",
                "ReplaceAll",
            ]),
            find_replace_click_target_find_input,
            find_replace_click_target_replace_input,
        ],
    ));
    let fr_hit_region = b.define(obj(
        "FrHitRegion",
        vec![
            field("col", int_()),
            field("row", int_()),
            field("width", int_()),
        ],
    ));
    let find_replace_panel_descriptor = b.define(obj(
        "FindReplacePanelDescriptor",
        vec![
            field("query", str_()),
            field_opt("replacement", str_()),
            field_opt("show_replace", bool_()),
            field_opt("focus", int_()),
            field_opt("cursor", int_()),
            field_opt("sel_anchor", nullable(int_())),
            field_opt("match_info", str_()),
            field_opt("case_sensitive", bool_()),
            field_opt("whole_word", bool_()),
            field_opt("use_regex", bool_()),
            field_opt("preserve_case", bool_()),
            field_opt("in_selection", bool_()),
            field("group_bounds", rect.clone()),
            field_opt("panel_width", int_()),
            field_opt("replace_one_glyph", str_()),
            field_opt("replace_all_glyph", str_()),
            field_opt(
                "hit_regions",
                list(tup(vec![fr_hit_region, find_replace_click_target])),
            ),
        ],
    ));
    let control_width_fixed = b.define(obj("_ControlWidthFixed", vec![field("Fixed", float_())]));
    let control_width = b.define(union_of(
        "ControlWidth",
        vec![lit(&["Fill", "Content"]), control_width_fixed],
    ));
    let label_position = b.define(alias(
        "LabelPosition",
        lit(&["Left", "Top", "Inline", "Hidden"]),
    ));
    let label_align = b.define(alias("LabelAlign", lit(&["Start", "Center", "End"])));
    let row_align = b.define(alias(
        "RowAlign",
        lit(&["Start", "Center", "End", "Stretch"]),
    ));
    let form_spacing = b.define(obj(
        "FormSpacing",
        vec![
            field_opt("padding", float_()),
            field_opt("item_gap", nullable(float_())),
            field_opt("row_align", row_align.clone()),
        ],
    ));
    let validation_state_error =
        b.define(obj("_ValidationStateError", vec![field("Error", str_())]));
    let validation_state_warning = b.define(obj(
        "_ValidationStateWarning",
        vec![field("Warning", str_())],
    ));
    let validation_state = b.define(union_of_doc(
        "ValidationState",
        vec![validation_state_error, validation_state_warning],
        "`{\"Error\": \"...\"}` or `{\"Warning\": \"...\"}`.",
    ));
    let toggle_group_item = b.define(obj(
        "ToggleGroupItem",
        vec![
            field("id", re("WidgetId")),
            field("label", str_()),
            field("value", bool_()),
        ],
    ));
    let button_row_item = b.define(obj(
        "ButtonRowItem",
        vec![
            field("id", re("WidgetId")),
            field("label", str_()),
            field_opt("disabled", bool_()),
            field_opt("icon", nullable(icon.clone())),
            field_opt("spec", nullable(button_spec_descriptor.clone())),
        ],
    ));
    let field_kind_text_input = b.define(obj(
        "_FieldKindTextInput",
        vec![
            field("value", str_()),
            field_opt("placeholder", str_()),
            field_opt("cursor", nullable(int_())),
            field_opt("selection_anchor", nullable(int_())),
        ],
    ));
    let field_kind_read_only = b.define(obj(
        "_FieldKindReadOnly",
        vec![field("value", styled_text.clone())],
    ));
    let field_kind_slider = b.define(obj(
        "_FieldKindSlider",
        vec![
            field("value", float_()),
            field("min", float_()),
            field("max", float_()),
            field_opt("step", float_()),
        ],
    ));
    let field_kind_color_picker = b.define(obj(
        "_FieldKindColorPicker",
        vec![field("value", color.clone())],
    ));
    let field_kind_dropdown = b.define(obj(
        "_FieldKindDropdown",
        vec![
            field("options", list(styled_text.clone())),
            field("selected_idx", int_()),
        ],
    ));
    let field_kind_text_area = b.define(obj(
        "_FieldKindTextArea",
        vec![
            field("value", str_()),
            field_opt("placeholder", str_()),
            field_opt("cursor", nullable(int_())),
            field_opt("visible_rows", int_()),
        ],
    ));
    let field_kind_password_input = b.define(obj(
        "_FieldKindPasswordInput",
        vec![
            field("value", str_()),
            field_opt("placeholder", str_()),
            field_opt("cursor", nullable(int_())),
            field_opt("mask_char", str_()),
        ],
    ));
    let field_kind_segmented_control = b.define(obj(
        "_FieldKindSegmentedControl",
        vec![
            field("options", list(str_())),
            field("selected_idx", int_()),
        ],
    ));
    let field_kind_toggle_group = b.define(obj(
        "_FieldKindToggleGroup",
        vec![field("toggles", list(toggle_group_item))],
    ));
    let field_kind_button_row = b.define(obj(
        "_FieldKindButtonRow",
        vec![field("buttons", list(button_row_item))],
    ));
    let field_kind_toolbar = b.define(obj(
        "_FieldKindToolbar",
        vec![field("Toolbar", toolbar_descriptor.clone())],
    ));
    let field_kind_date_picker_wrapper = b.define(obj(
        "_FieldKindDatePicker",
        vec![field("DatePicker", date_picker_descriptor.clone())],
    ));
    let field_kind_combo_box_wrapper = b.define(obj(
        "_FieldKindComboBox",
        vec![field("ComboBox", combo_box_descriptor.clone())],
    ));
    let field_kind_number_input_wrapper = b.define(obj(
        "_FieldKindNumberInput",
        vec![field("NumberInput", number_input_descriptor.clone())],
    ));
    let field_kind_radio_group_wrapper = b.define(obj(
        "_FieldKindRadioGroup",
        vec![field("RadioGroup", radio_group_descriptor.clone())],
    ));
    let field_kind_toggle = b.define(obj("_FieldKindToggle", vec![field("value", bool_())]));
    let field_kind_dict = b.define(flat(
        "_FieldKindDict",
        &[
            ("Toggle", field_kind_toggle),
            ("TextInput", field_kind_text_input),
            ("ReadOnly", field_kind_read_only),
            ("Slider", field_kind_slider),
            ("ColorPicker", field_kind_color_picker),
            ("Dropdown", field_kind_dropdown),
            ("TextArea", field_kind_text_area),
            ("PasswordInput", field_kind_password_input),
            ("SegmentedControl", field_kind_segmented_control),
            ("ToggleGroup", field_kind_toggle_group),
            ("ButtonRow", field_kind_button_row),
        ],
        vec![],
    ));
    let field_kind = b.define(alias_doc(
        "FieldKind",
        uniont(vec![
            lit(&["Label", "Button"]),
            field_kind_dict,
            field_kind_toolbar,
            field_kind_date_picker_wrapper,
            field_kind_combo_box_wrapper,
            field_kind_number_input_wrapper,
            field_kind_radio_group_wrapper,
        ]),
        "A `FormField`'s kind -- `\"Label\"`/`\"Button\"` (bare strings, no payload), a tagged variant from `_FieldKindDict`, or one of the embedded-primitive wrappers (`{\"Toolbar\": ...}`, `{\"DatePicker\": ...}`, `{\"ComboBox\": ...}`, `{\"NumberInput\": ...}`, `{\"RadioGroup\": ...}`).",
    ));
    let form_field_descriptor = b.define(obj(
        "FormFieldDescriptor",
        vec![
            field("id", re("WidgetId")),
            field("label", styled_text.clone()),
            field("kind", field_kind),
            field_opt("hint", styled_text.clone()),
            field_opt("disabled", bool_()),
            field_opt("validation", nullable(validation_state)),
            field_opt("label_position", nullable(label_position.clone())),
            field_opt("label_align", nullable(label_align.clone())),
            field_opt("label_width", nullable(float_())),
            field_opt("tab_index", nullable(int_())),
            field_opt("control_width", nullable(control_width.clone())),
            field_opt("min_control_width", nullable(float_())),
            field_opt("row_align", nullable(row_align)),
        ],
    ));
    let form_descriptor = b.define(obj(
        "FormDescriptor",
        vec![
            field("id", re("WidgetId")),
            field("fields", list(form_field_descriptor)),
            field_opt("focused_field", nullable(re("WidgetId"))),
            field_opt("scroll_offset", int_()),
            field_opt("has_focus", bool_()),
            field_opt("label_position", label_position),
            field_opt("label_align", label_align),
            field_opt("label_width", nullable(float_())),
            field_opt("control_width", control_width),
            field_opt("min_control_width", nullable(float_())),
            field_opt("spacing", form_spacing),
        ],
    ));
    let msv_axis = b.define(alias("MsvAxis", lit(&["Vertical", "Horizontal"])));
    let scroll_mode = b.define(alias("ScrollMode", lit(&["PerSection", "WholePanel"])));
    let section_size_fixed = b.define(obj("_SectionSizeFixed", vec![field("Fixed", int_())]));
    let section_size_percent =
        b.define(obj("_SectionSizePercent", vec![field("Percent", float_())]));
    let section_size_weight = b.define(obj("_SectionSizeWeight", vec![field("Weight", float_())]));
    let section_size = b.define(union_of(
        "SectionSize",
        vec![
            lit(&["Content", "EqualShare"]),
            section_size_fixed,
            section_size_percent,
            section_size_weight,
        ],
    ));
    let header_action = b.define(obj(
        "HeaderAction",
        vec![
            field("id", str_()),
            field("icon", icon.clone()),
            field("tooltip", nullable(str_())),
            field_opt("enabled", bool_()),
        ],
    ));
    let section_header = b.define(obj(
        "SectionHeader",
        vec![
            field("icon", nullable(icon.clone())),
            field("title", styled_text.clone()),
            field("badge", nullable(styled_text.clone())),
            field_opt("actions", list(header_action.clone())),
            field_opt("show_chevron", bool_()),
        ],
    ));
    let empty_body = b.define(obj(
        "EmptyBody",
        vec![
            field("icon", nullable(icon.clone())),
            field("text", styled_text.clone()),
            field("hint", nullable(styled_text.clone())),
            field("action", nullable(header_action.clone())),
        ],
    ));
    let inline_input = b.define(obj(
        "InlineInput",
        vec![
            field("id", re("WidgetId")),
            field("text", str_()),
            field_opt("caret", int_()),
            field("placeholder", nullable(str_())),
            field_opt("has_focus", bool_()),
        ],
    ));
    let section_aux_input = b.define(obj(
        "_SectionAuxInput",
        vec![field("Input", inline_input.clone())],
    ));
    let section_aux_toolbar = b.define(obj(
        "_SectionAuxToolbar",
        vec![field("Toolbar", list(header_action))],
    ));
    let section_aux_search = b.define(obj(
        "_SectionAuxSearch",
        vec![field("Search", inline_input)],
    ));
    let section_aux_custom = b.define(obj(
        "_SectionAuxCustom",
        vec![field("Custom", re("WidgetId"))],
    ));
    let section_aux = b.define(union_of(
        "SectionAux",
        vec![
            section_aux_input,
            section_aux_toolbar,
            section_aux_search,
            section_aux_custom,
        ],
    ));
    let section_body_tree = b.define(obj(
        "_SectionBodyTree",
        vec![field("Tree", tree_view_descriptor.clone())],
    ));
    let section_body_list = b.define(obj(
        "_SectionBodyList",
        vec![field("List", list_view_descriptor.clone())],
    ));
    let section_body_form = b.define(obj(
        "_SectionBodyForm",
        vec![field("Form", form_descriptor.clone())],
    ));
    let section_body_terminal = b.define(obj(
        "_SectionBodyTerminal",
        vec![field("Terminal", terminal_descriptor.clone())],
    ));
    let section_body_message_list = b.define(obj(
        "_SectionBodyMessageList",
        vec![field("MessageList", message_list_descriptor.clone())],
    ));
    let section_body_text = b.define(obj(
        "_SectionBodyText",
        vec![field("Text", list(styled_text.clone()))],
    ));
    let section_body_chart = b.define(obj(
        "_SectionBodyChart",
        vec![field("Chart", chart_descriptor.clone())],
    ));
    let section_body_empty = b.define(obj("_SectionBodyEmpty", vec![field("Empty", empty_body)]));
    let section_body_custom = b.define(obj(
        "_SectionBodyCustom",
        vec![field("Custom", re("WidgetId"))],
    ));
    let section_body = b.define(union_of(
        "SectionBody",
        vec![
            section_body_tree,
            section_body_list,
            section_body_form,
            section_body_terminal,
            section_body_message_list,
            section_body_text,
            section_body_chart,
            section_body_empty,
            section_body_custom,
        ],
    ));
    let section = b.define(obj(
        "Section",
        vec![
            field("id", str_()),
            field("header", section_header),
            field("body", section_body),
            field_opt("aux", nullable(section_aux)),
            field("size", section_size),
            field_opt("collapsed", bool_()),
            field_opt("min_size", nullable(int_())),
            field_opt("max_size", nullable(int_())),
        ],
    ));
    let multi_section_view_descriptor = b.define(obj(
        "MultiSectionViewDescriptor",
        vec![
            field("id", re("WidgetId")),
            field("sections", list(section)),
            field_opt("active_section", nullable(int_())),
            field("axis", msv_axis),
            field_opt("allow_resize", bool_()),
            field_opt("allow_collapse", bool_()),
            field("scroll_mode", scroll_mode),
            field_opt("has_focus", bool_()),
            field_opt("panel_scroll", float_()),
        ],
    ));
    let key_binding_literal = b.define(obj("_KeyBindingLiteral", vec![field("Literal", str_())]));
    let key_binding = b.define(union_of_doc(
        "KeyBinding",
        vec![
            lit(&[
                "Save", "Open", "New", "Close", "Copy", "Cut", "Paste", "Undo", "Redo",
                "SelectAll", "Find", "Replace", "Quit",
            ]),
            key_binding_literal,
        ],
        "A universal accelerator name (bare string) or `{\"Literal\": \"...\"}` for an app-specific binding.",
    ));
    let accelerator_scope_widget = b.define(obj(
        "_AcceleratorScopeWidget",
        vec![field("Widget", re("WidgetId"))],
    ));
    let accelerator_scope_mode =
        b.define(obj("_AcceleratorScopeMode", vec![field("Mode", str_())]));
    let accelerator_scope = b.define(union_of(
        "AcceleratorScope",
        vec![
            lit(&["Global"]),
            accelerator_scope_widget,
            accelerator_scope_mode,
        ],
    ));
    let accelerator = b.define(obj(
        "Accelerator",
        vec![
            field("id", re("AcceleratorId")),
            field("binding", key_binding),
            field("scope", accelerator_scope),
            field("label", nullable(str_())),
        ],
    ));
    let context_menu_item = re("ContextMenuItem");
    let context_menu_item_def = b.define(obj(
        "ContextMenuItem",
        vec![
            field_opt("id", nullable(re("WidgetId"))),
            field("label", styled_text.clone()),
            field_opt("detail", nullable(styled_text.clone())),
            field_opt("disabled", bool_()),
            field_opt("key_equivalent", nullable(accelerator)),
            field_opt("checked", nullable(bool_())),
            field_opt("icon", nullable(icon)),
            field_opt("submenu", nullable(list(context_menu_item))),
        ],
    ));
    let menu_bar_item = b.define(obj(
        "MenuBarItemDescriptor",
        vec![
            field("id", re("WidgetId")),
            field("label", str_()),
            field_opt("disabled", bool_()),
            field_opt("submenu", nullable(list(context_menu_item_def))),
        ],
    ));
    let menu_bar_descriptor = b.define(obj(
        "MenuBarDescriptor",
        vec![
            field("id", re("WidgetId")),
            field("items", list(menu_bar_item)),
            field_opt("open_item", nullable(int_())),
            field_opt("focused_item", nullable(int_())),
        ],
    ));
    let diff_row_kind = b.define(alias(
        "DiffRowKind",
        lit(&["Same", "Changed", "Removed", "Added"]),
    ));
    let diff_row = b.define(obj(
        "DiffRow",
        vec![
            field("left", nullable(str_())),
            field("right", nullable(str_())),
            field("kind", diff_row_kind),
        ],
    ));
    let diff_hunk = b.define(obj(
        "DiffHunk",
        vec![
            field("left_start", int_()),
            field("right_start", int_()),
            field("rows", list(diff_row)),
        ],
    ));
    let diff_mode = b.define(alias("DiffMode", lit(&["SideBySide", "Unified"])));
    let diff_editability = b.define(alias(
        "DiffEditability",
        lit(&["ReadOnly", "RightEditable"]),
    ));
    let diff_pane = b.define(alias("DiffPane", lit(&["Left", "Right"])));
    let diff_view_descriptor = b.define(obj(
        "DiffViewDescriptor",
        vec![
            field("id", re("WidgetId")),
            field("left", str_()),
            field("right", str_()),
            field("left_label", nullable(str_())),
            field("right_label", nullable(str_())),
            field("hunks", list(diff_hunk)),
            field("mode", diff_mode),
            field("editability", diff_editability),
            field("scroll_offset", int_()),
            field("focused_pane", diff_pane),
            field("has_focus", bool_()),
        ],
    ));

    // `LayoutLeafNode`/`RenderFrameDict`'s descriptor-kind keys: the six
    // fixed `LEAF_KIND_KEYS`, `status_bar`, and every `EXTRA_PRIMITIVE_KINDS`
    // additive kind are all fully typed. Reading straight off the real Rust
    // consts means a new fixed or additive kind always gets *a* schema entry
    // with no further edit here (the `unreachable!` arm below is this
    // function's own reminder to type it before merging).
    //
    // `leaf_fields` and `render_frame_fields` are built independently
    // (rather than one sliced from the other) because they are not the
    // same shape: a `LayoutLeafNode` is a `{"id", "<kind>": <descriptor>}`
    // dict where exactly one kind key is present, so every kind field here
    // is non-nullable (a *present* kind key is never `null`). A
    // `RenderFrameDict` is a flat `{"<kind>": <descriptor-or-null>, ...}`
    // frame where any number of slots may be present, and `render_frame_
    // from_value`'s `slot_entry` (`crate::wire`) treats a missing key and
    // an explicit `null` value identically ("draw nothing in this slot this
    // frame") -- so every kind field there must accept `null` too.
    let mut leaf_fields = vec![field("id", str_()), wire_version_field.clone()];
    let mut render_frame_fields = vec![wire_version_field.clone()];
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
        leaf_fields.push(field_opt(key, ty.clone()));
        render_frame_fields.push(field_opt(key, nullable(ty)));
    }
    leaf_fields.push(field_opt("status_bar", status_bar_descriptor.clone()));
    render_frame_fields.push(field_opt(
        "status_bar",
        nullable(status_bar_descriptor.clone()),
    ));
    for kind in EXTRA_PRIMITIVE_KINDS {
        if kind == "status_bar" {
            continue;
        }
        let ty = match kind {
            "progress" => progress_bar_descriptor.clone(),
            "spinner" => spinner_descriptor.clone(),
            "image" => image_descriptor.clone(),
            "form" => form_descriptor.clone(),
            "text_input" => text_input_descriptor.clone(),
            "number_input" => number_input_descriptor.clone(),
            "combo_box" => combo_box_descriptor.clone(),
            "date_picker" => date_picker_descriptor.clone(),
            "radio_group" => radio_group_descriptor.clone(),
            "switch" => switch_descriptor.clone(),
            "chart" => chart_descriptor.clone(),
            "message_list" => message_list_descriptor.clone(),
            "board" => board_model_descriptor.clone(),
            "pipeline_view" => pipeline_view_descriptor.clone(),
            "minimap" => minimap_descriptor.clone(),
            "multi_section_view" => multi_section_view_descriptor.clone(),
            "diff_view" => diff_view_descriptor.clone(),
            "activity_bar" => activity_bar_descriptor.clone(),
            "toolbar" => toolbar_descriptor.clone(),
            "menu_bar" => menu_bar_descriptor.clone(),
            "command_line" => command_line_descriptor.clone(),
            "command_center" => command_center_descriptor.clone(),
            "palette" => palette_descriptor.clone(),
            "scrollbar" => scrollbar_descriptor.clone(),
            "editor" => editor_descriptor.clone(),
            "terminal" => terminal_descriptor.clone(),
            "canvas" => canvas_descriptor.clone(),
            "scroll_view" => scroll_view_descriptor.clone(),
            "find_replace" => find_replace_panel_descriptor.clone(),
            "checkbox" => checkbox_descriptor.clone(),
            "slider" => slider_descriptor.clone(),
            "select" => select_descriptor.clone(),
            "password" => password_descriptor.clone(),
            "textarea" => textarea_descriptor.clone(),
            "markdown" => markdown_descriptor.clone(),
            other => unreachable!("EXTRA_PRIMITIVE_KINDS grew an unhandled kind {other:?}"),
        };
        leaf_fields.push(field_opt(kind, ty.clone()));
        render_frame_fields.push(field_opt(kind, nullable(ty)));
    }
    // A `LayoutLeafNode` is itself a valid row/column child, so it takes
    // the same `"size"`/`"min"`/`"max"` sibling keys every other
    // node-kind object above does -- `RenderFrameDict` (the flat shape,
    // never a row/column child) does not, so `render_frame_fields` stays
    // untouched. Same reasoning for the `"style"` sibling key every other
    // node-kind object above also takes.
    leaf_fields.push(field_opt("style", style_value.clone()));
    leaf_fields.push(field_opt("class", class_value.clone()));
    leaf_fields.extend(child_size_fields());
    let layout_leaf_node = b.define(obj_doc(
        "LayoutLeafNode",
        leaf_fields,
        "One `{\"id\", \"<kind>\": <descriptor>}` leaf -- exactly one kind key is present. Every registered kind (the six core kinds, `status_bar`, and every `EXTRA_PRIMITIVE_KINDS` additive kind) is fully typed.",
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
            layout_scroll_node,
            layout_leaf_node,
        ],
        "One node of a render callback's returned layout tree.",
    ));

    b.define(obj_doc(
        "RenderFrameDict",
        render_frame_fields,
        "The pre-layout-tree flat per-slot shape a render callback may still return instead of a `LayoutNode` tree. Every registered kind is fully typed.",
    ));
    b.define(alias_doc(
        "RenderOutput",
        uniont(vec![re("LayoutNode"), re("RenderFrameDict")]),
        "A render callback's return value: a layout tree or the flat shape.",
    ));

    // ── Content-bearing overlay descriptors ──────────────────────────────
    let anchor_descriptor = b.define(obj_doc(
        "AnchorDescriptor",
        vec![
            with_doc(
                field("rect", re("Rect")),
                "Bounds of the element the overlay opens against, in absolute coordinates.",
            ),
            field_opt(
                "preferred",
                lit(&["Top", "Bottom", "Left", "Right", "AtPoint"]),
            ),
            field_opt("margin", float_()),
        ],
        "Where a float/popover opens: a rect, the side of it to prefer, and the gap along that axis.",
    ));
    let float_descriptor = b.define(obj_doc(
        "FloatDescriptor",
        vec![
            field("id", re("WidgetId")),
            field("anchor", anchor_descriptor),
            field_opt("focusable", bool_()),
            field_opt("border", bool_()),
            field_opt("bg", nullable(re("Color"))),
        ],
        "A floating surface's own chrome: anchor, border and background.",
    ));
    let dialog_button_descriptor = b.define(obj_doc(
        "DialogButtonDescriptor",
        vec![
            field("id", re("WidgetId")),
            field("label", str_()),
            field_opt("is_default", bool_()),
            field_opt("is_cancel", bool_()),
            field_opt("tint", nullable(re("Color"))),
            field_opt("spec", nullable(re("ButtonSpecDescriptor"))),
        ],
        "One button in a dialog's button row.",
    ));
    let dialog_chrome_descriptor = b.define(obj_doc(
        "DialogChromeDescriptor",
        vec![
            field("id", re("WidgetId")),
            field_opt("title", str_()),
            with_doc(
                field_opt("body", str_()),
                "Text shown above the content slot; split on newlines into one line each.",
            ),
            field("buttons", list(dialog_button_descriptor)),
            field_opt("severity", lit(&["info", "question", "warning", "error"])),
            field_opt("vertical_buttons", bool_()),
        ],
        "A layout dialog's own title/body/button chrome -- the same friendly shape the \"message_dialog\" overlay kind takes.",
    ));
    b.define(obj_doc(
        "PopoverDescriptor",
        vec![
            field("float", float_descriptor),
            field("content", re("LayoutNode")),
        ],
        "A `show_overlay(\"popover\", ...)` descriptor: float chrome plus the layout subtree painted inside it, whose leaves focus and emit events exactly as in the main tree.",
    ));
    b.define(obj_doc(
        "LayoutDialogDescriptor",
        vec![
            field("dialog", dialog_chrome_descriptor),
            field("content", re("LayoutNode")),
        ],
        "A `show_overlay(\"layout_dialog\", ...)` descriptor: modal dialog chrome plus the layout subtree painted in its body.",
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
            ("FocusChanged", id_payload.clone()),
            ("ButtonClicked", id_payload.clone()),
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
        ("_HitDetailScrollBody", "scroll_body", &[]),
        (
            "_HitDetailScrollScrollbarTrackBefore",
            "scroll_scrollbar_track_before",
            &[],
        ),
        (
            "_HitDetailScrollScrollbarThumb",
            "scroll_scrollbar_thumb",
            &[],
        ),
        (
            "_HitDetailScrollScrollbarTrackAfter",
            "scroll_scrollbar_track_after",
            &[],
        ),
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
    //! no `..` (except where the type itself is `#[non_exhaustive]` -- see
    //! this module's own doc). Adding a variant or field to the matching
    //! Rust type without updating [`super::schema`] breaks compilation
    //! here (`E0004` non-exhaustive match, or `E0027` pattern missing a
    //! field) -- [`super::schema`] can't silently drift out of sync with
    //! the real types it describes.
    //!
    //! Most of these guards never run as tests (there is nothing to
    //! execute -- the function body only destructures its argument), but
    //! this whole module is still gated by `#[cfg(test)]`, so it is only
    //! *compiled* -- and the guards only take effect -- under a command
    //! that builds test targets (`cargo test`, `cargo check --tests`), not
    //! a bare `cargo build`/`cargo check`. [`value_shape`] and
    //! [`variant_shape`] below are the exception: they have real runtime
    //! assertions, because a `#[non_exhaustive]` struct's added field or a
    //! `#[non_exhaustive]` enum's added variant each has a case a pure
    //! pattern-match guard can't see (see this module's own doc). Keeping
    //! every guard in a dedicated test module (rather than scattered
    //! through `schema()` itself) keeps the schema construction and its
    //! coverage guards reviewable side by side without interleaving
    //! unrelated code.

    #![allow(dead_code, unused_variables)]
    // `Panel`/`PanelAction` are `#[deprecated]` (use `SidebarPanel` + a
    // header `Toolbar` instead); this module's own coverage guards for
    // their fields are the deprecated type's own plumbing, same rationale
    // as `crate::wire`'s identical allow.
    #![allow(deprecated)]

    use quadraui::{
        Accelerator, AcceleratorScope, ActivityBar, ActivityBarEvent, ActivityItem,
        BackendNativeEvent, Badge, BadgeStatus, BoardCard, BoardColumn, BoardModel, Button,
        ButtonIconPosition, ButtonMask, ButtonRowItem, ButtonSpec, ButtonVariant, CalendarDate,
        Canvas, CardBadge, Chart, ChartEvent, ChartKind, Color, Column, ColumnAlign, ColumnWidth,
        ComboBox, CommandCenter, CommandLine, ContextMenuItem, ControlWidth, DataRow, DataTable,
        DataTableEvent, DateFormat, DateLocale, DatePicker, Decoration, DiagnosticMark,
        DiagnosticSeverity, DiffEditability, DiffHunk, DiffLine, DiffMode, DiffPane, DiffRow,
        DiffRowKind, DiffView, DrawOp, Editor, EditorCursor, EditorCursorPos as CursorPos,
        EditorCursorShape as CursorShape, EditorLine, EditorSelection,
        EditorSelectionKind as SelectionKind, EditorStyle, EditorStyledSpan, EmptyBody, FieldKind,
        FindReplaceClickTarget, FindReplacePanel, Form, FormEvent, FormField, FormSpacing,
        FrHitRegion, GitLineStatus, HeaderAction, Icon, Image, ImageFit, ImageSource, InlineInput,
        Key, KeyBinding, LabelAlign, LabelPosition, ListItem, ListView, ListViewEvent, MenuBar,
        MenuBarItem, MessageList, MessageRow, Minimap, MinimapLine, MinimapSpan, Modifiers,
        MouseButton, MsvAxis, MultiSectionView, NamedKey, NumberInput, Palette, PaletteEvent,
        PaletteItem, PaletteMode, PalettePreview, Panel, PanelAction, PipelineStage, PipelineView,
        Point, PowerEventKind, ProgressBar, RadioGroup, RadioOption, Rect, RowAlign, ScrollAxis,
        ScrollDelta, ScrollMode, ScrollView, Scrollbar, Section, SectionAux, SectionBody,
        SectionHeader, SectionSize, SelectionMode, Series, SidebarPanel, SortDirection, SpellMark,
        Spinner, StageStatus, StatusBar, StatusBarEvent, StatusBarSegment, StyledSpan, StyledText,
        Switch, SystemTheme, TabBar, TabBarEvent, TabBarSegment, TabItem, Terminal, TerminalCell,
        TerminalCursorShape, TerminalEvent, TerminalScrollbar, TextDisplay, TextDisplayEvent,
        TextDisplayLine, TextInput, TimeOfDay, ToggleGroupItem, Toolbar, ToolbarButton,
        ToolbarEvent, TreeEvent, TreeRow, TreeRowEditState, TreeStyle, TreeView, UiEvent,
        ValidationState, Viewport, Weekday,
    };

    use quadraui::primitives::canvas::CanvasFontRole;

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
            HitDetail::ScrollBody => {}
            HitDetail::ScrollScrollbarTrackBefore => {}
            HitDetail::ScrollScrollbarThumb => {}
            HitDetail::ScrollScrollbarTrackAfter => {}
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
            LayoutNode::Scroll(_) => {}
            LayoutNode::Styled(_) => {}
        }
    }

    fn assert_selection_mode(m: SelectionMode) {
        match m {
            SelectionMode::None => {}
            SelectionMode::Single => {}
            SelectionMode::Multi => {}
        }
    }

    fn assert_decoration(d: Decoration) {
        match d {
            Decoration::Normal => {}
            Decoration::Header => {}
            Decoration::Muted => {}
            Decoration::Error => {}
            Decoration::Warning => {}
            Decoration::Modified => {}
        }
    }

    fn assert_power_event_kind(k: PowerEventKind) {
        match k {
            PowerEventKind::Suspend => {}
            PowerEventKind::Resume => {}
            PowerEventKind::OnBattery => {}
            PowerEventKind::OnAc => {}
            PowerEventKind::LockScreen => {}
            PowerEventKind::UnlockScreen => {}
        }
    }

    fn assert_column_align(a: ColumnAlign) {
        match a {
            ColumnAlign::Left => {}
            ColumnAlign::Center => {}
            ColumnAlign::Right => {}
        }
    }

    fn assert_sort_direction(d: SortDirection) {
        match d {
            SortDirection::Ascending => {}
            SortDirection::Descending => {}
        }
    }

    fn assert_column_width(w: ColumnWidth) {
        match w {
            // Deprecated in favour of `FixedCells`/`FixedPx` (ambiguous
            // unit) -- `schema()`'s own `ColumnWidth` entry deliberately
            // omits it; still named here so a field change to this variant
            // still trips this guard.
            ColumnWidth::Fixed(_) => {}
            ColumnWidth::FixedCells(_) => {}
            ColumnWidth::FixedPx(_) => {}
            ColumnWidth::Flex(_) => {}
            ColumnWidth::Content { min: _, max: _ } => {}
            // `ColumnWidth` is `#[non_exhaustive]` -- see
            // `assert_tab_bar_event`'s matching comment.
            _ => {}
        }
    }

    fn assert_key(k: Key) {
        match k {
            Key::Char(_) => {}
            Key::Named(_) => {}
        }
    }

    fn assert_named_key(k: NamedKey) {
        match k {
            NamedKey::Escape => {}
            NamedKey::Tab => {}
            NamedKey::BackTab => {}
            NamedKey::Enter => {}
            NamedKey::Backspace => {}
            NamedKey::Delete => {}
            NamedKey::Insert => {}
            NamedKey::Home => {}
            NamedKey::End => {}
            NamedKey::PageUp => {}
            NamedKey::PageDown => {}
            NamedKey::Up => {}
            NamedKey::Down => {}
            NamedKey::Left => {}
            NamedKey::Right => {}
            NamedKey::F(_) => {}
            NamedKey::CapsLock => {}
            NamedKey::NumLock => {}
            NamedKey::ScrollLock => {}
            NamedKey::Menu => {}
        }
    }

    fn assert_mouse_button(b: MouseButton) {
        match b {
            MouseButton::Left => {}
            MouseButton::Right => {}
            MouseButton::Middle => {}
            MouseButton::X1 => {}
            MouseButton::X2 => {}
            MouseButton::Other(_) => {}
        }
    }

    fn assert_button_variant(v: ButtonVariant) {
        match v {
            ButtonVariant::Primary => {}
            ButtonVariant::Secondary => {}
            ButtonVariant::Icon => {}
            ButtonVariant::Danger => {}
            ButtonVariant::Warning => {}
            // `ButtonVariant` is `#[non_exhaustive]` -- see
            // `assert_tab_bar_event`'s matching comment.
            _ => {}
        }
    }

    fn assert_button_icon_position(p: ButtonIconPosition) {
        match p {
            ButtonIconPosition::Leading => {}
            ButtonIconPosition::Trailing => {}
            // `ButtonIconPosition` is `#[non_exhaustive]` -- see
            // `assert_tab_bar_event`'s matching comment.
            _ => {}
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
    assert_fields!(
        assert_icon_fields,
        Icon {
            glyph,
            fallback,
            color
        }
    );
    assert_fields!(
        assert_modifiers_fields,
        Modifiers {
            shift,
            ctrl,
            alt,
            cmd
        }
    );
    assert_fields!(assert_point_fields, Point { x, y });
    assert_fields!(
        assert_rect_fields,
        Rect {
            x,
            y,
            width,
            height
        }
    );
    assert_fields!(assert_scroll_delta_fields, ScrollDelta { x, y });
    assert_fields!(
        assert_anchor_fields,
        quadraui::Anchor {
            rect,
            preferred,
            margin
        }
    );
    assert_fields_non_exhaustive!(
        assert_float_fields,
        quadraui::Float {
            id,
            anchor,
            focusable,
            border,
            bg
        }
    );
    assert_fields!(
        assert_dialog_button_fields,
        quadraui::DialogButton {
            id,
            label,
            is_default,
            is_cancel,
            tint,
            spec
        }
    );
    assert_fields!(
        assert_button_mask_fields,
        ButtonMask {
            left,
            right,
            middle
        }
    );
    assert_fields!(
        assert_viewport_fields,
        Viewport {
            width,
            height,
            scale
        }
    );
    assert_fields!(
        assert_tree_style_fields,
        TreeStyle {
            indent,
            show_chevrons,
            chevron_expanded,
            chevron_collapsed,
            row_height,
            bordered
        }
    );
    assert_fields!(
        assert_system_theme_fields,
        SystemTheme {
            dark,
            accent,
            high_contrast,
            reduce_motion
        }
    );
    assert_fields!(
        assert_tab_bar_segment_fields,
        TabBarSegment {
            text,
            width_cells,
            id,
            is_active
        }
    );
    assert_fields!(
        assert_backend_native_event_fields,
        BackendNativeEvent {
            backend,
            kind,
            payload
        }
    );
    assert_fields!(
        assert_calendar_date_fields,
        CalendarDate { year, month, day }
    );
    assert_fields_non_exhaustive!(
        assert_button_spec_fields,
        ButtonSpec {
            label,
            variant,
            icon,
            icon_position,
            icon_gap,
            tooltip,
            disabled
        }
    );

    // ── Additive primitive descriptors: field/variant drift guards ──────

    assert_fields!(
        assert_progress_bar_fields,
        ProgressBar {
            id,
            label,
            value,
            frame_idx,
            cancellable,
            accent
        }
    );
    assert_fields!(
        assert_spinner_fields,
        Spinner {
            id,
            label,
            frame_idx,
            accent
        }
    );
    fn assert_image_source(s: ImageSource) {
        match s {
            ImageSource::Bytes(_) => {}
            ImageSource::Path(_) => {}
        }
    }
    fn assert_image_fit(f: ImageFit) {
        match f {
            ImageFit::Contain => {}
            ImageFit::Cover => {}
            ImageFit::Fill => {}
            ImageFit::None => {}
        }
    }
    assert_fields!(
        assert_image_fields,
        Image {
            id,
            source,
            intrinsic_size,
            fit,
            fallback_text
        }
    );
    assert_fields_non_exhaustive!(
        assert_text_input_fields,
        TextInput {
            id,
            lines,
            cursor_line,
            cursor_col,
            placeholder,
            scroll_offset,
            scroll_col,
            has_focus
        }
    );
    assert_fields_non_exhaustive!(
        assert_number_input_fields,
        NumberInput {
            id,
            value,
            min,
            max,
            step,
            precision,
            draft,
            disabled,
            focused
        }
    );
    assert_fields_non_exhaustive!(
        assert_combo_box_fields,
        ComboBox {
            id,
            options,
            text,
            committed,
            open,
            highlighted,
            disabled,
            focused
        }
    );
    assert_fields_non_exhaustive!(
        assert_switch_fields,
        Switch {
            id,
            value,
            disabled,
            focused
        }
    );
    fn assert_scroll_axis(a: ScrollAxis) {
        match a {
            ScrollAxis::Vertical => {}
            ScrollAxis::Horizontal => {}
        }
    }
    assert_fields_non_exhaustive!(
        assert_scrollbar_fields,
        Scrollbar {
            id,
            axis,
            track,
            thumb_start,
            thumb_len,
            hovered,
            dragging
        }
    );
    assert_fields_non_exhaustive!(
        assert_scroll_view_fields,
        ScrollView {
            id,
            axis,
            viewport,
            content_len,
            scroll_offset,
            focused
        }
    );
    assert_fields!(
        assert_command_line_fields,
        CommandLine {
            id,
            text,
            cursor_offset,
            right_align
        }
    );
    assert_fields!(
        assert_command_center_fields,
        CommandCenter {
            id,
            back_enabled,
            forward_enabled,
            search_label
        }
    );
    assert_fields!(
        assert_radio_option_fields,
        RadioOption {
            id,
            label,
            disabled
        }
    );
    assert_fields_non_exhaustive!(
        assert_radio_group_fields,
        RadioGroup {
            id,
            options,
            selected,
            focused
        }
    );
    assert_fields!(
        assert_message_row_fields,
        MessageRow {
            text,
            fg,
            indent,
            spans,
            scale
        }
    );
    assert_fields!(
        assert_message_list_fields,
        MessageList {
            id,
            rows,
            scroll_top
        }
    );
    fn assert_weekday(w: Weekday) {
        match w {
            Weekday::Sunday => {}
            Weekday::Monday => {}
            Weekday::Tuesday => {}
            Weekday::Wednesday => {}
            Weekday::Thursday => {}
            Weekday::Friday => {}
            Weekday::Saturday => {}
        }
    }
    fn assert_date_format(f: DateFormat) {
        match f {
            DateFormat::Iso8601 => {}
            DateFormat::UsSlash => {}
            DateFormat::EuSlash => {}
        }
    }
    assert_fields!(
        assert_date_locale_fields,
        DateLocale {
            first_weekday,
            format
        }
    );
    assert_fields!(assert_time_of_day_fields, TimeOfDay { hour, minute });
    assert_fields_non_exhaustive!(
        assert_date_picker_fields,
        DatePicker {
            id,
            visible_month,
            selected,
            focused,
            locale,
            time,
            has_focus
        }
    );
    fn assert_chart_kind(k: ChartKind) {
        match k {
            ChartKind::Sparkline => {}
            ChartKind::Line => {}
            ChartKind::Bar => {}
            ChartKind::BarGrouped => {}
            // `ChartKind` is `#[non_exhaustive]` -- see
            // `assert_tab_bar_event`'s matching comment.
            _ => {}
        }
    }
    assert_fields!(
        assert_series_fields,
        Series {
            label,
            data,
            color,
            fill
        }
    );
    assert_fields!(
        assert_chart_fields,
        Chart {
            id,
            kind,
            series,
            x_label,
            y_label,
            y_range,
            x_range,
            show_legend,
            y_ticks,
            x_ticks,
            show_grid
        }
    );
    fn assert_stage_status(s: StageStatus) {
        match s {
            StageStatus::Pending => {}
            StageStatus::Active => {}
            StageStatus::Done => {}
            StageStatus::Failed => {}
            StageStatus::Skipped => {}
            StageStatus::Stale => {}
        }
    }
    assert_fields!(
        assert_pipeline_stage_fields,
        PipelineStage {
            label,
            status,
            action
        }
    );
    assert_fields!(
        assert_pipeline_view_fields,
        PipelineView {
            id,
            stages,
            focused_stage
        }
    );
    fn assert_badge_status(s: BadgeStatus) {
        match s {
            BadgeStatus::Pending => {}
            BadgeStatus::Running => {}
            BadgeStatus::Passed => {}
            BadgeStatus::Warning => {}
            BadgeStatus::Blocked => {}
        }
    }
    assert_fields!(assert_card_badge_fields, CardBadge { label, status });
    assert_fields!(
        assert_board_card_fields,
        BoardCard {
            id,
            title,
            labels,
            badges,
            hint
        }
    );
    assert_fields!(
        assert_board_column_fields,
        BoardColumn {
            id,
            title,
            cards,
            scroll_offset
        }
    );
    assert_fields!(
        assert_board_model_fields,
        BoardModel {
            id,
            columns,
            selected_card_id,
            col_scroll_offset
        }
    );
    assert_fields!(assert_minimap_line_fields, MinimapLine { text, line_idx });
    assert_fields!(
        assert_minimap_span_fields,
        MinimapSpan {
            line_idx,
            start_col,
            end_col,
            color
        }
    );
    assert_fields!(
        assert_minimap_fields,
        Minimap {
            id,
            lines,
            syntax_spans,
            visible_row_start,
            visible_row_count,
            total_buffer_lines
        }
    );
    assert_fields!(
        assert_activity_item_fields,
        ActivityItem {
            id,
            icon,
            tooltip,
            is_active,
            is_keyboard_selected
        }
    );
    assert_fields!(
        assert_activity_bar_fields2,
        ActivityBar {
            id,
            top_items,
            bottom_items,
            active_accent,
            selection_bg,
            is_keyboard_focused
        }
    );
    fn assert_palette_mode(m: PaletteMode) {
        match m {
            PaletteMode::List => {}
            PaletteMode::Input => {}
        }
    }
    assert_fields!(
        assert_palette_item_fields,
        PaletteItem {
            text,
            detail,
            icon,
            match_positions,
            depth,
            expandable,
            expanded
        }
    );
    assert_fields!(
        assert_palette_preview_fields,
        PalettePreview {
            lines,
            title,
            scroll_offset,
            highlight_line
        }
    );
    assert_fields!(
        assert_palette_fields2,
        Palette {
            id,
            title,
            query,
            query_cursor,
            items,
            selected_idx,
            scroll_offset,
            total_count,
            has_focus,
            show_query,
            create_label,
            preview,
            mode
        }
    );
    fn assert_terminal_cursor_shape(s: TerminalCursorShape) {
        match s {
            TerminalCursorShape::Block => {}
            TerminalCursorShape::Underline => {}
            TerminalCursorShape::Bar => {}
        }
    }
    assert_fields!(
        assert_terminal_cell_fields,
        TerminalCell {
            text,
            fg,
            bg,
            bold,
            italic,
            underline,
            dim,
            selected,
            is_cursor,
            cursor_shape,
            cursor_blinking,
            is_find_match,
            is_find_active
        }
    );
    assert_fields!(
        assert_terminal_scrollbar_fields,
        TerminalScrollbar {
            total_lines,
            visible_lines,
            scroll_offset,
            inverted,
            width
        }
    );
    assert_fields!(
        assert_terminal_fields2,
        Terminal {
            id,
            cells,
            scrollbar
        }
    );
    fn assert_canvas_font_role(r: CanvasFontRole) {
        match r {
            CanvasFontRole::Chrome => {}
            CanvasFontRole::Editor => {}
        }
    }
    fn assert_draw_op(op: DrawOp) {
        match op {
            DrawOp::Rect { rect: _, color: _ } => {}
            DrawOp::RoundedRect {
                rect: _,
                radius: _,
                color: _,
            } => {}
            DrawOp::Line {
                from: _,
                to: _,
                color: _,
                stroke_width: _,
            } => {}
            DrawOp::Path {
                points: _,
                color: _,
                stroke_width: _,
                closed: _,
            } => {}
            DrawOp::TextRun {
                rect: _,
                text: _,
                color: _,
            } => {}
            DrawOp::Image { rect: _, image: _ } => {}
            DrawOp::PushClip { rect: _ } => {}
            DrawOp::PopClip => {}
            DrawOp::Polygon {
                points: _,
                color: _,
            } => {}
            DrawOp::Ellipse {
                center: _,
                radius_x: _,
                radius_y: _,
                color: _,
            } => {}
            DrawOp::Arc {
                center: _,
                radius: _,
                start_angle: _,
                end_angle: _,
                color: _,
                stroke_width: _,
            } => {}
            DrawOp::Bezier {
                from: _,
                control1: _,
                control2: _,
                to: _,
                color: _,
                stroke_width: _,
            } => {}
            DrawOp::DashedLine {
                from: _,
                to: _,
                color: _,
                stroke_width: _,
                dash_len: _,
                gap_len: _,
            } => {}
            DrawOp::StrokeRect {
                rect: _,
                color: _,
                stroke_width: _,
            } => {}
            DrawOp::StyledTextRun {
                rect: _,
                text: _,
                color: _,
                role: _,
                italic: _,
                size: _,
            } => {}
            // `DrawOp` is `#[non_exhaustive]` -- see
            // `assert_tab_bar_event`'s matching comment.
            _ => {}
        }
    }
    assert_fields!(assert_canvas_fields, Canvas { id, ops });
    fn assert_diagnostic_severity(s: DiagnosticSeverity) {
        match s {
            DiagnosticSeverity::Error => {}
            DiagnosticSeverity::Warning => {}
            DiagnosticSeverity::Information => {}
            DiagnosticSeverity::Hint => {}
        }
    }
    fn assert_git_line_status(s: GitLineStatus) {
        match s {
            GitLineStatus::Added => {}
            GitLineStatus::Modified => {}
            GitLineStatus::Deleted => {}
        }
    }
    fn assert_editor_diff_line(d: DiffLine) {
        match d {
            DiffLine::Same => {}
            DiffLine::Added => {}
            DiffLine::Removed => {}
            DiffLine::Padding => {}
        }
    }
    fn assert_cursor_shape(s: CursorShape) {
        match s {
            CursorShape::Block => {}
            CursorShape::Bar => {}
            CursorShape::Underline => {}
        }
    }
    fn assert_selection_kind(k: SelectionKind) {
        match k {
            SelectionKind::Char => {}
            SelectionKind::Line => {}
            SelectionKind::Block => {}
        }
    }
    assert_fields!(assert_cursor_pos_fields, CursorPos { view_line, col });
    assert_fields!(assert_editor_cursor_fields, EditorCursor { pos, shape });
    assert_fields!(
        assert_editor_selection_fields,
        EditorSelection {
            kind,
            start_line,
            start_col,
            end_line,
            end_col
        }
    );
    assert_fields!(
        assert_editor_style_fields,
        EditorStyle {
            fg,
            bg,
            bold,
            italic,
            font_scale
        }
    );
    assert_fields!(
        assert_editor_styled_span_fields,
        EditorStyledSpan {
            start_byte,
            end_byte,
            style
        }
    );
    assert_fields!(
        assert_diagnostic_mark_fields,
        DiagnosticMark {
            start_col,
            end_col,
            severity,
            message
        }
    );
    assert_fields!(assert_spell_mark_fields, SpellMark { start_col, end_col });
    assert_fields!(
        assert_editor_line_fields,
        EditorLine {
            raw_text,
            gutter_text,
            spans,
            line_idx,
            is_current_line,
            is_fold_header,
            folded_line_count,
            git_diff,
            diff_status,
            diagnostics,
            spell_errors,
            is_breakpoint,
            is_conditional_bp,
            is_dap_current,
            is_wrap_continuation,
            segment_col_offset,
            annotation,
            ghost_suffix,
            is_ghost_continuation,
            indent_guides,
            colorcolumns
        }
    );
    assert_fields_non_exhaustive!(
        assert_editor_fields,
        Editor {
            id,
            rect,
            lines,
            cursor,
            extra_cursors,
            selection,
            extra_selections,
            yank_highlight,
            scroll_top,
            scroll_left,
            total_lines,
            max_col,
            gutter_char_width,
            is_active,
            show_active_bg,
            has_git_diff,
            has_breakpoints,
            diagnostic_gutter,
            code_action_lines,
            bracket_match_positions,
            active_indent_col,
            tabstop,
            cursorline,
            lightbulb_glyph
        }
    );
    fn assert_find_replace_click_target(t: FindReplaceClickTarget) {
        match t {
            FindReplaceClickTarget::Chevron => {}
            FindReplaceClickTarget::FindInput(_) => {}
            FindReplaceClickTarget::ReplaceInput(_) => {}
            FindReplaceClickTarget::ToggleCase => {}
            FindReplaceClickTarget::ToggleWholeWord => {}
            FindReplaceClickTarget::ToggleRegex => {}
            FindReplaceClickTarget::PrevMatch => {}
            FindReplaceClickTarget::NextMatch => {}
            FindReplaceClickTarget::ToggleInSelection => {}
            FindReplaceClickTarget::Close => {}
            FindReplaceClickTarget::TogglePreserveCase => {}
            FindReplaceClickTarget::ReplaceCurrent => {}
            FindReplaceClickTarget::ReplaceAll => {}
        }
    }
    assert_fields!(assert_fr_hit_region_fields, FrHitRegion { col, row, width });
    assert_fields!(
        assert_find_replace_panel_fields,
        FindReplacePanel {
            query,
            replacement,
            show_replace,
            focus,
            cursor,
            sel_anchor,
            match_info,
            case_sensitive,
            whole_word,
            use_regex,
            preserve_case,
            in_selection,
            group_bounds,
            panel_width,
            replace_one_glyph,
            replace_all_glyph,
            hit_regions
        }
    );
    fn assert_control_width(w: ControlWidth) {
        match w {
            ControlWidth::Fill => {}
            ControlWidth::Fixed(_) => {}
            ControlWidth::Content => {}
        }
    }
    fn assert_label_position(p: LabelPosition) {
        match p {
            LabelPosition::Left => {}
            LabelPosition::Top => {}
            LabelPosition::Inline => {}
            LabelPosition::Hidden => {}
        }
    }
    fn assert_label_align(a: LabelAlign) {
        match a {
            LabelAlign::Start => {}
            LabelAlign::Center => {}
            LabelAlign::End => {}
        }
    }
    fn assert_row_align(a: RowAlign) {
        match a {
            RowAlign::Start => {}
            RowAlign::Center => {}
            RowAlign::End => {}
            RowAlign::Stretch => {}
        }
    }
    assert_fields!(
        assert_form_spacing_fields,
        FormSpacing {
            padding,
            item_gap,
            row_align
        }
    );
    fn assert_validation_state(v: ValidationState) {
        match v {
            ValidationState::Error(_) => {}
            ValidationState::Warning(_) => {}
        }
    }
    assert_fields!(
        assert_toggle_group_item_fields,
        ToggleGroupItem { id, label, value }
    );
    assert_fields!(
        assert_button_row_item_fields,
        ButtonRowItem {
            id,
            label,
            disabled,
            icon,
            spec
        }
    );
    fn assert_field_kind(k: FieldKind) {
        match k {
            FieldKind::Label => {}
            FieldKind::Toggle { value: _ } => {}
            FieldKind::TextInput {
                value: _,
                placeholder: _,
                cursor: _,
                selection_anchor: _,
            } => {}
            FieldKind::Button => {}
            FieldKind::ReadOnly { value: _ } => {}
            FieldKind::Slider {
                value: _,
                min: _,
                max: _,
                step: _,
            } => {}
            FieldKind::ColorPicker { value: _ } => {}
            FieldKind::Dropdown {
                options: _,
                selected_idx: _,
            } => {}
            FieldKind::TextArea {
                value: _,
                placeholder: _,
                cursor: _,
                visible_rows: _,
            } => {}
            FieldKind::PasswordInput {
                value: _,
                placeholder: _,
                cursor: _,
                mask_char: _,
            } => {}
            FieldKind::SegmentedControl {
                options: _,
                selected_idx: _,
            } => {}
            FieldKind::ToggleGroup { toggles: _ } => {}
            FieldKind::ButtonRow { buttons: _ } => {}
            FieldKind::Toolbar(_) => {}
            FieldKind::DatePicker(_) => {}
            FieldKind::ComboBox(_) => {}
            FieldKind::NumberInput(_) => {}
            FieldKind::RadioGroup(_) => {}
        }
    }
    assert_fields!(
        assert_form_field_fields,
        FormField {
            id,
            label,
            kind,
            hint,
            disabled,
            validation,
            label_position,
            label_align,
            label_width,
            tab_index,
            control_width,
            min_control_width,
            row_align
        }
    );
    assert_fields!(
        assert_form_fields2,
        Form {
            id,
            fields,
            focused_field,
            scroll_offset,
            has_focus,
            label_position,
            label_align,
            label_width,
            control_width,
            min_control_width,
            spacing
        }
    );
    fn assert_msv_axis(a: MsvAxis) {
        match a {
            MsvAxis::Vertical => {}
            MsvAxis::Horizontal => {}
        }
    }
    fn assert_scroll_mode(m: ScrollMode) {
        match m {
            ScrollMode::PerSection => {}
            ScrollMode::WholePanel => {}
        }
    }
    fn assert_section_size(s: SectionSize) {
        match s {
            SectionSize::Fixed(_) => {}
            SectionSize::Percent(_) => {}
            SectionSize::Weight(_) => {}
            SectionSize::Content => {}
            SectionSize::EqualShare => {}
        }
    }
    assert_fields!(
        assert_header_action_fields,
        HeaderAction {
            id,
            icon,
            tooltip,
            enabled
        }
    );
    assert_fields!(
        assert_section_header_fields,
        SectionHeader {
            icon,
            title,
            badge,
            actions,
            show_chevron
        }
    );
    assert_fields!(
        assert_empty_body_fields,
        EmptyBody {
            icon,
            text,
            hint,
            action
        }
    );
    assert_fields!(
        assert_inline_input_fields,
        InlineInput {
            id,
            text,
            caret,
            placeholder,
            has_focus
        }
    );
    fn assert_section_aux(a: SectionAux) {
        match a {
            SectionAux::Input(_) => {}
            SectionAux::Toolbar(_) => {}
            SectionAux::Search(_) => {}
            SectionAux::Custom(_) => {}
        }
    }
    fn assert_section_body(b: SectionBody) {
        match b {
            SectionBody::Tree(_) => {}
            SectionBody::List(_) => {}
            SectionBody::Form(_) => {}
            SectionBody::Terminal(_) => {}
            SectionBody::MessageList(_) => {}
            SectionBody::Text(_) => {}
            SectionBody::Chart(_) => {}
            SectionBody::Empty(_) => {}
            SectionBody::Custom(_) => {}
        }
    }
    assert_fields!(
        assert_section_fields,
        Section {
            id,
            header,
            body,
            aux,
            size,
            collapsed,
            min_size,
            max_size
        }
    );
    assert_fields!(
        assert_multi_section_view_fields,
        MultiSectionView {
            id,
            sections,
            active_section,
            axis,
            allow_resize,
            allow_collapse,
            scroll_mode,
            has_focus,
            panel_scroll
        }
    );
    fn assert_key_binding(k: KeyBinding) {
        match k {
            KeyBinding::Save => {}
            KeyBinding::Open => {}
            KeyBinding::New => {}
            KeyBinding::Close => {}
            KeyBinding::Copy => {}
            KeyBinding::Cut => {}
            KeyBinding::Paste => {}
            KeyBinding::Undo => {}
            KeyBinding::Redo => {}
            KeyBinding::SelectAll => {}
            KeyBinding::Find => {}
            KeyBinding::Replace => {}
            KeyBinding::Quit => {}
            KeyBinding::Literal(_) => {}
        }
    }
    fn assert_accelerator_scope(s: AcceleratorScope) {
        match s {
            AcceleratorScope::Global => {}
            AcceleratorScope::Widget(_) => {}
            AcceleratorScope::Mode(_) => {}
        }
    }
    assert_fields!(
        assert_accelerator_fields,
        Accelerator {
            id,
            binding,
            scope,
            label
        }
    );
    assert_fields!(
        assert_context_menu_item_fields,
        ContextMenuItem {
            id,
            label,
            detail,
            disabled,
            key_equivalent,
            checked,
            icon,
            submenu
        }
    );
    assert_fields!(
        assert_menu_bar_item_fields,
        MenuBarItem {
            id,
            label,
            disabled,
            submenu
        }
    );
    assert_fields!(
        assert_menu_bar_fields,
        MenuBar {
            id,
            items,
            open_item,
            focused_item
        }
    );
    fn assert_diff_row_kind(k: DiffRowKind) {
        match k {
            DiffRowKind::Same => {}
            DiffRowKind::Changed => {}
            DiffRowKind::Removed => {}
            DiffRowKind::Added => {}
        }
    }
    assert_fields!(assert_diff_row_fields, DiffRow { left, right, kind });
    assert_fields!(
        assert_diff_hunk_fields,
        DiffHunk {
            left_start,
            right_start,
            rows
        }
    );
    fn assert_diff_mode(m: DiffMode) {
        match m {
            DiffMode::SideBySide => {}
            DiffMode::Unified => {}
        }
    }
    fn assert_diff_editability(e: DiffEditability) {
        match e {
            DiffEditability::ReadOnly => {}
            DiffEditability::RightEditable => {}
        }
    }
    fn assert_diff_pane(p: DiffPane) {
        match p {
            DiffPane::Left => {}
            DiffPane::Right => {}
        }
    }
    assert_fields!(
        assert_diff_view_fields,
        DiffView {
            id,
            left,
            right,
            left_label,
            right_label,
            hunks,
            mode,
            editability,
            scroll_offset,
            focused_pane,
            has_focus
        }
    );

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

    /// Runtime key-set guards for every `#[non_exhaustive]` **struct**
    /// [`assert_fields_non_exhaustive!`] can't fully cover -- a struct
    /// pattern built outside its defining crate must end in `..`, so a
    /// brand-new named field never shows up in that macro's destructure.
    /// Serializing a real value and comparing its JSON key set against the
    /// exact field list [`super::schema`] declares catches that new field
    /// instead: `serde_json` emits one key per struct field regardless of
    /// `..`, so an added field changes the key set even though it was
    /// invisible to the pattern match above.
    mod value_shape {
        use std::collections::BTreeSet;

        use quadraui::{
            Button, ButtonVariant, CalendarDate, ComboBox, DatePicker, Editor, NumberInput,
            RadioGroup, RadioOption, Rect, ScrollView, Scrollbar, Switch, TextInput, Toolbar,
            WidgetId,
        };

        fn keys(v: &serde_json::Value) -> BTreeSet<String> {
            v.as_object()
                .expect("value must serialize to a JSON object")
                .keys()
                .cloned()
                .collect()
        }

        fn set(names: &[&str]) -> BTreeSet<String> {
            names.iter().map(|s| s.to_string()).collect()
        }

        /// Emits one `#[test]` that serializes `$value` and asserts its
        /// JSON key set against the exact field list `schema()`'s
        /// `$desc_name` entry declares for `$ty_name` -- the runtime
        /// backstop [`assert_fields_non_exhaustive!`] can't provide for a
        /// `#[non_exhaustive]` struct's *added* field (see this module's
        /// own doc). An optional trailing `extra: { .. }` block runs after
        /// the assertion, for a type that needs one more line of coverage
        /// beyond the key-set check itself.
        macro_rules! value_shape_test {
            (
                $fn_name:ident,
                $ty_name:literal,
                $desc_name:literal,
                $value:expr,
                [$($key:literal),+ $(,)?]
                $(, extra: $extra:block)?
            ) => {
                #[test]
                fn $fn_name() {
                    let value = serde_json::to_value(&$value)
                        .unwrap_or_else(|e| panic!(concat!($ty_name, " must serialize: {}"), e));
                    assert_eq!(
                        keys(&value),
                        set(&[$($key),+]),
                        concat!(
                            "a field was added to or removed from `",
                            $ty_name,
                            "` without a matching update to `schema()`'s `",
                            $desc_name,
                            "` entry -- `assert_fields_non_exhaustive!` can't see a \
                             new field on a `#[non_exhaustive]` struct, so this \
                             key-set check is the guard that catches it",
                        )
                    );
                    $($extra)?
                }
            };
        }

        value_shape_test!(
            button_json_keys_match_the_schema_field_list,
            "Button",
            "ButtonDescriptor",
            Button::new(WidgetId::new("test"), "label"),
            [
                "id",
                "label",
                "variant",
                "disabled",
                "focused",
                "tooltip",
                "icon",
                "icon_position",
                "icon_gap",
            ],
            extra: {
                // `ButtonVariant` is itself `#[non_exhaustive]`; constructing
                // one here (rather than only via `Button::default()`'s
                // `Primary`) keeps this test from silently losing coverage if
                // `Button`'s own default variant ever changes.
                let _ = ButtonVariant::Secondary;
            }
        );

        value_shape_test!(
            float_json_keys_match_the_schema_field_list,
            "Float",
            "FloatDescriptor",
            quadraui::Float::new(
                WidgetId::new("test"),
                quadraui::Anchor::new(Rect::new(0.0, 0.0, 1.0, 1.0), quadraui::Side::Bottom),
            ),
            ["id", "anchor", "focusable", "border", "bg"]
        );

        value_shape_test!(
            toolbar_json_keys_match_the_schema_field_list,
            "Toolbar",
            "ToolbarDescriptor",
            Toolbar::new(WidgetId::new("test")),
            ["id", "buttons", "bg", "focused_index"]
        );

        value_shape_test!(
            text_input_json_keys_match_the_schema_field_list,
            "TextInput",
            "TextInputDescriptor",
            TextInput::new(WidgetId::new("test")),
            [
                "id",
                "lines",
                "cursor_line",
                "cursor_col",
                "placeholder",
                "scroll_offset",
                "scroll_col",
                "has_focus",
            ]
        );

        value_shape_test!(
            number_input_json_keys_match_the_schema_field_list,
            "NumberInput",
            "NumberInputDescriptor",
            NumberInput::new(WidgetId::new("test"), 0.0, 10.0),
            [
                "id",
                "value",
                "min",
                "max",
                "step",
                "precision",
                "draft",
                "disabled",
                "focused",
            ]
        );

        value_shape_test!(
            combo_box_json_keys_match_the_schema_field_list,
            "ComboBox",
            "ComboBoxDescriptor",
            ComboBox::new(WidgetId::new("test"), vec!["one".to_string()]),
            [
                "id",
                "options",
                "text",
                "committed",
                "open",
                "highlighted",
                "disabled",
                "focused",
            ]
        );

        value_shape_test!(
            switch_json_keys_match_the_schema_field_list,
            "Switch",
            "SwitchDescriptor",
            Switch::new(WidgetId::new("test")),
            ["id", "value", "disabled", "focused"]
        );

        value_shape_test!(
            scrollbar_json_keys_match_the_schema_field_list,
            "Scrollbar",
            "ScrollbarDescriptor",
            Scrollbar::vertical(
                WidgetId::new("test"),
                Rect::new(0.0, 0.0, 10.0, 10.0),
                0.0,
                10.0,
                5.0,
                1.0
            ),
            [
                "id",
                "axis",
                "track",
                "thumb_start",
                "thumb_len",
                "hovered",
                "dragging",
            ]
        );

        value_shape_test!(
            scroll_view_json_keys_match_the_schema_field_list,
            "ScrollView",
            "ScrollViewDescriptor",
            ScrollView::new(WidgetId::new("test"), Rect::new(0.0, 0.0, 10.0, 10.0), 20.0),
            [
                "id",
                "axis",
                "viewport",
                "content_len",
                "scroll_offset",
                "focused",
            ]
        );

        value_shape_test!(
            radio_group_json_keys_match_the_schema_field_list,
            "RadioGroup",
            "RadioGroupDescriptor",
            RadioGroup::new(
                WidgetId::new("test"),
                vec![RadioOption::new(WidgetId::new("test:option"), "label")]
            ),
            ["id", "options", "selected", "focused"]
        );

        value_shape_test!(
            date_picker_json_keys_match_the_schema_field_list,
            "DatePicker",
            "DatePickerDescriptor",
            DatePicker::new(WidgetId::new("test"), CalendarDate::new(2024, 1, 1)),
            [
                "id",
                "visible_month",
                "selected",
                "focused",
                "locale",
                "time",
                "has_focus",
            ]
        );

        value_shape_test!(
            editor_json_keys_match_the_schema_field_list,
            "Editor",
            "EditorDescriptor",
            Editor::new(WidgetId::new("test"), Rect::new(0.0, 0.0, 10.0, 10.0)),
            [
                "id",
                "rect",
                "lines",
                "cursor",
                "extra_cursors",
                "selection",
                "extra_selections",
                "yank_highlight",
                "scroll_top",
                "scroll_left",
                "total_lines",
                "max_col",
                "gutter_char_width",
                "is_active",
                "show_active_bg",
                "has_git_diff",
                "has_breakpoints",
                "diagnostic_gutter",
                "code_action_lines",
                "bracket_match_positions",
                "active_indent_col",
                "tabstop",
                "cursorline",
                "lightbulb_glyph",
            ]
        );
    }

    /// Runtime variant-set guards for every `#[non_exhaustive]` **enum** a
    /// pattern match can't fully cover -- an external match only needs a
    /// wildcard arm for *unlisted* variants (see this module's own doc), so
    /// a brand-new variant added upstream is invisible to a `match` like
    /// [`assert_toolbar_button`]/[`assert_tab_bar_event`] even though every
    /// *known* variant's own fields are still matched exhaustively there.
    ///
    /// Serde's own derive has no such blind spot: deserializing an
    /// externally-tagged enum with an unrecognised tag produces an error
    /// whose message serde generates straight from the enum's real variant
    /// list (`unknown variant` ``tag``, expected one of ``a``, ``b``, ...``),
    /// so parsing that message back out gives the exact variant set the
    /// *compiled* enum has today -- catching an added, removed, or renamed
    /// variant the same way [`value_shape`]'s JSON key-set check catches a
    /// field `assert_fields_non_exhaustive!` can't see. This also covers a
    /// fieldless (C-like) `#[non_exhaustive]` enum like `ChartKind`: serde's
    /// generated `Deserialize` still routes an externally-tagged map probe
    /// through the same "expected one of ..." error, even though every real
    /// variant there only ever appears as a bare string on the wire.
    mod variant_shape {
        use std::collections::BTreeSet;

        use quadraui::{ChartKind, DrawOp, TabBarEvent, ToolbarButton};

        /// Deserializes a deliberately-unknown tag into `T` and parses the
        /// resulting serde error's "expected one of `A`, `B`, ..." variant
        /// list back out. Panics if `T` accepts the probe tag (it shouldn't
        /// -- the tag is namespaced specifically so it can never collide
        /// with a real variant) or if serde's message shape ever changes in
        /// a way this parser doesn't recognise.
        fn known_variants<T: serde::de::DeserializeOwned + std::fmt::Debug>() -> BTreeSet<String> {
            let err = serde_json::from_str::<T>(r#"{"__quadraui_schema_probe__":null}"#)
                .expect_err("the probe tag must not be a real variant");
            let message = err.to_string();
            let marker = "expected one of ";
            let list = message
                .split_once(marker)
                .map(|(_, rest)| rest)
                .unwrap_or_else(|| {
                    panic!(
                        "serde's unknown-variant error message no longer contains \
                         {marker:?} -- update `known_variants`'s parser to match \
                         the new shape (got: {message:?})"
                    )
                });
            // Each name is backtick-delimited (`` `A`, `B`, ... ``); the
            // list is followed by serde_json's own trailing " at line N
            // column N" location suffix, outside the last pair of
            // backticks, so splitting on backtick alone (rather than on
            // `, `) is what keeps that suffix out of the last entry.
            list.split('`')
                .skip(1)
                .step_by(2)
                .map(str::to_string)
                .collect()
        }

        /// Emits one `#[test]` asserting `known_variants::<$ty>()` against
        /// the exact tag list `schema()`'s `$desc_name` entry declares for
        /// `$ty_name` -- the backstop a wildcard-arm `match` can't provide
        /// for a `#[non_exhaustive]` enum's *added* variant (see this
        /// module's own doc).
        macro_rules! variant_shape_test {
            ($fn_name:ident, $ty:ty, $ty_name:literal, $desc_name:literal, [$($variant:literal),+ $(,)?]) => {
                #[test]
                fn $fn_name() {
                    assert_eq!(
                        known_variants::<$ty>(),
                        [$($variant),+]
                            .into_iter()
                            .map(String::from)
                            .collect::<BTreeSet<_>>(),
                        concat!(
                            "a variant was added to or removed from `",
                            $ty_name,
                            "` without a matching update to `schema()`'s `",
                            $desc_name,
                            "` entry -- a wildcard arm can't see a new variant \
                             on a `#[non_exhaustive]` enum, so this \
                             probe-deserialize check is the guard that catches it",
                        )
                    );
                }
            };
        }

        variant_shape_test!(
            toolbar_button_variants_match_the_schema_tag_list,
            ToolbarButton,
            "ToolbarButton",
            "ToolbarButtonDescriptor",
            ["Action", "Separator", "Label", "Spacer"]
        );

        variant_shape_test!(
            tab_bar_event_variants_match_the_schema_tag_list,
            TabBarEvent,
            "TabBarEvent",
            "TabBarEvent",
            [
                "TabActivated",
                "TabClosed",
                "ButtonClicked",
                "KeyPressed",
                "FocusMoved",
            ]
        );

        variant_shape_test!(
            chart_kind_variants_match_the_schema_tag_list,
            ChartKind,
            "ChartKind",
            "ChartKind",
            ["Sparkline", "Line", "Bar", "BarGrouped"]
        );

        variant_shape_test!(
            draw_op_variants_match_the_schema_tag_list,
            DrawOp,
            "DrawOp",
            "DrawOp",
            [
                "Rect",
                "RoundedRect",
                "Line",
                "Path",
                "TextRun",
                "Image",
                "PushClip",
                "PopClip",
                "Polygon",
                "Ellipse",
                "Arc",
                "Bezier",
                "DashedLine",
                "StrokeRect",
                "StyledTextRun",
            ]
        );
    }
}
