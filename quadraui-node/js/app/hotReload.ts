// Hot reload for `./runtime.ts`'s `App`: re-require edited source modules
// while the app keeps running, and keep every piece of state the user
// built up. The TypeScript counterpart of
// `quadraui-py/python/quadraui/app/hot_reload.py`, with the two
// divergences CommonJS forces, called out below.
//
// What survives a reload, and why:
//
// * **Component state.** `App` keys a component's state slot by its
//   explicit `key` or its position in the tree, never by the constructor
//   object, so the class a reload replaces finds the slot its predecessor
//   filled. On the first render after a reload each surviving slot passes
//   through the component's own `migrateState(state)` if it defines one;
//   the default keeps every existing value and adds any key the current
//   `initialState()` introduced.
// * **App-object state.** An object whose class lives in a watched module
//   keeps its identity and its own properties: list it in
//   `enableHotReload`'s `preserve` option and its prototype is re-pointed
//   at the reloaded class of the same name, so every method resolves to
//   the new code.
// * **Framework widget state.** Scroll offsets, selection, expanded rows
//   and focus live in the wrapped `QuiApp`, keyed by widget id, and are
//   never touched by a reload.
//
// What happens on a broken edit: if re-requiring throws (a syntax error,
// an exception at load time) or the first render with the new code throws,
// `App` puts the previous module exports and prototypes back, records the
// exception on `App.reloadError`, and keeps rendering the last good code;
// the next successful reload clears it.
//
// *Divergence 1: a view must call through the module or object, not a
// captured function.* Python rebinds a bound-method view to the reloaded
// class itself; a JS closure carries no reference back to the function's
// module, so there is nothing to rebind. Write
// `{view: () => notes.view()}` (a closure over a `preserve`d object, whose
// prototype swap makes `.view` the new code) or
// `{view: () => mod.view()}` (a property read at call time, which sees
// the re-required exports) -- never `{view: mod.view}`, which captures the
// old function object forever.
//
// *Divergence 2: watched modules are resolved paths, not module objects.*
// `enableHotReload(["/abs/path/view.ts"])`, normally written
// `enableHotReload([require.resolve("./view.ts")])` so the caller's own
// module resolution applies rather than this file's.
//
// What does not survive: top-level code of a reloaded module is re-run
// (that is what re-requiring means), and new properties an edited
// constructor would set are absent on an object that already exists --
// read them defensively or set them in an `onReload` hook.
//
// Change detection is a stat of each watched file (modification time and
// size). `App.reloadIfChanged()` does one check synchronously;
// `App.enableHotReload(..., {interval})` additionally starts an unref'd
// timer that does the same check every `interval` milliseconds and, on a
// change, wakes the app's event loop through `QuiApp.waker()`, so the
// reload itself always runs from the app's own event stream.
//
// Runtime exports go through `exports.X = ...` rather than `export`, the
// same reason `../index.ts`'s own header comment gives.

const fs = require("node:fs");

/**
 * Payload the watcher timer posts through `QuiAppWaker.post`; `App` claims
 * a `{User: HOT_RELOAD_WAKE}` event itself instead of passing it to
 * `onEvent`.
 */
const HOT_RELOAD_WAKE = "quadraui.app.hotReload";
exports.HOT_RELOAD_WAKE = HOT_RELOAD_WAKE;

export type ModuleExports = Record<string, unknown>;

/** One watched module's exports as they were before a reload, for `restoreModules`. */
export type SavedModules = Array<{ path: string; exports: ModuleExports; snapshot: ModuleExports }>;

/** One `preserve`d object and the prototype it had before a reload. */
export type ReboundPrototypes = Array<{ instance: object; prototype: object }>;

/** The instance types of the two classes below, nameable from `./runtime.ts`. */
export type Watcher = ModuleWatcher;
export type Poller = PollTimer;

/**
 * Turns the caller's watch list into resolved absolute paths, rejecting
 * anything a reload cannot re-require from a file on disk.
 */
function resolveModules(modules: ReadonlyArray<string>): string[] {
  return modules.map((spec) => {
    if (typeof spec !== "string") {
      throw new TypeError(`not a module path: ${JSON.stringify(spec)} (pass require.resolve("./view.ts"))`);
    }
    let resolved: string;
    try {
      resolved = require.resolve(spec);
    } catch {
      throw new Error(
        `cannot hot-reload ${JSON.stringify(spec)}: it does not resolve to a file from this module -- ` +
          'pass an absolute path, normally require.resolve("./view.ts") from the module that owns it',
      );
    }
    if (!fs.existsSync(resolved)) {
      throw new Error(`cannot hot-reload ${JSON.stringify(spec)}: ${resolved} is not a file on disk`);
    }
    return resolved;
  });
}
exports.resolveModules = resolveModules;

type Stamp = string | null;

function stampOf(path: string): Stamp {
  try {
    const stat = fs.statSync(path, { bigint: true });
    return `${stat.mtimeNs}:${stat.size}`;
  } catch {
    return null;
  }
}

