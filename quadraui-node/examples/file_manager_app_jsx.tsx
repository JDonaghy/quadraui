// A JSX port of `./file_manager_app.ts`, behaviour-identical to it: same
// model, same navigation, same keys, same ids -- only `_view`,
// `_detailsPane`, and `_statusLeaf` differ, each rewritten from a nested
// `Split(..., Tree(...), {...})` call chain into `<split>...<tree
// .../>...</split>` markup against `../js/jsx-runtime.ts`'s own intrinsic
// elements (see that file's own header doc for the `jsx`/`jsxs` contract
// behind this, and the package README's "JSX" section for the
// `tsconfig.json` this file type-checks under). Every other method below
// is copied verbatim from `./file_manager_app.ts` -- see that file's own
// doc for what each one does.
//
// `Tab`/`Toolbar` are called as plain functions here, not JSX elements --
// `../js/jsx-runtime.ts`'s own header doc explains why (each returns a
// data record, not a `Node`).
//
// Run the same way as `./file_manager_app.ts`:
//
//     node --experimental-strip-types examples/file_manager_app_jsx.tsx
//
// -- except this file can't actually run that way: JSX syntax isn't valid
// JS, so Node's own type-stripping can never execute a `.tsx` file
// directly, regardless of the runtime it imports. Transform it first with
// `tsc`/esbuild/Bun (`npm run typecheck:jsx` only type-checks; it does not
// emit anything runnable).

const fs = require("node:fs");
const path = require("node:path");

const { App, Tab, Toolbar, keyName } = require("../js/app/index.ts");

import type { Renderable } from "../js/app/index";
import type {
  Color,
  ColumnDescriptor,
  DataRowDescriptor,
  Icon,
  StatusBarSegment,
  StyledText,
  ToolbarDescriptor,
  TreePath,
  TreeRowDescriptor,
  TreeStyle,
} from "../js/wire";

/** One filesystem entry in the lazily-scanned model. `children === null` means "not read yet" -- distinct from `[]` ("read, and empty"). */
interface FsNode {
  name: string;
  path: string;
  isDir: boolean;
  sizeBytes: number | null;
  parent: FsNode | null;
  children: FsNode[] | null;
  expanded: boolean;
  loadError: boolean;
}

function subdirs(node: FsNode): FsNode[] {
  return (node.children ?? []).filter((c) => c.isDir);
}

function safeSize(p: string): number | null {
  try {
    return fs.statSync(p).size;
  } catch {
    return null;
  }
}

function sortKey(node: FsNode): [number, string] {
  return [node.isDir ? 0 : 1, node.name.toLowerCase()];
}

function compareNodes(a: FsNode, b: FsNode): number {
  const [aKind, aName] = sortKey(a);
  const [bKind, bName] = sortKey(b);
  if (aKind !== bKind) return aKind - bKind;
  return aName < bName ? -1 : aName > bName ? 1 : 0;
}

function loadChildren(node: FsNode): void {
  if (!node.isDir) return;
  let entries: string[];
  try {
    entries = fs.readdirSync(node.path).filter((name: string) => !name.startsWith("."));
  } catch {
    node.children = [];
    node.loadError = true;
    return;
  }
  const kids: FsNode[] = entries.map((name: string) => {
    const childPath = path.join(node.path, name);
    const isDir = fs.statSync(childPath).isDirectory();
    return {
      name,
      path: childPath,
      isDir,
      sizeBytes: isDir ? null : safeSize(childPath),
      parent: node,
      children: null,
      expanded: false,
      loadError: false,
    };
  });
  kids.sort(compareNodes);
  node.children = kids;
  node.loadError = false;
}

function ensureLoaded(node: FsNode): void {
  if (node.isDir && node.children === null) {
    loadChildren(node);
  }
}

