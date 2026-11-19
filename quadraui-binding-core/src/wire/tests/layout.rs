use std::collections::HashSet;

use serde_json::json;

use crate::wire::test_support::{decode, err};
use crate::wire::value::UNIVERSAL_NODE_SIBLING_KEYS;

use super::*;

/// A `"split"` node round-trips its id, orientation and ratio, and
/// recursively decodes its two named panes.
#[test]
fn round_trips_a_split_node() {
    let value = json!({
        "split": {
            "id": "s1",
            "orientation": "vertical",
            "ratio": 0.4,
            "first": {"id": "a", "button": {"id": "a", "label": "A"}},
            "second": {"id": "b", "button": {"id": "b", "label": "B"}},
        }
    });
    let node = decode(&value).expect("valid split");
    let LayoutNode::Split(split) = node else {
        panic!("expected a split node");
    };
    assert_eq!(split.split.id.as_str(), "s1");
    assert_eq!(split.split.direction, SplitDirection::Vertical);
    assert_eq!(split.split.ratio, 0.4);
    assert!(matches!(*split.first, LayoutNode::Leaf(_)));
    assert!(matches!(*split.second, LayoutNode::Leaf(_)));
}

/// A `"row"`'s `"padding"`/`"margin"`/`"gap"` keys are siblings of
/// the `"row"` key in the same node dict, decoded into
/// [`crate::layout::Spacing`] alongside the children. `"padding"` as
/// a bare number applies uniformly to every edge.
#[test]
fn round_trips_a_row_node_s_spacing_keys() {
    let value = json!({
        "row": [{"id": "a", "button": {"id": "a", "label": "A"}}],
        "padding": 12,
        "margin": 4,
        "gap": 8,
    });
    let node = decode(&value).expect("valid row");
    let LayoutNode::Row(container) = node else {
        panic!("expected a row node");
    };
    assert_eq!(container.children.len(), 1);
    assert_eq!(container.spacing.padding, EdgeInsets::uniform(12.0));
    assert_eq!(container.spacing.margin, 4.0);
    assert_eq!(container.spacing.gap, 8.0);
}

/// `"padding"` also accepts an `{"x": .., "y": ..}` axis pair, applying
/// `x` to the left/right edges and `y` to top/bottom.
#[test]
fn row_padding_accepts_an_x_y_axis_pair() {
    let value = json!({
        "row": [{"id": "a", "button": {"id": "a", "label": "A"}}],
        "padding": {"x": 6, "y": 2},
    });
    let node = decode(&value).expect("valid row");
    let LayoutNode::Row(container) = node else {
        panic!("expected a row node");
    };
    assert_eq!(container.spacing.padding, EdgeInsets::xy(6.0, 2.0));
}

/// A `"row"` with no spacing keys at all -- the shape every
/// pre-existing render callback uses -- decodes to [`Spacing::default`]
/// (no inset, no gap), unchanged from before spacing keys existed.
#[test]
fn row_with_no_spacing_keys_decodes_to_the_default_spacing() {
    let value = json!({
        "row": [{"id": "a", "button": {"id": "a", "label": "A"}}],
    });
    let node = decode(&value).expect("valid row");
    let LayoutNode::Row(container) = node else {
        panic!("expected a row node");
    };
    assert_eq!(container.spacing, Spacing::default());
}

