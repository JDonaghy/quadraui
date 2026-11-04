// Black-box coverage for the callback bridge (`setHandler`/`setTickHandler`/
// `setRender`), the re-entrancy guard, and the `actions()` deferred-action
// queue -- ports the shape of `quadraui-py/src/lib.rs`'s
// `reaction_from_return_treats_only_explicit_false_as_exit`, its
// `native_app_logic_window_close_handler_*` cases, and
// `handler_queues_show_message_dialog_via_actions_handle_without_reentrant_borrow`,
// plus a new case this crate's own `QuiApp::check_not_dispatching` needs
// that PyO3's runtime borrow guard already covers for `quadraui-py`: a
// handler calling back into `QuiApp` mid-dispatch must raise, not corrupt
// state.
//
// Run against a built addon, e.g.:
//
//     npm run build:debug
//     npm test
const { test } = require("node:test");
const assert = require("node:assert/strict");

const { QuiApp } = require("../js/index.ts");

test("setHandler is called with the converted event payload", () => {
  const app = new QuiApp(10, 5);
  const received: unknown[] = [];
  app.setHandler((event: unknown) => {
    received.push(event);
  });
  app.pushKeyEvent("a".codePointAt(0));
  app.step();
  assert.equal(received.length, 1);
});

test("a handler returning only an explicit false exits; true/undefined/non-boolean keep running", () => {
  const app = new QuiApp(10, 5);
  let nextReturn: unknown = undefined;
  app.setHandler(() => nextReturn);

  nextReturn = true;
  app.pushKeyEvent("a".codePointAt(0));
  assert.equal(app.step(), true, "an explicit true must keep running");

  nextReturn = undefined;
  app.pushKeyEvent("a".codePointAt(0));
  assert.equal(app.step(), true, "no explicit return must keep running");

  nextReturn = 0;
  app.pushKeyEvent("a".codePointAt(0));
  assert.equal(app.step(), true, "a falsy non-boolean must keep running");

  nextReturn = false;
  app.pushKeyEvent("a".codePointAt(0));
  assert.equal(app.step(), false, "only an explicit false must request exit");
});

test("WindowClose with no handler registered keeps running, same as any other unhandled event", () => {
  const app = new QuiApp(10, 5);
  app.pushEvent("WindowClose");
  assert.equal(app.step(), true);
});

test("WindowClose: a handler returning true vetoes the close and stays open", () => {
  const app = new QuiApp(10, 5);
  app.setHandler(() => true);
  app.pushEvent("WindowClose");
  assert.equal(app.step(), true);
});

test("WindowClose: a handler returning false confirms the close and exits", () => {
  const app = new QuiApp(10, 5);
  app.setHandler(() => false);
  app.pushEvent("WindowClose");
  assert.equal(app.step(), false);
});

test("WindowClose: a handler with no explicit opinion (no return) confirms the close by default", () => {
  const app = new QuiApp(10, 5);
  app.setHandler(() => {
    /* no return */
  });
  app.pushEvent("WindowClose");
  assert.equal(app.step(), false);
});

test("setTickHandler is invoked whenever a step has no event queued", () => {
  const app = new QuiApp(10, 5);
  let ticks = 0;
  app.setTickHandler(() => {
    ticks += 1;
  });
  app.step();
  app.step();
  assert.equal(ticks, 2);
});

test("a tick handler returning false exits without raising", () => {
  const app = new QuiApp(10, 5);
  app.setTickHandler(() => false);
  assert.equal(app.step(), false);
});

test("setRender overrides the fixed slots for every frame", () => {
  const app = new QuiApp(40, 5);
  app.setRender(() => ({
    button: { id: "demo:ok", label: "From render" },
  }));
  assert.ok(app.screenContains("From render"));
});

test("setRender honours a row's padding/gap spacing keys", () => {
  // `"padding"`/`"gap"` are siblings of the `"row"` key in the same node
  // dict -- on this headless backend one native unit is one terminal
  // cell, so `padding: 2` must inset the leaf exactly 2 rows/columns from
  // the top-left corner.
  const app = new QuiApp(20, 6);
  app.setRender(() => ({
    row: [{ id: "left", text_display: { id: "text:left", lines: [{ spans: [{ text: "L", fg: null, bg: null }] }] } }],
    padding: 2,
  }));
  const lines = app.screen().split("\n");
  assert.equal(lines[0].trim(), "", "no padding rows stay blank");
  assert.equal(lines[2].slice(0, 2), "  ", "padding columns stay blank");
  assert.equal(lines[2].trim(), "L");
});

test("a handler calling back into QuiApp (e.g. insertButton) mid-dispatch raises instead of corrupting state", () => {
  const app = new QuiApp(20, 5);
  let reentrantCallThrew = false;
  app.setHandler(() => {
    try {
      app.insertButton({ id: "demo:ok", label: "OK" });
    } catch {
      reentrantCallThrew = true;
    }
  });
  app.pushKeyEvent("a".codePointAt(0));
  app.step();
  assert.ok(reentrantCallThrew, "a reentrant call into QuiApp must raise, not succeed");
  assert.ok(!app.screenContains("OK"), "the reentrant insertButton call must not have applied");
});

test("actions().showMessageDialog queues a dialog a handler cannot open directly mid-dispatch", () => {
  const app = new QuiApp(40, 20);
  const actions = app.actions();
  app.setHandler(() => {
    actions.showMessageDialog("Title", "Body", [{ id: "ok", label: "OK" }], null);
  });
  app.pushKeyEvent("a".codePointAt(0));
  app.step();
  assert.ok(app.screenContains("Title"), "the deferred dialog must be painted once step applies the queue");
});

test("actions() raises if called re-entrantly from inside a handler", () => {
  const app = new QuiApp(10, 5);
  let threw = false;
  app.setHandler(() => {
    try {
      app.actions();
    } catch {
      threw = true;
    }
  });
  app.pushKeyEvent("a".codePointAt(0));
  app.step();
  assert.ok(threw, "calling actions() mid-dispatch must raise");
});
