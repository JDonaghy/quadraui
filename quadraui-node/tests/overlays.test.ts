// Black-box coverage for `QuiApp.showOverlay`/`dismissOverlay` -- the generic
// entry point for every in-canvas modal-overlay kind wired onto
// `InteractionEngine`'s own anchored-overlay slot: `"context_menu"`,
// `"message_dialog"`, `"toast"`, `"tooltip"`, `"completions"`, `"float"`,
// `"rich_text_popup"`, `"popover"`, `"layout_dialog"` -- a port of
// `quadraui-py/tests/test_overlay_primitives.py` and
// `quadraui-py/tests/test_extra_primitives.py`'s `"message_dialog"`/
// `"context_menu"` cases. `QuiAppActions.showOverlay`
// (`tests/callbacks.test.ts`) is a different method on a different class --
// it queues through the deferred-action queue for use from inside a
// handler -- so it covers neither `QuiApp.showOverlay` nor
// `QuiApp.dismissOverlay` directly.
//
// Run against a built addon, e.g.:
//
//     npm run build:debug
//     npm test
const { test } = require("node:test");
const assert = require("node:assert/strict");

const { QuiApp } = require("@quadraui/node");

function locate(app: any, needle: string): [number, number] {
  const lines = app.screen().split("\n");
  for (let row = 0; row < lines.length; row++) {
    const col = lines[row].indexOf(needle);
    if (col !== -1) {
      return [row, col];
    }
  }
  throw new Error(`${JSON.stringify(needle)} not found on screen`);
}

function click(app: any, row: number, col: number) {
  app.pushEvent({
    MouseDown: {
      button: "Left",
      position: { x: col, y: row },
      modifiers: {},
    },
  });
  app.run(1);
}

function escape(app: any) {
  app.pushEvent({
    KeyPressed: { key: { Named: "Escape" }, modifiers: {}, repeat: false },
  });
  app.run(1);
}

// ── Context menu ─────────────────────────────────────────────────────────

function contextMenuItems() {
  return [
    { id: "copy", label: { spans: [{ text: "Copy", fg: null, bg: null }] } },
    { id: "paste", label: { spans: [{ text: "Paste", fg: null, bg: null }] } },
  ];
}

test("showOverlay context_menu then clicking an item activates it", () => {
  const app = new QuiApp(40, 20);
  const received: unknown[] = [];
  app.setHandler((event: unknown) => received.push(event));

  app.showOverlay("context_menu", { items: contextMenuItems() }, { x: 2.0, y: 2.0 });
  assert.ok(app.screenContains("Copy"));

  const [row, col] = locate(app, "Copy");
  click(app, row, col);

  assert.deepEqual(received, [{ ContextMenuItemActivated: "copy" }]);
});

test("showOverlay context_menu escape dismisses without item activation", () => {
  const app = new QuiApp(40, 20);
  const received: unknown[] = [];
  app.setHandler((event: unknown) => received.push(event));

  app.showOverlay("context_menu", { items: contextMenuItems() }, { x: 2.0, y: 2.0 });
  escape(app);

  // A unit `UiEvent` variant (no payload) encodes as the bare variant name,
  // not `{ ContextMenuDismissed: null }`.
  assert.deepEqual(received, ["ContextMenuDismissed"]);
  assert.ok(!app.screenContains("Copy"));
});

