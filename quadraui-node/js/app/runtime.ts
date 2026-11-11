// `App`: wraps a `QuiApp` so `view()` returns `Node`/`Component` trees
// instead of raw wire objects, and event dispatch goes through the `on*`
// callbacks those trees carried on their last render instead of one big
// `handle(event)` switch.
//
// Walking the tree (`walk`) does three things in the same pass: resolves
// every `Node`'s wire id (explicit `id`, or a stable auto-id derived from
// the node's position in the tree -- see `autoId`), expands every
// `Component` into whatever its own `view(state)` returns (creating that
// component's state slot from `initialState()` the first time its
// `key`/position is seen, and threading the *same* object back in on every
// later render), and records each node's `callbacks` into a fresh
// id -> {eventName: callback} registry. `App.handle` looks a dispatched
// event's own widget id up in that registry (rebuilt by the render this
// event's own `step()` just triggered) and calls whichever callback matches
// the event's shape -- see `CONTAINER_EVENT_MAP` for the event-shape ->
// callback-name table, and `Button`'s own doc (`./nodes.ts`) for why its
// `onClick` needs its own two-path dispatch instead of a table entry.
// `App.render` swaps in only the component-state keys seen during that
// same pass, so a component that stops being rendered (a closed tab, a
// removed list row) drops its state slot instead of leaving it behind for
// a future, unrelated component to inherit.
//
// A callback mutating app/component state needs no explicit "please
// redraw" call: `QuiApp` already repaints once per `step()` whenever a
// render callback is registered (see `../index.ts`'s own `run`/`pump`
// doc), so the mutation a callback makes is already visible by the time
// that same `step()` returns.
//
// Runtime exports go through `exports.X = ...` rather than `export`, the
// same reason `../index.ts`'s own header comment gives.

import type { Node, Renderable, ComponentLike } from "./nodes";
import type { WidgetState, RunHandle } from "../index";

const { QuiApp, run } = require("../index.ts");

type AnyCallback = (...args: unknown[]) => void;
type Registry = Record<string, Record<string, AnyCallback>>;
type ComponentStates = Map<unknown, Record<string, unknown>>;

/**
 * A stable id for a `Node` that didn't give its own: deterministic for a
 * given structural position, as long as the shape of the tree above it
 * (how many siblings, which slot names) doesn't change between renders --
 * the same assumption an array index or object key relies on anywhere
 * else.
 */
function autoId(path: ReadonlyArray<string | number>): string {
  return "auto:" + path.join(".");
}

function isNode(value: unknown): value is Node {
  return (
    typeof value === "object" &&
    value !== null &&
    typeof (value as Node).build === "function" &&
    typeof (value as Node).children === "object"
  );
}

function isComponentLike(value: unknown): value is ComponentLike {
  return (
    typeof value === "object" &&
    value !== null &&
    typeof (value as ComponentLike).view === "function" &&
    typeof (value as ComponentLike).initialState === "function"
  );
}

function walk(
  value: Renderable,
  path: ReadonlyArray<string | number>,
  registry: Registry,
  oldStates: ComponentStates,
  newStates: ComponentStates,
): unknown {
  if (value === null || value === undefined) {
    return null;
  }
  if (Array.isArray(value)) {
    return value.map((item, i) => walk(item, [...path, i], registry, oldStates, newStates));
  }
  if (isComponentLike(value)) {
    const stateKey = value.key !== undefined ? value.key : JSON.stringify(path);
    let state = newStates.get(stateKey);
    if (state === undefined) {
      state = oldStates.get(stateKey) ?? value.initialState();
      newStates.set(stateKey, state);
    }
    const child = value.view(state);
    return walk(child, [...path, "component"], registry, oldStates, newStates);
  }
  if (isNode(value)) {
    const nodeId = value.id ?? autoId(path);
    const childrenWire: Record<string, unknown> = {};
    for (const [slot, child] of Object.entries(value.children)) {
      childrenWire[slot] = walk(child, [...path, slot], registry, oldStates, newStates);
    }
    if (Object.keys(value.callbacks).length > 0) {
      registry[nodeId] = value.callbacks;
    }
    return value.build(nodeId, childrenWire);
  }
  if (typeof value === "object") {
    return value;
  }
  throw new TypeError(`not a renderable node: ${JSON.stringify(value)}`);
}

/**
 * Canonical name of a raw `Key`: a plain top-level `KeyPressed` carries a
 * tagged object (`{Named: "Down"}`/`{Char: "o"}`/`{Named: {F: 2}}`); a
 * focus-scoped leaf event carries the already-stringified name. Both
 * collapse to the name (a function key becomes `"F2"`).
 */
function rawKeyName(key: unknown): string | null {
  if (typeof key === "string") {
    return key;
  }
  if (typeof key !== "object" || key === null) {
    return null;
  }
  const obj = key as Record<string, unknown>;
  if (typeof obj.Char === "string") {
    return obj.Char;
  }
  const named = obj.Named;
  if (named === undefined || named === null) {
    return null;
  }
  if (typeof named === "object") {
    return `F${(named as Record<string, unknown>).F}`;
  }
  return named as string;
}

