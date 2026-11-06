//! A `"scroll_px"` key inside a `list`/`tree`/`data_table` leaf's own
//! descriptor dict — the shape a Python `dict` or JS object round-trips
//! into via `serde_json::Value` before `quadraui-py`/`quadraui-node`
//! decode it — moves the painted first visible row on TUI, through
//! exactly the shared `decode_render_output` -> `draw_bands`/
//! `paint_layout` path both bindings call unmodified. A dict that never
//! sets the key paints exactly as before.
//!
//! Covers both wire shapes `decode_render_output` recognizes: the flat
//! per-slot dict (`RenderFrame`/`draw_bands`) and the layout-tree dict
//! (`LayoutNode`/`paint_layout`).

use quadraui::tui::testing::TuiDriver;
use quadraui::{AppLogic, Backend, InteractionState, Reaction, Rect, UiEvent};
use quadraui_binding_core::{decode_render_output, draw_bands, paint_layout, RenderOutput};
use serde_json::{json, Value};

struct DictApp {
    value: Value,
}

impl AppLogic for DictApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let mut hits = Vec::new();
        let chrome = |_id| InteractionState::default();
        match decode_render_output("test", &self.value).expect("valid render payload") {
            RenderOutput::Flat(frame) => {
                draw_bands(backend, frame.as_refs(), &mut hits, &chrome);
            }
            RenderOutput::Tree(node) => {
                let viewport = backend.viewport();
                let area = Rect::new(0.0, 0.0, viewport.width, viewport.height);
                paint_layout(backend, area, &node, &mut hits, &chrome);
            }
        }
    }

    fn handle(&mut self, _event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        Reaction::Continue
    }
}

fn list_items(n: usize) -> Value {
    (0..n)
        .map(|i| {
            json!({
                "text": {
                    "spans": [
                        {"text": format!("Row {i:02}"), "fg": null, "bg": null},
                    ],
                },
            })
        })
        .collect::<Vec<_>>()
        .into()
}

#[test]
fn flat_frame_without_scroll_px_paints_the_whole_row_offset_unchanged() {
    let value = json!({
        "list": {
            "id": "l",
            "items": list_items(20),
            "selected_idx": 0,
            "scroll_offset": 3,
        }
    });
    let mut driver = TuiDriver::new(DictApp { value }, 20, 6);
    driver.render();
    assert!(
        driver.screen_contains("Row 03"),
        "no scroll_px key must paint exactly the whole-row scroll_offset:\n{}",
        driver.screen()
    );
}

#[test]
fn flat_frame_with_scroll_px_quantises_to_the_floored_row() {
    let value = json!({
        "list": {
            "id": "l",
            "items": list_items(20),
            "selected_idx": 0,
            "scroll_offset": 0,
            "scroll_px": 3.9,
        }
    });
    let mut driver = TuiDriver::new(DictApp { value }, 20, 6);
    driver.render();
    assert!(
        driver.screen_contains("Row 03") && !driver.screen_contains("Row 00"),
        "a scroll_px of 3.9 should floor to row 3 on TUI:\n{}",
        driver.screen()
    );
}

#[test]
fn layout_tree_leaf_with_scroll_px_quantises_to_the_floored_row() {
    let value = json!({
        "row": [
            {
                "id": "leaf-list",
                "list": {
                    "id": "l",
                    "items": list_items(20),
                    "selected_idx": 0,
                    "scroll_offset": 0,
                    "scroll_px": 2.2,
                },
            },
        ],
    });
    let mut driver = TuiDriver::new(DictApp { value }, 20, 6);
    driver.render();
    assert!(
        driver.screen_contains("Row 02") && !driver.screen_contains("Row 00"),
        "a layout-tree leaf's scroll_px must reach the paint the same way the flat shape does:\n{}",
        driver.screen()
    );
}

#[test]
fn layout_tree_leaf_without_scroll_px_is_unaffected() {
    let value = json!({
        "row": [
            {
                "id": "leaf-list",
                "list": {
                    "id": "l",
                    "items": list_items(20),
                    "selected_idx": 0,
                    "scroll_offset": 0,
                },
            },
        ],
    });
    let mut driver = TuiDriver::new(DictApp { value }, 20, 6);
    driver.render();
    assert!(
        driver.screen_contains("Row 00"),
        "a leaf dict with no scroll_px key must paint the plain scroll_offset:\n{}",
        driver.screen()
    );
}
