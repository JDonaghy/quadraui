// App-author test harness: drive a `quadraui` app from a test the way a
// user drives it, without writing a single wire event by hand. The
// TypeScript mirror of `quadraui-py`'s own `quadraui.testing`, concept
// for concept and name for name (camelCase here, snake_case there).
//
// A test built on this module says what a person would say -- *find the
// tree, click the row labelled "alpha", press Enter, assert "a1.txt" is
// on screen* -- and the harness turns each of those into the real
// `event -> handle -> render` round trip the headless runner already
// performs. Nothing here is a second rendering path or a mock: every
// query reads the descriptors the app actually rendered, and every
// synthesised action is the same `UiEvent` the engine itself would
// deliver.
//
//     const { harness } = require("@quadraui/node/js/testing.ts");
//
//     const h = harness(app);
//     h.find({ role: "tree" }).row("alpha").click();
//     h.press("Enter");
//     h.assertText("a1.txt");
//
// **Querying.** `find`/`findAll`/`query` take any combination of `id`,
// `role` (the wire kind key: `"tree"`, `"data_table"`, `"tabs"`,
// `"button"`, ...) and `text` (a substring of everything that widget
// renders). They walk the widget tree the last render produced, in
// document order, and by default skip whatever is not on screen -- the
// content of an inactive tab is there in the wire tree but invisible to
// a user, so it is invisible to a query too unless `includeHidden: true`.
// `Widget.find`/`Widget.findAll` run the same search over one subtree.
//
// **Acting.** Row, tab and divider actions are synthesised from the
// descriptor the app rendered, so a test names a row by its text instead
// of by a path or a pixel: `tree.row("alpha").toggle()`,
// `table.row({ index: 1 }).doubleClick()`, `tabs.tabs[0].close()`,
// `split.dragDivider(0.6)`. Keyboard input is `h.press("Down")` /
// `h.type("hello")` globally, or `widget.press("Down")` for the
// focus-scoped shape a real backend delivers to a focused widget. A
// button is clicked through the engine's own hit test:
// `h.clickText("Refresh")` (and `button.click()`) finds the label on the
// rendered frame and pushes a `MouseDown` at those coordinates, so the
// test never hardcodes one.
//
// **Asserting.** `h.screen()` is the rendered frame's text,
// `h.contains(...)` a substring check, and `h.assertText(...)` /
// `h.assertNotText(...)` the same checks with the whole frame printed in
// the failure message. Widget-level state the app itself does not track
// (scroll offset, active tab, split ratio) reads off `widget.state`.
//
// **Staleness.** A `Widget`/`Row`/`Tab` is a snapshot of one frame. Any
// action re-renders, so re-query after acting rather than reusing a
// handle across a state change.
//
// The harness wraps an `App` from `./app` directly. For a plain `QuiApp`
// it needs the same `render` callable the app gave the `QuiApp`, since
// there is no way to read a registered callback back out:
//
//     const h = harness(quiApp, { render: myRender });
//
// **The test runner.** `node --test` has no fixture mechanism, so a
// `require` of this module is the whole integration -- but a suite that
// enables hot reload should call `h.close()` from the runner's own
// `after()` so its watcher timer does not outlive the test.
// `quadraui-py`'s `quadraui.testing` packages the same teardown as a
// `pytest` fixture (`quadraui_harness`), since pytest does have one.
//
// Runtime exports go through `exports.X = ...` rather than `export`, the
// same reason `./index.ts`'s own header comment gives.

import type { WidgetState } from "./index";

/** No widget matched a `find()`. */
class WidgetNotFoundError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "WidgetNotFoundError";
  }
}
exports.WidgetNotFoundError = WidgetNotFoundError;

// Layout nodes that hold other nodes rather than painting a primitive; every
// other non-meta key on a node names the primitive kind it paints.
const CONTAINER_ROLES = new Set(["row", "column", "split", "tabs", "panel", "sidebar_panel", "split_tree"]);

// Keys a layout node may carry alongside its one role key.
const META_KEYS = new Set(["id", "padding", "margin", "gap"]);