test("showOverlay context_menu with a nested submenu opens by Right arrow and activates the nested item", () => {
  // An item's `submenu` nests through the same `"context_menu"` descriptor;
  // opening the child popup by Right arrow and activating its leaf
  // reports that leaf's own id via `ContextMenuItemActivated` -- the
  // same shape a flat menu's activation already carries, regardless of
  // nesting depth.
  const app = new QuiApp(40, 20);
  const received: unknown[] = [];
  app.setHandler((event: unknown) => received.push(event));

  app.showOverlay(
    "context_menu",
    {
      items: [
        {
          id: "view",
          label: { spans: [{ text: "View", fg: null, bg: null }] },
          submenu: [
            {
              id: "toggle_sidebar",
              label: { spans: [{ text: "Toggle Sidebar", fg: null, bg: null }] },
            },
          ],
        },
      ],
    },
    { x: 2.0, y: 2.0 },
  );
  assert.ok(app.screenContains("View"));

  app.pushEvent({
    KeyPressed: { key: { Named: "Right" }, modifiers: {}, repeat: false },
  });
  app.run(1);
  assert.ok(app.screenContains("Toggle Sidebar"), "the submenu must now be painted");
  assert.deepEqual(received, []);

  app.pushEvent({
    KeyPressed: { key: { Named: "Enter" }, modifiers: {}, repeat: false },
  });
  app.run(1);

  assert.deepEqual(received, [{ ContextMenuItemActivated: "toggle_sidebar" }]);
  assert.ok(!app.screenContains("Toggle Sidebar"));
});

test("showOverlay context_menu item with a codicon icon renders its fallback glyph", () => {
  // `ContextMenuItem.icon` accepts the same `{codicon: "...", fallback:
  // "..."}` wire shape a tree row's `icon` does (see
  // `descriptors.test.ts`'s `tree leaf accepts a codicon icon and
  // renders its fallback text` case),
  // but paints into the single-cell gutter `checked`'s checkmark shares
  // (only the fallback's first character renders) rather than a
  // measured-width slot -- the headless driver defaults to nerd fonts
  // off, so that's the fallback's plain-text first character, followed
  // by one blank gap cell ahead of the label.
  const app = new QuiApp(40, 20);
  app.showOverlay(
    "context_menu",
    {
      items: [
        {
          id: "new-file",
          label: { spans: [{ text: "New File", fg: null, bg: null }] },
          icon: { codicon: "new-file", fallback: "NF" },
        },
      ],
    },
    { x: 2.0, y: 2.0 },
  );
  assert.ok(app.screenContains("N New File"));
  assert.ok(!app.screenContains("NNew File"));
});

test("showOverlay context_menu item with an unknown codicon name raises InvalidDescriptorError", () => {
  const app = new QuiApp(40, 20);
  assert.throws(
    () =>
      app.showOverlay(
        "context_menu",
        {
          items: [
            {
              id: "new-file",
              label: { spans: [{ text: "New File", fg: null, bg: null }] },
              icon: { codicon: "not-a-real-codicon-name", fallback: "?" },
            },
          ],
        },
        { x: 2.0, y: 2.0 },
      ),
    /InvalidDescriptorError/,
  );
});

// ── Message dialog ───────────────────────────────────────────────────────

function dialogButtons() {
  return [
    { id: "save", label: "Save", is_default: true },
    { id: "cancel", label: "Cancel", is_cancel: true },
  ];
}

test("showOverlay message_dialog then clicking a button resolves it", () => {
  const app = new QuiApp(40, 20);
  const received: unknown[] = [];
  app.setHandler((event: unknown) => received.push(event));

  app.showOverlay(
    "message_dialog",
    { title: "Unsaved changes", body: "Do you want to save?", buttons: dialogButtons() },
    null,
  );
  assert.ok(app.screenContains("Save"));

  const [row, col] = locate(app, "Save");
  click(app, row, col);

  assert.deepEqual(received, [{ MessageDialogResolved: "save" }]);
  assert.ok(!app.screenContains("Save"));
});

test("showOverlay message_dialog enter resolves the default button", () => {
  const app = new QuiApp(40, 20);
  const received: unknown[] = [];
  app.setHandler((event: unknown) => received.push(event));

  app.showOverlay(
    "message_dialog",
    { title: "Unsaved changes", body: "Do you want to save?", buttons: dialogButtons() },
    null,
  );
  app.pushEvent({
    KeyPressed: { key: { Named: "Enter" }, modifiers: {}, repeat: false },
  });
  app.run(1);

  assert.deepEqual(received, [{ MessageDialogResolved: "save" }]);
});

