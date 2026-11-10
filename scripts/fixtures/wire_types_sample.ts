// Type-checking fixture for `scripts/gen_wire_types.py`'s generated
// `wire.d.ts` -- never executed, only type-checked:
//
//     npx tsc --strict --noEmit --module commonjs --target es2020 \
//         scripts/fixtures/wire_types_sample.ts
//
// Builds a tree + split + data_table layout tree, and matches on a
// tree-row click the same way `quadraui-node/tests/descriptors.test.ts`'s
// own JS callers do -- the two acceptance scenarios from the "generated
// .pyi / TypeScript types" issue. `scripts/fixtures/wire_types_sample_bad.ts` is
// the negative counterpart `scripts/check_wire_types.sh` asserts *fails*.

import type {
  ColumnDescriptor,
  DataRowDescriptor,
  DataTableDescriptor,
  LayoutNode,
  StyledText,
  TreeRowDescriptor,
  TreeViewDescriptor,
  UiEvent,
} from "../../quadraui-node/js/wire";

function styledText(text: string): StyledText {
  return { spans: [{ text, fg: null, bg: null }] };
}

function buildTree(): LayoutNode {
  const row: TreeRowDescriptor = {
    path: [0],
    indent: 0,
    icon: null,
    text: styledText("src/"),
    badge: null,
    is_expanded: true,
  };
  const tree: TreeViewDescriptor = {
    id: "demo:tree",
    rows: [row],
    selection_mode: "Single",
    selected_path: [0],
    style: {
      indent: 2,
      show_chevrons: true,
      chevron_expanded: "v",
      chevron_collapsed: ">",
    },
  };
  return { id: "demo:tree", tree };
}

function buildTable(): LayoutNode {
  const columns: ColumnDescriptor[] = [
    { title: "Name", width: { Flex: 2.0 }, align: "Left" },
    { title: "Size", width: { FixedCells: 10.0 } },
  ];
  const rows: DataRowDescriptor[] = [{ cells: [styledText("main.rs"), styledText("4 KB")] }];
  const table: DataTableDescriptor = { id: "demo:table", columns, rows };
  return { id: "demo:table", data_table: table };
}

function buildLayout(): LayoutNode {
  return {
    split: {
      id: "demo:split",
      orientation: "horizontal",
      ratio: 0.3,
      first: buildTree(),
      second: buildTable(),
    },
  };
}

function handle(event: UiEvent): string | undefined {
  if (typeof event === "string") {
    return undefined;
  }
  if ("Tree" in event) {
    const [, activation] = event.Tree;
    if ("RowClicked" in activation) {
      return `clicked ${activation.RowClicked.path}`;
    }
    if ("RowToggleExpand" in activation) {
      return `toggled ${activation.RowToggleExpand.path}`;
    }
  } else if ("DataTable" in event) {
    const [, tableActivation] = event.DataTable;
    if ("RowDoubleClicked" in tableActivation) {
      return `opened row ${tableActivation.RowDoubleClicked.idx}`;
    }
  } else if ("SplitDividerDragged" in event) {
    const drag = event.SplitDividerDragged;
    return `${drag.tree} -> ${drag.new_ratio}`;
  }
  return undefined;
}

export { buildLayout, handle };
