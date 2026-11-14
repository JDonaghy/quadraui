// Black-box acceptance test for `examples/settings_app.ts`: a list/detail
// form app with a category `ListView` on the left and a two-field
// `text_input`/`switch` form on the right. Written against `../js/testing.ts`
// -- the app-author test harness -- for the category list itself (a
// `List` node with the usual row verbs), and against `h.dispatch(...)` for
// the raw `text_input`/`switch` leaves, the same `Form` event shape
// `examples/settings_app.ts`'s own `runHeadless` pushes by hand: those two
// leaves have no dedicated `Node` constructor (this file's own header doc
// explains why), so there is no row/tab verb for them to ride on, same as
// `Harness.dispatch`'s own doc describes it as the escape hatch for a
// shape the harness has no verb for.

const { test } = require("node:test");
const assert = require("node:assert/strict");

const settingsExample = require("../examples/settings_app.ts");
const { harness } = require("../js/testing.ts");

function ui(width = 70, height = 20): { settings: InstanceType<typeof settingsExample.SettingsApp>; h: any } {
  const settings = new settingsExample.SettingsApp(width, height);
  return { settings, h: harness(settings.app) };
}

function list(h: any): any {
  return h.find({ role: "list" });
}

function setTextInput(h: any, id: string, value: string): void {
  h.dispatch({ Form: [id, { TextInputCommitted: { id, value } }] });
}

function toggleSwitch(h: any, id: string, value: boolean): void {
  h.dispatch({ Form: [id, { ToggleChanged: { id, value } }] });
}

// --- the category list --------------------------------------------------

test("every category is listed, General selected and focused by default", () => {
  const { h } = ui();

  assert.deepEqual(list(h).rowTexts, settingsExample.CATEGORIES);
  assert.equal(list(h).selectedRow.text, "General");
  h.assertText("General", "Appearance", "Privacy");
});

test("clicking a category row selects it and shows its detail pane", () => {
  const { settings, h } = ui();

  list(h).row("Appearance").click();

  assert.equal(settings.selectedIdx, settingsExample.CATEGORIES.indexOf("Appearance"));
  assert.equal(list(h).selectedRow.text, "Appearance");
  // The note is long enough to wrap onto a second terminal line, so check
  // it through the detail title's own widget rather than a contiguous
  // screen substring.
  assert.ok(h.find({ id: "detail:note" }).text.includes(DEFAULTS_NOTE("Appearance")));
});

test("Down/Up move the category cursor", () => {
  const { settings, h } = ui();

  h.press("Down");
  assert.equal(settings.selectedIdx, 1);
  assert.equal(list(h).selectedRow.text, "Appearance");

  h.press("Down");
  assert.equal(settings.selectedIdx, 2);

  h.press("Up");
  assert.equal(settings.selectedIdx, 1);
});

test("Enter opens the cursored category and reports it on the status bar", () => {
  const { h } = ui();

  h.press("Enter");

  h.assertText("Opened General");
});

test("activating a row (double click) opens it the same way Enter does", () => {
  const { settings, h } = ui();

  list(h).row("Privacy").activate();

  assert.equal(settings.selectedIdx, settingsExample.CATEGORIES.indexOf("Privacy"));
  h.assertText("Opened Privacy");
});

// --- the detail form: text_input + switch (raw wire leaves) -------------

test("the detail pane shows the selected category's current label, note and enabled switch", () => {
  const { h } = ui();

  const input = h.find({ id: "settings:General:label" });
  assert.deepEqual(input.get("lines"), [settingsExample.DEFAULTS.General.label]);

  const toggle = h.find({ id: "settings:General:enabled" });
  assert.equal(toggle.get("value"), settingsExample.DEFAULTS.General.enabled);

  h.assertText(settingsExample.DEFAULTS.General.note);
});