test("showOverlay message_dialog rejects an empty buttons array", () => {
  const app = new QuiApp(40, 20);
  assert.throws(
    () => app.showOverlay("message_dialog", { title: "Title", body: "Body", buttons: [] }, null),
    /InvalidDescriptorError/,
  );
});

test("showOverlay message_dialog rejects an unknown severity", () => {
  const app = new QuiApp(40, 20);
  assert.throws(
    () =>
      app.showOverlay(
        "message_dialog",
        { title: "Title", body: "Body", buttons: dialogButtons(), severity: "critical" },
        null,
      ),
    /InvalidDescriptorError/,
  );
});

test("dismissOverlay message_dialog closes without resolving", () => {
  const app = new QuiApp(40, 20);
  assert.equal(app.dismissOverlay("message_dialog"), false);

  app.showOverlay(
    "message_dialog",
    { title: "Title", body: "Body", buttons: dialogButtons() },
    null,
  );
  assert.equal(app.dismissOverlay("message_dialog"), true);
  assert.ok(!app.screenContains("Save"));
});

// ── Toast ────────────────────────────────────────────────────────────────

test("showOverlay toast paints over base content and action click routes", () => {
  const app = new QuiApp(40, 20);
  const received: unknown[] = [];
  app.setHandler((event: unknown) => received.push(event));

  app.setRender(() => ({ button: { id: "ok", label: "OK" } }));
  assert.ok(app.screenContains("OK"));

  app.showOverlay(
    "toast",
    {
      id: "toast:saved",
      title: "Saved",
      body: "",
      actions: [{ id: "undo", label: "Undo" }],
    },
    null,
  );
  assert.ok(app.screenContains("OK"));
  assert.ok(app.screenContains("Saved"));
  assert.ok(app.screenContains("Undo"));

  const [row, col] = locate(app, "Undo");
  click(app, row, col);

  assert.deepEqual(received, [{ ToastActionClicked: "undo" }]);
  assert.ok(app.screenContains("Saved"));
});

test("toast dismiss click closes with no event", () => {
  const app = new QuiApp(40, 20);
  const received: unknown[] = [];
  app.setHandler((event: unknown) => received.push(event));

  app.showOverlay("toast", { id: "toast:saved", title: "Saved", body: "" }, null);
  const [row, col] = locate(app, "×");

  click(app, row, col);

  assert.deepEqual(received, []);
  assert.ok(!app.screenContains("Saved"));
});

test("dismissOverlay toast by id closes without input", () => {
  const app = new QuiApp(40, 20);
  app.showOverlay("toast", { id: "toast:a", title: "A", body: "" }, null);
  app.showOverlay("toast", { id: "toast:b", title: "B", body: "" }, null);
  assert.ok(app.screenContains("A"));
  assert.ok(app.screenContains("B"));

  assert.equal(app.dismissOverlay("toast:a"), true);
  assert.ok(!app.screenContains("A"));
  assert.ok(app.screenContains("B"));

  assert.equal(app.dismissOverlay("toast:a"), false);
});

test("showOverlay toast rejects a malformed descriptor", () => {
  const app = new QuiApp(40, 20);
  assert.throws(
    () => app.showOverlay("toast", { id: "toast:bad" }, null),
    /InvalidDescriptorError/,
  );
});

// ── Tooltip ──────────────────────────────────────────────────────────────

test("showOverlay tooltip paints over base content then auto-dismisses on click", () => {
  const app = new QuiApp(40, 20);
  app.setHandler(() => undefined);

  app.setRender(() => ({ button: { id: "ok", label: "OK" } }));
  app.showOverlay(
    "tooltip",
    { id: "tip:hint", text: "Hello tip" },
    { x: 0.0, y: 2.0, width: 10.0, height: 1.0 },
  );

  assert.ok(app.screenContains("OK"));
  assert.ok(app.screenContains("Hello tip"));

  click(app, 0, 0);

  assert.ok(!app.screenContains("Hello tip"));
});

