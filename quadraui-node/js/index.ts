// Hand-maintained facade over the native napi-rs addon this package builds
// at `../index.js`/`../index.d.ts`/`../*.node` (via `npm run build`, which
// runs `napi build --platform --release` -- see package.json's `build`
// script). Those three are generated build artifacts, not checked in; a
// consumer's import path never names the generated loader directly, and
// any future TS-only ergonomics (overloads, a narrower public type) have
// exactly one place to be added without touching the generated file.
//
// This package's published `main`/`types` entry is `dist/index.js`/
// `dist/index.d.ts` instead of this file directly: `npm pack`/`npm
// publish`'s own `prepack` hook (`package.json`'s `build:dist` script)
// compiles this file with `tsc` into `dist/index.js`, and copies
// `index.d.ts` -- this file's own hand-maintained sibling declaration,
// not a `tsc --declaration` output; see that file's own header doc for
// why -- next to it as `dist/index.d.ts`. In-repo, every direct consumer
// (`js/app/runtime.ts`, every example, every test) still `require()`s
// this file by its own `.ts` path unchanged, so it still needs to run
// standalone under Node's native TypeScript support with no compile step
// at all; the paragraph below's constraint exists for that path, not the
// packaged one.
//
// Runtime exports go through `require`/`exports` rather than `import`/
// `export`: Node's native TypeScript support determines a `.ts` file's
// module format the same way it would a `.js` file with this package's
// `"type": "commonjs"`, so top-level `import`/runtime `export` syntax here
// would be a hard parse error, not silently transpiled, when this file
// runs standalone (every in-repo consumer's own path). `export type`
// below is exempt: it is erasable (type-only, no runtime value), so it
// disappears entirely rather than needing ESM semantics.
const native: typeof import("../index.js") = require("../index.js");

// ─── Error subclasses ───────────────────────────────────────────────────
//
// napi-rs has no custom-exception-class mechanism at the FFI boundary the
// way PyO3's `create_exception!` does, so the native addon only ever
// throws a plain `Error` whose `message` is tagged with one of the names
// below (e.g. `"InvalidDescriptorError: slot 0 already holds a
// descriptor"` -- see `quadraui-node/src/lib.rs`'s module doc's
// "Descriptor decode adapter" section). The classes below give a JS/TS
// caller the same `instanceof`-based catch ergonomics `quadraui-py`'s real
// exception subclasses have: `wrapNativeErrors` replaces every writable,
// configurable method `native.QuiApp`/`QuiShellApp`/`QuiAppWaker`/
// `QuiAppActions` exposes with a wrapper that recognizes that tag on a
// caught `Error` and re-throws the matching subclass instance instead.
// `.message` keeps the original tagged text verbatim, so an
// `assert.throws(fn, /TagName/)` check written against the untyped native
// error keeps passing unchanged against the typed one.
//
// `QuiApp.runShell` is the one static method the native addon exports that
// this section does not reach: napi-rs registers it as a non-writable,
// non-configurable property, so `wrapNativeErrors` (which only ever
// replaces a writable+configurable function property) silently leaves it
// alone rather than throwing at module load. A caller that wants a typed
// subclass out of a `runShell` validation failure (zero width/height, a
// malformed `ShellConfig`) can still pass the caught error through
// `toQuadrauiError` itself.

/**
 * Base class every subclass below extends. A bare
 * `catch (err) { if (err instanceof QuadrauiError) ... }` matches any of
 * them regardless of which specific subclass was thrown -- the napi-rs
 * analog of `quadraui-py`'s own `QuadrauiError` (see that package's
 * `src/lib.rs`).
 */
class QuadrauiError extends Error {
  constructor(message: string) {
    super(message);
    // `new.target` reflects the subclass actually being constructed even
    // through this `super(message)` call, so every subclass below gets
    // its own `name` for free without repeating this constructor.
    this.name = new.target.name;
  }
}
exports.QuadrauiError = QuadrauiError;

/** A descriptor/event shape this binding's decode adapter rejected -- mirrors `quadraui-py`'s `InvalidDescriptorError`. */
class InvalidDescriptorError extends QuadrauiError {}
exports.InvalidDescriptorError = InvalidDescriptorError;

/** A fixed slot or additive-primitive kind that already holds a descriptor -- mirrors `quadraui-py`'s `SlotOccupiedError`. */
class SlotOccupiedError extends QuadrauiError {}
exports.SlotOccupiedError = SlotOccupiedError;

