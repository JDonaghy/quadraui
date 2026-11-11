// Black-box coverage for the descriptor decode adapter (`quadraui-node/src/
// lib.rs`'s "Descriptor decode adapter" module doc section): insert each of
// the six fixed primitive slots and the three built-in additive primitives
// from a plain JS object, assert `screen()` shows it, and exercise the two
// audit-flagged pre-checks (`Buffer`-sourced `ImageSource::Bytes`, a
// non-finite float) plus the bad-shape/already-occupied error cases --
// ports the shape of `quadraui-ffi`'s `bad_json_reports_last_error_for_each_new_primitive`
// and `quadraui-py/tests/test_descriptors.py` + `test_extra_primitives.py`.
//
// Run against a built addon, e.g.:
//
//     npm run build:debug
//     npm test
const { test } = require("node:test");
const assert = require("node:assert/strict");

const { QuiApp } = require("../js/index.ts");

test("insertButton renders its label", () => {
  const app = new QuiApp(40, 20);
  const slot = app.insertButton({ id: "demo:ok", label: "OK" });
  assert.equal(slot, 0);
  assert.ok(app.screenContains("OK"));
});

test("insertButton accepts an optional tooltip", () => {
  const app = new QuiApp(40, 20);
  app.insertButton({ id: "demo:ok", label: "OK", tooltip: "Confirm the action" });
  assert.ok(app.screenContains("OK"));
});

test("insertButton rejects a non-string tooltip", () => {
  // `Button::tooltip` is typed `Option<String>`, not merely ignored the
  // way an unrecognized key would be -- a non-string value must fail to
  // decode, proving the field is actually wired into `Button`'s own
  // `Deserialize` rather than silently dropped.
  const app = new QuiApp(40, 20);
  assert.throws(
    () => app.insertButton({ id: "demo:ok", label: "OK", tooltip: 123 }),
    /InvalidDescriptorError/,
  );
});

test("insertTextDisplay renders its line text", () => {
  const app = new QuiApp(40, 20);
  app.insertTextDisplay({
    id: "text:1",
    lines: [{ spans: [{ text: "Hello", fg: null, bg: null }] }],
  });
  assert.ok(app.screenContains("Hello"));
});

test("insertList renders an item's text", () => {
  const app = new QuiApp(40, 20);
  app.insertList({
    id: "list:1",
    items: [{ text: { spans: [{ text: "Row 1", fg: null, bg: null }] } }],
    selected_idx: 0,
  });
  assert.ok(app.screenContains("Row 1"));
});

test("insertTree renders a row's text", () => {
  const app = new QuiApp(40, 20);
  app.insertTree({
    id: "tree:1",
    rows: [
      {
        path: [0],
        indent: 0,
        icon: null,
        text: { spans: [{ text: "Alpha", fg: null, bg: null }] },
        badge: null,
        is_expanded: null,
      },
    ],
    selection_mode: "Single",
    selected_path: null,
    style: {
      indent: 2,
      show_chevrons: true,
      chevron_expanded: "v",
      chevron_collapsed: ">",
    },
  });
  assert.ok(app.screenContains("Alpha"));
});

test("insertTree accepts a codicon icon and renders its fallback text", () => {
  // A tree row's `icon` accepts `{codicon: "...", fallback: "..."}` in
  // place of `{glyph: "...", fallback: "..."}` -- the wire shape `Icon`'s
  // `Deserialize` impl resolves through the bundled codicon name table.
  // The headless driver defaults to nerd fonts off, so the rendered row
  // shows the icon's plain-text fallback.
  const app = new QuiApp(40, 20);
  app.insertTree({
    id: "tree:1",
    rows: [
      {
        path: [0],
        indent: 0,
        icon: { codicon: "folder", fallback: "DIR" },
        text: { spans: [{ text: "src", fg: null, bg: null }] },
        badge: null,
        is_expanded: null,
      },
    ],
    selection_mode: "Single",
    selected_path: null,
    style: {
      indent: 2,
      show_chevrons: true,
      chevron_expanded: "v",
      chevron_collapsed: ">",
    },
  });
  assert.ok(app.screenContains("DIR"));
  assert.ok(app.screenContains("src"));
});

test("insertTree with an unknown codicon name raises InvalidDescriptorError", () => {
  const app = new QuiApp(40, 20);
  assert.throws(
    () =>
      app.insertTree({
        id: "tree:1",
        rows: [
          {
            path: [0],
            indent: 0,
            icon: { codicon: "not-a-real-codicon-name", fallback: "?" },
            text: { spans: [{ text: "src", fg: null, bg: null }] },
            badge: null,
            is_expanded: null,
          },
        ],
        selection_mode: "Single",
        selected_path: null,
        style: {
          indent: 2,
          show_chevrons: true,
          chevron_expanded: "v",
          chevron_collapsed: ">",
        },
      }),
    /InvalidDescriptorError/,
  );
});