// Descriptor fields whose string values are text a user can read. `glyph` and
// `fallback` (an icon and its ASCII stand-in) are deliberately absent: they are
// decoration, and matching `text` against them would make `find({text: "/"})`
// hit every folder row.
const TEXT_KEYS = new Set(["text", "label", "title", "placeholder", "fallback_text"]);

// The event wrapper each focusable role's own events arrive under.
// What `Harness.state` reports for a widget the engine is tracking nothing
// for: every field `null`, so a caller reads it the same way either way.
const EMPTY_STATE: WidgetState = {
  selectedPath: null,
  selectedIndex: null,
  scrollOffset: null,
  splitRatio: null,
  activeTab: null,
  expanded: null,
};

const EVENT_SCOPES: Record<string, string> = {
  tree: "Tree",
  data_table: "DataTable",
  list: "List",
  tabs: "TabBar",
  tab_bar: "TabBar",
  status_bar: "StatusBar",
};

const NAMED_KEYS = [
  "Escape",
  "Tab",
  "BackTab",
  "Enter",
  "Backspace",
  "Delete",
  "Insert",
  "Home",
  "End",
  "PageUp",
  "PageDown",
  "Up",
  "Down",
  "Left",
  "Right",
  "CapsLock",
  "NumLock",
  "ScrollLock",
  "Menu",
];

export type Modifiers = { shift?: boolean; ctrl?: boolean; alt?: boolean; meta?: boolean };

export type Query = { id?: string; role?: string; text?: string; includeHidden?: boolean };

export type RowSelector = string | { index?: number; path?: ReadonlyArray<number>; text?: string };

export type HarnessOptions = { render?: () => unknown };

/** The wire `Key` for a key named the way a person says it: `"Down"`, `"Enter"`, `"F2"`, or any single character. */
function keyPayload(key: string): unknown {
  const fn = /^[fF]([1-9][0-9]?)$/.exec(key);
  if (fn !== null) {
    return { Named: { F: Number(fn[1]) } };
  }
  const named = NAMED_KEYS.find((name) => name.toLowerCase() === key.toLowerCase());
  if (named !== undefined) {
    return { Named: named };
  }
  if ([...key].length === 1) {
    return { Char: key };
  }
  throw new TypeError(`not a key name: ${JSON.stringify(key)} (expected a named key, F1-F99, or one character)`);
}

/** The wire `Modifiers`; `meta` is the portable name for the key the wire calls `cmd`. */
function wireModifiers(mods: Modifiers = {}): Record<string, boolean> {
  return {
    shift: mods.shift === true,
    ctrl: mods.ctrl === true,
    alt: mods.alt === true,
    cmd: mods.meta === true,
  };
}

/** The canonical name of a key payload, as a focus-scoped event carries it. */
function keyName(payload: any): string {
  if (typeof payload.Char === "string") {
    return payload.Char;
  }
  const named = payload.Named;
  return typeof named === "object" && named !== null ? `F${named.F}` : String(named);
}

/** Every readable string inside a descriptor fragment, in render order, space-separated. */
function textOf(value: unknown): string {
  const out: string[] = [];
  collectText(value, out);
  return out.filter((part) => part !== "").join(" ");
}

function collectText(value: unknown, out: string[]): void {
  if (typeof value === "string") {
    out.push(value);
    return;
  }
  if (Array.isArray(value)) {
    for (const item of value) {
      collectText(item, out);
    }
    return;
  }
  if (typeof value !== "object" || value === null) {
    return;
  }
  for (const [key, item] of Object.entries(value)) {
    if (TEXT_KEYS.has(key)) {
      collectText(item, out);
    } else if (typeof item === "object" && item !== null) {
      collectText(item, out);
    }
  }
}

/**
 * One row of a tree, table or list, named by what it renders rather than
 * by its index or tree path. Every action is the `UiEvent` the engine
 * delivers when a user does that to this row.
 */
class Row {
  widget: Widget;
  index: number;
  descriptor: any;

  constructor(widget: Widget, index: number, descriptor: any) {
    this.widget = widget;
    this.index = index;
    this.descriptor = descriptor;
  }