/** A slot id that is not one of the documented `SLOT_*` constants -- mirrors `quadraui-py`'s `UnknownSlotError`. */
class UnknownSlotError extends QuadrauiError {}
exports.UnknownSlotError = UnknownSlotError;

/** A zero width/height, or a byte buffer whose length does not match the width/height it is supposed to encode -- mirrors `quadraui-py`'s `InvalidDimensionsError`. */
class InvalidDimensionsError extends QuadrauiError {}
exports.InvalidDimensionsError = InvalidDimensionsError;

/** A `pushKeyEvent` code point that is not a valid Unicode scalar value -- mirrors `quadraui-py`'s `InvalidCharError`. */
class InvalidCharError extends QuadrauiError {}
exports.InvalidCharError = InvalidCharError;

/** A Rust panic caught at this app's `guarded` boundary -- mirrors `quadraui-py`'s `PanicError`. The app is poisoned afterwards; every later call raises {@link PoisonedError} instead. */
class PanicError extends QuadrauiError {}
exports.PanicError = PanicError;

/** Raised by every method on an app an earlier caught panic poisoned -- mirrors `quadraui-py`'s `PoisonedError`. */
class PoisonedError extends QuadrauiError {}
exports.PoisonedError = PoisonedError;

/** A failure internal to this binding rather than caused by caller input -- mirrors `quadraui-py`'s `InternalError`. */
class InternalError extends QuadrauiError {}
exports.InternalError = InternalError;

/** A `role` string `measureText` does not recognize -- mirrors `quadraui-py`'s `InvalidFontRoleError`. */
class InvalidFontRoleError extends QuadrauiError {}
exports.InvalidFontRoleError = InvalidFontRoleError;

/**
 * A method called re-entrantly from inside this same app's own registered
 * event/tick/render callback. napi-rs's own per-instance borrow tracking
 * already rejects most of these at the FFI boundary; this is this
 * binding's own explicit, defense-in-depth guard for the rest -- no
 * `quadraui-py` equivalent (PyO3's runtime borrow check already covers the
 * whole surface there).
 */
class ReentrantCallError extends QuadrauiError {}
exports.ReentrantCallError = ReentrantCallError;

/** A `QuiApp` method called while `startTui`/`startNative` is live. */
class LiveRunnerActiveError extends QuadrauiError {}
exports.LiveRunnerActiveError = LiveRunnerActiveError;

/** `step`/`run` called after a live `startTui`/`startNative` session this app once held has already ended. */
class LiveRunnerExitedError extends QuadrauiError {}
exports.LiveRunnerExitedError = LiveRunnerExitedError;

/** `startNative` called on a binary built with none of the `gtk`/`macos`/`win` features -- there is no native runner to open a window with. */
class NoNativeBackendError extends QuadrauiError {}
exports.NoNativeBackendError = NoNativeBackendError;

/** `startNative` on the `gtk` feature found no display to open a window on (no `DISPLAY`/`WAYLAND_DISPLAY`) -- mirrors `quadraui-py`'s `NoDisplayError`. */
class NoDisplayError extends QuadrauiError {}
exports.NoDisplayError = NoDisplayError;

/** `startNative` on the `gtk` feature was given an `appId` that is not a valid `GApplication` id -- mirrors `quadraui-py`'s `InvalidAppIdError`. */
class InvalidAppIdError extends QuadrauiError {}
exports.InvalidAppIdError = InvalidAppIdError;

/** A `quadraui_binding_core::services` call (`checkForUpdate`, `setLoginItem`, `setDockMenu`, `setJumpList`, ...) this backend has no implementation for at all -- distinct from {@link InternalError}, which `setTrayIcon`/`setTrayMenu`/`setDocumentEdited`/`setRepresentedFilename` raise instead for the same "no implementation" reason (their own `WindowControl`/`TrayService` seam reports it differently), and from `setCursor`, which raises neither and instead returns `false` -- mirrors `quadraui-py`'s `UnsupportedError`. */
class UnsupportedError extends QuadrauiError {}
exports.UnsupportedError = UnsupportedError;

/**
 * Every subclass above, keyed by both its own name and the tag its native
 * `Error.message` prefix carries (the two always match -- see
 * `toQuadrauiError` below). Exported as `ERROR_CLASSES` so a
 * `test_public_api.py`-style check can enumerate every subclass this
 * module promises to export without hardcoding the list a second time;
 * see `tests/public_api.test.ts`.
 */