test("insertDataTable renders a cell's text", () => {
  const app = new QuiApp(40, 20);
  app.insertDataTable({
    id: "table:1",
    columns: [{ title: "Name" }],
    rows: [{ cells: [{ spans: [{ text: "Row1", fg: null, bg: null }] }] }],
  });
  assert.ok(app.screenContains("Row1"));
});

test("insertTabBar renders a tab's label", () => {
  const app = new QuiApp(40, 20);
  app.insertTabBar({ id: "tabbar:1", tabs: [{ label: "Tab 1" }] });
  assert.ok(app.screenContains("Tab 1"));
});

test("insertButton raises SlotOccupiedError when the slot is already occupied", () => {
  const app = new QuiApp(40, 20);
  app.insertButton({ id: "demo:ok", label: "OK" });
  assert.throws(
    () => app.insertButton({ id: "demo:ok2", label: "OK2" }),
    /SlotOccupiedError/,
  );

  assert.equal(app.slotClear(0), true);
  assert.ok(!app.screenContains("OK"));
  const slot = app.insertButton({ id: "demo:ok2", label: "OK2" });
  assert.equal(slot, 0);
  assert.ok(app.screenContains("OK2"));
});

test("insertButton with a bad shape raises InvalidDescriptorError", () => {
  const app = new QuiApp(40, 20);
  assert.throws(
    () => app.insertButton({ not: "a button" }),
    /InvalidDescriptorError/,
  );
});

test("insertProgress renders its label", () => {
  const app = new QuiApp(40, 20);
  app.insertProgress({ id: "progress:download", label: "Downloading", value: 0.5 });
  assert.ok(app.screenContains("Downloading"));
});

test("insertSpinner renders its label", () => {
  const app = new QuiApp(40, 20);
  app.insertSpinner({ id: "spinner:lsp_boot", label: "Indexing" });
  assert.ok(app.screenContains("Indexing"));
});

test("insertImage with a Path source renders its fallback text", () => {
  const app = new QuiApp(40, 20);
  app.insertImage({
    id: "image:logo",
    source: { Path: "assets/logo.png" },
    intrinsic_size: null,
    fit: "Contain",
    fallback_text: "[LOGO]",
  });
  assert.ok(app.screenContains("[LOGO]"));
});

test("insertImage with a Buffer-sourced Bytes image renders its fallback text", () => {
  // The audit-flagged pre-check: a `Buffer` must fold into the same
  // array-of-numbers shape `quadraui::ImageSource::Bytes(Vec<u8>)` decodes
  // from, not the generic-object branch a bare `Array.isArray` check would
  // otherwise route it through (see `to_descriptor_value`'s doc).
  const app = new QuiApp(40, 20);
  app.insertImage({
    id: "image:logo",
    source: { Bytes: Buffer.from([137, 80, 78, 71]) },
    intrinsic_size: null,
    fit: "Contain",
    fallback_text: "[PNG]",
  });
  assert.ok(app.screenContains("[PNG]"));
});

test("insertImage with a Uint8Array-sourced Bytes image renders its fallback text", () => {
  const app = new QuiApp(40, 20);
  app.insertImage({
    id: "image:logo",
    source: { Bytes: new Uint8Array([137, 80, 78, 71]) },
    intrinsic_size: null,
    fit: "Contain",
    fallback_text: "[PNG2]",
  });
  assert.ok(app.screenContains("[PNG2]"));
});

test("insertImage with a missing required field raises InvalidDescriptorError", () => {
  const app = new QuiApp(40, 20);
  assert.throws(
    () => app.insertImage({ id: "image:logo" }),
    /InvalidDescriptorError/,
  );
});

test("insertExtra rejects a non-finite float with InvalidDescriptorError", () => {
  // The other audit-flagged pre-check: NaN/Infinity must raise rather than
  // silently alias `null` (which `obj_get` treats as "field absent").
  const app = new QuiApp(40, 20);
  assert.throws(
    () => app.insertProgress({ id: "progress:download", label: "Downloading", value: NaN }),
    /InvalidDescriptorError/,
  );
  assert.throws(
    () =>
      app.insertProgress({
        id: "progress:download2",
        label: "Downloading",
        value: Infinity,
      }),
    /InvalidDescriptorError/,
  );
});

