// Black-box coverage for the platform-services surface this binding exposes
// as thin napi-rs wrappers over `quadraui_binding_core::services`: `QuiApp`'s
// auto-update (`checkForUpdate`/`downloadAndStage`/`applyOnQuit`),
// power-save blocker (`beginPowerSaveBlock`/`endPowerSaveBlock`),
// launch-at-login (`setLoginItem`/`loginItemEnabled`), protocol registration
// (`setAsDefaultProtocolClient`/`isDefaultProtocolClient`/
// `removeAsDefaultProtocolClient`), drag-out (`beginDragOut`), recent
// docs/dock menu/jump list (`addRecentDocument`/`clearRecentDocuments`/
// `setDockMenu`/`setJumpList`) methods, their `QuiAppActions` mirrors, and
// the three module-level, app-less functions (`appPath`, `setAppIdentifier`,
// `installCrashHandler`) -- a port of
// `quadraui-py/tests/test_platform_parity.py`.
//
// Plus the three surfaces only `quadraui-py` had before this file: the tray
// (`setTrayIcon`/`setTrayTooltip`/`setTrayMenu`), runtime cursor
// (`setCursor`), and window document-state (`setDocumentEdited`/
// `setRepresentedFilename`) methods.
//
// Every `QuiApp` method here resolves `quadraui::PlatformServices`/
// `WindowControl`/`TrayService`/`Backend::set_cursor` off this app's own
// headless `TuiBackend`, which implements none of the `PlatformServices`
// methods here: a terminal owns no sleep inhibitor, login-item
// registration, protocol registration, drag source, recent-documents list,
// dock menu, jump list, or updater. Every such method falls through to
// `BackendError::Unsupported` on this backend, surfaced as
// `UnsupportedError`/`InternalError` rather than a panic or a silent no-op --
// this file's main acceptance case. Argument and handle validation in
// `quadraui_binding_core` runs ahead of the service call, so a bad `kind`
// string or an unknown handle still raises `InvalidDescriptorError`
// instead.
//
// Run against a built addon, e.g.:
//
//     npm run build:debug
//     npm test
const { test } = require("node:test");
const assert = require("node:assert/strict");

const {
  QuiApp,
  InvalidDescriptorError,
  InternalError,
  UnsupportedError,
  appPath,
  setAppIdentifier,
  installCrashHandler,
} = require("../js/index.ts");

// ─── Auto-update ────────────────────────────────────────────────────────────

test("checkForUpdate raises UnsupportedError on TUI", () => {
  const app = new QuiApp(10, 10);
  assert.throws(
    () => app.checkForUpdate("https://example.test/appcast.xml"),
    (err: unknown) => err instanceof UnsupportedError,
  );
});

test("downloadAndStage raises UnsupportedError on TUI", () => {
  const app = new QuiApp(10, 10);
  const info = {
    version: "1.2.3",
    notes: "notes",
    url: "https://example.test/v1.2.3",
    signature: "sig",
  };
  assert.throws(
    () => app.downloadAndStage(info),
    (err: unknown) => err instanceof UnsupportedError,
  );
});

test("applyOnQuit rejects an unknown handle", () => {
  const app = new QuiApp(10, 10);
  assert.throws(
    () => app.applyOnQuit(999999),
    (err: unknown) => err instanceof InvalidDescriptorError,
  );
});

// ─── Power-save blocker ─────────────────────────────────────────────────────
//
// A terminal has no sleep-inhibitor API to call into, so `TuiBackend` keeps
// the trait's `Unsupported` default here rather than minting a handle it
// never backs with anything. Argument/handle validation still happens in
// `quadraui_binding_core` ahead of the service call, so the two rejection
// cases below are unaffected by that.

test("beginPowerSaveBlock raises UnsupportedError on TUI", () => {
  const app = new QuiApp(10, 10);
  assert.throws(
    () => app.beginPowerSaveBlock("prevent_display_sleep"),
    (err: unknown) => err instanceof UnsupportedError,
  );
});

test("beginPowerSaveBlock rejects an unknown kind", () => {
  const app = new QuiApp(10, 10);
  assert.throws(
    () => app.beginPowerSaveBlock("not_a_real_kind"),
    (err: unknown) => err instanceof InvalidDescriptorError,
  );
});

test("endPowerSaveBlock rejects an unknown handle", () => {
  const app = new QuiApp(10, 10);
  assert.throws(
    () => app.endPowerSaveBlock(999999),
    (err: unknown) => err instanceof InvalidDescriptorError,
  );
});

// ─── Launch at login ────────────────────────────────────────────────────────