const ERROR_CLASSES_BY_TAG: Record<string, new (message: string) => QuadrauiError> = {
  InvalidDescriptorError,
  SlotOccupiedError,
  UnknownSlotError,
  InvalidDimensionsError,
  InvalidCharError,
  PanicError,
  PoisonedError,
  InternalError,
  InvalidFontRoleError,
  ReentrantCallError,
  LiveRunnerActiveError,
  LiveRunnerExitedError,
  NoNativeBackendError,
  NoDisplayError,
  InvalidAppIdError,
  UnsupportedError,
};
exports.ERROR_CLASSES = Object.freeze({ ...ERROR_CLASSES_BY_TAG });

const ERROR_TAG_PATTERN = /^([A-Za-z]+Error): /;

/**
 * Converts a value caught from a native call into the {@link QuadrauiError}
 * subclass its message is tagged with, preserving the original message
 * and stack trace. Returns `err` unchanged if it is already a
 * `QuadrauiError`, or is not an `Error` carrying a recognized tag -- a
 * registered handler/tick/render callback can throw anything, and that
 * value must pass through unchanged rather than being coerced into a
 * `QuadrauiError` it has nothing to do with.
 */
function toQuadrauiError(err: unknown): unknown {
  if (!(err instanceof Error) || err instanceof QuadrauiError) {
    return err;
  }
  const match = ERROR_TAG_PATTERN.exec(err.message);
  const ErrorClass = match && ERROR_CLASSES_BY_TAG[match[1]];
  if (!ErrorClass) {
    return err;
  }
  const wrapped = new ErrorClass(err.message);
  wrapped.stack = err.stack;
  return wrapped;
}
exports.toQuadrauiError = toQuadrauiError;

/**
 * Replaces every own, writable, configurable function-valued property on
 * `target` (a native addon prototype, or a constructor used for its
 * static methods) with a wrapper that converts a thrown native `Error`
 * through {@link toQuadrauiError} before rethrowing. `"constructor"` is
 * always skipped (nothing to convert on a prototype's own back-reference
 * to its constructor); a property napi-rs registers as non-writable/
 * non-configurable is left alone the same way, which is what keeps
 * `QuiApp.runShell` (see this file's own "Error subclasses" section doc)
 * from needing special-case handling here.
 */
function wrapNativeErrors(target: object): void {
  for (const key of Object.getOwnPropertyNames(target)) {
    if (key === "constructor") continue;
    const descriptor = Object.getOwnPropertyDescriptor(target, key);
    if (
      !descriptor ||
      typeof descriptor.value !== "function" ||
      !descriptor.writable ||
      !descriptor.configurable
    ) {
      continue;
    }
    const original = descriptor.value as (...args: unknown[]) => unknown;
    (target as Record<string, unknown>)[key] = function (this: unknown, ...args: unknown[]) {
      try {
        return original.apply(this, args);
      } catch (err) {
        throw toQuadrauiError(err);
      }
    };
  }
}

wrapNativeErrors(native.QuiApp.prototype);
wrapNativeErrors(native.QuiShellApp.prototype);
wrapNativeErrors(native.QuiAppWaker.prototype);
wrapNativeErrors(native.QuiAppActions.prototype);

// ─── Module-level platform-services functions ──────────────────────────
//
// The three app-less seams `quadraui_binding_core::services` exposes with
// no live app needed at all -- mirrors `quadraui-py`'s own module-level
// `app_path`/`set_app_identifier`/`install_crash_handler` functions.
// `wrapNativeErrors` only reaches object prototypes, not these bare
// `native.*` functions, so each gets its own `toQuadrauiError` wrapper
// here, the same shape the `QuiApp` constructor above needs for the same
// reason.

/**
 * The standard, per-app filesystem location named by `kind` (one of
 * `"user_data"`, `"app_config"`, `"app_cache"`, `"logs"`, `"temp"`,
 * `"downloads"`, `"documents"`, `"home"`), as a path string. Every
 * app-scoped kind (everything but `"temp"`/`"downloads"`/`"documents"`/
 * `"home"`) is resolved relative to whatever identifier
 * {@link setAppIdentifier} last set, and raises {@link UnsupportedError}
 * until one has been.
 */