test("dismissOverlay tooltip closes explicitly", () => {
  const app = new QuiApp(40, 20);
  app.showOverlay(
    "tooltip",
    { id: "tip:hint", text: "Hello tip" },
    { x: 0.0, y: 2.0, width: 10.0, height: 1.0 },
  );
  assert.ok(app.screenContains("Hello tip"));

  assert.equal(app.dismissOverlay("tooltip"), true);
  assert.ok(!app.screenContains("Hello tip"));
  assert.equal(app.dismissOverlay("tooltip"), false);
});

// ── Completions ────────────────────────────────────────────────────────

function completionItems() {
  return [
    { label: { spans: [{ text: "map", fg: null, bg: null }] } },
    { label: { spans: [{ text: "filter", fg: null, bg: null }] } },
  ];
}

test("showOverlay completions click selects and double-click activates", () => {
  const app = new QuiApp(40, 20);
  const received: unknown[] = [];
  app.setHandler((event: unknown) => received.push(event));

  app.showOverlay(
    "completions",
    { id: "completions:editor", selected_idx: 0, items: completionItems() },
    { x: 2.0, y: 2.0 },
  );
  assert.ok(app.screenContains("map"));
  assert.ok(app.screenContains("filter"));

  const [row, col] = locate(app, "filter");
  click(app, row, col);

  assert.deepEqual(received, [
    { List: ["completions:editor", { SelectionChanged: { idx: 1 } }] },
  ]);
  assert.ok(app.screenContains("filter"));

  app.pushEvent({ DoubleClick: { button: "Left", position: { x: col, y: row } } });
  app.run(1);

  assert.deepEqual(received[received.length - 1], {
    CompletionItemActivated: ["completions:editor", 1],
  });
  assert.ok(!app.screenContains("filter"));
});

test("completions escape dismisses without activation", () => {
  const app = new QuiApp(40, 20);
  const received: unknown[] = [];
  app.setHandler((event: unknown) => received.push(event));

  app.showOverlay(
    "completions",
    { id: "completions:editor", selected_idx: 0, items: completionItems() },
    { x: 2.0, y: 2.0 },
  );
  escape(app);

  assert.deepEqual(received, []);
  assert.ok(!app.screenContains("map"));
});

test("dismissOverlay completions closes without input", () => {
  const app = new QuiApp(40, 20);
  app.showOverlay(
    "completions",
    { id: "completions:editor", selected_idx: 0, items: completionItems() },
    { x: 2.0, y: 2.0 },
  );
  assert.equal(app.dismissOverlay("completions"), true);
  assert.ok(!app.screenContains("map"));
  assert.equal(app.dismissOverlay("completions"), false);
});

// ── Float ────────────────────────────────────────────────────────────────

function anchor(x: number, y: number, width = 1.0, height = 1.0, side = "Bottom") {
  return {
    rect: { x, y, width, height },
    preferred: side,
    margin: 0.0,
  };
}

test("showOverlay float paints chrome then escape dismisses", () => {
  const app = new QuiApp(40, 20);
  const received: unknown[] = [];
  app.setHandler((event: unknown) => received.push(event));

  app.setRender(() => ({ button: { id: "ok", label: "OK" } }));
  app.showOverlay(
    "float",
    { id: "float:hints", anchor: anchor(2.0, 2.0) },
    { width: 10.0, height: 4.0 },
  );

  assert.ok(app.screenContains("OK"));
  assert.ok(app.screenContains("┌"));

  escape(app);

  assert.deepEqual(received, []);
  assert.ok(!app.screenContains("┌"));
});

test("dismissOverlay float closes without input", () => {
  const app = new QuiApp(40, 20);
  app.showOverlay(
    "float",
    { id: "float:hints", anchor: anchor(2.0, 2.0) },
    { width: 10.0, height: 4.0 },
  );
  assert.equal(app.dismissOverlay("float"), true);
  assert.equal(app.dismissOverlay("float"), false);
});

