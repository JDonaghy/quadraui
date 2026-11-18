// Negative counterpart of `scripts/fixtures/wire_types_sample.ts`: the same
// shapes, each with one key misspelled. `scripts/check_wire_types.sh`
// asserts `tsc --strict` reports an error against this file.

import type { LayoutNode, TreeViewDescriptor } from "../../quadraui-node/js/wire";

export function badMisspelledLayoutKey(): LayoutNode {
  // `colum` is not a `LayoutNode` union member's key.
  return { colum: [{ id: "x", button: { id: "x", label: "x" } }] };
}

export function badMisspelledGridTrackVariant(): LayoutNode {
  // `Weigth` is not a `GridTrack` union member's key.
  return {
    grid: {
      columns: [{ Weigth: 1.0 }],
      rows: [{ Weight: 1.0 }],
      cells: [{ id: "x", button: { id: "x", label: "x" } }],
    },
  };
}

export function badMisspelledDescriptorField(): TreeViewDescriptor {
  return {
    id: "demo:tree",
    rows: [],
    selection_mode: "Single",
    selected_pathh: [0], // typo: not a `TreeViewDescriptor` key
    style: {
      indent: 2,
      show_chevrons: true,
      chevron_expanded: "v",
      chevron_collapsed: ">",
    },
  };
}
