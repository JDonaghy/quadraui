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