test("a non-focusable float does not claim escape", () => {
  const app = new QuiApp(40, 20);
  const received: unknown[] = [];
  app.setHandler((event: unknown) => received.push(event));

  app.showOverlay(
    "float",
    { id: "float:hint", anchor: anchor(2.0, 2.0), focusable: false },
    { width: 10.0, height: 4.0 },
  );

  escape(app);

  assert.ok(received.some((event) => typeof event === "object" && event !== null && "KeyPressed" in (event as object)));
  assert.ok(app.screenContains("┌"));
});

// ── RichTextPopup ────────────────────────────────────────────────────────

test("showOverlay rich_text_popup then clicking a link routes", () => {
  const app = new QuiApp(40, 20);
  const received: unknown[] = [];
  app.setHandler((event: unknown) => received.push(event));

  app.showOverlay(
    "rich_text_popup",
    {
      id: "popup:hover",
      lines: [{ spans: [{ text: "See docs", fg: null, bg: null }] }],
      line_text: ["See docs"],
      max_visible_rows: 3,
      links: [{ line: 0, start_byte: 4, end_byte: 8, url: "https://example.com" }],
    },
    { x: 2.0, y: 2.0 },
  );
  assert.ok(app.screenContains("See docs"));

  const [row, col] = locate(app, "docs");
  click(app, row, col);

  assert.deepEqual(received, [
    { RichTextLinkClicked: ["popup:hover", "https://example.com"] },
  ]);
});

test("rich text popup escape dismisses", () => {
  const app = new QuiApp(40, 20);
  app.setHandler(() => undefined);
  app.showOverlay(
    "rich_text_popup",
    {
      id: "popup:hover",
      lines: [{ spans: [{ text: "Hello", fg: null, bg: null }] }],
      line_text: ["Hello"],
      max_visible_rows: 3,
    },
    { x: 2.0, y: 2.0 },
  );
  assert.ok(app.screenContains("Hello"));

  escape(app);

  assert.ok(!app.screenContains("Hello"));
});

test("dismissOverlay rich_text_popup closes without input", () => {
  const app = new QuiApp(40, 20);
  app.showOverlay(
    "rich_text_popup",
    {
      id: "popup:hover",
      lines: [{ spans: [{ text: "Hello", fg: null, bg: null }] }],
      line_text: ["Hello"],
      max_visible_rows: 3,
    },
    { x: 2.0, y: 2.0 },
  );
  assert.equal(app.dismissOverlay("rich_text_popup"), true);
  assert.equal(app.dismissOverlay("rich_text_popup"), false);
});

// ── showOverlay/dismissOverlay's own cross-kind contract ─────────────────
//
// The sections above each pin one kind's own event-routing/paint behavior;
// this one covers what `showOverlay`/`dismissOverlay` guarantee across
// every kind instead -- the returned id's shape, unknown-kind/missing-anchor
// rejection, and an unknown dismiss id being a no-op.

test("showOverlay toast returns its own id and dismissOverlay closes it", () => {
  const app = new QuiApp(40, 20);
  const returnedId = app.showOverlay(
    "toast",
    { id: "toast:saved", title: "Saved", body: "" },
    null,
  );
  assert.equal(returnedId, "toast:saved");
  assert.ok(app.screenContains("Saved"));

  assert.equal(app.dismissOverlay(returnedId), true);
  assert.ok(!app.screenContains("Saved"));
  // Dismissing the same id again is a no-op, not an error.
  assert.equal(app.dismissOverlay(returnedId), false);
});

test("showOverlay message_dialog returns a sentinel and dismissOverlay closes it", () => {
  const app = new QuiApp(40, 20);
  const returnedId = app.showOverlay(
    "message_dialog",
    { title: "Quit?", body: "", buttons: [{ id: "ok", label: "OK" }] },
    null,
  );
  assert.equal(returnedId, "message_dialog");
  assert.ok(app.screenContains("Quit?"));

  assert.equal(app.dismissOverlay("message_dialog"), true);
  assert.ok(!app.screenContains("Quit?"));
});

