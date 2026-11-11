// Black-box coverage proving the B3a `Button` fields (`variant`, `icon`,
// `iconPosition`, `iconGap`, `tooltip`) and the B3b `ButtonSpec` reach this
// binding through `quadraui-binding-core`'s wire decode with **no binding
// code of their own** -- both are plain `#[serde(default)]` fields, so a
// plain object missing them decodes to the same thing `Button::new`/
// `ButtonSpec::new` build, and an object naming them just works. Port of
// `quadraui-py/tests/test_button_model.py` -- see that module's own doc
// for the full rationale behind each of the three scenarios below.
//
// Run against a built addon, e.g.:
//
//     npm run build:debug
//     npm test
const { test } = require("node:test");
const assert = require("node:assert/strict");

const { QuiApp } = require("../js/index.ts");

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

function rgb(color: any): [number, number, number] {
  return [color.r, color.g, color.b];
}

test("an inline button leaf decodes variant/icon/tooltip from a plain object and paints the Danger colour", () => {
  const app = new QuiApp(40, 20);
  app.setRender(() => ({
    button: {
      id: "demo:danger",
      label: "Delete",
      variant: "Danger",
      icon: { glyph: "", fallback: "!", color: null },
      tooltip: "This cannot be undone",
    },
  }));

  // The label and the icon's plain-text fallback both paint -- the
  // headless backend defaults nerd fonts off, so `Icon::resolve` picks
  // `fallback` over `glyph`.
  assert.ok(app.screenContains("Delete"));
  assert.ok(app.screenContains("!"));

  // `variant: Danger` colours the label text in `Theme.error_fg` on
  // TUI -- there is no dedicated `danger_fg` field, the label
  // foreground *is* the intent colour. `tooltip` is a documented TUI
  // no-op, so it has nothing further to assert on here.
  const [row, col] = locate(app, "Delete");
  const [labelFg] = app.cellColors(col, row) as [number[], number[]];
  assert.deepEqual(labelFg, rgb((app.theme() as any).error_fg));
});

test("an inline button leaf without variant keeps the default primary look", () => {
  // Same shape minus `variant`/`icon`/`tooltip` -- proves the Danger
  // colouring above comes from the field actually being set, not from
  // some unrelated default already matching `error_fg`.
  const app = new QuiApp(40, 20);
  app.setRender(() => ({ button: { id: "demo:plain", label: "Delete" } }));

  const [row, col] = locate(app, "Delete");
  const [labelFg] = app.cellColors(col, row) as [number[], number[]];
  const theme: any = app.theme();
  assert.deepEqual(labelFg, rgb(theme.foreground));
  assert.notDeepEqual(labelFg, rgb(theme.error_fg));
});

test("showToast action with a Danger spec colours its row in error_fg", () => {
  // `Toast.actions` decodes straight to `ToastButton[]`, and
  // `ToastButton.spec` -- the same `ButtonSpec` `Button.icon`'s own
  // sibling fields live on -- is honoured by `tui::toast::paint_toast`
  // with no stripping in between, unlike `DialogButton.spec` (see the
  // next test). A `Danger` spec recolours the whole action row's
  // foreground to `Theme.error_fg`, dropping any `accent_bg`/`link_fg`
  // look the legacy `primary` flag would otherwise give it.
  const app = new QuiApp(40, 20);
  app.showToast({
    id: "toast:confirm",
    title: "Delete file?",
    body: "",
    actions: [
      { id: "keep", label: "Keep" },
      {
        id: "discard",
        label: "Discard",
        spec: { label: "Discard", variant: "Danger" },
      },
    ],
  });

  assert.ok(app.screenContains("Keep"));
  assert.ok(app.screenContains("Discard"));

  const [keepRow, keepCol] = locate(app, "Keep");
  const [discardRow, discardCol] = locate(app, "Discard");

  const [keepFg] = app.cellColors(keepCol, keepRow) as [number[], number[]];
  const [discardFg] = app.cellColors(discardCol, discardRow) as [
    number[],
    number[],
  ];

  assert.deepEqual(discardFg, rgb((app.theme() as any).error_fg));
  assert.notDeepEqual(keepFg, discardFg);
});

test("showMessageDialog accepts a dialog button with a Danger spec", () => {
  // `DialogButton.spec` decodes fine through the same decode path the
  // other two scenarios use -- a dialog button naming `spec` never
  // raises. It has no visible rendering effect on *this* surface yet:
  // `message_dialog_options_from_buttons` (`quadraui-binding-core/src/
  // engine.rs`) narrows every button down to
  // `quadraui::backend::MessageDialogButton` -- the same native-OS-
  // compatible shape that already rejects `tint` for the identical
  // reason -- before `MessageDialogController::new` (`quadraui/src/
  // compose/message_dialog.rs`) ever sees it. That stripping lives in
  // the `quadraui` crate itself, not `quadraui-binding-core`'s decode
  // glue, and touches every native backend's message-dialog
  // construction site, so wiring it through is out of this issue's
  // scope -- the toast scenario above is the one that proves the B3b
  // vocabulary paints.
  const app = new QuiApp(60, 20);
  app.showMessageDialog("Unsaved changes", "Delete the file permanently?", [
    {
      id: "discard",
      label: "Delete",
      is_default: true,
      spec: { label: "Delete", variant: "Danger" },
    },
    { id: "cancel", label: "Cancel", is_cancel: true },
  ]);

  assert.ok(app.screenContains("Delete"));
  assert.ok(app.screenContains("Cancel"));
});