function buildRoot(rootPath: string): FsNode {
  const root: FsNode = {
    name: path.basename(rootPath) || rootPath,
    path: rootPath,
    isDir: fs.statSync(rootPath).isDirectory(),
    sizeBytes: null,
    parent: null,
    children: null,
    expanded: false,
    loadError: false,
  };
  ensureLoaded(root);
  root.expanded = true;
  return root;
}

// Resent unchanged every frame, so the body split's ratio resolves as
// uncontrolled from the second frame on -- see `Split`'s own doc (`../js/app/nodes.ts`).
const BODY_SPLIT_RATIO = 0.3;

const TREE_STYLE: TreeStyle = {
  indent: 2,
  show_chevrons: true,
  chevron_expanded: "▾",
  chevron_collapsed: "▸",
};
const FOLDER_ICON: Icon = { glyph: "\u{1F4C1}", fallback: "/" };

function styledText(text: string): StyledText {
  return { spans: [{ text, fg: null, bg: null }] };
}

/** Walk the expanded model into index-aligned `[rows, nodes]`, directories only -- recomputed every frame. */
function folderRows(root: FsNode): [TreeRowDescriptor[], FsNode[]] {
  const rows: TreeRowDescriptor[] = [];
  const nodes: FsNode[] = [];

  function walk(node: FsNode, nodePath: TreePath, indent: number): void {
    rows.push({
      path: nodePath,
      indent,
      icon: FOLDER_ICON,
      text: styledText(node.name),
      badge: null,
      is_expanded: node.expanded,
    });
    nodes.push(node);
    if (node.expanded) {
      subdirs(node).forEach((child, i) => walk(child, [...nodePath, i], indent + 1));
    }
  }

  walk(root, [], 0);
  return [rows, nodes];
}

const SIZE_UNITS = ["B", "KB", "MB", "GB", "TB"];

function humanSize(n: number | null): string {
  if (n === null) return "";
  let size = n;
  let i = 0;
  while (size >= 1024 && i < SIZE_UNITS.length - 1) {
    size /= 1024;
    i += 1;
  }
  return i === 0 ? `${Math.trunc(size)} ${SIZE_UNITS[i]}` : `${size.toFixed(1)} ${SIZE_UNITS[i]}`;
}

/** The `data_table` rows for one opened folder's contents, subfolders first (sorted by `compareNodes` already), a trailing "/" marking a navigable row. */
function detailsRows(folder: FsNode): DataRowDescriptor[] {
  const children = folder.children ?? [];
  if (children.length === 0) {
    const note = folder.loadError ? "(unreadable)" : "(empty folder)";
    return [{ cells: [styledText(note), styledText(""), styledText("")] }];
  }
  return children.map((child) => {
    const name = child.isDir ? `${child.name}/` : child.name;
    const kind = child.isDir ? "folder" : "file";
    const size = child.isDir ? "" : humanSize(child.sizeBytes);
    return { cells: [styledText(name), styledText(size), styledText(kind)] };
  });
}

const DETAILS_COLUMNS: ColumnDescriptor[] = [
  { title: "Name", width: { Flex: 2.0 }, align: "Left" },
  { title: "Size", width: { Flex: 1.0 }, align: "Right" },
  { title: "Kind", width: { Flex: 1.0 }, align: "Left" },
];

const TOOLBAR: ToolbarDescriptor = Toolbar(
  [
    { Action: { id: "fm:k:move", label: "↑↓ Move" } },
    { Action: { id: "fm:k:expand", label: "→ Expand" } },
    { Action: { id: "fm:k:collapse", label: "← Collapse" } },
    { Action: { id: "fm:k:open", label: "↵ Open" } },
    { Action: { id: "fm:k:refresh", label: "r Refresh" } },
  ],
  { id: "fm:toolbar" },
);

const STATUS_FG: Color = { r: 220, g: 220, b: 230, a: 255 };
const STATUS_BG: Color = { r: 30, g: 32, b: 48, a: 255 };
const STATUS_ACCENT: Color = { r: 245, g: 224, b: 138, a: 255 };

