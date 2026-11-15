// A small list/detail form app on `js/app/index.ts`: a typed `List` node
// of setting categories on the left, a two-field `Form` (a `TextInputField`
// and a `ToggleField`) for whichever category is selected on the right --
// no raw wire object and no `"Form"` branch in `onEvent`: each field's own
// `onChange`/`onSubmit` option is registered under that field's own id the
// same way `List`'s `onSelect`/`onActivate` already are (see `./nodes.ts`'s
// own `Form` doc for how a field's id is resolved and registered).
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

const {
  App,
  Button,
  Column,
  Form,
  List,
  Row,
  SidebarPanel,
  StatusBar,
  TextDisplay,
  TextInputField,
  ToggleField,
  keyName,
} = require("../js/app/index.ts");

import type { Renderable } from "../js/app/index";
import type { Color, ListItemDescriptor, StatusBarSegment, StyledText } from "../js/wire";

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

  private _reset(name: string): void {
    this.settings[name] = { label: DEFAULTS[name].label, enabled: DEFAULTS[name].enabled };
    this.message = `${name} reset to defaults`;
  }

  // --- view --------------------------------------------------------------

  private _listLeaf(): Renderable {
    const items: ListItemDescriptor[] = CATEGORIES.map((name) => ({ text: styledText(name) }));
    return List(items, {
      id: "settings:list",
      selected: this.selectedIdx,
      hasFocus: true,
      onSelect: (idx: number) => this._select(idx),
      onActivate: (idx: number) => {
        this._select(idx);
        this.message = `Opened ${this._selectedName}`;
      },
    });
  }

  private _detailPane(): Renderable {
    const name = this._selectedName;
    const setting = this.settings[name];
    return Column([
      TextDisplay([{ spans: [{ text: name, fg: null, bg: null }] }], { id: "detail:title" }),
      TextDisplay([{ spans: [{ text: DEFAULTS[name].note, fg: null, bg: null }] }], { id: "detail:note" }),
      Form(
        [
          TextInputField("Display name", setting.label, {
            id: this._labelFieldId(name),
            onChange: (value: string) => {
              setting.label = value;
            },
            onSubmit: (value: string) => {
              setting.label = value;
            },
          }),
          ToggleField("Enabled", setting.enabled, {
            id: this._enabledFieldId(name),
            onChange: (value: boolean) => {
              setting.enabled = value;
            },
          }),
        ],
        { id: `settings:${name}:form`, hasFocus: true },
      ),
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

  /**
   * Only truly global input lands here now -- the category `List`'s own
   * `SelectionChanged`/`ItemActivated` and the detail `Form`'s own fields'
   * `onChange`/`onSubmit` are node callbacks (`List`'s `onSelect`/
   * `onActivate`, `Form`'s per-field options in `_detailPane`), not a
   * branch here.
   */
  onEvent(event: unknown): void {
    if (typeof event !== "object" || event === null) return;

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
