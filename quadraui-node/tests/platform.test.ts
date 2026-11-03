// Black-box coverage for `QuiApp`'s platform-services surface:
// `showFileOpenDialog`/`showFileSaveDialog`/`showFolderDialog` (plus the
// `queueDialogEvents` scripting hook they share), `systemTheme`, and
// `platformName` -- a port of
// `quadraui-py/tests/test_platform_services.py`.
//
// Every one of these resolves `quadraui::PlatformServices` off this
// `QuiApp`'s own headless `TuiBackend`, so every assertion below is
// reachable with no real terminal, no real desktop session, and no native
// dialog. `sendNotification`/`openUrl`/`revealInFileManager`/`openPath`/
// `beep` are fire-and-forget native effects with no assertable return
// value; this file only proves they're callable and don't raise on an
// obviously-benign input -- their actual visible effect needs a human at a
// real terminal, noted in this PR's SMOKE_TESTS instead of asserted here.
// `moveToTrash` has a directly observable filesystem effect (the file
// disappears), so this file asserts on that instead of only "didn't
// raise". `restoreFromTrash`'s round trip is platform-dependent -- see
// `quadraui::tui::services::TuiPlatformServices::restore_from_trash`'s own
// doc: full support on Windows/freedesktop.org Unix, `Unsupported` on
// macOS (the `trash` crate's `os_limited` module has no macOS
// implementation) -- so this file asserts whichever of those two
// documented outcomes matches `process.platform`, rather than assuming
// one platform the way a straight port of the Python suite would.
//
// Run against a built addon, e.g.:
//
//     npm run build:debug
//     npm test
const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

const { QuiApp } = require("../js/index.ts");

function keyChar(ch: string) {
  return {
    KeyPressed: {
      key: { Char: ch },
      modifiers: { shift: false, ctrl: false, alt: false, cmd: false },
      repeat: false,
    },
  };
}

function keyEnter() {
  return {
    KeyPressed: {
      key: { Named: "Enter" },
      modifiers: { shift: false, ctrl: false, alt: false, cmd: false },
      repeat: false,
    },
  };
}

function keyEscape() {
  return {
    KeyPressed: {
      key: { Named: "Escape" },
      modifiers: { shift: false, ctrl: false, alt: false, cmd: false },
      repeat: false,
    },
  };
}

function tmpDir(): string {
  return fs.mkdtempSync(path.join(os.tmpdir(), "quadraui-node-platform-"));
}

// ─── Folder picker ──────────────────────────────────────────────────────

test("showFolderDialog confirms a selected path", () => {
  const dir = tmpDir();
  fs.mkdirSync(path.join(dir, "targetdir"));
  const app = new QuiApp(60, 20);

  app.queueDialogEvents([
    keyChar("t"),
    keyChar("a"),
    keyChar("r"),
    keyChar("g"),
    keyEnter(),
  ]);

  const result = app.showFolderDialog(undefined, dir);

  assert.equal(result, path.join(dir, "targetdir"));
});

test("showFolderDialog cancelled returns null", () => {
  const dir = tmpDir();
  fs.mkdirSync(path.join(dir, "targetdir"));
  const app = new QuiApp(60, 20);

  app.queueDialogEvents([keyEscape()]);

  assert.equal(app.showFolderDialog(undefined, dir), null);
});

test("showFolderDialog with nothing queued cancels immediately", () => {
  // No `queueDialogEvents` call at all -- the nested loop's very first
  // synthesized event is Escape, so this must resolve to `null` rather
  // than hang.
  const dir = tmpDir();
  const app = new QuiApp(10, 10);
  assert.equal(app.showFolderDialog(undefined, dir), null);
});

// ─── File open/save pickers ─────────────────────────────────────────────

test("showFileOpenDialog confirms a selected path", () => {
  const dir = tmpDir();
  fs.writeFileSync(path.join(dir, "report.txt"), "hi");
  const app = new QuiApp(60, 20);

  app.queueDialogEvents([
    keyChar("r"),
    keyChar("e"),
    keyChar("p"),
    keyChar("o"),
    keyChar("r"),
    keyChar("t"),
    keyEnter(),
  ]);

  const result = app.showFileOpenDialog(undefined, dir);

  assert.equal(result, path.join(dir, "report.txt"));
});

test("showFileSaveDialog seeds the initial filename", () => {
  const dir = tmpDir();
  const app = new QuiApp(60, 20);
  app.queueDialogEvents([keyEnter()]);

  const result = app.showFileSaveDialog(undefined, dir, "untitled.txt");

  assert.equal(result, path.join(dir, "untitled.txt"));
});