  /** The tree path of this row, or `null` for a flat table/list row. */
  get path(): number[] | null {
    return this.descriptor.path ?? null;
  }

  /** Everything this row renders, cells and all, space-separated. */
  get text(): string {
    return textOf(this.descriptor);
  }

  /** One string per table column; a tree/list row reports a one-element array. */
  get cells(): string[] {
    const cells = this.descriptor.cells;
    if (cells === undefined) {
      return [textOf(this.descriptor.text)];
    }
    return cells.map((cell: unknown) => textOf(cell));
  }

  /** Whether a tree row is currently showing its children. */
  get expanded(): boolean {
    return this.descriptor.is_expanded === true;
  }

  /** A single click on the row body: selects, never opens. */
  click(): void {
    this.act("click");
  }

  /** Moves the selection onto this row, the way an arrow key does. */
  select(): void {
    this.act("select");
  }

  /** A double click: whatever "open this" means for this app. */
  doubleClick(): void {
    this.act("doubleClick");
  }

  /** Activates the row without a mouse -- Enter on a focused list or table. */
  activate(): void {
    this.act("activate");
  }

  /** Clicks a tree row's expand/collapse chevron. */
  toggle(): void {
    this.act("toggle");
  }

  // Each role renders rows differently and reports them differently: a tree
  // addresses a row by `path` and a flat table/list by `idx`, and each has its
  // own name for "the user opened this". `ROW_EVENTS` is that table.
  private act(verb: string): void {
    const variant = ROW_EVENTS[this.widget.role]?.[verb];
    if (variant === undefined) {
      throw new TypeError(`a ${JSON.stringify(this.widget.role)} row has no ${JSON.stringify(verb)} action`);
    }
    const [name, payload] = variant;
    this.widget.sendScoped({ [name]: payload(this) });
  }
}
exports.Row = Row;

const byPath = (row: Row) => ({ path: row.path });
const byIndex = (row: Row) => ({ idx: row.index });

// (role, verb) -> the `UiEvent` variant the engine delivers for it.
const ROW_EVENTS: Record<string, Record<string, [string, (row: Row) => unknown]>> = {
  tree: {
    click: ["RowClicked", (row: Row) => ({ path: row.path, modifiers: wireModifiers() })],
    select: ["SelectionChanged", byPath],
    doubleClick: ["RowDoubleClicked", byPath],
    activate: ["RowDoubleClicked", byPath],
    toggle: ["RowToggleExpand", byPath],
  },
  data_table: {
    click: ["RowSelected", byIndex],
    select: ["RowSelected", byIndex],
    doubleClick: ["RowDoubleClicked", byIndex],
    activate: ["RowActivated", byIndex],
  },
  list: {
    click: ["SelectionChanged", byIndex],
    select: ["SelectionChanged", byIndex],
    doubleClick: ["ItemDoubleClicked", byIndex],
    activate: ["ItemActivated", byIndex],
  },
};

/** One tab of a `tabs` node: its title, the widget tree it holds, and the click/close a user performs on its header. */
class Tab {
  widget: Widget;
  index: number;
  descriptor: any;
  content: Widget | null;

  constructor(widget: Widget, index: number, descriptor: any, content: Widget | null) {
    this.widget = widget;
    this.index = index;
    this.descriptor = descriptor;
    this.content = content;
  }

  get title(): string {
    return textOf(this.descriptor.title);
  }

  get active(): boolean {
    return this.index === this.widget.activeIndex;
  }

  /** Clicks the tab header, making this the shown tab. */
  click(): void {
    this.widget.sendScoped({ TabActivated: { index: this.index } });
  }

  /** Clicks the tab's close affordance. */
  close(): void {
    this.widget.sendScoped({ TabClosed: { index: this.index } });
  }
}
exports.Tab = Tab;

/** One button of a toolbar strip, clicked through the engine's own hit test against the label as it painted. */
class ToolbarButton {
  harness: Harness;
  index: number;
  id: string;
  label: string;

  constructor(harness: Harness, index: number, id: string, label: string) {
    this.harness = harness;
    this.index = index;
    this.id = id;
    this.label = label;
  }