/** Remembers each watched file's stamp and reports which ones changed since the stamps were last taken. */
class ModuleWatcher {
  paths: string[];
  private stamps = new Map<string, Stamp>();

  constructor(paths: ReadonlyArray<string>) {
    this.paths = [...paths];
    this.snapshot();
  }

  /** Records every watched file's current stamp as the baseline. */
  snapshot(): void {
    for (const path of this.paths) {
      this.stamps.set(path, stampOf(path));
    }
  }

  /**
   * The watched files whose stamp differs from the baseline, in watch
   * order. A file that is missing right now (an editor's write-by-rename
   * in progress) does not count as changed.
   */
  changed(): string[] {
    return this.paths.filter((path) => {
      const stamp = stampOf(path);
      return stamp !== null && stamp !== this.stamps.get(path);
    });
  }
}
exports.ModuleWatcher = ModuleWatcher;

/**
 * An unref'd timer calling `onChange()` whenever `watcher.changed()` is
 * non-empty, at most once per distinct change (it waits until the app has
 * taken a new snapshot before reporting again). Unref'd so a watching app
 * never keeps the process alive on its own.
 */
class PollTimer {
  private timer: ReturnType<typeof setInterval>;
  private reported: string | null = null;

  constructor(watcher: ModuleWatcher, intervalMs: number, onChange: () => void) {
    this.timer = setInterval(() => {
      const changed = watcher.changed();
      if (changed.length === 0) {
        this.reported = null;
        return;
      }
      const key = changed.map((path) => `${path}@${stampOf(path)}`).join("|");
      if (key !== this.reported) {
        this.reported = key;
        onChange();
      }
    }, intervalMs);
    this.timer.unref?.();
  }

  stop(): void {
    clearInterval(this.timer);
  }
}
exports.PollTimer = PollTimer;

/**
 * Re-requires every watched path in order, returning each one's exports as
 * they were (for `restoreModules`). Each module's *original* exports object
 * is mutated in place and put back in `require.cache`, so a caller that
 * captured the namespace with `const mod = require("./view.ts")` sees the
 * new code through the object it already holds -- the same in-place
 * semantics Python's `importlib.reload` has. Throws whatever the first
 * failing re-require threw, after restoring every module it touched.
 */
function reloadModules(paths: ReadonlyArray<string>): SavedModules {
  const saved: SavedModules = [];
  try {
    for (const path of paths) {
      const previous = require.cache[path];
      const exportsObject: ModuleExports = (previous?.exports as ModuleExports) ?? {};
      saved.push({ path, exports: exportsObject, snapshot: { ...exportsObject } });
      delete require.cache[path];
      let fresh: ModuleExports;
      try {
        fresh = require(path) as ModuleExports;
      } finally {
        if (previous !== undefined) {
          require.cache[path] = previous;
        }
      }
      for (const key of Object.keys(exportsObject)) {
        delete exportsObject[key];
      }
      Object.assign(exportsObject, fresh);
    }
  } catch (error) {
    restoreModules(saved);
    throw error;
  }
  return saved;
}
exports.reloadModules = reloadModules;

/** Puts each module's exports back to a `reloadModules` snapshot. */
function restoreModules(saved: SavedModules): void {
  for (const entry of saved) {
    for (const key of Object.keys(entry.exports)) {
      delete entry.exports[key];
    }
    Object.assign(entry.exports, entry.snapshot);
  }
}
exports.restoreModules = restoreModules;

/**
 * Re-points each `instance`'s prototype at the reloaded class of the same
 * name exported from one of `paths`, recording what it was so a failed
 * reload can put it back. An instance whose class is not exported from a
 * watched module is left alone.
 */
function repointPrototypes(
  instances: ReadonlyArray<object>,
  paths: ReadonlyArray<string>,
  rebound: ReboundPrototypes,
): void {
  for (const instance of instances) {
    const name = instance?.constructor?.name;
    if (typeof name !== "string" || name === "") {
      continue;
    }
    const replacement = lookupClass(paths, name);
    if (replacement === null || replacement.prototype === Object.getPrototypeOf(instance)) {
      continue;
    }
    rebound.push({ instance, prototype: Object.getPrototypeOf(instance) });
    Object.setPrototypeOf(instance, replacement.prototype);
  }
}
exports.repointPrototypes = repointPrototypes;

function lookupClass(paths: ReadonlyArray<string>, name: string): { prototype: object } | null {
  for (const path of paths) {
    const moduleExports = require.cache[path]?.exports as ModuleExports | undefined;
    const candidate = moduleExports?.[name];
    if (typeof candidate === "function" && candidate.prototype !== undefined && candidate.name === name) {
      return candidate as unknown as { prototype: object };
    }
  }
  return null;
}

/** Undoes `repointPrototypes`. */
function restorePrototypes(rebound: ReboundPrototypes): void {
  for (const entry of rebound) {
    Object.setPrototypeOf(entry.instance, entry.prototype);
  }
}
exports.restorePrototypes = restorePrototypes;
