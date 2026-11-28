// Black-box coverage for the descriptor decode adapter (`quadraui-node/src/
// lib.rs`'s "Descriptor decode adapter" module doc section): render each of
// the six fixed primitive slots and the three built-in additive primitives
// from a plain JS object through a render callback's flat `{"<kind>": {...}}`
// object, assert `screenContains()` shows it, and exercise the two
// audit-flagged pre-checks (`Buffer`-sourced `ImageSource::Bytes`, a
// non-finite float) via that same path. Ports the shape of `quadraui-ffi`'s
// `bad_json_reports_last_error_for_each_new_primitive` and
// `quadraui-py/tests/test_descriptors.py` + `test_extra_primitives.py`.
//
// Run against a built addon, e.g.:
//
//     npm run build:debug
//     npm test
const { test } = require("node:test");
const assert = require("node:assert/strict");

const { DecodeFailedError, InvalidDescriptorError, QuiApp } = require("../js/index.ts");

test("button leaf renders its label", () => {
  const app = new QuiApp(40, 20);
  app.setRender(() => ({ button: { id: "demo:ok", label: "OK" } }));
  assert.ok(app.screenContains("OK"));
});

test("button leaf accepts an optional tooltip", () => {
  const app = new QuiApp(40, 20);
  app.setRender(() => ({
    button: { id: "demo:ok", label: "OK", tooltip: "Confirm the action" },
  }));
  assert.ok(app.screenContains("OK"));
});

test("button leaf rejects a non-string tooltip", () => {
  // `Button::tooltip` is typed `Option<String>`, not merely ignored the
  // way an unrecognized key would be -- a non-string value must fail to
  // decode, proving the field is actually wired into `Button`'s own
  // `Deserialize` rather than silently dropped.
  const app = new QuiApp(40, 20);
  app.setRender(() => ({ button: { id: "demo:ok", label: "OK", tooltip: 123 } }));
  // The generic render-callback decode path wraps a serde-level type
  // mismatch as `DecodeFailedError`, not a plain `InvalidDescriptorError`
  // -- still a match for `InvalidDescriptorError` callers via the
  // subclass relationship, same as the missing-required-field case below.
  assert.throws(() => app.screen(), (err: unknown) => err instanceof InvalidDescriptorError);
});

test("text display leaf renders its line text", () => {
  const app = new QuiApp(40, 20);
  app.setRender(() => ({
    text_display: {
      id: "text:1",
      lines: [{ spans: [{ text: "Hello", fg: null, bg: null }] }],
    },
  }));
  assert.ok(app.screenContains("Hello"));
});

test("list leaf renders an item's text", () => {
  const app = new QuiApp(40, 20);
  app.setRender(() => ({
    list: {
      id: "list:1",
      items: [{ text: { spans: [{ text: "Row 1", fg: null, bg: null }] } }],
      selected_idx: 0,
    },
  }));
  assert.ok(app.screenContains("Row 1"));
});

test("tree leaf renders a row's text", () => {
  const app = new QuiApp(40, 20);
  app.setRender(() => ({
    tree: {
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
    },
  }));
  assert.ok(app.screenContains("Alpha"));
});

test("tree leaf accepts a codicon icon and renders its fallback text", () => {
  // A tree row's `icon` accepts `{codicon: "...", fallback: "..."}` in
  // place of `{glyph: "...", fallback: "..."}` -- the wire shape `Icon`'s
  // `Deserialize` impl resolves through the bundled codicon name table.
  // The headless driver defaults to nerd fonts off, so the rendered row
  // shows the icon's plain-text fallback.
  const app = new QuiApp(40, 20);
  app.setRender(() => ({
    tree: {
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
    },
  }));
  assert.ok(app.screenContains("DIR"));
  assert.ok(app.screenContains("src"));
});

test("tree leaf with an unknown codicon name raises InvalidDescriptorError", () => {
  const app = new QuiApp(40, 20);
  app.setRender(() => ({
    tree: {
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
    },
  }));
  // See "button leaf rejects a non-string tooltip"'s own comment for why
  // this checks the subclass relationship rather than a literal
  // `InvalidDescriptorError` substring match.
  assert.throws(() => app.screen(), (err: unknown) => err instanceof InvalidDescriptorError);
});