  click(): void {
    this.harness.clickText(this.label);
  }
}
exports.ToolbarButton = ToolbarButton;

/**
 * One node of the tree the app's last render produced: its wire `id`, its
 * `role` (the primitive kind it paints, or the layout kind it holds other
 * widgets in), the text it renders, and the actions a user can perform on
 * it.
 *
 * Read any descriptor field the app declared with `widget.get("field")`,
 * and the framework-owned state the app does *not* track (scroll offset,
 * active tab, split ratio, selection) with `widget.state`.
 */
class Widget {
  harness: Harness;
  role: string;
  descriptor: any;
  id: string | null;
  visible: boolean;
  children: Widget[] = [];
  /** Which tab of an enclosing `tabs` node holds this widget, if any. */
  tabIndex: number | null = null;

  constructor(harness: Harness, role: string, descriptor: any, id: string | null, visible: boolean) {
    this.harness = harness;
    this.role = role;
    this.descriptor = descriptor;
    this.id = id;
    this.visible = visible;
  }

  // --- reading -------------------------------------------------------------

  /** Everything this widget and its children render, space-separated. */
  get text(): string {
    const own = Array.isArray(this.descriptor) ? "" : textOf(this.descriptor);
    return [own, ...this.children.map((child) => child.text)].filter((part) => part !== "").join(" ");
  }

  /** The readable text of one descriptor field -- a status bar's `"left_segments"`, and so on. */
  textOf(field: string): string {
    return textOf(this.descriptor?.[field]);
  }

  /** One raw descriptor field, as the app declared it. */
  get(field: string): unknown {
    return this.descriptor?.[field];
  }

  /** The engine's own stored state for this widget id -- scroll offset, active tab, split ratio, selection. */
  get state(): WidgetState {
    return this.id === null ? EMPTY_STATE : this.harness.state(this.id);
  }

  /** How far this widget has scrolled, from the engine's own store. */
  get scrollOffset(): number {
    return this.state.scrollOffset ?? 0;
  }

  /**
   * A split's divider position as the engine resolved it -- which is where
   * an uncontrolled divider's dragged position actually lives, not in the
   * `ratio` the app keeps re-declaring.
   */
  get ratio(): number | null {
    return this.state.splitRatio ?? null;
  }

  // --- rows ----------------------------------------------------------------

  /** Every row this widget renders, in render order. */
  get rows(): Row[] {
    const rows = this.descriptor?.rows ?? this.descriptor?.items;
    if (rows === undefined) {
      throw new TypeError(`a ${JSON.stringify(this.role)} widget has no rows`);
    }
    return rows.map((row: unknown, i: number) => new Row(this, i, row));
  }

  /** Each row's rendered text, in render order. */
  get rowTexts(): string[] {
    return this.rows.map((row) => row.text);
  }

  /** The row matching a text substring, an explicit index, or a tree path. */
  row(selector: RowSelector): Row {
    const want = typeof selector === "string" ? { text: selector } : selector;
    const rows = this.rows;
    if (want.index !== undefined) {
      const row = rows[want.index];
      if (row === undefined) {
        throw new WidgetNotFoundError(`${this.role} ${this.id} has ${rows.length} rows, no index ${want.index}`);
      }
      return row;
    }
    if (want.path !== undefined) {
      const wanted = JSON.stringify([...want.path]);
      const row = rows.find((candidate) => JSON.stringify(candidate.path) === wanted);
      if (row === undefined) {
        throw new WidgetNotFoundError(`no row at path ${wanted} in ${this.role} ${this.id}`);
      }
      return row;
    }
    if (want.text === undefined) {
      throw new TypeError("row() needs one of text, index or path");
    }
    const row = rows.find((candidate) => candidate.text.includes(want.text as string));
    if (row === undefined) {
      throw new WidgetNotFoundError(
        `no row matching ${JSON.stringify(want.text)} in ${this.role} ${this.id}; rows are ${JSON.stringify(this.rowTexts)}`,
      );
    }
    return row;
  }

