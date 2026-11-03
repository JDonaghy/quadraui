// Black-box coverage for `QuiApp`'s theme/style/font configuration surface:
// `theme`/`setTheme`, `style`/`setStyle`, `setNerdFonts`/
// `nerdFontsEnabled`, the font setters (`setEditorFont`, `setUiFont`,
// `registerFontFromMemory`, `setNerdFontFallback`, `hasFontFamily`), and
// `setCaretShape` -- a port of `quadraui-py/tests/test_theme.py`.
//
// `cellColors` (RGB readback of one headless-backend cell) is how these
// tests prove `setTheme`/`setStyle` actually change what gets *painted*,
// not just what `theme()`/`style()` read back -- an empty `status_bar`
// fills its whole row with `Theme.background`, so inserting one with no
// segments gives a one-cell probe for the background colour with no other
// setup needed.
//
// Run against a built addon, e.g.:
//
//     npm run build:debug
//     npm test
const { test } = require("node:test");
const assert = require("node:assert/strict");

const { QuiApp } = require("../js/index.ts");

function insertEmptyStatusBar(app: any) {
  app.insertExtra("status_bar", {
    id: "status_bar:main",
    left_segments: [],
    right_segments: [],
  });
}

test("setTheme changes the painted background colour", () => {
  const app = new QuiApp(20, 5);
  insertEmptyStatusBar(app);

  // An empty `StatusBar` fills its row with `Theme::default()`'s
  // background (`20, 22, 30`) for both halves of the cell.
  assert.deepEqual(app.cellColors(0, 0), [
    [20, 22, 30],
    [20, 22, 30],
  ]);

  app.setTheme({ background: "#112233" });

  assert.deepEqual(app.cellColors(0, 0), [
    [17, 34, 51],
    [17, 34, 51],
  ]);
});

test("setTheme only touches the named fields", () => {
  const app = new QuiApp(20, 5);
  const before: any = app.theme();

  app.setTheme({ background: "#112233" });

  const after: any = app.theme();
  assert.deepEqual(after.background, { r: 17, g: 34, b: 51, a: 255 });
  // Every other field is untouched.
  assert.deepEqual(after.foreground, before.foreground);
  assert.deepEqual(after.accent_fg, before.accent_fg);
});

test("setTheme accepts an rgb object and an array", () => {
  const app = new QuiApp(20, 5);

  app.setTheme({ foreground: { r: 1, g: 2, b: 3, a: 255 } });
  assert.deepEqual((app.theme() as any).foreground, { r: 1, g: 2, b: 3, a: 255 });

  app.setTheme({ foreground: [9, 8, 7, 255] });
  assert.deepEqual((app.theme() as any).foreground, { r: 9, g: 8, b: 7, a: 255 });
});

test("setTheme rejects an unknown field", () => {
  const app = new QuiApp(20, 5);
  assert.throws(
    () => app.setTheme({ not_a_real_field: "#000000" }),
    /InvalidDescriptorError/,
  );
});

test("setTheme rejects a malformed hex colour", () => {
  const app = new QuiApp(20, 5);
  assert.throws(
    () => app.setTheme({ background: "not-a-colour" }),
    /InvalidDescriptorError/,
  );
});

test("setTheme rejects an undefined value instead of raising on the app", () => {
  // `Object.keys` reports `background` as an own key even though its value
  // is JS `undefined` -- a one-character slip in caller code
  // (`{ background: maybeUndefinedVar }`). This must raise a normal
  // `InvalidDescriptorError` the caller can catch, not panic and poison the
  // `QuiApp` instance for every later call.
  const app = new QuiApp(20, 5);
  let caught: any = null;
  try {
    app.setTheme({ background: undefined });
  } catch (err) {
    caught = err;
  }
  assert.match(String(caught), /InvalidDescriptorError/);
  // The app must still be usable afterwards.
  app.setTheme({ background: "#112233" });
  assert.deepEqual((app.theme() as any).background, { r: 17, g: 34, b: 51, a: 255 });
});

test("setStyle overrides a single token and round trips", () => {
  const app = new QuiApp(20, 5);
  const defaultPadding = (app.style() as any).padding;

  app.setStyle({ focus_ring_width: 9.5 });

  const style: any = app.style();
  assert.equal(style.focus_ring_width, 9.5);
  // Unrelated token is untouched.
  assert.deepEqual(style.padding, defaultPadding);
});

test("setStyle rejects an unknown field", () => {
  const app = new QuiApp(20, 5);
  assert.throws(
    () => app.setStyle({ not_a_real_token: 1.0 }),
    /InvalidDescriptorError/,
  );
});

test("setStyle rejects an undefined value instead of raising on the app", () => {
  // Same `Object.keys`-reports-an-`undefined`-value gap as `setTheme`, on
  // `setStyle`'s own merge path.
  const app = new QuiApp(20, 5);
  assert.throws(
    () => app.setStyle({ focus_ring_width: undefined }),
    /InvalidDescriptorError/,
  );
  // The app must still be usable afterwards.
  app.setStyle({ focus_ring_width: 9.5 });
  assert.equal((app.style() as any).focus_ring_width, 9.5);
});

test("nerdFonts round trips through set and get", () => {
  const app = new QuiApp(20, 5);
  // TUI starts disabled.
  assert.equal(app.nerdFontsEnabled(), false);

  app.setNerdFonts(true);
  assert.equal(app.nerdFontsEnabled(), true);

  app.setNerdFonts(false);
  assert.equal(app.nerdFontsEnabled(), false);
});

test("font setters are a no-op but do not raise on tui", () => {
  const app = new QuiApp(20, 5);
  // Fixed-cell TUI has no font concept. These must not raise even though
  // they paint nothing.
  app.setEditorFont("Monospace", 12.0);
  app.setUiFont("Sans 11");
  app.setNerdFontFallback("Nerd Font Fallback");
});

test("registerFontFromMemory returns null on tui", () => {
  const app = new QuiApp(20, 5);
  const bytes = Array.from(Buffer.from("not a real font"));
  assert.equal(app.registerFontFromMemory(bytes), null);
});

test("hasFontFamily returns null on tui", () => {
  const app = new QuiApp(20, 5);
  // `null` means "don't know", never "false".
  assert.equal(app.hasFontFamily("Monospace"), null);
});

test("setCaretShape accepts every documented shape", () => {
  const app = new QuiApp(20, 5);
  app.setCaretShape("block");
  app.setCaretShape("bar");
  app.setCaretShape("underline");
});

test("setCaretShape rejects an unknown shape", () => {
  const app = new QuiApp(20, 5);
  assert.throws(() => app.setCaretShape("not-a-shape"), /InvalidDescriptorError/);
});

test("app can react to a SystemThemeChanged event", () => {
  // `UiEvent::SystemThemeChanged` arrives through the generic
  // `pushEvent`/handler path. A handler cannot call back into this same
  // `QuiApp` while it's running, so a real app reacts by recording the
  // event and calling `setTheme` afterwards, once `step`/`run` has
  // returned -- exactly what this test does.
  const received: any[] = [];
  const app = new QuiApp(20, 5);
  app.setHandler((event: any) => received.push(event));
  insertEmptyStatusBar(app);

  app.pushEvent({
    SystemThemeChanged: { dark: true, accent: null, high_contrast: false },
  });
  app.step();

  assert.ok("SystemThemeChanged" in received[0]);
  const dark = received[0].SystemThemeChanged.dark;
  app.setTheme({ background: dark ? "#000000" : "#ffffff" });

  assert.deepEqual(app.cellColors(0, 0)[1], [0, 0, 0]);
});