/**
 * The canonical key name out of a key event in any shape a backend
 * delivers one: a plain top-level `KeyPressed`, or a focus-scoped
 * `{"<Tree/List/TabBar/...>": [id, {KeyPressed: ...}]}` -- the focus
 * manager rewrites a focused leaf's own keys into that shape before this
 * layer ever sees them. Returns `null` for anything else, including a
 * bare-string event.
 */
function keyName(event: unknown): string | null {
  if (typeof event !== "object" || event === null) {
    return null;
  }
  const obj = event as Record<string, unknown>;
  if ("KeyPressed" in obj) {
    const payload = obj.KeyPressed as { key: unknown };
    return rawKeyName(payload.key);
  }
  for (const inner of Object.values(obj)) {
    if (Array.isArray(inner) && inner.length === 2) {
      const [, activation] = inner as [unknown, unknown];
      if (activation && typeof activation === "object" && "KeyPressed" in (activation as object)) {
        return rawKeyName((activation as Record<string, { key: unknown }>).KeyPressed.key);
      }
    }
  }
  return null;
}
exports.keyName = keyName;

const ACTIVATE_KEY_NAMES = new Set(["Enter", " "]);

function isActivateKey(name: string | null): boolean {
  return name !== null && ACTIVATE_KEY_NAMES.has(name);
}

/**
 * Maps a container-event wrapper's own key (`event.Tree`, `event.DataTable`,
 * ...) to `{variantName: [callbackName, argsFromPayload]}` -- the shape
 * every typed leaf node constructor's own `on*` option documents.
 */
const CONTAINER_EVENT_MAP: Record<
  string,
  Record<string, [string, (payload: Record<string, unknown>) => unknown[]]>
> = {
  Tree: {
    RowClicked: ["on_row_click", (p) => [p.path]],
    RowToggleExpand: ["on_toggle_expand", (p) => [p.path]],
    RowDoubleClicked: ["on_activate", (p) => [p.path]],
    SelectionChanged: ["on_select", (p) => [p.path]],
  },
  DataTable: {
    RowDoubleClicked: ["on_activate", (p) => [p.idx]],
    RowActivated: ["on_activate", (p) => [p.idx]],
    RowSelected: ["on_select", (p) => [p.idx]],
    HeaderClicked: ["on_header_click", (p) => [p.col]],
  },
  TabBar: {
    TabClosed: ["on_close", (p) => [p.index]],
    TabActivated: ["on_activate", (p) => [p.index]],
  },
  StatusBar: {
    SegmentClicked: ["on_segment_click", (p) => [p.id]],
  },
};

export interface AppOptions {
  view(): Renderable;
  /**
   * Called for every event this layer's own dispatch doesn't claim (a
   * global key, an unrecognized shape, ...) -- the escape hatch a
   * keyboard-driven app (arrow keys moving a cursor with no single widget
   * "owning" them) still needs; see {@link keyName} for pulling a key name
   * out of whatever shape it arrived in.
   */
  onEvent?(event: unknown): void;
}

/**
 * A `QuiApp` driven by a `view()` callback returning `Node`/`Component`
 * trees, with events dispatched to the `on*` callbacks those trees
 * carried on their last render rather than a hand-written `handle`.
 */
class App {
  qui: InstanceType<typeof QuiApp>;
  /** A `QuiAppActions` handle -- see `../index.ts`'s own facade doc for why this package doesn't type-export that native class. */
  actions: unknown;
  /**
   * The id -> {eventName: callback} registry the most recent render built
   * -- leading-underscore, not actually private: an own test (or a REPL
   * poking at id-stability/dispatch) reads it directly the same way
   * `quadraui.app`'s own Python test suite reads `App._registry`.
   */
  _registry: Registry = {};
  _componentStates: ComponentStates = new Map();
  _focusedId: string | null = null;
  private _view: () => Renderable;
  private _onEvent: ((event: unknown) => void) | undefined;

  constructor(width: number, height: number, options: AppOptions) {
    this._view = options.view;
    this._onEvent = options.onEvent;
    this.qui = new QuiApp(width, height);
    this.qui.setRender(() => this._render());
    this.qui.setHandler((event: unknown) => this._handle(event));
    this.actions = this.qui.actions();
    // Forces one real native paint immediately, mirroring `quadraui-py`'s
    // own `App.__init__`: its `QuiApp` constructor takes `render` directly
    // and renders eagerly, so a callback is already dispatchable -- and a
    // `MouseDown` is already hit-testable against real painted geometry --
    // before the first explicit `step()`/`screen()` call. `setRender`
    // above only registers the callback; nothing has invoked it yet.
    // `qui.screen()` (not a bare JS-side `this._render()`) is what's
    // needed here: it drives the native engine's own paint pass, which is
    // also what records where each widget actually painted for a later
    // `MouseDown`'s hit-test to find -- a JS-side-only priming call would
    // populate `_registry` but leave that geometry unknown.
    this.qui.screen();
  }