  /** The row the app rendered as selected, or `null` when nothing is. */
  get selectedRow(): Row | null {
    if (this.descriptor?.selected_path !== undefined) {
      const wanted = JSON.stringify(this.descriptor.selected_path);
      return this.rows.find((row) => JSON.stringify(row.path) === wanted) ?? null;
    }
    const selected = this.state.selectedIndex ?? this.descriptor?.selected_idx;
    if (selected === undefined || selected === null) {
      return null;
    }
    return this.rows[selected] ?? null;
  }

  // --- tabs ----------------------------------------------------------------

  /** Every tab of a `tabs`/`tab_bar` node, in render order. */
  get tabs(): Tab[] {
    const entries = this.descriptor?.tabs;
    if (entries === undefined) {
      throw new TypeError(`a ${JSON.stringify(this.role)} widget has no tabs`);
    }
    const contents = new Map<number, Widget>();
    for (const child of this.children) {
      if (child.tabIndex !== null) {
        contents.set(child.tabIndex, child);
      }
    }
    return entries.map((entry: unknown, i: number) => new Tab(this, i, entry, contents.get(i) ?? null));
  }

  /** Which tab is showing -- from the engine's own store, where an uncontrolled tab strip's active tab lives. */
  get activeIndex(): number {
    const stored = this.state.activeTab;
    if (stored !== undefined && stored !== null) {
      return stored;
    }
    const declared = this.descriptor?.selected;
    return declared === undefined || declared === null ? 0 : Number(declared);
  }

  get activeTab(): Tab | null {
    return this.tabs[this.activeIndex] ?? null;
  }

  // --- toolbar -------------------------------------------------------------

  /** Every action button of a toolbar strip (separators and spacers are skipped). */
  get buttons(): ToolbarButton[] {
    const entries = this.descriptor?.buttons;
    if (entries === undefined) {
      throw new TypeError(`a ${JSON.stringify(this.role)} widget has no buttons`);
    }
    const out: ToolbarButton[] = [];
    entries.forEach((entry: unknown, i: number) => {
      if (typeof entry !== "object" || entry === null) {
        return;
      }
      for (const payload of Object.values(entry)) {
        if (typeof payload === "object" && payload !== null && "label" in payload) {
          const action = payload as { id?: string; label: string };
          out.push(new ToolbarButton(this.harness, i, action.id ?? "", action.label));
        }
      }
    });
    return out;
  }

  get buttonLabels(): string[] {
    return this.buttons.map((button) => button.label);
  }

  /** The toolbar button whose label contains `label`. */
  button(label: string): ToolbarButton {
    const found = this.buttons.find((button) => button.label.includes(label));
    if (found === undefined) {
      throw new WidgetNotFoundError(
        `no toolbar button matching ${JSON.stringify(label)}; labels are ${JSON.stringify(this.buttonLabels)}`,
      );
    }
    return found;
  }

  // --- acting ---------------------------------------------------------------

  /**
   * Clicks this widget through the engine's own hit test, by finding what
   * it painted on the frame. Defined for widgets a user clicks as a whole
   * (a button); click a row or a tab through `row(...)`/`tabs[...]`.
   */
  click(): void {
    if (this.role !== "button") {
      throw new TypeError(
        `a ${JSON.stringify(this.role)} widget is not clicked as a whole -- use .row(...)/.tabs[...] for a` +
          " part of it, or harness.clickText(...) for an exact label",
      );
    }
    const label = this.textOf("label") || this.text;
    if (label === "") {
      throw new WidgetNotFoundError(`button ${this.id} renders no label to click`);
    }
    this.harness.clickText(label);
  }

  /** Moves focus here, the way Tab does. */
  focus(): void {
    if (this.id === null) {
      throw new TypeError(`a ${JSON.stringify(this.role)} widget has no id to focus`);
    }
    this.harness.focus(this.id);
  }

  /** Presses a key *at this widget*: the focus-scoped shape a real backend delivers to a focused widget. */
  press(key: string, mods: Modifiers = {}): void {
    const scope = EVENT_SCOPES[this.role];
    if (scope === undefined || this.id === null) {
      throw new TypeError(`a ${JSON.stringify(this.role)} widget takes no focus-scoped keys`);
    }
    this.harness.dispatch({
      [scope]: [this.id, { KeyPressed: { key: keyName(keyPayload(key)), modifiers: wireModifiers(mods) } }],
    });
  }