test("loginItemEnabled raises UnsupportedError on TUI", () => {
  const app = new QuiApp(10, 10);
  assert.throws(
    () => app.loginItemEnabled(),
    (err: unknown) => err instanceof UnsupportedError,
  );
});

test("setLoginItem raises UnsupportedError on TUI", () => {
  const app = new QuiApp(10, 10);
  assert.throws(
    () => app.setLoginItem(true),
    (err: unknown) => err instanceof UnsupportedError,
  );
});

// ─── Protocol registration ──────────────────────────────────────────────────

test("protocol-client methods raise UnsupportedError on TUI", () => {
  const app = new QuiApp(10, 10);
  assert.throws(
    () => app.setAsDefaultProtocolClient("myapp"),
    (err: unknown) => err instanceof UnsupportedError,
  );
  assert.throws(
    () => app.isDefaultProtocolClient("myapp"),
    (err: unknown) => err instanceof UnsupportedError,
  );
  assert.throws(
    () => app.removeAsDefaultProtocolClient("myapp"),
    (err: unknown) => err instanceof UnsupportedError,
  );
});

// ─── Drag-out ────────────────────────────────────────────────────────────────

test("beginDragOut raises UnsupportedError on TUI", () => {
  const app = new QuiApp(10, 10);
  assert.throws(
    () => app.beginDragOut(["/tmp/a.txt"]),
    (err: unknown) => err instanceof UnsupportedError,
  );
});

// ─── Recent docs / dock menu / jump list ─────────────────────────────────────

test("addRecentDocument raises UnsupportedError on TUI", () => {
  const app = new QuiApp(10, 10);
  assert.throws(
    () => app.addRecentDocument("/tmp/a.txt"),
    (err: unknown) => err instanceof UnsupportedError,
  );
});

test("clearRecentDocuments raises UnsupportedError on TUI", () => {
  const app = new QuiApp(10, 10);
  assert.throws(
    () => app.clearRecentDocuments(),
    (err: unknown) => err instanceof UnsupportedError,
  );
});

test("setDockMenu raises UnsupportedError on TUI", () => {
  const app = new QuiApp(10, 10);
  assert.throws(
    () => app.setDockMenu([{ label: { spans: [] } }]),
    (err: unknown) => err instanceof UnsupportedError,
  );
});

test("setDockMenu rejects a malformed items value", () => {
  const app = new QuiApp(10, 10);
  assert.throws(
    () => app.setDockMenu("not a list"),
    (err: unknown) => err instanceof InvalidDescriptorError,
  );
});

test("setJumpList raises UnsupportedError on TUI", () => {
  const app = new QuiApp(10, 10);
  assert.throws(
    () => app.setJumpList([{ title: "Recent file" }]),
    (err: unknown) => err instanceof UnsupportedError,
  );
});

test("setJumpList rejects a malformed items value", () => {
  const app = new QuiApp(10, 10);
  assert.throws(
    () => app.setJumpList("not a list"),
    (err: unknown) => err instanceof InvalidDescriptorError,
  );
});

// ─── App standard paths / app identifier / crash handler (module-level) ────

test("appPath returns a real path", () => {
  assert.ok(appPath("temp"));
});

test("appPath rejects an unknown kind", () => {
  assert.throws(
    () => appPath("not_a_real_kind"),
    (err: unknown) => err instanceof InvalidDescriptorError,
  );
});

test("setAppIdentifier changes app-scoped paths", () => {
  setAppIdentifier("quadraui-node-parity-test");
  const resolved = appPath("app_config");
  assert.ok(resolved.includes("quadraui-node-parity-test"));
});

test("installCrashHandler accepts a config", () => {
  installCrashHandler({ log_dir: require("node:os").tmpdir() });
});

test("installCrashHandler rejects a malformed config", () => {
  assert.throws(
    () => installCrashHandler("not an object"),
    (err: unknown) => err instanceof InvalidDescriptorError,
  );
});

// ─── QuiAppActions mirrors ───────────────────────────────────────────────────
//
// Every void-returning method above has a matching `QuiAppActions` method,
// queued from inside a handler and applied right after that dispatch
// finishes. Each of these only proves the deferred path is reachable and
// eventually raises (recorded as `app.run`'s own pending error) the same
// `UnsupportedError`/`InvalidDescriptorError` the matching `QuiApp` method
// would raise synchronously.

test("actions().applyOnQuit surfaces its error after dispatch", () => {
  const app = new QuiApp(10, 10);
  const actions = app.actions();
  app.setHandler(() => {
    actions.applyOnQuit(999999);
  });
  app.pushKeyEvent("a".codePointAt(0));
  assert.throws(
    () => app.run(1),
    (err: unknown) => err instanceof InvalidDescriptorError,
  );
});

