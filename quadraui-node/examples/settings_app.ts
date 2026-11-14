// A small list/detail form app on `js/app/index.ts`: a `ListView` of
// setting categories on the left, a two-field form (a `text_input` and a
// `switch`) for whichever category is selected on the right. `text_input`
// and `switch` have no dedicated `Node` constructor in `./nodes.ts` (only
// the fixed-slot primitives and a handful of additive ones do) -- this
// example reaches for the documented escape hatch instead: a raw wire
// object (`{id, text_input: {...}}`/`{id, switch: {...}}`) passed straight
// into `Column`'s `items`, same shape `App._render`'s own `walk` already
// passes through unchanged for anything that isn't a `Node`/`Component`.
// Their events (`Form`'s `TextInputChanged`/`TextInputCommitted`/
// `ToggleChanged`) have no `CONTAINER_EVENT_MAP` entry either, so this
// example reads them off the `onEvent` escape hatch rather than a node
// callback -- see `./nodes.ts`'s own `Renderable` doc and `./runtime.ts`'s
// `App` doc for both seams.
//
// Run headless (default), `--tui`, or `--native`:
//
//     node examples/settings_app.ts [--tui|--native]
//
// Interactive keys: Up/Down move the category cursor, Enter opens the
// cursored category (same as clicking it). Inside a category's detail
// form, click the text field to focus it and type to edit the display
// name, click the switch to toggle it, and click (or Tab + Enter to) the
// "Reset" button to restore that category's defaults. `q`/Escape quits a
// live session.

const { App, Button, Column, Row, SidebarPanel, StatusBar, TextDisplay, keyName } = require("../js/app/index.ts");

import type { Renderable } from "../js/app/index";
import type { Color, LayoutLeafNode, ListItemDescriptor, StatusBarSegment, StyledText } from "../js/wire";

interface Setting {
  label: string;
  enabled: boolean;
}

interface CategoryDefaults {
  label: string;
  enabled: boolean;
  note: string;
}

const DEFAULTS: Record<string, CategoryDefaults> = {
  General: { label: "My Workspace", enabled: true, note: "Shown in the window title." },
  Appearance: { label: "Dark", enabled: false, note: "Theme name; the switch below is \"Compact\"." },
  Privacy: { label: "Standard", enabled: true, note: "Telemetry level; the switch below is \"Opt in\"." },
};
const CATEGORIES = Object.keys(DEFAULTS);

function styledText(text: string): StyledText {
  return { spans: [{ text, fg: null, bg: null }] };
}

function freshSettings(): Record<string, Setting> {
  const result: Record<string, Setting> = {};
  for (const name of CATEGORIES) {
    result[name] = { label: DEFAULTS[name].label, enabled: DEFAULTS[name].enabled };
  }
  return result;
}

const STATUS_FG: Color = { r: 220, g: 220, b: 230, a: 255 };
const STATUS_BG: Color = { r: 30, g: 32, b: 48, a: 255 };

function segment(text: string): StatusBarSegment {
  return { text, fg: STATUS_FG, bg: STATUS_BG, bold: false };
}

/**
 * Holds every category's `Setting` and the live UI state (which category
 * is selected, a status message), and drives the `QuiApp` wrapped by
 * `this.app`.
 */
class SettingsApp {
  settings: Record<string, Setting>;
  selectedIdx: number;
  message: string;
  app: InstanceType<typeof App>;

  constructor(width = 70, height = 20) {
    this.settings = freshSettings();
    this.selectedIdx = 0;
    this.message = "";
    this.app = new App(width, height, {
      view: () => this._view(),
      onEvent: (event: unknown) => this.onEvent(event),
    });
  }

  private get _selectedName(): string {
    return CATEGORIES[this.selectedIdx];
  }

  private _select(idx: number): void {
    this.selectedIdx = Math.max(0, Math.min(CATEGORIES.length - 1, idx));
  }

  private _labelFieldId(name: string): string {
    return `settings:${name}:label`;
  }

  private _enabledFieldId(name: string): string {
    return `settings:${name}:enabled`;
  }

  /** Applies a `TextInputChanged`/`TextInputCommitted` payload if `id` names a known category's label field. */
  private _applyLabel(id: string, value: string): void {
    for (const name of CATEGORIES) {
      if (this._labelFieldId(name) === id) {
        this.settings[name].label = value;
        return;
      }
    }
  }

  /** Applies a `ToggleChanged` payload if `id` names a known category's enabled field. */
  private _applyEnabled(id: string, value: boolean): void {
    for (const name of CATEGORIES) {
      if (this._enabledFieldId(name) === id) {
        this.settings[name].enabled = value;
        return;
      }
    }
  }

  private _reset(name: string): void {
    this.settings[name] = { label: DEFAULTS[name].label, enabled: DEFAULTS[name].enabled };
    this.message = `${name} reset to defaults`;
  }

  // --- view --------------------------------------------------------------

  private _listLeaf(): LayoutLeafNode {
    const items: ListItemDescriptor[] = CATEGORIES.map((name) => ({ text: styledText(name) }));
    return {
      id: "settings:list",
      list: {
        id: "settings:list",
        items,
        selected_idx: this.selectedIdx,
        has_focus: true,
      },
    };
  }

