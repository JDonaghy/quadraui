//! The binding wire schema, derived from the Rust types the wire decodes
//! into: one JSON Schema (draft 2020-12) document whose `$defs` hold every
//! descriptor, event and layout-node type, whose `kinds` map names each
//! leaf widget kind's descriptor, and whose `roots` map names the shapes a
//! binding sends or receives as a whole.
//!
//! The kind list is read from the tables the decoder itself dispatches
//! through: the fixed leaf slots ([`crate::layout::LEAF_KIND_KEYS`]), the
//! [`crate::extra_paint::EXTRA_PRIMITIVE_KINDS`] table, the
//! [`crate::extra_paint::WIDGET_KINDS`] kinds that `quadraui`'s
//! `widget_kinds!` table decodes, and every kind registered at runtime
//! through [`quadraui::register_widget_kind`]. Nothing here is specific to
//! one host language: any binding (Python, Node, Lua, ...) can call
//! [`wire_schema`] and generate its own types from the result.

use schemars::generate::SchemaSettings;
use schemars::{json_schema, JsonSchema, Schema, SchemaGenerator};
use serde_json::{Map, Value};

use crate::extra_paint::{visit_extra_primitive_kinds, WIDGET_KINDS};
use crate::layout::LEAF_KIND_KEYS;
use crate::wire::layout::schema::LayoutNode;
use crate::wire::overlay::DialogChromeWire;

pub use crate::wire::layout::schema::{LayoutLeafNode, StyleValue};

use crate::WIRE_VERSION;

/// A `{"float": {...}, "content": <node>}` popover overlay.
#[derive(JsonSchema)]
pub struct PopoverDescriptor {
    pub float: quadraui::Float,
    pub content: LayoutNode,
}

/// A `{"dialog": {...}, "content": <node>}` layout-dialog overlay.
#[derive(JsonSchema)]
pub struct LayoutDialogDescriptor {
    pub dialog: DialogChromeWire,
    pub content: LayoutNode,
}

/// One `run_shell` activity-bar panel entry.
#[derive(JsonSchema)]
pub struct PanelDefinitionDescriptor {
    pub id: String,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub tooltip: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
}

/// One tab of `run_shell`'s tabbed bottom panel.
#[derive(JsonSchema)]
pub struct BottomPanelTabDescriptor {
    pub id: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub closable: Option<bool>,
    #[serde(default)]
    pub badge: Option<String>,
}

/// `run_shell`'s `"bottom_panel"` dict.
#[derive(JsonSchema)]
pub struct BottomPanelConfigDescriptor {
    #[serde(default)]
    pub tabs: Option<Vec<BottomPanelTabDescriptor>>,
    #[serde(default)]
    pub active_tab_id: Option<String>,
    #[serde(default)]
    pub maximised: Option<bool>,
    #[serde(default)]
    pub height_fraction: Option<f32>,
}

#[derive(JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SidebarPositionValue {
    Left,
    Right,
}

/// `run_shell`'s `config` dict.
#[derive(JsonSchema)]
pub struct ShellConfigDescriptor {
    pub title: String,
    #[serde(default)]
    pub panels: Option<Vec<PanelDefinitionDescriptor>>,
    #[serde(default)]
    pub bottom_items: Option<Vec<PanelDefinitionDescriptor>>,
    #[serde(default)]
    pub position: Option<SidebarPositionValue>,
    #[serde(default)]
    pub sidebar_visible: Option<bool>,
    #[serde(default)]
    pub default_sidebar_width: Option<f32>,
    #[serde(default)]
    pub min_sidebar_width: Option<f32>,
    #[serde(default)]
    pub max_sidebar_width: Option<f32>,
    #[serde(default)]
    pub activity_bar_width: Option<f32>,
    #[serde(default)]
    pub has_title_bar: Option<bool>,
    #[serde(default)]
    pub title_bar_height_lh: Option<f32>,
    #[serde(default)]
    pub has_status_bar: Option<bool>,
    #[serde(default)]
    pub has_command_line: Option<bool>,
    #[serde(default)]
    pub has_bottom_panel: Option<bool>,
    #[serde(default)]
    pub bottom_panel_height_lh: Option<f32>,
    #[serde(default)]
    pub bottom_panel: Option<BottomPanelConfigDescriptor>,
}