test("insertExtra reaches the screen by kind name with no dedicated method", () => {
  const app = new QuiApp(40, 20);
  app.insertExtra("progress", { id: "progress:download", label: "Downloading" });
  assert.ok(app.screenContains("Downloading"));
});

test("insertExtra form with control_width and min_control_width renders without raising", () => {
  // `Form`/`FormField`'s `control_width`/`min_control_width` are plain
  // additive `#[serde(default)]` fields, so they decode through the same
  // generic `insertExtra("form", ...)` path as every other `Form`
  // descriptor -- no dedicated node binding code needed for this feature.
  // The narrow `Fixed(4.0)` control is too small for "exactwidth" (10
  // chars), so without `min_control_width` it would show truncated; the
  // 14-wide floor on the second field keeps its value intact.
  const app = new QuiApp(40, 20);
  app.insertExtra("form", {
    id: "settings",
    fields: [
      {
        id: "clipped",
        label: { spans: [{ text: "A", fg: null, bg: null }] },
        kind: { TextInput: { value: "exactwidth" } },
        control_width: { Fixed: 4.0 },
      },
      {
        id: "floored",
        label: { spans: [{ text: "B", fg: null, bg: null }] },
        kind: { TextInput: { value: "exactwidth" } },
        control_width: { Fixed: 4.0 },
        min_control_width: 14.0,
      },
    ],
  });
  assert.ok(app.screenContains("exactwidth"));
});

test("insertExtra with an unknown kind raises InvalidDescriptorError", () => {
  const app = new QuiApp(40, 20);
  assert.throws(
    () => app.insertExtra("not_a_real_kind", { id: "x" }),
    /InvalidDescriptorError/,
  );
});

test("extra primitives coexist with the fixed slots", () => {
  const app = new QuiApp(40, 20);
  app.insertButton({ id: "demo:ok", label: "OK" });
  app.insertProgress({ id: "progress:download", label: "Downloading" });
  assert.ok(app.screenContains("OK"));
  assert.ok(app.screenContains("Downloading"));
});

test("clearExtra empties a slot and a later insert can reuse it", () => {
  const app = new QuiApp(40, 20);
  app.insertSpinner({ id: "spinner:lsp_boot", label: "Indexing" });
  assert.equal(app.clearExtra("spinner"), true);
  assert.ok(!app.screenContains("Indexing"));

  app.insertSpinner({ id: "spinner:lsp_boot", label: "Booting" });
  assert.ok(app.screenContains("Booting"));
});