test("data table leaf renders a cell's text", () => {
  const app = new QuiApp(40, 20);
  app.setRender(() => ({
    data_table: {
      id: "table:1",
      columns: [{ title: "Name" }],
      rows: [{ cells: [{ spans: [{ text: "Row1", fg: null, bg: null }] }] }],
    },
  }));
  assert.ok(app.screenContains("Row1"));
});

test("tab bar leaf renders a tab's label", () => {
  const app = new QuiApp(40, 20);
  app.setRender(() => ({ tab_bar: { id: "tabbar:1", tabs: [{ label: "Tab 1" }] } }));
  assert.ok(app.screenContains("Tab 1"));
});

test("button leaf with a bad shape raises InvalidDescriptorError", () => {
  const app = new QuiApp(40, 20);
  app.setRender(() => ({ button: { not: "a button" } }));
  // See "button leaf rejects a non-string tooltip"'s own comment for why
  // this checks the subclass relationship rather than a literal
  // `InvalidDescriptorError` substring match.
  assert.throws(() => app.screen(), (err: unknown) => err instanceof InvalidDescriptorError);
});

test("progress leaf renders its label", () => {
  const app = new QuiApp(40, 20);
  app.setRender(() => ({
    progress: { id: "progress:download", label: "Downloading", value: 0.5 },
  }));
  assert.ok(app.screenContains("Downloading"));
});

test("spinner leaf renders its label", () => {
  const app = new QuiApp(40, 20);
  app.setRender(() => ({ spinner: { id: "spinner:lsp_boot", label: "Indexing" } }));
  assert.ok(app.screenContains("Indexing"));
});

test("image leaf with a Path source renders its fallback text", () => {
  const app = new QuiApp(40, 20);
  app.setRender(() => ({
    image: {
      id: "image:logo",
      source: { Path: "assets/logo.png" },
      intrinsic_size: null,
      fit: "Contain",
      fallback_text: "[LOGO]",
    },
  }));
  assert.ok(app.screenContains("[LOGO]"));
});

test("image leaf with a Buffer-sourced Bytes image renders its fallback text", () => {
  // The audit-flagged pre-check: a `Buffer` must fold into the same
  // array-of-numbers shape `quadraui::ImageSource::Bytes(Vec<u8>)` decodes
  // from, not the generic-object branch a bare `Array.isArray` check would
  // otherwise route it through (see `to_descriptor_value`'s doc).
  const app = new QuiApp(40, 20);
  app.setRender(() => ({
    image: {
      id: "image:logo",
      source: { Bytes: Buffer.from([137, 80, 78, 71]) },
      intrinsic_size: null,
      fit: "Contain",
      fallback_text: "[PNG]",
    },
  }));
  assert.ok(app.screenContains("[PNG]"));
});

test("image leaf with a Uint8Array-sourced Bytes image renders its fallback text", () => {
  const app = new QuiApp(40, 20);
  app.setRender(() => ({
    image: {
      id: "image:logo",
      source: { Bytes: new Uint8Array([137, 80, 78, 71]) },
      intrinsic_size: null,
      fit: "Contain",
      fallback_text: "[PNG2]",
    },
  }));
  assert.ok(app.screenContains("[PNG2]"));
});

test("image leaf with a missing required field raises InvalidDescriptorError", () => {
  const app = new QuiApp(40, 20);
  // A serde-level missing field surfaces as `DecodeFailedError`, which is
  // still an `InvalidDescriptorError` for callers catching the base class.
  app.setRender(() => ({ image: { id: "image:logo" } }));
  assert.throws(
    () => app.screen(),
    (err: unknown) =>
      err instanceof DecodeFailedError &&
      err instanceof InvalidDescriptorError &&
      /missing field `source`/.test((err as Error).message),
  );
});