function appPath(kind: string): string {
  try {
    return native.appPath(kind);
  } catch (err) {
    throw toQuadrauiError(err);
  }
}
exports.appPath = appPath;

/**
 * Sets the app identifier scoping {@link appPath}'s app-scoped kinds. A
 * process-global setting, not per-`QuiApp`.
 */
function setAppIdentifier(identifier: string): void {
  try {
    native.setAppIdentifier(identifier);
  } catch (err) {
    throw toQuadrauiError(err);
  }
}
exports.setAppIdentifier = setAppIdentifier;

/**
 * Installs a crash handler that writes a crash record on an uncaught
 * panic, decoding `cfg` into a crash-reporter config. `cfg` is a plain
 * object keyed by the underlying Rust shape's own `snake_case` field
 * names -- `app_version`/`upload_endpoint`/`metadata`/`log_dir`, every
 * key optional (a camelCase `appVersion`/`uploadEndpoint`/`metadata`/
 * `logDir` key is silently ignored, not an error, since this decodes with
 * no `camelCase` remap). No live `QuiApp` is needed: this installs a
 * process-global Rust panic hook.
 */
function installCrashHandler(cfg: unknown): void {
  try {
    native.installCrashHandler(cfg);
  } catch (err) {
    throw toQuadrauiError(err);
  }
}
exports.installCrashHandler = installCrashHandler;

/**
 * A headless quadraui app, driven entirely from JavaScript/TypeScript.
 *
 * Construct it with a `width`/`height` cell grid, insert descriptors with
 * `insertButton`/`insertTextDisplay`/.../`insertExtra`, and inspect the
 * rendered frame with `screen()`/`screenContains()`. See the native
 * binding's own doc comment (`quadraui-node/src/lib.rs`'s `QuiApp`) for the
 * full method list and panic/poison behaviour.
 *
 * A thin subclass of the native constructor rather than a bare re-export:
 * every other method is already wrapped in place on
 * `native.QuiApp.prototype` (this subclass inherits that prototype
 * unchanged), but a constructor call never goes through an instance's own
 * prototype method lookup the way every other wrapped call does, so the
 * zero-width/zero-height constructor check needs its own
 * {@link toQuadrauiError} call here. `static runShell` is inherited
 * unchanged -- see this file's own "Error subclasses" section doc for why
 * it stays unwrapped.
 */
class QuiApp extends native.QuiApp {
  constructor(width: number, height: number) {
    try {
      super(width, height);
    } catch (err) {
      throw toQuadrauiError(err);
    }
  }

  /**
   * Narrows `native.QuiApp.prototype.widgetState`'s napi-generated
   * `unknown` return type to {@link WidgetState} -- the shape
   * `QuiApp::widget_state` (`quadraui-node/src/lib.rs`) actually encodes,
   * one layer up from napi's own type erasure at the native boundary. A
   * thin override, not a reimplementation: delegates straight to the
   * inherited native method, same as every other wrapped call on this
   * class's own prototype.
   */
  widgetState(id: string): WidgetState | null {
    return super.widgetState(id) as WidgetState | null;
  }
}
exports.QuiApp = QuiApp;

/**
 * The resolved widget-state shape `QuiApp.widgetState(id)` returns -- a
 * `Tree`/`List`/`DataTable`'s cursor + scroll, a `Split`'s ratio, a
 * `TabBar`'s active tab index, or a `Tree`'s expanded row paths, whichever
 * apply to the widget `id` names. `id` names exactly one widget kind in
 * practice, so only one group of fields is ever non-`null` on a given
 * result. Every field defaults to `null` rather than being omitted, so a
 * caller can destructure without an `in` check.
 */
export interface WidgetState {
  /** A focused `Tree`'s selected row path, or `null` if none/not a tree. */
  selectedPath: number[] | null;
  /** A focused `List`/`DataTable`'s selected row index, or `null`. */
  selectedIndex: number | null;
  /** A `Tree`/`List`/`DataTable`'s first visible row index, or `null`. */
  scrollOffset: number | null;
  /**
   * A `Split`'s resolved divider ratio (`0.0`-`1.0`), or `null`. Crosses
   * the native boundary as an `f32` widened to JS's own `f64` number type
   * (via `serde_json::to_value`), so an exact-looking value like `0.3`
   * can arrive as `0.30000001192092896` -- compare with an epsilon, not
   * `===`.
   */
  splitRatio: number | null;
  /** A `TabBar`'s resolved active tab index, or `null`. */
  activeTab: number | null;
  /** A `Tree`'s expanded row paths, sorted, or `null`. */
  expanded: number[][] | null;
}

