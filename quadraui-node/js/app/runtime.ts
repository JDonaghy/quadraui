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
// callback-name table, `FORM_FIELD_EVENT_MAP` for the `Form`-field/
// standalone-leaf equivalent (looked up by the *payload's* own id rather
// than the event's outer one -- see its own doc for why), and `Button`'s
// own doc (`./nodes.ts`) for why its `onClick` needs its own two-path
// dispatch instead of a table entry.
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
import type { QuiAppActions, RenderCtx, WidgetState, RunHandle } from "../index";
import type { Poller, ReboundPrototypes, Watcher } from "./hotReload";

const { QuiApp, run, getCurrentCtx } = require("../index.ts");
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

/**
 * Mirrors `quadraui_binding_core::wire_schema::WIRE_VERSION` -- every tree
 * this layer renders carries it as a `"wire_version"` sibling key (see
 * `_renderOnce`), so the host's own version gate
 * (`quadraui_binding_core::wire::check_wire_version`) applies to a
 * typed-layer tree exactly the way it already does to a hand-built wire
 * object passed to `QuiApp.setRender` directly. This constant has no
 * runtime link to the Rust one (the Node package can't read a Rust crate
 * constant at import time), so it needs a matching bump by hand if that
 * constant ever changes.
 *
 * Read back through `exports.WIRE_VERSION` (not the module-local binding)
 * at stamping time below, so a test can force a mismatch by reassigning
 * `runtime.WIRE_VERSION` on the already-`require`d module -- the same
 * trick `quadraui-py`'s own test suite plays on its module-level
 * `WIRE_VERSION`, which Node's `const` would otherwise block.
 */
exports.WIRE_VERSION = 1;

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
    if (value.extraCallbacks !== undefined) {
      for (const [extraId, callbacks] of Object.entries(value.extraCallbacks(nodeId))) {
        if (Object.keys(callbacks).length > 0) {
          registry[extraId] = callbacks;
        }
      }
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
  List: {
    SelectionChanged: ["on_select", (p) => [p.idx]],
    ItemDoubleClicked: ["on_activate", (p) => [p.idx]],
    ItemActivated: ["on_activate", (p) => [p.idx]],
  },
  TabBar: {
    TabClosed: ["on_close", (p) => [p.index]],
    TabActivated: ["on_activate", (p) => [p.index]],
  },
  StatusBar: {
    SegmentClicked: ["on_segment_click", (p) => [p.id]],
  },
};

/**
 * Maps a `FormEvent` field-change variant (the `{"Form": [widgetId,
 * {<variant>: <payload>}]}` activation's own key) to `[callbackName,
 * extractArgs]` -- the shape every typed field builder's own
 * `onChange`/`onSubmit` keyword documents, and the one every standalone
 * leaf that reuses this event shape (`Checkbox`, `Slider`, `Select`,
 * `Password`, `TextArea` -- see `quadraui_binding_core::extra_paint`'s own
 * doc) shares with it.
 *
 * Unlike {@link CONTAINER_EVENT_MAP}, which is keyed by the event's own
 * wrapper id (one widget per event), `_dispatchFormField` looks this up
 * by the *payload's* own `id` -- a `Form`-embedded field's own id, never
 * the wrapping `Form`'s (the engine's `UiEvent::Form`'s outer id is the
 * `Form`'s; `FormEvent`'s own `id` is the field's). A standalone leaf's
 * payload id and its event's outer id are the same value, so this lookup
 * is correct for both shapes without needing to know which one it's
 * looking at. `ButtonClicked` is not here -- `_dispatchButtonClicked`
 * claims it first, with its own click-vs-activation split.
 */
const FORM_FIELD_EVENT_MAP: Record<string, [string, (payload: Record<string, unknown>) => unknown[]]> = {
  ToggleChanged: ["on_change", (p) => [p.value]],
  TextInputChanged: ["on_change", (p) => [p.value]],
  TextInputCommitted: ["on_submit", (p) => [p.value]],
  SliderChanged: ["on_change", (p) => [p.value]],
  DropdownChanged: ["on_change", (p) => [p.selected_idx]],
};

/**
 * One button `App.confirm`/`App.alert` offers -- the same shape
 * `showMessageDialog`'s own `buttons` argument takes. Kept local rather
 * than imported from `../wire`: that generated module has no standalone
 * name for this shape, only `DialogButtonDescriptor`'s `tint`/`spec`
 * extras this layer's own two helpers don't expose.
 */
export interface DialogButton {
  id: string;
  label: string;
  is_default?: boolean;
  is_cancel?: boolean;
}