function segment(text: string, opts: { accent?: boolean; bold?: boolean } = {}): StatusBarSegment {
  return { text, fg: opts.accent ? STATUS_ACCENT : STATUS_FG, bg: STATUS_BG, bold: opts.bold ?? false };
}

/**
 * Holds the lazily-scanned model and the live UI state `_view`/the node
 * callbacks below share -- identical to `./file_manager_app.ts`'s own
 * `FileManager`, except `_view`/`_detailsPane`/`_statusLeaf` are JSX.
 */
class FileManager {
  root: FsNode;
  height: number;
  cursor: FsNode;
  open: FsNode[];
  message: string;
  activeTabIndex: number;
  app: InstanceType<typeof App>;
  actions: unknown;

  constructor(rootPath: string, width = 100, height = 30) {
    this.root = buildRoot(rootPath);
    this.height = height;
    this.cursor = this.root;
    this.open = [];
    this.message = "";
    this.activeTabIndex = 0;
    this.app = new App(width, height, {
      view: () => this._view(),
      onEvent: (event: unknown) => this.onEvent(event),
    });
    this.actions = this.app.actions;
  }

  // --- navigation helpers --------------------------------------------------

  private _visible(): [TreeRowDescriptor[], FsNode[]] {
    const [rows, nodes] = folderRows(this.root);
    if (!nodes.includes(this.cursor)) {
      let node: FsNode | null = this.cursor;
      while (node !== null && !nodes.includes(node)) {
        node = node.parent;
      }
      this.cursor = node ?? this.root;
    }
    return [rows, nodes];
  }

  private _move(delta: number): void {
    const [, nodes] = this._visible();
    const i = nodes.indexOf(this.cursor);
    this.cursor = nodes[Math.max(0, Math.min(nodes.length - 1, i + delta))];
  }

  private _expand(): void {
    const node = this.cursor;
    if (!node.isDir) return;
    ensureLoaded(node);
    const subs = subdirs(node);
    if (!node.expanded && subs.length > 0) {
      node.expanded = true;
    } else if (node.expanded && subs.length > 0) {
      this.cursor = subs[0];
    }
  }

  private _collapse(): void {
    const node = this.cursor;
    if (node.isDir && node.expanded) {
      node.expanded = false;
    } else if (node.parent !== null) {
      this.cursor = node.parent;
    }
  }

  private _reveal(node: FsNode): void {
    let parent = node.parent;
    while (parent !== null) {
      ensureLoaded(parent);
      parent.expanded = true;
      parent = parent.parent;
    }
  }

  /** Opens `node` (default: the cursor) as a details tab, expanding it; focuses its tab if already open. A file just reports itself. */
  private _open(node?: FsNode): void {
    const target = node ?? this.cursor;
    if (!target.isDir) {
      this.message = `${target.name} (${humanSize(target.sizeBytes)})`;
      return;
    }
    ensureLoaded(target);
    const existing = this.open.findIndex((n) => n.path === target.path);
    if (existing !== -1) {
      this.activeTabIndex = existing;
      return;
    }
    this.open.push(target);
    this.activeTabIndex = this.open.length - 1;
  }

  private _refresh(): void {
    const node = this.cursor;
    if (node.isDir) {
      node.children = null;
      ensureLoaded(node);
      this.message = `reloaded ${node.name}`;
    }
  }

  private _closeTab(index: number): void {
    if (index >= 0 && index < this.open.length) {
      this.open.splice(index, 1);
      if (this.activeTabIndex >= this.open.length) {
        this.activeTabIndex = Math.max(0, this.open.length - 1);
      }
    }
  }

