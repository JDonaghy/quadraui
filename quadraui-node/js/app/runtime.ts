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
import type { Poller, ReboundPrototypes, Watcher } from "./hotReload";

const { QuiApp, run } = require("../index.ts");
const hr = require("./hotReload.ts");

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

/**
 * The state a surviving slot carries into the first render after a hot
 * reload: the component's own `migrateState` if it defines one, otherwise
 * every existing value plus any key the current `initialState()` added.
 */
function migrateComponentState(
  component: ComponentLike,
  state: Record<string, unknown>,
): Record<string, unknown> {
  if (component.migrateState !== undefined) {
    return component.migrateState(state);
  }
  for (const [name, value] of Object.entries(component.initialState())) {
    if (!(name in state)) {
      state[name] = value;
    }
  }
  return state;
}

function walk(
  value: Renderable,
  path: ReadonlyArray<string | number>,
  registry: Registry,
  oldStates: ComponentStates,
  newStates: ComponentStates,
  migrate = false,
): unknown {
  if (value === null || value === undefined) {
    return null;
  }
  if (Array.isArray(value)) {
    return value.map((item, i) => walk(item, [...path, i], registry, oldStates, newStates, migrate));
  }
  if (isComponentLike(value)) {
    const stateKey = value.key !== undefined ? value.key : JSON.stringify(path);
    let state = newStates.get(stateKey);
    if (state === undefined) {
      const surviving = oldStates.get(stateKey);
      if (surviving === undefined) {
        state = value.initialState();
      } else {
        state = migrate ? migrateComponentState(value, surviving) : surviving;
      }
      newStates.set(stateKey, state);
    }
    const child = value.view(state);
    return walk(child, [...path, "component"], registry, oldStates, newStates, migrate);
  }
  if (isNode(value)) {
    const nodeId = value.id ?? autoId(path);
    const childrenWire: Record<string, unknown> = {};
    for (const [slot, child] of Object.entries(value.children)) {
      childrenWire[slot] = walk(child, [...path, slot], registry, oldStates, newStates, migrate);
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
  /**
   * Builds this frame's tree. `Renderable` permits `null`/`undefined` for
   * a child slot (see that type's own doc), but returning either one
   * here at the top level still throws `InvalidDescriptorError` -- the
   * native `QuiApp.setRender` callback this wraps requires a plain
   * object, the same gate `quadraui-py`'s own `render` callback has
   * always had. Return `{}` (or a `Node`/`Component` tree) for an empty
   * frame instead.
   */
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

export interface HotReloadOptions {
  /**
   * How often (milliseconds) to stat the watched files; `null` checks
   * nothing until {@link App.reloadIfChanged} is called by hand.
   */
  interval?: number | null;
  /** Runs after every successful reload, before the first render with the new code. */
  onReload?(app: App): void;
  /**
   * Objects whose class lives in a watched module: each keeps its identity
   * and its own properties while its prototype is re-pointed at the
   * reloaded class of the same name, so its methods become the new code.
   */
  preserve?: ReadonlyArray<object>;
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
  /** The exception a failed reload recorded, or `null` after a successful one. */
  reloadError: unknown = null;
  /** How many reloads have succeeded and rendered. */
  reloadCount = 0;
  private _view: () => Renderable;
  private _onEvent: ((event: unknown) => void) | undefined;
  private _watcher: Watcher | null = null;
  private _poller: Poller | null = null;
  private _onReload: ((app: App) => void) | undefined;
  private _preserve: ReadonlyArray<object> = [];
  private _rollback: (() => void) | null = null;
  private _migrateNextRender = false;

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

  // --- hot reload -----------------------------------------------------------

  /**
   * Watches `modules` (resolved file paths, re-required in the order
   * given -- list a dependency before the module importing from it) and
   * re-requires them when their files change, keeping component,
   * app-object and widget state; see `./hotReload.ts` for exactly what
   * survives, and for why the view must call *through* a `preserve`d
   * object or the module exports rather than capture a function.
   *
   * Normally written `app.enableHotReload([require.resolve("./view.ts")],
   * {preserve: [notes]})` so the caller's own module resolution applies.
   * With `interval` an unref'd timer stats the files and wakes the event
   * loop on a change, so a live `runTui()`/`runNative()` session picks an
   * edit up without any input; with `interval: null` nothing is checked
   * until `reloadIfChanged()` is called. Calling this again replaces the
   * watch list.
   */
  enableHotReload(modules: ReadonlyArray<string>, options: HotReloadOptions = {}): void {
    this.disableHotReload();
    const paths = hr.resolveModules(modules);
    this._watcher = new hr.ModuleWatcher(paths);
    this._onReload = options.onReload;
    this._preserve = options.preserve ?? [];
    const interval = options.interval === undefined ? 500 : options.interval;
    if (interval !== null) {
      const waker = this.qui.waker();
      this._poller = new hr.PollTimer(this._watcher, interval, () => waker.post(hr.HOT_RELOAD_WAKE));
    }
  }

  /** Stops watching (and stops the watcher timer, if any). */
  disableHotReload(): void {
    this._poller?.stop();
    this._poller = null;
    this._watcher = null;
  }

  /**
   * Reloads the watched modules if any file changed since the last check;
   * `true` when the re-require succeeded, with the same "not yet
   * rendered" caveat {@link App.reload} documents. Needs
   * `enableHotReload()` first.
   */
  reloadIfChanged(): boolean {
    if (this._watcher === null) {
      throw new Error("enableHotReload() has not been called");
    }
    if (this._watcher.changed().length === 0) {
      return false;
    }
    return this.reload();
  }

  /**
   * Re-requires every watched module now, whether or not it changed, and
   * re-points every `preserve`d object's prototype at the reloaded class
   * of the same name. On failure the previous code stays in place, the
   * exception is stored on `reloadError`, and this returns `false`.
   *
   * `true` means the re-require and the prototype swaps succeeded -- *not*
   * that the new code has rendered, since rendering is driven separately
   * (the next `step()`). A view that throws on that first render is rolled
   * back there instead: `reloadError` becomes that exception and
   * `reloadCount` goes back down, after this call has already returned
   * `true`. Read `reloadError` after the next render, not this return
   * value, to know a reload stuck.
   */
  reload(): boolean {
    if (this._watcher === null) {
      throw new Error("enableHotReload() has not been called");
    }
    const paths = this._watcher.paths;
    this._watcher.snapshot();
    let saved;
    try {
      saved = hr.reloadModules(paths);
    } catch (error) {
      // A broken edit must not take the app down.
      this.reloadError = error;
      return false;
    }
    const rebound: ReboundPrototypes = [];
    const rollback = () => {
      hr.restorePrototypes(rebound);
      hr.restoreModules(saved);
      this._migrateNextRender = false;
    };
    try {
      hr.repointPrototypes(this._preserve, paths, rebound);
      this._onReload?.(this);
    } catch (error) {
      rollback();
      this.reloadError = error;
      return false;
    }
    this._rollback = rollback;
    this._migrateNextRender = true;
    this.reloadError = null;
    this.reloadCount += 1;
    return true;
  }

  // --- render / dispatch ---------------------------------------------------

  /** Re-runs `view()` through `walk`, rebuilding `_registry`/`_componentStates`, and returns the wire tree. Exposed (not actually private) for the same reason `_registry` is. */
  _render(): unknown {
    if (this._rollback === null) {
      return this._renderOnce();
    }
    // The first render with freshly reloaded code: if it throws, put the
    // last good code back and render that instead, so a broken edit never
    // takes the running app down.
    const rollback = this._rollback;
    this._rollback = null;
    try {
      return this._renderOnce();
    } catch (error) {
      rollback();
      this.reloadError = error;
      this.reloadCount -= 1;
      return this._renderOnce();
    }
  }

  private _renderOnce(): unknown {
    const registry: Registry = {};
    const newStates: ComponentStates = new Map();
    const migrate = this._migrateNextRender;
    this._migrateNextRender = false;
    let wire;
    try {
      wire = walk(this._view(), [], registry, this._componentStates, newStates, migrate);
    } catch (error) {
      this._migrateNextRender = migrate;
      throw error;
    }
    this._registry = registry;
    this._componentStates = newStates;
    return wire;
  }

  /** Dispatches one event to a node callback, falling back to `onEvent`. Exposed for driving a focus-scoped event straight through in a test, same as `_render`. */
  _handle(event: unknown): void {
    if (
      typeof event === "object" &&
      event !== null &&
      (event as Record<string, unknown>).User === hr.HOT_RELOAD_WAKE &&
      this._watcher !== null
    ) {
      this.reloadIfChanged();
      return;
    }
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

  /**
   * The `{"Form": [id, {"ButtonClicked": ...}]}` half of `_dispatch`: the
   * semantic activation a `button` leaf emits for *both* a pointer click on
   * its body and an Enter/Space while it holds keyboard focus.
   *
   * Only the keyboard-originated one runs `on_click` here. The
   * pointer-originated one carries the same `hit` dict as the raw `MouseDown`
   * that follows it, and that raw event is what `_dispatch` fires `on_click`
   * from -- keying the callback off the press keeps one `on_click` per press,
   * where claiming every semantic activation would fire twice for a fast
   * double-click (whose second press arrives as `DoubleClick`, carrying its
   * own activation). Either way the semantic event is claimed rather than
   * left to fall through to `onEvent`, so a button's own activation never
   * reaches the global escape hatch.
   *
   * An id this layer registered no `on_click` for is not one of its `Button`
   * nodes (an embedded `form` field's button, say), so its event falls
   * through untouched.
   */
  private _dispatchButtonClicked(obj: Record<string, unknown>): boolean {
    const inner = obj.Form;
    if (!Array.isArray(inner) || inner.length !== 2) {
      return false;
    }
    const [widgetId, activation] = inner as [string, unknown];
    if (typeof activation !== "object" || activation === null || !("ButtonClicked" in activation)) {
      return false;
    }
    const callback = this._registry[widgetId]?.on_click;
    if (callback === undefined) {
      return false;
    }
    if (!("hit" in obj)) {
      callback();
    }
    return true;
  }

  /**
   * Tries every node-callback dispatch path in turn; `true` means the event
   * was claimed -- a callback ran, or (see `_dispatchButtonClicked`) it is
   * the semantic twin of one that does -- so `_handle` must not also call
   * `onEvent`.
   */
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

    if ("Form" in obj) {
      return this._dispatchButtonClicked(obj);
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