  /**
   * Replaces the global `onEvent` fallback given at construction (or
   * clears it, with `undefined`) -- the public way to compose/swap the
   * escape-hatch handler after the fact, instead of assigning the private
   * field directly.
   */
  setOnEvent(onEvent: ((event: unknown) => void) | undefined): void {
    this._onEvent = onEvent;
  }

  // --- render / dispatch ---------------------------------------------------

  /** Re-runs `view()` through `walk`, rebuilding `_registry`/`_componentStates`, and returns the wire tree. Exposed (not actually private) for the same reason `_registry` is. */
  _render(): unknown {
    const registry: Registry = {};
    const newStates: ComponentStates = new Map();
    const wire = walk(this._view(), [], registry, this._componentStates, newStates);
    this._registry = registry;
    this._componentStates = newStates;
    return wire;
  }

  /** Dispatches one event to a node callback, falling back to `onEvent`. Exposed for driving a focus-scoped event straight through in a test, same as `_render`. */
  _handle(event: unknown): void {
    if (typeof event === "object" && event !== null && "FocusChanged" in (event as object)) {
      this._focusedId = (event as Record<string, unknown>).FocusChanged as string | null;
    }
    if (this._dispatch(event)) {
      return;
    }
    if (this._onEvent !== undefined) {
      this._onEvent(event);
    }
  }

  /** Tries every node-callback dispatch path in turn; `true` means a callback ran (so `_handle` must not also call `onEvent`). */
  private _dispatch(event: unknown): boolean {
    if (typeof event !== "object" || event === null) {
      return false;
    }
    const obj = event as Record<string, unknown>;

    if ("KeyPressed" in obj) {
      const payload = obj.KeyPressed as { key: unknown };
      const name = rawKeyName(payload.key);
      if (this._focusedId !== null && isActivateKey(name)) {
        const callback = this._registry[this._focusedId]?.on_click;
        if (callback !== undefined) {
          callback();
          return true;
        }
      }
      return false;
    }

    if ("MouseDown" in obj) {
      const payload = obj.MouseDown as { widget: string | null };
      const hit = obj.hit as Record<string, unknown> | undefined;
      if (payload.widget && hit !== undefined && hit.kind === "button_body") {
        const callback = this._registry[payload.widget]?.on_click;
        if (callback !== undefined) {
          callback();
          return true;
        }
      }
      return false;
    }

    for (const [container, variants] of Object.entries(CONTAINER_EVENT_MAP)) {
      if (!(container in obj)) {
        continue;
      }
      const [widgetId, activation] = obj[container] as [string, unknown];
      if (typeof activation !== "object" || activation === null) {
        return false;
      }
      const activationObj = activation as Record<string, unknown>;
      for (const [variantName, [callbackName, extractArgs]] of Object.entries(variants)) {
        if (variantName in activationObj) {
          const callback = this._registry[widgetId]?.[callbackName];
          if (callback !== undefined) {
            callback(...extractArgs(activationObj[variantName] as Record<string, unknown>));
            return true;
          }
          return false;
        }
      }
      return false;
    }

    return false;
  }

  // --- thin passthroughs to the wrapped QuiApp -----------------------------

  step(): boolean {
    return this.qui.step();
  }

  run(maxSteps?: number): boolean {
    return this.qui.run(maxSteps);
  }

  pushEvent(event: unknown): void {
    this.qui.pushEvent(event);
  }

  pushKeyEvent(ch: number): void {
    this.qui.pushKeyEvent(ch);
  }

  screen(): string {
    return this.qui.screen();
  }

  screenContains(needle: string): boolean {
    return this.qui.screenContains(needle);
  }

  widgetState(id: string): WidgetState | null {
    return this.qui.widgetState(id);
  }

  startTui(mouse?: boolean): void {
    this.qui.startTui(mouse);
  }

  startNative(title?: string, appId?: string, iconName?: string, singleInstance?: boolean): void {
    this.qui.startNative(title, appId, iconName, singleInstance);
  }

  /**
   * Starts a real-terminal session and drives it wake-driven via
   * `../index.ts`'s own `run` helper, returning the stop handle it returns.
   * Non-blocking, unlike `quadraui-py`'s own `run_tui` -- Node's run loop is
   * cooperative, not something a library call should block synchronously
   * on; the caller's own script keeps running (and the process stays
   * alive) for as long as `run`'s wake-driven loop has work, until the
   * handler quits the app or the returned handle's `stop()` is called.
   */
  runTui(mouse?: boolean): RunHandle {
    this.startTui(mouse);
    return run(this);
  }

  /** `runTui`'s `startNative` counterpart -- same non-blocking, wake-driven loop. */
  runNative(title?: string, appId?: string, iconName?: string, singleInstance?: boolean): RunHandle {
    this.startNative(title, appId, iconName, singleInstance);
    return run(this);
  }

  stopLive(): boolean {
    return this.qui.stopLive();
  }

  armWakeNotify(callback: () => void): void {
    this.qui.armWakeNotify(callback);
  }

  disarmWakeNotify(): void {
    this.qui.disarmWakeNotify();
  }
}
exports.App = App;