/// A row child's own `"size"`/`"min"`/`"max"` keys are siblings of
/// whatever key names its node kind, in the same child dict -- round
/// trips every [`SectionSize`] wire shape (bare string for the two
/// unit variants, `{"Fixed": n}`/`{"Percent": p}`/`{"Weight": w}` for
/// the three data-carrying ones), the same representation
/// `quadraui::MultiSectionView`'s own per-section `"size"` key uses.
#[test]
fn round_trips_a_row_childs_size_min_max_keys() {
    let value = json!({
        "row": [
            {"id": "a", "button": {"id": "a", "label": "A"}, "size": {"Fixed": 40}, "min": 10, "max": 50},
            {"id": "b", "button": {"id": "b", "label": "B"}, "size": "Content"},
            {"id": "c", "button": {"id": "c", "label": "C"}, "size": {"Percent": 0.3}},
            {"id": "d", "button": {"id": "d", "label": "D"}, "size": {"Weight": 2.0}},
            {"id": "e", "button": {"id": "e", "label": "E"}},
        ],
    });
    let node = decode(&value).expect("valid row with per-child sizing");
    let LayoutNode::Row(container) = node else {
        panic!("expected a row node");
    };
    assert_eq!(container.children[0].size, SectionSize::Fixed(40));
    assert_eq!(container.children[0].min, Some(10.0));
    assert_eq!(container.children[0].max, Some(50.0));
    assert_eq!(container.children[1].size, SectionSize::Content);
    assert_eq!(container.children[2].size, SectionSize::Percent(0.3));
    assert_eq!(container.children[3].size, SectionSize::Weight(2.0));
    // No `"size"` key at all defaults to `EqualShare`, with no
    // min/max -- the pre-per-child-sizing shape.
    assert_eq!(container.children[4].size, SectionSize::EqualShare);
    assert_eq!(container.children[4].min, None);
    assert_eq!(container.children[4].max, None);
}

/// `{"Percent": 25}` is a plausible "25 percent" typo -- rejected at
/// decode rather than silently read as `2500%` and clamped to `100%`
/// by `crate::layout::resolve_main_axis_sizes`.
#[test]
fn rejects_a_child_percent_size_outside_zero_to_one() {
    let value = json!({
        "row": [
            {"id": "a", "button": {"id": "a", "label": "A"}, "size": {"Percent": 25}},
        ],
    });
    let err = match decode(&value) {
        Ok(_) => panic!("expected an out-of-range Percent size to be rejected"),
        Err(err) => err,
    };
    assert!(err.to_string().contains("Percent"), "{}", err.to_string());
}

/// A `"row"`/`"column"` node's `"justify"`/`"align"` keys are siblings
/// of the `"row"`/`"column"` key, same as `"padding"`/`"margin"`/
/// `"gap"`.
#[test]
fn round_trips_a_row_nodes_justify_and_align_keys() {
    let value = json!({
        "row": [{"id": "a", "button": {"id": "a", "label": "A"}}],
        "justify": "space_between",
        "align": "center",
    });
    let node = decode(&value).expect("valid row with justify/align");
    let LayoutNode::Row(container) = node else {
        panic!("expected a row node");
    };
    assert_eq!(container.justify, Justify::SpaceBetween);
    assert_eq!(container.align, Align::Center);
}

/// Missing `"justify"`/`"align"` default to `Start`/`Stretch` -- the
/// pre-`justify`/`align` behaviour (every child filled the full
/// cross-axis extent, leftover main-axis space went after the last
/// child -- a no-op when every child is `EqualShare`).
#[test]
fn row_with_no_justify_or_align_key_decodes_to_the_default() {
    let value = json!({
        "row": [{"id": "a", "button": {"id": "a", "label": "A"}}],
    });
    let node = decode(&value).expect("valid row");
    let LayoutNode::Row(container) = node else {
        panic!("expected a row node");
    };
    assert_eq!(container.justify, Justify::Start);
    assert_eq!(container.align, Align::Stretch);
}

/// An unrecognized `"justify"` value is a decode error naming the
/// offending string, not a silent fallback to the default.
#[test]
fn rejects_an_unknown_justify_value() {
    let value = json!({
        "row": [{"id": "a", "button": {"id": "a", "label": "A"}}],
        "justify": "spread",
    });
    let err = match decode(&value) {
        Ok(_) => panic!("expected an unknown justify value to be rejected"),
        Err(err) => err,
    };
    assert!(err.to_string().contains("justify"), "{}", err.to_string());
}

/// [`rejects_an_unknown_justify_value`]'s counterpart for `"align"`.
#[test]
fn rejects_an_unknown_align_value() {
    let value = json!({
        "row": [{"id": "a", "button": {"id": "a", "label": "A"}}],
        "align": "top",
    });
    let err = match decode(&value) {
        Ok(_) => panic!("expected an unknown align value to be rejected"),
        Err(err) => err,
    };
    assert!(err.to_string().contains("align"), "{}", err.to_string());
}