  /** Drags a split's divider to `ratio` of the split's own extent. */
  dragDivider(ratio: number, index = 0): void {
    if ((this.role !== "split" && this.role !== "split_tree") || this.id === null) {
      throw new TypeError(`a ${JSON.stringify(this.role)} widget has no divider`);
    }
    this.harness.dispatch({ SplitDividerDragged: { tree: this.id, split_index: index, new_ratio: ratio } });
  }

  sendScoped(activation: unknown): void {
    const scope = EVENT_SCOPES[this.role];
    if (scope === undefined || this.id === null) {
      throw new TypeError(`a ${JSON.stringify(this.role)} widget delivers no scoped events`);
    }
    this.harness.dispatch({ [scope]: [this.id, activation] });
  }

  // --- searching this subtree -----------------------------------------------

  /** This widget and every descendant, in render order. */
  walk(includeHidden = false): Widget[] {
    if (!includeHidden && !this.visible) {
      return [];
    }
    const out: Widget[] = [this];
    for (const child of this.children) {
      out.push(...child.walk(includeHidden));
    }
    return out;
  }

  /** `Harness.find`, restricted to this widget's own subtree. */
  find(query: Query = {}): Widget {
    return firstMatch(this.walk(query.includeHidden === true), query, this);
  }

  /** `Harness.findAll`, restricted to this widget's own subtree. */
  findAll(query: Query = {}): Widget[] {
    return matches(this.walk(query.includeHidden === true), query);
  }
}
exports.Widget = Widget;

function matches(widgets: Widget[], query: Query): Widget[] {
  return widgets.filter((widget) => {
    if (query.id !== undefined && widget.id !== query.id) {
      return false;
    }
    if (query.role !== undefined && widget.role !== query.role) {
      return false;
    }
    if (query.text !== undefined && !widget.text.includes(query.text)) {
      return false;
    }
    return true;
  });
}

function firstMatch(widgets: Widget[], query: Query, scope: Widget | null): Widget {
  const found = matches(widgets, query);
  if (found.length > 0) {
    return found[0] as Widget;
  }
  const terms = Object.entries(query)
    .filter(([key, value]) => key !== "includeHidden" && value !== undefined)
    .map(([key, value]) => `${key}=${JSON.stringify(value)}`)
    .join(", ");
  const where = scope === null ? "" : ` under <${scope.role} ${scope.id}>`;
  throw new WidgetNotFoundError(`no widget matching ${terms === "" ? "<anything>" : terms}${where}`);
}

/** A running app, queried and driven the way a user drives it. Build one with `harness(app)`. */
class Harness {
  app: any;
  qui: any;
  private renderCallback: () => unknown;
  private wire: unknown = null;
  private rootWidget: Widget | null = null;

  constructor(app: any, options: HarnessOptions = {}) {
    let render = options.render;
    if (app.qui === undefined) {
      this.qui = app;
    } else {
      this.qui = app.qui;
      if (render === undefined) {
        render = () => app._render();
      }
    }
    if (render === undefined) {
      throw new TypeError(
        "harness(QuiApp) needs the same render callable the app was given ({ render }); an `App` supplies its own",
      );
    }
    this.app = app;
    this.renderCallback = render;
    // Stands in for the app's own render callback so every real paint hands
    // this harness the wire tree the app just produced -- queries then read
    // exactly what painted, with no second render pass.
    this.qui.setRender(() => {
      this.wire = this.renderCallback();
      this.rootWidget = null;
      return this.wire;
    });
    this.refresh();
  }

  // --- the frame ------------------------------------------------------------

  private invalidate(): void {
    this.wire = null;
    this.rootWidget = null;
  }

  /** Repaints and re-reads the widget tree. Every action here does this already. */
  refresh(): void {
    this.qui.screen();
    this.rootWidget = null;
  }

  /** The rendered frame as text, exactly as the terminal shows it. */
  screen(): string {
    return this.qui.screen();
  }