test("committing a new label updates the app's own settings and the rendered field", () => {
  const { settings, h } = ui();

  setTextInput(h, "settings:General:label", "Renamed Workspace");

  assert.equal(settings.settings.General.label, "Renamed Workspace");
  // The rendered field itself, not just the app's own state -- the text
  // field is narrow enough to truncate the display on screen, so this
  // reads the descriptor's full `lines` rather than a screen substring.
  assert.deepEqual(h.find({ id: "settings:General:label" }).get("lines"), ["Renamed Workspace"]);
});

test("an in-progress (uncommitted) edit also applies -- TextInputChanged, not just Committed", () => {
  const { settings, h } = ui();

  h.dispatch({ Form: ["settings:General:label", { TextInputChanged: { id: "settings:General:label", value: "Wor" } }] });

  assert.equal(settings.settings.General.label, "Wor");
});

test("toggling the switch flips the app's own settings and the rendered value", () => {
  const { settings, h } = ui();

  assert.equal(settings.settings.General.enabled, true);
  toggleSwitch(h, "settings:General:enabled", false);

  assert.equal(settings.settings.General.enabled, false);
  assert.equal(h.find({ id: "settings:General:enabled" }).get("value"), false);
});

test("editing one category's fields never touches another category's settings", () => {
  const { settings, h } = ui();

  list(h).row("Appearance").click();
  setTextInput(h, "settings:Appearance:label", "Nightfall");
  toggleSwitch(h, "settings:Appearance:enabled", true);

  assert.deepEqual(settings.settings.General, {
    label: settingsExample.DEFAULTS.General.label,
    enabled: settingsExample.DEFAULTS.General.enabled,
  });
  assert.deepEqual(settings.settings.Appearance, { label: "Nightfall", enabled: true });
});

// --- the Reset button: mouse click and focused-Enter paths ---------------

test("clicking Reset restores the selected category's defaults", () => {
  const { settings, h } = ui();

  setTextInput(h, "settings:General:label", "Scratch");
  toggleSwitch(h, "settings:General:enabled", false);

  h.find({ role: "button", id: "settings:reset" }).click();

  assert.deepEqual(settings.settings.General, {
    label: settingsExample.DEFAULTS.General.label,
    enabled: settingsExample.DEFAULTS.General.enabled,
  });
  h.assertText("General reset to defaults");
});

test("focusing Reset and pressing Enter resets it too, without the onEvent fallback stomping the message", () => {
  const { settings, h } = ui();

  setTextInput(h, "settings:General:label", "Scratch");
  h.focus("settings:reset");
  h.press("Enter");

  assert.equal(settings.settings.General.label, settingsExample.DEFAULTS.General.label);
  // The focused-Enter path runs the button's own `onClick` through the app
  // layer's node-callback dispatch and returns early -- if `onEvent`'s
  // fallback also ran afterwards, the plain-key handler would have
  // overwritten this message by clearing it first (`SettingsApp.onEvent`'s
  // own `this.message = ""`).
  h.assertText("General reset to defaults");
});

// --- quitting a live session ---------------------------------------------

test("installQuitKey wraps onEvent so a non-quit key still reaches the inner handler", () => {
  // `q`/Escape's own quit branch calls `settings.app.stopLive()`
  // re-entrantly from inside the dispatch `step()` is already running --
  // a shape only a real `startTui`/`startNative` live session exercises
  // (its run loop is not itself inside a `step()` call frame the way a
  // direct, headless `pushEvent`+`step()` call in this test is), so it is
  // not exercised here; see `examples/file_manager_app.ts`'s own
  // `installQuitKey`, which shares this exact shape and the same gap.
  const settings = new settingsExample.SettingsApp();
  settingsExample.installQuitKey(settings);

  settings.app.pushEvent({
    KeyPressed: { key: { Named: "Down" }, modifiers: { shift: false, ctrl: false, alt: false, cmd: false }, repeat: false },
  });
  settings.app.step();
  assert.equal(settings.selectedIdx, 1);
});

function DEFAULTS_NOTE(name: string): string {
  return settingsExample.DEFAULTS[name].note;
}