test("progress leaf rejects a non-finite float with InvalidDescriptorError", () => {
  // The other audit-flagged pre-check: NaN/Infinity must raise rather than
  // silently alias `null` (which `obj_get` treats as "field absent").
  const app = new QuiApp(40, 20);
  app.setRender(() => ({
    progress: { id: "progress:download", label: "Downloading", value: NaN },
  }));
  assert.throws(() => app.screen(), /InvalidDescriptorError/);

  app.setRender(() => ({
    progress: { id: "progress:download2", label: "Downloading", value: Infinity },
  }));
  assert.throws(() => app.screen(), /InvalidDescriptorError/);
});

test("form leaf with control_width and min_control_width renders without raising", () => {
  // `Form`/`FormField`'s `control_width`/`min_control_width` are plain
  // additive `#[serde(default)]` fields, so they decode through the same
  // generic render-callback `{ form: {...} }` path as every other `Form`
  // descriptor -- no dedicated node binding code needed for this feature.
  // The narrow `Fixed(4.0)` control is too small for "exactwidth" (10
  // chars), so without `min_control_width` it would show truncated; the
  // 14-wide floor on the second field keeps its value intact.
  const app = new QuiApp(40, 20);
  app.setRender(() => ({
    form: {
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
    },
  }));
  assert.ok(app.screenContains("exactwidth"));
});

test("text field leaf renders its value inside the bracket row", () => {
  const app = new QuiApp(40, 20);
  app.setRender(() => ({ text_field: { id: "tf:1", value: "hello" } }));
  assert.ok(app.screenContains("[hello"));
});

test("extra primitives coexist with the primitive leaves in one render object", () => {
  const app = new QuiApp(40, 20);
  app.setRender(() => ({
    button: { id: "demo:ok", label: "OK" },
    progress: { id: "progress:download", label: "Downloading" },
  }));
  assert.ok(app.screenContains("OK"));
  assert.ok(app.screenContains("Downloading"));
});

