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

test("insertExtra with an unknown kind raises UnknownVariantError", () => {
  const app = new QuiApp(40, 20);
  assert.throws(
    () => app.insertExtra("not_a_real_kind", { id: "x" }),
    /UnknownVariantError/,
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
// event the handler receives. `progress`/`spinner`/`image` have no event
// at all, so neither appears here.
const INLINE_LEAF_CASES: Array<{
  name: string;
  tree: unknown;
  events: unknown[];
  expected: Array<null | Record<string, unknown> | [string, unknown]>;
}> = [
  {
    // The leaf's own tree-address id ("leaf:1") deliberately differs
    // from the `Switch` descriptor's embedded id ("sw:embedded") --
    // `paint_leaf` keys the painted hit by the leaf id specifically
    // because a render callback may reuse the same descriptor id across
    // leaves, so both the click-to-focus `FocusChanged` and the
    // `ToggleChanged` it emits must agree on "leaf:1".
    name: "switch",
    tree: { row: [{ id: "leaf:1", switch: { id: "sw:embedded", value: false } }] },
    events: [{ MouseDown: { button: "Left", position: { x: 1.0, y: 1.0 }, modifiers: {} } }],
    expected: [
      { FocusChanged: "leaf:1" },
      ["leaf:1", { ToggleChanged: { id: "leaf:1", value: true } }],
    ],
  },
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
  {
    name: "checkbox",
    tree: { row: [{ id: "cb:1", checkbox: { id: "cb:1", value: false } }] },
    events: [{ MouseDown: { button: "Left", position: { x: 1.0, y: 0.0 }, modifiers: {} } }],
    expected: [
      { FocusChanged: "cb:1" },
      ["cb:1", { ToggleChanged: { id: "cb:1", value: true } }],
    ],
  },
  {
    name: "slider",
    tree: {
      row: [
        { id: "sl:1", slider: { id: "sl:1", value: 0.0, min: 0.0, max: 10.0, step: 1.0 } },
      ],
    },
    events: [
      // x: 0.0 lands focus on the slider without moving it -- the mapped
      // fraction is 0.0, same as the current value -- so the second event
      // (Right) is what actually exercises the change.
      { MouseDown: { button: "Left", position: { x: 0.0, y: 0.0 }, modifiers: {} } },
      { KeyPressed: { key: { Named: "Right" }, modifiers: {}, repeat: false } },
    ],
    expected: [
      { FocusChanged: "sl:1" },
      null,
      ["sl:1", { SliderChanged: { id: "sl:1", value: 1.0 } }],
    ],
  },
  {
    name: "select",
    tree: { row: [{ id: "se:1", select: { id: "se:1", options: ["One", "Two", "Three"] } }] },
    events: [
      { MouseDown: { button: "Left", position: { x: 1.0, y: 0.0 }, modifiers: {} } },
      { KeyPressed: { key: { Named: "Down" }, modifiers: {}, repeat: false } },
    ],
    expected: [
      { FocusChanged: "se:1" },
      // Unlike `radio_group`'s click (unclaimed), a click on a standalone
      // `select` leaf steps the selection itself, so it reaches the
      // handler as `DropdownChanged` too, not `null`.
      ["se:1", { DropdownChanged: { id: "se:1", selected_idx: 1 } }],
      ["se:1", { DropdownChanged: { id: "se:1", selected_idx: 1 } }],
    ],
  },
  {
    name: "password",
    tree: { row: [{ id: "pw:1", password: { id: "pw:1", value: "hi" } }] },
    events: [
      { MouseDown: { button: "Left", position: { x: 1.0, y: 0.0 }, modifiers: {} } },
      { CharTyped: "!" },
    ],
    expected: [
      { FocusChanged: "pw:1" },
      null,
      ["pw:1", { TextInputChanged: { id: "pw:1", value: "hi!" } }],
    ],
  },
  {
    name: "textarea",
    tree: { row: [{ id: "ta:1", textarea: { id: "ta:1", value: "hi" } }] },
    events: [
      { MouseDown: { button: "Left", position: { x: 1.0, y: 0.0 }, modifiers: {} } },
      { KeyPressed: { key: { Named: "Enter" }, modifiers: {}, repeat: false } },
    ],
    expected: [
      { FocusChanged: "ta:1" },
      null,
      // The focusing click lands at column 0 (the default cursor position
      // for a textarea with no explicit `cursor`), so Enter's inserted
      // newline lands before "hi", not after it.
      ["ta:1", { TextInputChanged: { id: "ta:1", value: "\nhi" } }],
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

// The four chrome-bar kinds below each have their own click-routing
// mechanism (`ExtraPaint::paint_chrome`'s `chrome_hits` for
// toolbar/status_bar/activity_bar, `render_overlays`'s `sync_menu_bar` for
// menu_bar) that doesn't fit `INLINE_LEAF_CASES`' generic "one Form event
// per pushed event" shape above, so each gets its own test here instead of
// a table row -- proving the same semantic event a flat-slot chrome bar
// dispatches from an `insertExtra`-based flat slot still reaches the
// handler when the bar is painted as a literal inline
// `{ row: [{ id, <kind>: {...} }] }` leaf.

test("inline tree leaf toolbar click emits Toolbar(ButtonClicked)", () => {
  const app = new QuiApp(40, 20);
  app.setRender(() => ({
    row: [
      {
        id: "toolbar:main",
        toolbar: {
          id: "toolbar:main",
          buttons: [
            {
              Action: {
                id: "toolbar:refresh",
                label: "Refresh",
                icon: null,
                key_hint: null,
                enabled: true,
                is_active: false,
                tooltip: "",
              },
            },
          ],
        },
      },
    ],
  }));
  app.screen();

  const received: any[] = [];
  app.setHandler((event: unknown) => received.push(event));
  app.pushEvent({
    MouseDown: { button: "Left", position: { x: 1.0, y: 0.0 }, modifiers: {} },
  });
  app.run(1);

  assert.equal(received.length, 1, JSON.stringify(received));
  const [barId, toolbarEvent] = received[0].Toolbar;
  assert.equal(barId, "toolbar:main");
  assert.deepStrictEqual(toolbarEvent, {
    ButtonClicked: { id: "toolbar:refresh", is_active: true },
  });
});

test("inline tree leaf status_bar click emits StatusBar(SegmentClicked)", () => {
  const app = new QuiApp(40, 20);
  app.setRender(() => ({
    row: [
      {
        id: "status_bar:main",
        status_bar: {
          id: "status_bar:main",
          left_segments: [
            {
              text: "main",
              fg: { r: 255, g: 255, b: 255, a: 255 },
              bg: { r: 0, g: 0, b: 0, a: 255 },
              action_id: "status:branch",
            },
          ],
          right_segments: [],
        },
      },
    ],
  }));
  app.screen();

  const received: any[] = [];
  app.setHandler((event: unknown) => received.push(event));
  app.pushEvent({
    MouseDown: { button: "Left", position: { x: 1.0, y: 0.0 }, modifiers: {} },
  });
  app.run(1);

  assert.equal(received.length, 1, JSON.stringify(received));
  const [widgetId, statusEvent] = received[0].StatusBar;
  assert.equal(widgetId, "status_bar:main");
  assert.deepStrictEqual(statusEvent, { SegmentClicked: { id: "status:branch" } });
});

test("inline tree leaf activity_bar click emits ActivityBar(ItemClicked)", () => {
  const app = new QuiApp(40, 20);
  app.setRender(() => ({
    row: [
      {
        id: "activity_bar:main",
        activity_bar: {
          id: "activity_bar:main",
          top_items: [
            {
              id: "activity:explorer",
              icon: { glyph: "", fallback: "E" },
              tooltip: "Explorer",
              is_active: true,
            },
          ],
        },
      },
    ],
  }));
  app.screen();

  const received: any[] = [];
  app.setHandler((event: unknown) => received.push(event));
  app.pushEvent({
    MouseDown: { button: "Left", position: { x: 0.0, y: 0.0 }, modifiers: {} },
  });
  app.run(1);

  assert.equal(received.length, 1, JSON.stringify(received));
  const [widgetId, activityEvent] = received[0].ActivityBar;
  assert.equal(widgetId, "activity_bar:main");
  assert.deepStrictEqual(activityEvent, { ItemClicked: { id: "activity:explorer" } });
});

test("inline tree leaf menu_bar Alt activation then Enter emits MenuActivated", () => {
  // `render_overlays`'s `sync_menu_bar` call reads the hit map generically
  // -- it never distinguishes a flat-slot `"menu_bar"` from one painted
  // through an inline leaf -- so `Alt+F` opens the bar's one top-level menu
  // and `Enter` activates its sole leaf item exactly as it would for a
  // slot-painted bar.
  const app = new QuiApp(40, 20);
  app.setRender(() => ({
    row: [
      {
        id: "menu_bar:main",
        menu_bar: {
          id: "menu_bar:main",
          items: [
            {
              id: "menu:file",
              label: "&File",
              submenu: [
                {
                  id: "menu:file:save",
                  label: { spans: [{ text: "Save", fg: null, bg: null }] },
                },
              ],
            },
          ],
        },
      },
    ],
  }));
  app.screen();

  const received: any[] = [];
  app.setHandler((event: unknown) => received.push(event));
  app.pushEvent({
    KeyPressed: { key: { Char: "f" }, modifiers: { alt: true }, repeat: false },
  });
  app.run(1);
  app.pushEvent({
    KeyPressed: { key: { Named: "Enter" }, modifiers: {}, repeat: false },
  });
  app.run(1);

  assert.ok(
    received.some((event) => event.MenuActivated === "menu:file:save"),
    `expected a MenuActivated event, got: ${JSON.stringify(received)}`,
  );
});

test("inline tree leaf markdown renders formatting and link click emits RichTextLinkClicked", () => {
  // `"markdown"` reuses `render_markdown_to_styled` (the same adapter
  // `push_turn_markdown`/`RichTextPopup` content already goes through),
  // not a second parser -- asserted here by checking the raw syntax is
  // gone from the painted screen (a renderer bypass that printed `source`
  // verbatim would still pass a plain `screenContains` check on each
  // substring, since every one of them is also present in the raw text).
  // The link click resolves through the same `ExtraChrome` chrome-hit
  // mechanism toolbar/status_bar/activity_bar above use, into
  // `RichTextLinkClicked` -- the event `RichTextPopup`'s own link click
  // already dispatches -- so this gets its own test rather than a row in
  // `INLINE_LEAF_CASES`, matching those three.
  const app = new QuiApp(40, 20);
  app.setRender(() => ({
    row: [
      {
        id: "md:1",
        markdown: {
          id: "md:1",
          source: "# Title\n**bold** *italic* `code`\n- item\n[docs](https://example.com/docs)",
        },
      },
    ],
  }));
  app.screen();

  assert.ok(app.screenContains("Title"));
  assert.ok(app.screenContains("bold"));
  assert.ok(app.screenContains("italic"));
  assert.ok(app.screenContains("code"));
  assert.ok(app.screenContains("item"));
  assert.ok(app.screenContains("docs"));
  assert.ok(!app.screenContains("# Title"));
  assert.ok(!app.screenContains("**bold**"));
  assert.ok(!app.screenContains("*italic*"));
  assert.ok(!app.screenContains("`code`"));
  assert.ok(!app.screenContains("[docs]"));
  assert.ok(app.screenContains("•"));

  // `QuiApp` has no `find` helper, so the link's cell position is located
  // the same way `callbacks.test.ts` locates a column: scan `screen()`'s
  // own rows/columns directly.
  const lines = app.screen().split("\n");
  const y = lines.findIndex((line: string) => line.includes("docs"));
  const x = lines[y].indexOf("docs");
  const received: any[] = [];
  app.setHandler((event: unknown) => received.push(event));
  app.pushEvent({ MouseDown: { button: "Left", position: { x, y }, modifiers: {} } });
  app.run(1);

  assert.equal(received.length, 1, JSON.stringify(received));
  const [widgetId, url] = received[0].RichTextLinkClicked;
  assert.equal(widgetId, "md:1");
  assert.equal(url, "https://example.com/docs");
});

test("inline tree leaf find_replace renders its query text", () => {
  // `FindReplacePanel` carries no `id` field of its own (see
  // `ExtraPaint::id`'s fixed-sentinel doc), so this is its first coverage
  // in this file, via an inline leaf. `hit_regions` drives both click
  // routing AND which content the TUI rasteriser paints
  // (`tui::draw_find_replace` walks it to place the find/replace input
  // text) -- a real host builds it via `compute_find_replace_hit_regions`;
  // this test supplies just the one entry it needs directly. An inline
  // leaf gets the full 40-column viewport rather than a narrower
  // `insertExtra` band, so a `width` of 20 keeps the region inside it.
  const app = new QuiApp(40, 20);
  app.setRender(() => ({
    row: [
      {
        id: "fr:1",
        find_replace: {
          query: "needle",
          group_bounds: { x: 0.0, y: 0.0, width: 40.0, height: 20.0 },
          hit_regions: [[{ col: 0, row: 0, width: 20 }, { FindInput: 0 }]],
        },
      },
    ],
  }));

  assert.ok(app.screenContains("needle"));
});

test("inline tree leaf command_line renders its text", () => {
  const app = new QuiApp(40, 20);
  app.setRender(() => ({
    row: [
      { id: "command_line:main", command_line: { id: "command_line:main", text: ":wq" } },
    ],
  }));

  assert.ok(app.screenContains(":wq"));
});

test("inline tree leaf command_center renders its search label", () => {
  const app = new QuiApp(40, 20);
  app.setRender(() => ({
    row: [
      {
        id: "command_center:main",
        command_center: {
          id: "command_center:main",
          back_enabled: true,
          forward_enabled: false,
          search_label: "my-project",
        },
      },
    ],
  }));

  assert.ok(app.screenContains("my-project"));
});

test("inline tree leaf palette renders its item text", () => {
  // Palette's TUI rasteriser refuses to paint anything below a 4x4 area,
  // and `insertExtra` only ever gives an additive primitive a fixed 3-row
  // band, so this one routes through a render-callback `row` leaf instead,
  // which hands the leaf the full viewport -- mirrors
  // `test_palette_renders_item_text` in `quadraui-py/tests/test_extra_primitives.py`.
  const app = new QuiApp(40, 20);
  app.setRender(() => ({
    row: [
      {
        id: "palette:1",
        palette: {
          id: "palette:commands",
          title: "Commands",
          query: "",
          items: [{ text: { spans: [{ text: "Save File", fg: null, bg: null }] } }],
          selected_idx: 0,
        },
      },
    ],
  }));

  assert.ok(app.screenContains("Save File"));
});

// The data-view kinds below (`message_list`, `board`, `diff_view`,
// `minimap`, `multi_section_view`, `chart`, `pipeline_view`) are pure
// display -- no `insertExtra` method or dedicated semantic event of their
// own -- so each gets a paint-only inline leaf proof: a literal
// `{ row: [{ id, <kind>: {...} }] }` object, the exact shape a render
// callback returns, decodes through `layout_leaf_from_value`'s generic
// `EXTRA_PRIMITIVE_KINDS` loop and paints through the matching
// `ExtraPaint` impl, same as it already does for the eight chrome-bar
// kinds just above.
test("inline tree leaf message_list renders a row's text", () => {
  const app = new QuiApp(40, 20);
  app.setRender(() => ({
    row: [
      {
        id: "messages:1",
        message_list: {
          id: "messages:1",
          rows: [{ text: "Build succeeded", fg: { r: 0, g: 200, b: 0, a: 255 } }],
        },
      },
    ],
  }));

  assert.ok(app.screenContains("Build succeeded"));
});

test("inline tree leaf board renders a column title", () => {
  const app = new QuiApp(40, 20);
  app.setRender(() => ({
    row: [
      {
        id: "board:1",
        board: {
          id: "board:1",
          columns: [
            {
              id: "col:backlog",
              title: "Backlog",
              cards: [{ id: "card:1", title: "Fix flaky test" }],
            },
          ],
        },
      },
    ],
  }));

  assert.ok(app.screenContains("Backlog"));
});

test("inline tree leaf diff_view renders a unified hunk header", () => {
  const app = new QuiApp(40, 20);
  app.setRender(() => ({
    row: [
      {
        id: "diff:1",
        diff_view: {
          id: "diff:1",
          left: "foo\n",
          right: "foo\nbar\n",
          left_label: null,
          right_label: null,
          hunks: [
            {
              left_start: 1,
              right_start: 1,
              rows: [
                { left: "foo", right: "foo", kind: "Same" },
                { left: null, right: "bar", kind: "Added" },
              ],
            },
          ],
          mode: "Unified",
          editability: "ReadOnly",
          scroll_offset: 0,
          focused_pane: "Left",
          has_focus: false,
        },
      },
    ],
  }));

  assert.ok(app.screenContains("@@"));
});

test("inline tree leaf minimap renders braille cells", () => {
  const app = new QuiApp(40, 20);
  app.setRender(() => ({
    row: [
      {
        id: "minimap:1",
        minimap: {
          id: "minimap:1",
          lines: [{ text: "fn main() {}", line_idx: 0 }],
          syntax_spans: [],
          visible_row_start: 0,
          visible_row_count: 1,
          total_buffer_lines: 1,
        },
      },
    ],
  }));

  assert.ok(app.screenContains("⠀"));
});

test("inline tree leaf multi_section_view renders a section header title", () => {
  const app = new QuiApp(40, 20);
  app.setRender(() => ({
    row: [
      {
        id: "sections:1",
        multi_section_view: {
          id: "sections:1",
          sections: [
            {
              id: "section:explorer",
              header: {
                icon: null,
                title: { spans: [{ text: "Explorer", fg: null, bg: null }] },
                badge: null,
              },
              body: { Text: [] },
              size: "EqualShare",
            },
          ],
          axis: "Vertical",
          scroll_mode: "PerSection",
        },
      },
    ],
  }));

  assert.ok(app.screenContains("Explorer"));
});

test("inline tree leaf pipeline_view renders stage labels", () => {
  const app = new QuiApp(40, 20);
  app.setRender(() => ({
    row: [
      {
        id: "pl:1",
        pipeline_view: {
          id: "pipeline:1",
          stages: [
            { label: "Build", status: "Done" },
            { label: "Deploy", status: "Pending" },
          ],
        },
      },
    ],
  }));

  assert.ok(app.screenContains("Build"));
  assert.ok(app.screenContains("Deploy"));
});

test("inline tree leaf chart renders its x_label", () => {
  const app = new QuiApp(40, 20);
  app.setRender(() => ({
    row: [
      {
        id: "c:1",
        chart: {
          id: "chart:cpu",
          kind: "Line",
          series: [{ label: "cpu", data: [1.0, 2.0, 3.0] }],
          x_label: "Time",
        },
      },
    ],
  }));

  assert.ok(app.screenContains("Time"));
});

// `editor`/`terminal`/`canvas` are each a tab stop (the first two) or a
// bare hit target (`canvas`, which `ExtraPaint::tab_stop` leaves
// non-focusable) with no dedicated semantic event of their own -- a click
// reaches the handler as the raw `MouseDown` with `widget` resolved to the
// leaf's id, same as `insertExtra`'s flat-slot path.
test("inline tree leaf editor focuses it and routes the hit", () => {
  const app = new QuiApp(40, 20);
  app.setRender(() => ({
    row: [
      {
        id: "editor:main",
        editor: {
          id: "editor:main",
          rect: { x: 0.0, y: 0.0, width: 40.0, height: 10.0 },
          lines: [{ raw_text: "fn main() {}", gutter_text: "1", spans: [], line_idx: 0 }],
          scroll_top: 0,
          total_lines: 1,
          is_active: true,
          show_active_bg: false,
          has_git_diff: false,
          has_breakpoints: false,
          tabstop: 4,
          cursorline: false,
        },
      },
    ],
  }));
  app.screen();

  const received: any[] = [];
  app.setHandler((event: unknown) => received.push(event));
  app.pushEvent({
    MouseDown: { button: "Left", position: { x: 1.0, y: 1.0 }, modifiers: {} },
  });
  app.run(1);

  assert.equal(received.length, 2, JSON.stringify(received));
  assert.equal(received[0].FocusChanged, "editor:main");
  assert.equal(received[1].MouseDown.widget, "editor:main");
});

test("inline tree leaf terminal focuses it and routes the hit", () => {
  const app = new QuiApp(40, 20);
  app.setRender(() => ({
    row: [
      {
        id: "terminal:shell",
        terminal: {
          id: "terminal:shell",
          cells: [[{ text: "$", fg: { r: 200, g: 200, b: 200, a: 255 }, bg: { r: 0, g: 0, b: 0, a: 255 } }]],
        },
      },
    ],
  }));
  app.screen();

  const received: any[] = [];
  app.setHandler((event: unknown) => received.push(event));
  app.pushEvent({
    MouseDown: { button: "Left", position: { x: 1.0, y: 1.0 }, modifiers: {} },
  });
  app.run(1);

  assert.equal(received.length, 2, JSON.stringify(received));
  assert.equal(received[0].FocusChanged, "terminal:shell");
  assert.equal(received[1].MouseDown.widget, "terminal:shell");
});

test("inline tree leaf canvas routes the hit without focusing", () => {
  const app = new QuiApp(40, 20);
  app.setRender(() => ({
    row: [
      {
        id: "canvas:sparkline",
        canvas: {
          id: "canvas:sparkline",
          ops: [
            {
              Rect: {
                rect: { x: 0.0, y: 0.0, width: 10.0, height: 1.0 },
                color: { r: 0, g: 200, b: 0, a: 255 },
              },
            },
          ],
        },
      },
    ],
  }));
  app.screen();

  const received: any[] = [];
  app.setHandler((event: unknown) => received.push(event));
  app.pushEvent({
    MouseDown: { button: "Left", position: { x: 1.0, y: 1.0 }, modifiers: {} },
  });
  app.run(1);

  assert.equal(received.length, 1, JSON.stringify(received));
  assert.equal(received[0].MouseDown.widget, "canvas:sparkline");
});

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

test("uncontrolled form Tab then type focuses and types the first field", () => {
  // Acceptance: an app that never sets `focused_field` (the key is
  // omitted every frame) still gets Tab-driven field focus from the
  // host itself, and typing afterward lands on the right field.
  const app = new QuiApp(40, 20);
  const state = { name: "" };

  app.setRender(() => ({
    form: {
      id: "settings",
      fields: [
        {
          id: "name",
          label: { spans: [{ text: "Name", fg: null, bg: null }] },
          kind: { TextInput: { value: state.name } },
        },
      ],
    },
  }));
  app.screen();

  let received: any[] = [];
  app.setHandler((event: any) => {
    received.push(event);
    if (!event.Form) return;
    const [, formEvent] = event.Form;
    if (formEvent.TextInputChanged) {
      state.name = formEvent.TextInputChanged.value;
    }
  });

  // First Tab: nothing is focused anywhere yet, so the generic
  // app-level cycle (the form is its only stop) lands engine focus on
  // the form as a whole.
  app.pushEvent({ KeyPressed: { key: { Named: "Tab" }, modifiers: {}, repeat: false } });
  app.run(1);
  assert.equal(received[received.length - 1].FocusChanged, "settings");

  // Second Tab: the host's own field cycling takes over from here,
  // landing on "name".
  received = [];
  app.pushEvent({ KeyPressed: { key: { Named: "Tab" }, modifiers: {}, repeat: false } });
  app.run(1);
  let [widgetId, formEvent] = received[received.length - 1].Form;
  assert.equal(widgetId, "settings");
  assert.deepStrictEqual(formEvent, { FocusChanged: { id: "name" } });

  // Typing now lands on "name" even though the app never echoed
  // `focused_field` back at all -- `None` is no longer a dead form.
  received = [];
  app.pushEvent({ CharTyped: "x" });
  app.run(1);
  [widgetId, formEvent] = received[received.length - 1].Form;
  assert.equal(widgetId, "settings");
  assert.deepStrictEqual(formEvent, { TextInputChanged: { id: "name", value: "x" } });
  assert.equal(state.name, "x");
});

test("uncontrolled form click focuses and types with no focused_field tracking", () => {
  // Same "`None` is no longer a dead form" property as the Tab test
  // above, via a click instead -- the handler never reads or writes
  // `focused_field` at all, and typing right after a click still lands
  // on the clicked field.
  const app = new QuiApp(40, 20);
  const state = { name: "" };

  app.setRender(() => ({
    form: {
      id: "settings",
      fields: [
        {
          id: "name",
          label: { spans: [{ text: "Name", fg: null, bg: null }] },
          kind: { TextInput: { value: state.name } },
        },
      ],
    },
  }));
  app.screen();

  app.setHandler((event: any) => {
    if (!event.Form) return;
    const [, formEvent] = event.Form;
    if (formEvent.TextInputChanged) {
      state.name = formEvent.TextInputChanged.value;
    }
  });
  app.pushEvent({
    MouseDown: { button: "Left", position: { x: 6.0, y: 0.0 }, modifiers: {} },
  });
  app.pushEvent({ CharTyped: "z" });
  app.run(2);

  assert.equal(state.name, "z");
});

test("controlled form focused_field keeps app-set focus despite a click", () => {
  // An app that sets `focused_field` explicitly stays controlled: a
  // click that would otherwise move engine-owned focus to a different
  // field must not change which row the next render paints as focused.
  const app = new QuiApp(40, 20);

  app.setRender(() => ({
    form: {
      id: "settings",
      focused_field: "a",
      fields: [
        {
          id: "a",
          label: { spans: [{ text: "A", fg: null, bg: null }] },
          kind: { TextInput: { value: "" } },
        },
        {
          id: "b",
          label: { spans: [{ text: "B", fg: null, bg: null }] },
          kind: { TextInput: { value: "" } },
        },
      ],
    },
  }));
  app.setHandler(() => {});
  app.screen();

  // Clicking field "b" (row 1) still moves engine focus onto the form
  // and reports a click internally -- the app's own descriptor just
  // keeps pinning "a" every frame, so the highlight stays put.
  app.pushEvent({
    MouseDown: { button: "Left", position: { x: 6.0, y: 1.0 }, modifiers: {} },
  });
  app.run(1);

  assert.deepEqual(app.cellColors(0, 0)[1], [50, 60, 90]);
  assert.deepEqual(app.cellColors(0, 1)[1], [20, 22, 30]);
});
