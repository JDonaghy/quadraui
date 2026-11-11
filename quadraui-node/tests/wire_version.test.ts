// Black-box coverage for the `"wire_version"` key `quadraui_binding_core::
// wire::check_wire_version` accepts on a `setRender` callback's returned
// object -- a versionless tree (every pre-existing render callback), a
// tree at the current `quadraui_binding_core::wire_schema::WIRE_VERSION`,
// and an older version are all accepted; a version newer than this host
// understands is a clear `InvalidDescriptorError`.
//
// Run against a built addon, e.g.:
//
//     npm run build:debug
//     npm test

const { test } = require("node:test");
const assert = require("node:assert/strict");

const { QuiApp } = require("../js/index.ts");

test("setRender: a tree with no wire_version key is accepted", () => {
  const app = new QuiApp(20, 5);
  app.setRender(() => ({
    row: [{ id: "a", button: { id: "a", label: "OK" } }],
  }));
  assert.ok(app.screenContains("OK"));
});

test("setRender: a tree carrying wire_version: 1 is accepted", () => {
  const app = new QuiApp(20, 5);
  app.setRender(() => ({
    wire_version: 1,
    row: [{ id: "a", button: { id: "a", label: "OK" } }],
  }));
  assert.ok(app.screenContains("OK"));
});

test("setRender: the flat (pre-layout-tree) shape also accepts wire_version", () => {
  const app = new QuiApp(20, 5);
  app.setRender(() => ({
    wire_version: 1,
    button: { id: "demo:ok", label: "OK" },
  }));
  assert.ok(app.screenContains("OK"));
});

test("setRender: a wire_version newer than this host understands is a clear error", () => {
  const app = new QuiApp(20, 5);
  app.setRender(() => ({
    wire_version: 999,
    button: { id: "demo:ok", label: "OK" },
  }));
  assert.throws(() => app.screen(), /InvalidDescriptorError/);
  assert.throws(() => app.screen(), /wire_version/);
});
