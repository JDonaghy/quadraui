// Black-box acceptance test for `examples/file_manager_app.ts`: an
// interactive, cross-backend, resizable two-pane file manager -- a
// lazily-loaded folders-only tree on the left, and a details area on the
// right that opens one closable tab per folder (each tab a data_table of
// that folder's contents), all built from one node tree the app layer's
// `view` callback returns.
//
// Written entirely against `../js/testing.ts` -- the app-author test
// harness -- so every line below reads the way a user describes the app:
// find the tree, click the row labelled "alpha", press Enter, assert the
// frame shows "a1.txt". There is no raw wire event, no hand-built key
// object and no hardcoded coordinate anywhere in this file; the harness
// synthesises the real `UiEvent` for each action and drives the real
// `event -> handle -> render` path on the headless backend. Mirrors
// `quadraui-py/tests/test_file_manager_app.py` 1:1.
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
// until alpha is expanded or opened -- which is exactly why "sub" is
// absent from the tree below until something expands alpha. Files
// (readme.md, a1.txt, ...) never appear in the tree; they show in a
// folder's details table once it is opened.

const { test, after } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

const fm = require("../examples/file_manager_app.ts");
const { harness } = require("../js/testing.ts");

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

/** A root with `n` subfolders -- enough to overflow a short frame so the tree must scroll. */
function deepRoot(n: number): string {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "quadraui-node-file-manager-deep-"));
  createdTmpDirs.push(root);
  for (let i = 0; i < n; i++) {
    fs.mkdirSync(path.join(root, `folder_${String(i).padStart(2, "0")}`));
  }
  return root;
}

/** The file manager over a directory, wrapped in the harness. */
function ui(root: string, width = 80, height = 30): any {
  return harness(new fm.FileManager(root, width, height).app);
}

function tree(h: any): any {
  return h.find({ role: "tree" });
}

/** The folder the tree renders as selected -- the app's own cursor, read back off the frame. */
function cursor(h: any): string {
  return tree(h).selectedRow.text;
}

function folderNames(h: any): string[] {
  return tree(h).rowTexts;
}

/** The title of every open details tab, left to right. */
function openTitles(h: any): string[] {
  const tabs = h.query({ role: "tabs" });
  return tabs === null ? [] : tabs.tabs.map((tab: any) => tab.title);
}

/** The Name column of the details table behind the active tab, or `null` for the placeholder. */
function detailsNames(h: any): string[] | null {
  const table = h.query({ role: "data_table" });
  return table === null ? null : table.rows.map((row: any) => row.cells[0]);
}

function status(h: any): any {
  return h.find({ role: "status_bar" });
}

function closeTo(actual: number, expected: number): void {
  assert.ok(Math.abs(actual - expected) < 1e-4, `${actual} is not close to ${expected}`);
}

// --- the tree: folders-only, lazily loaded -----------------------------------

test("tree starts folders-only and lazily loaded", () => {
  const root = treeRoot();
  const h = ui(root);

  assert.deepEqual(folderNames(h), [path.basename(root), "alpha", "beta"]);
  h.assertNotText("sub");
  // No file ever reaches the tree, at any depth.
  h.assertNotText("readme.md", "a1.txt", "deep.txt");
  // The frame is an outer vertical split (chrome + body over a status
  // bar) whose body is the horizontal tree/details divider.
  assert.equal(h.root.role, "split");
  assert.equal(h.root.get("orientation"), "vertical");
  assert.equal(h.find({ id: "fm:split" }).get("orientation"), "horizontal");
});

test("Right expands and loads on demand", () => {
  const root = treeRoot();
  const h = ui(root);

  h.press("Down"); // cursor root -> alpha
  assert.equal(cursor(h), "alpha");
  h.press("Right"); // expand alpha (loads it)

  assert.ok(tree(h).row("alpha").expanded);
  assert.deepEqual(folderNames(h), [path.basename(root), "alpha", "sub", "beta"]);
  h.assertText("sub");
});

test("Left collapses then climbs to parent", () => {
  const root = treeRoot();
  const h = ui(root);

  h.press("Down"); // -> alpha
  h.press("Right"); // expand alpha
  h.press("Down"); // -> sub (collapsed leaf dir)
  h.press("Left"); // sub collapsed -> climb to parent alpha
  assert.equal(cursor(h), "alpha");
  assert.ok(tree(h).row("alpha").expanded);

  h.press("Left"); // alpha expanded -> collapse it
  assert.equal(cursor(h), "alpha");
  assert.ok(!tree(h).row("alpha").expanded);
  assert.deepEqual(folderNames(h), [path.basename(root), "alpha", "beta"]); // sub hidden

  h.press("Left"); // alpha collapsed -> climb to root
  assert.equal(cursor(h), path.basename(root));
});