// ─── System theme / platform identity ──────────────────────────────────

test("systemTheme reports dark from COLORFGBG", () => {
  const previous = process.env.COLORFGBG;
  process.env.COLORFGBG = "15;0";
  try {
    const app = new QuiApp(10, 10);
    const theme: any = app.systemTheme();
    assert.equal(theme.dark, true);
    assert.equal(theme.accent, null);
    assert.equal(theme.high_contrast, false);
  } finally {
    if (previous === undefined) {
      delete process.env.COLORFGBG;
    } else {
      process.env.COLORFGBG = previous;
    }
  }
});

test("systemTheme reports light from COLORFGBG", () => {
  const previous = process.env.COLORFGBG;
  process.env.COLORFGBG = "0;15";
  try {
    const app = new QuiApp(10, 10);
    const theme: any = app.systemTheme();
    assert.equal(theme.dark, false);
  } finally {
    if (previous === undefined) {
      delete process.env.COLORFGBG;
    } else {
      process.env.COLORFGBG = previous;
    }
  }
});

test("systemTheme raises when COLORFGBG is unset", () => {
  const previous = process.env.COLORFGBG;
  delete process.env.COLORFGBG;
  try {
    const app = new QuiApp(10, 10);
    assert.throws(() => app.systemTheme());
  } finally {
    if (previous !== undefined) {
      process.env.COLORFGBG = previous;
    }
  }
});

test("platformName is tui", () => {
  const app = new QuiApp(10, 10);
  assert.equal(app.platformName(), "tui");
});

// ─── Fire-and-forget native effects ─────────────────────────────────────
//
// No native desktop/terminal session is guaranteed under `node --test` --
// these only prove the call is reachable and well-typed, not that
// anything actually appeared on screen. See this module's own doc and the
// owning PR's SMOKE_TESTS for the manual-verification story.

test("sendNotification does not raise", () => {
  const app = new QuiApp(10, 10);
  app.sendNotification("Title", "Body");
  app.sendNotification("Title", "Body", true, true, "t1");
});

test("beep does not raise", () => {
  const app = new QuiApp(10, 10);
  app.beep();
});

// On macOS, `trash`'s own `AppleScript`-driven delete needs the host to
// have granted the running process Automation permission to send Apple
// events to Finder -- unavailable in a sandboxed/headless macOS run (no
// human ever clicked "Allow" in System Settings). That is a host
// permission gap, not a `quadraui`/`quadraui-node` bug, so these two
// skip (rather than fail) when `moveToTrash` itself reports it.
function isSandboxedMacTrashDenial(err: unknown): boolean {
  return (
    process.platform === "darwin" &&
    err instanceof Error &&
    err.message.includes("Not authorized to send Apple events")
  );
}

test("moveToTrash moves a real file", (t: any) => {
  const dir = tmpDir();
  const victim = path.join(dir, "doomed.txt");
  fs.writeFileSync(victim, "bye");
  const app = new QuiApp(10, 10);

  try {
    app.moveToTrash(victim);
  } catch (err) {
    if (isSandboxedMacTrashDenial(err)) {
      t.skip("sandboxed macOS host has not granted Finder automation permission");
      return;
    }
    throw err;
  }

  assert.equal(fs.existsSync(victim), false);
});

test("restoreFromTrash's documented platform split", (t: any) => {
  const dir = tmpDir();
  const victim = path.join(dir, "doomed.txt");
  fs.writeFileSync(victim, "bye");
  const app = new QuiApp(10, 10);

  try {
    app.moveToTrash(victim);
  } catch (err) {
    if (isSandboxedMacTrashDenial(err)) {
      t.skip("sandboxed macOS host has not granted Finder automation permission");
      return;
    }
    throw err;
  }
  assert.equal(fs.existsSync(victim), false);

  if (process.platform === "darwin") {
    // No programmatic trash listing/restore API on macOS -- documented
    // `Unsupported`, surfaced here as `InternalError`.
    assert.throws(() => app.restoreFromTrash(victim), /InternalError/);
  } else {
    app.restoreFromTrash(victim);
    assert.equal(fs.existsSync(victim), true);
    assert.equal(fs.readFileSync(victim, "utf8"), "bye");
  }
});

test("restoreFromTrash on a path never trashed raises", () => {
  const app = new QuiApp(10, 10);
  assert.throws(() =>
    app.restoreFromTrash("/tmp/quadraui-1932-never-trashed-does-not-exist.txt"),
  );
});