// Table proving every interactive `EXTRA_PRIMITIVE_KINDS` entry that has a
// dedicated `FormEvent` round-trips through a literal inline layout-tree
// leaf -- a `{ row: [{ id, <kind>: {...} }] }` object, the exact shape a
// render callback returns. Each case dispatches the events a real app
// would push (a focusing click, then a key/char where the kind needs one)
// and asserts the final event the handler receives. `progress`/`spinner`/
// `image` have no event at all, so neither appears here. `switch` and
// `radio_group` ride the generic `WIDGET_KINDS` seam rather than
// `ExtraPaint`, and emit a bare top-level event instead of a `Form`-wrapped
// one, but are kept here too for parity coverage of the same literal-leaf
// shape.
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
    // `SwitchToggled` it emits must agree on "leaf:1". `switch` rides the
    // generic `WIDGET_KINDS` seam, not `ExtraPaint`, so its click resolves
    // to a bare `SwitchToggled` event rather than a `Form`-wrapped one.
    name: "switch",
    tree: { row: [{ id: "leaf:1", switch: { id: "sw:embedded", value: false } }] },
    events: [{ MouseDown: { button: "Left", position: { x: 1.0, y: 1.0 }, modifiers: {} } }],
    expected: [
      { FocusChanged: "leaf:1" },
      { SwitchToggled: { widget: "leaf:1", value: true } },
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
      ["ni:1", { NumberChanged: { id: "ni:1", value: 43 } }],
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
      // `RadioGroup::select` resolves a click straight to the clicked
      // option -- no `Form` wrapper, since `RadioGroup` is a
      // `WIDGET_KINDS` member, not an `EXTRA_PRIMITIVE_KINDS` one.
      { RadioGroupSelectionChanged: { widget: "rg:1", index: 1 } },
      // The arrow key routes through `InteractionEngine`'s own
      // dyn-widget-key dispatch, which attaches a `hit` dict even to a
      // non-`MouseDown` event.
      {
        RadioGroupSelectionChanged: { widget: "rg:1", index: 0 },
        hit: { kind: "widget_body" },
      },
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
      // The click lands on the grid's first visible cell, so the picker
      // reports the focus move before Enter confirms a day.
      [
        "dp:1",
        { DateFocusMoved: { id: "dp:1", value: { year: 2024, month: 2, day: 25 } } },
      ],
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
      // Typing filters the options, so the box highlights the first match
      // before reporting the new text.
      ["cb:1", { ComboBoxHighlightChanged: { id: "cb:1", index: 0 } }],
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
      // Unlike `slider`'s focusing click above, which only focuses, a
      // click on a standalone `select` leaf steps the selection itself,
      // so it reaches the handler as `DropdownChanged` too, not `null`.
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
      // A textarea with no explicit `cursor` edits from the end of its
      // value, so Enter's inserted newline lands after "hi".
      ["ta:1", { TextInputChanged: { id: "ta:1", value: "hi\n" } }],
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

// A focused `text_input` edits through the core text state: Ctrl+Backspace
// and Ctrl+Delete delete a word, Ctrl+Left jumps one, Home/End reach the
// line's ends, and Ctrl+Z/Ctrl+Shift+Z undo and redo -- with the caret the
// input's own state walked to, never one the app echoes back.
test("text_input word, line-end and undo keys edit through the core state", () => {
  const app = new QuiApp(40, 20);
  const state = { value: "foo bar" };
  app.setRender(() => ({ text_input: { id: "ti:1", lines: [state.value] } }));
  app.screen();
  app.setHandler((event: any) => {
    const changed = event.Form?.[1]?.TextInputChanged;
    if (changed) state.value = changed.value;
  });
  // Content starts two cells in (border + padding): x = 9 is column 7, the
  // end of "foo bar".
  app.pushEvent({ MouseDown: { button: "Left", position: { x: 9.0, y: 1.0 }, modifiers: {} } });
  app.run(1);
  const key = (named: string, modifiers: Record<string, boolean> = {}) => {
    app.pushEvent({ KeyPressed: { key: { Named: named }, modifiers, repeat: false } });
    app.run(1);
  };
  key("Backspace", { ctrl: true });
  assert.equal(state.value, "foo ");
  key("Home");
  app.pushEvent({ CharTyped: "X" });
  app.run(1);
  assert.equal(state.value, "Xfoo ");
  key("End");
  key("Left", { ctrl: true });
  key("Delete", { ctrl: true });
  assert.equal(state.value, " ");
  app.pushEvent({ KeyPressed: { key: { Char: "z" }, modifiers: { ctrl: true }, repeat: false } });
  app.run(1);
  assert.equal(state.value, "Xfoo ");
  app.pushEvent({
    KeyPressed: { key: { Char: "z" }, modifiers: { ctrl: true, shift: true }, repeat: false },
  });
  app.run(1);
  assert.equal(state.value, " ");
});

// The four chrome-bar kinds below each report events that don't fit
// `INLINE_LEAF_CASES`' generic "one Form event per pushed event" shape
// above -- toolbar/status_bar/activity_bar their own click events, and
// menu_bar the application menu's view changes plus `MenuActivated` -- so
// each gets its own test here instead of a table row, proving the same
// semantic event a flat-slot chrome bar dispatches from a render
// callback's flat `{"<kind>": {...}}` object still reaches the handler
// when the bar is painted as a literal inline
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

  assert.equal(received.length, 2, JSON.stringify(received));
  assert.deepStrictEqual(received[0], { FocusChanged: "toolbar:main" });
  const [barId, toolbarEvent] = received[1].Toolbar;
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
  // A status-bar segment activates on release over the segment it was
  // pressed on, like every desktop button.
  app.pushEvent({
    MouseDown: { button: "Left", position: { x: 1.0, y: 0.0 }, modifiers: {} },
  });
  app.run(1);
  app.pushEvent({
    MouseUp: { widget: null, button: "Left", position: { x: 1.0, y: 0.0 } },
  });
  app.run(1);

  const statusEvents = received.filter((e) => e.StatusBar !== undefined);
  assert.equal(statusEvents.length, 1, JSON.stringify(received));
  const [widgetId, statusEvent] = statusEvents[0].StatusBar;
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

  // The rail is a focus stop, so the click may also report focus moving
  // onto it; only the activation is checked here.
  const activityEvents = received.filter((e) => e.ActivityBar !== undefined);
  assert.equal(activityEvents.length, 1, JSON.stringify(received));
  const [widgetId, activityEvent] = activityEvents[0].ActivityBar;
  assert.equal(widgetId, "activity_bar:main");
  assert.deepStrictEqual(activityEvent, { ItemClicked: { id: "activity:explorer" } });
});