test("chevron click toggles expansion without moving the cursor", () => {
  const root = treeRoot();
  const h = ui(root);

  tree(h).row("alpha").toggle();
  assert.equal(cursor(h), path.basename(root)); // cursor never moved
  assert.deepEqual(folderNames(h), [path.basename(root), "alpha", "sub", "beta"]);

  tree(h).row("alpha").toggle();
  assert.equal(cursor(h), path.basename(root));
  assert.deepEqual(folderNames(h), [path.basename(root), "alpha", "beta"]); // sub hidden again
});

test("a row body click moves the cursor", () => {
  const h = ui(treeRoot());

  tree(h).row("beta").click();

  assert.equal(cursor(h), "beta");
  assert.equal(folderNames(h).at(-1), "beta"); // nothing expanded
});

// --- opening folders as detail tabs ------------------------------------------

test("details is a placeholder until a folder is opened", () => {
  const h = ui(treeRoot());

  assert.equal(detailsNames(h), null);
  h.assertText("Select a folder");
  h.assertNotText("Kind"); // no table header yet
});

test("Enter opens the cursored folder as a tab", () => {
  const h = ui(treeRoot());

  h.press("Down"); // -> alpha
  h.press("Enter");

  assert.deepEqual(openTitles(h), ["alpha"]);
  assert.equal(h.find({ role: "tabs" }).activeIndex, 0);
  assert.deepEqual(detailsNames(h), ["sub/", "a1.txt", "a2.txt"]);
  h.assertText("alpha", "Name", "Size", "Kind");
});

test("reopening an open folder focuses its tab", () => {
  const h = ui(treeRoot());

  h.press("Down"); // -> alpha
  h.press("Enter"); // open alpha (tab 0)
  h.press("Down"); // -> beta
  h.press("Enter"); // open beta (tab 1, active)
  assert.equal(h.find({ role: "tabs" }).activeIndex, 1);

  h.press("Up"); // -> alpha
  h.press("Enter");
  assert.deepEqual(openTitles(h), ["alpha", "beta"]);
  assert.equal(h.find({ role: "tabs" }).activeIndex, 0);
});

// --- drilling in from the details table --------------------------------------

test("activating a subfolder row drills into it", () => {
  const h = ui(treeRoot());

  h.press("Down"); // -> alpha
  h.press("Enter"); // open alpha (row 0 is sub/)
  assert.equal(detailsNames(h)?.[0], "sub/");

  h.find({ role: "data_table" }).row("sub/").click();
  assert.deepEqual(openTitles(h), ["alpha"], "a single click must not drill in");

  h.find({ role: "data_table" }).row("sub/").doubleClick();

  assert.deepEqual(openTitles(h), ["alpha", "sub"]);
  assert.equal(h.find({ role: "tabs" }).activeIndex, 1);
  assert.equal(cursor(h), "sub");
  assert.deepEqual(detailsNames(h), ["deep.txt"]);
  h.assertText("deep.txt");
});

test("activating a file row reports it without opening a tab", () => {
  const h = ui(treeRoot());

  h.press("Down"); // -> alpha
  h.press("Enter"); // open alpha
  h.find({ role: "data_table" }).row("a1.txt").doubleClick();

  assert.deepEqual(openTitles(h), ["alpha"]); // no new tab
  assert.ok(status(h).textOf("left_segments").includes("a1.txt"));
});

// --- tabs: click to switch, close to remove ----------------------------------

test("clicking a tab switches the shown tab", () => {
  const h = ui(treeRoot());

  h.press("Down");
  h.press("Enter"); // open alpha (tab 0)
  h.press("Down");
  h.press("Enter"); // open beta (tab 1, active)
  assert.equal(h.find({ role: "tabs" }).activeIndex, 1);

  h.find({ role: "tabs" }).tabs[0].click();

  assert.equal(h.find({ role: "tabs" }).activeIndex, 0);
  assert.equal(detailsNames(h)?.[0], "sub/"); // alpha's contents
  // Both tables are in the tree; only the active tab's is visible.
  assert.equal(h.findAll({ role: "data_table" }).length, 1);
  assert.equal(h.findAll({ role: "data_table", includeHidden: true }).length, 2);
});

