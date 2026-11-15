// Black-box acceptance test for `examples/settings_app.ts`: a list/detail
// app with a category `ListView` on the left and a two-field `Form`
// (`TextInputField`/`ToggleField`) on the right. Written against
// `../js/testing.ts` -- the app-author test harness -- for the category
// list itself (a `List` node with the usual row verbs), and against
// `h.dispatch(...)` for the form fields, the same `Form` event shape
// `examples/settings_app.ts`'s own `runHeadless` pushes by hand: `../js/
// testing.ts`'s `Harness` has no row/tab-style verb for one `Form` field
// yet (it indexes a `form` node as one leaf widget, not per field), so a
// field's own id is read back through `formField` below -- `Form`'s own
// `FormDescriptor.fields` array, found with the same `Widget.get` escape
// hatch `Harness.dispatch`'s own doc describes for a shape the harness has
// no dedicated verb for -- rather than `h.find({id: <field id>})`.

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

/** The `FormFieldDescriptor` for category `name`'s `label`/`enabled` field, read off the detail `Form`'s own `fields` array -- see this file's own header doc for why, not `h.find({id: fieldId})`. */
function detailField(h: any, name: string, field: "label" | "enabled"): any {
  const fields = h.find({ id: `settings:${name}:form` }).get("fields") as Array<{ id: string }>;
  const fieldId = field === "label" ? `settings:${name}:label` : `settings:${name}:enabled`;
  const found = fields.find((f) => f.id === fieldId);
  if (found === undefined) {
    throw new Error(`no field ${JSON.stringify(fieldId)} in settings:${name}:form`);
  }
  return found;
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

// --- the detail form: TextInputField + ToggleField -----------------------

test("the detail pane shows the selected category's current label, note and enabled switch", () => {
  const { h } = ui();

  const input = detailField(h, "General", "label");
  assert.deepEqual(input.kind, { TextInput: { value: settingsExample.DEFAULTS.General.label } });

  const toggle = detailField(h, "General", "enabled");
  assert.deepEqual(toggle.kind, { Toggle: { value: settingsExample.DEFAULTS.General.enabled } });

  h.assertText(settingsExample.DEFAULTS.General.note);
});

test("committing a new label updates the app's own settings and the rendered field", () => {
  const { settings, h } = ui();

  setTextInput(h, "settings:General:label", "Renamed Workspace");

  assert.equal(settings.settings.General.label, "Renamed Workspace");
  // The rendered field itself, not just the app's own state -- the text
  // field is narrow enough to truncate the display on screen, so this
  // reads the field descriptor's own `value` rather than a screen substring.
  assert.deepEqual(detailField(h, "General", "label").kind, { TextInput: { value: "Renamed Workspace" } });
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
  assert.deepEqual(detailField(h, "General", "enabled").kind, { Toggle: { value: false } });
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