test("inline tree leaf menu_bar Alt activation then Enter emits MenuActivated", () => {
  // The `"menu_bar"` leaf is the application menu, routed like any other
  // leaf: once it holds focus (Tab, its only stop), `Alt+F` opens the
  // bar's one top-level menu and `Enter` activates its sole leaf item.
  // Which menu is open is the app's to keep: it echoes every
  // `AppMenu(..., { Changed: view })` back onto the descriptor.
  const app = new QuiApp(40, 20);
  let view: Record<string, unknown> = {};
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
          ...view,
        },
      },
    ],
  }));
  app.screen();

  const received: any[] = [];
  app.setHandler((event: any) => {
    received.push(event);
    if (event.AppMenu) {
      view = event.AppMenu[1].Changed;
    }
  });
  for (const [key, modifiers] of [
    [{ Named: "Tab" }, {}],
    [{ Char: "f" }, { alt: true }],
    [{ Named: "Enter" }, {}],
  ] as const) {
    app.pushEvent({ KeyPressed: { key, modifiers, repeat: false } });
    app.run(1);
  }

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
  // (the cell painter walks it to place the find/replace input
  // text) -- a real host builds it via `compute_find_replace_hit_regions`;
  // this test supplies just the one entry it needs directly. An inline
  // leaf gets the full 40-column viewport rather than a narrower flat-slot
  // band, so a `width` of 20 keeps the region inside it.
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
  // and a render callback's flat `{"<kind>": {...}}` object only ever
  // gives an additive primitive a fixed 3-row band, so this one routes
  // through a render-callback `row` leaf instead,
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
// display -- no dedicated semantic event of their own -- so each gets a
// paint-only inline leaf proof: a literal
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

test("inline tree leaf icon_grid renders an item's label", () => {
  const app = new QuiApp(40, 20);
  app.setRender(() => ({
    row: [
      {
        id: "icons:1",
        icon_grid: {
          id: "icons:1",
          items: [
            {
              id: "file:readme",
              icon: { glyph: "", fallback: "F" },
              label: { spans: [{ text: "README.md", fg: null, bg: null }] },
            },
          ],
          selected_idx: 0,
          has_focus: true,
        },
      },
    ],
  }));

  assert.ok(app.screenContains("README.md"));
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

// `editor`/`canvas` are a tab stop and a bare hit target (`canvas`, which
// `ExtraPaint::tab_stop` leaves non-focusable) with no dedicated semantic
// event of their own -- a click reaches the handler as the raw `MouseDown`
// with `widget` resolved to the leaf's id, same as a render callback's
// flat-slot path. `terminal` is a widget-contract kind whose own
// `Interactive` impl turns the click into a selection start.
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
  assert.equal(received[1].Editor[0], "editor:main");
  assert.ok("Pointer" in received[1].Editor[1], JSON.stringify(received));
});

