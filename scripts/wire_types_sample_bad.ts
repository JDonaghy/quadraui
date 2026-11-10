// Negative counterpart of `scripts/wire_types_sample.ts`: the same two
// shapes, each with one key misspelled. `scripts/check_wire_types.sh`
// asserts `tsc --strict` reports an error against this file.

import type { LayoutNode, TreeViewDescriptor } from "../quadraui-node/js/wire";

export function badMisspelledLayoutKey(): LayoutNode {
  // `colum` is not a `LayoutNode` union member's key.
  return { colum: [{ id: "x", button: { id: "x", label: "x" } }] };
}

export function badMisspelledDescriptorField(): TreeViewDescriptor {
  return {
    id: "demo:tree",
    rows: [],
    selection_mode: "Single",
    selected_pathh: [0], // typo: not a `TreeViewDescriptor` key
  };
}