/// What a render callback returns: a layout tree, or a flat frame with one
/// optional descriptor per widget kind.
#[derive(JsonSchema)]
#[serde(untagged)]
#[schemars(rename = "RenderOutput")]
pub enum RenderOutputShape {
    Tree(Box<LayoutNode>),
    Frame(RenderFrameDict),
}

/// A flat render frame: `{"<kind>": <descriptor> | null, ...}` for every
/// leaf kind except `scripted_widget`, plus `wire_version`.
pub struct RenderFrameDict;

impl JsonSchema for RenderFrameDict {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "RenderFrameDict".into()
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        let mut props = Map::new();
        props.insert(
            "wire_version".into(),
            json_schema!({"type": "integer", "minimum": 0}).to_value(),
        );
        for (kind, schema) in leaf_kind_schemas(generator) {
            if kind == "scripted_widget" {
                continue;
            }
            props.insert(
                kind.to_string(),
                json_schema!({"anyOf": [schema, {"type": "null"}]}).to_value(),
            );
        }
        json_schema!({
            "description": "A flat render frame: one optional descriptor per widget kind.",
            "type": "object",
            "properties": props,
        })
    }
}

/// The extra keys a windowed (`list`/`tree`/`data_table`) or
/// `text_display` leaf body accepts beyond its descriptor's own fields: the
/// body is the plain descriptor or `name`, the descriptor plus those keys.
fn leaf_body_with_extras<T: JsonSchema>(
    generator: &mut SchemaGenerator,
    name: &str,
    scroll_px: bool,
) -> Schema {
    let base = generator.subschema_for::<T>();
    let items_window = generator.subschema_for::<Option<crate::render::ItemsWindow>>();
    let mut extras = Map::new();
    extras.insert("items_window".into(), items_window.to_value());
    if scroll_px {
        extras.insert(
            "scroll_px".into(),
            json_schema!({"type": "number"}).to_value(),
        );
    }
    let def = json_schema!({
        "allOf": [base.clone(), {"type": "object", "properties": extras}],
    });
    generator
        .definitions_mut()
        .insert(name.to_string(), def.to_value());
    json_schema!({"anyOf": [base, {"$ref": format!("#/$defs/{name}")}]})
}

struct KindCollector<'g, 'k> {
    generator: &'g mut SchemaGenerator,
    only: Option<&'k [&'k str]>,
    out: Vec<(&'static str, Schema)>,
}

impl quadraui::KindVisitor for KindCollector<'_, '_> {
    fn visit<T: quadraui::KindSchema>(&mut self, kind: &'static str) {
        if self.only.is_some_and(|only| !only.contains(&kind)) {
            return;
        }
        self.out.push((kind, T::kind_schema(self.generator)));
    }
}