test("inline tree leaf terminal focuses it and starts a selection", () => {
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
  assert.equal(received[1].Terminal[0], "terminal:shell");
  assert.ok("SelectStart" in received[1].Terminal[1], JSON.stringify(received));
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

function canvasWithRegionRender() {
  // A single `canvas` leaf with one declared region covering the whole
  // first cell row -- built from a plain object, exercising
  // `Canvas::regions`'/`CanvasRegion`'s own serde-default decode path the
  // same way every other inline leaf above is built.
  return {
    row: [
      {
        id: "canvas:regions",
        canvas: {
          id: "canvas:regions",
          ops: [],
          regions: [
            {
              id: "region:a",
              rect: { x: 0.0, y: 0.0, width: 10.0, height: 1.0 },
            },
          ],
          focusable: false,
        },
      },
    ],
  };
}

test("canvas region click emits a typed RegionClicked event", () => {
  const app = new QuiApp(40, 20);
  app.setRender(canvasWithRegionRender);
  app.screen();

  const received: any[] = [];
  app.setHandler((event: unknown) => received.push(event));
  app.pushEvent({
    MouseDown: { button: "Left", position: { x: 1.0, y: 0.0 }, modifiers: {} },
  });
  app.run(1);

  assert.equal(received.length, 1, JSON.stringify(received));
  const [widgetId, canvasEvent] = received[0].Canvas;
  assert.equal(widgetId, "canvas:regions");
  assert.deepEqual(canvasEvent, {
    RegionClicked: {
      region: "region:a",
      button: "Left",
      modifiers: { shift: false, ctrl: false, alt: false, cmd: false },
      x: 1.0,
      y: 0.0,
    },
  });
});

test("canvas region double-click emits a typed RegionDoubleClicked event", () => {
  const app = new QuiApp(40, 20);
  app.setRender(canvasWithRegionRender);
  app.screen();

  const received: any[] = [];
  app.setHandler((event: unknown) => received.push(event));
  app.pushEvent({ DoubleClick: { position: { x: 1.0, y: 0.0 } } });
  app.run(1);

  assert.equal(received.length, 1, JSON.stringify(received));
  const [widgetId, canvasEvent] = received[0].Canvas;
  assert.equal(widgetId, "canvas:regions");
  assert.deepEqual(canvasEvent, {
    RegionDoubleClicked: { region: "region:a", x: 1.0, y: 0.0 },
  });
});

test("canvas region hover emits RegionHovered alongside MouseEntered/MouseLeft", () => {
  // Entering the canvas reports the generic `MouseEntered` for its body,
  // and the canvas's own `Interactive` reports the region it entered on
  // (or `None` on leaving) as `Canvas`/`RegionHovered`.
  const app = new QuiApp(40, 20);
  app.setRender(canvasWithRegionRender);
  app.screen();

  const received: any[] = [];
  app.setHandler((event: unknown) => received.push(event));

  app.pushEvent({ MouseMoved: { position: { x: 1.0, y: 0.0 }, buttons: {} } });
  app.run(1);
  assert.equal(received.length, 3, JSON.stringify(received));
  assert.deepEqual(received[0], {
    MouseEntered: { widget: "canvas:regions" },
    hit: { kind: "widget_body" },
  });
  const [widgetId, canvasEvent] = received[1].Canvas;
  assert.equal(widgetId, "canvas:regions");
  assert.deepEqual(canvasEvent, { RegionHovered: { region: "region:a" } });

  received.length = 0;
  // (30, 10) is still inside the canvas but outside the declared region.
  // The canvas opts into pointer moves while it is hovered, so a move that
  // stays on the canvas but leaves the region reports the region change
  // (`null`) ahead of the move itself.
  app.pushEvent({ MouseMoved: { position: { x: 30.0, y: 10.0 }, buttons: {} } });
  app.run(1);
  assert.equal(received.length, 2, JSON.stringify(received));
  const [movedWidgetId, movedCanvasEvent] = received[0].Canvas;
  assert.equal(movedWidgetId, "canvas:regions");
  assert.deepEqual(movedCanvasEvent, { RegionHovered: { region: null } });
  assert.ok("MouseMoved" in received[1], JSON.stringify(received));

  received.length = 0;
  // Leaving the canvas reports `MouseLeft`; the hovered region is already
  // `null`, so no further `RegionHovered` follows.
  app.pushEvent({ MouseMoved: { position: { x: 60.0, y: 30.0 }, buttons: {} } });
  app.run(1);
  assert.deepEqual(received[0], {
    MouseLeft: { widget: "canvas:regions" },
    hit: { kind: "widget_body" },
  });
  assert.ok(!received.some((event) => "Canvas" in event), JSON.stringify(received));
});

test("pushEvent queues an event step consumes without raising", () => {
  const app = new QuiApp(40, 20);
  app.setRender(() => ({ button: { id: "demo:ok", label: "OK" } }));
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
