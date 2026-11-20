//! Golden test for [`quadraui::A11yTree`]'s JSON form — the `snapshot`
//! MCP tool's wire shape documented in `src/a11y/tree.rs`'s "Stable JSON
//! form" module-doc section.
//!
//! Drives the same backend-independent seam the other `a11y_*_tree.rs`
//! tests use (`quadraui::testing::a11y::A11yScenario`), but asserts on
//! `serde_json::to_value(tree)` instead of the text snapshot: this is the
//! shape a `snapshot` tool actually hands an agent, and the text
//! snapshot deliberately omits bounds, ids and the per-node action list
//! this test exists to pin down.

use quadraui::testing::a11y::A11yScenario;
use quadraui::{
    A11yNodeId, Decoration, FieldKind, Form, FormField, ListItem, ListView, Rect, SelectionMode,
    StyledText, Surface, TreeRow, TreeStyle, TreeView, WidgetId,
};
use serde_json::json;

fn list_surface() -> Surface {
    Surface::List {
        rect: Rect::new(0.0, 0.0, 30.0, 5.0),
        list: ListView {
            id: WidgetId::new("files"),
            title: Some(StyledText::plain("Files")),
            items: vec![ListItem {
                text: StyledText::plain("a.rs"),
                icon: None,
                detail: None,
                decoration: Decoration::default(),
            }],
            selected_idx: 0,
            scroll_offset: 0,
            has_focus: false,
            bordered: false,
            h_scroll: 0,
            max_content_width: None,
            show_v_scrollbar: false,
        },
    }
}

fn tree_surface() -> Surface {
    Surface::Tree {
        rect: Rect::new(0.0, 5.0, 30.0, 5.0),
        tree: TreeView {
            id: WidgetId::new("proj"),
            rows: vec![TreeRow {
                path: vec![0],
                indent: 0,
                icon: None,
                text: StyledText::plain("src"),
                badge: None,
                is_expanded: None,
                decoration: Decoration::default(),
                edit: None,
            }],
            selection_mode: SelectionMode::Single,
            selected_path: None,
            scroll_offset: 0,
            style: TreeStyle::default(),
            has_focus: false,
        },
    }
}

fn form_surface() -> Surface {
    Surface::Form {
        rect: Rect::new(0.0, 10.0, 30.0, 5.0),
        form: Form {
            id: WidgetId::new("settings"),
            fields: vec![FormField {
                id: WidgetId::new("settings:notify"),
                label: StyledText::plain("Notify"),
                kind: FieldKind::Toggle { value: true },
                hint: StyledText::plain(""),
                disabled: false,
                validation: None,
                label_position: None,
                label_align: None,
                label_width: None,
                tab_index: None,
                control_width: None,
                min_control_width: None,
                row_align: None,
            }],
            focused_field: Some(WidgetId::new("settings:notify")),
            scroll_offset: 0,
            has_focus: true,
            label_position: Default::default(),
            label_align: Default::default(),
            label_width: None,
            control_width: Default::default(),
            min_control_width: None,
            spacing: Default::default(),
        },
    }
}

