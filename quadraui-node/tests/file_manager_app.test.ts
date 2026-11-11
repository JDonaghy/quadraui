// Black-box acceptance test for `examples/file_manager_app.ts`: an
// interactive, cross-backend, resizable two-pane file manager -- a
// lazily-loaded folders-only tree on the left, and a details area on the
// right that opens one closable tab per folder (each tab a data_table of
// that folder's contents), all built from one layout tree `App._render()`
// returns. Drives the real `event -> handle -> render` path on the
// headless backend -- push an event, `step()`, then assert against the
// rebuilt descriptors and `screen()`/`screenContains()`, never hardcoded
// coordinates. Mirrors `quadraui-py/tests/test_file_manager_app.py` 1:1.
//
// Built against a deterministic temp directory (not the repo), so names
// and ordering are fixed:
//
//     root/
//       alpha/   sub/ (deep.txt)   a1.txt  a2.txt
//       beta/    b1.txt
//       readme.md
//
// The tree pane is FOLDERS ONLY and loads one level at a time. Initially
// the root is expanded and its subfolders are collapsed + unloaded, so
// (depth-first, dirs only) the visible folder rows are:
//
//     0 root   1 alpha   2 beta
//
// alpha's own children (including the `sub/` subfolder) are not read
// until alpha is expanded or opened. Files (readme.md, a1.txt, ...) never
// appear in the tree; they show in a folder's details table once it is
// opened.

const { test, after } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

const fm = require("../examples/file_manager_app.ts");

const NO_MODS = { shift: false, ctrl: false, alt: false, cmd: false };