/// A `"row"`/`"column"` node's `"wrap"` key is a sibling of the `"row"`/
/// `"column"` key, same as `"justify"`/`"align"`.
#[test]
fn round_trips_a_row_nodes_wrap_key() {
    let value = json!({
        "row": [{"id": "a", "button": {"id": "a", "label": "A"}}],
        "wrap": true,
    });
    let node = decode(&value).expect("valid row with wrap");
    let LayoutNode::Row(container) = node else {
        panic!("expected a row node");
    };
    assert!(container.wrap);

    let value = json!({
        "column": [{"id": "a", "button": {"id": "a", "label": "A"}}],
        "wrap": true,
    });
    let node = decode(&value).expect("valid column with wrap");
    let LayoutNode::Column(container) = node else {
        panic!("expected a column node");
    };
    assert!(container.wrap);
}

/// Missing `"wrap"` defaults to `false` -- the pre-`wrap` behaviour, every
/// child laid out on a single line regardless of overflow.
#[test]
fn row_with_no_wrap_key_decodes_to_the_default() {
    let value = json!({
        "row": [{"id": "a", "button": {"id": "a", "label": "A"}}],
    });
    let node = decode(&value).expect("valid row");
    let LayoutNode::Row(container) = node else {
        panic!("expected a row node");
    };
    assert!(!container.wrap);
}

/// A non-bool `"wrap"` value is a decode error naming the key, not a
/// silent coercion.
#[test]
fn rejects_a_non_bool_wrap_value() {
    let value = json!({
        "row": [{"id": "a", "button": {"id": "a", "label": "A"}}],
        "wrap": "yes",
    });
    let err = match decode(&value) {
        Ok(_) => panic!("expected a non-bool wrap value to be rejected"),
        Err(err) => err,
    };
    assert!(err.to_string().contains("wrap"), "{}", err.to_string());
}

/// A `"split"` node is itself a valid row/column child, so it takes
/// the same sibling `"size"`/`"min"`/`"max"` keys a leaf child does --
/// unrelated to (and not rejected alongside) its own `"padding"`/
/// `"margin"`/`"gap"` spacing keys.
#[test]
fn a_split_child_of_a_row_accepts_size_min_max() {
    let value = json!({
        "row": [
            {
                "split": {
                    "id": "s1",
                    "orientation": "horizontal",
                    "ratio": 0.5,
                    "first": {"id": "a", "button": {"id": "a", "label": "A"}},
                    "second": {"id": "b", "button": {"id": "b", "label": "B"}},
                },
                "size": {"Fixed": 30},
            },
            {"id": "c", "button": {"id": "c", "label": "C"}},
        ],
    });
    let node = decode(&value).expect("valid row with a sized split child");
    let LayoutNode::Row(container) = node else {
        panic!("expected a row node");
    };
    assert_eq!(container.children[0].size, SectionSize::Fixed(30));
    assert!(matches!(container.children[0].node, LayoutNode::Split(_)));
}

/// A `"tabs"` node's `"padding"`/`"margin"` keys decode the same way a
/// `"split"` node's do.
#[test]
fn round_trips_a_tabs_node_s_padding_and_margin() {
    let value = json!({
        "tabs": {
            "id": "t1",
            "tabs": [
                {"id": "one", "title": "One", "content": {"id": "one-leaf", "text_display": {"id": "one-leaf", "lines": []}}},
            ],
        },
        "padding": 6,
        "margin": 2,
    });
    let node = decode(&value).expect("valid tabs with spacing");
    let LayoutNode::Tabs(tabs) = node else {
        panic!("expected a tabs node");
    };
    assert_eq!(tabs.spacing.padding, EdgeInsets::uniform(6.0));
    assert_eq!(tabs.spacing.margin, 2.0);
    assert_eq!(tabs.spacing.gap, 0.0);
}

/// `"gap"` has no meaning on a `"tabs"` node either.
#[test]
fn tabs_rejects_a_gap_key_it_does_not_support() {
    let value = json!({
        "tabs": {
            "id": "t1",
            "tabs": [
                {"id": "one", "title": "One", "content": {"id": "one-leaf", "text_display": {"id": "one-leaf", "lines": []}}},
            ],
        },
        "gap": 8,
    });
    let error = err(decode(&value));
    assert!(error.to_string().contains("gap"), "{}", error.to_string());
}