  private _detailPane(): Renderable {
    const name = this._selectedName;
    const setting = this.settings[name];
    return Column([
      TextDisplay([{ spans: [{ text: name, fg: null, bg: null }] }], { id: "detail:title" }),
      TextDisplay([{ spans: [{ text: DEFAULTS[name].note, fg: null, bg: null }] }], { id: "detail:note" }),
      Row([
        TextDisplay([{ spans: [{ text: "Display name", fg: null, bg: null }] }], { id: "detail:label-caption" }),
        {
          id: this._labelFieldId(name),
          text_input: { id: this._labelFieldId(name), lines: [setting.label] },
        } satisfies LayoutLeafNode,
      ]),
      Row([
        TextDisplay([{ spans: [{ text: "Enabled", fg: null, bg: null }] }], { id: "detail:enabled-caption" }),
        {
          id: this._enabledFieldId(name),
          switch: { id: this._enabledFieldId(name), value: setting.enabled },
        } satisfies LayoutLeafNode,
      ]),
      Button("Reset", { id: "settings:reset", onClick: () => this._reset(name) }),
    ]);
  }

  private _statusLeaf(): Renderable {
    const left = [segment(` ${this._selectedName} `)];
    if (this.message) left.push(segment(` ${this.message} `));
    return StatusBar(left, [], { id: "settings:status" });
  }

  private _view(): Renderable {
    const body = Row([this._listLeaf(), this._detailPane()]);
    const chrome = SidebarPanel(body, { id: "settings:chrome" });
    return Column([chrome, this._statusLeaf()]);
  }

  // --- events --------------------------------------------------------------

  /** Only truly global input, plus `List`/`Form` events (neither has a `CONTAINER_EVENT_MAP`/node-callback path) land here. */
  onEvent(event: unknown): void {
    if (typeof event !== "object" || event === null) return;
    const obj = event as Record<string, unknown>;

    if ("List" in obj) {
      const [, activation] = obj.List as [string, Record<string, unknown>];
      if ("SelectionChanged" in activation) {
        this._select((activation.SelectionChanged as { idx: number }).idx);
      } else if ("ItemActivated" in activation) {
        this._select((activation.ItemActivated as { idx: number }).idx);
        this.message = `Opened ${this._selectedName}`;
      }
      return;
    }

    if ("Form" in obj) {
      const [, formEvent] = obj.Form as [string, Record<string, unknown>];
      if ("ToggleChanged" in formEvent) {
        const payload = formEvent.ToggleChanged as { id: string; value: boolean };
        this._applyEnabled(payload.id, payload.value);
      } else if ("TextInputChanged" in formEvent) {
        const payload = formEvent.TextInputChanged as { id: string; value: string };
        this._applyLabel(payload.id, payload.value);
      } else if ("TextInputCommitted" in formEvent) {
        const payload = formEvent.TextInputCommitted as { id: string; value: string };
        this._applyLabel(payload.id, payload.value);
      }
      return;
    }

    // Everything else that reaches `onEvent` (`_dispatch` returned `false`)
    // is a keystroke this app has no node callback for, or a mouse/focus
    // event this app does not react to at all -- a button click's own
    // `MouseUp` half, in particular, following the `MouseDown` half
    // `_dispatch` already resolved into an `onClick` call. `keyName` is
    // `null` for the latter group, so checking it before clearing
    // `message` keeps a `MouseUp`/`FocusChanged`/etc. from wiping out a
    // message a node callback (Reset's `onClick`) just set.
    const name = keyName(event);
    if (name === null) return;
    this.message = "";
    if (name === "Down") this._select(this.selectedIdx + 1);
    else if (name === "Up") this._select(this.selectedIdx - 1);
    else if (name === "Enter") this.message = `Opened ${this._selectedName}`;
  }
}

/**
 * Wraps `settings.app`'s own `onEvent` so `q`/Escape ends a live session --
 * same shape `examples/file_manager_app.ts`'s own `installQuitKey` uses.
 */
function installQuitKey(settings: SettingsApp): void {
  const inner = settings.onEvent.bind(settings);
  settings.app.setOnEvent((event: unknown) => {
    const name = keyName(event);
    if (name === "q" || name === "Escape") {
      settings.app.stopLive();
      return;
    }
    inner(event);
  });
}

/** Selects "Appearance", edits its display name and enabled switch, then activates "Reset" via a focused Enter -- then prints the final screen. */
function runHeadless(settings: SettingsApp): void {
  const appearanceIdx = CATEGORIES.indexOf("Appearance");
  const labelId = "settings:Appearance:label";
  const enabledId = "settings:Appearance:enabled";

  settings.app.pushEvent({ List: ["settings:list", { SelectionChanged: { idx: appearanceIdx } }] });
  settings.app.step();
  settings.app.pushEvent({
    Form: [labelId, { TextInputCommitted: { id: labelId, value: "Nightfall" } }],
  });
  settings.app.step();
  settings.app.pushEvent({
    Form: [enabledId, { ToggleChanged: { id: enabledId, value: true } }],
  });
  settings.app.step();
  console.log(settings.app.screen());
  console.log(`\nafter editing: selected_idx=${settings.selectedIdx}  settings=${JSON.stringify(settings.settings)}`);

  settings.app.pushEvent({ FocusChanged: "settings:reset" });
  settings.app.step();
  settings.app.pushEvent({
    KeyPressed: { key: { Named: "Enter" }, modifiers: { shift: false, ctrl: false, alt: false, cmd: false }, repeat: false },
  });
  settings.app.step();

  console.log(`\nafter Reset: settings=${JSON.stringify(settings.settings)}`);
}

exports.SettingsApp = SettingsApp;
exports.CATEGORIES = CATEGORIES;
exports.DEFAULTS = DEFAULTS;
exports.freshSettings = freshSettings;
exports.installQuitKey = installQuitKey;

if (require.main === module) {
  const flags = new Set(process.argv.slice(2));
  const settings = new SettingsApp();

  if (flags.has("--tui")) {
    installQuitKey(settings);
    settings.app.runTui();
  } else if (flags.has("--native")) {
    installQuitKey(settings);
    settings.app.runNative("quadraui settings");
  } else {
    runHeadless(settings);
  }
}