/**
 * A headless `ShellApp`/`ShellConfig`-backed app: an AppShell (activity bar
 * + sidebar + optional bottom-panel tab strip) around JS-registered
 * content and event handlers. Built by `QuiApp.runShell(width, height,
 * config, ...)`, not constructed directly. See the native binding's own
 * doc comment (`quadraui-node/src/lib.rs`'s `QuiShellApp`) for the full
 * method list.
 */
export type QuiShellApp = InstanceType<typeof native.QuiShellApp>;

/**
 * A thread-safe handle, returned by `QuiApp.waker()`, that a background
 * thread calls `post`/`wake`/`postAfter` on to deliver a value into the
 * owning `QuiApp`'s event stream. See the native binding's own doc comment
 * (`quadraui-node/src/lib.rs`'s `QuiAppWaker`) for the full method list.
 */
export type QuiAppWaker = InstanceType<typeof native.QuiAppWaker>;

/**
 * A handle returned by `pump()`: stops the `setInterval` loop it started.
 * Calling `stop()` more than once, or after the loop already stopped
 * itself (because `step()` reported the app exited), is a harmless no-op --
 * `clearInterval` on an already-cleared handle does nothing.
 */
export interface PumpHandle {
  stop(): void;
}

/**
 * Drives `app.step()` repeatedly on a fixed timer, cooperatively yielding to
 * the rest of Node's event loop between calls instead of looping
 * synchronously -- the JS-side half of the non-blocking run-loop contract
 * `startTui`/`startNative`/`step` establish on the Rust side (see
 * `quadraui-node/src/lib.rs`'s module doc's "Live run loop" section).
 * `step()` itself never blocks (`startTui`/`startNative` each move this app
 * onto a non-blocking runner `step()`, and the headless path was always
 * non-blocking), so a `setInterval` cadence here never starves a
 * concurrent timer/microtask/I/O callback the way a tight synchronous loop
 * calling `step()` forever would. Works unchanged for a `startNative`
 * session -- it drives the same `step()`/`stopLive()` surface as a
 * `startTui` one.
 *
 * {@link run} is the default entry point now -- it drives the same
 * `step()`/`stopLive()` surface wake-driven instead of on a fixed interval,
 * so an idle session makes far fewer `step()` calls. `pump` stays exported
 * as the documented fallback for two cases `run` doesn't (yet) cover as
 * well: a session that needs every real native keypress/click observed at
 * the full `intervalMs` cadence regardless of idle-CPU cost (see `run`'s own
 * doc for why its `fallbackIntervalMs` default trades that latency away by
 * default), and a caller that wants `step()` called at a perfectly steady
 * cadence with no dependency on `armWakeNotify`/`disarmWakeNotify` existing
 * on `app` at all.
 *
 * Stops automatically once `step()` reports the app has exited (returns
 * `false`), or earlier if the returned handle's `stop()` is called.
 * `intervalMs` defaults to 16 (roughly 60Hz); pass a larger value to trade
 * input latency for CPU.
 *
 * If `step()` itself throws (a registered handler threw, or a callback
 * raised a pending error `step()` surfaces), this stops the interval and,
 * if `app` is a live `startTui`/`startNative` session (has a `stopLive`
 * method), calls it to tear the session down (terminal: raw mode off,
 * leave alternate screen; native: destroy the window, where the runner
 * supports it) before rethrowing -- otherwise an uncaught exception inside
 * `setInterval`'s callback would tear the process down with the session
 * left exactly as `startTui`/`startNative` last left it, since the
 * Rust-side live runner's own cleanup never runs on an abrupt process
 * exit.
 */
function pump(
  app: { step(): boolean; stopLive?(): boolean },
  intervalMs = 16,
): PumpHandle {
  const handle = setInterval(() => {
    try {
      if (!app.step()) {
        clearInterval(handle);
      }
    } catch (err) {
      clearInterval(handle);
      app.stopLive?.();
      throw err;
    }
  }, intervalMs);
  return {
    stop(): void {
      clearInterval(handle);
    },
  };
}
exports.pump = pump;

