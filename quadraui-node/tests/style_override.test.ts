// Black-box coverage for the per-node "style" key
// `quadraui_binding_core::wire::style_override_from_value` decodes off
// any layout-tree node dict, and
// `quadraui_binding_core::render::paint_layout`'s `LayoutNode::Styled` arm
// applies as a scoped Theme/Style override for that node's own subtree --
// a port of `quadraui-py/tests/test_style_override.py`.
//
// Every tree here is wrapped in a "row" -- "style" only takes effect on
// the layout-tree decode path (`quadraui_binding_core::wire::is_layout_tree`
// sniffs for "row"/"column"/"split"/..., never a bare leaf key like
// "button"/"text_display" on its own), the same flat-vs-tree split
// `wire_version.test.ts`'s own "the flat (pre-layout-tree) shape also
// accepts wire_version" case exercises from the other side.
//
// `cellColors` is this surface's own pixel-readback probe -- see
// `theme.test.ts`'s own doc for why -- used the same way here: a known,
// fixed-size viewport makes the probed coordinate deterministic without
// needing a find-by-label lookup (`QuiApp` has none).
//
// Run against a built addon, e.g.:
//
//     npm run build:debug
//     npm test

const { test } = require("node:test");
const assert = require("node:assert/strict");

const { QuiApp } = require("../js/index.ts");

test("style: a \"bg\" override on a row changes a background-reading leaf's cell", () => {
  // A "text_display" leaf reads Theme.background for its own fill
  // (unlike a bordered/filled surface), so a plain "bg" override on the
  // enclosing "row" is the right probe for the generic "container-ish"
  // case.
  const app = new QuiApp(10, 1);
  app.setRender(() => ({
    row: [
      {
        id: "td",
        text_display: { id: "td", lines: [{ spans: [{ text: "x" }] }] },
      },
    ],
    style: { bg: "#112233" },
  }));
  assert.deepEqual(app.cellColors(0, 0)[1], [17, 34, 51]);
});

test("style: an override on a nested leaf does not leak to its sibling", () => {
  // Two equal-share "text_display" leaves in a 10-wide row -- each gets
  // 5 columns. Only the second carries its own "style" override.
  const app = new QuiApp(10, 1);
  app.setRender(() => ({
    row: [
      {
        id: "left",
        text_display: { id: "left", lines: [{ spans: [{ text: "x" }] }] },
      },
      {
        id: "right",
        text_display: { id: "right", lines: [{ spans: [{ text: "x" }] }] },
        style: { bg: "#112233" },
      },
    ],
  }));
  const defaultBg = app.cellColors(0, 0)[1];
  assert.deepEqual(app.cellColors(1, 0)[1], defaultBg, "the unstyled left child is unaffected");
  assert.deepEqual(
    app.cellColors(6, 0)[1],
    [17, 34, 51],
    "the styled right child shows its own override",
  );
});

test("style: \"surface_bg\" is the alias a Secondary button fill actually reads", () => {
  // "[ Go ]" at a 6x1 viewport exactly fills the button -- cell (0, 0)
  // is the opening bracket, filled with the same colour as the rest of
  // the button body.
  function appWith(style: Record<string, unknown> | undefined) {
    const app = new QuiApp(6, 1);
    const leaf: Record<string, unknown> = {
      id: "btn",
      button: { id: "btn", label: "Go", variant: "Secondary" },
    };
    if (style !== undefined) {
      leaf.style = style;
    }
    app.setRender(() => ({ row: [leaf] }));
    return app;
  }

  const defaultBg = appWith(undefined).cellColors(0, 0)[1];
  assert.deepEqual(
    appWith({ bg: "#00ff00" }).cellColors(0, 0)[1],
    defaultBg,
    'a plain "bg" override must not change a Secondary button\'s fill -- it reads Theme.surface_bg, not Theme.background',
  );
  assert.deepEqual(
    appWith({ surface_bg: "#00ff00" }).cellColors(0, 0)[1],
    [0, 255, 0],
    '"surface_bg" is the alias that actually reaches a Secondary button\'s fill',
  );
});

test("style: an unknown key is a clear UnknownKeyError", () => {
  const app = new QuiApp(10, 1);
  app.setRender(() => ({
    row: [{ id: "td", text_display: { id: "td", lines: [] } }],
    style: { background: "#000000" },
  }));
  assert.throws(() => app.screen(), /UnknownKeyError/);
  assert.throws(() => app.screen(), /style/);
});