/// A `"split"` node's `"padding"`/`"margin"`/`"gap"` keys all decode --
/// unlike `"tabs"`, `"gap"` has a real meaning here: the
/// space between the two panes.
#[test]
fn round_trips_a_split_node_s_spacing_keys() {
    let value = json!({
        "split": {
            "id": "s1",
            "orientation": "horizontal",
            "ratio": 0.5,
            "first": {"id": "a", "button": {"id": "a", "label": "A"}},
            "second": {"id": "b", "button": {"id": "b", "label": "B"}},
        },
        "padding": 10,
        "margin": 3,
        "gap": 6,
    });
    let node = decode(&value).expect("valid split with spacing");
    let LayoutNode::Split(split) = node else {
        panic!("expected a split node");
    };
    assert_eq!(split.spacing.padding, EdgeInsets::uniform(10.0));
    assert_eq!(split.spacing.margin, 3.0);
    assert_eq!(split.spacing.gap, 6.0);
}

/// A `"sidebar_panel"` node still rejects every spacing key -- only
/// `"row"`/`"column"`/`"split"`/`"tabs"` nodes support any
/// of them.
#[test]
fn sidebar_panel_rejects_a_padding_key_it_does_not_support_yet() {
    let value = json!({
        "sidebar_panel": {
            "id": "sb1",
            "content": {"id": "sb1-leaf", "button": {"id": "sb1-leaf", "label": "Go"}},
        },
        "padding": 12,
    });
    let error = err(decode(&value));
    assert!(
        error.to_string().contains("padding"),
        "{}",
        error.to_string()
    );
}

/// An unknown key inside a `"padding"` object (a typo like `"lef"`)
/// is a decode error, not a silently-zero `left`.
#[test]
fn row_padding_rejects_an_unknown_object_key() {
    let value = json!({
        "row": [{"id": "a", "button": {"id": "a", "label": "A"}}],
        "padding": {"lef": 4},
    });
    let error = err(decode(&value));
    assert!(error.to_string().contains("lef"), "{}", error.to_string());
}

/// Mixing the `{"x", "y"}` axis shape with the `{"top", ...}` side
/// shape in the same `"padding"` object is a decode error instead of
/// silently dropping whichever side keys were present.
#[test]
fn row_padding_rejects_mixing_axis_and_side_keys() {
    let value = json!({
        "row": [{"id": "a", "button": {"id": "a", "label": "A"}}],
        "padding": {"x": 6, "top": 3},
    });
    let error = err(decode(&value));
    assert!(error.to_string().contains("mix"), "{}", error.to_string());
}

/// A `"tabs"` node marks the `"selected"` entry active and carries
/// every other tab's content un-selected.
#[test]
fn round_trips_a_tabs_node() {
    let value = json!({
        "tabs": {
            "id": "t1",
            "selected": 1,
            "tabs": [
                {"id": "one", "title": "One", "content": {"id": "one-leaf", "text_display": {"id": "one-leaf", "lines": []}}},
                {"id": "two", "title": "Two", "content": {"id": "two-leaf", "text_display": {"id": "two-leaf", "lines": []}}},
            ],
        }
    });
    let node = decode(&value).expect("valid tabs");
    let LayoutNode::Tabs(tabs) = node else {
        panic!("expected a tabs node");
    };
    assert_eq!(tabs.selected, 1);
    assert_eq!(tabs.tab_bar.tabs.len(), 2);
    assert!(!tabs.tab_bar.tabs[0].is_active);
    assert!(tabs.tab_bar.tabs[1].is_active);
    assert_eq!(tabs.entries.len(), 2);
}

/// A `"tabs"` node's optional `"focused_index"` key decodes to
/// `LayoutTabs::focused_index`, independent of `"selected"` — a py/
/// node app can keyboard-focus a different tab than the one whose
/// content is currently shown.
#[test]
fn round_trips_a_tabs_node_s_focused_index() {
    let value = json!({
        "tabs": {
            "id": "t1",
            "selected": 1,
            "focused_index": 0,
            "tabs": [
                {"id": "one", "title": "One", "content": {"id": "one-leaf", "text_display": {"id": "one-leaf", "lines": []}}},
                {"id": "two", "title": "Two", "content": {"id": "two-leaf", "text_display": {"id": "two-leaf", "lines": []}}},
            ],
        }
    });
    let node = decode(&value).expect("valid tabs");
    let LayoutNode::Tabs(tabs) = node else {
        panic!("expected a tabs node");
    };
    assert_eq!(tabs.focused_index, Some(0));
    assert_eq!(tabs.selected, 1);
}