const createdTmpDirs: string[] = [];
after(() => {
  for (const dir of createdTmpDirs) {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

/** Builds the fixed directory tree this file's own doc describes and returns its root path. */
function treeRoot(): string {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "quadraui-node-file-manager-"));
  createdTmpDirs.push(root);
  const alpha = path.join(root, "alpha");
  fs.mkdirSync(alpha);
  fs.writeFileSync(path.join(alpha, "a1.txt"), "one");
  fs.writeFileSync(path.join(alpha, "a2.txt"), "two");
  const sub = path.join(alpha, "sub");
  fs.mkdirSync(sub);
  fs.writeFileSync(path.join(sub, "deep.txt"), "deep");
  const beta = path.join(root, "beta");
  fs.mkdirSync(beta);
  fs.writeFileSync(path.join(beta, "b1.txt"), "bee");
  fs.writeFileSync(path.join(root, "readme.md"), "# readme");
  return root;
}

function manager(root: string) {
  return new fm.FileManager(root, 80, 30);
}

function press(instance: any, key: unknown) {
  instance.app.pushEvent(fm.keyEvent(key));
  instance.app.step();
}

function send(instance: any, event: unknown) {
  instance.app.pushEvent(event);
  instance.app.step();
}

/** The resizable tree/details split. The frame is an outer vertical split (chrome+status over a status bar); the body lives in the first pane, inside the `sidebar_panel` that carries the toolbar. */
function bodySplit(instance: any): any {
  const outer = instance.app._render().split;
  return outer.first.sidebar_panel.content.split;
}

function statusBar(instance: any): any {
  return instance.app._render().split.second.status_bar;
}

function activeTabIndex(instance: any): number {
  const state = instance.app.widgetState("fm:tabs");
  return state === null ? 0 : state.activeTab ?? 0;
}

function activeTable(instance: any): any {
  const second = bodySplit(instance).second;
  if (!("tabs" in second)) return null;
  const tabsNode = second.tabs;
  const content = tabsNode.tabs[activeTabIndex(instance)].content;
  return content.data_table;
}

function names(table: any): string[] {
  return table.rows.map((r: any) => r.cells[0].spans[0].text);
}

function folderNames(instance: any): string[] {
  const [rows] = fm.folderRows(instance.root);
  return rows.map((r: any) => r.text.spans[0].text);
}

function segmentText(segments: any[]): string {
  return segments.map((s: any) => s.text).join("");
}

// --- the tree: folders-only, lazily loaded -----------------------------------

test("tree starts folders-only and lazily loaded", () => {
  const root = treeRoot();
  const instance = manager(root);

  assert.deepEqual(folderNames(instance), [path.basename(root), "alpha", "beta"]);
  // alpha's contents (and its `sub/` subfolder) are not read yet.
  const alpha = instance.root.children[0];
  assert.equal(alpha.name, "alpha");
  assert.equal(alpha.children, null);
  // The top-level object is the outer vertical split; the body beneath is
  // the horizontal tree/details divider.
  assert.deepEqual(Object.keys(instance.app._render()), ["split"]);
  assert.equal(instance.app._render().split.orientation, "vertical");
  assert.equal(bodySplit(instance).orientation, "horizontal");
});

test("Right expands and loads on demand", () => {
  const root = treeRoot();
  const instance = manager(root);

  press(instance, fm.DOWN_KEY); // cursor 0 -> alpha
  assert.equal(instance.cursor.name, "alpha");
  press(instance, fm.RIGHT_KEY); // expand alpha (loads it)

  assert.notEqual(instance.cursor.children, null);
  assert.deepEqual(folderNames(instance), [path.basename(root), "alpha", "sub", "beta"]);
  assert.ok(instance.app.screenContains("sub"));
});

test("Left collapses then climbs to parent", () => {
  const root = treeRoot();
  const instance = manager(root);

  press(instance, fm.DOWN_KEY); // -> alpha
  press(instance, fm.RIGHT_KEY); // expand alpha
  press(instance, fm.DOWN_KEY); // -> sub (collapsed leaf dir)
  press(instance, fm.LEFT_KEY); // sub collapsed -> climb to parent alpha
  assert.equal(instance.cursor.name, "alpha");
  assert.ok(instance.cursor.expanded); // alpha still expanded

  press(instance, fm.LEFT_KEY); // alpha expanded -> collapse it
  assert.equal(instance.cursor.name, "alpha");
  assert.ok(!instance.cursor.expanded);
  assert.deepEqual(folderNames(instance), [path.basename(root), "alpha", "beta"]); // sub hidden

  press(instance, fm.LEFT_KEY); // alpha collapsed -> climb to root
  assert.equal(instance.cursor.name, path.basename(root));
});

test("chevron click toggles expansion without moving the cursor", () => {
  const root = treeRoot();
  const instance = manager(root);

  send(instance, { Tree: ["fm:tree", { RowToggleExpand: { path: [0] } }] }); // alpha
  assert.equal(instance.cursor.name, path.basename(root)); // cursor never moved
  assert.deepEqual(folderNames(instance), [path.basename(root), "alpha", "sub", "beta"]);

  send(instance, { Tree: ["fm:tree", { RowToggleExpand: { path: [0] } }] }); // alpha again
  assert.equal(instance.cursor.name, path.basename(root));
  assert.deepEqual(folderNames(instance), [path.basename(root), "alpha", "beta"]); // sub hidden again
});

// --- opening folders as detail tabs ------------------------------------------

test("details is a placeholder until a folder is opened", () => {
  const root = treeRoot();
  const instance = manager(root);

  assert.equal(activeTable(instance), null);
  assert.ok(instance.app.screenContains("Select a folder"));
  assert.ok(!instance.app.screenContains("Kind")); // no table header yet
});

test("Enter opens the cursored folder as a tab", () => {
  const root = treeRoot();
  const instance = manager(root);

  press(instance, fm.DOWN_KEY); // -> alpha
  press(instance, fm.ENTER_KEY);

  assert.deepEqual(instance.open.map((n: any) => n.name), ["alpha"]);
  assert.equal(activeTabIndex(instance), 0);
  assert.deepEqual(names(activeTable(instance)), ["sub/", "a1.txt", "a2.txt"]);
  assert.ok(instance.app.screenContains("alpha"));
  for (const title of ["Name", "Size", "Kind"]) {
    assert.ok(instance.app.screenContains(title));
  }
});

test("reopening an open folder focuses its tab", () => {
  const root = treeRoot();
  const instance = manager(root);

  press(instance, fm.DOWN_KEY); // -> alpha
  press(instance, fm.ENTER_KEY); // open alpha (tab 0)
  press(instance, fm.DOWN_KEY); // -> beta
  press(instance, fm.ENTER_KEY); // open beta (tab 1, active)
  assert.equal(activeTabIndex(instance), 1);

  press(instance, fm.UP_KEY); // -> alpha
  press(instance, fm.ENTER_KEY);
  assert.deepEqual(instance.open.map((n: any) => n.name), ["alpha", "beta"]);
  assert.equal(activeTabIndex(instance), 0);
});

// --- drilling in from the details table --------------------------------------

test("activating a subfolder row drills into it", () => {
  const root = treeRoot();
  const instance = manager(root);

  press(instance, fm.DOWN_KEY); // -> alpha
  press(instance, fm.ENTER_KEY); // open alpha (row 0 is sub/)
  assert.equal(names(activeTable(instance))[0], "sub/");

  send(instance, { DataTable: ["fm:grid:0", { RowSelected: { idx: 0 } }] });
  assert.deepEqual(instance.open.map((n: any) => n.name), ["alpha"], "a single click must not drill in");

  send(instance, { DataTable: ["fm:grid:0", { RowDoubleClicked: { idx: 0 } }] });

  assert.deepEqual(instance.open.map((n: any) => n.name), ["alpha", "sub"]);
  assert.equal(activeTabIndex(instance), 1);
  assert.equal(instance.cursor.name, "sub");
  assert.deepEqual(names(activeTable(instance)), ["deep.txt"]);
  assert.ok(instance.app.screenContains("deep.txt"));
});

test("activating a file row reports it without opening a tab", () => {
  const root = treeRoot();
  const instance = manager(root);

  press(instance, fm.DOWN_KEY); // -> alpha
  press(instance, fm.ENTER_KEY); // open alpha
  send(instance, { DataTable: ["fm:grid:0", { RowDoubleClicked: { idx: 1 } }] }); // a1.txt

  assert.deepEqual(instance.open.map((n: any) => n.name), ["alpha"]); // no new tab
  assert.ok(segmentText(statusBar(instance).left_segments).includes("a1.txt"));
});

// --- tabs: click to switch, close to remove ----------------------------------

test("a TabActivated event switches the shown tab", () => {
  const root = treeRoot();
  const instance = manager(root);

  press(instance, fm.DOWN_KEY);
  press(instance, fm.ENTER_KEY); // open alpha (tab 0)
  press(instance, fm.DOWN_KEY);
  press(instance, fm.ENTER_KEY); // open beta (tab 1, active)
  assert.equal(activeTabIndex(instance), 1);

  send(instance, { TabBar: ["fm:tabs", { TabActivated: { index: 0 } }] });

  assert.equal(activeTabIndex(instance), 0);
  assert.equal(names(activeTable(instance))[0], "sub/"); // alpha's contents
});

test("a TabClosed event removes the tab", () => {
  const root = treeRoot();
  const instance = manager(root);

  press(instance, fm.DOWN_KEY);
  press(instance, fm.ENTER_KEY); // open alpha (tab 0)
  press(instance, fm.DOWN_KEY);
  press(instance, fm.ENTER_KEY); // open beta (tab 1, active)

  send(instance, { TabBar: ["fm:tabs", { TabClosed: { index: 0 } }] }); // close alpha

  assert.deepEqual(instance.open.map((n: any) => n.name), ["beta"]);
  assert.equal(activeTabIndex(instance), 0);
  assert.deepEqual(names(activeTable(instance)), ["b1.txt"]);
});

// --- the split divider --------------------------------------------------------

test("body divider drag updates the ratio", () => {
  const root = treeRoot();
  const instance = manager(root);

  assert.ok(Math.abs(instance.app.widgetState("fm:split").splitRatio - fm.BODY_SPLIT_RATIO) < 1e-4);
  send(instance, { SplitDividerDragged: { tree: "fm:split", split_index: 0, new_ratio: 0.6 } });

  assert.ok(Math.abs(instance.app.widgetState("fm:split").splitRatio - 0.6) < 1e-4);
  assert.ok(Math.abs(bodySplit(instance).ratio - fm.BODY_SPLIT_RATIO) < 1e-6);
});

test("outer divider drag persists but never touches the body split", () => {
  const root = treeRoot();
  const instance = manager(root);

  send(instance, { SplitDividerDragged: { tree: "fm:outer", split_index: 0, new_ratio: 0.9 } });

  assert.ok(Math.abs(instance.app.widgetState("fm:outer").splitRatio - 0.9) < 1e-4);
  assert.ok(Math.abs(instance.app.widgetState("fm:split").splitRatio - fm.BODY_SPLIT_RATIO) < 1e-4);
});

// --- chrome: toolbar legend + live status bar --------------------------------

test("toolbar band is present and rendered", () => {
  const root = treeRoot();
  const instance = manager(root);

  const chrome = instance.app._render().split.first.sidebar_panel;
  assert.equal(chrome.id, "fm:chrome");
  assert.equal(chrome.toolbar.id, "fm:toolbar");
  const labels = chrome.toolbar.buttons.map((b: any) => b.Action.label);
  assert.ok(labels.some((label: string) => label.includes("Open")));
  for (const word of ["Move", "Expand", "Collapse", "Open", "Refresh"]) {
    assert.ok(instance.app.screenContains(word));
  }
});

test("status bar reflects cursor and open tabs", () => {
  const root = treeRoot();
  const instance = manager(root);

  let status = statusBar(instance);
  assert.equal(status.id, "fm:status");
  // The root is loaded (alpha, beta, readme.md == 3), no tabs yet.
  assert.ok(segmentText(status.left_segments).includes(path.basename(root)));
  assert.ok(segmentText(status.left_segments).includes("3 items"));
  assert.ok(segmentText(status.right_segments).includes("no tabs"));

  press(instance, fm.DOWN_KEY); // -> alpha (still unopened)
  assert.ok(segmentText(statusBar(instance).left_segments).includes("unopened"));

  press(instance, fm.ENTER_KEY); // open alpha: sub/, a1.txt, a2.txt == 3
  status = statusBar(instance);
  assert.ok(segmentText(status.left_segments).includes("alpha"));
  assert.ok(segmentText(status.left_segments).includes("3 items"));
  assert.ok(segmentText(status.right_segments).includes("1 tab"));
  assert.ok(instance.app.screenContains("1 tab"));
});

// --- scrolling: the view follows the cursor, the scrollbar paints ------------

function deepRoot(n: number): string {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "quadraui-node-file-manager-deep-"));
  createdTmpDirs.push(root);
  for (let i = 0; i < n; i++) {
    fs.mkdirSync(path.join(root, `folder_${String(i).padStart(2, "0")}`));
  }
  return root;
}