/// Every leaf widget kind the layout-tree decoder accepts, each paired with
/// the schema of its descriptor body: the fixed slots, the additive
/// primitives, the generic widget kinds and the runtime-registered kinds,
/// in that order.
pub fn leaf_kind_schemas(generator: &mut SchemaGenerator) -> Vec<(&'static str, Schema)> {
    let mut out: Vec<(&'static str, Schema)> = Vec::new();
    for kind in LEAF_KIND_KEYS {
        let schema = match kind {
            "text_display" => {
                leaf_body_with_extras::<quadraui::TextDisplay>(generator, "TextDisplayLeaf", false)
            }
            "list" => leaf_body_with_extras::<quadraui::ListView>(generator, "ListViewLeaf", true),
            "tree" => leaf_body_with_extras::<quadraui::TreeView>(generator, "TreeViewLeaf", true),
            "data_table" => {
                leaf_body_with_extras::<quadraui::DataTable>(generator, "DataTableLeaf", true)
            }
            "tab_bar" => generator.subschema_for::<quadraui::TabBar>(),
            "scripted_widget" => {
                generator.subschema_for::<crate::scripted::ScriptedWidgetDescriptor>()
            }
            other => unreachable!("LEAF_KIND_KEYS entry {other:?} has no schema arm"),
        };
        out.push((kind, schema));
    }
    let mut extras = KindCollector {
        generator,
        only: None,
        out: Vec::new(),
    };
    visit_extra_primitive_kinds(&mut extras);
    out.extend(extras.out);
    let mut widgets = KindCollector {
        generator,
        only: Some(&WIDGET_KINDS),
        out: Vec::new(),
    };
    quadraui::visit_widget_kinds(&mut widgets);
    out.extend(widgets.out);
    for kind in quadraui::widget_kinds::runtime_widget_kinds() {
        let schema = quadraui::runtime_widget_kind_schema(kind)
            .and_then(|value| Schema::try_from(value).ok())
            .unwrap_or_else(|| json_schema!({"type": "object"}));
        out.push((kind, schema));
    }
    out
}

fn root<T: JsonSchema>(generator: &mut SchemaGenerator, roots: &mut Map<String, Value>) {
    let schema = generator.subschema_for::<T>();
    roots.insert(T::schema_name().into_owned(), schema.to_value());
}

/// Builds the whole wire schema document. Deterministic: two calls in one
/// process with the same runtime registrations return equal values.
pub fn wire_schema() -> Value {
    let mut generator = SchemaSettings::draft2020_12().into_generator();
    let mut kinds = Map::new();
    for (kind, schema) in leaf_kind_schemas(&mut generator) {
        kinds.insert(kind.to_string(), schema.to_value());
    }
    let mut roots = Map::new();
    root::<quadraui::UiEvent>(&mut generator, &mut roots);
    root::<LayoutNode>(&mut generator, &mut roots);
    root::<RenderOutputShape>(&mut generator, &mut roots);
    root::<PopoverDescriptor>(&mut generator, &mut roots);
    root::<LayoutDialogDescriptor>(&mut generator, &mut roots);
    root::<crate::deferred::ContextMenuOverlayDescriptor>(&mut generator, &mut roots);
    root::<crate::deferred::MessageDialogOverlayDescriptor>(&mut generator, &mut roots);
    root::<ShellConfigDescriptor>(&mut generator, &mut roots);
    root::<crate::hit::HitDetail>(&mut generator, &mut roots);
    root::<crate::semantic::DragGestureWireEvent>(&mut generator, &mut roots);
    root::<quadraui::TreeTableModelEvent>(&mut generator, &mut roots);
    root::<crate::host::WidgetStateSnapshot>(&mut generator, &mut roots);
    root::<crate::services::UpdateInfoWire>(&mut generator, &mut roots);
    root::<crate::render::RowsRequestedEvent>(&mut generator, &mut roots);
    let defs: Map<String, Value> = generator
        .definitions()
        .iter()
        .map(|(name, schema)| (name.clone(), schema.clone()))
        .collect();
    let mut sorted: Vec<(String, Value)> = defs.into_iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    let mut doc = Map::new();
    doc.insert(
        "$schema".into(),
        "https://json-schema.org/draft/2020-12/schema".into(),
    );
    doc.insert("title".into(), "quadraui binding wire format".into());
    doc.insert("wire_version".into(), WIRE_VERSION.into());
    doc.insert("kinds".into(), Value::Object(kinds));
    doc.insert("roots".into(), Value::Object(roots));
    doc.insert("$defs".into(), Value::Object(sorted.into_iter().collect()));
    Value::Object(doc)
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use serde_json::{json, Value};

    use super::*;
    use crate::wire::layout::{
        COLUMN_SHAPE_KEYS, GRID_SHAPE_KEYS, ROW_SHAPE_KEYS, SCROLL_SHAPE_KEYS,
        SIDEBAR_PANEL_SHAPE_KEYS, SPLIT_SHAPE_KEYS, SPLIT_TREE_SHAPE_KEYS, TABS_SHAPE_KEYS,
    };
    use crate::wire::value::UNIVERSAL_NODE_SIBLING_KEYS;

    /// Serializes the tests that build the document against the one that
    /// registers a runtime kind, so no document sees a kind that is gone
    /// by the time the test decodes it.
    static RUNTIME_KINDS: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn doc() -> Value {
        let _guard = RUNTIME_KINDS.lock().unwrap_or_else(|e| e.into_inner());
        wire_schema()
    }

    fn resolve<'a>(doc: &'a Value, schema: &'a Value) -> &'a Value {
        match schema.get("$ref").and_then(Value::as_str) {
            Some(reference) => {
                let name = reference.trim_start_matches("#/$defs/");
                resolve(doc, &doc["$defs"][name])
            }
            None => schema,
        }
    }

    /// A schema-valid instance carrying only the required properties, with
    /// the simplest value of each type.
    fn minimal(doc: &Value, schema: &Value, depth: usize) -> Value {
        assert!(depth < 40, "minimal sample recursion too deep: {schema}");
        let schema = resolve(doc, schema);
        if let Some(value) = schema.get("const") {
            return value.clone();
        }
        if let Some(values) = schema.get("enum").and_then(Value::as_array) {
            return values[0].clone();
        }
        for key in ["oneOf", "anyOf"] {
            if let Some(options) = schema.get(key).and_then(Value::as_array) {
                let option = options
                    .iter()
                    .find(|o| resolve(doc, o).get("type") != Some(&json!("null")))
                    .unwrap_or(&options[0]);
                return minimal(doc, option, depth + 1);
            }
        }
        if let Some(parts) = schema.get("allOf").and_then(Value::as_array) {
            let mut merged = serde_json::Map::new();
            for part in parts {
                if let Value::Object(fields) = minimal(doc, part, depth + 1) {
                    merged.extend(fields);
                }
            }
            return Value::Object(merged);
        }
        let ty = match schema.get("type") {
            Some(Value::Array(types)) => types
                .iter()
                .find(|t| *t != "null")
                .cloned()
                .unwrap_or(json!("null")),
            Some(ty) => ty.clone(),
            None if schema.get("properties").is_some() => json!("object"),
            None => return json!({}),
        };
        match ty.as_str().unwrap_or("null") {
            "string" => json!("x"),
            "integer" => json!(schema.get("minimum").cloned().unwrap_or(json!(0))),
            "number" => json!(0.0),
            "boolean" => json!(false),
            "array" => match schema.get("prefixItems").and_then(Value::as_array) {
                Some(items) => Value::Array(
                    items
                        .iter()
                        .map(|item| minimal(doc, item, depth + 1))
                        .collect(),
                ),
                None => json!([]),
            },
            "object" => {
                let mut out = serde_json::Map::new();
                for name in schema
                    .get("required")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                {
                    let prop = &schema["properties"][name];
                    out.insert(name.to_string(), minimal(doc, prop, depth + 1));
                }
                Value::Object(out)
            }
            _ => Value::Null,
        }
    }

    fn validator_for_root(doc: &Value, root: &str) -> jsonschema::Validator {
        let mut schema = doc.clone();
        schema["$ref"] = doc["roots"][root]["$ref"].clone();
        jsonschema::validator_for(&schema).expect("the exported wire schema compiles")
    }

    fn assert_valid(validator: &jsonschema::Validator, instance: &Value) {
        let errors: Vec<String> = validator
            .iter_errors(instance)
            .map(|e| format!("{} at {}", e, e.instance_path()))
            .collect();
        assert!(errors.is_empty(), "{instance}: {errors:#?}");
    }

    #[test]
    fn the_document_names_its_version_and_every_kind() {
        let doc = doc();
        assert_eq!(doc["wire_version"], json!(WIRE_VERSION));
        let kinds: HashSet<&str> = doc["kinds"]
            .as_object()
            .expect("kinds map")
            .keys()
            .map(String::as_str)
            .collect();
        for kind in LEAF_KIND_KEYS
            .iter()
            .chain(crate::extra_paint::EXTRA_PRIMITIVE_KINDS.iter())
            .chain(WIDGET_KINDS.iter())
        {
            assert!(
                kinds.contains(kind),
                "kind {kind:?} missing from the schema"
            );
        }
        for (name, root) in doc["roots"].as_object().expect("roots map") {
            let reference = root["$ref"].as_str().expect("roots are references");
            let def = reference.trim_start_matches("#/$defs/");
            assert!(
                doc["$defs"].get(def).is_some(),
                "root {name} → missing def {def}"
            );
        }
    }

    #[test]
    fn a_runtime_kind_joins_the_kinds_map_with_its_schema() {
        fn decode(
            _kind: &str,
            value: serde_json::Value,
        ) -> Result<Box<dyn quadraui::primitive::DynWidget>, serde_json::Error> {
            let bar: quadraui::ProgressBar = serde_json::from_value(value)?;
            Ok(Box::new(bar))
        }
        fn schema() -> serde_json::Value {
            json!({"type": "object", "required": ["id"], "properties": {"id": {"type": "string"}}})
        }
        let doc = {
            let _guard = RUNTIME_KINDS.lock().unwrap_or_else(|e| e.into_inner());
            quadraui::register_widget_kind_with_schema("schema_test_runtime", decode, schema)
                .expect("registers");
            let doc = wire_schema();
            quadraui::unregister_widget_kind("schema_test_runtime");
            doc
        };
        assert_eq!(doc["kinds"]["schema_test_runtime"], schema());
        assert_eq!(
            doc["$defs"]["LayoutLeafNode"]["properties"]["schema_test_runtime"],
            schema()
        );
    }

    /// A minimal sample of every leaf kind, built from nothing but the
    /// schema's required properties, validates against the schema and
    /// decodes through the real layout-tree decoder: the schema never
    /// leaves out a field the decoder needs.
    #[test]
    fn a_minimal_sample_of_every_kind_validates_and_decodes() {
        let doc = doc();
        let validator = validator_for_root(&doc, "LayoutNode");
        for (kind, schema) in doc["kinds"].as_object().expect("kinds map") {
            let mut body = minimal(&doc, schema, 0);
            if let Some(fields) = body.as_object_mut() {
                if fields.contains_key("id") {
                    fields.insert("id".into(), json!(format!("{kind}-leaf")));
                }
            }
            let leaf = json!({"id": format!("{kind}-leaf"), kind.clone(): body});
            let round_tripped: Value =
                serde_json::from_str(&serde_json::to_string(&leaf).expect("serializes"))
                    .expect("parses");
            assert_valid(&validator, &round_tripped);
            if kind == "scripted_widget" {
                continue;
            }
            if let Err(err) = crate::wire::decode_layout_node("schema", &round_tripped) {
                panic!("minimal {kind} sample {round_tripped} fails to decode: {err}");
            }
        }
    }

    /// Container nodes the decoder accepts validate against the schema.
    #[test]
    fn decoded_layout_trees_validate() {
        let doc = doc();
        let validator = validator_for_root(&doc, "LayoutNode");
        let samples = [
            json!({"row": [{"id": "a", "button": {"id": "a", "label": "A"}}],
                   "gap": 4.0, "justify": "space_between", "align": "center",
                   "padding": {"x": 2.0}, "style": {"bg": "#112233", "hover": {"fg": {"r": 1, "g": 2, "b": 3, "a": 255}}},
                   "class": "primary wide", "size": {"Weight": 2.0}}),
            json!({"column": [], "wrap": true, "margin": 1.0, "class": ["a", "b"]}),
            json!({"split": {"id": "s", "orientation": "horizontal", "ratio": 0.5,
                             "first": {"column": []}, "second": {"row": []}}, "gap": 2.0}),
            json!({"tabs": {"id": "t", "selected": 0,
                            "tabs": [{"id": "one", "title": "One", "content": {"row": []}}]}}),
            json!({"scroll": {"id": "sc", "axis": "vertical", "offset": 3.0, "content": {"row": []}}}),
            json!({"sidebar_panel": {"id": "sp", "content": {"row": []}}}),
            json!({"split_tree": {"id": "st", "root": {"split": {"orientation": "vertical", "ratio": 0.3,
                    "first": {"leaf": {"row": []}}, "second": {"leaf": {"column": []}}}}}}),
            json!({"grid": {"columns": [{"Fixed": 10.0}], "rows": [{"Weight": 1.0}],
                            "cells": [{"row": []}]}}),
            json!({"id": "l", "list": {"id": "l", "items": [], "selected_idx": 0,
                   "scroll_px": 2.0, "items_window": {"start": 0, "total": 10}},
                   "draggable": true, "a11y": {"label": "Files", "live": "polite"}}),
        ];
        for sample in samples {
            if let Err(err) = crate::wire::decode_layout_node("schema", &sample) {
                panic!("sample {sample} fails to decode: {err}");
            }
            assert_valid(&validator, &sample);
        }
    }

    /// Every container's decoder-side sibling-key list names exactly the
    /// properties its schema type declares, less the universal keys.
    #[test]
    fn node_shape_key_constants_match_the_schema_properties() {
        let doc = doc();
        let cases: &[(&str, &[&str])] = &[
            ("LayoutRowNode", &ROW_SHAPE_KEYS),
            ("LayoutColumnNode", &COLUMN_SHAPE_KEYS),
            ("LayoutGridNode", &GRID_SHAPE_KEYS),
            ("LayoutSplitNode", &SPLIT_SHAPE_KEYS),
            ("LayoutTabsNode", &TABS_SHAPE_KEYS),
            ("LayoutSidebarPanelNode", &SIDEBAR_PANEL_SHAPE_KEYS),
            ("LayoutSplitTreeNode", &SPLIT_TREE_SHAPE_KEYS),
            ("LayoutScrollNode", &SCROLL_SHAPE_KEYS),
        ];
        for (def, shape_keys) in cases {
            let mut expected: HashSet<String> = doc["$defs"][def]["properties"]
                .as_object()
                .unwrap_or_else(|| panic!("{def} has no properties"))
                .keys()
                .cloned()
                .collect();
            for universal in UNIVERSAL_NODE_SIBLING_KEYS {
                expected.remove(universal);
            }
            let actual: HashSet<String> = shape_keys.iter().map(|s| s.to_string()).collect();
            assert_eq!(
                actual, expected,
                "{def}'s shape keys drifted from its schema"
            );
        }
    }

    /// Every `UiEvent` the engine emits validates against the schema.
    #[test]
    fn serialized_ui_events_validate() {
        let doc = doc();
        let validator = validator_for_root(&doc, "UiEvent");
        let events = [
            quadraui::UiEvent::WindowClose,
            quadraui::UiEvent::KeyPressed {
                key: quadraui::Key::Char('a'),
                modifiers: quadraui::Modifiers::default(),
                repeat: false,
            },
        ];
        for event in events {
            let value = serde_json::to_value(&event).expect("serializes");
            assert_valid(&validator, &value);
        }
    }
}