/// Omitting `"focused_index"` entirely decodes to `None` — no
/// keyboard-focus treatment painted, matching every other optional
/// `"tabs"` key's absent-means-default convention.
#[test]
fn tabs_node_focused_index_defaults_to_none() {
    let value = json!({
        "tabs": {
            "id": "t1",
            "tabs": [
                {"id": "one", "title": "One", "content": {"id": "one-leaf", "text_display": {"id": "one-leaf", "lines": []}}},
            ],
        }
    });
    let node = decode(&value).expect("valid tabs");
    let LayoutNode::Tabs(tabs) = node else {
        panic!("expected a tabs node");
    };
    assert_eq!(tabs.focused_index, None);
}

/// A `"focused_index"` past the end of `"tabs"` is a decode error, the
/// same out-of-range convention `"selected"` already uses.
#[test]
fn tabs_node_rejects_an_out_of_range_focused_index() {
    let value = json!({
        "tabs": {
            "id": "t1",
            "focused_index": 5,
            "tabs": [
                {"id": "one", "title": "One", "content": {"id": "one-leaf", "text_display": {"id": "one-leaf", "lines": []}}},
            ],
        }
    });
    let error = err(decode(&value));
    assert!(
        error.to_string().contains("focused_index"),
        "{}",
        error.to_string()
    );
}

/// `"focused_index": 0` on an empty `"tabs"` array decodes without
/// error — the same `!items.is_empty() && idx >= items.len()` escape
/// hatch `"selected"` already uses two blocks up, so a `0` default
/// payload for an as-yet-empty tab group doesn't trip the range check
/// before there's anything to be out of range of.
#[test]
fn tabs_node_allows_a_zero_focused_index_on_an_empty_tab_list() {
    let value = json!({
        "tabs": {
            "id": "t1",
            "focused_index": 0,
            "tabs": [],
        }
    });
    let node = decode(&value).expect("valid tabs");
    let LayoutNode::Tabs(tabs) = node else {
        panic!("expected a tabs node");
    };
    assert_eq!(tabs.focused_index, Some(0));
}

/// A `"sidebar_panel"` node decodes its chrome generically and recurses
/// into its required `"content"` child.
#[test]
fn round_trips_a_sidebar_panel_node() {
    let value = json!({
        "sidebar_panel": {
            "id": "sb1",
            "content": {"id": "sb1-leaf", "button": {"id": "sb1-leaf", "label": "Go"}},
        }
    });
    let node = decode(&value).expect("valid sidebar_panel");
    let LayoutNode::SidebarPanel(sidebar) = node else {
        panic!("expected a sidebar_panel node");
    };
    assert_eq!(sidebar.sidebar_panel.id.as_str(), "sb1");
    assert!(matches!(*sidebar.content, LayoutNode::Leaf(_)));
}

/// A `"split_tree"` node builds a `SplitTree` geometry with one leaf
/// per `{"leaf": ...}` entry, in pre-order.
#[test]
fn round_trips_a_split_tree_node() {
    let value = json!({
        "split_tree": {
            "id": "st1",
            "root": {
                "split": {
                    "orientation": "horizontal",
                    "ratio": 0.5,
                    "first": {"leaf": {"id": "a", "button": {"id": "a", "label": "A"}}},
                    "second": {"leaf": {"id": "b", "button": {"id": "b", "label": "B"}}},
                }
            },
        }
    });
    let node = decode(&value).expect("valid split_tree");
    let LayoutNode::SplitTree(split_tree) = node else {
        panic!("expected a split_tree node");
    };
    assert_eq!(split_tree.id, "st1");
    assert_eq!(split_tree.tree.leaf_count(), 2);
    assert_eq!(split_tree.tree.split_count(), 1);
    assert_eq!(split_tree.leaves.len(), 2);
}