test("showOverlay context_menu then dismissOverlay closes it", () => {
  const app = new QuiApp(40, 20);
  const returnedId = app.showOverlay(
    "context_menu",
    { items: contextMenuItems() },
    { x: 2.0, y: 2.0 },
  );
  assert.equal(returnedId, "context_menu");
  assert.ok(app.screenContains("Copy"));

  assert.equal(app.dismissOverlay("context_menu"), true);
  assert.ok(!app.screenContains("Copy"));
  assert.equal(app.dismissOverlay("context_menu"), false);
});

test("showOverlay tooltip returns the descriptor's own id", () => {
  const app = new QuiApp(40, 20);
  const returnedId = app.showOverlay(
    "tooltip",
    { id: "tip:hint", text: "Hello tip" },
    { x: 0.0, y: 2.0, width: 10.0, height: 1.0 },
  );

  assert.equal(returnedId, "tip:hint");
  assert.ok(app.screenContains("Hello tip"));
});

test("showOverlay rejects an unknown kind", () => {
  const app = new QuiApp(40, 20);
  assert.throws(
    () => app.showOverlay("not_a_real_kind", {}, null),
    /InvalidDescriptorError/,
  );
});

test("showOverlay rejects a missing anchor when one is required", () => {
  const app = new QuiApp(40, 20);
  assert.throws(
    () =>
      app.showOverlay("tooltip", { id: "tip:hint", text: "Hello tip" }, null),
    /InvalidDescriptorError/,
  );
});

test("dismissOverlay with an unknown id is a no-op", () => {
  const app = new QuiApp(40, 20);
  assert.equal(app.dismissOverlay("not-a-known-overlay-id"), false);
});

// ── Content-bearing overlays (popover / layout dialog) ──────────────────

function toggleFormLeaf() {
  return {
    id: "settings",
    form: {
      id: "settings",
      fields: [
        {
          id: "wrap",
          label: { spans: [{ text: "Word wrap", fg: null, bg: null }] },
          kind: { Toggle: { value: false } },
        },
      ],
    },
  };
}

// The button comes first deliberately: opening a content-bearing overlay
// seeds keyboard focus onto its first tab stop, and the TUI focus-ring
// convention paints a border box over the focused widget's own edge rows
// (`quadraui/src/tui/focus_ring.rs`). A single-row form field as the
// *first* stop would have its only row covered by that ring, which would
// make the assertions below read as "the content never painted".
function popoverDescriptor() {
  return {
    float: { id: "popover:menu", anchor: anchor(2.0, 2.0) },
    content: {
      column: [
        { id: "submit", button: { id: "submit", label: "Submit" } },
        toggleFormLeaf(),
      ],
    },
  };
}

function layoutDialogDescriptor() {
  return {
    dialog: {
      id: "dialog:prefs",
      title: "Preferences",
      buttons: [
        { id: "save", label: "Save", is_default: true },
        { id: "cancel", label: "Cancel", is_cancel: true },
      ],
    },
    content: {
      column: [
        { id: "apply", button: { id: "apply", label: "Apply" } },
        toggleFormLeaf(),
      ],
    },
  };
}

function formToggles(received: unknown[]) {
  return received.filter(
    (event) =>
      typeof event === "object" &&
      event !== null &&
      "Form" in (event as object) &&
      "ToggleChanged" in ((event as any).Form[1] as object),
  );
}

test("showOverlay popover paints its content and routes a button click", () => {
  const app = new QuiApp(60, 24);
  const received: unknown[] = [];
  app.setHandler((event: unknown) => received.push(event));
  app.setRender(() => ({ button: { id: "ok", label: "OK" } }));

  const returnedId = app.showOverlay("popover", popoverDescriptor(), {
    width: 24.0,
    height: 8.0,
  });

  assert.equal(returnedId, "popover:menu");
  assert.ok(app.screenContains("Submit"));
  assert.ok(app.screenContains("Word wrap"));

  const [row, col] = locate(app, "Submit");
  click(app, row, col);

  // The popover's `button` leaf rides the generic widget seam, so its
  // click resolves to a bare `ButtonActivated` naming the leaf.
  assert.ok(
    received.some(
      (event) =>
        typeof event === "object" &&
        event !== null &&
        (event as any).ButtonActivated?.widget === "submit",
    ),
    JSON.stringify(received),
  );
});