  private _onRowClick(nodePath: TreePath): void {
    const [rows, nodes] = folderRows(this.root);
    for (let i = 0; i < rows.length; i++) {
      if (pathsEqual(rows[i].path, nodePath)) {
        const node = nodes[i];
        this.cursor = node;
        ensureLoaded(node);
        if (subdirs(node).length > 0) node.expanded = !node.expanded;
        return;
      }
    }
  }

  private _onRowToggleExpand(nodePath: TreePath): void {
    const [rows, nodes] = folderRows(this.root);
    for (let i = 0; i < rows.length; i++) {
      if (pathsEqual(rows[i].path, nodePath)) {
        const node = nodes[i];
        ensureLoaded(node);
        if (subdirs(node).length > 0) node.expanded = !node.expanded;
        return;
      }
    }
  }

  /** A details-table row was activated (double-click or Enter): drill into a subfolder, or report a file's size. */
  private _activateDetail(folder: FsNode, idx: number): void {
    const children = folder.children ?? [];
    if (idx < 0 || idx >= children.length) return;
    const child = children[idx];
    if (child.isDir) {
      folder.expanded = true;
      this._reveal(child);
      this.cursor = child;
      this._open(child);
    } else {
      this.message = `${child.name} (${humanSize(child.sizeBytes)})`;
    }
  }

  // --- view + global keys ---------------------------------------------------

  /**
   * `<tabs>`'s own `activeIndex` is pinned to `activeTabIndex` on every
   * render, and mirrored back from a real tab click via `onActivate` --
   * see `Tabs`' own doc (`../js/app/nodes.ts`).
   */
  private _detailsPane(): Renderable {
    if (this.open.length === 0) {
      return (
        <textDisplay
          id="fm:empty"
          lines={[{ spans: [{ text: "Select a folder and press Enter to open it.", fg: null, bg: null }] }]}
        />
      );
    }
    const entries = this.open.map((folder, i) =>
      Tab(
        <table
          id={`fm:grid:${i}`}
          columns={DETAILS_COLUMNS}
          rows={detailsRows(folder)}
          onActivate={(idx: number) => this._activateDetail(folder, idx)}
        />,
        { title: folder.name, closable: true },
      ),
    );
    return (
      <tabs
        id="fm:tabs"
        closable
        activeIndex={this.activeTabIndex}
        onClose={(idx: number) => this._closeTab(idx)}
        onActivate={(idx: number) => {
          this.activeTabIndex = idx;
        }}
      >
        {entries}
      </tabs>
    );
  }

  private _statusLeaf(): Renderable {
    const node = this.cursor;
    const countText = node.children === null ? "unopened" : `${node.children.length} items`;
    const left = [segment(` ${node.name} `, { accent: true, bold: true }), segment(` ${countText} `)];
    if (this.message) left.push(segment(` ${this.message} `));
    const openN = this.open.length;
    const right = openN > 0 ? [segment(` ${openN} tab${openN !== 1 ? "s" : ""} `)] : [segment(" no tabs ")];
    return <statusBar id="fm:status" leftSegments={left} rightSegments={right} />;
  }

  private _view(): Renderable {
    const [rows, nodes] = this._visible();
    const selectedPath = rows[nodes.indexOf(this.cursor)].path;
    const h = Math.max(this.height, 2);
    return (
      <split id="fm:outer" orientation="vertical" ratio={(h - 1) / h} firstMin={3.0} secondMin={1.0}>
        <sidebarPanel id="fm:chrome" toolbar={TOOLBAR}>
          <split id="fm:split" ratio={BODY_SPLIT_RATIO}>
            <tree
              id="fm:tree"
              rows={rows}
              selectedPath={selectedPath}
              hasFocus
              style={TREE_STYLE}
              onRowClick={(p: TreePath) => this._onRowClick(p)}
              onToggleExpand={(p: TreePath) => this._onRowToggleExpand(p)}
            />
            {this._detailsPane()}
          </split>
        </sidebarPanel>
        {this._statusLeaf()}
      </split>
    );
  }