  screenLines(): string[] {
    return this.screen().split("\n");
  }

  /** Whether `text` is somewhere on the rendered frame. */
  contains(text: string): boolean {
    return this.qui.screenContains(text);
  }

  /** Throws, printing the whole frame, unless every `texts` is on it. */
  assertText(...texts: string[]): void {
    const missing = texts.filter((text) => !this.contains(text));
    if (missing.length > 0) {
      throw new Error(`not on screen: ${JSON.stringify(missing)}\n--- screen ---\n${this.screen()}`);
    }
  }

  /** Throws, printing the whole frame, if any `texts` is on it. */
  assertNotText(...texts: string[]): void {
    const present = texts.filter((text) => this.contains(text));
    if (present.length > 0) {
      throw new Error(`unexpectedly on screen: ${JSON.stringify(present)}\n--- screen ---\n${this.screen()}`);
    }
  }

  /** The engine's own stored state for a widget id, all-`null` when the engine tracks nothing for it yet. */
  state(widgetId: string): WidgetState {
    return this.qui.widgetState(widgetId) ?? EMPTY_STATE;
  }

  // --- querying -------------------------------------------------------------

  /** The outermost widget of the last rendered frame. */
  get root(): Widget {
    if (this.rootWidget === null) {
      if (this.wire === null) {
        this.refresh();
      }
      this.rootWidget = buildTree(this, this.wire);
    }
    return this.rootWidget;
  }

  /** Every widget of the last rendered frame, in render order. */
  widgets(includeHidden = false): Widget[] {
    return this.root.walk(includeHidden);
  }

  /** The first widget matching every given term, in render order. */
  find(query: Query = {}): Widget {
    return firstMatch(this.root.walk(query.includeHidden === true), query, null);
  }

  /** Every widget matching the given terms, in render order. */
  findAll(query: Query = {}): Widget[] {
    return matches(this.root.walk(query.includeHidden === true), query);
  }

  /** `find`, but `null` instead of throwing when nothing matches. */
  query(query: Query = {}): Widget | null {
    return this.findAll(query)[0] ?? null;
  }

  /** Whether any widget matches the given terms. */
  exists(query: Query = {}): boolean {
    return this.findAll(query).length > 0;
  }

  // --- acting ---------------------------------------------------------------

  /** Pushes one raw `UiEvent` and steps the app -- the escape hatch for a shape this harness has no verb for. */
  dispatch(event: unknown): void {
    this.qui.pushEvent(event);
    this.qui.step();
    this.invalidate();
  }

  /** Presses a key with nothing focus-scoped about it -- the plain top-level `KeyPressed` a terminal delivers. */
  press(key: string, mods: Modifiers = {}): void {
    this.dispatch({ KeyPressed: { key: keyPayload(key), modifiers: wireModifiers(mods), repeat: false } });
  }

  /** Types `text` one character at a time. */
  type(text: string): void {
    for (const char of text) {
      this.press(char);
    }
  }

  /** Moves focus to a widget (or clears it with `null`). */
  focus(target: string | Widget | null): void {
    const id = target instanceof Widget ? target.id : target;
    this.dispatch({ FocusChanged: id });
  }

  /** Clicks one cell of the frame; the engine hit-tests it against whatever actually painted there. */
  clickAt(x: number, y: number): void {
    const position = { x, y };
    this.dispatch({ MouseDown: { button: "Left", position, modifiers: {} } });
    this.dispatch({ MouseUp: { button: "Left", position } });
  }

  /** Clicks the middle of where `text` painted on the frame, with no coordinate in the test. */
  clickText(text: string, nth = 0): void {
    const found = this.locateAll(text);
    const at = found[nth];
    if (at === undefined) {
      throw new WidgetNotFoundError(
        `${JSON.stringify(text)} appears ${found.length} time(s) on screen, no occurrence ${nth}` +
          `\n--- screen ---\n${this.screen()}`,
      );
    }
    this.clickAt(at[0], at[1]);
  }