test("actions().endPowerSaveBlock surfaces its error after dispatch", () => {
  const app = new QuiApp(10, 10);
  const actions = app.actions();
  app.setHandler(() => {
    actions.endPowerSaveBlock(999999);
  });
  app.pushKeyEvent("a".codePointAt(0));
  assert.throws(
    () => app.run(1),
    (err: unknown) => err instanceof InvalidDescriptorError,
  );
});

test("actions().setLoginItem surfaces UnsupportedError after dispatch", () => {
  const app = new QuiApp(10, 10);
  const actions = app.actions();
  app.setHandler(() => {
    actions.setLoginItem(true);
  });
  app.pushKeyEvent("a".codePointAt(0));
  assert.throws(
    () => app.run(1),
    (err: unknown) => err instanceof UnsupportedError,
  );
});

test("actions().setDockMenu surfaces UnsupportedError after dispatch", () => {
  const app = new QuiApp(10, 10);
  const actions = app.actions();
  app.setHandler(() => {
    actions.setDockMenu([{ label: { spans: [] } }]);
  });
  app.pushKeyEvent("a".codePointAt(0));
  assert.throws(
    () => app.run(1),
    (err: unknown) => err instanceof UnsupportedError,
  );
});

test("actions().setDockMenu surfaces a malformed items value after dispatch", () => {
  // Unlike `setTrayMenu`, `QuiAppActions.setDockMenu` only converts `items`
  // to JSON at queue time (a bare string is already valid JSON); the
  // actual item-list shape is decoded by
  // `quadraui_binding_core::set_dock_menu` when `flush_deferred_actions`
  // applies it.
  const app = new QuiApp(10, 10);
  const actions = app.actions();
  app.setHandler(() => {
    actions.setDockMenu("not a list");
  });
  app.pushKeyEvent("a".codePointAt(0));
  assert.throws(
    () => app.run(1),
    (err: unknown) => err instanceof InvalidDescriptorError,
  );
});

test("actions().beginDragOut surfaces UnsupportedError after dispatch", () => {
  const app = new QuiApp(10, 10);
  const actions = app.actions();
  app.setHandler(() => {
    actions.beginDragOut(["/tmp/a.txt"]);
  });
  app.pushKeyEvent("a".codePointAt(0));
  assert.throws(
    () => app.run(1),
    (err: unknown) => err instanceof UnsupportedError,
  );
});

test("actions().clearRecentDocuments surfaces UnsupportedError after dispatch", () => {
  const app = new QuiApp(10, 10);
  const actions = app.actions();
  app.setHandler(() => {
    actions.clearRecentDocuments();
  });
  app.pushKeyEvent("a".codePointAt(0));
  assert.throws(
    () => app.run(1),
    (err: unknown) => err instanceof UnsupportedError,
  );
});

// ─── Tray, cursor & window document state ──────────────────────────────────
//
// Genuinely new surfaces this binding didn't expose before this file --
// `quadraui-py` already had all three. Every one of these raises
// `InternalError` on the headless `TuiBackend` -- `tray()` is always `None`
// there, and `WindowControl::set_document_edited`/
// `set_represented_filename` are both unimplemented -- except `setCursor`,
// which returns `false` rather than raising (`Backend::set_cursor` has no
// error case; see that method's own doc).

test("setTrayIcon raises InternalError on TUI (no tray concept)", () => {
  const app = new QuiApp(10, 10);
  assert.throws(
    () => app.setTrayIcon({ Path: "/tmp/icon.png" }),
    (err: unknown) => err instanceof InternalError,
  );
});

test("setTrayIcon accepts a Buffer Bytes source, still raising InternalError on TUI", () => {
  // `Buffer`/`Uint8Array`/`ArrayBuffer` under `Bytes` needs the
  // byte-array materialization `setTrayIcon`'s decode does via
  // `decode_via_json` rather than `decode_direct` -- asserting
  // `InternalError` here (not `InvalidDescriptorError`) is what proves
  // the icon was decoded successfully and only then rejected by
  // `with_tray`'s `Unsupported` fold on this backend.
  const app = new QuiApp(10, 10);
  assert.throws(
    () => app.setTrayIcon({ Bytes: Buffer.from([0, 1, 2, 3]) }),
    (err: unknown) => err instanceof InternalError,
  );
});

test("setTrayIcon rejects a malformed descriptor", () => {
  const app = new QuiApp(10, 10);
  assert.throws(
    () => app.setTrayIcon({ NotAVariant: "oops" }),
    (err: unknown) => err instanceof InvalidDescriptorError,
  );
});