test("closing a tab removes it", () => {
  const h = ui(treeRoot());

  h.press("Down");
  h.press("Enter"); // open alpha (tab 0)
  h.press("Down");
  h.press("Enter"); // open beta (tab 1, active)

  h.find({ role: "tabs" }).tabs[0].close(); // close alpha

  assert.deepEqual(openTitles(h), ["beta"]);
  assert.equal(h.find({ role: "tabs" }).activeIndex, 0);
  assert.deepEqual(detailsNames(h), ["b1.txt"]);
});

// --- the split divider --------------------------------------------------------

test("body divider drag updates the ratio", () => {
  const h = ui(treeRoot());

  closeTo(h.find({ id: "fm:split" }).ratio, fm.BODY_SPLIT_RATIO);

  h.find({ id: "fm:split" }).dragDivider(0.6);

  closeTo(h.find({ id: "fm:split" }).ratio, 0.6);
  // The app keeps re-declaring its own starting literal; the store alone
  // is what the next real render actually paints.
  closeTo(h.find({ id: "fm:split" }).get("ratio"), fm.BODY_SPLIT_RATIO);
});

test("outer divider drag persists but never touches the body split", () => {
  const h = ui(treeRoot());

  h.find({ id: "fm:outer" }).dragDivider(0.9);

  closeTo(h.find({ id: "fm:outer" }).ratio, 0.9);
  closeTo(h.find({ id: "fm:split" }).ratio, fm.BODY_SPLIT_RATIO);
});

// --- chrome: toolbar legend + live status bar --------------------------------

test("toolbar band is present and rendered", () => {
  const h = ui(treeRoot());

  const chrome = h.find({ role: "sidebar_panel" });
  assert.equal(chrome.id, "fm:chrome");

  const toolbar = chrome.find({ role: "toolbar" });
  assert.equal(toolbar.id, "fm:toolbar");
  assert.ok(toolbar.button("Open").label.includes("Open"));
  h.assertText("Move", "Expand", "Collapse", "Open", "Refresh");
});

test("status bar reflects cursor and open tabs", () => {
  const root = treeRoot();
  const h = ui(root);

  assert.equal(status(h).id, "fm:status");
  // The root is loaded (alpha, beta, readme.md == 3), no tabs yet.
  assert.ok(status(h).textOf("left_segments").includes(path.basename(root)));
  assert.ok(status(h).textOf("left_segments").includes("3 items"));
  assert.ok(status(h).textOf("right_segments").includes("no tabs"));

  h.press("Down"); // -> alpha (still unopened)
  assert.ok(status(h).textOf("left_segments").includes("unopened"));

  h.press("Enter"); // open alpha: sub/, a1.txt, a2.txt == 3
  assert.ok(status(h).textOf("left_segments").includes("alpha"));
  assert.ok(status(h).textOf("left_segments").includes("3 items"));
  assert.ok(status(h).textOf("right_segments").includes("1 tab"));
  h.assertText("1 tab");
});

// --- scrolling: the view follows the cursor, the scrollbar paints ------------

test("a short tree paints no scrollbar", () => {
  const h = ui(treeRoot()); // 3 rows, 30 tall

  h.assertNotText("░", "█"); // track glyph, thumb glyph
});

test("an overflowing tree paints a scrollbar on TUI", () => {
  const h = ui(deepRoot(20), 60, 12);

  h.assertText("░", "█"); // track, thumb
});

test("scroll follows the cursor down and clamps", () => {
  const root = deepRoot(20);
  const h = ui(root, 60, 12);

  assert.equal(tree(h).scrollOffset, 0);
  for (let i = 0; i < 15; i++) {
    h.press("Down");
  }

  // The cursor advanced past the first window, so the view scrolled...
  assert.ok(tree(h).scrollOffset > 0);
  // ...and the cursored folder is actually on screen (not off the bottom).
  h.assertText(cursor(h));

  // Arrowing back to the top pulls the window back to the start.
  for (let i = 0; i < 20; i++) {
    h.press("Up");
  }
  assert.equal(tree(h).scrollOffset, 0);
  assert.equal(cursor(h), path.basename(root));
});

test("a focus-scoped key event moves the cursor", () => {
  const root = treeRoot();
  const h = ui(root);

  assert.equal(cursor(h), path.basename(root));

  tree(h).press("Down"); // the shape a focused tree receives
  assert.equal(cursor(h), "alpha");

  h.press("Down"); // the plain top-level shape
  assert.equal(cursor(h), "beta");
});
