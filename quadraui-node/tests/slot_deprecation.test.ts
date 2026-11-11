// Black-box coverage for the slot/extra API's deprecation warnings: every
// `insert*`/`insertExtra`/`clearExtra`/`slotClear` method still works
// exactly as before, but each reports a one-time `process.emitWarning`
// naming the inline layout-leaf kind that replaces it -- exactly once per
// app, even across repeated calls to the same method.
//
// Run against a built addon, e.g.:
//
//     npm run build:debug
//     npm test
const { test } = require("node:test");
const assert = require("node:assert/strict");

const { QuiApp } = require("../js/index.ts");

/** Collects every `process` "warning" event fired while (and immediately
 * after) `fn` runs, restoring the listener afterward regardless of whether
 * `fn` throws. `process.emitWarning` delivers the event asynchronously
 * (queued, not synchronous with the call that triggered it), so this awaits
 * a macrotask turn before returning -- long enough for Node's own warning
 * dispatch to have run. */
async function collectWarnings(fn: () => void): Promise<Error[]> {
  const warnings: Error[] = [];
  const listener = (warning: Error) => warnings.push(warning);
  process.on("warning", listener);
  try {
    fn();
    await new Promise((resolve) => setImmediate(resolve));
  } finally {
    process.off("warning", listener);
  }
  return warnings;
}

const DESCRIPTOR_FOR: Record<string, unknown> = {
  insertButton: { id: "demo:btn", label: "OK" },
  insertTextDisplay: { id: "demo:text", lines: [] },
  insertList: { id: "demo:list", items: [], selected_idx: 0 },
  insertTree: {
    id: "demo:tree",
    rows: [],
    selection_mode: "Single",
    selected_path: null,
    style: {
      indent: 2,
      show_chevrons: true,
      chevron_expanded: "v",
      chevron_collapsed: ">",
    },
  },
  insertDataTable: { id: "demo:table", columns: [], rows: [] },
  insertTabBar: { id: "demo:tabs", tabs: [] },
  insertProgress: { id: "demo:progress", label: "Working", value: 0.5 },
  insertSpinner: { id: "demo:spinner", label: "Loading" },
  insertImage: {
    id: "demo:image",
    source: { Path: "logo.png" },
    intrinsic_size: null,
    fit: "Contain",
    fallback_text: "[LOGO]",
  },
};

const LEAF_KIND_FOR: Record<string, string> = {
  insertButton: "button",
  insertTextDisplay: "text_display",
  insertList: "list",
  insertTree: "tree",
  insertDataTable: "data_table",
  insertTabBar: "tab_bar",
  insertProgress: "progress",
  insertSpinner: "spinner",
  insertImage: "image",
};

for (const method of Object.keys(DESCRIPTOR_FOR)) {
  test(`${method} reports a DeprecationWarning naming the inline replacement`, async () => {
    const app = new QuiApp(40, 20);
    const leafKind = LEAF_KIND_FOR[method];
    const warnings = await collectWarnings(() => {
      (app as unknown as Record<string, (...args: unknown[]) => unknown>)[method](
        DESCRIPTOR_FOR[method],
      );
    });
    assert.equal(warnings.length, 1);
    assert.equal(warnings[0].name, "DeprecationWarning");
    assert.match(warnings[0].message, new RegExp(`${method}.*\`${leafKind}\``));
  });
}

test("insertExtra reports a DeprecationWarning naming the requested kind as the leaf", async () => {
  const app = new QuiApp(40, 20);
  const warnings = await collectWarnings(() => {
    app.insertExtra("progress", { id: "demo:progress2", label: "Working", value: 0.5 });
  });
  assert.equal(warnings.length, 1);
  assert.match(warnings[0].message, /insertExtra.*`progress`/);
});

test("clearExtra reports a DeprecationWarning naming omission from render as the replacement", async () => {
  const app = new QuiApp(40, 20);
  // Flushes `insertProgress`'s own warning first -- `process.emitWarning`
  // delivers asynchronously, so an un-awaited setup call here would
  // otherwise still be pending when the listener below attaches, and get
  // counted as part of this test's own measurement.
  await collectWarnings(() => {
    app.insertProgress({ id: "demo:progress", label: "Working", value: 0.5 });
  });
  const warnings = await collectWarnings(() => {
    app.clearExtra("progress");
  });
  assert.equal(warnings.length, 1);
  assert.match(warnings[0].message, /clearExtra.*`progress`/);
});

test("slotClear reports a DeprecationWarning naming the fixed slot's leaf kind", async () => {
  const app = new QuiApp(40, 20);
  let slot!: number;
  // See `clearExtra`'s own test above for why this setup call is flushed
  // through `collectWarnings` before the real measurement below.
  await collectWarnings(() => {
    slot = app.insertButton({ id: "demo:btn", label: "OK" });
  });
  const warnings = await collectWarnings(() => {
    app.slotClear(slot);
  });
  assert.equal(warnings.length, 1);
  assert.match(warnings[0].message, /slotClear.*`button`/);
});

test("a deprecated method still has its normal effect underneath the warning", async () => {
  const app = new QuiApp(40, 20);
  let slot: number | undefined;
  await collectWarnings(() => {
    slot = app.insertButton({ id: "demo:btn", label: "OK" });
  });
  assert.equal(slot, 0);
  assert.ok(app.screenContains("OK"));
});

test("a deprecated method called repeatedly on the same app warns only once per method", async () => {
  const app = new QuiApp(40, 20);
  const warnings = await collectWarnings(() => {
    app.insertProgress({ id: "demo:progress", label: "Working", value: 0.5 });
    app.clearExtra("progress");
    app.insertProgress({ id: "demo:progress", label: "Working", value: 0.5 });
    app.clearExtra("progress");
  });
  const insertWarnings = warnings.filter((w) => /insertProgress/.test(w.message));
  const clearWarnings = warnings.filter((w) => /clearExtra/.test(w.message));
  assert.equal(insertWarnings.length, 1);
  assert.equal(clearWarnings.length, 1);
});