test("setTrayTooltip raises InternalError on TUI", () => {
  const app = new QuiApp(10, 10);
  assert.throws(
    () => app.setTrayTooltip("hello"),
    (err: unknown) => err instanceof InternalError,
  );
});

test("setTrayMenu raises InternalError on TUI", () => {
  const app = new QuiApp(10, 10);
  assert.throws(
    () => app.setTrayMenu([{ label: { spans: [{ text: "Quit", style: {} }] } }]),
    (err: unknown) => err instanceof InternalError,
  );
});

test("setTrayMenu rejects a malformed items value", () => {
  const app = new QuiApp(10, 10);
  assert.throws(
    () => app.setTrayMenu([{ not: "an item" }]),
    (err: unknown) => err instanceof InvalidDescriptorError,
  );
});

test("setDocumentEdited raises InternalError on TUI", () => {
  const app = new QuiApp(10, 10);
  assert.throws(
    () => app.setDocumentEdited(true),
    (err: unknown) => err instanceof InternalError,
  );
});

test("setRepresentedFilename raises InternalError on TUI", () => {
  const app = new QuiApp(10, 10);
  assert.throws(
    () => app.setRepresentedFilename("/tmp/document.txt"),
    (err: unknown) => err instanceof InternalError,
  );
});

test("setRepresentedFilename accepts null to clear", () => {
  const app = new QuiApp(10, 10);
  assert.throws(
    () => app.setRepresentedFilename(null),
    (err: unknown) => err instanceof InternalError,
  );
});

test("setCursor reports false on TUI (no native pointer concept)", () => {
  const app = new QuiApp(10, 10);
  assert.equal(app.setCursor("pointer"), false);
});

test("setCursor rejects an unknown shape", () => {
  const app = new QuiApp(10, 10);
  assert.throws(
    () => app.setCursor("not_a_real_shape"),
    (err: unknown) => err instanceof InvalidDescriptorError,
  );
});

test("setCursor('resize') requires an edge", () => {
  const app = new QuiApp(10, 10);
  assert.throws(
    () => app.setCursor("resize"),
    (err: unknown) => err instanceof InvalidDescriptorError,
  );
  assert.equal(app.setCursor("resize", "north"), false);
});

test("setCursor('resize') rejects an unknown edge", () => {
  const app = new QuiApp(10, 10);
  assert.throws(
    () => app.setCursor("resize", "not_an_edge"),
    (err: unknown) => err instanceof InvalidDescriptorError,
  );
});

test("actions().setTrayIcon surfaces InternalError after dispatch", () => {
  const app = new QuiApp(10, 10);
  const actions = app.actions();
  app.setHandler(() => {
    actions.setTrayIcon({ Path: "/tmp/icon.png" });
  });
  app.pushKeyEvent("a".codePointAt(0));
  assert.throws(
    () => app.run(1),
    (err: unknown) => err instanceof InternalError,
  );
});

test("actions().setTrayMenu rejects a malformed items value at queue time", () => {
  // Decoding happens synchronously inside the queueing call itself, from
  // the handler's own call frame -- a malformed descriptor still raises
  // right there, not deferred to `flush_deferred_actions`.
  const app = new QuiApp(10, 10);
  const actions = app.actions();
  app.setHandler(() => {
    assert.throws(
      () => actions.setTrayMenu([{ not: "an item" }]),
      (err: unknown) => err instanceof InvalidDescriptorError,
    );
  });
  app.pushKeyEvent("a".codePointAt(0));
  app.run(1);
});

test("actions().setDocumentEdited surfaces InternalError after dispatch", () => {
  const app = new QuiApp(10, 10);
  const actions = app.actions();
  app.setHandler(() => {
    actions.setDocumentEdited(true);
  });
  app.pushKeyEvent("a".codePointAt(0));
  assert.throws(
    () => app.run(1),
    (err: unknown) => err instanceof InternalError,
  );
});

test("actions().setCursor queued from a handler does not raise", () => {
  // Unlike the direct `QuiApp.setCursor`, `QuiAppActions.setCursor` has
  // no return value to assert on -- this only proves the deferred path
  // is reachable at all (the gap this fix round closes) and doesn't
  // itself raise, the same shape `quadraui-py`'s
  // `test_set_cursor_queued_from_a_handler_does_not_raise` pins.
  const app = new QuiApp(10, 10);
  const actions = app.actions();
  app.setHandler(() => {
    actions.setCursor("col_resize");
  });
  app.pushKeyEvent("a".codePointAt(0));
  app.run(1);
});
