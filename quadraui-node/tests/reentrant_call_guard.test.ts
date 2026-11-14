// Black-box coverage for three ergonomics fixes to `QuiApp`'s error
// surface:
//
// 1. Every `QuiApp` method restricted to top-level script code (the ones
//    with a matching `QuiAppActions` method -- `showMessageDialog`,
//    `setTitle`, ...) throws `ReentrantCallError`, with a message naming
//    the method and pointing at `actions()`, instead of napi-rs's own
//    generic "cannot be borrowed mutably" rejection, when called from
//    inside this app's own registered handler -- see `js/index.ts`'s
//    `wrapNativeErrors`/`REENTRANT_RESTRICTED_METHODS`.
// 2. A `render` callback that returns something other than a plain object
//    (an array, a string, `null`, ...) throws `InvalidDescriptorError`
//    with the message `"render callback must return a dict"` -- the same
//    message `quadraui-py`'s own `render_output_from_callback` raises,
//    where `to_descriptor_value`'s JSON materialization would otherwise
//    decode an array as a (silently empty) flat frame instead of
//    rejecting it.
// 3. `step()`/`run()` with nothing queued (no event, no tick handler that
//    requests a redraw) still renders once, so a `render` callback that
//    decodes badly throws immediately rather than only on the next
//    `screen()`/`screenContains()` call.
//
// Run against a built addon, e.g.:
//
//     npm run build:debug
//     npm test

const { test } = require("node:test");
const assert = require("node:assert/strict");

const { InvalidDescriptorError, QuiApp, ReentrantCallError } = require("../js/index.ts");

const RESTRICTED_METHOD_CALLS: Array<[string, unknown[]]> = [
  ["showMessageDialog", ["Title", "Body", [{ id: "ok", label: "OK" }], null]],
  ["showContextMenu", [[{ id: "a", label: "A" }], { x: 0, y: 0 }]],
  ["showToast", [{ id: "t1", title: "Hi", body: "there" }, null]],
  ["showTooltip", [{ id: "tip", text: "hint" }, { x: 0, y: 0, width: 1, height: 1 }]],
  ["hideTooltip", []],
  ["sendNotification", ["Title", "Body", null, null, null]],
  ["openPath", ["/tmp"]],
  ["revealInFileManager", ["/tmp"]],
  ["moveToTrash", ["/tmp/does-not-exist-quadraui-test"]],
  ["restoreFromTrash", ["/tmp/does-not-exist-quadraui-test"]],
  ["setDocumentEdited", [true]],
  ["setRepresentedFilename", ["/tmp/report.pdf"]],
  ["setTrayIcon", [{ Path: "/tmp/icon.png" }]],
  ["setTrayTooltip", ["hint"]],
  ["setTrayMenu", [[{ id: "a", label: "A" }]]],
  ["setCursor", ["default", null]],
  ["setTheme", [{ fg: "#ffffff" }]],
  ["defineStyle", ["card", { bg: "#445566" }]],
  ["addRecentDocument", ["/tmp/does-not-exist-quadraui-test"]],
  ["clearRecentDocuments", []],
  ["setDockMenu", [[{ id: "a", label: "A" }]]],
  ["setJumpList", [[{ title: "t", program: "p" }]]],
  ["setLoginItem", [true]],
  ["setAsDefaultProtocolClient", ["quadraui-test"]],
  ["removeAsDefaultProtocolClient", ["quadraui-test"]],
  ["beginDragOut", [["/tmp/does-not-exist-quadraui-test"], null]],
  ["endPowerSaveBlock", [1]],
  ["applyOnQuit", [1]],
];

for (const [method, args] of RESTRICTED_METHOD_CALLS) {
  test(`${method} called from a handler throws ReentrantCallError naming it`, () => {
    const app = new QuiApp(40, 20);
    let caught: unknown = null;
    app.setHandler(() => {
      try {
        (app as unknown as Record<string, (...a: unknown[]) => unknown>)[method](...args);
      } catch (err) {
        caught = err;
      }
    });
    app.pushKeyEvent("a".codePointAt(0));
    app.step();

    assert.ok(caught instanceof ReentrantCallError, `${method} did not throw ReentrantCallError`);
    assert.equal(
      (caught as Error).message,
      `app.${method} can't be called from inside a handler; use the actions() handle passed to / available in the handler`,
    );
  });
}

test("a restricted method still works normally from top-level script code", () => {
  const app = new QuiApp(40, 20);
  app.setCursor("default", null);
  app.showMessageDialog("Confirm", "Really?", [{ id: "ok", label: "OK" }], null);
  assert.ok(app.screenContains("Confirm"));
});

test("a render callback returning an array throws InvalidDescriptorError", () => {
  const app = new QuiApp(10, 5);
  app.setRender(() => [{ button: { id: "x", label: "x" } }]);
  assert.throws(
    () => app.screen(),
    (err: unknown) =>
      err instanceof InvalidDescriptorError && (err as Error).message === "InvalidDescriptorError: render callback must return a dict",
  );
});

test("an idle step with nothing queued still renders and surfaces a render error", () => {
  const app = new QuiApp(10, 5);
  let calls = 0;
  app.setRender(() => {
    calls++;
    return [{ button: { id: "x", label: "x" } }];
  });

  assert.throws(() => app.step());
  assert.equal(calls, 1, "an idle step() must invoke the render callback");
});

test("an idle step with nothing queued still re-renders a valid frame", () => {
  const app = new QuiApp(20, 5);
  let calls = 0;
  app.setRender(() => {
    calls++;
    return { button: { id: "demo:btn", label: "OK" } };
  });
  assert.equal(calls, 0);

  // No event, no tick handler requesting a redraw -- still re-renders.
  app.step();
  assert.equal(calls, 1);
});