/// A `"scroll"` node with no `"axis"`/`"offset"` key defaults to a
/// vertical, uncontrolled scroll -- the shape every pre-`"offset"`
/// caller would decode to.
#[test]
fn round_trips_a_minimal_scroll_node() {
    let value = json!({
        "scroll": {
            "id": "sc1",
            "content": {"id": "sc1-leaf", "button": {"id": "sc1-leaf", "label": "Go"}},
        }
    });
    let node = decode(&value).expect("valid scroll");
    let LayoutNode::Scroll(scroll) = node else {
        panic!("expected a scroll node");
    };
    assert_eq!(scroll.id, "sc1");
    assert_eq!(scroll.axis, ScrollAxis::Vertical);
    assert_eq!(scroll.offset, None);
    assert!(matches!(*scroll.content, LayoutNode::Leaf(_)));
}

/// A `"scroll"` node's `"axis"`/`"offset"` keys round-trip -- `"offset"`
/// present pins the controlled scroll position (see
/// `crate::layout::LayoutScroll::offset`'s own doc).
#[test]
fn round_trips_a_horizontal_scroll_node_with_a_controlled_offset() {
    let value = json!({
        "scroll": {
            "id": "sc1",
            "axis": "horizontal",
            "offset": 20.0,
            "content": {"id": "sc1-leaf", "button": {"id": "sc1-leaf", "label": "Go"}},
        }
    });
    let node = decode(&value).expect("valid scroll");
    let LayoutNode::Scroll(scroll) = node else {
        panic!("expected a scroll node");
    };
    assert_eq!(scroll.axis, ScrollAxis::Horizontal);
    assert_eq!(scroll.offset, Some(20.0));
}

/// A `"scroll"` node's `"padding"`/`"margin"` keys decode the same way
/// a `"tabs"` node's do.
#[test]
fn round_trips_a_scroll_node_s_padding_and_margin() {
    let value = json!({
        "scroll": {
            "id": "sc1",
            "content": {"id": "sc1-leaf", "button": {"id": "sc1-leaf", "label": "Go"}},
        },
        "padding": 12,
        "margin": 4,
    });
    let node = decode(&value).expect("valid scroll with spacing");
    let LayoutNode::Scroll(scroll) = node else {
        panic!("expected a scroll node");
    };
    assert_eq!(scroll.spacing.padding, EdgeInsets::uniform(12.0));
    assert_eq!(scroll.spacing.margin, 4.0);
    assert_eq!(scroll.spacing.gap, 0.0);
}

/// `"gap"` has no meaning on a `"scroll"` node -- there's no second
/// sibling to space it from -- so it's a decode error, same as
/// `"tabs"`'s own.
#[test]
fn scroll_rejects_a_gap_key_it_does_not_support() {
    let value = json!({
        "scroll": {
            "id": "sc1",
            "content": {"id": "sc1-leaf", "button": {"id": "sc1-leaf", "label": "Go"}},
        },
        "gap": 8,
    });
    let error = err(decode(&value));
    assert!(error.to_string().contains("gap"), "{}", error.to_string());
}

/// A `"grid"` node round-trips both track lists and its row-major
/// `"cells"`, recursively decoding each cell as a full layout node.
#[test]
fn round_trips_a_grid_node() {
    let value = json!({
        "grid": {
            "columns": [{"Weight": 1.0}, {"Fixed": 24.0}],
            "rows": [{"Percent": 0.5}, {"Content": 16.0}],
            "cells": [
                {"id": "a", "button": {"id": "a", "label": "A"}},
                {"id": "b", "button": {"id": "b", "label": "B"}},
                {"id": "c", "button": {"id": "c", "label": "C"}},
            ],
        }
    });
    let node = decode(&value).expect("valid grid");
    let LayoutNode::Grid(grid) = node else {
        panic!("expected a grid node");
    };
    assert_eq!(
        grid.columns,
        vec![GridTrack::Weight(1.0), GridTrack::Fixed(24.0)]
    );
    assert_eq!(
        grid.rows,
        vec![GridTrack::Percent(0.5), GridTrack::Content(16.0)]
    );
    // Three cells in a 2x2 grid: the trailing cell stays empty.
    assert_eq!(grid.children.len(), 3);
    assert!(grid
        .children
        .iter()
        .all(|c| matches!(c, LayoutNode::Leaf(_))));
}