test("a short tree paints no scrollbar", () => {
  const instance = new fm.FileManager(treeRoot(), 80, 30); // 3 rows, 30 tall
  assert.ok(!instance.app.screenContains("░")); // track glyph
  assert.ok(!instance.app.screenContains("█")); // thumb glyph
});

test("an overflowing tree paints a scrollbar on TUI", () => {
  const instance = new fm.FileManager(deepRoot(20), 60, 12);
  instance.app.step();
  assert.ok(instance.app.screenContains("░")); // track
  assert.ok(instance.app.screenContains("█")); // thumb
});

function treeScrollOffset(instance: any): number {
  const state = instance.app.widgetState("fm:tree");
  return state === null ? 0 : state.scrollOffset ?? 0;
}

test("scroll follows the cursor down and clamps", () => {
  const root = deepRoot(20);
  const instance = new fm.FileManager(root, 60, 12);

  assert.equal(treeScrollOffset(instance), 0);
  for (let i = 0; i < 15; i++) {
    press(instance, fm.DOWN_KEY);
  }

  // The cursor advanced past the first window, so the view scrolled...
  assert.ok(treeScrollOffset(instance) > 0);
  // ...and the cursored folder is actually on screen (not off the bottom).
  assert.ok(instance.app.screenContains(instance.cursor.name));

  // Arrowing back to the top pulls the window back to the start.
  for (let i = 0; i < 20; i++) {
    press(instance, fm.UP_KEY);
  }
  assert.equal(treeScrollOffset(instance), 0);
  assert.equal(instance.cursor.name, path.basename(root));
});

test("a focus-scoped key event moves the cursor", () => {
  const root = treeRoot();
  const instance = manager(root);

  assert.equal(instance.cursor.name, path.basename(root));
  // A focused tree stringifies the key ("Down"), unlike a plain
  // KeyPressed's tagged {Named: "Down"}. Deliver that shape straight to
  // the handler (it is what the real backend hands this layer after focus
  // routing).
  instance.app._handle({ Tree: ["fm:tree", { KeyPressed: { key: "Down", modifiers: NO_MODS } }] });
  assert.equal(instance.cursor.name, "alpha");

  // And the plain top-level KeyPressed shape still works.
  press(instance, fm.DOWN_KEY);
  assert.equal(instance.cursor.name, "beta");
});