  /** The centre cell of every place `text` painted, in reading order. */
  locateAll(text: string): Array<[number, number]> {
    const out: Array<[number, number]> = [];
    this.screenLines().forEach((line, y) => {
      let start = line.indexOf(text);
      while (start !== -1) {
        out.push([start + text.length / 2, y]);
        start = line.indexOf(text, start + 1);
      }
    });
    return out;
  }

  /** Pumps one event-loop iteration without pushing anything. */
  step(): boolean {
    const result = this.qui.step();
    this.invalidate();
    return result;
  }

  /**
   * Releases what the wrapped app holds outside this process's own
   * control -- today, the hot-reload watcher timer, so a test that enabled
   * it does not leak the timer into the rest of the run. Call it from the
   * test runner's own teardown (`node:test`'s `after`). Safe to call more
   * than once.
   */
  close(): void {
    this.app.disableHotReload?.();
  }
}
exports.Harness = Harness;

/**
 * Wraps a running app for querying and driving from a test. Pass an `App`
 * from `./app` on its own, or a plain `QuiApp` together with the same
 * `render` callable it was constructed with.
 */
function harness(app: unknown, options: HarnessOptions = {}): Harness {
  return new Harness(app, options);
}
exports.harness = harness;

// --- building the widget tree out of one rendered frame ----------------------

function roleOf(node: Record<string, unknown>): string | null {
  const roles = Object.keys(node).filter((key) => !META_KEYS.has(key));
  return roles.length === 1 ? (roles[0] as string) : null;
}

function buildTree(harnessRef: Harness, wire: unknown): Widget {
  const root = new Widget(harnessRef, "frame", {}, null, true);
  appendNode(harnessRef, root, wire, true);
  return root.children.length === 1 ? (root.children[0] as Widget) : root;
}

function appendNode(harnessRef: Harness, parent: Widget, node: unknown, visible: boolean): void {
  if (node === null || node === undefined) {
    return;
  }
  if (Array.isArray(node)) {
    for (const item of node) {
      appendNode(harnessRef, parent, item, visible);
    }
    return;
  }
  if (typeof node !== "object") {
    return;
  }
  const record = node as Record<string, any>;
  const role = roleOf(record);
  if (role === null) {
    return;
  }
  const payload = record[role];
  const id = (typeof payload === "object" && payload !== null ? payload.id : undefined) ?? record.id ?? null;
  const widget = new Widget(harnessRef, role, payload, id, visible);
  parent.children.push(widget);
  if (CONTAINER_ROLES.has(role)) {
    appendChildren(harnessRef, widget, role, payload, visible);
  }
}

function appendChildren(harnessRef: Harness, widget: Widget, role: string, payload: any, visible: boolean): void {
  if (role === "row" || role === "column") {
    appendNode(harnessRef, widget, payload, visible);
  } else if (role === "split") {
    appendNode(harnessRef, widget, payload.first, visible);
    appendNode(harnessRef, widget, payload.second, visible);
  } else if (role === "panel" || role === "sidebar_panel") {
    if (payload.toolbar !== undefined && payload.toolbar !== null) {
      appendNode(harnessRef, widget, { toolbar: payload.toolbar }, visible);
    }
    appendNode(harnessRef, widget, payload.content, visible);
  } else if (role === "tabs") {
    const active = widget.activeIndex;
    (payload.tabs ?? []).forEach((entry: any, index: number) => {
      const before = widget.children.length;
      appendNode(harnessRef, widget, entry.content, visible && index === active);
      for (const child of widget.children.slice(before)) {
        child.tabIndex = index;
      }
    });
  } else if (role === "split_tree") {
    appendSplitTree(harnessRef, widget, payload.root, visible);
  }
}

function appendSplitTree(harnessRef: Harness, widget: Widget, node: any, visible: boolean): void {
  if (typeof node !== "object" || node === null) {
    return;
  }
  if (node.leaf !== undefined) {
    appendNode(harnessRef, widget, node.leaf, visible);
    return;
  }
  if (typeof node.split === "object" && node.split !== null) {
    appendSplitTree(harnessRef, widget, node.split.first, visible);
    appendSplitTree(harnessRef, widget, node.split.second, visible);
  }
}