  /** Only truly global input lands here: every click/row-activation the view's own node callbacks above already claim never reaches this at all. */
  onEvent(event: unknown): void {
    const name = keyName(event);
    if (name === null) return;
    this.message = "";
    if (name === "Down") this._move(1);
    else if (name === "Up") this._move(-1);
    else if (name === "Right") this._expand();
    else if (name === "Left") this._collapse();
    else if (name === "Enter" || name === "o") this._open();
    else if (name === "r") this._refresh();
  }
}

function pathsEqual(a: TreePath, b: TreePath): boolean {
  return a.length === b.length && a.every((v, i) => v === b[i]);
}

/** Wraps `fm`'s own `onEvent` so `q`/Escape ends a live session -- identical to `./file_manager_app.ts`'s own `installQuitKey`. */
function installQuitKey(fm: FileManager): void {
  const inner = fm.onEvent.bind(fm);
  fm.app.setOnEvent((event: unknown) => {
    const name = keyName(event);
    if (name === "q" || name === "Escape") {
      fm.app.stopLive();
      return;
    }
    inner(event);
  });
}

const UP_KEY = { Named: "Up" };
const DOWN_KEY = { Named: "Down" };
const LEFT_KEY = { Named: "Left" };
const RIGHT_KEY = { Named: "Right" };
const ENTER_KEY = { Named: "Enter" };

function keyEvent(key: unknown) {
  return {
    KeyPressed: { key, modifiers: { shift: false, ctrl: false, alt: false, cmd: false }, repeat: false },
  };
}

/** Expand a folder, open it as a tab, drill into a subfolder from its details table, then print the final screen -- same steps as `./file_manager_app.ts`'s own `runHeadless`, so the two examples' own output is directly diffable. */
function runHeadless(fm: FileManager): void {
  const steps = [
    keyEvent(DOWN_KEY),
    keyEvent(RIGHT_KEY),
    keyEvent(ENTER_KEY),
    { DataTable: ["fm:grid:0", { RowDoubleClicked: { idx: 0 } }] },
  ];
  for (const event of steps) {
    fm.app.pushEvent(event);
    fm.app.step();
  }
  console.log(fm.app.screen());
  const openNames = fm.open.map((n) => n.name);
  const treeState = fm.app.widgetState("fm:tree");
  const splitState = fm.app.widgetState("fm:split");
  const tabsState = fm.app.widgetState("fm:tabs");
  console.log(
    `\ncursor=${fm.cursor.name}  open_tabs=${JSON.stringify(openNames)}  ` +
      `active_tab=${tabsState?.activeTab}  ` +
      `tree_scroll_offset=${treeState?.scrollOffset}  ` +
      `split_ratio=${splitState?.splitRatio}`,
  );
}

exports.FileManager = FileManager;
exports.folderRows = folderRows;
exports.buildRoot = buildRoot;
exports.ensureLoaded = ensureLoaded;
exports.BODY_SPLIT_RATIO = BODY_SPLIT_RATIO;
exports.UP_KEY = UP_KEY;
exports.DOWN_KEY = DOWN_KEY;
exports.LEFT_KEY = LEFT_KEY;
exports.RIGHT_KEY = RIGHT_KEY;
exports.ENTER_KEY = ENTER_KEY;
exports.keyEvent = keyEvent;
exports.installQuitKey = installQuitKey;

if (require.main === module) {
  const argv = process.argv.slice(2);
  const args = argv.filter((a) => !a.startsWith("--"));
  const flags = new Set(argv.filter((a) => a.startsWith("--")));
  const root = args[0] ? path.resolve(args[0]) : process.cwd();

  const fm = new FileManager(root);

  if (flags.has("--tui")) {
    installQuitKey(fm);
    fm.app.runTui();
  } else if (flags.has("--native")) {
    installQuitKey(fm);
    fm.app.runNative("quadraui file manager");
  } else {
    runHeadless(fm);
  }
}
