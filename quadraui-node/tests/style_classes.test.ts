// Black-box coverage for named style sets: `QuiApp.defineStyle(name,
// overrides)` and the "class" key any layout-tree node may carry to pull a
// registered set's tokens in -- the CSS-class analogue of the per-node
// "style" key `style_override.test.ts` covers, and a port of
// `quadraui-py/tests/test_style_classes.py`.
//
// Every tree here is wrapped in a "row", and every assertion reads back a
// painted cell through `cellColors`, for the same reasons
// `style_override.test.ts`'s own header gives.
//
// Run against a built addon, e.g.:
//
//     npm run build:debug
//     npm test

const { test } = require("node:test");
const assert = require("node:assert/strict");

const { QuiApp } = require("../js/index.ts");

/** One "text_display" leaf -- the probe for a plain "bg" override, since a
 * text display reads Theme.background for its own fill. */
function textLeaf(leafId: string, extra: Record<string, unknown> = {}) {
  return {
    id: leafId,
    text_display: { id: leafId, lines: [{ spans: [{ text: "x" }] }] },
    ...extra,
  };
}

function appWith(leaf: unknown, width = 10) {
  const app = new QuiApp(width, 1);
  app.setRender(() => ({ row: [leaf] }));
  return app;
}

test("class: a defined class applies its set's tokens to the node", () => {
  const app = new QuiApp(10, 1);
  app.defineStyle("card", { bg: "#112233" });
  app.setRender(() => ({ row: [textLeaf("td", { class: "card" })] }));
  assert.deepEqual(app.cellColors(0, 0)[1], [17, 34, 51]);
});

test("class: a class does not leak to an unclassed sibling", () => {
  // Two equal-share leaves in a 10-wide row -- 5 columns each. Only the
  // second names the class.
  const app = new QuiApp(10, 1);
  app.defineStyle("card", { bg: "#112233" });
  app.setRender(() => ({
    row: [textLeaf("left"), textLeaf("right", { class: "card" })],
  }));
  const defaultBg = app.cellColors(0, 0)[1];
  assert.deepEqual(app.cellColors(1, 0)[1], defaultBg);
  assert.deepEqual(app.cellColors(6, 0)[1], [17, 34, 51]);
});

test("class: later classes win the tokens earlier ones also named", () => {
  const appWithClasses = (classes: unknown) => {
    const app = new QuiApp(10, 1);
    app.defineStyle("first", { bg: "#ff0000" });
    app.defineStyle("second", { bg: "#00ff00" });
    app.setRender(() => ({ row: [textLeaf("td", { class: classes })] }));
    return app;
  };

  assert.deepEqual(appWithClasses(["first", "second"]).cellColors(0, 0)[1], [0, 255, 0]);
  assert.deepEqual(appWithClasses(["second", "first"]).cellColors(0, 0)[1], [255, 0, 0]);
  // A single whitespace-separated string names the same two sets, in the
  // same order -- the CSS class= convention.
  assert.deepEqual(appWithClasses("first second").cellColors(0, 0)[1], [0, 255, 0]);
  // A whitespace-separated array entry splits into the same two names too,
  // not one literal name containing a space.
  assert.deepEqual(appWithClasses(["first second"]).cellColors(0, 0)[1], [0, 255, 0]);
});

test("class: an inline \"style\" key wins over the node's own class", () => {
  const app = new QuiApp(10, 1);
  app.defineStyle("card", { bg: "#ff0000", fg: "#0000ff" });
  app.setRender(() => ({
    row: [textLeaf("td", { class: "card", style: { bg: "#00ff00" } })],
  }));
  const [fg, bg] = app.cellColors(0, 0);
  assert.deepEqual(bg, [0, 255, 0], "the inline style bg beats the class's own");
  assert.deepEqual(fg, [0, 0, 255], "a class token the inline style didn't name still applies");
});