// Table proving every interactive `EXTRA_PRIMITIVE_KINDS` entry that has a
// dedicated `FormEvent` round-trips through a literal inline layout-tree
// leaf -- a `{ row: [{ id, <kind>: {...} }] }` object, the exact shape a
// render callback returns -- not just through `insertExtra`'s flat-slot
// path. Each case dispatches the events a real app would push (a focusing
// click, then a key/char where the kind needs one) and asserts the final
// event the handler receives. `switch` already has its own dedicated
// tree-leaf test; `progress`/`spinner`/`image` have no event at all, so
// neither appears here.
const INLINE_LEAF_CASES: Array<{
  name: string;
  tree: unknown;
  events: unknown[];
  expected: Array<null | Record<string, unknown> | [string, unknown]>;
}> = [
  {
    name: "number_input",
    tree: {
      row: [{ id: "ni:1", number_input: { id: "ni:1", min: 0.0, max: 100.0, value: 42.0 } }],
    },
    events: [{ MouseDown: { button: "Left", position: { x: 38.0, y: 0.0 }, modifiers: {} } }],
    expected: [
      { FocusChanged: "ni:1" },
      ["ni:1", { TextInputChanged: { id: "ni:1", value: "43" } }],
    ],
  },
  {
    name: "radio_group",
    tree: {
      row: [
        {
          id: "rg:1",
          radio_group: {
            id: "rg:1",
            options: [
              { id: "opt:a", label: "Alpha" },
              { id: "opt:b", label: "Beta" },
            ],
            focused: true,
          },
        },
      ],
    },
    events: [
      { MouseDown: { button: "Left", position: { x: 1.0, y: 1.0 }, modifiers: {} } },
      { KeyPressed: { key: { Named: "Down" }, modifiers: {}, repeat: false } },
    ],
    expected: [
      { FocusChanged: "rg:1" },
      null, // the focusing click itself falls through unclaimed
      ["rg:1", { SegmentedControlChanged: { id: "rg:1", selected_idx: 0 } }],
    ],
  },
  {
    name: "date_picker",
    tree: {
      row: [
        {
          id: "dp:1",
          date_picker: {
            id: "dp:1",
            visible_month: { year: 2024, month: 3, day: 1 },
            focused: { year: 2024, month: 3, day: 15 },
            locale: { first_weekday: "Sunday", format: "Iso8601" },
            has_focus: true,
          },
        },
      ],
    },
    events: [
      { MouseDown: { button: "Left", position: { x: 1.0, y: 1.0 }, modifiers: {} } },
      { KeyPressed: { key: { Named: "Enter" }, modifiers: {}, repeat: false } },
    ],
    expected: [
      { FocusChanged: "dp:1" },
      null,
      [
        "dp:1",
        { DateChanged: { id: "dp:1", value: { year: 2024, month: 3, day: 15 } } },
      ],
    ],
  },
  {
    name: "combo_box",
    tree: {
      row: [
        {
          id: "cb:1",
          combo_box: {
            id: "cb:1",
            options: ["alpha", "beta"],
            text: "",
            focused: true,
            open: false,
          },
        },
      ],
    },
    events: [
      { MouseDown: { button: "Left", position: { x: 1.0, y: 1.0 }, modifiers: {} } },
      { KeyPressed: { key: { Char: "a" }, modifiers: {}, repeat: false } },
    ],
    expected: [
      { FocusChanged: "cb:1" },
      null,
      ["cb:1", { ComboBoxTextChanged: { id: "cb:1", value: "a" } }],
    ],
  },
  {
    name: "form",
    tree: {
      row: [
        {
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
        },
      ],
    },
    events: [{ MouseDown: { button: "Left", position: { x: 1.0, y: 0.0 }, modifiers: {} } }],
    expected: [
      { FocusChanged: "settings" },
      ["settings", { ToggleChanged: { id: "wrap", value: true } }],
    ],
  },
  {
    name: "text_input",
    tree: { row: [{ id: "ti:1", text_input: { id: "ti:1", lines: ["hello"] } }] },
    events: [
      { MouseDown: { button: "Left", position: { x: 1.0, y: 0.0 }, modifiers: {} } },
      { CharTyped: "!" },
    ],
    expected: [
      { FocusChanged: "ti:1" },
      null,
      ["ti:1", { TextInputChanged: { id: "ti:1", value: "!hello" } }],
    ],
  },
];

for (const { name, tree, events, expected } of INLINE_LEAF_CASES) {
  test(`inline tree leaf round-trips a semantic event: ${name}`, () => {
    const app = new QuiApp(40, 20);
    app.setRender(() => tree);
    // `setRender` itself never repaints -- force the first render so the
    // hit map actually reflects this leaf before the events below.
    app.screen();

    const received: any[] = [];
    app.setHandler((event: unknown) => received.push(event));
    for (const event of events) {
      app.pushEvent(event);
    }
    app.run(events.length);

    assert.equal(
      received.length,
      expected.length,
      `unexpected event count for ${name}: ${JSON.stringify(received)}`,
    );
    for (let i = 0; i < expected.length; i += 1) {
      const want = expected[i];
      const got = received[i];
      if (want === null) {
        // The focusing click itself lands as a raw, unclaimed hit -- only
        // its `FocusChanged` precursor and the later key/char's semantic
        // event are asserted precisely.
        assert.ok(!got.Form, `expected no Form event at index ${i} for ${name}: ${JSON.stringify(got)}`);
        continue;
      }
      if (Array.isArray(want)) {
        assert.deepStrictEqual(got.Form, want, `${name} at index ${i}: ${JSON.stringify(got)}`);
      } else {
        assert.deepStrictEqual(got, want, `${name} at index ${i}: ${JSON.stringify(got)}`);
      }
    }
  });
}

test("clearExtra on an empty or unknown kind returns false, not an error", () => {
  const app = new QuiApp(40, 20);
  assert.equal(app.clearExtra("progress"), false);
  assert.equal(app.clearExtra("not_a_real_kind"), false);
});

test("pushEvent queues an event step consumes without raising", () => {
  const app = new QuiApp(40, 20);
  app.insertButton({ id: "demo:ok", label: "OK" });
  app.pushEvent({
    MouseDown: {
      button: "Left",
      position: { x: 1.0, y: 1.0 },
      modifiers: {},
    },
  });
  assert.equal(app.step(), true);
});

test("pushEvent with a bad shape raises InvalidDescriptorError", () => {
  const app = new QuiApp(40, 20);
  assert.throws(() => app.pushEvent({ NotARealEvent: {} }), /InvalidDescriptorError/);
});