/** `App.confirm`/`App.alert`'s own `severity` option -- an optional tint on the dialog chrome; has no effect on which button resolves the returned promise. */
export type DialogSeverity = "info" | "question" | "warning" | "error";

export interface AppOptions {
  /**
   * Builds this frame's tree. `Renderable` permits `null`/`undefined` for
   * a child slot (see that type's own doc), but returning either one
   * here at the top level still throws `InvalidDescriptorError` -- the
   * native `QuiApp.setRender` callback this wraps requires a plain
   * object, the same gate `quadraui-py`'s own `render` callback has
   * always had. Return `{}` (or a `Node`/`Component` tree) for an empty
   * frame instead.
   *
   * `ctx` is this frame's {@link RenderCtx} -- real text measurement
   * (`measureText`/`lineHeight`/`charWidth`), cell-grid-vs-pixel surface
   * facts (`cellGrid`/`scale`), and read-only theme/style token lookup by
   * name (`themeToken`/`styleToken`), answered from the live backend
   * painting this exact frame. A `view` declared with no parameters (every
   * one written before this existed) keeps working unchanged -- JS simply
   * ignores an argument a function never declared.
   */
  view(ctx: RenderCtx): Renderable;
  /**
   * Called for every event this layer's own dispatch doesn't claim (a
   * global key, an unrecognized shape, ...) -- the escape hatch a
   * keyboard-driven app (arrow keys moving a cursor with no single widget
   * "owning" them) still needs; see {@link keyName} for pulling a key name
   * out of whatever shape it arrived in.
   */
  onEvent?(event: unknown): void;
  /**
   * Turns an unrecognised optional wire key on `view()`'s returned tree
   * (a misspelt prop, a typo'd slot) into a raised `InvalidDescriptorError`
   * out of the render that found it, instead of the default one-time
   * {@link onUnknownKey} report -- see the wrapped `QuiApp.setStrict`'s own
   * doc. Omit (or pass `false`) to keep the warn-and-continue default.
   */
  strict?: boolean;
  /**
   * Called as `onUnknownKey({key, path})` at most once per distinct `key`
   * for this app's whole lifetime, whenever `view()`'s returned tree
   * carries an unrecognised optional key and {@link strict} hasn't turned
   * that into a raised error instead -- see the wrapped
   * `QuiApp.setOnUnknownKey`'s own doc.
   */
  onUnknownKey?(info: { key: string; path: string }): void;
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
  /**
   * A `QuiAppActions` handle, shared with every render/event/tick callback
   * -- the sanctioned way to open a dialog/toast/context menu/... from
   * inside one instead of calling the matching `qui` method directly (that
   * would raise `ReentrantCallError` mid-dispatch); see `../index.ts`'s own
   * facade doc for the full restricted-method list. `confirm`/`alert` queue
   * through this rather than `qui` for exactly that reason.
   */
  actions: QuiAppActions;
  /**
   * The id -> {eventName: callback} registry the most recent render built
   * -- leading-underscore, not actually private: an own test (or a REPL
   * poking at id-stability/dispatch) reads it directly the same way
   * `quadraui.app`'s own Python test suite reads `App._registry`.
   */
  _registry: Registry = {};
  _componentStates: ComponentStates = new Map();
  _focusedId: string | null = null;
  /**
   * The resolver for a pending `confirm`/`alert` promise, waiting on
   * `UiEvent::MessageDialogResolved` -- `null` when none is outstanding. A
   * dialog opened some other way (a raw `actions.showMessageDialog`/
   * `qui.showMessageDialog` call `confirm`/`alert` never wrapped a promise
   * around) resolves nothing here; its event falls through `_dispatch` to
   * `onEvent` like any other unclaimed one.
   */
  private _pendingDialogResolve: ((id: string) => void) | null = null;
  /** The exception a failed reload recorded, or `null` after a successful one. */
  reloadError: unknown = null;
  /** How many reloads have succeeded and rendered. */
  reloadCount = 0;
  private _view: (ctx: RenderCtx) => Renderable;
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
    if (options.strict !== undefined) {
      this.qui.setStrict(options.strict);
    }
    if (options.onUnknownKey !== undefined) {
      const onUnknownKey = options.onUnknownKey;
      this.qui.setOnUnknownKey((info: unknown) => onUnknownKey(info as { key: string; path: string }));
    }
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

  // --- dialogs --------------------------------------------------------------