test("class: redefining a set repaints with the new tokens", () => {
  const app = new QuiApp(10, 1);
  app.defineStyle("card", { bg: "#ff0000" });
  app.setRender(() => ({ row: [textLeaf("td", { class: "card" })] }));
  assert.deepEqual(app.cellColors(0, 0)[1], [255, 0, 0]);

  app.defineStyle("card", { bg: "#00ff00" });
  assert.deepEqual(app.cellColors(0, 0)[1], [0, 255, 0]);
});

test("class: an unknown class name is ignored rather than throwing", () => {
  // The v1 unknown-key policy: a name no set was defined under resolves to
  // nothing, leaving the node painted exactly as if it had no "class" key
  // at all -- unlike an unknown key *inside* "style", which throws.
  const plain = appWith(textLeaf("td")).cellColors(0, 0)[1];
  const unknown = appWith(textLeaf("td", { class: "no-such-set" })).cellColors(0, 0)[1];
  assert.deepEqual(unknown, plain);

  const app = new QuiApp(10, 1);
  app.defineStyle("card", { bg: "#112233" });
  app.setRender(() => ({
    row: [textLeaf("td", { class: ["no-such-set", "card"] })],
  }));
  assert.deepEqual(app.cellColors(0, 0)[1], [17, 34, 51], "the known name still applies");
});

test("class: the built-in sets exist without any defineStyle call", () => {
  const app = new QuiApp(10, 1);
  assert.deepEqual(app.styleSetNames(), ["danger", "primary", "secondary", "subtle"]);

  // "danger" maps onto the Danger button variant's own resting fill,
  // Theme.error_fg, through both the surface_bg and accent_bg aliases --
  // so it reaches a Secondary button (surface_bg).
  const plain = new QuiApp(6, 1);
  plain.setRender(() => ({
    row: [{ id: "b", button: { id: "b", label: "Go", variant: "Secondary" } }],
  }));
  const defaultBg = plain.cellColors(0, 0)[1];

  const classed = new QuiApp(6, 1);
  const errorFg = classed.theme().error_fg;
  classed.setRender(() => ({
    row: [
      {
        id: "b",
        button: { id: "b", label: "Go", variant: "Secondary" },
        class: "danger",
      },
    ],
  }));
  const dangerBg = classed.cellColors(0, 0)[1];
  assert.notDeepEqual(dangerBg, defaultBg);
  assert.deepEqual(dangerBg, [errorFg.r, errorFg.g, errorFg.b]);
});

test("class: a defined set replaces a built-in of the same name", () => {
  const app = new QuiApp(10, 1);
  app.defineStyle("danger", { bg: "#00ff00" });
  app.setRender(() => ({ row: [textLeaf("td", { class: "danger" })] }));
  assert.deepEqual(app.cellColors(0, 0)[1], [0, 255, 0]);
  assert.deepEqual(app.styleSetNames(), ["danger", "primary", "secondary", "subtle"]);
});

test("defineStyle: rejects an unknown override key", () => {
  const app = new QuiApp(10, 1);
  assert.throws(() => app.defineStyle("card", { background: "#000000" }), /unknown key/);
});

test("defineStyle: rejects an empty name", () => {
  const app = new QuiApp(10, 1);
  assert.throws(() => app.defineStyle("", { bg: "#000000" }), /name/);
});

test("defineStyle: rejects a name containing whitespace", () => {
  // Closes the gap a "class" string/array entry would otherwise open: both
  // split on whitespace (see the array-entry test below), so a name
  // containing it could never be referenced through "class" at all.
  const app = new QuiApp(10, 1);
  assert.throws(() => app.defineStyle("card danger", { bg: "#000000" }), /whitespace/);
});

test("class: rejects a wrongly typed value", () => {
  const app = new QuiApp(10, 1);
  app.setRender(() => ({ row: [textLeaf("td", { class: 7 })] }));
  assert.throws(() => app.screen(), /class/);
});
