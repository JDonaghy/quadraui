// Black-box coverage for `QuiApp`'s accelerator / global-shortcut surface:
// `registerAccelerator`, `unregisterAccelerator`, `registerGlobalShortcut`,
// `unregisterGlobalShortcut`, `unregisterAllGlobalShortcuts` -- a port of
// `quadraui-py/tests/test_accelerators.py`.
//
// Run against a built addon, e.g.:
//
//     npm run build:debug
//     npm test
const { test } = require("node:test");
const assert = require("node:assert/strict");

const { QuiApp } = require("../js/index.ts");

const CTRL = { shift: false, ctrl: true, alt: false, cmd: false };

function ctrlSEvent() {
  return {
    KeyPressed: { key: { Char: "s" }, modifiers: CTRL, repeat: false },
  };
}

test("a registered accelerator fires an Accelerator event for its chord", () => {
  const app = new QuiApp(10, 10);
  const received: any[] = [];
  app.setHandler((event: any) => received.push(event));

  app.registerAccelerator({
    id: "app.save",
    binding: "Save",
    scope: "Global",
    label: null,
  });
  app.pushEvent(ctrlSEvent());
  app.run(1);

  assert.equal(received.length, 1);
  const [accelId, modifiers] = received[0].Accelerator;
  assert.equal(accelId, "app.save");
  assert.deepEqual(modifiers, CTRL);
});

test("a literal vim-style binding matches the same chord as the universal save", () => {
  const app = new QuiApp(10, 10);
  const received: any[] = [];
  app.setHandler((event: any) => received.push(event));

  app.registerAccelerator({
    id: "app.save.literal",
    binding: { Literal: "<C-s>" },
    scope: "Global",
    label: null,
  });
  app.pushEvent(ctrlSEvent());
  app.run(1);

  assert.equal(received.length, 1);
  const [accelId] = received[0].Accelerator;
  assert.equal(accelId, "app.save.literal");
});

test("an unregistered accelerator leaves the key press as a plain KeyPressed", () => {
  const app = new QuiApp(10, 10);
  const received: any[] = [];
  app.setHandler((event: any) => received.push(event));

  app.registerAccelerator({
    id: "app.save",
    binding: "Save",
    scope: "Global",
    label: null,
  });
  app.unregisterAccelerator("app.save");
  app.pushEvent(ctrlSEvent());
  app.run(1);

  assert.equal(received.length, 1);
  assert.ok("KeyPressed" in received[0]);
  assert.ok(!("Accelerator" in received[0]));
});

test("unregisterAccelerator is idempotent on an unknown id", () => {
  const app = new QuiApp(10, 10);
  // Never registered -- must not raise.
  app.unregisterAccelerator("never.registered");
});

test("registering the same id again replaces its binding wholesale", () => {
  const app = new QuiApp(10, 10);
  let received: any[] = [];
  app.setHandler((event: any) => received.push(event));

  app.registerAccelerator({
    id: "app.action",
    binding: "Save",
    scope: "Global",
    label: null,
  });
  // Re-registering the same id swaps in a new binding, dropping the old
  // one entirely rather than keeping both live.
  app.registerAccelerator({
    id: "app.action",
    binding: { Literal: "<C-q>" },
    scope: "Global",
    label: null,
  });

  app.pushEvent(ctrlSEvent());
  app.run(1);
  assert.equal(received.length, 1);
  assert.ok("KeyPressed" in received[0], "the replaced Ctrl+S binding must no longer fire");

  received = [];
  app.pushEvent({
    KeyPressed: { key: { Char: "q" }, modifiers: CTRL, repeat: false },
  });
  app.run(1);
  assert.equal(received.length, 1);
  const [accelId] = received[0].Accelerator;
  assert.equal(accelId, "app.action");
});

test("registerAccelerator rejects a malformed descriptor", () => {
  const app = new QuiApp(10, 10);
  assert.throws(
    () => app.registerAccelerator({ not: "an accelerator" }),
    /InvalidDescriptorError/,
  );
});

test("registerGlobalShortcut raises InternalError on this binding", () => {
  // No in-tree `PlatformServices` override claims a real OS-wide hotkey
  // yet, so every backend this binding ships -- including the headless
  // `TuiBackend` `QuiApp` always wraps -- inherits
  // `Backend::register_global_shortcut`'s `Unsupported` default. This
  // locks in that seam's current behavior; a real success path needs
  // manual verification once a backend implements one.
  const app = new QuiApp(10, 10);
  assert.throws(
    () =>
      app.registerGlobalShortcut({
        id: "app.summon",
        binding: { Literal: "<C-S-space>" },
        scope: "Global",
        label: null,
      }),
    /InternalError/,
  );
});

test("unregisterGlobalShortcut raises InternalError on this binding", () => {
  const app = new QuiApp(10, 10);
  assert.throws(() => app.unregisterGlobalShortcut(0), /InternalError/);
});

test("unregisterAllGlobalShortcuts raises InternalError on this binding", () => {
  const app = new QuiApp(10, 10);
  assert.throws(() => app.unregisterAllGlobalShortcuts(), /InternalError/);
});