  /**
   * Opens an in-canvas confirm/alert/prompt dialog and resolves with
   * whichever button's `id` the user picked -- a click, or Enter/Escape
   * activating the button marked `is_default`/`is_cancel` respectively.
   * Queues through `actions` rather than calling `qui.showMessageDialog`
   * directly, so this is safe to call from inside a node callback (a
   * `Button`'s `onClick`, say) as well as from top-level script code --
   * see {@link actions}'s own doc for why.
   *
   * Calling `confirm`/`alert` again while an earlier call's dialog is
   * still open replaces it silently, the same "replaces any dialog
   * already open" behaviour `showMessageDialog` itself documents: the
   * earlier call's promise is then abandoned, never resolved or
   * rejected. Only one `confirm`/`alert` call should be outstanding at a
   * time.
   */
  confirm(
    title: string,
    body: string,
    buttons: ReadonlyArray<DialogButton>,
    severity?: DialogSeverity,
  ): Promise<string> {
    return new Promise<string>((resolve) => {
      this._pendingDialogResolve = resolve;
      this.actions.showMessageDialog(title, body, buttons as unknown[], severity);
    });
  }

  /**
   * `confirm` with a single button (`"ok"` unless `options.id`
   * overrides it), both `is_default` and `is_cancel` -- Enter, Escape,
   * and a click on it all resolve the same returned promise. `options.label`
   * overrides its text (`"OK"` by default).
   */
  alert(
    title: string,
    body: string,
    options: { id?: string; label?: string; severity?: DialogSeverity } = {},
  ): Promise<string> {
    const id = options.id ?? "ok";
    return this.confirm(
      title,
      body,
      [{ id, label: options.label ?? "OK", is_default: true, is_cancel: true }],
      options.severity,
    );
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
      // `getCurrentCtx()` only answers non-`null` while this `setRender`
      // callback's own native call is on the stack -- which it is here,
      // since `_renderOnce` is reached synchronously from
      // `qui.setRender(() => this._render())`, so the cast below is safe
      // -- see `RenderCtx`'s own doc (`../index.ts`) for the staleness
      // contract past that point.
      const ctx = getCurrentCtx() as RenderCtx;
      wire = walk(this._view(ctx), [], registry, this._componentStates, newStates, migrate);
    } catch (error) {
      this._migrateNextRender = migrate;
      throw error;
    }
    this._registry = registry;
    this._componentStates = newStates;
    if (typeof wire === "object" && wire !== null && !Array.isArray(wire) && !("wire_version" in (wire as object))) {
      wire = { ...(wire as Record<string, unknown>), wire_version: exports.WIRE_VERSION };
    }
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
   * The non-`ButtonClicked` half of a `{"Form": [id, activation]}` event:
   * a field (or standalone-leaf) value change, routed to whichever
   * `onChange`/`onSubmit` that field's own id was registered under -- see
   * {@link FORM_FIELD_EVENT_MAP}'s own doc for why the lookup key is the
   * *payload's* `id`, not this event's outer one. Tried only after
   * `_dispatchButtonClicked` has already ruled out `ButtonClicked`.
   */
  private _dispatchFormField(obj: Record<string, unknown>): boolean {
    const inner = obj.Form;
    if (!Array.isArray(inner) || inner.length !== 2) {
      return false;
    }
    const [, activation] = inner as [string, unknown];
    if (typeof activation !== "object" || activation === null) {
      return false;
    }
    const activationObj = activation as Record<string, unknown>;
    for (const [variantName, [callbackName, extractArgs]] of Object.entries(FORM_FIELD_EVENT_MAP)) {
      const payload = activationObj[variantName] as Record<string, unknown> | undefined;
      if (payload === undefined) {
        continue;
      }
      const fieldId = payload.id as string | undefined;
      const callback = fieldId !== undefined ? this._registry[fieldId]?.[callbackName] : undefined;
      if (callback === undefined) {
        return false;
      }
      callback(...extractArgs(payload));
      return true;
    }
    return false;
  }

  /**
   * Resolves a pending `confirm`/`alert` promise with the clicked button's
   * id. `false` (so the event falls through to `onEvent` like any other
   * unclaimed one, per {@link _pendingDialogResolve}'s own doc) when no
   * `confirm`/`alert` call is waiting on this dialog.
   */
  private _dispatchMessageDialogResolved(obj: Record<string, unknown>): boolean {
    const resolve = this._pendingDialogResolve;
    if (resolve === null) {
      return false;
    }
    this._pendingDialogResolve = null;
    resolve(obj.MessageDialogResolved as string);
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
      if (this._dispatchButtonClicked(obj)) {
        return true;
      }
      return this._dispatchFormField(obj);
    }

    if ("MessageDialogResolved" in obj) {
      return this._dispatchMessageDialogResolved(obj);
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