test("showOverlay popover toggle emits the normal semantic event", () => {
  const app = new QuiApp(60, 24);
  const received: unknown[] = [];
  app.setHandler((event: unknown) => received.push(event));

  app.showOverlay("popover", popoverDescriptor(), { width: 24.0, height: 8.0 });
  const [row, col] = locate(app, "Word wrap");
  click(app, row, col);

  const toggles = formToggles(received);
  assert.ok(toggles.length > 0, JSON.stringify(received));
  assert.equal((toggles[0] as any).Form[1].ToggleChanged.value, true);
});

test("showOverlay popover is dismissed by an outside click", () => {
  const app = new QuiApp(60, 24);
  // A registered handler is what routes events through the engine at all,
  // so the dismissal below needs one even though it asserts on the screen.
  app.setHandler(() => undefined);
  app.setRender(() => ({ button: { id: "ok", label: "OK" } }));
  app.showOverlay("popover", popoverDescriptor(), { width: 24.0, height: 8.0 });
  assert.ok(app.screenContains("Submit"));

  click(app, 23, 59);

  assert.ok(!app.screenContains("Submit"));
});

test("showOverlay layout_dialog paints chrome plus content", () => {
  const app = new QuiApp(60, 24);
  app.setRender(() => ({ button: { id: "ok", label: "OK" } }));

  const returnedId = app.showOverlay("layout_dialog", layoutDialogDescriptor(), {
    width: 40.0,
    height: 6.0,
  });

  assert.equal(returnedId, "dialog:prefs");
  assert.ok(app.screenContains("Preferences"));
  assert.ok(app.screenContains("Cancel"));
  assert.ok(app.screenContains("Word wrap"));
});

test("showOverlay layout_dialog toggle emits the normal semantic event", () => {
  const app = new QuiApp(60, 24);
  const received: unknown[] = [];
  app.setHandler((event: unknown) => received.push(event));

  app.showOverlay("layout_dialog", layoutDialogDescriptor(), {
    width: 40.0,
    height: 6.0,
  });
  const [row, col] = locate(app, "Word wrap");
  click(app, row, col);

  assert.ok(formToggles(received).length > 0, JSON.stringify(received));
});

test("showOverlay layout_dialog escape resolves its cancel button", () => {
  const app = new QuiApp(60, 24);
  const received: unknown[] = [];
  app.setHandler((event: unknown) => received.push(event));

  app.showOverlay("layout_dialog", layoutDialogDescriptor(), {
    width: 40.0,
    height: 6.0,
  });
  escape(app);

  const resolved = received.filter(
    (event) =>
      typeof event === "object" &&
      event !== null &&
      "MessageDialogResolved" in (event as object),
  );
  assert.ok(resolved.length > 0, JSON.stringify(received));
  assert.equal((resolved[0] as any).MessageDialogResolved, "cancel");
  assert.ok(!app.screenContains("Preferences"));
});

test("dismissOverlay closes a popover and a layout dialog", () => {
  const app = new QuiApp(60, 24);
  app.showOverlay("popover", popoverDescriptor(), { width: 24.0, height: 8.0 });
  assert.equal(app.dismissOverlay("popover"), true);
  assert.equal(app.dismissOverlay("popover"), false);

  app.showOverlay("layout_dialog", layoutDialogDescriptor(), {
    width: 40.0,
    height: 6.0,
  });
  assert.equal(app.dismissOverlay("layout_dialog"), true);
  assert.equal(app.dismissOverlay("layout_dialog"), false);
});

test("showOverlay popover rejects a descriptor missing its content", () => {
  const app = new QuiApp(60, 24);
  assert.throws(
    () =>
      app.showOverlay(
        "popover",
        { float: { id: "popover:menu", anchor: anchor(2.0, 2.0) } },
        { width: 24.0, height: 8.0 },
      ),
    /MissingFieldError/,
  );
});
