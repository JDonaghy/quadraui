// Black-box coverage for `Toolbar` click routing: a click on an `Action`
// button must be replaced by the semantic `UiEvent::Toolbar` it stands for
// (the clicked button's own id, plus `is_active` flipped from its starting
// state) rather than reaching the handler as a raw `MouseDown`. Ports
// `quadraui-py/src/lib.rs`'s `mouse_down_over_a_toolbar_button_emits_a_
// semantic_toolbar_event` test.
//
// Run against a built addon, e.g.:
//
//     npm run build:debug
//     npm test
const { test } = require("node:test");
const assert = require("node:assert/strict");

const { QuiApp } = require("../js/index.ts");

// The toolbar is the only leaf this render callback returns, so
// `draw_bands` lays its band at the very top of the viewport (row 0) --
// the single button starts at its own x=0, and the TUI rasteriser's
// `[ Filter ]` bracket puts the clickable body one cell in, at x=1.
function insertFilterToolbar(app: any) {
  app.setRender(() => ({
    toolbar: {
      id: "toolbar:main",
      buttons: [
        {
          Action: {
            id: "filter",
            label: "Filter",
            icon: null,
            key_hint: null,
            enabled: true,
            is_active: false,
            tooltip: "",
          },
        },
      ],
    },
  }));
}

test("click on a toolbar action button emits Toolbar(ButtonClicked) with the flipped state", () => {
  const app = new QuiApp(40, 5);
  insertFilterToolbar(app);
  assert.ok(app.screenContains("Filter"), "the button's label must be painted");

  const received: any[] = [];
  app.setHandler((event: unknown) => received.push(event));
  app.pushEvent({
    MouseDown: { button: "Left", position: { x: 1.0, y: 0.0 }, modifiers: {} },
  });
  app.run(1);

  assert.equal(
    received.length,
    1,
    `a toolbar button click carries no leading FocusChanged -- only the bar's own id (not \
the clicked button's) is a tab stop: ${JSON.stringify(received)}`,
  );
  const [event] = received;
  assert.ok(event.Toolbar, `expected a Toolbar event, got: ${JSON.stringify(event)}`);
  const [barId, toolbarEvent] = event.Toolbar;
  assert.equal(barId, "toolbar:main", "the widget id must be the toolbar's own");
  assert.equal(
    toolbarEvent.ButtonClicked?.id,
    "filter",
    "the clicked button's own id must be reported",
  );
  assert.equal(
    toolbarEvent.ButtonClicked?.is_active,
    true,
    "the pre-click is_active (false) must be reported flipped (true)",
  );
});

test("a disabled toolbar action's click stays a raw, unresolved MouseDown", () => {
  const app = new QuiApp(40, 5);
  app.setRender(() => ({
    toolbar: {
      id: "toolbar:main",
      buttons: [
        {
          Action: {
            id: "disabled-action",
            label: "Disabled",
            icon: null,
            key_hint: null,
            enabled: false,
            is_active: false,
            tooltip: "",
          },
        },
      ],
    },
  }));
  // Force the first paint so the toolbar's hit-test layout is on the hit
  // map before the click below -- `pushEvent`/`run` only ever re-render
  // on a handler-returned `Reaction::Redraw`, never on their own.
  app.screen();

  const received: any[] = [];
  app.setHandler((event: unknown) => received.push(event));
  app.pushEvent({
    MouseDown: { button: "Left", position: { x: 1.0, y: 0.0 }, modifiers: {} },
  });
  app.run(1);

  assert.ok(
    !received.some((event) => event.Toolbar),
    `a disabled action must never dispatch a Toolbar event: ${JSON.stringify(received)}`,
  );
});

// An exclusive `Group` (a view-mode segmented control): 3 children,
// "Icons" / "List" / "Columns", each keeping its own `[ ... ]` bracket
// (see `tui::toolbar::tui_item_width`'s `Group` arm). Widths: 4 + label
// width each (no icon/hint) -- "Icons" (5) = 9, "List" (4) = 8, "Columns"
// (7) = 11 -- plus 1 cell of padding before/between/after all three
// (`layout_group_children`'s even split of the group's own chrome). So
// the group's own leading `[` sits at x=0, "Icons" at [1, 10), "List" at
// [11, 19), "Columns" at [20, 31), and the group's trailing `]` at x=31
// -- "Columns" itself (inside its own brackets) starts painting at
// x=22, so x=22..29 all land inside its clickable body.
function insertViewSwitchGroup(app: any) {
  app.setRender(() => ({
    toolbar: {
      id: "toolbar:main",
      buttons: [
        {
          Group: {
            id: "view_switch",
            exclusive: true,
            items: [
              {
                Action: {
                  id: "icons",
                  label: "Icons",
                  icon: null,
                  key_hint: null,
                  enabled: true,
                  is_active: true,
                  tooltip: "",
                },
              },
              {
                Action: {
                  id: "list",
                  label: "List",
                  icon: null,
                  key_hint: null,
                  enabled: true,
                  is_active: false,
                  tooltip: "",
                },
              },
              {
                Action: {
                  id: "columns",
                  label: "Columns",
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
    },
  }));
}

test("a click on the third segment of an exclusive toolbar group emits GroupSelected, and the next frame paints it active", () => {
  const app = new QuiApp(40, 5);
  insertViewSwitchGroup(app);
  assert.ok(app.screenContains("Columns"), "the third segment's label must be painted");

  const received: any[] = [];
  app.setHandler((event: unknown) => received.push(event));
  app.pushEvent({
    MouseDown: { button: "Left", position: { x: 22.0, y: 0.0 }, modifiers: {} },
  });
  app.run(1);

  assert.equal(
    received.length,
    1,
    `a group segment click carries no leading FocusChanged: ${JSON.stringify(received)}`,
  );
  const [event] = received;
  assert.ok(event.Toolbar, `expected a Toolbar event, got: ${JSON.stringify(event)}`);
  const [barId, toolbarEvent] = event.Toolbar;
  assert.equal(barId, "toolbar:main", "the widget id must be the toolbar's own");
  assert.ok(
    toolbarEvent.GroupSelected,
    `an exclusive group's child click must carry a GroupSelected key, not ButtonClicked: \
${JSON.stringify(toolbarEvent)}`,
  );
  assert.equal(
    toolbarEvent.GroupSelected.group,
    "view_switch",
    "the owning group's own id must be reported",
  );
  assert.equal(
    toolbarEvent.GroupSelected.id,
    "columns",
    "the clicked child's own id must be reported",
  );

  // The host decides which sibling to mark active; re-rendering with
  // "columns" now the only active child must repaint it reversed/
  // selected, same as any other `is_active` toggle.
  app.setRender(() => ({
    toolbar: {
      id: "toolbar:main",
      buttons: [
        {
          Group: {
            id: "view_switch",
            exclusive: true,
            items: [
              {
                Action: {
                  id: "icons",
                  label: "Icons",
                  icon: null,
                  key_hint: null,
                  enabled: true,
                  is_active: false,
                  tooltip: "",
                },
              },
              {
                Action: {
                  id: "list",
                  label: "List",
                  icon: null,
                  key_hint: null,
                  enabled: true,
                  is_active: false,
                  tooltip: "",
                },
              },
              {
                Action: {
                  id: "columns",
                  label: "Columns",
                  icon: null,
                  key_hint: null,
                  enabled: true,
                  is_active: true,
                  tooltip: "",
                },
              },
            ],
          },
        },
      ],
    },
  }));
  assert.ok(
    app.screenContains("Columns"),
    "the now-active third segment must still paint its label",
  );
});