/// The whole tree's JSON form, over list/tree/form fixtures pushed
/// together the way a real screen would, matches a hand-built golden
/// document field for field — every id computed through the same
/// [`A11yNodeId::for_widget`]/[`A11yNodeId::child`] constructors
/// [`quadraui::a11y::route_action`] (not reachable from outside this
/// crate, so this test can't name it directly) resolves a `target`
/// against, so a JSON id really is a usable `act` target and not merely
/// a stable-looking number.
#[test]
fn list_tree_form_snapshot_matches_the_golden_json() {
    let scenario = A11yScenario::new()
        .with_surface(list_surface())
        .with_surface(tree_surface())
        .with_surface(form_surface())
        .with_focus(WidgetId::new("settings"));

    let tree = scenario.snapshot().tree().clone();
    let actual = serde_json::to_value(&tree).expect("A11yTree serializes");

    let list_id = A11yNodeId::for_widget(&WidgetId::new("files"));
    let list_item_id = list_id.child("item.0");
    let tree_id = A11yNodeId::for_widget(&WidgetId::new("proj"));
    let tree_row_id = tree_id.child("row.0");
    let form_id = A11yNodeId::for_widget(&WidgetId::new("settings"));
    let form_field_id = form_id.child("field.settings:notify");

    let expected = json!({
        "focus": form_field_id.as_u64(),
        "root": {
            "id": A11yNodeId::ROOT.as_u64(),
            "role": "window",
            "label": "quadraui",
            "focused": false,
            "bounds": {"x": 0.0, "y": 0.0, "width": 30.0, "height": 15.0},
            "actions": ["focus"],
            "children": [
                {
                    "id": list_id.as_u64(),
                    "widget_id": "files",
                    "role": "list",
                    "label": "Files",
                    "focused": false,
                    "bounds": {"x": 0.0, "y": 0.0, "width": 30.0, "height": 5.0},
                    "actions": ["focus"],
                    "children": [
                        {
                            "id": list_item_id.as_u64(),
                            "widget_id": "files",
                            "role": "list_item",
                            "label": "a.rs",
                            "focused": false,
                            "selected": true,
                            "actions": ["focus", "click"],
                            "children": []
                        }
                    ]
                },
                {
                    "id": tree_id.as_u64(),
                    "widget_id": "proj",
                    "role": "tree",
                    "focused": false,
                    "bounds": {"x": 0.0, "y": 5.0, "width": 30.0, "height": 5.0},
                    "actions": ["focus"],
                    "children": [
                        {
                            "id": tree_row_id.as_u64(),
                            "widget_id": "proj",
                            "role": "tree_item",
                            "label": "src",
                            "focused": false,
                            "selected": false,
                            "actions": ["focus", "click"],
                            "children": []
                        }
                    ]
                },
                {
                    "id": form_id.as_u64(),
                    "widget_id": "settings",
                    "role": "form",
                    "label": "Form",
                    "focused": false,
                    "bounds": {"x": 0.0, "y": 10.0, "width": 30.0, "height": 5.0},
                    "actions": ["focus"],
                    "children": [
                        {
                            "id": form_field_id.as_u64(),
                            "widget_id": "settings",
                            "role": "check_box",
                            "label": "Notify",
                            "focused": true,
                            "checked": "true",
                            "disabled": false,
                            "actions": ["focus", "click"],
                            "children": []
                        }
                    ]
                }
            ]
        }
    });

    assert_eq!(
        actual,
        expected,
        "\n--- actual ---\n{}\n--- expected ---\n{}\n",
        serde_json::to_string_pretty(&actual).unwrap(),
        serde_json::to_string_pretty(&expected).unwrap(),
    );
}

/// Node ids in the JSON are exactly the ids
/// [`quadraui::a11y::A11yTree::node`]'s own lookup uses — the acceptance
/// bar this whole feature exists to meet, spelled out as its own
/// assertion rather than left implicit in the golden document above.
#[test]
fn json_node_ids_equal_the_real_a11y_node_ids() {
    let scenario = A11yScenario::new().with_surface(list_surface());
    let tree = scenario.snapshot().tree().clone();
    let actual = serde_json::to_value(&tree).unwrap();

    let list_id = A11yNodeId::for_widget(&WidgetId::new("files"));
    let item_id = list_id.child("item.0");

    assert_eq!(actual["root"]["id"], json!(A11yNodeId::ROOT.as_u64()));
    let children = actual["root"]["children"].as_array().unwrap();
    assert_eq!(children[0]["id"], json!(list_id.as_u64()));
    assert!(tree.node(list_id).is_some());

    let grandchildren = children[0]["children"].as_array().unwrap();
    assert_eq!(grandchildren[0]["id"], json!(item_id.as_u64()));
    assert!(tree.node(item_id).is_some());
}

/// A field this builder never populates (`total_rows`, since no
/// primitive's row-loading window feeds it yet) is omitted from the JSON
/// entirely rather than serialized as `null` — see `src/a11y/tree.rs`'s
/// "Stable JSON form" section.
#[test]
fn absent_optional_fields_are_omitted_not_null() {
    let scenario = A11yScenario::new().with_surface(list_surface());
    let tree = scenario.snapshot().tree().clone();
    let actual = serde_json::to_value(&tree).unwrap();

    let list_json = &actual["root"]["children"][0];
    assert!(list_json.get("total_rows").is_none());
    assert!(list_json.get("description").is_none());
    assert!(list_json.get("value").is_none());

    let item_json = &list_json["children"][0];
    assert!(item_json.get("bounds").is_none());
    assert!(item_json.get("widget_id").is_some());
}