/// A `"grid"` node's `"padding"`/`"margin"` keys decode the same way a
/// `"scroll"` node's do; `"gap"` is rejected, since grid tracks carry no
/// gutter of their own.
#[test]
fn round_trips_a_grid_node_s_padding_and_margin_and_rejects_gap() {
    let spaced = json!({
        "grid": {
            "columns": [{"Weight": 1.0}],
            "rows": [{"Weight": 1.0}],
            "cells": [{"id": "a", "button": {"id": "a", "label": "A"}}],
        },
        "padding": 12,
        "margin": 4,
    });
    let node = decode(&spaced).expect("valid grid with spacing");
    let LayoutNode::Grid(grid) = node else {
        panic!("expected a grid node");
    };
    assert_eq!(grid.spacing.padding, EdgeInsets::uniform(12.0));
    assert_eq!(grid.spacing.margin, 4.0);
    assert_eq!(grid.spacing.gap, 0.0);

    let gapped = json!({
        "grid": {
            "columns": [{"Weight": 1.0}],
            "rows": [{"Weight": 1.0}],
            "cells": [{"id": "a", "button": {"id": "a", "label": "A"}}],
        },
        "gap": 8,
    });
    let error = err(decode(&gapped));
    assert!(error.to_string().contains("gap"), "{}", error.to_string());
}

/// More `"cells"` than `columns x rows` has room for is a decode error
/// naming both counts -- the overflow has nowhere to paint, and dropping
/// it silently would read as "my last tile vanished".
#[test]
fn grid_rejects_more_cells_than_its_tracks_have_room_for() {
    let value = json!({
        "grid": {
            "columns": [{"Weight": 1.0}, {"Weight": 1.0}],
            "rows": [{"Weight": 1.0}],
            "cells": [
                {"id": "a", "button": {"id": "a", "label": "A"}},
                {"id": "b", "button": {"id": "b", "label": "B"}},
                {"id": "c", "button": {"id": "c", "label": "C"}},
            ],
        }
    });
    let error = err(decode(&value));
    let text = error.to_string();
    assert!(text.contains("cells"), "{text}");
    assert!(text.contains('3'), "{text}");
    assert!(text.contains('2'), "{text}");
}

/// A `{"Percent": p}` track outside `0.0..=1.0` is a decode error, the
/// same range check a row/column child's own `"size"` key applies -- so
/// `{"Percent": 25}` meaning "25 percent" is caught instead of painting
/// one track over the whole axis.
#[test]
fn grid_rejects_an_out_of_range_percent_track() {
    let value = json!({
        "grid": {
            "columns": [{"Percent": 25}],
            "rows": [{"Weight": 1.0}],
            "cells": [{"id": "a", "button": {"id": "a", "label": "A"}}],
        }
    });
    let error = err(decode(&value));
    let text = error.to_string();
    assert!(text.contains("columns"), "{text}");
    assert!(text.contains("Percent"), "{text}");
}

/// A `"grid"` node with no `"columns"`/`"rows"`/`"cells"` key names the
/// missing one rather than decoding to an empty grid that paints nothing.
#[test]
fn grid_requires_its_columns_rows_and_cells_keys() {
    for missing in ["columns", "rows", "cells"] {
        let mut grid = serde_json::Map::new();
        grid.insert("columns".into(), json!([{"Weight": 1.0}]));
        grid.insert("rows".into(), json!([{"Weight": 1.0}]));
        grid.insert("cells".into(), json!([]));
        grid.remove(missing);
        let error = err(decode(&json!({"grid": grid})));
        assert!(error.to_string().contains(missing), "{error}");
    }
}

/// An unrecognized `"axis"` value is a decode error naming the
/// accepted ones, not a silent fall-back to vertical.
#[test]
fn scroll_rejects_an_unknown_axis_value() {
    let value = json!({
        "scroll": {
            "id": "sc1",
            "axis": "diagonal",
            "content": {"id": "sc1-leaf", "button": {"id": "sc1-leaf", "label": "Go"}},
        }
    });
    let error = err(decode(&value));
    assert!(error.to_string().contains("axis"), "{}", error.to_string());
}