/** The type of {@link pump} -- lets a TS consumer `import type { PumpFn }` to name it, since `pump` itself is exported only at runtime (see this file's own doc comment on why this module has no runtime `export`). */
export type PumpFn = typeof pump;

/** An `app` {@link run} can drive -- `pump`'s own app shape plus the two native-side hooks `run` arms/disarms (see `quadraui-node/src/lib.rs`'s module doc's "Wake-driven stepping" section for what fires the armed callback). */
export interface RunnableApp {
  step(): boolean;
  stopLive?(): boolean;
  armWakeNotify(callback: () => void): void;
  disarmWakeNotify(): void;
}

/** A handle returned by {@link run}: stops the wake-driven loop it started. Calling `stop()` more than once, or after the loop already stopped itself (because `step()` reported the app exited), is a harmless no-op. */
export interface RunHandle {
  stop(): void;
}

/**
 * Drives `app.step()` wake-driven instead of on a fixed interval -- the
 * default entry point `startTui`/`startNative` sessions should reach for
 * instead of `pump` (see that function's own doc for when `pump` is still
 * the better choice). Arms `app`'s native `armWakeNotify` hook with a
 * callback that calls `step()` immediately, so a background thread's
 * `waker().post()`/`.wake()`/`.postAfter()`/`.wakeAfter()` is observed the
 * instant it fires rather than on `pump`'s next interval tick, and a session
 * with no pending waker/timer work in between makes zero `step()` calls at
 * all -- unlike `pump`, which calls `step()` on every tick regardless of
 * whether there is anything to do.
 *
 * That wake-notify hook only covers two of the three things a live session
 * can be waiting on -- a background thread's wake, and a scheduled
 * `postAfter`/`wakeAfter` timer -- not real native input (a keypress, a
 * click) arriving on an idle `startNative` window: no backend today exposes
 * a way to observe that without the window's own thread polling for it (see
 * `quadraui-node/src/lib.rs`'s module doc's "Wake-driven stepping" section).
 * `fallbackIntervalMs` is this function's safety net for that gap -- `step()`
 * is also called on this plain timer, same as `pump`'s own `intervalMs`,
 * just at a much coarser default cadence (250ms instead of 16ms) since most
 * of what it is covering for is genuinely rare compared to a waker/timer
 * wake. A session whose real native input latency matters more than its
 * idle CPU cost should pass a smaller `fallbackIntervalMs` (or use `pump`
 * directly) until a backend grows a real wake primitive for native input.
 *
 * Calls `step()` once immediately (synchronously, before this function
 * returns) so the first frame renders without waiting on either the wake
 * notify or the fallback timer. Stops automatically once `step()` reports
 * the app has exited, or earlier if the returned handle's `stop()` is
 * called -- either way disarms the wake-notify hook and clears the fallback
 * timer. On `step()` throwing, mirrors `pump`'s own cleanup-then-rethrow
 * contract (see that function's own doc) exactly.
 */
function run(app: RunnableApp, fallbackIntervalMs = 250): RunHandle {
  let stopped = false;
  let fallback: ReturnType<typeof setInterval> | undefined;

  const stop = (): void => {
    if (stopped) return;
    stopped = true;
    if (fallback !== undefined) {
      clearInterval(fallback);
      fallback = undefined;
    }
    app.disarmWakeNotify();
  };

  const stepOnce = (): void => {
    if (stopped) return;
    try {
      if (!app.step()) {
        stop();
      }
    } catch (err) {
      stop();
      app.stopLive?.();
      throw err;
    }
  };

  app.armWakeNotify(stepOnce);
  fallback = setInterval(stepOnce, fallbackIntervalMs);
  stepOnce();

  return { stop };
}
exports.run = run;

/** The type of {@link run} -- lets a TS consumer `import type { RunFn }` to name it, since `run` itself is exported only at runtime (see this file's own doc comment on why this module has no runtime `export`). */
export type RunFn = typeof run;

// ─── Wire types ───────────────────────────────────────────────────────────
//
// Re-exported from `./wire` (`wire.d.ts`, generated by
// `scripts/gen_wire_types.py`) so a consumer names every layout-tree/
// leaf-descriptor/`UiEvent` shape through this package's own entry point
// instead of reaching into `wire.d.ts` directly. `export type * from` is
// erasable, same as the bare `export type` lines above -- no runtime cost,
// no runtime dependency on `./wire` resolving at all.
export type * from "./wire";
