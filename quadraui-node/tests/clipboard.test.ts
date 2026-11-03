// Black-box coverage for `QuiApp`'s clipboard surface:
// `clipboardWriteText`/`clipboardReadText`/`clipboardReadPrimarySelection`/
// `clipboardReadImage`/`clipboardWriteImage`/`clipboardWriteHtml`/
// `clipboardReadFileList` -- a port of
// `quadraui-py/tests/test_clipboard.py`.
//
// Unlike a native-window run (which this crate's `QuiApp` has no analog
// of), these calls never touch the controlling terminal's raw mode, so
// they are safe to exercise in-process:
// `quadraui::tui::services::TuiClipboard::write_text` only (1)
// best-effort tries the local `arboard` clipboard, (2) emits an OSC 52
// escape sequence to stdout and, on Unix, `/dev/tty` (both skipped
// silently if unavailable), and (3) best-effort spawns a native CLI tool
// on a detached background thread. None of those legs block or require a
// real terminal, and all degrade silently on failure.
//
// A fully headless CI runner without `arboard`/X11/Wayland support may
// not actually retain what gets written, so the roundtrip assertion
// tolerates a `null` read-back -- the behavioral guarantee under test is
// that both calls complete without raising, not that every environment
// has a working system clipboard.
//
// Run against a built addon, e.g.:
//
//     npm run build:debug
//     npm test
const { test } = require("node:test");
const assert = require("node:assert/strict");

const { QuiApp } = require("../js/index.ts");

test("clipboard bindings are present", () => {
  const app: any = new QuiApp(40, 20);
  assert.equal(typeof app.clipboardWriteText, "function");
  assert.equal(typeof app.clipboardReadText, "function");
});

test("clipboardWriteText does not raise", () => {
  const app = new QuiApp(40, 20);
  app.clipboardWriteText("quadraui-node clipboard smoke test");
});

test("clipboardReadText returns a string or null", () => {
  const app = new QuiApp(40, 20);
  const result = app.clipboardReadText();
  assert.ok(result === null || typeof result === "string");
});

test("clipboardWriteText then clipboardReadText round trips or null", () => {
  const app = new QuiApp(40, 20);
  const written = "quadraui-node clipboard roundtrip marker";
  app.clipboardWriteText(written);
  const readBack = app.clipboardReadText();
  // A working system clipboard returns exactly what was written; an
  // environment with no clipboard backend at all returns `null` instead
  // of raising -- both are acceptable here.
  assert.ok(readBack === null || readBack === written);
});

test("the clipboard backend persists across calls", () => {
  // The underlying `TuiBackend` this `QuiApp` always wraps is constructed
  // once and reused -- the arboard handle and any clipboard-serving
  // background thread must stay alive for the lifetime of the app, not
  // be torn down and rebuilt on every call. Multiple calls in a row should
  // not raise or panic.
  const app = new QuiApp(40, 20);
  app.clipboardWriteText("first");
  app.clipboardWriteText("second");
  const result = app.clipboardReadText();
  assert.ok(result === null || typeof result === "string");
});

// ── Richer clipboard formats ────────────────────────────────────────────
//
// This app's one `TuiBackend` always wraps `TuiClipboard`, which only
// overrides `Clipboard::read_text`/`write_text`. Every other `Clipboard`
// method -- `read_image`/`write_image`/`write_html`/`read_file_list`/
// `read_primary_selection` -- therefore takes its trait default here
// (`Err(Unsupported)` or `None`), deterministically, regardless of the
// host environment. These assertions are exact, not "or null" like the
// plain-text tests above, for exactly that reason.

test("richer clipboard bindings are present", () => {
  const app: any = new QuiApp(40, 20);
  assert.equal(typeof app.clipboardReadImage, "function");
  assert.equal(typeof app.clipboardWriteImage, "function");
  assert.equal(typeof app.clipboardWriteHtml, "function");
  assert.equal(typeof app.clipboardReadFileList, "function");
  assert.equal(typeof app.clipboardReadPrimarySelection, "function");
});

test("clipboardReadImage is null on a text-only backend", () => {
  const app = new QuiApp(40, 20);
  assert.equal(app.clipboardReadImage(), null);
});

test("clipboardWriteImage raises InternalError on an unsupported backend", () => {
  const app = new QuiApp(40, 20);
  assert.throws(
    () => app.clipboardWriteImage(1, 1, [0xff, 0x00, 0x00, 0xff]),
    /InternalError/,
  );
});

test("clipboardWriteImage rejects a mismatched pixel buffer", () => {
  const app = new QuiApp(40, 20);
  // 2x2 RGBA8 needs 16 entries; this is deliberately short.
  assert.throws(
    () => app.clipboardWriteImage(2, 2, [0, 0, 0, 0]),
    /InvalidDimensionsError/,
  );
});

test("clipboardWriteHtml raises InternalError on an unsupported backend", () => {
  const app = new QuiApp(40, 20);
  assert.throws(
    () => app.clipboardWriteHtml("<b>hi</b>", "hi"),
    /InternalError/,
  );
});

test("clipboardReadFileList is null on a text-only backend", () => {
  const app = new QuiApp(40, 20);
  assert.equal(app.clipboardReadFileList(), null);
});

test("clipboardReadPrimarySelection is null on a text-only backend", () => {
  const app = new QuiApp(40, 20);
  assert.equal(app.clipboardReadPrimarySelection(), null);
});