/// A scroll node missing its required `"id"` key is rejected, same as
/// `"split"`'s own.
#[test]
fn rejects_a_scroll_missing_id() {
    let value = json!({
        "scroll": {
            "content": {"id": "sc1-leaf", "button": {"id": "sc1-leaf", "label": "Go"}},
        }
    });
    let error = err(decode(&value));
    assert!(error.to_string().contains("id"), "{}", error.to_string());
}

/// A split node missing its required `"id"` key is rejected.
#[test]
fn rejects_a_split_missing_id() {
    let value = json!({
        "split": {
            "orientation": "horizontal",
            "ratio": 0.5,
            "first": {"id": "a", "button": {"id": "a", "label": "A"}},
            "second": {"id": "b", "button": {"id": "b", "label": "B"}},
        }
    });
    let err = err(decode(&value));
    assert!(matches!(err, DecodeError::MissingField { .. }), "{err:?}");
    assert!(
        err.to_string().contains("missing field"),
        "{}",
        err.to_string()
    );
    assert!(err.to_string().contains("split.id"), "{}", err.to_string());
}

/// A split's `"orientation"` must be `"horizontal"` or `"vertical"`.
#[test]
fn rejects_a_bad_orientation() {
    let value = json!({
        "split": {
            "id": "s1",
            "orientation": "diagonal",
            "ratio": 0.5,
            "first": {"id": "a", "button": {"id": "a", "label": "A"}},
            "second": {"id": "b", "button": {"id": "b", "label": "B"}},
        }
    });
    let err = err(decode(&value));
    assert!(
        err.to_string().contains("must be one of"),
        "{}",
        err.to_string()
    );
}

/// A tabs node's `"selected"` index must be in range for its tab list.
#[test]
fn rejects_an_out_of_range_selected_index() {
    let value = json!({
        "tabs": {
            "id": "t1",
            "selected": 5,
            "tabs": [
                {"id": "one", "title": "One", "content": {"id": "one-leaf", "button": {"id": "one-leaf", "label": "A"}}},
            ],
        }
    });
    let err = err(decode(&value));
    assert!(
        err.to_string().contains("out of range"),
        "{}",
        err.to_string()
    );
}

/// The field names `crate::wire_schema::schema()` declares for `def_name`
/// — [`crate::wire_schema::DefJson::Obj`]'s own field list, flattened to
/// just the names. Panics if no such `Obj` def exists, so a typo'd
/// `def_name` in a caller below fails loudly rather than silently
/// comparing against an empty set.
fn schema_field_names(def_name: &str) -> HashSet<String> {
    crate::wire_schema::schema()
        .into_iter()
        .find_map(|def| match def {
            crate::wire_schema::DefJson::Obj { name, fields, .. } if name == def_name => {
                Some(fields.into_iter().map(|f| f.name).collect())
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("no such schema `Obj` def: {def_name}"))
}

/// Every per-kind sibling-key constant [`layout_node_from_value_inner`]
/// passes to [`check_node_sibling_keys`] (plus [`UNIVERSAL_NODE_SIBLING_KEYS`]
/// it folds in automatically) names exactly the field set
/// `crate::wire_schema::schema()` declares for that node's own generated
/// def -- the same source `docs/bindings-wire-format.md`'s *Generated
/// reference* section renders from. A field added to one side without
/// the other would otherwise only surface as a silent false "unknown
/// key" warning (or a surprise strict-mode rejection) the first time a
/// real app sent it, rather than failing this test at compile-review
/// time.
#[test]
fn node_shape_key_constants_match_the_wire_schema_field_list() {
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
    for (def_name, shape_keys) in cases {
        let mut expected = schema_field_names(def_name);
        for universal in UNIVERSAL_NODE_SIBLING_KEYS {
            // `schema_field_names` is the generated reference's own
            // field list, which already carries these; strip them back
            // out so the comparison is against `shape_keys` alone, the
            // same split `check_node_sibling_keys` itself makes.
            expected.remove(universal);
        }
        let actual: HashSet<String> = shape_keys.iter().map(|s| s.to_string()).collect();
        assert_eq!(
            actual, expected,
            "{def_name}'s shape-key constant has drifted from its wire_schema field list"
        );
    }
}
